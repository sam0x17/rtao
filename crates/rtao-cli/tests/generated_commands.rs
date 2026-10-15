// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/tx.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/cli/query.py and cli/stake_picker.py.
//! Generated parser and SDK dispatch exercised with authenticated FakeChain state.

use rtao::{
    client::{Client, ClientConfig, HeadProvider, Network, View},
    primitives::{AccountId, CryptoType, NetUid},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{ExtrinsicResult, IntentInterface},
};
use rtao_cli::{
    application,
    books::Books,
    commands::{
        catalog,
        generated::{self, QueryRequest, TxRequest},
        schema,
    },
    context::{AppContext, SubmissionHooks, SubmissionOptions},
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
type TestResult = Result<(), Box<dyn std::error::Error>>;

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
fn tree() -> Result<clap::Command, rtao_cli::error::CliError> {
    Ok(application::command_tree([
        generated::tx_command()?,
        generated::query_command()?,
        catalog::tools_command(),
    ]))
}
fn context(
    parsed: &clap::ArgMatches,
    reserved: &[&str],
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("family")?.1;
    let leaf = group.subcommand().map(|(_, leaf)| leaf).unwrap_or(group);
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/generated-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, reserved, |_| None)?;
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
#[derive(Clone)]
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
}
impl Keys {
    fn new() -> Result<Self, SignerError> {
        Ok(Self {
            key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
            unlocks: Arc::new(AtomicUsize::new(0)),
        })
    }
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
        Box::pin(async {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
#[derive(Default)]
struct Hooks {
    before: AtomicUsize,
    results: AtomicUsize,
    public_preparation_unlocks: Option<Arc<AtomicUsize>>,
}
impl SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            if let Some(unlocks) = &self.public_preparation_unlocks {
                assert_eq!(unlocks.load(Ordering::SeqCst), 0);
            }
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        _: &'a ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            self.results.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
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
fn funded(keys: &Keys) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":1_000_000_000_000_u64}}),
    )?;
    Ok(state)
}
fn leaf(parsed: &clap::ArgMatches) -> Result<&clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(parsed
        .subcommand()
        .ok_or("family")?
        .1
        .subcommand()
        .ok_or("operation")?
        .1)
}

#[test]
fn advertisement_distinguishes_real_behavior_and_compiled_feature_availability() -> TestResult {
    let tx = generated::tx_command()?;
    assert!(tx.find_subcommand("transfer").is_some());
    assert!(tx.find_subcommand("set-identity").is_none());
    assert!(tx.find_subcommand("add-stake").is_none());
    let rows = rtao_intents::interface_metadata();
    let pending = rows
        .iter()
        .find(|row| row.name == "set_identity")
        .ok_or("pending interface")?;
    assert!(pending.compiled && !pending.implemented);
    assert!(pending.type_name.is_some());
    let staking = rows
        .iter()
        .find(|row| row.name == "add_stake")
        .ok_or("staking interface")?;
    assert!(!staking.implemented);
    // Workspace --all-features enables staking too. Its pending body must
    // remain unadvertised whether the interface is compiled or disabled.
    assert_eq!(staking.type_name.is_some(), staking.compiled);
    assert_eq!(staking.role.is_some(), staking.compiled);
    assert_eq!(staking.output.is_some(), staking.compiled);
    for row in &rows {
        let command = row.name.replace('_', "-");
        assert_eq!(
            tx.find_subcommand(&command).is_some(),
            row.compiled && row.implemented,
            "{}",
            row.name
        );
        assert_eq!(row.type_name.is_some(), row.compiled, "{}", row.name);
        assert_eq!(row.role.is_some(), row.compiled, "{}", row.name);
        assert_eq!(row.output.is_some(), row.compiled, "{}", row.name);
        if !row.compiled || !row.implemented {
            assert!(generated::intent_command(row.name).is_err(), "{}", row.name);
        }
    }
    let transfer = rows
        .iter()
        .find(|row| row.name == "transfer")
        .ok_or("transfer")?;
    assert_eq!(
        transfer.type_name,
        Some(std::any::type_name::<rtao_intents::Transfer>())
    );
    assert_eq!(
        transfer.role,
        Some(std::any::type_name::<
            <rtao_intents::Transfer as IntentInterface>::Role,
        >())
    );
    assert_eq!(
        transfer.output,
        Some(std::any::type_name::<
            <rtao_intents::Transfer as IntentInterface>::Output,
        >())
    );
    assert!(generated::intent_command("set_identity").is_err());
    assert!(generated::read_command("accepts_locked_alpha".parse()?).is_err());
    let balance: rtao::reads::ReadName = "balance".parse()?;
    assert!(balance.implemented());
    assert_eq!(
        balance.interface_metadata().output_type_name,
        std::any::type_name::<rtao::primitives::DynBalance>()
    );
    Ok(())
}

#[test]
fn catalog_interface_metadata_covers_every_operation_and_pending_body() -> TestResult {
    let pending: toml_edit::DocumentMut = include_str!("../../../parity/pending.toml").parse()?;
    let pending_intents = pending["intents"]
        .as_array()
        .ok_or("pending intents")?
        .iter()
        .map(|name| name.as_str().ok_or("pending intent name"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let pending_reads = pending["reads"]
        .as_array()
        .ok_or("pending reads")?
        .iter()
        .map(|name| name.as_str().ok_or("pending read name"))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let rows = rtao_intents::interface_metadata();
    assert_eq!(rows.len(), rtao_catalog::INTENTS.len());
    assert_eq!(
        rows.iter().map(|row| row.name).collect::<BTreeSet<_>>(),
        rtao_catalog::INTENTS
            .iter()
            .map(|spec| spec.name)
            .collect::<BTreeSet<_>>()
    );
    for row in rows {
        assert_eq!(row.implemented, !pending_intents.contains(row.name));
        assert_eq!(row.type_name.is_some(), row.compiled);
        assert_eq!(row.role.is_some(), row.compiled);
        assert_eq!(row.output.is_some(), row.compiled);
    }
    let reads = rtao::reads::ReadName::ALL;
    assert_eq!(reads.len(), rtao_catalog::READS.len());
    assert_eq!(
        reads
            .iter()
            .map(|name| name.as_str())
            .collect::<BTreeSet<_>>(),
        rtao_catalog::READS
            .iter()
            .map(|spec| spec.name)
            .collect::<BTreeSet<_>>()
    );
    for read in reads {
        let metadata = read.interface_metadata();
        assert_eq!(metadata.implemented, !pending_reads.contains(read.as_str()));
        assert_eq!(read.implemented(), metadata.implemented);
        assert!(!metadata.params_type_name.is_empty());
        assert!(!metadata.output_type_name.is_empty());
    }
    Ok(())
}

#[test]
fn tools_retain_every_upstream_field_and_leave_pending_cli_null() -> TestResult {
    let tools = catalog::tools_payload(&generated::rust_tools()?)?;
    let tools = tools.as_array().ok_or("tools")?;
    assert_eq!(tools.len(), rtao_catalog::INTENTS.len());
    for spec in &rtao_catalog::INTENTS {
        let mut actual = tools
            .iter()
            .find(|row| row["name"] == json!(spec.name))
            .ok_or("catalog row")?
            .clone();
        actual.as_object_mut().ok_or("tool object")?.remove("rust");
        let mut expected: Value = serde_json::from_str(spec.catalog_json)?;
        expected["docs_url"] = json!(format!(
            "{}/tx/{}",
            rtao_catalog::py::DOCS_URL,
            spec.name.replace('_', "-")
        ));
        assert_eq!(actual, expected, "{}", spec.name);
    }
    let pending = tools
        .iter()
        .find(|row| row["name"] == json!("set_identity"))
        .ok_or("pending")?;
    assert!(pending["rust"]["type"].is_string());
    assert!(pending["rust"]["cli"].is_null());
    Ok(())
}

#[test]
fn exact_money_keeps_native_u64_precision_exponents_and_all_policy() -> TestResult {
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "transfer")
        .ok_or("transfer")?;
    let fields = schema::intent_fields(spec)?;
    let money = fields
        .iter()
        .find(|field| field.name == "amount_tao")
        .ok_or("amount")?;
    assert_eq!(
        schema::coerce(money, "18446744073.709551615")?,
        json!("18446744073.709551615")
    );
    assert_eq!(schema::coerce(money, "1e-9")?, json!("0.000000001"));
    for value in ["18446744073.709551616", "-1", "NaN", "Infinity", "1e100"] {
        assert!(schema::coerce(money, value).is_err(), "{value}");
    }
    let mut allowed = money.clone();
    allowed.allow_all = true;
    assert_eq!(schema::coerce(&allowed, " ALL ")?, json!("all"));
    allowed.allow_all = false;
    assert!(schema::coerce(&allowed, "all").is_err());
    let alpha_spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "transfer_stake")
        .ok_or("alpha intent")?;
    let alpha = schema::intent_fields(alpha_spec)?
        .into_iter()
        .find(|field| field.name == "amount_alpha")
        .ok_or("alpha")?;
    assert!(alpha.help.contains("alpha"));
    assert!(money.help.contains("TAO"));
    assert_eq!(schema::coerce(&alpha, "0.000000001")?, json!("0.000000001"));
    Ok(())
}

#[test]
fn tx_lists_accept_csv_and_json_and_mapping_values_preserve_integer_digits() -> TestResult {
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "set_weights")
        .ok_or("set_weights")?;
    let fields = schema::intent_fields(spec)?;
    let uids = fields
        .iter()
        .find(|field| field.name == "uids")
        .ok_or("uids")?;
    assert_eq!(schema::coerce(uids, " 1, 2,,٣ ")?, json!([1, 2, 3]));
    assert_eq!(schema::coerce(uids, "[1,2,3]")?, json!([1, 2, 3]));
    let weights = fields
        .iter()
        .find(|field| field.name == "weights")
        .ok_or("weights")?;
    assert_eq!(
        schema::coerce(weights, r#"{"1":0.5,"9":1.0}"#)?,
        json!({"1":0.5,"9":1.0})
    );
    assert!(schema::coerce(weights, "{bad}").is_err());
    let mut integer = uids.clone();
    integer.kind = "integer".into();
    integer.annotation = "int";
    assert_eq!(
        schema::coerce(&integer, "+18_446_744_073_709_551_615")?,
        json!(u64::MAX)
    );
    assert!(schema::coerce(&integer, "1.1").is_err());
    Ok(())
}

#[test]
fn parser_aliases_boolean_twins_defaults_and_leaf_globals_use_the_real_schema() -> TestResult {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "-n",
        "finney",
        "tx",
        "transfer",
        "--dest",
        "contact",
        "--amount",
        "1.000000001",
        "--no-keep-alive",
        "-n",
        "local",
        "--yes",
        "--json",
    ])?;
    let reserved = generated::intent_reserved("transfer")?;
    let (mut ctx, _) = context(&parsed, &reserved)?;
    let dest = AccountId([7; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"contact","address":dest}]),
        json!([]),
        json!([]),
    );
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(ctx.globals.network, "local");
    assert_eq!(request.args["dest_ss58"], json!(dest));
    assert_eq!(request.args["amount_tao"], json!("1.000000001"));
    assert_eq!(request.args["keep_alive"], json!(false));
    assert_eq!(request.intent()?.to_json()["keep_alive"], json!(false));
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "tx", "transfer", "--amount", "not-money"])
            .is_err()
    );
    let snake = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer_all",
        "--dest",
        "contact",
        "--yes",
    ])?;
    assert_eq!(
        snake
            .subcommand()
            .ok_or("tx")?
            .1
            .subcommand()
            .ok_or("op")?
            .0,
        "transfer-all"
    );
    Ok(())
}

#[test]
fn field_collision_reserves_leaf_force_proxy_type_without_rewriting_outer_route() -> TestResult {
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "execute_proxy_announced")
        .ok_or("proxy schema")?;
    let fields = schema::intent_fields(spec)?;
    let probe = with_globals(
        schema::arguments(clap::Command::new("probe"), &fields, true),
        Tier::Tx,
    );
    let root = application::command_tree([probe]);
    let parsed =
        root.try_get_matches_from(["rtao", "probe", "--force-proxy-type", "NonTransfer"])?;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/generated-home"),
        |_| None,
    )?;
    // A caller's already-selected outer proxy route must remain separate
    // from the generated intent's identically named semantic argument.
    globals.force_proxy_type = Some("Any".into());
    let leaf = parsed.subcommand().ok_or("probe")?.1;
    globals.apply_with_env(
        leaf,
        &fields.iter().map(|field| field.name).collect::<Vec<_>>(),
        |_| None,
    )?;
    assert_eq!(globals.force_proxy_type.as_deref(), Some("Any"));
    assert_eq!(
        schema::values(leaf, &fields)?["force_proxy_type"],
        json!("NonTransfer")
    );
    Ok(())
}

#[test]
fn missing_required_values_refuse_with_input_status_before_any_connection() -> TestResult {
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "transfer", "--json"])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let error = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )
    .err()
    .ok_or("missing fields")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("--dest") && error.message.contains("--amount-tao"));
    assert!(sink.text(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn generated_transfer_dry_run_uses_sdk_plan_and_never_unlocks_or_runs_hooks() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let dest = AccountId([7; 32]).to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &dest,
        "--amount",
        "1.000000001",
        "--period",
        "64",
        "--dry-run",
        "--json",
        "--yes",
    ])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    let hooks = Hooks::default();
    let receipt = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            replay: Some("rtao tx transfer --amount 1.000000001 --period 64"),
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?;
    assert!(receipt.is_none());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["args"]["amount_tao"], json!("1.000000001"));
    assert!(
        plan["effects"]
            .as_array()
            .ok_or("effects")?
            .iter()
            .any(|value| value == "mortal transaction period: 64 blocks")
    );
    assert!(
        plan["command"]
            .as_str()
            .is_some_and(|command| command.contains("--period 64"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn generated_transfer_submits_real_semantic_bytes_hooks_and_requested_period() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let dest = AccountId([7; 32]).to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &dest,
        "--amount",
        "1.000000001",
        "--period",
        "64",
        "--json",
        "--yes",
    ])?;
    let (mut ctx, sink) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    let hooks = Hooks::default();
    let receipt = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?
    .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let runtime = View::runtime(&client).await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    assert_eq!(decoded["era"][0], json!(64));
    assert_eq!(decoded["call"]["call_module"], json!("Balances"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn invalid_period_refuses_before_unlock_and_any_submission() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let request = TxRequest {
        op: "transfer",
        args: serde_json::Map::from_iter([
            ("dest_ss58".into(), json!(AccountId([7; 32]))),
            ("amount_tao".into(), json!("1")),
        ]),
        period: Some(3),
    };
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "transfer", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let error = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await
    .err()
    .ok_or("invalid period")?;
    assert_eq!(error.code, 2);
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

#[tokio::test]
async fn block_query_pins_real_storage_and_does_not_read_the_current_head_value() -> TestResult {
    let keys = Keys::new()?;
    let mut state = funded(&keys)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":10}}),
    )?;
    let old = state.block;
    state.advance(1);
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":20}}),
    )?;
    let (client, chain) = client(state).await?;
    let address = keys.key.account().to_string();
    let block = old.to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "query",
        "balance",
        "--coldkey",
        &address,
        "--block",
        &block,
        "--json",
    ])?;
    let name = "balance".parse()?;
    let (ctx, sink) = context(&parsed, &generated::read_reserved(name))?;
    let request =
        QueryRequest::from_matches(&ctx, name, leaf(&parsed)?, &mut PromptHistory::default())?;
    let payload = generated::execute_query(&client, &request, &ctx.output).await?;
    assert_eq!(payload, json!("τ0.000000010"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    let head = QueryRequest {
        block: None,
        ..request
    };
    let (other, _) = {
        let sink = MemorySink::default();
        (
            Output::new(
                OutputOptions {
                    json_mode: true,
                    ..Default::default()
                },
                Arc::new(sink.clone()),
            ),
            sink,
        )
    };
    assert_eq!(
        generated::execute_query(&client, &head, &other).await?,
        json!("τ0.000000020")
    );
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

#[test]
fn optional_own_key_null_defaults_survive_and_query_arrays_use_csv_rules() -> TestResult {
    let optional = rtao::reads::ReadName::ALL
        .into_iter()
        .find(|name| {
            schema::read_fields(*name).is_ok_and(|fields| {
                fields
                    .iter()
                    .any(|field| field.own_key && field.preserve_null())
            })
        })
        .ok_or("optional own key read")?;
    let fields = schema::read_fields(optional)?;
    // Signature defaults remain independently inspectable without touching a wallet.
    assert!(
        fields
            .iter()
            .any(|field| field.own_key && field.preserve_null())
    );
    let name: rtao::reads::ReadName = "balances".parse()?;
    let array = schema::read_fields(name)?
        .into_iter()
        .find(|field| field.name == "coldkey_ss58s")
        .ok_or("array")?;
    assert_eq!(
        schema::coerce(&array, " alice, bob ")?,
        json!(["alice", "bob"])
    );
    assert_eq!(
        schema::coerce(&array, "[alice,bob]")?,
        json!(["[alice", "bob]"])
    );
    Ok(())
}

#[test]
fn account_list_queries_resolve_every_reference_and_refuse_an_unknown_entry() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    for (command, parameter, flag) in [
        ("balances", "coldkey_ss58s", "--coldkeys"),
        ("hotkey-identities", "hotkey_ss58s", "--hotkeys"),
        ("stake-value-for-coldkeys", "coldkey_ss58s", "--coldkeys"),
    ] {
        let read = command.replace('-', "_").parse()?;
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "query",
            command,
            flag,
            "bob, alice, bob",
            "--json",
        ])?;
        let (mut ctx, _) = context(&parsed, &generated::read_reserved(read))?;
        ctx.books = Books::from_values(
            json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
            json!([]),
            json!([]),
        );
        let request =
            QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())?;
        // Each entry is resolved independently; input order and duplicates
        // survive so the read receives exactly the requested account list.
        assert_eq!(request.params[parameter], json!([bob, alice, bob]));
        let field = schema::read_fields(read)?
            .into_iter()
            .find(|field| field.name == parameter)
            .ok_or("account-list field")?;
        assert!(field.help.contains("address-book"));
        assert!(field.help.contains("Comma-separated"));
        assert!(!field.help.contains("Defaults to your wallet"));

        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "query",
            command,
            flag,
            "alice,not-in-book",
            "--json",
        ])?;
        let error =
            QueryRequest::from_matches(&ctx, read, leaf(&parsed)?, &mut PromptHistory::default())
                .err()
                .ok_or("unknown list entry must be refused before connecting")?;
        assert_eq!(error.code, 2);
    }
    Ok(())
}

#[tokio::test]
async fn query_resolves_csv_contact_names_before_actual_batched_read_dispatch() -> TestResult {
    let keys = Keys::new()?;
    let alice = keys.key.account();
    let bob = AccountId([8; 32]);
    let mut state = funded(&keys)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(bob)],
        json!({"data":{"free":42}}),
    )?;
    let (client, _) = client(state).await?;
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "query",
        "balances",
        "--coldkeys",
        "alice,bob",
        "--json",
    ])?;
    let name = "balances".parse()?;
    let (mut ctx, sink) = context(&parsed, &generated::read_reserved(name))?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
        json!([]),
        json!([]),
    );
    let request =
        QueryRequest::from_matches(&ctx, name, leaf(&parsed)?, &mut PromptHistory::default())?;
    assert_eq!(request.params["coldkey_ss58s"], json!([alice, bob]));
    let result = generated::execute_query(&client, &request, &ctx.output).await?;
    assert_eq!(result[&bob.to_string()], json!("τ0.000000042"));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result
    );
    client.close().await;
    Ok(())
}

#[test]
fn human_grouped_or_matrix_projection_never_changes_machine_json() -> TestResult {
    let name = rtao::reads::ReadName::ALL
        .into_iter()
        .find(|name| name.implemented() && name.spec().render.is_some())
        .ok_or("render hint")?;
    let payload = match name.spec().render {
        Some(rtao::reads::RenderHint::Grouped { .. }) => {
            json!({"owner":[{"amount":"btcli user data","raw":{"opaque":true}}]})
        }
        Some(rtao::reads::RenderHint::Matrix { .. }) => json!({"row":{"column":0.25}}),
        _ => return Err("missing hint".into()),
    };
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    generated::render_query(&output, name, &payload)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        payload
    );
    Ok(())
}

#[test]
fn hyperparameter_union_preserves_text_bool_and_exact_integer_until_native_conversion() -> TestResult
{
    for (name, input, expected) in [
        ("epoch_consensus", "Yuma", json!("Yuma")),
        ("registration_allowed", "true", json!(1)),
        (
            "weights_version",
            "9007199254740993",
            json!(9_007_199_254_740_993u64),
        ),
    ] {
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "tx",
            "set-hyperparameter",
            "--netuid",
            "1",
            "--name",
            name,
            "--value",
            input,
            "--yes",
        ])?;
        let (mut ctx, _) = context(&parsed, &generated::intent_reserved("set_hyperparameter")?)?;
        let request = TxRequest::from_matches(
            &mut ctx,
            "set_hyperparameter",
            leaf(&parsed)?,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(request.args["value"], json!(input));
        assert_eq!(request.intent()?.to_json()["value"], expected);
    }
    // This union's native field is Number. It is discovered from generator
    // field_type rather than treating every union as a floating point number.
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--netuid",
        "1",
        "--work-block",
        "100",
        "--nonce",
        "0",
        "--work-hex",
        &"00".repeat(32),
        "--hotkey",
        &AccountId([7; 32]).to_string(),
        "--mining-timeout",
        "9007199254740993",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "pow_register",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(
        request.intent()?.to_json()["mining_timeout"],
        json!(9_007_199_254_740_993u64)
    );
    Ok(())
}

#[test]
fn all_overrides_trimmed_empty_amount_before_native_money_validation() -> TestResult {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "--dest",
        &AccountId([7; 32]).to_string(),
        "--all",
        "--amount",
        "  ",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let request = TxRequest::from_matches(
        &mut ctx,
        "transfer",
        leaf(&parsed)?,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(request.args["amount_tao"], json!("all"));
    assert_eq!(request.intent()?.to_json()["amount_tao"], json!("all"));
    Ok(())
}

struct PromptTerminal {
    answers: VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}
impl Terminal for PromptTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(label.into());
        self.answers.pop_front().ok_or(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(message.into());
        Ok(())
    }
}

#[tokio::test]
async fn transfer_amount_prompt_reads_proxy_free_balance_and_records_public_replay() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(funded(&keys)?).await?;
    let address = keys.key.account().to_string();
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "transfer",
        "-w",
        "chosen",
        "--dest",
        &address,
        "--proxy-for",
        &address,
    ])?;
    let (mut ctx, _) = context(&parsed, &generated::intent_reserved("transfer")?)?;
    let messages = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(PromptTerminal {
        answers: VecDeque::from(["1.000000001".into()]),
        messages: messages.clone(),
    })));
    let mut history = PromptHistory::default();
    let request = TxRequest::from_matches_with_client(
        &mut ctx,
        &client,
        "transfer",
        leaf(&parsed)?,
        &mut history,
    )
    .await?;
    assert_eq!(request.args["amount_tao"], json!("1.000000001"));
    let messages = messages.lock().map_err(|_| "messages")?;
    let hint = messages
        .iter()
        .position(|s| s.contains("free balance of") && s.contains("1000.000000000"))
        .ok_or("live free balance hint")?;
    let prompt = messages
        .iter()
        .position(|s| s.starts_with("  amount-tao"))
        .ok_or("amount prompt")?;
    assert!(hint < prompt);
    assert!(
        history
            .replay_command(&["rtao".into(), "tx".into(), "transfer".into()])
            .contains("--amount-tao 1.000000001")
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    drop(messages);
    client.close().await;
    Ok(())
}

fn pow_state(keys: &Keys) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::new(FakeState::try_default()?.runtime);
    state.set_finalized(98)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(keys.key.account())],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    let netuid = NetUid(1);
    for (item, enabled) in [
        ("NetworksAdded", true),
        ("NetworkRegistrationAllowed", false),
        ("NetworkPowRegistrationAllowed", true),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), enabled)?;
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
    Ok(state)
}
async fn pow_request(
    client: &Client,
    keys: &Keys,
) -> Result<TxRequest, Box<dyn std::error::Error>> {
    let hash = client.rpc().block_hash(100).await?.ok_or("work hash")?;
    let prefix = rtao_intents::pow::registration_prefix(
        NetUid(1),
        hash,
        keys.key.account(),
        keys.key.account(),
    )?;
    let work = rtao::core::pow::seal(&prefix, 0)?;
    Ok(TxRequest {
        op: "pow_register",
        args: serde_json::Map::from_iter([
            ("netuid".into(), json!(1)),
            ("hotkey_ss58".into(), json!(keys.key.account())),
            ("work_block".into(), json!(100)),
            ("nonce".into(), json!(0)),
            ("work_hex".into(), json!(hex::encode(work))),
            ("mining_workers".into(), json!(1)),
            ("mining_timeout".into(), json!(10)),
        ]),
        period: None,
    })
}

#[tokio::test]
async fn initially_expired_future_and_difficulty_invalid_pow_prepare_before_signing() -> TestResult
{
    for (work_block, difficulty) in [(95u64, 1u64), (105, 1), (100, 2)] {
        let keys = Keys::new()?;
        let owner = AccountId([48; 32]);
        let hotkey = AccountId([47; 32]);
        assert_ne!(keys.key.account(), owner);
        assert_ne!(keys.key.account(), hotkey);
        let mut state = pow_state(&keys)?;
        state.seed_typed("SubtensorModule", "Difficulty", (NetUid(1),), difficulty)?;
        let (client, chain) = client(state).await?;
        let mut request = pow_request(&client, &keys).await?;
        let hash = client
            .rpc()
            .block_hash(work_block.min(100))
            .await?
            .ok_or("original work hash")?;
        let prefix = rtao_intents::pow::registration_prefix(NetUid(1), hash, hotkey, owner)?;
        let nonce = if difficulty == 1 {
            0
        } else {
            let mut invalid = None;
            for candidate in 0..1000 {
                if rtao::core::pow::mine_cpu(&prefix, difficulty, candidate, 1)?.is_none() {
                    invalid = Some(candidate);
                    break;
                }
            }
            invalid.ok_or("difficulty-invalid fixture nonce")?
        };
        request.args.insert("work_block".into(), json!(work_block));
        request.args.insert("nonce".into(), json!(nonce));
        request.args.insert("hotkey_ss58".into(), json!(hotkey));
        request.args.insert(
            "work_hex".into(),
            json!(hex::encode(rtao::core::pow::seal(&prefix, nonce)?)),
        );
        let original = request.intent()?;
        assert!(
            rtao::Executor::new(client.clone())
                .plan_dyn(
                    original.as_dyn(),
                    &keys,
                    &rtao::tx::PlanOptions {
                        proxy_for: Some(owner),
                        ..Default::default()
                    },
                )
                .await
                .is_err()
        );
        let owner_flag = owner.to_string();
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "tx",
            "pow-register",
            "--json",
            "--yes",
            "--proxy-for",
            &owner_flag,
        ])?;
        let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
        let hooks = Hooks {
            public_preparation_unlocks: Some(keys.unlocks.clone()),
            ..Default::default()
        };
        let result = generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
        )
        .await?
        .ok_or("receipt")?;
        assert!(result.success);
        assert_eq!(result.data["proxy_for"], json!(owner));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
        assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
        assert_eq!(request.intent()?, original);
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
        assert_eq!(proxy["call_function"], json!("proxy"));
        let proxy_args = proxy["call_args"].as_array().ok_or("proxy args")?;
        let real = proxy_args
            .iter()
            .find(|arg| arg["name"] == "real")
            .map(|arg| &arg["value"])
            .ok_or("proxy owner")?;
        assert_eq!(real, &json!(owner));
        let call = proxy_args
            .iter()
            .find(|arg| arg["name"] == "call")
            .map(|arg| &arg["value"])
            .ok_or("inner call")?;
        assert_eq!(call["call_function"], json!("pow_register"));
        let args = call["call_args"].as_array().ok_or("call args")?;
        let arg = |name: &str| {
            args.iter()
                .find(|arg| arg["name"] == name)
                .map(|arg| &arg["value"])
        };
        assert_eq!(arg("work_block"), Some(&json!(100)));
        assert_eq!(arg("hotkey"), Some(&json!(hotkey)));
        let nonce = arg("nonce").and_then(Value::as_u64).ok_or("nonce")?;
        let hash = client.rpc().block_hash(100).await?.ok_or("fresh hash")?;
        let prefix = rtao_intents::pow::registration_prefix(NetUid(1), hash, hotkey, owner)?;
        let expected = rtao::core::pow::mine_cpu(&prefix, difficulty, nonce, 1)?
            .ok_or("submitted proof must satisfy the current difficulty")?
            .1;
        assert_eq!(
            arg("work"),
            Some(
                &rtao::core::DynValue::new(rtao::core::codec::Value::Bytes(expected.to_vec()))
                    .to_json()
            )
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn expired_pow_dry_run_never_enters_public_mining_or_unlocks() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let mut request = pow_request(&client, &keys).await?;
    request.args.insert("work_block".into(), json!(95));
    // Preparation would refuse this backend before solving. The dry run must
    // instead retain strict SDK freshness validation without entering mining.
    request
        .args
        .insert("mining_backend".into(), json!("disabled-fixture"));
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--json",
        "--yes",
        "--dry-run",
    ])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let error = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await
    .err()
    .ok_or("strict dry-run freshness refusal")?;
    assert!(error.message.contains("work block"));
    assert!(!error.message.contains("supports CPU PoW refresh"));
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

struct ExpiringKeys {
    keys: Keys,
    chain: FakeChain,
}
impl KeyView for ExpiringKeys {
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
impl DynKeySource for ExpiringKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.chain
                .advance(5)
                .map_err(|e| SignerError::KeyView(e.to_string()))?;
            self.keys.signer_for(role).await
        })
    }
}

#[tokio::test]
async fn pow_uses_real_cpu_refresh_after_unlock_delay_and_only_one_cached_signer() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let expiring = ExpiringKeys {
        keys: keys.clone(),
        chain: chain.clone(),
    };
    let hooks = Hooks::default();
    let result = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &expiring,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await?
    .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
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
    assert_eq!(decoded["call"]["call_function"], json!("pow_register"));
    let args = decoded["call"]["call_args"].as_array().ok_or("call args")?;
    let work_block = args
        .iter()
        .find(|arg| arg["name"] == "work_block")
        .ok_or("work block")?;
    assert_eq!(work_block["value"], json!(105));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pow_preview_and_fresh_invalid_seal_cannot_start_signing_or_replace_the_proof() -> TestResult
{
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let mut request = pow_request(&client, &keys).await?;
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "pow-register",
        "--json",
        "--yes",
        "--dry-run",
    ])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    assert!(
        generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await?
        .is_none()
    );
    request
        .args
        .insert("work_hex".into(), json!("00".repeat(32)));
    let original = request.intent()?;
    let refreshed =
        rtao_cli::commands::pow::refresh_if_needed(&client, original.clone(), &keys).await?;
    assert_eq!(refreshed, original);
    assert!(
        rtao::Executor::new(client.clone())
            .plan_dyn(refreshed.as_dyn(), &keys, &Default::default())
            .await
            .is_err()
    );
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

#[tokio::test]
async fn pow_retries_only_three_confirmed_expired_pool_rejections() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            state.advance(5);
            rtao_testkit::Reply::error(1010, "Invalid Transaction", Some(json!({"Custom":255})))
        })
    });
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, sink) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    let hooks = Hooks::default();
    let failure = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions {
            hooks: Some(&hooks),
            ..Default::default()
        },
    )
    .await
    .err()
    .ok_or("final rejected receipt")?;
    assert_eq!(failure.code, 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.results.load(Ordering::SeqCst), 1);
    let receipt: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(receipt["success"], json!(false));
    assert_eq!(receipt["error"]["name"], json!("BadRequest"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pow_fresh_pool_rejection_does_not_retry_even_when_name_matches() -> TestResult {
    let keys = Keys::new()?;
    let (client, chain) = client(pow_state(&keys)?).await?;
    let request = pow_request(&client, &keys).await?;
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = attempts.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            rtao_testkit::Reply::error(1010, "Invalid Transaction", Some(json!({"Custom":255})))
        })
    });
    let parsed = tree()?.try_get_matches_from(["rtao", "tx", "pow-register", "--json", "--yes"])?;
    let (ctx, _) = context(&parsed, &generated::intent_reserved("pow_register")?)?;
    assert!(
        generated::execute_tx_with_keys(
            &ctx,
            &client,
            &request,
            &keys,
            SubmissionOptions::default(),
        )
        .await
        .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    client.close().await;
    Ok(())
}
