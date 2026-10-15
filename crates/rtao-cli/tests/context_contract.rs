//! Shared mutation context through a real Client, SCALE metadata and executor.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rtao::{
    client::{Client, ClientConfig, HeadProvider, Network, View},
    primitives::{AccountId, CryptoType},
    signer::{
        BoxFuture, Coldkey, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole,
    },
    tx::{
        BuildCx, Built, Intent, IntentError, IntentInterface, IntentPreflight, PreflightCx, Signed,
    },
};
use rtao_cli::{
    application::command_tree,
    books::Books,
    context::{AppContext, ShieldChoice},
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct TerminalFixture {
    tty: bool,
    lines: VecDeque<String>,
    reads: Arc<AtomicUsize>,
}
impl Terminal for TerminalFixture {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.lines.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn context(
    flags: &[&str],
) -> Result<(AppContext, MemorySink, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
    let parsed = command_tree([with_globals(clap::Command::new("submit"), Tier::Tx)])
        .try_get_matches_from(
            std::iter::once("rtao")
                .chain(std::iter::once("submit"))
                .chain(flags.iter().copied()),
        )?;
    let mut globals = Globals::resolve(
        &parsed,
        &serde_json::Map::new(),
        Path::new("/isolated/home"),
        |_| None,
    )?;
    globals.apply(parsed.subcommand().ok_or("submit")?.1, &[])?;
    let reads = Arc::new(AtomicUsize::new(0));
    let terminal = TerminalFixture {
        tty: true,
        lines: VecDeque::from(["yes".into()]),
        reads: reads.clone(),
    };
    let mut ctx = AppContext::new(
        globals,
        serde_json::Map::new(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(terminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink, reads))
}
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
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
async fn fixture(free: u64) -> Result<(Client, FakeChain, Keys), Box<dyn std::error::Error>> {
    let keys = Keys {
        key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
        unlocks: Arc::new(AtomicUsize::new(0)),
    };
    let mut state = FakeState::default();
    for seed in 19u8..22 {
        state.seed_python(
            "System",
            "Account",
            &[json!(
                KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?.account()
            )],
            json!({"data":{"free":free}}),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain, keys))
}
// The real metadata/executor runs a remark under the catalog's default-shield
// interface. No staking operation or hosted oracle behavior is claimed here.
struct Preview {
    blocked: bool,
}
impl IntentInterface for Preview {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[2];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for Preview {
    fn summary(&self) -> String {
        "review context fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![8, 9]).compose(cx.runtime)?,
        ))
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        _: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            warnings: vec!["source-backed warning fixture".into()],
            blocks: if self.blocked {
                vec!["preflight hard stop".into()]
            } else {
                Vec::new()
            },
            ..Default::default()
        })
    }
}
#[tokio::test]
async fn default_shield_downgrades_before_dry_run_and_forced_shortfall_refuses() -> TestResult {
    let (client, chain, keys) = fixture(0).await?;
    let (ctx, sink, _) = context(&["--dry-run", "--json"])?;
    assert!(
        ctx.submit_with(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            Some("rtao tx add-stake")
        )
        .await?
        .is_none()
    );
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert!(
        !payload["effects"]
            .as_array()
            .ok_or("effects")?
            .iter()
            .any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("submitted MEV-shielded")))
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    let (ctx, _, reads) = context(&["--mev-shield", "--dry-run", "--json"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await
        .err()
        .ok_or("forced shortfall")?;
    assert_eq!(error.code, 2);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn preflight_blocks_before_confirmation_and_json_without_yes_never_signs() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let (ctx, sink, reads) = context(&["--no-mev-shield"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: true }, &keys, false, None)
        .await
        .err()
        .ok_or("hard stop")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("preflight hard stop"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(
        sink.text(Stream::Stderr)?
            .contains("source-backed warning fixture")
    );
    let (ctx, _, reads) = context(&["--no-mev-shield", "--json"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await
        .err()
        .ok_or("confirmation refusal")?;
    assert_eq!(error.code, 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn real_execution_unlocks_once_and_renders_one_canonical_receipt() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let (ctx, sink, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let receipt = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await?
        .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

#[derive(Default)]
struct SnapshotFacts {
    builds: AtomicUsize,
    previews: AtomicUsize,
    reviewed_call: Mutex<Option<String>>,
}
impl IntentInterface for SnapshotFacts {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op": "add_stake"})
    }
}
impl Intent for SnapshotFacts {
    fn summary(&self) -> String {
        "review one call and its facts".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let build = self.builds.fetch_add(1, Ordering::SeqCst) + 1;
        let value =
            u8::try_from(build).map_err(|_| IntentError::validation("too many fixture builds"))?;
        Ok(rtao_runtime::calls::system::remark(vec![value])
            .compose(cx.runtime)?
            .into())
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        self.previews.fetch_add(1, Ordering::SeqCst);
        let call = format!("0x{}", hex::encode(cx.call.as_bytes()));
        *self
            .reviewed_call
            .lock()
            .map_err(|_| IntentError::validation("fixture lock poisoned"))? = Some(call.clone());
        Ok(IntentPreflight {
            facts_title: rtao::tx::FactsTitle::Custom("Authenticated call facts".into()),
            facts: vec![("reviewed bytes".into(), call)],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn review_facts_share_one_build_and_preflight_with_the_reviewed_call() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let intent = SnapshotFacts::default();
    let (plan, preview) = rtao::Executor::new(client.clone())
        .plan_and_preflight_dyn(&intent, &keys, &Default::default())
        .await?;
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert_eq!(intent.previews.load(Ordering::SeqCst), 1);
    assert_eq!(
        preview.facts,
        vec![(
            "reviewed bytes".into(),
            format!("0x{}", hex::encode(plan.call().as_bytes()))
        )]
    );
    assert_eq!(preview.facts_title.as_str(), "Authenticated call facts");

    let intent = SnapshotFacts::default();
    let (ctx, sink, _) = context(&["--no-mev-shield"])?;
    *ctx.terminal.lock().map_err(|_| "terminal")? = Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["no".into()]),
        reads: Arc::new(AtomicUsize::new(0)),
    });
    assert!(
        ctx.submit_with(&client, &intent, &keys, false, None)
            .await
            .is_err()
    );
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert_eq!(intent.previews.load(Ordering::SeqCst), 1);
    let reviewed = intent
        .reviewed_call
        .lock()
        .map_err(|_| "reviewed call")?
        .clone()
        .ok_or("missing reviewed call")?;
    let rendered = sink.text(Stream::Stdout)?;
    assert!(rendered.contains("Authenticated call facts"));
    assert!(rendered.contains("reviewed bytes"));
    assert!(rendered.contains(&reviewed));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
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

struct ExpiringProxyProofKeys {
    keys: Keys,
    chain: FakeChain,
}
impl KeyView for ExpiringProxyProofKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.coldkey_address()
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.keys.hotkey_public_key()
    }
}
impl DynKeySource for ExpiringProxyProofKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            // The reviewed work is valid at height 100 and expires while the
            // delegate unlocks. A cached signer prevents a second such delay.
            self.chain
                .advance(5)
                .map_err(|error| SignerError::KeyView(error.to_string()))?;
            self.keys.signer_for(role).await
        })
    }
}

#[tokio::test]
async fn late_pow_refresh_preserves_proxy_owner_and_explicit_hotkey_after_unlock() -> TestResult {
    use rtao::primitives::NetUid;
    use rtao_cli::{commands::generated, context::SubmissionOptions};
    use rtao_intents::pow::registration_prefix;

    fn arg<'a>(call: &'a Value, name: &str) -> Option<&'a Value> {
        call["call_args"]
            .as_array()?
            .iter()
            .find(|arg| arg["name"] == name)
            .map(|arg| &arg["value"])
    }

    let keys = Keys {
        key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
        unlocks: Arc::new(AtomicUsize::new(0)),
    };
    let delegate = keys.key.account();
    let owner = AccountId([48; 32]);
    let hotkey = AccountId([47; 32]);
    assert_ne!(delegate, owner);
    assert_ne!(delegate, hotkey);
    let mut state = FakeState::new(FakeState::try_default()?.runtime);
    state.set_finalized(98)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(delegate)],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    let netuid = NetUid(1);
    for item in ["NetworksAdded", "NetworkPowRegistrationAllowed"] {
        state.seed_typed("SubtensorModule", item, (netuid,), true)?;
    }
    for (item, value) in [
        ("Difficulty", 1),
        ("MinDifficulty", 1),
        ("MaxDifficulty", u64::MAX),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    for (item, value) in [
        ("MaxAllowedUids", 16u16),
        ("MaxRegistrationsPerBlock", 1u16),
        ("RegistrationsThisBlock", 0u16),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let original_hash = client.rpc().block_hash(100).await?.ok_or("original hash")?;
    let prefix = registration_prefix(netuid, original_hash, hotkey, owner)?;
    let work = rtao::core::pow::seal(&prefix, 0)?;
    let request = generated::TxRequest {
        op: "pow_register",
        args: serde_json::Map::from_iter([
            ("netuid".into(), json!(netuid)),
            ("hotkey_ss58".into(), json!(hotkey)),
            ("work_block".into(), json!(100)),
            ("nonce".into(), json!(0)),
            ("work_hex".into(), json!(hex::encode(work))),
            ("mining_workers".into(), json!(1)),
            ("mining_timeout".into(), json!(10)),
        ]),
        period: None,
    };
    let owner_flag = owner.to_string();
    let (ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--proxy-for",
        &owner_flag,
    ])?;
    let unlocks = keys.unlocks.clone();
    let keys = ExpiringProxyProofKeys {
        keys,
        chain: chain.clone(),
    };
    let result = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await?
    .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(result.data["proxy_for"], json!(owner));
    assert_eq!(unlocks.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let runtime = client.refresh_runtime().await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    let proxy = &decoded["call"];
    assert_eq!(proxy["call_module"], json!("Proxy"));
    assert_eq!(proxy["call_function"], json!("proxy"));
    assert_eq!(arg(proxy, "real"), Some(&json!(owner)));
    let call = arg(proxy, "call").ok_or("inner call")?;
    assert_eq!(call["call_function"], json!("pow_register"));
    assert_eq!(arg(call, "hotkey"), Some(&json!(hotkey)));
    assert_eq!(arg(call, "work_block"), Some(&json!(105)));
    let nonce = arg(call, "nonce").and_then(Value::as_u64).ok_or("nonce")?;
    let hash = client.rpc().block_hash(105).await?.ok_or("fresh hash")?;
    let prefix = registration_prefix(netuid, hash, hotkey, owner)?;
    let expected = rtao::core::pow::seal(&prefix, nonce)?;
    assert_eq!(
        arg(call, "work"),
        Some(
            &rtao::core::DynValue::new(rtao::core::codec::Value::Bytes(expected.to_vec()))
                .to_json()
        )
    );
    let wrong_prefix = registration_prefix(netuid, hash, hotkey, delegate)?;
    assert_ne!(expected, rtao::core::pow::seal(&wrong_prefix, nonce)?);
    client.close().await;
    Ok(())
}
#[test]
fn only_literal_config_false_blocks_required_shield_and_off_disables_endpoints() -> TestResult {
    let (mut ctx, _, _) = context(&[])?;
    for false_like in [json!(0), json!(""), json!([]), json!({})] {
        ctx.config.insert("mev_shield".into(), false_like);
        assert_eq!(
            ctx.resolve_mev_shield(true, true, "burned_register")?,
            ShieldChoice {
                enabled: true,
                forced: true
            }
        );
    }
    ctx.config.insert("mev_shield".into(), json!(false));
    assert!(
        ctx.resolve_mev_shield(true, true, "burned_register")
            .is_err()
    );
    assert!(rtao_cli::context::endpoint_pool(" OFF ")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn transient_balance_failure_reprobes_known_zero_before_carrier_decision() -> TestResult {
    let (client, chain, keys) = fixture(0).await?;
    let (ctx, _, _) = context(&[])?;
    chain.queue_transport_error(
        "state_queryStorageAt",
        rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Timeout {
            detail: "fixture failure".into(),
        })),
    );
    let warning = ctx
        .shield_fee_shortfall(&client, keys.key.account())
        .await
        .ok_or("fallback zero balance")?;
    assert_eq!(
        warning,
        "free TAO cannot cover the MEV-shield outer carrier fee; alpha fees only apply to the unshielded call — use --no-mev-shield"
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
#[test]
fn signing_account_precedence_preserves_explicit_typo_and_member_before_config() -> TestResult {
    let (mut ctx, _, _) = context(&["--signer", "vault"])?;
    let first = AccountId([1; 32]);
    let second = AccountId([2; 32]);
    let third = AccountId([3; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"explicit","address":first},{"name":"member","address":second},{"name":"configured","address":third}]),
        json!([]),
        json!([]),
    );
    ctx.config
        .insert("signer_address".into(), json!("configured"));
    ctx.globals.signatories = vec!["member".into()];
    assert_eq!(ctx.external_signer_address()?, Some(second));
    ctx.globals.signer_address = Some("explicit".into());
    assert_eq!(ctx.external_signer_address()?, Some(first));
    ctx.globals.signer_address = Some("unknown-typo".into());
    assert_eq!(ctx.external_signer_address()?, None);
    ctx.globals.signer_address = None;
    ctx.globals.signatories.clear();
    assert_eq!(ctx.external_signer_address()?, Some(third));
    Ok(())
}
#[test]
fn confirmation_reasks_invalid_input_and_accepts_python_control_space() -> TestResult {
    let (mut ctx, _, reads) = context(&[])?;
    ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["maybe".into(), "\u{1c}YES\u{1f}".into()]),
        reads: reads.clone(),
    })));
    ctx.confirm("submit?")?;
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    Ok(())
}

struct Hooks {
    before: AtomicUsize,
    after: AtomicUsize,
    failed: AtomicUsize,
}
impl rtao_cli::context::SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.after.fetch_add(1, Ordering::SeqCst);
            if !result.success {
                self.failed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}
#[tokio::test]
async fn recovery_hooks_skip_preview_and_run_before_failed_receipt_rendering() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let hooks = Hooks {
        before: AtomicUsize::new(0),
        after: AtomicUsize::new(0),
        failed: AtomicUsize::new(0),
    };
    let (preview, _, _) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
    preview
        .submit_with_hooks(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            None,
            Some(&hooks),
        )
        .await?;
    assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 0);
    chain.mutate_state(|state|state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {events:json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}])}));
    let (ctx, sink, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let error = ctx
        .submit_with_hooks(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            None,
            Some(&hooks),
        )
        .await
        .err()
        .ok_or("failed receipt exit")?;
    assert_eq!(error.code, 1);
    assert!(error.message.is_empty());
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.failed.load(Ordering::SeqCst), 1);
    let receipt: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(receipt["success"], json!(false));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    client.close().await;
    Ok(())
}

#[test]
fn configured_proxy_truthiness_and_self_sentinel_preserve_dispatch_identity() -> TestResult {
    let (mut ctx, _, _) = context(&[])?;
    let account = AccountId([44; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"self","address":account},{"name":"True","address":account}]),
        json!([]),
        json!([]),
    );
    for empty in [json!(""), Value::Null, json!(false), json!(0)] {
        ctx.config.insert("proxy_for".into(), empty);
        assert_eq!(ctx.dispatch_proxy(None)?, None);
    }
    ctx.config.insert("proxy_for".into(), json!("self"));
    assert_eq!(ctx.dispatch_proxy(None)?, Some(account));
    assert_eq!(ctx.dispatch_proxy(Some("self"))?, None);
    assert!(ctx.dispatch_proxy(Some("")).is_err());
    ctx.config.insert("proxy_for".into(), json!(true));
    assert_eq!(ctx.dispatch_proxy(None)?, Some(account));
    Ok(())
}

#[derive(Clone)]
struct Connected(Client);
impl rtao_cli::context::ConnectionFactory for Connected {
    fn connect<'a>(
        &'a self,
        _: ClientConfig,
    ) -> BoxFuture<'a, Result<Client, rtao_cli::error::CliError>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}
struct CacheRoot(std::path::PathBuf);
impl CacheRoot {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/cli-context-tests")
            });
        let root = base.join("tmp").join(format!(
            "context-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root)?;
        Ok(Self(root))
    }
}
impl Drop for CacheRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn cached_context(
    store: &rtao::client::config::Store,
) -> Result<AppContext, Box<dyn std::error::Error>> {
    let parsed = command_tree([with_globals(clap::Command::new("read"), Tier::Read)])
        .try_get_matches_from(["rtao", "--network", "local", "read"])?;
    let terminal = TerminalFixture {
        tty: false,
        lines: VecDeque::new(),
        reads: Arc::new(AtomicUsize::new(0)),
    };
    Ok(AppContext::from_store(
        &parsed,
        parsed.subcommand().ok_or("read")?.1,
        &[],
        store,
        Path::new("/isolated/home"),
        |_| None,
        Arc::new(Mutex::new(Box::new(terminal))),
    )
    .await?)
}
#[tokio::test]
async fn fresh_name_cache_skips_rpc_and_stale_names_refresh_and_persist() -> TestResult {
    use rtao::primitives::NetUid;
    use std::collections::BTreeMap;
    let root = CacheRoot::new()?;
    let paths = rtao::client::config::Paths::new(&root.0, |_| None);
    let store = rtao::client::config::Store::with_clock(paths.clone(), || 1000.0);
    store
        .save_subnet_names(
            "local",
            BTreeMap::from([(NetUid(5), "cached".into())]).into(),
        )
        .await?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let before = chain.state().lock().map_err(|_| "chain")?.requests.len();
    let ctx = cached_context(&store).await?;
    ctx.run_with(&Connected(client.clone()), |_| async { Ok(()) })
        .await?;
    assert!(client.is_closed());
    assert_eq!(ctx.output.subnet_names()?.get(&5), Some(&"cached".into()));
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.requests.len(),
        before
    );
    let (client, chain, _) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "SubnetIdentitiesV3",
            &[json!(5)],
            json!({"subnet_name":"fresh"}),
        )
    })?;
    let stale = rtao::client::config::Store::with_clock(paths, || 200000.0);
    let ctx = cached_context(&stale).await?;
    let clone = ctx.output.clone();
    ctx.run_with(&Connected(client.clone()), |_| async { Ok(()) })
        .await?;
    assert!(client.is_closed());
    assert_eq!(clone.subnet_names()?.get(&5), Some(&"fresh".into()));
    assert_eq!(
        stale
            .load_subnet_names("local")
            .await?
            .chain_values()
            .values
            .get(&NetUid(5)),
        Some(&"fresh".into())
    );
    Ok(())
}

#[tokio::test]
async fn cosmetic_name_rpc_failure_does_not_fail_work_and_connection_is_closed() -> TestResult {
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let (ctx, _, _) = context(&[])?;
    chain.queue_transport_error(
        "state_getKeysPaged",
        rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Timeout {
            detail: "cosmetic fixture".into(),
        })),
    );
    assert_eq!(
        ctx.run_with(&Connected(client.clone()), |_| async { Ok(77) })
            .await?,
        77
    );
    assert!(client.is_closed());
    Ok(())
}

#[derive(Default)]
struct CallCache(Mutex<std::collections::BTreeMap<String, Value>>);
impl rtao_cli::call::RawCallCache for CallCache {
    fn save(
        &self,
        hash: String,
        spec: Value,
    ) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.0
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture cache"))?
                .insert(hash, spec);
            Ok(())
        })
    }
}
struct MemberFactory {
    keys: std::collections::BTreeMap<AccountId, Keys>,
    acquired: AtomicUsize,
    releases: Mutex<Vec<bool>>,
    fail_source_at: Option<usize>,
    fail_release: bool,
    source_override: Option<AccountId>,
    on_release: Option<Arc<dyn Fn() -> Result<(), rtao_cli::error::CliError> + Send + Sync>>,
}
impl rtao_cli::call::RawSignerFactory for MemberFactory {
    fn source<'a>(
        &'a self,
        _: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            let attempt = self.acquired.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_source_at == Some(attempt) {
                return Err(rtao_cli::error::CliError::failure(
                    "fixture source unavailable",
                ));
            }
            let key = self
                .keys
                .get(
                    &self
                        .source_override
                        .or(selection.address)
                        .ok_or_else(|| rtao_cli::error::CliError::input("fixture member"))?,
                )
                .ok_or_else(|| rtao_cli::error::CliError::input("fixture source"))?;
            Ok(Arc::new(Keys {
                key: key.key.clone(),
                unlocks: key.unlocks.clone(),
            }) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a rtao_cli::call::SigningSelection,
        _: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.releases
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture release"))?
                .push(success);
            if let Some(action) = &self.on_release {
                action()?;
            }
            if self.fail_release {
                return Err(rtao_cli::error::CliError::failure("fixture close failed"));
            }
            Ok(())
        })
    }
}
fn members() -> Result<Vec<Keys>, Box<dyn std::error::Error>> {
    (19u8..22)
        .map(|seed| {
            Ok(Keys {
                key: KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?,
                unlocks: Arc::new(AtomicUsize::new(0)),
            })
        })
        .collect()
}
fn saved_context(
    keys: &[Keys],
    flags: &[&str],
    count: usize,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let (mut ctx, sink, _) = context(flags)?;
    ctx.globals.wallet_name = "team".into();
    ctx.globals.wallet_given = true;
    ctx.globals.signatories = keys
        .iter()
        .take(count)
        .map(|keys| format!("{}=extension", keys.key.account()))
        .collect();
    ctx.books = Books::from_values(
        json!([]),
        json!([]),
        json!([{"name":"team","threshold":3,"signatories":keys.iter().map(|keys|keys.key.account().to_string()).collect::<Vec<_>>()}]),
    );
    Ok((ctx, sink))
}
fn factory(keys: &[Keys]) -> MemberFactory {
    MemberFactory {
        keys: keys
            .iter()
            .map(|key| {
                (
                    key.key.account(),
                    Keys {
                        key: key.key.clone(),
                        unlocks: key.unlocks.clone(),
                    },
                )
            })
            .collect(),
        acquired: AtomicUsize::new(0),
        releases: Mutex::new(Vec::new()),
        fail_source_at: None,
        fail_release: false,
        source_override: None,
        on_release: None,
    }
}
fn success_events(executed: Option<(AccountId, &rtao::tx::Multisig, [u8; 32])>) -> Value {
    let mut events = vec![
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}),
    ];
    if let Some((approving, group, hash)) = executed {
        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{"approving":approving,"timepoint":{"height":10,"index":1},"multisig":group.address,"call_hash":format!("0x{}",hex::encode(hash)),"result":{"Ok":[]}},"topics":[]}));
    }
    json!(events)
}
#[tokio::test]
async fn saved_semantic_dry_run_and_refusal_do_not_acquire_any_source() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let factory = factory(&keys);
    let cache = CallCache::default();
    let (ctx, sink) = saved_context(&keys, &["--dry-run", "--json", "--no-mev-shield"], 2)?;
    assert!(
        ctx.submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            Some("rtao tx fixture"),
            &factory,
            &cache
        )
        .await?
        .is_none()
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["op"], json!("multisig_execute"));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    let (ctx, _) = saved_context(&keys, &["--json", "--no-mev-shield"], 2)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &cache,
        )
        .await
        .err()
        .ok_or("refusal")?;
    assert_eq!(error.code, 1);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn saved_semantic_executed_first_round_stops_and_emits_one_receipt() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let runtime = client.refresh_runtime().await?;
    let group = rtao::tx::Multisig::new(
        &runtime,
        &keys
            .iter()
            .map(|keys| keys.key.account())
            .collect::<Vec<_>>(),
        3,
    )?;
    let inner = rtao_runtime::calls::system::remark(vec![8, 9]).compose(&runtime)?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[0].key.account(), &group, inner.hash()))),
        })
    });
    let factory = factory(&keys);
    let cache = CallCache::default();
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let result = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &cache,
        )
        .await?
        .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*factory.releases.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        result.data["multisig_followup"]["status"],
        json!("executed")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

fn publish_pending_after_release(
    factory: &mut MemberFactory,
    chain: FakeChain,
    group: &rtao::tx::Multisig,
    member: AccountId,
    call: &rtao_runtime::CallBytes,
) {
    let group = group.address;
    let hash = format!("0x{}", hex::encode(call.hash()));
    chain.mutate_state(|state| {
        let mut events = success_events(None).as_array().cloned().unwrap_or_default();
        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"NewMultisig","attributes":{"approving":member,"multisig":group,"call_hash":hash},"topics":[]}));
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized { events:json!(events) });
    });
    factory.on_release = Some(Arc::new(move || {
        chain
            .mutate_state(|state| {
                let height = state.block;
                state.seed_python(
                    "Multisig",
                    "Multisigs",
                    &[json!(group), json!(hash)],
                    json!({"when":{"height":height,"index":0},"deposit":0,"depositor":member,"approvals":[member]}),
                )
            })
            .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))
    }));
}

#[tokio::test]
async fn saved_semantic_replaced_opening_refuses_before_second_source() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let group_address = group.address;
    let call_hash = format!("0x{}", hex::encode(call.hash()));
    let member = keys[0].key.account();
    let changed_chain = chain.clone();
    factory.on_release = Some(Arc::new(move || {
        changed_chain.mutate_state(|state| state.seed_python(
        "Multisig", "Multisigs", &[json!(group_address), json!(call_hash)],
        json!({"when":{"height":999,"index":7},"deposit":0,"depositor":member,"approvals":[member]}),
    )).map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))
    }));
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("replaced opening refusal")?;
    assert!(error.message.contains("replaced"));
    assert!(
        error
            .help
            .ok_or("recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_missing_opening_identity_refuses_automatic_continuation() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let factory = factory(&keys);
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("unknown opening refusal")?;
    assert!(error.message.contains("timepoint") || error.message.contains("opening"));
    assert!(
        error
            .help
            .ok_or("recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}
async fn saved_group(
    client: &Client,
    keys: &[Keys],
) -> Result<rtao::tx::Multisig, Box<dyn std::error::Error>> {
    Ok(rtao::Executor::new(client.clone())
        .multisig(
            &keys
                .iter()
                .map(|keys| keys.key.account())
                .collect::<Vec<_>>(),
            3,
        )
        .await?)
}

struct ChangingPreview(AtomicUsize);
impl IntentInterface for ChangingPreview {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for ChangingPreview {
    fn summary(&self) -> String {
        "review changing-byte fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let build = u8::try_from(self.0.fetch_add(1, Ordering::SeqCst))
            .map_err(|_| IntentError::InvalidArgument("fixture build count".into()))?;
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![build]).compose(cx.runtime)?,
        ))
    }
}

#[tokio::test]
async fn saved_semantic_two_rounds_reuse_first_submitted_bytes_and_keep_the_original_context()
-> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let runtime = client.refresh_runtime().await?;
    // The first round reviews once, then the executor builds its actual call.
    // Later rounds must retain that submitted [1] call, never rebuild [2].
    let call = rtao_runtime::calls::system::remark(vec![1]).compose(&runtime)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[1].key.account(), &group, call.hash()))),
        })
    });
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 2)?;
    let intent = ChangingPreview(AtomicUsize::new(0));
    let hooks = Hooks {
        before: AtomicUsize::new(0),
        after: AtomicUsize::new(0),
        failed: AtomicUsize::new(0),
    };
    let result = ctx
        .submit_saved_with_options(
            &client,
            &intent,
            rtao_cli::context::SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("receipt")?;
    assert_eq!(intent.0.load(Ordering::SeqCst), 2);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.failed.load(Ordering::SeqCst), 0);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true, true]
    );
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "chain")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 2);
    for bytes in &submissions {
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(bytes, true)?).to_json();
        let args = decoded["call"]["call_args"]
            .as_array()
            .ok_or("as_multi args")?;
        let inner = args
            .iter()
            .find(|arg| arg["name"] == json!("call"))
            .ok_or("inner call")?;
        assert_eq!(
            inner["value"],
            rtao::core::DynValue::new(runtime.core().decode_spec(
                &rtao::core::runtime::TypeSpec::Call,
                call.as_bytes(),
                true
            )?)
            .to_json()
        );
    }
    assert_eq!(
        result.data["multisig_followup"]["status"],
        json!("executed")
    );
    assert_eq!(
        result.data["multisig_followup"]["call_data"],
        json!(format!("0x{}", hex::encode(call.as_bytes())))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    assert_eq!(ctx.globals.wallet_name, "team");
    assert!(ctx.globals.signer_backend.is_none());
    assert!(ctx.globals.signer_address.is_none());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_one_confirmation_covers_identical_later_approvals() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[1].key.account(), &group, call.hash()))),
        })
    });
    let (mut ctx, _) = saved_context(&keys, &["--no-mev-shield"], 2)?;
    let reads = Arc::new(AtomicUsize::new(0));
    ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["yes".into()]),
        reads: reads.clone(),
    })));
    let result = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("receipt")?;
    assert!(rtao_cli::call::multisig_executed(&result));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true, true]
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_later_source_failure_retains_exact_pending_recovery() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    factory.fail_source_at = Some(2);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "fixture source unavailable");
    let help = error.help.ok_or("pending recovery")?;
    assert!(help.contains("earlier approval(s) may remain pending"));
    assert!(help.contains("rtao multisig pending"));
    assert!(help.contains(&hex::encode(call.hash())));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true]
    );
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_close_failure_stops_next_member_and_preserves_pending_hash() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    factory.fail_release = true;
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("close failure")?;
    assert_eq!(error.message, "fixture close failed");
    assert!(
        error
            .help
            .ok_or("pending recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_identity_error_stays_primary_when_source_close_also_fails() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let mut factory = factory(&keys);
    factory.source_override = Some(keys[1].key.account());
    factory.fail_release = true;
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("identity failure")?;
    assert_eq!(error.code, 2);
    assert!(
        error
            .message
            .contains("does not match the reviewed approval member")
    );
    assert!(error.help.is_none());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![false]
    );
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    client.close().await;
    Ok(())
}
struct DirectExternalFactory {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
    acquired: AtomicUsize,
    released: Mutex<Vec<bool>>,
    fail_release: bool,
}
impl DirectExternalFactory {
    fn new(keys: &Keys, fail_release: bool) -> Self {
        Self {
            key: keys.key.clone(),
            unlocks: keys.unlocks.clone(),
            acquired: AtomicUsize::new(0),
            released: Mutex::new(Vec::new()),
            fail_release,
        }
    }
}
impl rtao_cli::context::ExternalSignerFactory for DirectExternalFactory {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(globals.signer_backend.as_deref(), Some("extension"));
            assert_eq!(role, SignerRole::Coldkey);
            self.acquired.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(Keys {
                key: self.key.clone(),
                unlocks: self.unlocks.clone(),
            }) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        source: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(globals.signer_backend.as_deref(), Some("extension"));
            assert_eq!(
                source
                    .coldkey_address()
                    .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))?,
                self.key.account()
            );
            self.released
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("direct fixture release lock"))?
                .push(success);
            if self.fail_release {
                Err(rtao_cli::error::CliError::failure(
                    "direct close failure sentinel",
                ))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn direct_external_preview_releases_without_unlock_or_submission() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&[
        "--dry-run",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let result = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?;
    assert!(result.is_none());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    let preview: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(preview["op"], json!("add_stake"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_confirmation_refusal_releases_without_unlock_or_submission() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&["--json", "--no-mev-shield", "--signer", "extension"])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("direct refusal")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("without confirmation"));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_success_releases_once_and_preserves_the_receipt() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let receipt = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?
        .ok_or("direct receipt")?;
    assert!(receipt.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*factory.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

async fn direct_external_failed_chain_receipt_case(fail_release: bool) -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: json!([{
                "phase":"ApplyExtrinsic", "extrinsic_idx":0,
                "module_id":"System", "event_id":"ExtrinsicFailed",
                "attributes":{
                    "dispatch_error":{"BadOrigin":null},
                    "dispatch_info":{
                        "weight":{"ref_time":1,"proof_size":0},
                        "class":"Normal","pays_fee":"Yes"
                    }
                }, "topics":[]
            }]),
        });
    });
    let factory = DirectExternalFactory::new(&keys, fail_release);
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("failed direct receipt")?;
    assert_eq!(error.code, 1);
    assert!(error.message.is_empty());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    let rendered: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(rendered["success"], json!(false));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_failed_chain_receipt_releases_false_and_renders_once() -> TestResult {
    direct_external_failed_chain_receipt_case(false).await
}

#[tokio::test]
async fn direct_external_failed_receipt_remains_primary_when_release_also_fails() -> TestResult {
    direct_external_failed_chain_receipt_case(true).await
}

#[tokio::test]
async fn direct_external_close_error_propagates_after_a_known_successful_submission() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, true);
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("direct close failure")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "direct close failure sentinel");
    assert_eq!(*factory.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert!(
        error
            .help
            .ok_or("known submission recovery")?
            .contains("already submitted")
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

struct DirectExecutionFailure {
    preflights: AtomicUsize,
}
impl IntentInterface for DirectExecutionFailure {
    const SPEC: &'static rtao_catalog::IntentSpec = <Preview as IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        <Preview as IntentInterface>::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for DirectExecutionFailure {
    fn summary(&self) -> String {
        "direct execution failure fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Preview { blocked: false }.build(cx).await
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        if self.preflights.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(IntentError::Validation(
                "direct primary execution sentinel".into(),
            ));
        }
        Preview { blocked: false }.preflight(cx).await
    }
}

#[tokio::test]
async fn direct_external_primary_execution_error_wins_over_release_failure() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, true);
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let intent = DirectExecutionFailure {
        preflights: AtomicUsize::new(0),
    };
    let error = ctx
        .submit(
            &client,
            &intent,
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("primary execution failure")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("direct primary execution sentinel"));
    assert!(!error.message.contains("direct close failure sentinel"));
    assert!(intent.preflights.load(Ordering::SeqCst) >= 2);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}
type CallbackTrace = Arc<Mutex<Vec<&'static str>>>;

fn callback_phase(
    trace: &CallbackTrace,
    phase: &'static str,
) -> Result<(), rtao_cli::error::CliError> {
    trace
        .lock()
        .map_err(|_| rtao_cli::error::CliError::failure("callback trace lock"))?
        .push(phase);
    Ok(())
}

struct CallbackSink {
    trace: CallbackTrace,
    memory: MemorySink,
}
impl rtao_cli::output::OutputSink for CallbackSink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
        if stream == Stream::Stdout && !bytes.is_empty() {
            self.trace
                .lock()
                .map_err(|_| std::io::Error::other("callback trace lock"))?
                .push("render");
        }
        rtao_cli::output::OutputSink::write(&self.memory, stream, bytes)
    }
}
fn callback_sink(ctx: &mut AppContext, trace: &CallbackTrace) -> MemorySink {
    let memory = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        Arc::new(CallbackSink {
            trace: trace.clone(),
            memory: memory.clone(),
        }),
    );
    memory
}

struct CallbackHooks {
    trace: CallbackTrace,
    sink: MemorySink,
    fail_before: bool,
    expected_followup: Option<&'static str>,
    observed: Mutex<Option<Value>>,
}
impl rtao_cli::context::SubmissionHooks for CallbackHooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            callback_phase(&self.trace, "before")?;
            if self.fail_before {
                Err(rtao_cli::error::CliError::failure(
                    "before-submit failure sentinel",
                ))
            } else {
                Ok(())
            }
        })
    }
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            // Enforce hook before render against the actual output sink, in
            // addition to checking the complete lifecycle order afterwards.
            assert!(
                self.sink
                    .text(Stream::Stdout)
                    .map_err(|error| { rtao_cli::error::CliError::failure(error.to_string()) })?
                    .is_empty()
            );
            if let Some(status) = self.expected_followup {
                assert_eq!(result.data["multisig_followup"]["status"], json!(status));
                assert!(
                    result.data["multisig_followup"]["call_hash"]
                        .as_str()
                        .is_some()
                );
                assert!(
                    result.data["multisig_followup"]["call_data"]
                        .as_str()
                        .is_some()
                );
            }
            callback_phase(&self.trace, "after")?;
            *self
                .observed
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("callback observed lock"))? =
                Some(result.to_json());
            Ok(())
        })
    }
}

struct CallbackExternal<'a> {
    inner: &'a DirectExternalFactory,
    trace: CallbackTrace,
    fail_source: bool,
}
impl rtao_cli::context::ExternalSignerFactory for CallbackExternal<'_> {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            if self.fail_source {
                callback_phase(&self.trace, "source-error")?;
                return Err(rtao_cli::error::CliError::failure(
                    "callback source failure sentinel",
                ));
            }
            let keys =
                rtao_cli::context::ExternalSignerFactory::keys(self.inner, globals, role).await?;
            // Record completion, not merely entry into a lazy future.
            callback_phase(&self.trace, "acquired")?;
            Ok(keys)
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            rtao_cli::context::ExternalSignerFactory::release(self.inner, globals, keys, success)
                .await?;
            callback_phase(&self.trace, "released")
        })
    }
}

struct CallbackMember<'a> {
    inner: &'a MemberFactory,
    trace: CallbackTrace,
}
impl rtao_cli::call::RawSignerFactory for CallbackMember<'_> {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            match rtao_cli::call::RawSignerFactory::source(self.inner, ctx, selection).await {
                Ok(keys) => {
                    callback_phase(&self.trace, "acquired")?;
                    Ok(keys)
                }
                Err(error) => {
                    callback_phase(&self.trace, "source-error")?;
                    Err(error)
                }
            }
        })
    }
    fn release<'a>(
        &'a self,
        selection: &'a rtao_cli::call::SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            rtao_cli::call::RawSignerFactory::release(self.inner, selection, keys, success).await?;
            callback_phase(&self.trace, "released")
        })
    }
}

fn callback_options(hooks: &CallbackHooks) -> rtao_cli::context::SubmissionOptions<'_> {
    rtao_cli::context::SubmissionOptions {
        hooks: Some(hooks),
        ..Default::default()
    }
}

#[tokio::test]
async fn direct_callback_phases_acquire_before_hook_and_release_before_receipt_and_render()
-> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: false,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let result = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?
        .ok_or("direct receipt")?;
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "released", "after", "render"]
    );
    assert_eq!(*base.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        hooks.observed.lock().map_err(|_| "observed")?.as_ref(),
        Some(&result.to_json())
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_callback_phases_release_before_enriched_receipt_hook_and_render() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let runtime = client.refresh_runtime().await?;
    let group = rtao::tx::Multisig::new(
        &runtime,
        &keys
            .iter()
            .map(|keys| keys.key.account())
            .collect::<Vec<_>>(),
        3,
    )?;
    let inner = rtao_runtime::calls::system::remark(vec![8, 9]).compose(&runtime)?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[0].key.account(), &group, inner.hash()))),
        })
    });
    let base = factory(&keys);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: Some("executed"),
        observed: Mutex::new(None),
    };
    let result = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await?
        .ok_or("saved receipt")?;
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "released", "after", "render"]
    );
    assert_eq!(*base.releases.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        hooks.observed.lock().map_err(|_| "observed")?.as_ref(),
        Some(&result.to_json())
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_before_submit_failure_releases_without_signing_or_receipt_hook() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: false,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: true,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("before hook failure")?;
    assert_eq!(error.message, "before-submit failure sentinel");
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "released"]
    );
    assert_eq!(*base.released.lock().map_err(|_| "release")?, vec![false]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_before_submit_failure_releases_without_signing_or_receipt_hook() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let base = factory(&keys);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: true,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("before hook failure")?;
    assert_eq!(error.message, "before-submit failure sentinel");
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "released"]
    );
    assert_eq!(*base.releases.lock().map_err(|_| "release")?, vec![false]);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_source_failure_invokes_neither_callback_and_has_nothing_to_release() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: true,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "callback source failure sentinel");
    assert_eq!(*trace.lock().map_err(|_| "trace")?, vec!["source-error"]);
    assert!(base.released.lock().map_err(|_| "release")?.is_empty());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_source_failure_invokes_neither_callback_and_has_nothing_to_release() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let mut base = factory(&keys);
    base.fail_source_at = Some(1);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "fixture source unavailable");
    assert_eq!(*trace.lock().map_err(|_| "trace")?, vec!["source-error"]);
    assert_eq!(base.acquired.load(Ordering::SeqCst), 1);
    assert!(base.releases.lock().map_err(|_| "release")?.is_empty());
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}
