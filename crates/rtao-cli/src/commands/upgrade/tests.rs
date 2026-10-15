//! Metadata-backed verification, approval selection and pre-key refusal fixtures.

use super::*;
use crate::{
    application,
    books::Books,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao::{
    client::{ClientConfig, Network},
    primitives::TaoBalance,
    signer::{BoxFuture, DynKeySource, KeypairSigner, Signer, SignerError},
};
use rtao_runtime::{
    CallBytes, RuntimeHandle,
    models::{MultisigInfo, MultisigTimepoint},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CI: AccountId = AccountId([29; 32]);
const OTHER: AccountId = AccountId([31; 32]);
const THIRD: AccountId = AccountId([33; 32]);
const HEX_URL: &str = "https://artifacts.invalid/proxy_proxy_blob.hex";

struct NoTerminal;
impl Terminal for NoTerminal {
    fn stdin_is_tty(&self) -> bool {
        false
    }
    fn stderr_is_tty(&self) -> bool {
        false
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn parse(args: &[&str]) -> Result<ArgMatches, Box<dyn std::error::Error>> {
    let parsed = application::command_tree([command()]).try_get_matches_from(
        std::iter::once("rtao")
            .chain(std::iter::once("upgrade"))
            .chain(args.iter().copied()),
    )?;
    Ok(parsed)
}
fn context(
    parsed: &ArgMatches,
    signer: AccountId,
) -> Result<(AppContext, ArgMatches, MemorySink), Box<dyn std::error::Error>> {
    let family = parsed.subcommand().ok_or("upgrade family")?.1;
    let leaf = family.subcommand().ok_or("upgrade leaf")?.1;
    let config = Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/upgrade-fixture-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &reserved(family)?, |_| None)?;
    globals.signer_backend = Some("ledger".into());
    globals.signer_address = Some(signer.to_string());
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, family.clone(), sink))
}
fn runtime() -> Result<RuntimeHandle, Box<dyn std::error::Error>> {
    Ok(RuntimeHandle::parse(
        include_bytes!("../../../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?)
}
fn when() -> MultisigTimepoint {
    MultisigTimepoint {
        height: 70,
        index: 3,
    }
}
fn pending(approvals: Vec<AccountId>) -> MultisigInfo {
    MultisigInfo {
        when: when(),
        deposit: TaoBalance(100),
        depositor: CI,
        approvals,
    }
}
fn hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}
fn seed_pending(
    state: &mut FakeState,
    address: AccountId,
    call_hash: [u8; 32],
    pending: &MultisigInfo,
) -> TestResult {
    state.seed_python("Multisig","Multisigs",&[json!(address),json!(hash(call_hash))],json!({"when":pending.when,"deposit":pending.deposit.0,"depositor":pending.depositor,"approvals":pending.approvals}))?;
    Ok(())
}
fn proposal_state(
    signer: AccountId,
    threshold: u16,
    approvals: Option<Vec<AccountId>>,
) -> Result<(FakeState, CallBytes, CallBytes, AccountId), Box<dyn std::error::Error>> {
    let runtime = runtime()?;
    let sudo = Multisig::new(&runtime, &[signer, OTHER, THIRD], threshold)?.address;
    let deploy = Multisig::new(&runtime, &[CI, sudo], 2)?.address;
    let blob = chain::reconstruct(&runtime, b"\0asm\x01\0\0\0", sudo)?;
    let record = json!({"ci_address":CI,"timepoint":when()});
    let finalizing = chain::finalizing(&runtime, blob.as_bytes(), &record)?;
    let mut state = FakeState::try_default()?;
    state.seed_python("Sudo", "Key", &[], json!(sudo))?;
    state.seed_python(
        "Proxy",
        "Proxies",
        &[json!(sudo)],
        json!([[{"delegate":deploy,"proxy_type":"SudoUncheckedSetCode","delay":0}],0]),
    )?;
    seed_pending(&mut state, deploy, blob.hash(), &pending(vec![CI]))?;
    if let Some(approvals) = approvals {
        seed_pending(&mut state, sudo, finalizing.hash(), &pending(approvals))?;
    }
    Ok((state, blob, finalizing, sudo))
}
async fn make_client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(chain.clone()), config).await?,
        chain,
    ))
}
#[derive(Default)]
struct Fetch {
    responses: BTreeMap<String, Vec<u8>>,
    requests: Mutex<Vec<(String, bool)>>,
}
impl Fetch {
    fn blob(call: &CallBytes) -> Self {
        Self {
            responses: BTreeMap::from([(
                HEX_URL.into(),
                format!("0x{}\n", hex::encode(call.as_bytes())).into_bytes(),
            )]),
            ..Self::default()
        }
    }
}
impl UpgradeFetch for Fetch {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>> {
        Box::pin(async move {
            self.requests
                .lock()
                .map_err(|_| failure("requests poisoned"))?
                .push((url.into(), api));
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| failure("unavailable artifact"))
        })
    }
}
struct Keys {
    key: KeypairSigner,
    unlocks: AtomicUsize,
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.key.account().0)
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
struct Factory {
    keys: Arc<Keys>,
    sources: AtomicUsize,
    releases: AtomicUsize,
}
impl Factory {
    fn new() -> Result<Self, SignerError> {
        Ok(Self {
            keys: Arc::new(Keys {
                key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
                unlocks: AtomicUsize::new(0),
            }),
            sources: AtomicUsize::new(0),
            releases: AtomicUsize::new(0),
        })
    }
    fn signer(&self) -> AccountId {
        self.keys.key.account()
    }
}
impl RawSignerFactory for Factory {
    fn source<'a>(
        &'a self,
        _: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            assert_eq!(selection.address, Some(self.signer()));
            self.sources.fetch_add(1, Ordering::SeqCst);
            Ok(self.keys.clone() as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a SigningSelection,
        _: &'a dyn DynKeySource,
        _: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
fn docs(sink: &MemorySink) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    Ok(
        serde_json::Deserializer::from_str(&sink.text(Stream::Stdout)?)
            .into_iter::<Value>()
            .collect::<Result<Vec<_>, _>>()?,
    )
}

#[test]
fn source_validation_and_signer_set_options_match_the_python_surface() -> TestResult {
    for args in [
        &["check"][..],
        &["sign", "--url", "A/B@v1", "--hex-url", HEX_URL][..],
    ] {
        let parsed = parse(args)?;
        let family = parsed.subcommand().ok_or("family")?.1;
        assert!(validate_before_connect(family).is_err());
    }
    for args in [
        &["pending", "--repo", "A/B"][..],
        &["check", "--url", "A/B@v1"][..],
        &[
            "sign",
            "--hex-file",
            "proposal.hex",
            "--wasm",
            "runtime.wasm",
            "--multisig-threshold",
            "2",
            "--signatories",
            "a,b",
            "--yes",
        ][..],
        &["check", "--url", "", "--hex-url", HEX_URL][..],
    ] {
        let parsed = parse(args)?;
        let family = parsed.subcommand().ok_or("family")?.1;
        validate_before_connect(family)?;
    }
    for bad in ["../repo", "A/B/C", "A/", "A/B?per_page=999"] {
        let parsed = parse(&["pending", "--repo", bad])?;
        assert!(validate_before_connect(parsed.subcommand().ok_or("family")?.1).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn check_reconstructs_exact_live_metadata_and_preserves_python_machine_record() -> TestResult
{
    let factory = Factory::new()?;
    let (state, blob, _, sudo) = proposal_state(factory.signer(), 2, None)?;
    let (client, chain) = make_client(state).await?;
    let parsed = parse(&["check", "--hex-url", HEX_URL, "--json"])?;
    let (ctx, group, sink) = context(&parsed, factory.signer())?;
    execute_with(&ctx, &client, &group, &Fetch::blob(&blob), &factory).await?;
    let docs = docs(&sink)?;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["ok"], true);
    assert_eq!(docs[0]["data"]["sudo_key"], json!(sudo));
    assert_eq!(docs[0]["data"]["call_hash"], hash(blob.hash()));
    let checks = docs[0]["checks"].as_array().ok_or("checks")?;
    assert_eq!(
        checks
            .iter()
            .map(|check| check["name"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        [
            "call structure",
            "re-encoding",
            "runtime match",
            "sudo key",
            "on-chain proposal"
        ]
    );
    assert!(checks[2]["ok"].is_null());
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn malformed_or_substituted_call_and_wrong_sudo_refuse_before_private_source() -> TestResult {
    let factory = Factory::new()?;
    for wrong_sudo in [false, true] {
        let (mut state, blob, _, _) = proposal_state(factory.signer(), 2, None)?;
        let fetch = if wrong_sudo {
            state.seed_python("Sudo", "Key", &[], json!(AccountId([97; 32])))?;
            Fetch::blob(&blob)
        } else {
            let mut data = blob.as_bytes().to_vec();
            data.push(0);
            Fetch {
                responses: BTreeMap::from([(HEX_URL.into(), hex::encode(data).into_bytes())]),
                ..Fetch::default()
            }
        };
        let (client, chain) = make_client(state).await?;
        let parsed = parse(&["sign", "--hex-url", HEX_URL, "--json", "--yes"])?;
        let (ctx, group, sink) = context(&parsed, factory.signer())?;
        let error = execute_with(&ctx, &client, &group, &fetch, &factory)
            .await
            .err()
            .ok_or("refusal")?;
        assert!(error.message.starts_with("verification failed:"));
        assert_eq!(docs(&sink)?[0]["ok"], false);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
    }
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn all_signing_positions_and_dry_runs_do_not_acquire_a_private_source() -> TestResult {
    let factory = Factory::new()?;
    let refs = format!("{},{},{}", factory.signer(), OTHER, THIRD);
    for (approvals, expected) in [
        (None, "first"),
        (Some(vec![OTHER]), "interior"),
        (Some(vec![OTHER, THIRD]), "final"),
        (Some(vec![factory.signer()]), "signed"),
    ] {
        let (state, blob, _, _) = proposal_state(factory.signer(), 3, approvals)?;
        let (client, chain) = make_client(state).await?;
        let parsed = parse(&[
            "sign",
            "--hex-url",
            HEX_URL,
            "--multisig-threshold",
            "3",
            "--signatories",
            &refs,
            "--json",
            "--dry-run",
        ])?;
        let (ctx, group, sink) = context(&parsed, factory.signer())?;
        execute_with(&ctx, &client, &group, &Fetch::blob(&blob), &factory).await?;
        let docs = docs(&sink)?;
        assert_eq!(docs[0]["ok"], true);
        if expected != "signed" {
            assert_eq!(docs[1]["position"], expected);
        } else {
            assert_eq!(docs.len(), 1);
        }
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
    }
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    assert_eq!(factory.keys.unlocks.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn confirmation_refusal_precedes_source_and_unlock() -> TestResult {
    let factory = Factory::new()?;
    let refs = format!("{},{},{}", factory.signer(), OTHER, THIRD);
    let (state, blob, _, _) = proposal_state(factory.signer(), 2, None)?;
    let (client, _) = make_client(state).await?;
    let parsed = parse(&[
        "sign",
        "--hex-url",
        HEX_URL,
        "--multisig-threshold",
        "2",
        "--signatories",
        &refs,
        "--json",
    ])?;
    let (ctx, group, _) = context(&parsed, factory.signer())?;
    let error = execute_with(&ctx, &client, &group, &Fetch::blob(&blob), &factory)
        .await
        .err()
        .ok_or("confirmation")?;
    assert_eq!(error.message, "refusing to submit without confirmation");
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    assert_eq!(factory.keys.unlocks.load(Ordering::SeqCst), 0);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn first_interior_and_final_submissions_use_correct_hash_or_full_call_and_padded_weight()
-> TestResult {
    for approvals in [None, Some(vec![OTHER]), Some(vec![OTHER, THIRD])] {
        let factory = Factory::new()?;
        let refs = format!("{},{},{}", factory.signer(), OTHER, THIRD);
        let final_approval = approvals
            .as_ref()
            .is_some_and(|approvals| approvals.len() == 2);
        let interior = approvals.is_some();
        let (mut state, blob, finalizing, _) = proposal_state(factory.signer(), 3, approvals)?;
        state.weight = json!({"ref_time":90_000_000_000u64,"proof_size":60_000});
        let (client, chain) = make_client(state).await?;
        let parsed = parse(&[
            "sign",
            "--hex-url",
            HEX_URL,
            "--multisig-threshold",
            "3",
            "--signatories",
            &refs,
            "--json",
            "--yes",
        ])?;
        let (ctx, group, _) = context(&parsed, factory.signer())?;
        execute_with(&ctx, &client, &group, &Fetch::blob(&blob), &factory).await?;
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), 1);
        let decoded = runtime()?.core().decode_extrinsic(&submissions[0], true)?;
        let call = decoded.field("call").ok_or("extrinsic call")?;
        assert_eq!(
            call.field("call_function").and_then(ValueExt::as_str),
            Some(if final_approval {
                "as_multi"
            } else {
                "approve_as_multi"
            })
        );
        let args = call.field("call_args").ok_or("call args")?;
        let weight = args.field("max_weight").ok_or("max weight")?;
        assert_eq!(
            weight.field("ref_time").and_then(ValueExt::as_u128),
            Some(99_000_000_000)
        );
        assert_eq!(
            weight.field("proof_size").and_then(ValueExt::as_u128),
            Some(66_000)
        );
        assert_eq!(
            args.field("maybe_timepoint")
                .is_some_and(|value| !matches!(value, rtao::core::codec::Value::Null)),
            interior
        );
        if !final_approval {
            assert_eq!(
                args.field("call_hash")
                    .and_then(ValueExt::value_bytes)
                    .as_deref(),
                Some(finalizing.hash().as_slice())
            );
        }
        assert_eq!(factory.sources.load(Ordering::SeqCst), 1);
        assert_eq!(factory.releases.load(Ordering::SeqCst), 1);
        assert_eq!(factory.keys.unlocks.load(Ordering::SeqCst), 1);
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn post_review_sudo_change_and_reopened_operation_refuse_before_source() -> TestResult {
    let factory = Factory::new()?;
    let signer = factory.signer();
    let (state, blob, finalizing, sudo) = proposal_state(signer, 3, None)?;
    let (client, chain) = make_client(state).await?;
    let parsed = parse(&["sign", "--hex-url", HEX_URL, "--json", "--yes"])?;
    let (ctx, _, _) = context(&parsed, signer)?;
    let bundle = artifacts::fetch_proposal_bundle(
        &BundleInputs {
            release_url: None,
            hex_url: Some(HEX_URL.into()),
            hex_file: None,
            wasm_path: None,
        },
        &Fetch::blob(&blob),
    )
    .await?;
    let group = RawMultisig {
        threshold: 3,
        signatories: vec![signer, OTHER, THIRD],
        refs: vec![],
        preset: None,
    };
    let selection = SigningSelection {
        wallet: "public".into(),
        backend: "ledger".into(),
        address: Some(signer),
    };
    chain.mutate_state(|state| {
        let _ = state.seed_python("Sudo", "Key", &[], json!(OTHER));
    });
    let error = submit_verified(
        &ctx,
        &client,
        &bundle,
        &group,
        &selection,
        signer,
        sudo,
        finalizing.hash(),
        None,
        None,
        &factory,
    )
    .await
    .err()
    .ok_or("changed sudo")?;
    assert!(
        error.message.contains("verification failed") || error.message.contains("sudo key changed")
    );
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    client.close().await;
    let (state, blob, finalizing, sudo) = proposal_state(signer, 3, Some(vec![OTHER]))?;
    let (client, chain) = make_client(state).await?;
    let bundle = artifacts::fetch_proposal_bundle(
        &BundleInputs {
            release_url: None,
            hex_url: Some(HEX_URL.into()),
            hex_file: None,
            wasm_path: None,
        },
        &Fetch::blob(&blob),
    )
    .await?;
    let mut reopened = pending(vec![OTHER]);
    reopened.when.height = 71;
    chain.mutate_state(|state| {
        let _ = seed_pending(state, sudo, finalizing.hash(), &reopened);
    });
    let error = submit_verified(
        &ctx,
        &client,
        &bundle,
        &group,
        &selection,
        signer,
        sudo,
        finalizing.hash(),
        Some(&when()),
        None,
        &factory,
    )
    .await
    .err()
    .ok_or("reopened")?;
    assert!(error.message.contains("sudo proposal opening changed"));
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    client.close().await;
    Ok(())
}

#[test]
fn mixed_signer_sources_refuse_and_threshold_one_review_is_executing() -> TestResult {
    let factory = Factory::new()?;
    for args in [
        &[
            "sign",
            "--hex-url",
            HEX_URL,
            "--multisig",
            "team",
            "--multisig-threshold",
            "2",
        ][..],
        &[
            "sign",
            "--hex-url",
            HEX_URL,
            "--multisig",
            "team",
            "--signatories",
            "alice,bob",
        ][..],
        &["sign", "--hex-url", HEX_URL, "--multisig-threshold", "2"][..],
    ] {
        let parsed = parse(args)?;
        let (ctx, group, _) = context(&parsed, factory.signer())?;
        let leaf = group.subcommand().ok_or("leaf")?.1;
        assert!(resolve_group(&ctx, leaf, None).is_err());
    }
    let (position, question) = signing_position(None, 1, factory.signer(), &Map::new());
    assert_eq!(position, "final");
    assert!(question.contains("upgrade executes in this extrinsic"));
    Ok(())
}

#[tokio::test]
async fn threshold_one_dispatches_full_finalizing_call_with_native_threshold_one_wrapper()
-> TestResult {
    let factory = Factory::new()?;
    let refs = format!("{},{},{}", factory.signer(), OTHER, THIRD);
    let (state, blob, _, _) = proposal_state(factory.signer(), 1, None)?;
    let (client, chain) = make_client(state).await?;
    let parsed = parse(&[
        "sign",
        "--hex-url",
        HEX_URL,
        "--multisig-threshold",
        "1",
        "--signatories",
        &refs,
        "--json",
        "--yes",
    ])?;
    let (ctx, group, _) = context(&parsed, factory.signer())?;
    execute_with(&ctx, &client, &group, &Fetch::blob(&blob), &factory).await?;
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let decoded = runtime()?.core().decode_extrinsic(&submissions[0], true)?;
    let call = decoded.field("call").ok_or("call")?;
    assert_eq!(
        call.field("call_function").and_then(ValueExt::as_str),
        Some("as_multi_threshold_1")
    );
    let inner = call
        .field("call_args")
        .and_then(|args| args.field("call"))
        .ok_or("inner call")?;
    assert_eq!(
        inner.field("call_module").and_then(ValueExt::as_str),
        Some("Multisig")
    );
    assert_eq!(
        inner.field("call_function").and_then(ValueExt::as_str),
        Some("as_multi")
    );
    assert_eq!(factory.sources.load(Ordering::SeqCst), 1);
    assert_eq!(factory.releases.load(Ordering::SeqCst), 1);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn release_wasm_digest_and_manifest_crosschecks_are_all_required() -> TestResult {
    let factory = Factory::new()?;
    let (state, blob, _, _) = proposal_state(factory.signer(), 2, None)?;
    let (client, _) = make_client(state).await?;
    let snapshot = client.at(BlockRef::Best).await?;
    let code = b"\0asm\x01\0\0\0";
    for mismatch in [None, Some("wasm"), Some("digest"), Some("hash"), Some("ci")] {
        let bundle = ProposalBundle {
            blob: blob.as_bytes().to_vec(),
            manifest: Some(
                json!({"call_hash":if mismatch==Some("hash"){hash([9;32])}else{hash(blob.hash())},"ci_address":if mismatch==Some("ci"){OTHER}else{CI},"spec_version":999,"repo":"A/B","tag":"v999","commit":"source","wasm_sha256":artifacts::sha256_hex(code)}),
            ),
            wasm: Some(if mismatch == Some("wasm") {
                b"wrong".to_vec()
            } else {
                code.to_vec()
            }),
            digest: Some(
                json!({"sha256":if mismatch==Some("digest"){String::new()}else{artifacts::sha256_hex(code)}}),
            ),
            wasm_source: Some("release"),
            notes: vec![],
        };
        let outcome = chain::verify(&snapshot, &bundle).await?;
        assert_eq!(outcome.ok(), mismatch.is_none());
        assert_eq!(
            outcome
                .checks
                .iter()
                .map(|check| check.name)
                .collect::<Vec<_>>(),
            [
                "call structure",
                "re-encoding",
                "runtime match",
                "srtool digest",
                "sudo key",
                "on-chain proposal",
                "manifest call hash",
                "manifest CI key"
            ]
        );
        assert_eq!(
            outcome.data["code_sha256"],
            format!("0x{}", artifacts::sha256_hex(code))
        );
        if let Some(mismatch) = mismatch {
            let expected = match mismatch {
                "wasm" => "runtime match",
                "digest" => "srtool digest",
                "hash" => "manifest call hash",
                _ => "manifest CI key",
            };
            assert_eq!(
                outcome
                    .checks
                    .iter()
                    .filter(|check| check.ok == Some(false))
                    .map(|check| check.name)
                    .collect::<Vec<_>>(),
                [expected]
            );
        }
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pending_uses_configured_repository_and_retains_discovery_when_enrichment_is_unavailable()
-> TestResult {
    let factory = Factory::new()?;
    let (state, blob, _, sudo) = proposal_state(factory.signer(), 2, None)?;
    let (client, chain) = make_client(state).await?;
    let parsed = parse(&["pending", "--repo", "Example/custom", "--json"])?;
    let (ctx, group, sink) = context(&parsed, factory.signer())?;
    let fetch = Fetch::default();
    execute_with(&ctx, &client, &group, &fetch, &factory).await?;
    let docs = docs(&sink)?;
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["count"], 1);
    assert_eq!(
        docs[0]["pending_upgrades"][0]["call_hash"],
        hash(blob.hash())
    );
    assert_eq!(docs[0]["pending_upgrades"][0]["sudo_key"], json!(sudo));
    assert_eq!(
        *fetch.requests.lock().map_err(|_| "requests")?,
        [(
            "https://api.github.com/repos/Example/custom/releases?per_page=15".into(),
            true
        )]
    );
    assert_eq!(factory.sources.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
