// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/{commands/root.py,commands/root_move.py,commands/root_swap.py,root_helpers.py}
// @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Root commands prepare real intents over metadata-backed fake transports.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::AccountId,
};
use rtao_cli::{
    application,
    books::Books,
    commands::root::{self, Request},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);
const OWNER: AccountId = AccountId([7; 32]);
struct WalletRoot(PathBuf);
impl WalletRoot {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/root-command-tests")
            });
        fs::create_dir_all(&base)?;
        let root = base.join(format!(
            "root-command-wallets-{}-{}",
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
        Ok(())
    }
}
impl Drop for WalletRoot {
    fn drop(&mut self) {
        // This exact public-only fixture owns no processes, symlinks, private
        // files or services; nothing outside its unique recorded child is removed.
        if self.0.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(&format!("root-command-wallets-{}-", std::process::id()))
        }) {
            let _ = fs::remove_dir_all(&self.0);
        }
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
struct ScriptTerminal {
    answers: VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}
impl Terminal for ScriptTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
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
fn parse(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(application::command_tree([root::command()?]).try_get_matches_from(args)?)
}
fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("root group")?.1;
    let leaf = group.subcommand().ok_or("root leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/root-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &root::reserved(group)?, |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    // Public contacts avoid every filesystem or private signer dependency.
    ctx.books.addresses = vec![
        json!({"address":HOT,"name":"Straße"}),
        json!({"address":OTHER,"name":"Destination"}),
    ];
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
fn parse_request(
    ctx: &mut AppContext,
    parsed: &clap::ArgMatches,
) -> Result<Request, rtao_cli::error::CliError> {
    Request::from_matches(
        ctx,
        parsed
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing root"))?
            .1,
        &mut PromptHistory::default(),
    )
}
fn summary(hotkey: AccountId, nav: u64) -> Value {
    json!({"hotkey":hotkey,"nav_tao":nav,"spot_nav_tao":nav,"shares":1_000_000_000u64,"deposited_tao":nav,"redeemed_tao":0,"holdings":[{"netuid":0,"alpha":nav,"spot_tao":nav,"realizable_tao":nav}]})
}
fn seed() -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    state.seed_runtime("StakeInfoRuntimeApi","get_stake_info_for_coldkey",json!([
        {"hotkey":HOT,"coldkey":OWNER,"netuid":0,"stake":9_007_199_254_740_993u64,"is_registered":true},
        {"hotkey":OTHER,"coldkey":OWNER,"netuid":1,"stake":99,"is_registered":true}
    ]))?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_root_basket_positions",
        json!([[HOT, 2, 123456789], [OTHER, 3, 7]]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_validator_basket_summary",
        summary(HOT, 2_000_000_000),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_beta_pricing", Value::Null)?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_all_validator_baskets",
        json!([summary(HOT, 2_000_000_000), summary(OTHER, 7)]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_all_beta_pricing",
        json!({"pricing":[],"next":null}),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_validator_basket",
        json!([[1, 1_000_000_000, 4_000_000_000u64], [0, 123, 123]]),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi","get_basket_trading_status",json!({"enabled":true,"frozen":false,"refill_blocks":7200,"tao_available":20_000_000_000u64,"budget_tao":30_000_000_000u64}))?;
    state.seed_runtime("SwapRuntimeApi","sim_swap_alpha_for_tao",json!({"tao_amount":4_000_000_000u64,"alpha_amount":1_000_000_000,"tao_fee":1,"alpha_fee":2,"tao_slippage":3,"alpha_slippage":4}))?;
    state.seed_runtime("SwapRuntimeApi","sim_swap_tao_for_alpha",json!({"tao_amount":4_000_000_000u64,"alpha_amount":7_000_000_003u64,"tao_fee":5,"alpha_fee":6,"tao_slippage":7,"alpha_slippage":8}))?;
    for (hotkey, stake) in [(HOT, 9_007_199_254_740_993u64), (OTHER, 42)] {
        state.seed_python(
            "SubtensorModule",
            "BasketRate",
            &[json!(hotkey)],
            json!({"bits":0}),
        )?;
        state.seed_python(
            "SubtensorModule",
            "TotalHotkeyAlpha",
            &[json!(hotkey), json!(0)],
            json!(stake),
        )?;
    }
    state.seed_python(
        "SubtensorModule",
        "Keys",
        &[json!(0), json!(12)],
        json!(HOT),
    )?;
    Ok(state)
}
async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}

#[test]
fn root_flags_keep_read_and_transaction_tiers_and_early_noninteractive_errors() -> TestResult {
    let hotkey = HOT.to_string();
    let owner = OWNER.to_string();
    for args in [
        vec!["rtao", "root", "claim", "--coldkey", &owner, "--json"],
        vec!["rtao", "root", "allocate", "--amount", "1", "--json"],
        vec![
            "rtao",
            "root",
            "move",
            "--from",
            &hotkey,
            "--coldkey",
            &owner,
            "--json",
        ],
        vec![
            "rtao",
            "root",
            "list",
            &hotkey,
            "--mine",
            "--coldkey",
            &owner,
            "--json",
        ],
        vec![
            "rtao", "root", "allocate", "--hotkey", &hotkey, "--amount", "1", "--all", "--yes",
        ],
    ] {
        let parsed = parse(&args)?;
        let (mut ctx, _) = context(&parsed)?;
        let error = parse_request(&mut ctx, &parsed)
            .err()
            .ok_or("expected input refusal")?;
        assert_eq!(error.code, 2);
    }
    for invalid in ["NaN", "inf", "-1", "100.1"] {
        assert!(
            parse(&[
                "rtao",
                "root",
                "swap",
                "--hotkey",
                &hotkey,
                "--from",
                "1",
                "--to",
                "2",
                "--amount",
                "1",
                "--max-slippage",
                invalid
            ])
            .is_err()
        );
    }
    assert!(
        parse(&[
            "rtao", "root", "swap", "--hotkey", &hotkey, "--from", "4096", "--to", "0", "--amount",
            "all"
        ])
        .is_err()
    );
    for (origin, dest, amount) in [("1", "1", "all"), ("1", "2", "18446744073.709551616")] {
        let parsed = parse(&[
            "rtao", "root", "swap", "--hotkey", &hotkey, "--from", origin, "--to", dest,
            "--amount", amount, "--json",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        assert_eq!(
            parse_request(&mut ctx, &parsed)
                .err()
                .ok_or("invalid basket input")?
                .code,
            2
        );
    }
    let parsed = parse(&[
        "rtao", "rt", "register", "--hotkey", &hotkey, "--period", "16", "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Transaction(tx) = parse_request(&mut ctx, &parsed)? else {
        return Err("register transaction".into());
    };
    assert_eq!(tx.op, "root_register");
    assert_eq!(tx.period, Some(16));
    assert_eq!(tx.intent()?.to_json()["hotkey_ss58"], hotkey);
    let parsed = parse(&["rtao", "root", "list", "--json"])?;
    let (mut ctx, _) = context(&parsed)?;
    assert!(matches!(
        parse_request(&mut ctx, &parsed)?,
        Request::List {
            mine: false,
            owner: None,
            ..
        }
    ));
    Ok(())
}

#[test]
fn slippage_floor_uses_python_decimal_context_at_native_boundaries() -> TestResult {
    for (expected, percent, floor) in [
        (0, 1.0, 0),
        (101, 1.0, 99),
        (u64::MAX, 1.0, 18_262_276_632_972_456_098),
        (u64::MAX, 0.1, 18_428_297_329_635_842_063),
        (u64::MAX, 100.0, 0),
        (u64::MAX, 0.0, u64::MAX),
        (1, 5e-27, 1),
        (1, 6e-27, 0),
        (u64::MAX, 1e-100, u64::MAX),
        (7_000_000_003, 1.0, 6_930_000_002),
    ] {
        assert_eq!(
            root::minimum_out(expected, percent)?,
            floor,
            "amount={expected}, percentage={percent}"
        );
    }
    for percent in [f64::NAN, f64::INFINITY, -0.1, 100.1] {
        assert!(root::minimum_out(1, percent).is_err());
    }
    Ok(())
}

#[test]
fn interactive_explicit_position_owner_selects_its_local_signing_wallet() -> TestResult {
    let root = WalletRoot::new()?;
    root.add("A", OTHER)?;
    root.add("B", OWNER)?;
    let owner = OWNER.to_string();
    let hotkey = HOT.to_string();
    for argv in [
        vec!["rtao", "root", "claim", "--coldkey", &owner],
        vec![
            "rtao",
            "root",
            "move",
            "--from",
            &hotkey,
            "--coldkey",
            &owner,
        ],
    ] {
        let parsed = parse(&argv)?;
        let (mut ctx, _) = context(&parsed)?;
        ctx.globals.wallet_path = root.0.clone();
        ctx.globals.wallet_name = "A".into();
        ctx.globals.wallet_given = false;
        ctx.terminal = Arc::new(Mutex::new(Box::new(ScriptTerminal {
            answers: VecDeque::new(),
            messages: Arc::new(Mutex::new(Vec::new())),
        })));
        let request = parse_request(&mut ctx, &parsed)?;
        assert!(matches!(
            request,
            Request::Claim {
                owner: OWNER,
                hotkey: None
            } | Request::Move { owner: OWNER, .. }
        ));
        assert_eq!(ctx.globals.wallet_name, "B");
        assert!(ctx.globals.wallet_given);
    }
    Ok(())
}

#[tokio::test]
async fn allocation_retains_all_for_fee_and_existential_deposit_planning() -> TestResult {
    let (client, chain) = client(seed()?).await?;
    let hotkey = HOT.to_string();
    let parsed = parse(&[
        "rtao", "root", "allocate", "--hotkey", &hotkey, "--amount", " ALL ", "--all", "--json",
    ])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("allocation")?;
    assert_eq!(tx.op, "stake_into_basket");
    assert_eq!(tx.args["amount_tao"], "all");
    assert_eq!(tx.intent()?.to_json()["hotkey_ss58"], hotkey);
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn terminal_fund_picker_pages_past_twenty_and_records_the_exact_hotkey() -> TestResult {
    let mut state = seed()?;
    let funds: Vec<_> = (0..21u8)
        .map(|index| {
            summary(
                AccountId([20 + index; 32]),
                u64::from(21 - index) * 1_000_000_000,
            )
        })
        .collect();
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_all_validator_baskets",
        json!(funds),
    )?;
    let (client, chain) = client(state).await?;
    let parsed = parse(&["rtao", "root", "allocate", "--amount", "all", "--yes"])?;
    let (mut ctx, _) = context(&parsed)?;
    ctx.books.addresses = (0..21u8)
        .map(|index| json!({"address":AccountId([20+index;32]),"name":format!("Fund {index}")}))
        .collect();
    let messages = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(ScriptTerminal {
        answers: VecDeque::from([String::new(), "21".into()]),
        messages: messages.clone(),
    })));
    let request = parse_request(&mut ctx, &parsed)?;
    let mut history = PromptHistory::default();
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut history)
        .await?
        .ok_or("selected fund")?;
    let expected = AccountId([40; 32]).to_string();
    assert_eq!(tx.args["hotkey_ss58"], expected);
    assert_eq!(tx.args["amount_tao"], "all");
    let replay = history.replay_command(&[
        "rtao".into(),
        "root".into(),
        "allocate".into(),
        "--amount".into(),
        "all".into(),
    ]);
    assert!(replay.contains("--hotkey"));
    assert!(replay.contains(&expected));
    {
        let messages = messages.lock().map_err(|_| "prompt messages lock")?;
        assert!(
            messages
                .iter()
                .any(|message| message.contains("20 of 21 shown"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.starts_with("21. Fund 20")
                    && message.contains("rate")
                    && message.contains("NAV"))
        );
    }
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn claim_checks_accrued_yield_and_move_claims_then_moves_all_root_stake() -> TestResult {
    let (client, chain) = client(seed()?).await?;
    let owner = OWNER.to_string();
    let hotkey = HOT.to_string();
    let parsed = parse(&[
        "rtao",
        "root",
        "claim",
        "--hotkey",
        &hotkey,
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("claim")?;
    assert_eq!(tx.op, "claim_root_with_hotkey");
    assert_eq!(tx.args.len(), 1);
    assert_eq!(tx.args["hotkey_ss58"], hotkey);
    // Name folding matches Python's casefold (ß -> ss), and the destination is
    // independently resolved rather than accidentally reusing the source.
    let parsed = parse(&[
        "rtao",
        "root",
        "move",
        "--from",
        "STRASSE",
        "--to",
        "destination",
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("move")?;
    assert_eq!(tx.op, "move_stake");
    assert_eq!(tx.args,json!({"origin_hotkey_ss58":HOT,"origin_netuid":0,"dest_hotkey_ss58":OTHER,"dest_netuid":0,"amount_alpha":"all","claim":true}).as_object().ok_or("move object")?.clone());
    // A separate on-chain name remains a valid selector even when the local
    // display alias for this hotkey is different.
    chain.mutate_state(|state| {
        state.seed_python("SubtensorModule", "Owner", &[json!(HOT)], json!(OWNER))
    })?;
    chain.mutate_state(|state| {
        state.seed_python("SubtensorModule", "Owner", &[json!(OTHER)], json!(OTHER))
    })?;
    chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "IdentitiesV2",
            &[json!(OWNER)],
            json!({"name":"Chain Alias"}),
        )
    })?;
    chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "IdentitiesV2",
            &[json!(OTHER)],
            json!({"name":"Another Validator"}),
        )
    })?;
    let parsed = parse(&[
        "rtao",
        "root",
        "move",
        "--from",
        "CHAIN ALIAS",
        "--to",
        "destination",
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let chain_named =
        root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
            .await?
            .ok_or("chain selector")?;
    assert_eq!(chain_named.args["origin_hotkey_ss58"], json!(HOT));
    assert_eq!(chain_named.args["dest_hotkey_ss58"], json!(OTHER));
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    chain.mutate_state(|state| {
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([]),
        )
    })?;
    let parsed = parse(&[
        "rtao",
        "root",
        "claim",
        "--hotkey",
        &hotkey,
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let error =
        root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
            .await
            .err()
            .ok_or("no accrued yield")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("no accrued yield"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn swap_quotes_both_legs_sets_destination_floor_and_keeps_all_sentinel() -> TestResult {
    let (client, chain) = client(seed()?).await?;
    let hotkey = HOT.to_string();
    let parsed = parse(&[
        "rtao", "root", "swap", "--hotkey", &hotkey, "--from", "1", "--to", "2", "--amount", "all",
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let start = chain.requests().len();
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("swap")?;
    assert_eq!(tx.op, "swap_basket");
    assert_eq!(tx.args["amount"], "all");
    assert_eq!(tx.args["min_amount_out"], "6.930000002");
    assert_eq!(tx.intent()?.to_json()["dest_netuid"], 2);
    let calls: Vec<_> = chain
        .requests()
        .into_iter()
        .skip(start)
        .filter(|request| request.method == "state_call")
        .filter_map(|request| {
            request
                .params
                .get(0)
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert!(
        calls
            .iter()
            .any(|method| method == "SwapRuntimeApi_sim_swap_alpha_for_tao")
    );
    assert!(
        calls
            .iter()
            .any(|method| method == "SwapRuntimeApi_sim_swap_tao_for_alpha")
    );
    let swap_calls: Vec<_> = chain
        .requests()
        .into_iter()
        .skip(start)
        .filter(|request| request.method == "state_call")
        .filter(|request| {
            request.params[0] == "SwapRuntimeApi_sim_swap_tao_for_alpha"
                || request.params[0] == "SwapRuntimeApi_sim_swap_alpha_for_tao"
        })
        .collect();
    // Independently constructed SCALE parameter bytes prove the all holding
    // supplied the sell amount, and the sell quote's TAO supplied the buy leg.
    let sell_input = [
        1u16.to_le_bytes().as_slice(),
        1_000_000_000u64.to_le_bytes().as_slice(),
    ]
    .concat();
    let buy_input = [
        2u16.to_le_bytes().as_slice(),
        4_000_000_000u64.to_le_bytes().as_slice(),
    ]
    .concat();
    assert_eq!(swap_calls.len(), 2);
    assert_eq!(
        swap_calls[0].params[1],
        format!("0x{}", hex::encode(&sell_input))
    );
    assert_eq!(
        swap_calls[1].params[1],
        format!("0x{}", hex::encode(&buy_input))
    );
    chain.mutate_state(|state| {
        state.seed_runtime_encoded("SwapRuntimeApi_sim_swap_alpha_for_tao", sell_input, vec![])
    });
    let failed_quote =
        root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
            .await?
            .ok_or("quote failure fallback")?;
    assert_eq!(failed_quote.args["min_amount_out"], "0");
    // Root cash legs are TAO 1:1 and must bypass their AMM simulation.
    for (origin, dest, amount, expected_floor, expected_method, absent_method) in [
        (
            "0",
            "2",
            "1",
            "6.930000002",
            "SwapRuntimeApi_sim_swap_tao_for_alpha",
            "SwapRuntimeApi_sim_swap_alpha_for_tao",
        ),
        (
            "1",
            "0",
            "2",
            "3.96",
            "SwapRuntimeApi_sim_swap_alpha_for_tao",
            "SwapRuntimeApi_sim_swap_tao_for_alpha",
        ),
    ] {
        let parsed = parse(&[
            "rtao", "root", "swap", "--hotkey", &hotkey, "--from", origin, "--to", dest,
            "--amount", amount, "--json",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        let request = parse_request(&mut ctx, &parsed)?;
        let start = chain.requests().len();
        let tx =
            root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
                .await?
                .ok_or("root cash leg")?;
        assert_eq!(tx.args["min_amount_out"], expected_floor);
        let calls: Vec<_> = chain
            .requests()
            .into_iter()
            .skip(start)
            .filter(|request| request.method == "state_call")
            .map(|request| request.params[0].clone())
            .collect();
        assert!(calls.contains(&json!(expected_method)));
        assert!(!calls.contains(&json!(absent_method)));
    }
    // No holding means there is no amount to quote, matching Python's best-
    // effort floor fallback. It still delegates refusal/execution to the SDK.
    let parsed = parse(&[
        "rtao", "root", "swap", "--hotkey", &hotkey, "--from", "3", "--to", "2", "--amount", "all",
        "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    let tx = root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("unquoted swap")?;
    assert_eq!(tx.args["min_amount_out"], "0");
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn list_json_keeps_dust_positions_and_native_precision_and_uid_detail_shape() -> TestResult {
    let (client, chain) = client(seed()?).await?;
    let owner = OWNER.to_string();
    let parsed = parse(&[
        "rtao",
        "root",
        "list",
        "--mine",
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let records: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(records.as_array().ok_or("position rows")?.len(), 2);
    let records = records.as_array().ok_or("position rows")?;
    let main = records
        .iter()
        .find(|record| record["hotkey"] == json!(HOT))
        .ok_or("main position")?;
    assert_eq!(main["staked"], "τ9,007,199.254740993");
    assert_eq!(main["name"], "Straße");
    assert!(
        records
            .iter()
            .any(|record| record["hotkey"] == json!(OTHER) && record["accrued"] == "τ0.000000007")
    );
    // The detail command resolves the source UID through Keys(0,12), then
    // returns the complete summary without adding the user's position to JSON.
    let parsed = parse(&["rtao", "root", "list", "12", "--coldkey", &owner, "--json"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let detail: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(detail["hotkey"], json!(HOT));
    assert!(detail.get("holdings").is_some());
    assert!(detail.get("staked").is_none());
    let parsed = parse(&["rtao", "root", "list", "--json"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let board: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(board["funds"].as_array().ok_or("funds")?.len(), 2);
    assert_eq!(board["funds"][0]["hotkey"], json!(HOT));
    assert_eq!(board["funds"][0]["stake_tao"], "τ9,007,199.254740993");
    assert_eq!(board["funds"][1]["nav_tao"], "τ0.000000007");
    assert!(board.get("basket_index").is_some());
    // JSON returns individual checked positions before the human aggregate.
    // Two individually valid native balances must not fail because their
    // display-only all-position sum would exceed u64.
    chain.mutate_state(|state| {
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([
                {"hotkey":HOT,"coldkey":OWNER,"netuid":0,"stake":u64::MAX,"is_registered":true},
                {"hotkey":OTHER,"coldkey":OWNER,"netuid":0,"stake":1,"is_registered":true}
            ]),
        )
    })?;
    chain.mutate_state(|state| {
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([]),
        )
    })?;
    let parsed = parse(&[
        "rtao",
        "root",
        "list",
        "--mine",
        "--coldkey",
        &owner,
        "--json",
    ])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let records: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(records.as_array().ok_or("native positions")?.len(), 2);
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn human_lists_preserve_percentages_fund_notes_dust_and_detail_returns() -> TestResult {
    let (client, chain) = client(seed()?).await?;
    let owner = OWNER.to_string();
    let hotkey = HOT.to_string();
    let parsed = parse(&["rtao", "root", "list"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let stdout = sink.text(Stream::Stdout)?;
    assert!(stdout.contains("9,007,199.2547"));
    assert!(stdout.contains('%'));
    assert!(stdout.contains("β index"));
    assert!(stdout.contains("1 dust funds hidden"));
    assert!(stdout.contains("full-redemption"));
    assert!(
        sink.text(Stream::Stderr)?
            .contains("not yet in the frozen baseline table")
    );
    let parsed = parse(&["rtao", "root", "list", "--mine", "--coldkey", &owner])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    assert!(
        sink.text(Stream::Stderr)?
            .contains("1 dust position hidden (each < τ0.001")
    );
    let parsed = parse(&["rtao", "root", "list", &hotkey, "--coldkey", &owner])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let stdout = sink.text(Stream::Stdout)?;
    assert!(stdout.contains("lifetime 1.0000x"));
    assert!(stdout.contains("staker yield"));
    let parsed = parse(&[
        "rtao", "root", "allocate", "--hotkey", &hotkey, "--amount", "1", "--yes",
    ])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::prepare_transaction(&mut ctx, &client, &request, &mut PromptHistory::default())
        .await?
        .ok_or("allocation review")?;
    let stdout = sink.text(Stream::Stdout)?;
    assert!(stdout.contains("~0.5000 β"));
    assert!(stdout.contains("entry slippage and swap fees"));
    chain.mutate_state(|state| {
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([]),
        )
    })?;
    chain.mutate_state(|state| {
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_positions",
            json!([]),
        )
    })?;
    let parsed = parse(&["rtao", "root", "list", "--mine", "--coldkey", &owner])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = parse_request(&mut ctx, &parsed)?;
    root::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    assert_eq!(
        sink.text(Stream::Stderr)?,
        format!("root positions of {owner}: none\n")
    );
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}
