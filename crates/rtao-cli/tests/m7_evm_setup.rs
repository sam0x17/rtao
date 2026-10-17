// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/setup.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Socket-free EVM setup fixtures and metadata-backed native sudo plans.

#![cfg(feature = "evm")]

use rtao::{
    client::{Client, ClientConfig, Inclusion, Network},
    evm::{self, EvmKeyInfo},
    primitives::{AccountId, CryptoType, H256},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{ExtrinsicResult, IntentInterface, Plan, PlanOptions},
};
use rtao_cli::{
    application,
    books::Books,
    commands::evm::setup::{self, SetupBackend, SetupExecution, SetupStep},
    context::{AppContext, ExternalSignerFactory, SubmissionHooks, SubmissionOptions},
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn Error>>;
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
fn tree() -> clap::Command {
    let mut command = clap::Command::new("evm").subcommand_required(true);
    for child in setup::commands() {
        command = command.subcommand(child);
    }
    application::command_tree([command])
}
struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
}
impl Fixture {
    fn new(args: &[&str]) -> Result<Self, Box<dyn Error>> {
        let matches = tree().try_get_matches_from(args)?;
        let (name, leaf) = matches
            .subcommand()
            .ok_or("missing EVM")?
            .1
            .subcommand()
            .ok_or("missing leaf")?;
        let mut globals = Globals::resolve(
            &matches,
            &Default::default(),
            Path::new("/fixture/no-user-home"),
            |_| None,
        )?;
        globals.apply_with_env(leaf, &setup::reserved(name), |_| None)?;
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode: ctx.globals.json,
                quiet: ctx.globals.quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        Ok(Self { matches, ctx, sink })
    }
    async fn run(&mut self, backend: &Fake) -> Result<(), CliError> {
        let (name, leaf) = self
            .matches
            .subcommand()
            .and_then(|(_, group)| group.subcommand())
            .ok_or_else(|| CliError::input("missing route"))?;
        let network = evm::networks::evm_network_with_env(&self.ctx.globals.network, |_| None)
            .map_err(|e| CliError::failure(e.to_string()))?;
        setup::run_with(
            &mut self.ctx,
            name,
            leaf,
            &network,
            backend,
            SetupExecution {
                client: None,
                options: SubmissionOptions::default(),
                external: None,
                history: &mut PromptHistory::default(),
            },
        )
        .await
    }
    fn json(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_str(&self.sink.text(Stream::Stdout)?)?)
    }
}
struct Response {
    method: &'static str,
    params: Vec<Value>,
    result: evm::Result<Value>,
}
#[derive(Default)]
struct Fake {
    replies: Mutex<VecDeque<Response>>,
    keys: Vec<EvmKeyInfo>,
    selected: Mutex<Vec<Option<String>>>,
    steps: Mutex<Vec<SetupStep>>,
    outcomes: Mutex<VecDeque<Result<Option<ExtrinsicResult>, CliError>>>,
}
impl Fake {
    fn reply(
        self,
        method: &'static str,
        params: Vec<Value>,
        result: evm::Result<Value>,
    ) -> Result<Self, Box<dyn Error>> {
        self.replies
            .lock()
            .map_err(|_| "replies lock")?
            .push_back(Response {
                method,
                params,
                result,
            });
        Ok(self)
    }
    fn chain(id: Value) -> Result<Self, Box<dyn Error>> {
        Self::default().reply("eth_chainId", vec![], Ok(id))
    }
    fn outcome(
        self,
        result: Result<Option<ExtrinsicResult>, CliError>,
    ) -> Result<Self, Box<dyn Error>> {
        self.outcomes
            .lock()
            .map_err(|_| "outcomes lock")?
            .push_back(result);
        Ok(self)
    }
    fn done(&self) -> TestResult {
        assert!(self.replies.lock().map_err(|_| "replies lock")?.is_empty());
        Ok(())
    }
}
impl SetupBackend for Fake {
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, evm::Result<Value>> {
        Box::pin(async move {
            let next = self
                .replies
                .lock()
                .map_err(|_| evm::Error::invalid("replies lock"))?
                .pop_front()
                .ok_or_else(|| evm::Error::invalid(format!("unexpected request {method}")))?;
            if method != next.method || params != next.params {
                return Err(evm::Error::invalid(format!(
                    "request mismatch: {method} {params:?}"
                )));
            }
            next.result
        })
    }
    fn stored_keys<'a>(
        &'a self,
        _: &'a AppContext,
    ) -> BoxFuture<'a, Result<Vec<EvmKeyInfo>, CliError>> {
        Box::pin(async move { Ok(self.keys.clone()) })
    }
    fn key_info<'a>(
        &'a self,
        _: &'a AppContext,
        key: Option<&'a str>,
    ) -> BoxFuture<'a, Result<EvmKeyInfo, CliError>> {
        Box::pin(async move {
            self.selected
                .lock()
                .map_err(|_| CliError::failure("selected lock"))?
                .push(key.map(str::to_owned));
            self.keys
                .first()
                .cloned()
                .ok_or_else(|| CliError::failure("missing fixture key"))
        })
    }
    fn submit<'a>(
        &'a self,
        _: &'a mut AppContext,
        step: &'a SetupStep,
        _: SetupExecution<'a>,
    ) -> BoxFuture<'a, Result<Option<ExtrinsicResult>, CliError>> {
        Box::pin(async move {
            self.steps
                .lock()
                .map_err(|_| CliError::failure("steps lock"))?
                .push(step.clone());
            self.outcomes
                .lock()
                .map_err(|_| CliError::failure("outcomes lock"))?
                .pop_front()
                .unwrap_or_else(|| Err(CliError::failure("unexpected native submission")))
        })
    }
}
fn key(name: &str) -> EvmKeyInfo {
    EvmKeyInfo {
        name: name.into(),
        address: format!("0x{}", "11".repeat(20)),
        ss58_mirror: "fixture-mirror".into(),
        path: PathBuf::from("/fixture/public-only"),
    }
}
fn receipt() -> ExtrinsicResult {
    ExtrinsicResult {
        success: true,
        message: "included".into(),
        extrinsic_hash: None,
        block_hash: Some(H256([7; 32])),
        extrinsic_id: Some("12-1".into()),
        explorer_url: None,
        fee: None,
        events: Vec::new(),
        error: None,
        data: Default::default(),
        output: (),
        inclusion: Inclusion::InBlock,
        fate_unknown: None,
    }
}
fn rpc_error() -> evm::Error {
    evm::Error::Rpc {
        code: -32603,
        message: "chain id is unset".into(),
        data: None,
    }
}

#[test]
fn routes_have_exact_names_defaults_and_global_tiers() -> TestResult {
    let tree = tree();
    let evm = tree
        .get_subcommands()
        .find(|child| child.get_name() == "evm")
        .ok_or("missing EVM")?;
    assert_eq!(
        evm.get_subcommands()
            .map(|child| child.get_name())
            .collect::<Vec<_>>(),
        ["networks", "config", "doctor", "setup-localnet"]
    );
    let parsed = tree
        .clone()
        .try_get_matches_from(["rtao", "evm", "setup-localnet"])?;
    assert_eq!(
        parsed
            .subcommand()
            .ok_or("EVM")?
            .1
            .subcommand()
            .ok_or("setup")?
            .1
            .get_one::<u64>("chain_id"),
        Some(&945)
    );
    assert!(
        tree.clone()
            .try_get_matches_from(["rtao", "evm", "networks", "--dry-run"])
            .is_err()
    );
    assert!(
        tree.clone()
            .try_get_matches_from(["rtao", "evm", "doctor", "--chain-id", "42"])
            .is_err()
    );
    assert!(
        tree.try_get_matches_from(["rtao", "evm", "setup-localnet", "--chain-id", "-1"])
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn network_records_preserve_python_order_fields_and_null_local_id() -> TestResult {
    let mut f = Fixture::new(&["rtao", "evm", "networks", "--json"])?;
    f.run(&Fake::default()).await?;
    let local = evm::evm_network("local")?;
    assert_eq!(
        f.json()?,
        json!([
            {"network":"finney","chain_id":964,"rpc_url":"https://lite.chain.opentensor.ai","currency":"TAO"},
            {"network":"test","chain_id":945,"rpc_url":"https://test.chain.opentensor.ai","currency":"TAO"},
            {"network":"local","chain_id":null,"rpc_url":local.rpc_url,"currency":"TAO"}
        ])
    );
    Ok(())
}

#[tokio::test]
async fn configs_preserve_machine_shapes_and_escape_hardhat_endpoint() -> TestResult {
    for (network, display, chain) in [
        ("finney", "Subtensor", json!(964)),
        ("test", "Subtensor test", json!(945)),
        ("local", "Subtensor local", Value::Null),
    ] {
        let mut f = Fixture::new(&["rtao", "evm", "config", "--network", network, "--json"])?;
        f.run(&Fake::default()).await?;
        assert_eq!(
            f.json()?,
            json!({"network_name":display,"rpc_url":evm::networks::evm_network_with_env(network, |_| None)?.rpc_url,"chain_id":chain,"currency_symbol":"TAO"})
        );
    }
    let mut remix = Fixture::new(&["rtao", "evm", "config", "--format", "remix", "--json"])?;
    remix.run(&Fake::default()).await?;
    assert_eq!(
        remix.json()?,
        json!({"Compiler":"0.8.24 (or lower)","EVM version":"cancun","Environment":"Injected Provider - MetaMask (connected to the network above)","RPC URL":"https://lite.chain.opentensor.ai"})
    );
    let endpoint = "https://example.invalid/\";throw 1;//";
    let mut hardhat = Fixture::new(&[
        "rtao",
        "evm",
        "config",
        "--format",
        "hardhat",
        "--network",
        endpoint,
        "--json",
    ])?;
    hardhat.run(&Fake::default()).await?;
    let snippet = hardhat.json()?.as_str().ok_or("snippet")?.to_owned();
    assert!(snippet.contains(&format!("url: {},", json!(endpoint))));
    for required in [
        "const { Wallet } = require(\"ethers\");",
        "Wallet.fromEncryptedJsonSync(",
        "solidity: { version: \"0.8.24\", settings: { evmVersion: \"cancun\" } }",
        "accounts: deployerKey ? [deployerKey] : []",
        "mocha: { timeout: 300000 }",
    ] {
        assert!(snippet.contains(required), "missing {required}");
    }
    let mut human = Fixture::new(&[
        "rtao",
        "evm",
        "config",
        "--format",
        "hardhat",
        "--network",
        endpoint,
    ])?;
    human.run(&Fake::default()).await?;
    assert_eq!(human.sink.text(Stream::Stdout)?, format!("{snippet}\n"));
    assert!(snippet.lines().count() > 20);
    let mut unknown = Fixture::new(&["rtao", "evm", "config", "--format", "Hardhat", "--json"])?;
    let error = unknown
        .run(&Fake::default())
        .await
        .err()
        .ok_or("expected unknown format")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "unknown format 'Hardhat'");
    Ok(())
}

#[tokio::test]
async fn doctor_chain_mismatch_is_nonfatal_and_default_key_precedes_filename_order() -> TestResult {
    let backend = Fake {
        keys: vec![key("aaa"), key("default")],
        ..Default::default()
    };
    let address = backend.keys[1].address.clone();
    let backend = backend
        .reply("eth_blockNumber", vec![], Ok(json!("0x10")))?
        .reply("eth_chainId", vec![], Ok(json!("0x2a")))?
        .reply("eth_gasPrice", vec![], Ok(json!("0x3b9aca00")))?
        .reply(
            "eth_getBalance",
            vec![json!(address), json!("latest")],
            Ok(json!("0x0")),
        )?
        .reply(
            "eth_getTransactionCount",
            vec![json!(address), json!("latest")],
            Ok(json!("0x7")),
        )?;
    let mut f = Fixture::new(&["rtao", "evm", "doctor", "--json"])?;
    f.run(&backend).await?;
    let mut expected = json!({"rpc url":"https://lite.chain.opentensor.ai","block number":16,"chain id":42,"gas price (wei)":1_000_000_000,"nonce":7});
    expected[format!("key default ({address})")] = json!("τ0.000000000");
    assert_eq!(f.json()?, expected);
    let errors = f.sink.text(Stream::Stderr)?;
    assert!(errors.contains("chain ID mismatch: node reports 42, expected 964 for finney"));
    assert!(!errors.contains("key default has no balance"));
    assert!(
        backend
            .selected
            .lock()
            .map_err(|_| "selected lock")?
            .is_empty()
    );
    backend.done()
}

#[tokio::test]
async fn doctor_unset_chain_keeps_exact_wide_gas_number_and_explicit_key_reference() -> TestResult {
    let backend = Fake {
        keys: vec![key("other")],
        ..Default::default()
    };
    let address = backend.keys[0].address.clone();
    let backend = backend
        .reply("eth_blockNumber", vec![], Ok(json!("0x1")))?
        .reply("eth_chainId", vec![], Err(rpc_error()))?
        .reply("eth_gasPrice", vec![], Ok(json!("0x100000000000000000000")))?
        .reply(
            "eth_getBalance",
            vec![json!(address), json!("latest")],
            Ok(json!("0xde0b6b3a7640000")),
        )?
        .reply(
            "eth_getTransactionCount",
            vec![json!(address), json!("latest")],
            Ok(json!("0x0")),
        )?;
    let mut f = Fixture::new(&[
        "rtao",
        "evm",
        "doctor",
        "--evm-key",
        "otherwallet/other",
        "--json",
    ])?;
    f.run(&backend).await?;
    let fields = f.json()?;
    assert_eq!(
        fields["gas price (wei)"].to_string(),
        "1208925819614629174706176"
    );
    assert_eq!(fields[format!("key other ({address})")], "τ1.000000000");
    assert!(
        fields["chain id"]
            .as_str()
            .is_some_and(|value| value.starts_with("error: "))
    );
    assert_eq!(
        *backend.selected.lock().map_err(|_| "selected lock")?,
        [Some("otherwallet/other".into())]
    );
    assert!(
        f.sink
            .text(Stream::Stderr)?
            .contains("chain ID not set on this node")
    );
    backend.done()
}

#[tokio::test]
async fn doctor_transport_failure_and_noncanonical_block_number_stop_without_key_access()
-> TestResult {
    for reply in [
        Err(evm::Error::Timeout("fixture deadline".into())),
        Ok(json!("0x00")),
    ] {
        let backend = Fake::default().reply("eth_blockNumber", vec![], reply)?;
        let mut f = Fixture::new(&["rtao", "evm", "doctor", "--json"])?;
        let error = f.run(&backend).await.err().ok_or("expected unreachable")?;
        assert_eq!(error.code, 1);
        assert!(error.message.contains("EVM RPC unreachable at"));
        assert!(f.sink.text(Stream::Stdout)?.is_empty());
        assert!(
            backend
                .selected
                .lock()
                .map_err(|_| "selected lock")?
                .is_empty()
        );
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn malformed_chain_id_observation_cannot_enable_native_setup() -> TestResult {
    for reply in [
        Ok(json!("0x00")),
        Ok(json!("0x10000000000000000")),
        Ok(json!("garbage")),
        Err(evm::Error::invalid("mismatched envelope ID")),
    ] {
        let backend = Fake::default().reply("eth_chainId", vec![], reply)?;
        let mut f = Fixture::new(&[
            "rtao",
            "evm",
            "setup-localnet",
            "--network",
            "local",
            "--yes",
            "--json",
        ])?;
        let error = f
            .run(&backend)
            .await
            .err()
            .ok_or("expected malformed response refusal")?;
        assert!(error.message.starts_with("EVM RPC error:"));
        assert!(backend.steps.lock().map_err(|_| "steps lock")?.is_empty());
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn public_network_refusal_precedes_rpc_and_native_operations() -> TestResult {
    for network in ["finney", "test"] {
        let backend = Fake::default();
        let mut f = Fixture::new(&[
            "rtao",
            "evm",
            "setup-localnet",
            "--network",
            network,
            "--rpc-url",
            "http://example.invalid",
            "--yes",
            "--json",
        ])?;
        let error = f.run(&backend).await.err().ok_or("expected refusal")?;
        assert_eq!(error.code, 2);
        assert!(
            error
                .message
                .starts_with(&format!("refusing to run against {network}"))
        );
        assert!(backend.steps.lock().map_err(|_| "steps lock")?.is_empty());
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn setup_dry_run_accepts_unset_chain_and_never_acquires_native_boundary() -> TestResult {
    for (reply, steps) in [
        (
            Err(rpc_error()),
            json!({"step 1":"set EVM chain ID to 945","step 2":"disable the contract deployment whitelist"}),
        ),
        (
            Ok(json!("0x3b1")),
            json!({"step 1":"disable the contract deployment whitelist"}),
        ),
    ] {
        let backend = Fake::default().reply("eth_chainId", vec![], reply)?;
        let mut f = Fixture::new(&[
            "rtao",
            "evm",
            "setup-localnet",
            "--network",
            "local",
            "--dry-run",
            "--json",
        ])?;
        f.run(&backend).await?;
        assert_eq!(f.json()?, steps);
        assert!(backend.steps.lock().map_err(|_| "steps lock")?.is_empty());
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn setup_requires_confirmation_before_native_submission() -> TestResult {
    let backend = Fake::chain(json!("0x2a"))?;
    let mut f = Fixture::new(&[
        "rtao",
        "evm",
        "setup-localnet",
        "--network",
        "local",
        "--json",
    ])?;
    let error = f.run(&backend).await.err().ok_or("expected refusal")?;
    assert_eq!(error.message, "refusing to submit without confirmation");
    assert!(backend.steps.lock().map_err(|_| "steps lock")?.is_empty());
    backend.done()
}

#[tokio::test]
async fn setup_changes_chain_then_whitelist_and_skips_already_correct_id() -> TestResult {
    for (current, expected) in [
        (
            "0x2a",
            vec![
                SetupStep::ChainId {
                    chain_id: 945,
                    current: Some(42),
                },
                SetupStep::DisableWhitelist,
            ],
        ),
        ("0x3b1", vec![SetupStep::DisableWhitelist]),
    ] {
        let backend = Fake::chain(json!(current))?;
        for _ in &expected {
            backend
                .outcomes
                .lock()
                .map_err(|_| "outcomes lock")?
                .push_back(Ok(Some(receipt())));
        }
        let mut f = Fixture::new(&[
            "rtao",
            "evm",
            "setup-localnet",
            "--network",
            "local",
            "--yes",
            "--json",
        ])?;
        f.run(&backend).await?;
        assert_eq!(*backend.steps.lock().map_err(|_| "steps lock")?, expected);
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn setup_stops_on_unknown_rejected_or_unincluded_result_without_retry() -> TestResult {
    let mut unresolved = receipt();
    unresolved.fate_unknown = Some(H256([8; 32]));
    let mut rejected = receipt();
    rejected.success = false;
    let mut accepted = receipt();
    accepted.inclusion = Inclusion::PoolAccepted;
    for result in [unresolved, rejected, accepted] {
        let unknown = result.fate_unknown;
        let backend = Fake::chain(json!("0x2a"))?.outcome(Ok(Some(result)))?;
        let mut f = Fixture::new(&[
            "rtao",
            "evm",
            "setup-localnet",
            "--network",
            "local",
            "--yes",
            "--json",
        ])?;
        let error = f
            .run(&backend)
            .await
            .err()
            .ok_or("expected unresolved setup")?;
        assert!(error.message.contains("reconcile the last native result"));
        assert_eq!(backend.steps.lock().map_err(|_| "steps lock")?.len(), 1);
        assert!(
            error
                .help
                .as_deref()
                .is_some_and(|help| help.contains("12-1"))
        );
        if let Some(hash) = unknown {
            assert!(
                error
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains(&format!("0x{}", hex::encode(hash.0))))
            );
        }
        backend.done()?;
    }
    Ok(())
}

#[tokio::test]
async fn setup_does_not_advance_after_a_successful_pending_multisig_approval() -> TestResult {
    let backend = Fake::chain(json!("0x2a"))?.outcome(Ok(Some(receipt())))?;
    let mut f = Fixture::new(&[
        "rtao",
        "evm",
        "setup-localnet",
        "--network",
        "local",
        "--yes",
        "--json",
    ])?;
    f.ctx.books.multisigs = vec![json!({"name":f.ctx.globals.wallet_name,"threshold":2})];
    let error = f
        .run(&backend)
        .await
        .err()
        .ok_or("expected pending approval stop")?;
    assert!(error.message.contains("reconcile the last native result"));
    assert_eq!(backend.steps.lock().map_err(|_| "steps lock")?.len(), 1);
    backend.done()
}

#[tokio::test]
async fn second_step_error_retains_first_receipt_and_original_recovery_help() -> TestResult {
    let backend = Fake::chain(json!("0x2a"))?
        .outcome(Ok(Some(receipt())))?
        .outcome(Err(CliError::failure(
            "transaction 0xfeed has unresolved fate",
        )
        .with_help("do not retransmit 0xfeed")))?;
    let mut f = Fixture::new(&[
        "rtao",
        "evm",
        "setup-localnet",
        "--network",
        "local",
        "--yes",
        "--json",
    ])?;
    let error = f.run(&backend).await.err().ok_or("expected error")?;
    assert_eq!(error.message, "transaction 0xfeed has unresolved fate");
    let help = error.help.ok_or("missing recovery")?;
    assert!(help.contains("do not retransmit 0xfeed"));
    assert!(help.contains("set EVM chain ID to 945 (currently 42)"));
    assert!(help.contains("12-1"));
    assert_eq!(backend.steps.lock().map_err(|_| "steps lock")?.len(), 2);
    backend.done()
}

struct PublicKeys(Arc<AtomicUsize>);
impl KeyView for PublicKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([7; 32]))
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([8; 32]))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok([8; 32])
    }
}
impl DynKeySource for PublicKeys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(SignerError::KeyView("planning must never unlock".into())) })
    }
}

#[tokio::test]
async fn actual_executor_wraps_both_native_steps_in_sudo_without_unlocking() -> TestResult {
    let chain = FakeChain::from_state(FakeState::try_default()?);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let keys = PublicKeys(calls.clone());
    for (step, pallet, name, field, expected) in [
        (
            SetupStep::ChainId {
                chain_id: u64::MAX,
                current: Some(42),
            },
            "AdminUtils",
            "sudo_set_evm_chain_id",
            "chain_id",
            json!(u64::MAX),
        ),
        (
            SetupStep::DisableWhitelist,
            "EVM",
            "disable_whitelist",
            "disabled",
            json!(true),
        ),
    ] {
        assert_eq!(step.to_json()[field], expected);
        let plan = rtao::Executor::new(client.clone())
            .plan_dyn(&step, &keys, &PlanOptions::default())
            .await?;
        assert!(plan.ok());
        let decoded = client
            .decode_scale("Call", plan.call().as_bytes())
            .await?
            .to_json();
        assert_eq!(decoded["call_module"], "Sudo");
        assert_eq!(decoded["call_function"], "sudo");
        let inner = &decoded["call_args"][0]["value"];
        assert_eq!(inner["call_module"], pallet);
        assert_eq!(inner["call_function"], name);
        assert_eq!(inner["call_args"][0]["name"], field);
        assert_eq!(inner["call_args"][0]["value"], expected);
        assert_eq!(plan.signer, SignerRole::Coldkey);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 0);
    client.close().await;
    Ok(())
}

// Retain the production SetupBackend::submit implementation. Only HTTP and
// public-key reads are faked, so the test traverses AppContext's real review,
// signing, exact-call binding, receipt rendering and forwarded recovery hooks.
struct NativeSubmit(Fake);
impl SetupBackend for NativeSubmit {
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, evm::Result<Value>> {
        self.0.request(method, params)
    }
    fn stored_keys<'a>(
        &'a self,
        ctx: &'a AppContext,
    ) -> BoxFuture<'a, Result<Vec<EvmKeyInfo>, CliError>> {
        self.0.stored_keys(ctx)
    }
    fn key_info<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
    ) -> BoxFuture<'a, Result<EvmKeyInfo, CliError>> {
        self.0.key_info(ctx, key)
    }
}
#[derive(Default)]
struct Trace {
    events: Mutex<Vec<&'static str>>,
    plans: Mutex<Vec<Vec<u8>>>,
    receipts: Mutex<Vec<Value>>,
}
impl Trace {
    fn event(&self, event: &'static str) -> Result<(), CliError> {
        self.events
            .lock()
            .map_err(|_| CliError::failure("trace lock"))?
            .push(event);
        Ok(())
    }
}
impl SubmissionHooks for Trace {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async {
            Err(CliError::failure(
                "setup must forward the actual reviewed plan",
            ))
        })
    }
    fn before_submit_with_plan<'a>(
        &'a self,
        plan: &'a Plan,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.event("reviewed")?;
            self.plans
                .lock()
                .map_err(|_| CliError::failure("plans lock"))?
                .push(plan.call().as_bytes().to_vec());
            Ok(())
        })
    }
    fn on_result<'a>(&'a self, result: &'a ExtrinsicResult) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.event("receipt")?;
            self.receipts
                .lock()
                .map_err(|_| CliError::failure("receipts lock"))?
                .push(result.to_json());
            Ok(())
        })
    }
}
struct NativeKeys {
    signer: KeypairSigner,
    trace: Arc<Trace>,
}
impl KeyView for NativeKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
    }
}
impl DynKeySource for NativeKeys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.trace
                .event("unlock")
                .map_err(|error| SignerError::KeyView(error.message))?;
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
    }
}
struct NativeFactory(Arc<NativeKeys>);
impl ExternalSignerFactory for NativeFactory {
    fn keys<'a>(
        &'a self,
        _: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Coldkey);
            Ok(self.0.clone() as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a Globals,
        _: &'a dyn DynKeySource,
        _: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move { self.0.trace.event("release") })
    }
}

#[tokio::test]
async fn default_native_submission_preserves_reviewed_calls_and_hooks_for_both_steps() -> TestResult
{
    let trace = Arc::new(Trace::default());
    let keys = Arc::new(NativeKeys {
        signer: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
        trace: trace.clone(),
    });
    let factory = NativeFactory(keys.clone());
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.signer.account())],
        json!({"data":{"free":1_000_000_000u64}}),
    )?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let backend = NativeSubmit(Fake::chain(json!("0x2a"))?);
    let mut f = Fixture::new(&[
        "rtao",
        "evm",
        "setup-localnet",
        "--network",
        "local",
        "--yes",
        "--json",
        "--signer",
        "extension",
        "--no-mev-shield",
    ])?;
    let (name, leaf) = f
        .matches
        .subcommand()
        .and_then(|(_, group)| group.subcommand())
        .ok_or("setup route")?;
    let network = evm::networks::evm_network_with_env("local", |_| None)?;
    setup::run_with(
        &mut f.ctx,
        name,
        leaf,
        &network,
        &backend,
        SetupExecution {
            client: Some(&client),
            options: SubmissionOptions {
                hooks: Some(trace.as_ref()),
                ..Default::default()
            },
            external: Some(&factory),
            history: &mut PromptHistory::default(),
        },
    )
    .await?;
    assert_eq!(
        *trace.events.lock().map_err(|_| "trace lock")?,
        [
            "reviewed", "unlock", "receipt", "release", "reviewed", "unlock", "receipt", "release"
        ]
    );
    let plans = trace.plans.lock().map_err(|_| "plans lock")?.clone();
    let receipts = trace.receipts.lock().map_err(|_| "receipts lock")?.clone();
    assert_eq!(plans.len(), 2);
    assert_eq!(receipts.len(), 2);
    let text = f.sink.text(Stream::Stdout)?;
    let rendered = serde_json::Deserializer::from_str(&text)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(rendered, receipts);
    let runtime = client.refresh_runtime().await?;
    let submissions = chain.mutate_state(|state| state.submissions.clone());
    assert_eq!(submissions.len(), 2);
    for (index, encoded) in submissions.iter().enumerate() {
        let decoded = runtime.core().decode_extrinsic(encoded, true)?;
        let decoded = rtao::core::DynValue::new(decoded).to_json();
        let reviewed = client.decode_scale("Call", &plans[index]).await?.to_json();
        assert_eq!(decoded["call"], reviewed);
    }
    backend.0.done()?;
    client.close().await;
    Ok(())
}
