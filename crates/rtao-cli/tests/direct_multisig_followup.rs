// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/multisig_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Direct generated approval/execute receipts through the real context/executor.

use rtao::{
    client::{
        Client, ClientConfig, Network,
        config::{ConfigFile, Paths, Store},
    },
    core::{DynValue, codec::Value as CoreValue},
    primitives::{AccountId, Balance, CryptoType, IntoCoreValue, Tao},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{DynIntent, ExecuteOptions, ExtrinsicResult, Multisig},
};
use rtao_cli::{
    application,
    books::Books,
    commands::generated::{self, TxRequest},
    context::{
        AppContext, ExternalSignerFactory, SubmissionExecution, SubmissionHooks, SubmissionOptions,
    },
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_runtime::{CallBytes, RuntimeHandle};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Trace = Arc<Mutex<Vec<&'static str>>>;
fn fail(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn phase(trace: &Trace, value: &'static str) -> Result<(), CliError> {
    trace.lock().map_err(fail)?.push(value);
    Ok(())
}
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> std::io::Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../proof/direct-multisig-followup")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root)?;
        Ok(Self(root))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
#[derive(Clone)]
struct Keys {
    key: KeypairSigner,
    trace: Trace,
    signatures: Arc<AtomicUsize>,
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([91; 32]))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok([91; 32])
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Coldkey);
            self.signatures.fetch_add(1, Ordering::SeqCst);
            self.trace
                .lock()
                .map_err(|_| SignerError::KeyView("trace".into()))?
                .push("sign");
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
struct Factory {
    keys: Keys,
    acquired: AtomicUsize,
    publish_pending: Option<(FakeChain, AccountId, [u8; 32])>,
}
impl ExternalSignerFactory for Factory {
    fn keys<'a>(
        &'a self,
        _: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Coldkey);
            self.acquired.fetch_add(1, Ordering::SeqCst);
            phase(&self.keys.trace, "source")?;
            Ok(Arc::new(self.keys.clone()) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a Globals,
        _: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            assert!(success);
            phase(&self.keys.trace, "release")?;
            if let Some((chain, group, hash)) = &self.publish_pending {
                chain.mutate_state(|state| state.seed_python("Multisig", "Multisigs", &[json!(group),json!(format!("0x{}",hex::encode(hash)))],json!({"when":{"height":10,"index":1},"deposit":0,"depositor":self.keys.key.account(),"approvals":[self.keys.key.account()]}))).map_err(fail)?;
            }
            Ok(())
        })
    }
}
struct Hooks {
    trace: Trace,
    raw: Mutex<Vec<ExtrinsicResult>>,
    enriched: Mutex<Vec<ExtrinsicResult>>,
    fail_raw: bool,
    fail_enriched: bool,
}
impl Hooks {
    fn new(trace: Trace) -> Self {
        Self {
            trace,
            raw: Mutex::new(Vec::new()),
            enriched: Mutex::new(Vec::new()),
            fail_raw: false,
            fail_enriched: false,
        }
    }
}
impl SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async { phase(&self.trace, "before") })
    }
    fn on_result<'a>(&'a self, result: &'a ExtrinsicResult) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            assert!(result.success);
            assert!(!result.data.contains_key("multisig_followup"));
            phase(&self.trace, "raw")?;
            self.raw.lock().map_err(fail)?.push(result.clone());
            if self.fail_raw {
                return Err(fail("raw persistence sentinel"));
            }
            Ok(())
        })
    }
    fn after_followup<'a>(
        &'a self,
        result: &'a ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            assert_eq!(self.raw.lock().map_err(fail)?.len(), 1);
            phase(&self.trace, "followup")?;
            self.enriched.lock().map_err(fail)?.push(result.clone());
            if self.fail_enriched {
                return Err(fail("enriched persistence sentinel"));
            }
            Ok(())
        })
    }
}
struct Fixture {
    root: Root,
    client: Client,
    chain: FakeChain,
    keys: Keys,
    other: AccountId,
    runtime: RuntimeHandle,
    group: Multisig,
    inner: CallBytes,
}
impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let root = Root::new()?;
        let keys = Keys {
            key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
            trace: Trace::default(),
            signatures: Arc::new(AtomicUsize::new(0)),
        };
        let other = AccountId([20; 32]);
        let mut state = FakeState::try_default()?;
        state.seed_constant("Multisig", "DepositBase", json!(100))?;
        state.seed_constant("Multisig", "DepositFactor", json!(10))?;
        state.seed_python(
            "System",
            "Account",
            &[json!(keys.key.account())],
            json!({"data":{"free":100_000_000_000u64}}),
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let runtime = client.refresh_runtime().await?;
        let group = Multisig::new(&runtime, &[keys.key.account(), other], 2)?;
        let inner = rtao_runtime::calls::balances::transfer_allow_death(
            DynValue::new(AccountId([42; 32]).into_core_value()),
            Balance::<Tao>::from_rao(1).to_wire(),
        )
        .compose(&runtime)?;
        Ok(Self {
            root,
            client,
            chain,
            keys,
            other,
            runtime,
            group,
            inner,
        })
    }
    fn context(
        &self,
        op: &'static str,
    ) -> Result<(AppContext, TxRequest, MemorySink), Box<dyn std::error::Error>> {
        let others = json!([self.other]).to_string();
        let call=json!({"op":"transfer","dest_ss58":AccountId([42;32]),"amount_tao":"0.000000001","keep_alive":false}).to_string();
        let command = op.replace('_', "-");
        let parsed =
            application::command_tree([generated::tx_command()?]).try_get_matches_from([
                "rtao",
                "tx",
                &command,
                "--threshold",
                "2",
                "--other-signatories",
                &others,
                "--call",
                &call,
                "-w",
                "operator",
                "--yes",
                "--json",
                "--no-mev-shield",
            ])?;
        let leaf = parsed
            .subcommand()
            .ok_or("tx")?
            .1
            .subcommand()
            .ok_or("intent")?
            .1;
        let config = Default::default();
        let mut globals = Globals::resolve(&parsed, &config, &self.root.0, |_| None)?;
        globals.apply_with_env(leaf, &generated::intent_reserved(op)?, |_| None)?;
        globals.wallet_path = self.root.0.join("wallets");
        globals.signer_backend = Some("extension".into());
        globals.signer_address = Some(self.keys.key.account().to_string());
        let books = Books::from_values(
            json!([{"name":"alice","address":self.keys.key.account()},{"name":"bob","address":self.other}]),
            json!([]),
            json!([
                {"name":"wrong-threshold","threshold":3,"signatories":["alice","bob"]},
                {"name":"wrong-members","threshold":2,"signatories":["alice",AccountId([30;32])]},
                {"name":"team","threshold":2,"signatories":["bob","alice","bob","unavailable-public-ref"]},
                {"name":"later-match","threshold":2,"signatories":["alice","bob"]}
            ]),
        );
        let mut ctx = AppContext::new(
            globals,
            config,
            books,
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        let paths = Paths::new(&self.root.0, |_| None);
        ctx.config_store = Some(Store::new(paths));
        let sink = MemorySink::default();
        ctx.output = Output::new(
            OutputOptions {
                json_mode: true,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        let request = TxRequest::from_matches(&mut ctx, op, leaf, &mut PromptHistory::default())?;
        Ok((ctx, request, sink))
    }
    fn factory(&self, pending: bool) -> Factory {
        Factory {
            keys: self.keys.clone(),
            acquired: AtomicUsize::new(0),
            publish_pending: pending
                .then(|| (self.chain.clone(), self.group.address, self.inner.hash())),
        }
    }
    fn cache_path(&self) -> PathBuf {
        Paths::new(&self.root.0, |_| None)
            .path(ConfigFile::MultisigCache)
            .to_owned()
    }
    fn queue_executed(&self) {
        self.chain.mutate_state(|state|state.queue_submit(SubmitScript::InBlockThenFinalized {events:json!([
            {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]},
            {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{"approving":self.keys.key.account(),"timepoint":{"height":10,"index":1},"multisig":self.group.address,"call_hash":format!("0x{}",hex::encode(self.inner.hash())),"result":{"Ok":[]}},"topics":[]}
        ])}));
    }
    fn assert_once(&self, factory: &Factory) -> TestResult {
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(self.keys.signatures.load(Ordering::SeqCst), 1);
        assert_eq!(
            self.chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .len(),
            1
        );
        Ok(())
    }
}

#[tokio::test]
async fn generated_approve_and_execute_attach_saved_book_followup_after_raw_receipt_and_release()
-> TestResult {
    for op in ["multisig_approve", "multisig_execute"] {
        let fixture = Fixture::new().await?;
        let (mut ctx, request, sink) = fixture.context(op)?;
        let pending = op == "multisig_approve";
        let factory = fixture.factory(pending);
        if !pending {
            fixture.queue_executed();
        }
        let hooks = Hooks::new(fixture.keys.trace.clone());
        let receipt = generated::execute_tx(
            &mut ctx,
            &fixture.client,
            &request,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await?
        .ok_or("receipt")?;
        assert!(receipt.success);
        assert_eq!(
            receipt.data["multisig_call_hash"],
            format!("0x{}", hex::encode(fixture.inner.hash()))
        );
        assert_eq!(
            receipt.data["multisig_call_data"],
            format!("0x{}", hex::encode(fixture.inner.as_bytes()))
        );
        let followup = &receipt.data["multisig_followup"];
        assert_eq!(followup["multisig_preset"], "team");
        assert_eq!(followup["multisig_address"], json!(fixture.group.address));
        assert_eq!(followup["target"], "Balances.transfer_allow_death");
        assert_eq!(
            followup["status"],
            if pending { "pending" } else { "executed" }
        );
        if pending {
            assert_eq!(
                followup["approvals_so_far"],
                json!([fixture.keys.key.account()])
            );
            assert_eq!(followup["timepoint"], json!({"height":10,"index":1}));
            assert_eq!(followup["remaining_signatories"], json!([fixture.other]));
            let command = followup["co_signer_commands"][0]["command"]
                .as_str()
                .ok_or("command")?;
            assert!(command.contains("-w team --signatory bob"));
            assert!(!command.contains("--signer hotkey"));
        }
        let cache: Value = serde_json::from_slice(&std::fs::read(fixture.cache_path())?)?;
        let hash = format!("0x{}", hex::encode(fixture.inner.hash()));
        assert_eq!(
            cache[&hash]["call_data"],
            receipt.data["multisig_call_data"]
        );
        assert_eq!(cache[&hash]["multisig_preset"], "team");
        let raw = hooks.raw.lock().map_err(|_| "raw")?;
        assert_eq!(raw.len(), 1);
        assert!(!raw[0].data.contains_key("multisig_followup"));
        assert_eq!(hooks.enriched.lock().map_err(|_| "enriched")?.len(), 1);
        assert_eq!(
            *fixture.keys.trace.lock().map_err(|_| "trace")?,
            vec!["source", "before", "sign", "raw", "release", "followup"]
        );
        let rendered: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
        assert_eq!(rendered["data"]["multisig_followup"], *followup);
        fixture.assert_once(&factory)?;
        fixture.client.close().await;
    }
    Ok(())
}

// Exercise malformed/future cosmetic receipt domains after a real generated
// execute, without changing the reviewed or submitted extrinsic.
struct ReceiptExtras {
    bytes: Option<Vec<u8>>,
    hash: Option<[u8; 32]>,
}
impl SubmissionExecution for ReceiptExtras {
    fn execute<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a dyn DynIntent,
        keys: &'a dyn DynKeySource,
        options: &'a ExecuteOptions,
    ) -> BoxFuture<'a, Result<ExtrinsicResult, CliError>> {
        Box::pin(async move {
            let mut result = rtao::Executor::new(client.clone())
                .execute_dyn(intent, keys, options)
                .await
                .map_err(fail)?;
            if let Some(bytes) = &self.bytes {
                result.data.insert(
                    "multisig_call_data".into(),
                    json!(format!("0x{}", hex::encode(bytes))),
                );
            }
            if let Some(hash) = self.hash {
                result.data.insert(
                    "multisig_call_hash".into(),
                    json!(format!("0x{}", hex::encode(hash))),
                );
            }
            Ok(result)
        })
    }
}
#[tokio::test]
async fn direct_multisig_advisory_identity_and_cache_failures_preserve_the_successful_receipt()
-> TestResult {
    for mode in ["wrong-hash", "incomplete-scale", "cache-unwritable"] {
        let fixture = Fixture::new().await?;
        let (mut ctx, request, _) = fixture.context("multisig_execute")?;
        fixture.queue_executed();
        let factory = fixture.factory(false);
        let hooks = Hooks::new(fixture.keys.trace.clone());
        let mutation = ReceiptExtras {
            bytes: (mode == "incomplete-scale").then(|| vec![255]),
            hash: (mode == "wrong-hash").then_some([0; 32]),
        };
        if mode == "cache-unwritable" {
            std::fs::create_dir_all(fixture.cache_path())?;
        }
        let receipt = generated::execute_tx(
            &mut ctx,
            &fixture.client,
            &request,
            SubmissionOptions {
                hooks: Some(&hooks),
                execution: Some(&mutation),
                ..Default::default()
            },
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await?
        .ok_or("receipt")?;
        assert!(receipt.success);
        assert!(!receipt.data.contains_key("multisig_followup"));
        assert_eq!(hooks.raw.lock().map_err(|_| "raw")?.len(), 1);
        assert_eq!(hooks.enriched.lock().map_err(|_| "enriched")?.len(), 1);
        if mode != "cache-unwritable" {
            assert!(!fixture.cache_path().exists());
        }
        fixture.assert_once(&factory)?;
        fixture.client.close().await;
    }
    Ok(())
}
#[tokio::test]
async fn direct_multisig_unknown_decoded_target_never_poison_caches_and_keeps_recovery_hooks()
-> TestResult {
    let fixture = Fixture::new().await?;
    let (mut ctx, request, _) = fixture.context("multisig_execute")?;
    fixture.queue_executed();
    // A valid MultiAddress::Index is admitted by the codec, but cannot be
    // rendered as an SS58 proxy identity by the cosmetic replay decoder.
    let unknown = rtao_runtime::calls::proxy::proxy(
        DynValue::new(CoreValue::record(vec![(
            "Index".into(),
            CoreValue::Uint(3),
        )])),
        None,
        fixture.inner.clone(),
    )
    .compose(&fixture.runtime)?;
    let mutation = ReceiptExtras {
        bytes: Some(unknown.as_bytes().to_vec()),
        hash: Some(unknown.hash()),
    };
    let hooks = Hooks::new(fixture.keys.trace.clone());
    let factory = fixture.factory(false);
    let receipt = generated::execute_tx(
        &mut ctx,
        &fixture.client,
        &request,
        SubmissionOptions {
            hooks: Some(&hooks),
            execution: Some(&mutation),
            ..Default::default()
        },
        Some(&factory),
        &mut PromptHistory::default(),
    )
    .await?
    .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(receipt.data["multisig_followup"]["status"], "executed");
    assert_eq!(receipt.data["multisig_followup"]["target"], Value::Null);
    assert_eq!(
        receipt.data["multisig_followup"]["call_data"],
        format!("0x{}", hex::encode(unknown.as_bytes()))
    );
    assert!(!fixture.cache_path().exists());
    assert_eq!(hooks.enriched.lock().map_err(|_| "enriched")?.len(), 1);
    fixture.assert_once(&factory)?;
    fixture.client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_multisig_durable_hook_failures_retain_known_receipt_without_resubmission()
-> TestResult {
    for raw_failure in [true, false] {
        let fixture = Fixture::new().await?;
        let (ctx, request, _) = fixture.context("multisig_execute")?;
        fixture.queue_executed();
        let mut hooks = Hooks::new(fixture.keys.trace.clone());
        hooks.fail_raw = raw_failure;
        hooks.fail_enriched = !raw_failure;
        // Exercise the injected-key generated route as well as execute_tx's
        // external-source route; both use the same context finish boundary.
        let error = generated::execute_tx_with_keys(
            &ctx,
            &fixture.client,
            &request,
            &fixture.keys,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
        )
        .await
        .err()
        .ok_or("durable hook must fail")?;
        assert!(error.message.contains(if raw_failure {
            "raw persistence sentinel"
        } else {
            "enriched persistence sentinel"
        }));
        assert!(
            error
                .help
                .as_deref()
                .is_some_and(|text| text.contains("transaction was already submitted"))
        );
        assert_eq!(hooks.raw.lock().map_err(|_| "raw")?.len(), 1);
        assert_eq!(
            hooks.enriched.lock().map_err(|_| "enriched")?.len(),
            usize::from(!raw_failure)
        );
        assert_eq!(fixture.cache_path().exists(), !raw_failure);
        assert_eq!(fixture.keys.signatures.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture
                .chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .len(),
            1
        );
        fixture.client.close().await;
    }
    Ok(())
}
