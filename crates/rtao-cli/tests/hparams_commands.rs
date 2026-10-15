// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/sudo.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Curated owner commands preserve real SDK arguments and lossless read output.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::NetUid,
};
use rtao_cli::{
    application,
    books::Books,
    commands::hparams::{self, Request},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn sequential_null_pruning_flags_bound_root_target_waits_and_batches() -> TestResult {
    let parsed = hparams::trim_null_command().try_get_matches_from([
        "trim-null",
        "--netuid",
        "3",
        "--target",
        "128",
        "--switch-to-yuma",
        "--wait-timeout",
        "1.5",
        "--poll-seconds",
        "1.25",
        "--max-batches",
        "4",
        "--state-file",
        "pruning-recovery.json",
        "--period",
        "16",
        "--yes",
    ])?;
    assert_eq!(parsed.get_one::<u16>("netuid"), Some(&3));
    assert_eq!(parsed.get_one::<u16>("target"), Some(&128));
    assert_eq!(parsed.get_one::<u32>("max_batches"), Some(&4));
    assert_eq!(parsed.get_one::<u32>("__period"), Some(&16));
    assert!(parsed.get_flag("switch_to_yuma"));
    for (flag, value) in [
        ("--netuid", "0"),
        ("--netuid", "4096"),
        ("--target", "0"),
        ("--wait-timeout", "NaN"),
        ("--wait-timeout", "86401"),
        ("--poll-seconds", "0"),
        ("--poll-seconds", "61"),
        ("--max-batches", "0"),
        ("--max-batches", "4097"),
    ] {
        let args = if flag == "--netuid" {
            vec!["trim-null", flag, value]
        } else {
            vec!["trim-null", "--netuid", "3", flag, value]
        };
        assert!(
            hparams::trim_null_command()
                .try_get_matches_from(args)
                .is_err(),
            "{flag}={value}"
        );
    }
    Ok(())
}

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

fn parse(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(application::command_tree([hparams::command()?]).try_get_matches_from(args)?)
}

fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("hparams")?.1;
    let mut leaf = group;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/hparams-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &hparams::reserved(group)?, |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink))
}

async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
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

#[test]
fn owner_spellings_select_actual_intents_and_keep_exact_protocol_values() -> TestResult {
    for (name, input, expected) in [
        ("max_difficulty", "18446744073709551615", json!(u64::MAX)),
        ("registration_allowed", "false", json!(0)),
        ("epoch_consensus", "Null", json!("Null")),
    ] {
        let parsed = parse(&[
            "rtao", "su", "set", "--netuid", "1", "--name", name, "--value", input, "--yes",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        let Request::Transaction(request) = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?
        else {
            return Err("expected transaction".into());
        };
        assert_eq!(request.op, "set_hyperparameter");
        assert_eq!(request.args["value"], json!(input));
        assert_eq!(request.intent()?.to_json()["value"], expected);
    }
    Ok(())
}

#[test]
fn identity_python_flags_map_to_native_fields_and_default_other_fields_to_empty() -> TestResult {
    let parsed = parse(&[
        "rtao",
        "hparams",
        "set_identity",
        "--netuid",
        "2",
        "--name",
        "τ two",
        "--url",
        "https://example.test",
        "--description",
        "example",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Transaction(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected transaction".into());
    };
    let intent = request.intent()?.to_json();
    assert_eq!(intent["op"], "set_subnet_identity");
    assert_eq!(intent["subnet_name"], "τ two");
    assert_eq!(intent["subnet_url"], "https://example.test");
    assert_eq!(intent["description"], "example");
    assert_eq!(intent["github_repo"], "");
    assert_eq!(intent["additional"], "");
    Ok(())
}

#[test]
fn count_without_value_stays_a_read_and_explicit_zero_is_not_omission() -> TestResult {
    let parsed = parse(&[
        "rtao", "hparams", "mech", "count", "--netuid", "1", "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Read(query) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected read".into());
    };
    assert_eq!(query.name, rtao::reads::ReadName::MechanismCount);
    let parsed = parse(&[
        "rtao",
        "hparams",
        "mechanisms",
        "count",
        "--netuid",
        "1",
        "--value",
        "0",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    // Zero is supplied as a transaction, never silently changed into the read
    // branch. The chain retains responsibility for semantic count bounds.
    let Request::Transaction(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("explicit value became a read".into());
    };
    assert_eq!(request.op, "set_mechanism_count");
    assert_eq!(request.args["mechanism_count"], 0);
    Ok(())
}

#[tokio::test]
async fn count_and_emissions_use_the_real_read_dispatch_without_wallet_or_signer() -> TestResult {
    for (command, storage, value, expected) in [
        (
            "count",
            "MechanismCountCurrent",
            json!(3),
            json!({"netuid": 2, "mechanism_count": 3}),
        ),
        (
            "emissions",
            "MechanismEmissionSplit",
            json!([8, 2, 6]),
            json!({"netuid": 2, "split": [8, 2, 6]}),
        ),
    ] {
        let parsed = parse(&[
            "rtao",
            "hparams",
            "mechanisms",
            command,
            "--netuid",
            "2",
            "--json",
        ])?;
        let (mut ctx, sink) = context(&parsed)?;
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        let mut state = FakeState::try_default()?;
        state.seed_python("SubtensorModule", storage, &[json!(2)], value)?;
        let (client, chain) = client(state).await?;
        let result = hparams::execute(
            &mut ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await?;
        assert!(result.is_none());
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            expected
        );
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn parameter_listing_keeps_runtime_values_and_augments_missing_fixed128_storage() -> TestResult
{
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_hyperparams_v3",
        json!([
            {"name":"rho","value":{"U16":7}},
            {"name":"future_name","value":{"U128":u128::MAX}}
        ]),
    )?;
    let item = rtao_catalog::hyperparams::TABLE
        .storage
        .iter()
        .find(|item| item.parameter == "burn_increase_mult")
        .ok_or("fixed128 storage")?;
    state.seed_python(
        item.container,
        item.name,
        &[json!(1)],
        json!({"bits": (1u128 << 64) + 1}),
    )?;
    let (client, chain) = client(state).await?;
    let params = hparams::fetch_hyperparameters(&client, NetUid(1))
        .await?
        .ok_or("parameters")?;
    assert_eq!(params["rho"], 7);
    assert_eq!(params["burn_increase_mult"], json!((1u128 << 64) + 1));
    assert_eq!(params["future_name"], json!(u128::MAX));
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    hparams::render_hyperparameters(&output, NetUid(1), &Some(params.clone()), None)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        Value::Object(params)
    );
    let state_hashes = chain
        .requests()
        .into_iter()
        .filter_map(|request| match request.method.as_str() {
            "state_getStorage" | "state_queryStorageAt" => request.params.get(1).cloned(),
            "state_call" => request.params.get(2).cloned(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!state_hashes.is_empty());
    assert!(state_hashes.iter().all(|hash| hash == &state_hashes[0]));
    client.close().await;
    Ok(())
}

#[test]
fn single_parameter_machine_record_is_exact_and_missing_names_refuse() -> TestResult {
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let params = Some(serde_json::Map::from_iter([(
        "max_difficulty".into(),
        json!(u64::MAX),
    )]));
    hparams::render_hyperparameters(&output, NetUid(1), &params, Some("max_difficulty"))?;
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(payload["value"], json!(u64::MAX));
    assert_eq!(payload["name"], "max_difficulty");
    assert!(payload["normalized"].is_null());
    assert_eq!(payload["owner_settable"], true);
    assert_eq!(
        hparams::render_hyperparameters(&output, NetUid(1), &params, Some("missing"))
            .err()
            .ok_or("missing refusal")?
            .code,
        2
    );
    assert_eq!(
        hparams::render_hyperparameters(&output, NetUid(1), &None, None)
            .err()
            .ok_or("missing subnet refusal")?
            .code,
        1
    );
    Ok(())
}

mod null_pruning {
    use super::*;
    use rtao::{
        primitives::{AccountId, CryptoType, H256},
        signer::{
            BoxFuture, DynKeySource, KeyView, KeypairSigner, MultiSignature, Signer, SignerError,
            SignerRole, SigningRequest,
        },
        tx::ExtrinsicResult,
    };
    use rtao_cli::{context::ExternalSignerFactory, error::CliError};
    use rtao_testkit::SubmitScript;
    use std::{
        collections::VecDeque,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU16, AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const NET: u16 = 3;
    const TARGET: u16 = 128;

    fn machine_records(sink: &MemorySink) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        // Each output record is pretty-printed and may span several lines.
        let text = sink.text(Stream::Stdout)?;
        Ok(serde_json::Deserializer::from_str(&text)
            .into_iter::<Value>()
            .collect::<Result<Vec<_>, _>>()?)
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let root = std::env::var_os("RTAO_TEST_TMPDIR")
                .map(PathBuf::from)
                .ok_or("set RTAO_TEST_TMPDIR to the recorded test root")?;
            let path = root.join(format!(
                "cli-null-pruning-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
        fn journal(&self) -> PathBuf {
            self.0.join("recovery.json")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Clone, Copy)]
    enum Step {
        Batch,
        MissingProgress,
        Unknown,
        Switch,
    }

    #[derive(Clone)]
    struct JournalSigner {
        key: KeypairSigner,
        owner: AccountId,
        chain: FakeChain,
        journal: PathBuf,
        population: Arc<AtomicU16>,
        steps: Arc<Mutex<VecDeque<Step>>>,
        calls: Arc<Mutex<Vec<Vec<u8>>>>,
        saved: Option<Arc<SavedPruning>>,
    }
    fn success() -> Value {
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
    }
    fn seed_target(
        state: &mut FakeState,
        target: Option<u16>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(target) = target {
            state.seed_python(
                "SubtensorModule",
                "NullPruningTarget",
                &[json!(NET)],
                json!(target),
            )?;
        } else {
            let info = state
                .runtime
                .storage_entry("SubtensorModule", "NullPruningTarget")
                .ok_or("pruning target storage")?;
            let key = state
                .runtime
                .storage_key(info, &[rtao::core::codec::Value::Uint(u128::from(NET))])?;
            state.seed_encoded(key, None);
        }
        Ok(())
    }
    impl Signer for JournalSigner {
        fn account(&self) -> AccountId {
            self.key.account()
        }
        fn crypto_type(&self) -> CryptoType {
            self.key.crypto_type()
        }
        fn sign<'a>(
            &'a self,
            request: &'a SigningRequest<'a>,
        ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
            Box::pin(async move {
                let record: Value =
                    serde_json::from_slice(&fs::read(&self.journal).map_err(|_| {
                        SignerError::KeyView("journal must precede signing".into())
                    })?)
                    .map_err(|_| SignerError::KeyView("invalid journal".into()))?;
                assert_eq!(record["owner"], json!(self.owner));
                assert_eq!(record["target"], TARGET);
                assert_eq!(
                    record["call_hex"],
                    json!(format!("0x{}", hex::encode(request.call)))
                );
                self.calls
                    .lock()
                    .map_err(|_| SignerError::KeyView("call lock".into()))?
                    .push(request.call.to_vec());
                if let Some(saved) = &self.saved {
                    return saved.sign(&self.key, request).await;
                }
                let step = self
                    .steps
                    .lock()
                    .map_err(|_| SignerError::KeyView("step lock".into()))?
                    .pop_front()
                    .unwrap_or(Step::Batch);
                self.chain.mutate_state(|state| -> Result<(), String> {
                    if matches!(step, Step::Unknown) { state.queue_submit(SubmitScript::Dropped); return Ok(()) }
                    let mut events = vec![success()];
                    if self.owner != self.key.account() {
                            events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Proxy","event_id":"ProxyExecuted","attributes":{"result":{"Ok":[]}},"topics":[]}));
                    }
                    if matches!(step, Step::Switch) {
                        state.seed_python("SubtensorModule", "SubnetEpochConsensus", &[json!(NET)], json!("Yuma")).map_err(|error| error.to_string())?;
                    } else {
                        let before = self.population.load(Ordering::SeqCst);
                        let removed = before.saturating_sub(TARGET).min(64);
                        let after = before - removed;
                        self.population.store(after, Ordering::SeqCst);
                        state.seed_python("SubtensorModule", "SubnetworkN", &[json!(NET)], json!(after)).map_err(|error| error.to_string())?;
                        seed_target(state, (after > TARGET).then_some(TARGET)).map_err(|error| error.to_string())?;
                        if !matches!(step, Step::MissingProgress) {
                            events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NullUidsPruningProgress","attributes":{"netuid":NET,"target":TARGET,"remaining":after,"removed":removed,"cleared_commits":0},"topics":[]}));
                        }
                    }
                    state.queue_submit(SubmitScript::InBlockThenFinalized {events:json!(events)});
                    Ok(())
                }).map_err(SignerError::KeyView)?;
                self.key.sign(request).await
            })
        }
    }

    struct Keys {
        signer: JournalSigner,
        unlocks: Arc<AtomicUsize>,
    }
    impl KeyView for Keys {
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.signer.key.account())
        }
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.signer.key.account())
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(self.signer.key.account().0)
        }
    }
    impl DynKeySource for Keys {
        fn signer_for<'a>(
            &'a self,
            _: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
            Box::pin(async move {
                assert!(
                    self.signer.journal.is_file(),
                    "durable reviewed call before private signer acquisition"
                );
                self.unlocks.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
            })
        }
    }
    struct Factory {
        keys: Arc<Keys>,
        fail_release: std::sync::atomic::AtomicBool,
    }
    impl ExternalSignerFactory for Factory {
        fn keys<'a>(
            &'a self,
            _: &'a Globals,
            _: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
            Box::pin(async move { Ok(self.keys.clone() as Arc<dyn DynKeySource>) })
        }
        fn release<'a>(
            &'a self,
            _: &'a Globals,
            _: &'a dyn DynKeySource,
            _: bool,
        ) -> BoxFuture<'a, Result<(), CliError>> {
            Box::pin(async move {
                if self.fail_release.load(Ordering::SeqCst) {
                    Err(CliError::failure("external session release failed"))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct SavedPruning {
        chain: FakeChain,
        journal: PathBuf,
        group: rtao::tx::Multisig,
        inner: rtao_runtime::CallBytes,
        population: Arc<AtomicU16>,
        sink: MemorySink,
        signed: AtomicUsize,
        opening: Mutex<Option<Value>>,
        source_records: Mutex<Vec<Value>>,
    }

    #[allow(clippy::expect_used)]
    impl SavedPruning {
        async fn sign(
            &self,
            key: &KeypairSigner,
            request: &SigningRequest<'_>,
        ) -> Result<MultiSignature, SignerError> {
            let index = self.signed.fetch_add(1, Ordering::SeqCst);
            assert!(index < 2, "one opening approval and one execution only");
            let runtime = self.chain.state().lock().expect("state").runtime.clone();
            let decoded = rtao::core::DynValue::new(
                runtime
                    .decode_spec(&rtao::core::runtime::TypeSpec::Call, request.call, true)
                    .map_err(|error| SignerError::KeyView(error.to_string()))?,
            )
            .to_json();
            let args = decoded["call_args"].as_array().expect("as_multi arguments");
            let arg = |name| {
                &args
                    .iter()
                    .find(|arg| arg["name"] == name)
                    .expect("as_multi argument")["value"]
            };
            assert_eq!(decoded["call_module"], "Multisig");
            assert_eq!(decoded["call_function"], "as_multi");
            assert_eq!(
                arg("call"),
                &rtao::core::DynValue::new(
                    runtime
                        .decode_spec(
                            &rtao::core::runtime::TypeSpec::Call,
                            self.inner.as_bytes(),
                            true,
                        )
                        .map_err(|error| SignerError::KeyView(error.to_string()))?,
                )
                .to_json()
            );
            if index == 0 {
                assert!(arg("maybe_timepoint").is_null());
            } else {
                assert_eq!(
                    arg("maybe_timepoint"),
                    self.opening
                        .lock()
                        .expect("opening")
                        .as_ref()
                        .expect("opening")
                );
                assert_eq!(self.population.load(Ordering::SeqCst), 192);
                assert!(
                    machine_records(&self.sink)
                        .map_err(|error| SignerError::KeyView(error.to_string()))?
                        .iter()
                        .all(
                            |value| value["status"] != "finalized" && value["status"] != "complete"
                        ),
                    "an approval alone cannot count as a pruning batch"
                );
            }
            self.chain
                .mutate_state(|state| -> Result<(), String> {
                    let hash = format!("0x{}", hex::encode(self.inner.hash()));
                    let mut events = vec![success()];
                    if index == 0 {
                        let opening = json!({"height":state.block + 1,"index":0});
                        *self.opening.lock().expect("opening") = Some(opening.clone());
                        state.seed_python("Multisig", "Multisigs", &[json!(self.group.address), json!(hash)], json!({
                            "when":opening,"deposit":0,"depositor":key.account(),"approvals":[key.account()],
                        })).map_err(|error| error.to_string())?;
                        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"NewMultisig","attributes":{"approving":key.account(),"multisig":self.group.address,"call_hash":hash},"topics":[]}));
                    } else {
                        let info = state.runtime.storage_entry("Multisig", "Multisigs").ok_or("multisig storage")?;
                        let storage_key = state.runtime.storage_key(info, &[
                            rtao::core::codec::Value::Bytes(self.group.address.0.to_vec()),
                            rtao::core::codec::Value::Bytes(self.inner.hash().to_vec()),
                        ]).map_err(|error| error.to_string())?;
                        state.seed_encoded(storage_key, None);
                        self.population.store(TARGET, Ordering::SeqCst);
                        state.seed_python("SubtensorModule", "SubnetworkN", &[json!(NET)], json!(TARGET)).map_err(|error| error.to_string())?;
                        seed_target(state, None).map_err(|error| error.to_string())?;
                        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{"approving":key.account(),"timepoint":self.opening.lock().expect("opening").clone().expect("opening"),"multisig":self.group.address,"call_hash":hash,"result":{"Ok":[]}},"topics":[]}));
                        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NullUidsPruningProgress","attributes":{"netuid":NET,"target":TARGET,"remaining":TARGET,"removed":64,"cleared_commits":0},"topics":[]}));
                    }
                    state.queue_submit(SubmitScript::InBlockThenFinalized {events:json!(events)});
                    Ok(())
                })
                .map_err(SignerError::KeyView)?;
            key.sign(request).await
        }
    }

    struct SavedFactory {
        keys: Vec<Arc<Keys>>,
        shared: Arc<SavedPruning>,
    }
    #[allow(clippy::expect_used)]
    impl ExternalSignerFactory for SavedFactory {
        fn keys<'a>(
            &'a self,
            globals: &'a Globals,
            _: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
            Box::pin(async move {
                let record: Value = serde_json::from_slice(
                    &fs::read(&self.shared.journal).expect("journal before external source"),
                )
                .expect("valid journal before external source");
                let mut observed = self.shared.source_records.lock().expect("source records");
                let index = observed.len();
                assert_eq!(
                    record["attempts"].as_array().expect("attempts").len(),
                    index + 1
                );
                assert_eq!(record["owner"], json!(self.shared.group.address));
                assert_eq!(record["approval_confirmed"], false);
                assert_eq!(
                    record["multisig"]["call_data"],
                    format!("0x{}", hex::encode(self.shared.inner.as_bytes()))
                );
                if index > 0 {
                    let opening = self
                        .shared
                        .opening
                        .lock()
                        .expect("opening")
                        .clone()
                        .expect("opening");
                    assert_eq!(record["pending_timepoint"], opening);
                    assert_eq!(record["attempts"][0]["receipt"]["inclusion"], "Finalized");
                    assert_eq!(record["attempts"][0]["receipt"]["success"], true);
                    assert_eq!(
                        record["attempts"][index]["reviewed_args"]["timepoint"],
                        opening
                    );
                }
                observed.push(record);
                let keys = self
                    .keys
                    .iter()
                    .find(|keys| {
                        globals.signer_address.as_deref()
                            == Some(keys.signer.key.account().to_string().as_str())
                    })
                    .ok_or_else(|| CliError::failure("unknown saved member"))?;
                Ok(keys.clone() as Arc<dyn DynKeySource>)
            })
        }
    }

    struct Fixture {
        _temp: Temp,
        journal: PathBuf,
        client: Client,
        chain: FakeChain,
        ctx: AppContext,
        sink: MemorySink,
        request: Request,
        factory: Factory,
    }
    async fn fixture(
        population: u16,
        steps: Vec<Step>,
        proxy: bool,
        preview: bool,
        switch: bool,
    ) -> Result<Fixture, Box<dyn std::error::Error>> {
        let temp = Temp::new()?;
        let journal = temp.journal();
        let key = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?;
        let owner = if proxy {
            AccountId([17; 32])
        } else {
            key.account()
        };
        let path = journal.to_string_lossy().into_owned();
        let mut args = vec![
            "rtao",
            "hparams",
            "trim-null",
            "--netuid",
            "3",
            "--target",
            "128",
            "--state-file",
            &path,
            "--wait-timeout",
            "2",
            "--poll-seconds",
            "1",
            "--yes",
            "--no-mev-shield",
            "--json",
        ];
        if preview {
            args.push("--dry-run");
        }
        if switch {
            args.push("--switch-to-yuma");
        }
        let parsed = parse(&args)?;
        let (mut ctx, sink) = context(&parsed)?;
        ctx.globals.signer_backend = Some("extension".into());
        ctx.globals.signer_address = Some(key.account().to_string());
        if proxy {
            ctx.globals.proxy_for = Some(owner.to_string());
        }
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        let mut state = FakeState::try_default()?;
        for (item, keys, value) in [
            ("NetworksAdded", vec![json!(NET)], json!(true)),
            ("SubnetOwner", vec![json!(NET)], json!(owner)),
            ("SubnetworkN", vec![json!(NET)], json!(population)),
            ("MechanismCountCurrent", vec![json!(NET)], json!(1)),
            ("MinAllowedUids", vec![json!(NET)], json!(64)),
            ("MaxAllowedUids", vec![json!(NET)], json!(2000)),
            ("SubnetEpochConsensus", vec![json!(NET)], json!("Null")),
            ("Tempo", vec![json!(NET)], json!(360)),
            ("LastEpochBlock", vec![json!(NET)], json!(100)),
            ("PendingEpochAt", vec![json!(NET)], json!(0)),
            ("AdminFreezeWindow", vec![], json!(10)),
        ] {
            state.seed_python("SubtensorModule", item, &keys, value)?;
        }
        seed_target(&mut state, None)?;
        state.seed_python(
            "System",
            "Account",
            &[json!(key.account())],
            json!({"data":{"free":1_000_000_000_000u64}}),
        )?;
        let genesis: [u8; 32] = hex::decode(state.genesis_hash.trim_start_matches("0x"))?
            .try_into()
            .map_err(|_| "genesis width")?;
        state.seed_python("System", "BlockHash", &[json!(0)], json!(H256(genesis)))?;
        if proxy {
            state.seed_python(
                "Proxy",
                "Proxies",
                &[json!(owner)],
                json!([[{"delegate":key.account(),"proxy_type":"Any","delay":0}],0]),
            )?;
        }
        let (client, chain) = super::client(state).await?;
        let factory = Factory {
            keys: Arc::new(Keys {
                signer: JournalSigner {
                    key,
                    owner,
                    chain: chain.clone(),
                    journal: journal.clone(),
                    population: Arc::new(AtomicU16::new(population)),
                    steps: Arc::new(Mutex::new(steps.into())),
                    calls: Arc::new(Mutex::new(Vec::new())),
                    saved: None,
                },
                unlocks: Arc::new(AtomicUsize::new(0)),
            }),
            fail_release: std::sync::atomic::AtomicBool::new(false),
        };
        Ok(Fixture {
            _temp: temp,
            journal,
            client,
            chain,
            ctx,
            sink,
            request,
            factory,
        })
    }

    async fn run(f: &mut Fixture) -> Result<Option<ExtrinsicResult>, CliError> {
        hparams::execute(
            &mut f.ctx,
            &f.client,
            &f.request,
            SubmissionOptions::default(),
            Some(&f.factory),
            &mut PromptHistory::default(),
        )
        .await
    }

    #[tokio::test]
    async fn cli_null_pruning_uses_finalized_bounded_batches_and_exact_proxy_owner_journals()
    -> TestResult {
        for proxy in [false, true] {
            let mut f = fixture(260, vec![], proxy, false, false).await?;
            assert!(run(&mut f).await?.is_some());
            assert!(!f.journal.exists());
            assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 3);
            assert_eq!(
                f.chain
                    .state()
                    .lock()
                    .map_err(|_| "state")?
                    .submissions
                    .len(),
                3
            );
            let payloads = machine_records(&f.sink)?;
            assert_eq!(
                payloads.last().ok_or("summary")?,
                &json!({"status":"complete","population":128,"target":128,"consensus":"Null","batches":3})
            );
            assert_eq!(
                payloads
                    .iter()
                    .filter_map(|value| value.get("removed_uids").and_then(Value::as_u64))
                    .collect::<Vec<_>>(),
                [64, 64, 4]
            );
            f.client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_pruning_saved_multisig_keeps_one_opening_and_counts_only_semantic_execution()
    -> TestResult {
        let mut f = fixture(192, vec![], false, false, false).await?;
        let members = [
            f.factory.keys.signer.key.clone(),
            KeypairSigner::from_seed(&[8; 32], CryptoType::Ed25519)?,
        ];
        let group = rtao::Executor::new(f.client.clone())
            .multisig(&members.iter().map(Signer::account).collect::<Vec<_>>(), 2)
            .await?;
        let inner =
            rtao_runtime::calls::admin_utils::sudo_trim_null_uids_batch(NetUid(NET), TARGET)
                .compose(&f.client.refresh_runtime().await?)?;
        f.chain
            .mutate_state(|state| -> Result<(), rtao::core::CoreError> {
                state.seed_python(
                    "SubtensorModule",
                    "SubnetOwner",
                    &[json!(NET)],
                    json!(group.address),
                )?;
                for member in &members {
                    state.seed_python(
                        "System",
                        "Account",
                        &[json!(member.account())],
                        json!({"data":{"free":1_000_000_000_000u64}}),
                    )?;
                }
                state.seed_python(
                    "System",
                    "Account",
                    &[json!(group.address)],
                    json!({"data":{"free":1_000_000_000_000u64}}),
                )?;
                Ok(())
            })?;
        f.ctx.globals.wallet_name = "team".into();
        f.ctx.globals.wallet_given = true;
        f.ctx.globals.signer_backend = None;
        f.ctx.globals.signer_address = None;
        f.ctx.globals.signatories = members
            .iter()
            .map(|member| format!("{}=extension", member.account()))
            .collect();
        f.ctx.books = Books::from_values(
            json!([]),
            json!([]),
            json!([{"name":"team","threshold":2,"signatories":members.iter().map(|member| member.account().to_string()).collect::<Vec<_>>()}]),
        );
        // Saved-call caching must stay inside this fixture's recorded root.
        f.ctx.config_store = Some(rtao::client::config::Store::new(
            rtao::client::config::Paths::new(&f._temp.0, |_| None),
        ));
        let population = Arc::new(AtomicU16::new(192));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(SavedPruning {
            chain: f.chain.clone(),
            journal: f.journal.clone(),
            group,
            inner,
            population: population.clone(),
            sink: f.sink.clone(),
            signed: AtomicUsize::new(0),
            opening: Mutex::new(None),
            source_records: Mutex::new(Vec::new()),
        });
        let factory = SavedFactory {
            keys: members
                .into_iter()
                .map(|key| {
                    Arc::new(Keys {
                        signer: JournalSigner {
                            key,
                            owner: shared.group.address,
                            chain: f.chain.clone(),
                            journal: f.journal.clone(),
                            population: population.clone(),
                            steps: Arc::new(Mutex::new(VecDeque::new())),
                            calls: calls.clone(),
                            saved: Some(shared.clone()),
                        },
                        unlocks: Arc::new(AtomicUsize::new(0)),
                    })
                })
                .collect(),
            shared: shared.clone(),
        };
        // Both approvals must finish inside this single AppContext workflow.
        let result = hparams::execute(
            &mut f.ctx,
            &f.client,
            &f.request,
            SubmissionOptions::default(),
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await?
        .ok_or("executed batch receipt")?;
        assert_eq!(shared.signed.load(Ordering::SeqCst), 2);
        assert_eq!(
            shared.source_records.lock().map_err(|_| "records")?.len(),
            2
        );
        assert_eq!(calls.lock().map_err(|_| "calls")?.len(), 2);
        for member in &factory.keys {
            assert_eq!(member.unlocks.load(Ordering::SeqCst), 1);
        }
        assert_eq!(
            f.chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .len(),
            2
        );
        assert_eq!(population.load(Ordering::SeqCst), TARGET);
        assert!(
            !f.journal.exists(),
            "verified execution removes the journal"
        );
        assert_eq!(result.inclusion, rtao::client::Inclusion::Finalized);
        assert!(result.success);
        assert_eq!(result.data["multisig_followup"]["status"], "executed");
        assert_eq!(f.ctx.globals.wallet_name, "team");
        assert!(f.ctx.globals.signer_address.is_none());
        let payloads = machine_records(&f.sink)?;
        let batches = payloads
            .iter()
            .filter(|payload| payload["status"] == "finalized")
            .collect::<Vec<_>>();
        assert_eq!(batches.len(), 1, "two approvals execute one pruning batch");
        assert_eq!(batches[0]["batch"], 1);
        assert_eq!(batches[0]["removed_uids"], 64);
        assert_eq!(
            payloads.last().ok_or("summary")?,
            &json!({"status":"complete","population":128,"target":128,"consensus":"Null","batches":1})
        );
        f.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_preview_never_unlocks_or_creates_a_submission_record() -> TestResult {
        let mut f = fixture(260, vec![], false, true, false).await?;
        assert!(run(&mut f).await?.is_none());
        assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(!f.journal.exists());
        assert!(
            f.chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        f.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_preview_reports_a_closed_window_without_waiting_for_it() -> TestResult {
        let mut f = fixture(260, vec![], false, true, false).await?;
        f.chain.mutate_state(|state| {
            state.seed_python("SubtensorModule", "AdminFreezeWindow", &[], json!(500))
        })?;
        // This future is a public preview and cannot broadcast. A closed window
        // must reach the SDK's preview refusal, never the workflow's poll sleep.
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), run(&mut f)).await;
        assert!(result.is_ok(), "preview entered a window wait");
        assert!(result?.is_err());
        assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(!f.journal.exists());
        assert!(
            f.chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        f.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_ambiguous_delivery_and_missing_progress_keep_recovery_and_block_restart()
    -> TestResult {
        for step in [Step::Unknown, Step::MissingProgress] {
            let mut f = fixture(260, vec![step], false, false, false).await?;
            assert!(run(&mut f).await.is_err());
            assert!(f.journal.is_file());
            assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 1);
            let attempts = f
                .chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .len();
            assert!(run(&mut f).await.is_err());
            assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 1);
            assert_eq!(
                f.chain
                    .state()
                    .lock()
                    .map_err(|_| "state")?
                    .submissions
                    .len(),
                attempts
            );
            assert!(
                !machine_records(&f.sink)?
                    .iter()
                    .any(|value| value["status"] == "complete")
            );
            f.client.close().await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_external_close_failure_keeps_known_progress_and_resumes_from_finalized_state()
    -> TestResult {
        let mut f = fixture(260, vec![], false, false, false).await?;
        f.factory.fail_release.store(true, Ordering::SeqCst);
        let error = run(&mut f).await.err().ok_or("release failure")?;
        assert!(error.message.contains("release failed"));
        assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(f.factory.keys.signer.population.load(Ordering::SeqCst), 196);
        // The recorded receipt was verified before reporting the close error.
        assert!(!f.journal.exists());
        f.factory.fail_release.store(false, Ordering::SeqCst);
        assert!(run(&mut f).await?.is_some());
        assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 3);
        assert_eq!(
            f.chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .len(),
            3
        );
        f.client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn cli_null_yuma_switch_is_opt_in_and_confirmed_in_finalized_storage() -> TestResult {
        let mut f = fixture(128, vec![Step::Switch], false, false, true).await?;
        assert!(run(&mut f).await?.is_some());
        assert_eq!(f.factory.keys.unlocks.load(Ordering::SeqCst), 1);
        assert!(!f.journal.exists());
        let payloads = machine_records(&f.sink)?;
        let last = payloads.last().ok_or("summary")?;
        assert_eq!(
            last,
            &json!({"status":"complete","population":128,"target":128,"consensus":"Yuma","batches":0})
        );
        f.client.close().await;
        Ok(())
    }
}
