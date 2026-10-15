//! Wallet accounting and read-only dispatch through authenticated metadata and FakeChain.

use rtao::{
    client::{Client, ClientConfig, Network},
    core::{codec::Value as CoreValue, ops::runtime_api, runtime::TypeSpec},
    primitives::AccountId,
};
use rtao_cli::{
    application,
    books::Books,
    commands::wallet,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{
    FakeChain, FakeState,
    fakes::{RpcRequest, json_to_core},
    rpc::RpcClient,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

type TestResult = Result<(), Box<dyn Error>>;
const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);

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

struct WalletRoot(PathBuf);
impl WalletRoot {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wallet-read-tests")
            });
        fs::create_dir_all(&base)?;
        let root = base.join(format!(
            "wallet-reads-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root)?;
        Ok(Self(root))
    }

    fn add(&self, name: &str, address: AccountId) -> TestResult {
        let directory = self.0.join(name);
        fs::create_dir(&directory)?;
        fs::write(
            directory.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":address,"cryptoType":1}))?,
        )?;
        // A read command must not load or interpret this secret-file sentinel.
        fs::write(
            directory.join("coldkey"),
            b"not a keyfile; never unlock for wallet reads",
        )?;
        Ok(())
    }
}
impl Drop for WalletRoot {
    fn drop(&mut self) {
        // Fixtures spawn no processes/services and hold no open file handles.
        // Remove only this test's exact, validated child of the recorded root.
        if self.0.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(&format!("wallet-reads-{}-", std::process::id()))
        }) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn context(
    args: &[&str],
    json_mode: bool,
    root: &Path,
) -> Result<(AppContext, MemorySink, clap::ArgMatches), Box<dyn Error>> {
    let mut argv = vec!["rtao".to_owned()];
    if json_mode {
        argv.push("--json".into());
    }
    argv.extend(["--wallet-path".into(), root.to_string_lossy().into_owned()]);
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    let parsed = application::command_tree([wallet::command()]).try_get_matches_from(argv)?;
    let group = parsed.subcommand().ok_or("wallet group")?.1;
    let leaf = group.subcommand().ok_or("wallet leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/wallet-read-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &[], |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::with_sink(
        OutputOptions {
            json_mode,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink, parsed))
}

fn seed() -> Result<FakeState, Box<dyn Error>> {
    let mut state = FakeState::default();
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD)],
        json!({"data":{"free":1_250_000_000u64}}),
    )?;
    for hotkey in [HOT, OTHER] {
        state.seed_python("SubtensorModule", "Owner", &[json!(hotkey)], json!(COLD))?;
    }
    state.seed_python(
        "SubtensorModule",
        "IdentitiesV2",
        &[json!(COLD)],
        json!({"name":"Example"}),
    )?;
    state.seed_python("SubtensorModule", "TokenSymbol", &[json!(1)], json!("Ω"))?;
    let positions = json!([
        {"hotkey":HOT,"coldkey":COLD,"netuid":1,"stake":3_000_000_000u64,"is_registered":true},
        {"hotkey":OTHER,"coldkey":COLD,"netuid":0,"stake":2_000_000_000u64,"is_registered":false},
    ]);
    for (api, method, value) in [
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            positions.clone(),
        ),
        (
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([[COLD, positions]]),
        ),
        (
            "SwapRuntimeApi",
            "current_alpha_price_all",
            json!([{"netuid":1,"price":500_000_000}]),
        ),
        (
            "DelegateInfoRuntimeApi",
            "get_delegated",
            json!([[{"delegate_ss58":HOT,"owner_ss58":COLD,"take":32768},[1,1_000_000_000]]]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_owed",
            json!(250_000_000),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([
                [HOT, 1_000_000_000, 100_000_000],
                [OTHER, 2_000_000_000u64, 150_000_000]
            ]),
        ),
        (
            "BetaBasketRuntimeApi",
            "get_validator_basket_summary",
            json!({"hotkey":HOT,"shares":4_000_000_000u64,"nav_tao":8_000_000_000u64,"spot_nav_tao":9_000_000_000u64,"deposited_tao":4_000_000_000u64,"redeemed_tao":0,"holdings":[]}),
        ),
        ("BetaBasketRuntimeApi", "get_beta_pricing", Value::Null),
    ] {
        state.seed_runtime(api, method, value)?;
    }
    // JSON object keys would turn native NetUid map keys into strings. Seed
    // this response through metadata with integer keys and its exact arguments.
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let input = runtime_api::encode_params(
        &state.runtime,
        api,
        signature,
        &[
            CoreValue::List(vec![CoreValue::Bytes(COLD.0.to_vec())]),
            CoreValue::List(vec![CoreValue::Int(0), CoreValue::Int(1)]),
        ],
    )?;
    let output = state.runtime.encode_spec(
        &TypeSpec::Id(signature.output),
        &CoreValue::Dict(vec![(
            CoreValue::Bytes(COLD.0.to_vec()),
            CoreValue::Dict(vec![
                (
                    CoreValue::Int(0),
                    json_to_core(&json!({"total":2_000_000_000u64,"locked":0,"available":2_000_000_000u64}))?,
                ),
                (
                    CoreValue::Int(1),
                    json_to_core(&json!({"total":3_000_000_000u64,"locked":2_000_000_000u64,"available":1_000_000_000u64}))?,
                ),
            ]),
        )]),
    )?;
    state.seed_runtime_encoded(format!("{api}_{method}"), input, output);
    Ok(state)
}

async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn Error>> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}

async fn run(
    ctx: &mut AppContext,
    client: &Client,
    parsed: &clap::ArgMatches,
) -> Result<(), rtao_cli::error::CliError> {
    let group = parsed
        .subcommand()
        .ok_or_else(|| rtao_cli::error::CliError::input("missing wallet group"))?
        .1;
    wallet::run_with_client(ctx, client, group, &mut PromptHistory::default()).await
}

fn assert_pinned_reads(requests: &[RpcRequest]) {
    let mut hashes = BTreeSet::new();
    let mut reads = 0;
    for request in requests {
        assert!(
            !request.method.starts_with("author_"),
            "wallet reads must not submit transactions"
        );
        let index = match request.method.as_str() {
            "state_call" => 2,
            "state_getStorage" | "state_getStorageAt" | "state_queryStorageAt" => 1,
            _ => continue,
        };
        reads += 1;
        let hash = request.params.get(index).and_then(Value::as_str);
        assert!(
            hash.is_some(),
            "read must carry its pinned block hash: {request:?}"
        );
        if let Some(hash) = hash {
            hashes.insert(hash);
        }
    }
    assert!(reads > 0);
    assert_eq!(
        hashes.len(),
        1,
        "composed wallet reads must share one immutable state"
    );
}

#[tokio::test]
async fn single_balance_keeps_locked_alpha_inside_spot_value_and_beta_separate() -> TestResult {
    let address = COLD.to_string();
    let (mut ctx, sink, parsed) = context(
        &["wallet", "balance", &address],
        true,
        Path::new("/isolated/no-local-wallets"),
    )?;
    let (client, chain) = client(seed()?).await?;
    let before = chain.requests().len();
    run(&mut ctx, &client, &parsed).await?;
    let record: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(record["free"], "τ1.250000000");
    assert_eq!(record["stake_value"], "τ3.500000000");
    assert_eq!(record["beta_value"], "τ0.250000000");
    assert_eq!(record["total_value"], "τ5.000000000");
    assert_eq!(record["locked_value"], "τ1.000000000");
    assert_eq!(record["locked_subnets"], 1);
    assert_eq!(record["beta_tokens"], 3.0);
    assert_eq!(record["beta_validators"], 2);
    assert_eq!(record["stake_positions"], 2);
    assert_eq!(record["stake_subnets"], 2);
    assert_eq!(record["block"], 100);
    assert!(record.get("accrued_basket_yield").is_none());
    assert!(record.get("kind").is_none());
    assert_pinned_reads(&chain.requests()[before..]);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn inspect_preserves_position_units_basket_order_and_coldkey_wide_yield() -> TestResult {
    let address = COLD.to_string();
    let (mut ctx, sink, parsed) = context(
        &["wallet", "inspect", "--coldkey", &address],
        true,
        Path::new("/isolated/no-local-wallets"),
    )?;
    let (client, chain) = client(seed()?).await?;
    let before = chain.requests().len();
    run(&mut ctx, &client, &parsed).await?;
    let record: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(record["balance"]["total_value"], "τ5.000000000");
    assert!(record["balance"].get("locked_value").is_none());
    assert_eq!(record["stakes"][0]["stake"], "3.000000000Ω");
    assert_eq!(record["stakes"][0]["stake_unit"], "alpha (netuid 1)");
    assert_eq!(record["stakes"][0]["value"], "τ1.500000000");
    assert_eq!(record["stakes"][1]["stake_unit"], "TAO");
    assert_eq!(record["stakes"][1]["registered"], false);
    assert_eq!(record["delegations"][0]["value_tao"], 0.5);
    assert_eq!(record["baskets"][0]["hotkey"], json!(OTHER));
    assert_eq!(record["baskets"][0]["fund_share"], 0.5);
    assert_eq!(record["baskets"][0]["fund_nav"], "τ8.000000000");
    assert_eq!(record["baskets"][0]["lifetime_return"], 2.0);
    assert_eq!(record["identity"]["name"], "Example");
    for target in [
        &record,
        &record["balance"],
        &record["stakes"][0],
        &record["stakes"][1],
    ] {
        assert_eq!(target["accrued_basket_yield"], "τ0.250000000");
        assert_eq!(target["root_staked"], "τ2.000000000");
        assert_eq!(target["personal_yield"], 0.125);
        assert_eq!(target["auto_claim"], false);
    }
    assert_pinned_reads(&chain.requests()[before..]);
    assert!(
        !chain.requests()[before..]
            .iter()
            .any(|request| request.method == "state_call"
                && request.params.get(0)
                    == Some(&json!(
                        "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys"
                    )))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn all_balances_keep_duplicate_and_empty_json_rows_but_count_each_coldkey_once() -> TestResult
{
    let root = WalletRoot::new()?;
    root.add("wallet10", COLD)?;
    root.add("wallet2", COLD)?;
    root.add("wallet3", OTHER)?;
    std::fs::create_dir_all(root.0.join("empty_address"))?;
    std::fs::write(
        root.0.join("empty_address/coldkeypub.txt"),
        br#"{"ss58Address":""}"#,
    )?;
    let mut state = seed()?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    let (client, chain) = client(state).await?;
    let (mut ctx, sink, parsed) = context(
        &["wallet", "balance", "--all", "--sort", "name"],
        true,
        &root.0,
    )?;
    let before = chain.requests().len();
    run(&mut ctx, &client, &parsed).await?;
    let records: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(records.as_array().ok_or("balance rows")?.len(), 3);
    assert_eq!(records[0]["wallet"], "wallet2");
    assert_eq!(records[1]["wallet"], "wallet3");
    assert_eq!(records[2]["wallet"], "wallet10");
    assert_eq!(records[1]["total_value_tao"], 0.0);
    assert_eq!(records[0]["kind"], "wallet");
    assert!(records[0].get("locked_value").is_none());
    assert!(records[0].get("beta_tokens").is_none());
    assert_pinned_reads(&chain.requests()[before..]);
    let (mut ctx, sink, parsed) = context(&["wallet", "balance", "--all"], false, &root.0)?;
    run(&mut ctx, &client, &parsed).await?;
    let output = sink.text(Stream::Stdout)?;
    assert!(output.contains(
        "total τ4.750  (alpha at spot; beta realizable; 1 duplicate-coldkey wallets counted once)"
    ));
    assert!(!output.contains("wallet3"));
    assert!(
        sink.text(Stream::Stderr)?
            .contains("1 empty wallets hidden")
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn overflowing_total_fails_without_a_partial_payload_or_submission() -> TestResult {
    let mut state = seed()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD)],
        json!({"data":{"free":u64::MAX}}),
    )?;
    let address = COLD.to_string();
    let (mut ctx, sink, parsed) = context(
        &["wallet", "balance", &address],
        true,
        Path::new("/isolated/no-local-wallets"),
    )?;
    let (client, chain) = client(state).await?;
    let error = run(&mut ctx, &client, &parsed)
        .await
        .err()
        .ok_or("expected native balance overflow")?;
    assert!(error.message.contains("total value exceeds native u64 rao"));
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn unstaked_wallet_does_not_call_stake_availability() -> TestResult {
    let mut state = seed()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_root_basket_positions",
        json!([]),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    let address = COLD.to_string();
    let (mut ctx, sink, parsed) = context(
        &["wallet", "balance", &address],
        true,
        Path::new("/isolated/no-local-wallets"),
    )?;
    let (client, chain) = client(state).await?;
    let before = chain.requests().len();
    run(&mut ctx, &client, &parsed).await?;
    let record: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(record["stake_positions"], 0);
    assert_eq!(record["locked_value_tao"], 0.0);
    assert_eq!(record["beta_tokens"], 0.0);
    assert_eq!(record["total_value_tao"], 1.25);
    assert!(
        !chain.requests()[before..]
            .iter()
            .any(|request| request.method == "state_call"
                && request.params.get(0)
                    == Some(&json!(
                        "StakeInfoRuntimeApi_get_stake_availability_for_coldkeys"
                    )))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn inspect_keeps_dust_in_json_and_omits_yield_hint_without_root_context() -> TestResult {
    let mut state = seed()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([{"hotkey":HOT,"coldkey":COLD,"netuid":1,"stake":1,"is_registered":true}]),
    )?;
    state.seed_runtime("DelegateInfoRuntimeApi", "get_delegated", json!([]))?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_root_basket_positions",
        json!([]),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    let address = COLD.to_string();
    let (client, _) = client(state).await?;
    let (mut ctx, sink, parsed) = context(
        &["wallet", "inspect", "--coldkey", &address],
        true,
        Path::new("/isolated/no-local-wallets"),
    )?;
    run(&mut ctx, &client, &parsed).await?;
    let record: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(record["stakes"].as_array().map(Vec::len), Some(1));
    assert_eq!(record["stakes"][0]["stake"], "0.000000001Ω");
    assert_eq!(record["stakes"][0]["value_tao"], 0.0);
    assert_eq!(record["root_staked_tao"], 0.0);
    assert!(record["personal_yield"].is_null());
    let (mut ctx, sink, parsed) = context(
        &["wallet", "inspect", "--coldkey", &address],
        false,
        Path::new("/isolated/no-local-wallets"),
    )?;
    run(&mut ctx, &client, &parsed).await?;
    let human = sink.text(Stream::Stdout)?;
    assert!(!human.contains("0.000000001"));
    assert!(!human.contains("accrued basket yield"));
    assert!(sink.text(Stream::Stderr)?.contains("1 dust subnet hidden"));
    let (mut ctx, sink, parsed) = context(
        &["wallet", "inspect", "--coldkey", &address, "--dust"],
        false,
        Path::new("/isolated/no-local-wallets"),
    )?;
    run(&mut ctx, &client, &parsed).await?;
    assert!(sink.text(Stream::Stdout)?.contains("0.000000001"));
    client.close().await;
    Ok(())
}
