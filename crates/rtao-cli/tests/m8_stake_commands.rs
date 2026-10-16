// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/stake.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_exit.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Native-amount stake records and command routes over fake chain transports.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::{AccountId, DynBalance, NetUid},
    reads::reads::{
        m6_locks::ColdkeyLock,
        m6_staking::StakeAvailability,
        owner::{StakePosition, StakeValuation},
    },
};
use rtao_cli::{
    application,
    books::Books,
    commands::stake::{self, Request, exit},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

// Public-only owner fixtures use the recorded task root and owned cleanup.
#[path = "../../rtao-wallet/tests/support/mod.rs"]
mod support;
use support::TempRoot;

type TestResult = Result<(), Box<dyn std::error::Error>>;
const OWNER: AccountId = AccountId([7; 32]);
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
fn parse(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(application::command_tree([stake::command()?]).try_get_matches_from(args)?)
}
fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("stake group")?.1;
    let mut leaf = group;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/m8-stake-no-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &stake::reserved(group)?, |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    ctx.books.addresses = vec![
        json!({"name":"Owner","address":OWNER}),
        json!({"name":"Straße","address":HOT}),
        json!({"name":"Other","address":OTHER}),
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
fn request(
    ctx: &mut AppContext,
    parsed: &clap::ArgMatches,
) -> Result<Request, rtao_cli::error::CliError> {
    Request::from_matches(
        ctx,
        parsed
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing stake group"))?
            .1,
        &mut PromptHistory::default(),
    )
}
fn position(hotkey: AccountId, netuid: u16, rao: u64) -> StakePosition {
    StakePosition {
        hotkey,
        coldkey: OWNER,
        netuid: NetUid(netuid),
        stake: DynBalance::from_rao(rao, NetUid(netuid)),
        is_registered: true,
    }
}
fn valuation(positions: Vec<StakePosition>) -> StakeValuation {
    StakeValuation {
        coldkey: OWNER,
        block: 3,
        positions,
        stake_value: DynBalance::from_rao(99, NetUid::ROOT),
        tao_per_alpha: BTreeMap::from([(NetUid(1), 2.0), (NetUid(2), 0.0)]),
    }
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
fn aliases_routes_and_read_transaction_flag_tiers_are_preserved() -> TestResult {
    for (argv, expected) in [
        (
            vec![
                "rtao",
                "st",
                "unstake_all",
                "--hotkey",
                "Straße",
                "--json",
                "--yes",
            ],
            "unstake-all",
        ),
        (
            vec![
                "rtao",
                "stake",
                "unstake_all_alpha",
                "--all-hotkeys",
                "--proxy-for",
                "Owner",
                "--dry-run",
            ],
            "unstake-all-alpha",
        ),
        (
            vec![
                "rtao", "stake", "set_auto", "--netuid", "1", "--hotkey", "Straße", "--yes",
            ],
            "set-auto",
        ),
        (
            vec![
                "rtao", "stake", "children", "revoke", "--netuid", "1", "--hotkey", "Straße",
                "--yes",
            ],
            "child",
        ),
        (vec!["rtao", "stake", "wizard", "--json"], "wizard"),
    ] {
        let parsed = parse(&argv)?;
        assert_eq!(
            parsed.subcommand().ok_or("group")?.1.subcommand_name(),
            Some(expected)
        );
        let (mut ctx, _) = context(&parsed)?;
        request(&mut ctx, &parsed)?;
    }
    assert!(parse(&["rtao", "stake", "auto", "--coldkey", "Owner", "--yes"]).is_err());
    assert!(
        parse(&[
            "rtao",
            "stake",
            "unstake-all",
            "--hotkey",
            "Straße",
            "--all-hotkeys"
        ])
        .is_err()
    );
    for (verb, args, op) in [
        (
            "add",
            vec![
                "--netuid",
                "1",
                "--hotkey",
                "Straße",
                "--amount",
                "0.000000001",
            ],
            "add_stake",
        ),
        (
            "move_swap",
            vec![
                "--origin-netuid",
                "1",
                "--dest-netuid",
                "2",
                "--origin-hotkey",
                "Straße",
                "--dest-hotkey",
                "Other",
                "--amount-alpha",
                "all",
            ],
            "move_swap_stake",
        ),
    ] {
        let mut argv = vec!["rtao", "st", verb];
        argv.extend(args);
        argv.push("--json");
        let parsed = parse(&argv)?;
        assert_eq!(stake::operation(parsed.subcommand().ok_or("group")?.1)?, op);
    }
    for (name, op, kind) in [
        ("unstake-all", "unstake_all", "live non-zero position"),
        (
            "unstake-all-alpha",
            "unstake_all_alpha",
            "live non-zero alpha position outside root",
        ),
    ] {
        let spec = rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == op)
            .ok_or("exit catalog description")?;
        let mut command = exit::command(name, name == "unstake-all");
        let help = command.render_long_help().to_string();
        // Flatten only wrapping whitespace; require the full pinned intent
        // description, including its slippage/skipped-position caveats.
        let compact = |value: &str| value.split_whitespace().collect::<Vec<_>>().join(" ");
        let help = compact(&help);
        assert!(help.contains(&compact(spec.description)));
        assert!(help.contains(&format!(
            "Docs: {}/tx/{}",
            rtao_catalog::py::DOCS_URL,
            op.replace('_', "-")
        )));
        assert!(help.contains("Utility.batch_all"));
        assert!(help.contains("Staking proxy"));
        assert!(help.contains("NonTransfer"));
        assert!(help.contains(&format!("every hotkey with a {kind}")));
        let all = command
            .get_arguments()
            .find(|argument| argument.get_id() == "all_hotkeys")
            .ok_or("all-hotkeys option")?;
        assert_eq!(
            all.get_help().map(ToString::to_string),
            Some(format!(
                "Sweep every hotkey with a {kind} for this coldkey."
            ))
        );
    }
    Ok(())
}

#[test]
fn child_arguments_use_the_real_constructor_and_revoke_is_an_empty_child_set() -> TestResult {
    let children = format!("[[\"0.5\",\"{OTHER}\"]]");
    for (verb, extra, op) in [
        ("set", vec!["--children", children.as_str()], "set_children"),
        ("revoke", vec![], "set_children"),
        ("take", vec!["--take", "0.09"], "set_childkey_take"),
    ] {
        let mut argv = vec![
            "rtao", "stake", "child", verb, "--netuid", "1", "--hotkey", "Straße", "--json",
        ];
        argv.extend(extra);
        let parsed = parse(&argv)?;
        let (mut ctx, _) = context(&parsed)?;
        let Request::Tx(tx) = request(&mut ctx, &parsed)? else {
            return Err("expected semantic transaction".into());
        };
        assert_eq!(tx.op, op);
        assert_eq!(tx.args["hotkey_ss58"], json!(HOT));
        if verb == "revoke" {
            assert_eq!(tx.args["children"], json!([]));
        }
        if verb == "take" {
            assert_eq!(tx.intent()?.as_dyn().to_json_dyn()["take"], json!(5898));
        }
    }
    for malformed in ["{", "null", "[[2,3,4]]", "[[18446744073709551616,\"x\"]]"] {
        let parsed = parse(&[
            "rtao",
            "stake",
            "child",
            "set",
            "--netuid",
            "1",
            "--hotkey",
            "Straße",
            "--children",
            malformed,
            "--json",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        assert!(request(&mut ctx, &parsed).is_err());
    }
    Ok(())
}

#[test]
fn exit_discovery_ignores_zero_root_alpha_only_and_preserves_runtime_order() -> TestResult {
    let positions = vec![
        position(OTHER, 0, 3),
        position(HOT, 1, 9_007_199_254_740_993),
        position(OTHER, 2, 1),
        position(HOT, 3, 0),
        position(OWNER, 1, 0),
    ];
    assert_eq!(exit::live_hotkeys(&positions, true), vec![OTHER, HOT]);
    assert_eq!(exit::live_hotkeys(&positions, false), vec![HOT, OTHER]);
    let root_only = vec![position(OTHER, 0, u64::MAX)];
    assert!(exit::live_hotkeys(&root_only, false).is_empty());
    let single = exit::transaction(true, &[OTHER], Some(64))?;
    assert_eq!(single.op, "unstake_all");
    assert_eq!(single.period, Some(64));
    let multiple = exit::transaction(false, &[HOT, OTHER], None)?;
    assert_eq!(multiple.op, "batch");
    assert_eq!(
        multiple.args["intents"],
        json!([{"op":"unstake_all_alpha","hotkey_ss58":HOT},{"op":"unstake_all_alpha","hotkey_ss58":OTHER}])
    );
    assert!(exit::transaction(true, &[], None).is_err());
    assert_eq!(multiple.intent()?.as_dyn().wraps()[0].pallet, "Utility");
    Ok(())
}

#[test]
fn flat_json_keeps_subnet_currency_locks_root_yield_and_all_wallet_context() -> TestResult {
    let positions_value = valuation(vec![
        position(HOT, 0, 2_000_000_000),
        position(OTHER, 1, 1_500_000_000),
        position(HOT, 2, 1),
    ]);
    let locked = DynBalance::from_rao(900_000_000, NetUid(1));
    let locks = vec![ColdkeyLock {
        hotkey: HOT,
        netuid: NetUid(1),
        locked_alpha: locked.clone(),
        is_perpetual: true,
    }];
    let availability = vec![StakeAvailability {
        netuid: NetUid(1),
        total: DynBalance::from_rao(1_500_000_000, NetUid(1)),
        locked,
        available: DynBalance::from_rao(600_000_000, NetUid(1)),
    }];
    let owed = DynBalance::from_rao(500_000_000, NetUid::ROOT);
    let records = stake::position_records(&positions_value, &locks, &availability, &owed, None)?;
    assert_eq!(records.len(), 3); // The zero-valued dust record remains in JSON.
    assert_eq!(records[0]["stake_unit"], "TAO");
    assert_eq!(records[1]["stake_unit"], "alpha (netuid 1)");
    assert_eq!(records[1]["stake_amount"], json!(1.5));
    assert_eq!(records[1]["value_tao"], json!(3.0));
    assert_eq!(records[1]["locked_amount"], json!(0.9));
    assert_eq!(records[1]["available_amount"], json!(0.6));
    assert_eq!(records[1]["lock_hotkey"], json!(HOT));
    assert_eq!(records[1]["is_perpetual"], true);
    for record in &records {
        assert_eq!(record["personal_yield"], json!(0.25));
        assert_eq!(record["root_staked_tao"], json!(2.0));
        assert_eq!(record["accrued_basket_yield_tao"], json!(0.5));
        assert_eq!(record["auto_claim"], false);
        assert!(record.get("coldkey").is_none());
        assert!(record.get("wallet").is_none());
    }
    let all = stake::position_records(
        &positions_value,
        &locks,
        &availability,
        &owed,
        Some("wallet2"),
    )?;
    assert_eq!(all[0]["wallet"], "wallet2");
    assert_eq!(all[0]["coldkey"], json!(OWNER));
    let overflow = valuation(vec![position(HOT, 0, u64::MAX), position(OTHER, 0, 1)]);
    assert!(stake::position_records(&overflow, &[], &[], &owed, None).is_err());
    let boundary = valuation(vec![position(HOT, 0, 1_000_000_000)]);
    let records = stake::position_records(
        &boundary,
        &[],
        &[],
        &DynBalance::from_rao(9_007_199_254_740_995, NetUid::ROOT),
        None,
    )?;
    assert_eq!(
        records[0]["personal_yield"]
            .as_f64()
            .ok_or("yield ratio")?
            .to_bits(),
        0x4161_2e0b_e826_d696
    );
    let no_root = valuation(vec![position(HOT, 1, 1)]);
    assert_eq!(
        stake::position_records(&no_root, &[], &[], &owed, None)?[0]["personal_yield"],
        Value::Null
    );
    Ok(())
}

#[tokio::test]
async fn show_children_and_auto_execute_real_typed_reads_over_fake_rpc() -> TestResult {
    let mut state = FakeState::try_default()?;
    state.seed_runtime("StakeInfoRuntimeApi","get_stake_info_for_hotkey_coldkey_netuid",json!({"hotkey":HOT,"coldkey":OWNER,"netuid":1,"stake":9_007_199_254_740_993u64,"is_registered":true}))?;
    state.seed_python(
        "SubtensorModule",
        "ChildKeys",
        &[json!(HOT), json!(1)],
        json!([[u64::MAX, HOT], [1, OTHER]]),
    )?;
    state.seed_python(
        "SubtensorModule",
        "AutoStakeDestination",
        &[json!(OWNER), json!(1)],
        json!(HOT),
    )?;
    let (client, _) = client(state).await?;
    for (argv, expected) in [
        (
            vec![
                "rtao",
                "stake",
                "show",
                "--coldkey",
                "Owner",
                "--hotkey",
                "Straße",
                "--netuid",
                "1",
                "--json",
            ],
            json!({"coldkey":OWNER,"hotkey":HOT,"netuid":1,"stake":DynBalance::from_rao(9_007_199_254_740_993,NetUid(1)).to_python_string(),"stake_amount":9_007_199.254_740_993,"stake_unit":"alpha (netuid 1)"}),
        ),
        (
            vec![
                "rtao", "stake", "child", "get", "--hotkey", "Straße", "--netuid", "1", "--json",
            ],
            json!({"hotkey":HOT,"netuid":1,"children":[[u64::MAX,HOT],[1,OTHER]]}),
        ),
        (
            vec!["rtao", "stake", "auto", "--coldkey", "Owner", "--json"],
            json!([{"netuid":1,"hotkey":HOT}]),
        ),
    ] {
        let parsed = parse(&argv)?;
        let (mut ctx, sink) = context(&parsed)?;
        let request = request(&mut ctx, &parsed)?;
        stake::execute(
            &mut ctx,
            &client,
            &request,
            Default::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await?;
        let actual: Value = serde_json::from_slice(&sink.bytes(Stream::Stdout)?)?;
        assert_eq!(actual, expected);
    }
    client.close().await;
    Ok(())
}

#[test]
fn wizard_emits_the_exact_pinned_python_payload_without_a_connection() -> TestResult {
    let parsed = parse(&["rtao", "stake", "wizard", "--json"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = request(&mut ctx, &parsed)?;
    assert!(stake::execute_local(&ctx, &request)?);
    let actual: Value = serde_json::from_slice(&sink.bytes(Stream::Stdout)?)?;
    assert_eq!(
        actual["move"],
        "btcli stake move --origin-netuid N --destination-netuid M --all"
    );
    assert_eq!(
        actual["units"],
        "amounts are in the ORIGIN subnet's alpha (TAO when origin netuid is 0); pass `--all` or `--amount all` to move the whole position"
    );
    assert_eq!(actual.as_object().ok_or("wizard object")?.len(), 5);
    Ok(())
}

#[derive(Clone)]
struct PublicKeys(Arc<std::sync::atomic::AtomicUsize>);
impl rtao::signer::KeyView for PublicKeys {
    fn coldkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        Ok(OWNER)
    }
    fn hotkey_address(&self) -> Result<AccountId, rtao::signer::SignerError> {
        Ok(HOT)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], rtao::signer::SignerError> {
        Ok(HOT.0)
    }
}
impl rtao::signer::DynKeySource for PublicKeys {
    fn signer_for<'a>(
        &'a self,
        _: rtao::signer::SignerRole,
    ) -> rtao::signer::BoxFuture<'a, Result<Arc<dyn rtao::signer::Signer>, rtao::signer::SignerError>>
    {
        Box::pin(async {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(rtao::signer::SignerError::KeyView(
                "a preview must never acquire a signer".into(),
            ))
        })
    }
}
impl rtao_cli::context::ExternalSignerFactory for PublicKeys {
    fn keys<'a>(
        &'a self,
        _: &'a Globals,
        _: rtao::signer::SignerRole,
    ) -> rtao::signer::BoxFuture<
        'a,
        Result<Arc<dyn rtao::signer::DynKeySource>, rtao_cli::error::CliError>,
    > {
        Box::pin(async { Ok(Arc::new(self.clone()) as Arc<dyn rtao::signer::DynKeySource>) })
    }
}

#[tokio::test]
async fn bulk_exit_uses_atomic_calls_and_child_shield_default_without_acquiring_a_key() -> TestResult
{
    use rtao::client::View;
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "System",
        "Account",
        &[json!(OWNER)],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([
            {"hotkey":HOT,"coldkey":OWNER,"netuid":1,"stake":1,"is_registered":true},
            {"hotkey":OTHER,"coldkey":OWNER,"netuid":2,"stake":1,"is_registered":true}
        ]),
    )?;
    let (client, chain) = client(state).await?;
    let keys = PublicKeys(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
    let runtime = View::runtime(&client).await?;
    let expected = rtao_runtime::calls::utility::batch_all(vec![
        rtao_runtime::calls::subtensor_module::unstake_all_alpha(HOT).compose(&runtime)?,
        rtao_runtime::calls::subtensor_module::unstake_all_alpha(OTHER).compose(&runtime)?,
    ])
    .compose(&runtime)?;
    let tx = exit::transaction(false, &[HOT, OTHER], None)?;
    let intent = tx.intent()?;
    let plan = rtao::Executor::new(client.clone())
        .plan_dyn(intent.as_dyn(), &keys, &Default::default())
        .await?;
    assert_eq!(plan.call().as_bytes(), expected.as_bytes());
    for (shield, period, wants_marker, wants_error) in [
        (None, None, true, false),
        (Some(false), None, false, false),
        (Some(true), None, true, false),
        // NextKey is absent. An era forces the normal availability check:
        // a default can fall back, while an explicit opt-in must refuse.
        (None, Some(4), false, false),
        (Some(true), Some(4), false, true),
    ] {
        let parsed = parse(&[
            "rtao",
            "stake",
            "unstake-all-alpha",
            "--all-hotkeys",
            "--dry-run",
            "--json",
        ])?;
        let (mut ctx, sink) = context(&parsed)?;
        ctx.globals.signer_backend = Some("ledger".into());
        ctx.globals.signer_address = Some(OWNER.to_string());
        ctx.globals.mev_shield = shield;
        let request = Request::Exit(exit::Request {
            include_root: false,
            hotkey: None,
            all_hotkeys: true,
            period,
        });
        let result = stake::execute(
            &mut ctx,
            &client,
            &request,
            Default::default(),
            Some(&keys),
            &mut PromptHistory::default(),
        )
        .await;
        assert_eq!(result.is_err(), wants_error);
        if wants_error {
            assert!(
                result
                    .err()
                    .ok_or("expected explicit shield refusal")?
                    .message
                    .contains("MEV shield is not active")
            );
        } else {
            let payload: Value = serde_json::from_slice(&sink.bytes(Stream::Stdout)?)?;
            assert_eq!(payload["op"], "batch");
            assert_eq!(
                payload["effects"]
                    .as_array()
                    .ok_or("effects")?
                    .iter()
                    .any(|effect| effect
                        .as_str()
                        .is_some_and(|text| text.starts_with("submitted MEV-shielded:"))),
                wants_marker
            );
        }
    }
    assert_eq!(keys.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "fake state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

fn seed_list_availability(state: &mut FakeState, has_root: bool, locked: u64) -> TestResult {
    use rtao::core::{codec::Value as CoreValue, ops::runtime_api, runtime::TypeSpec};
    use rtao_testkit::fakes::json_to_core;
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let netuids = if has_root {
        vec![0, 1, 2, 3]
    } else {
        vec![1, 2, 3]
    };
    let input = runtime_api::encode_params(
        &state.runtime,
        api,
        signature,
        &[
            CoreValue::List(vec![CoreValue::Bytes(OWNER.0.to_vec())]),
            CoreValue::List(netuids.into_iter().map(CoreValue::Int).collect()),
        ],
    )?;
    let output = state.runtime.encode_spec(
        &TypeSpec::Id(signature.output),
        &CoreValue::Dict(vec![(
            CoreValue::Bytes(OWNER.0.to_vec()),
            CoreValue::Dict(vec![(
                CoreValue::Int(1),
                json_to_core(&json!({"total":101_000_000_001u64,"locked":locked,"available":101_000_000_001u64-locked}))?,
            )]),
        )]),
    )?;
    state.seed_runtime_encoded(format!("{api}_{method}"), input, output);
    Ok(())
}

#[tokio::test]
async fn list_keeps_all_json_positions_and_globally_sorts_the_filtered_human_rows() -> TestResult {
    const TINY: AccountId = AccountId([10; 32]);
    const DUST: AccountId = AccountId([11; 32]);
    const LOCK_TARGET: AccountId = AccountId([12; 32]);
    for (locked_rao, principal_rao, accrued_rao) in [
        (0u64, 0u64, 0u64),
        (900_000_000, 0, 0),
        (0, 2_000_000_000, 0),
        (900_000_000, 2_000_000_000, 500_000_000),
    ] {
        let mut state = FakeState::try_default()?;
        let mut positions = json!([
            {"hotkey":HOT,"coldkey":OWNER,"netuid":1,"stake":100_000_000_000u64,"is_registered":true},
            {"hotkey":OTHER,"coldkey":OWNER,"netuid":1,"stake":1_000_000_000u64,"is_registered":true},
            {"hotkey":OWNER,"coldkey":OWNER,"netuid":2,"stake":50_000_000_000u64,"is_registered":true},
            {"hotkey":TINY,"coldkey":OWNER,"netuid":1,"stake":1,"is_registered":false},
            {"hotkey":DUST,"coldkey":OWNER,"netuid":3,"stake":1,"is_registered":false}
        ]);
        if principal_rao > 0 {
            positions.as_array_mut().ok_or("positions")?.push(json!(
                {"hotkey":HOT,"coldkey":OWNER,"netuid":0,"stake":principal_rao,"is_registered":true}
            ));
        }
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            positions.clone(),
        )?;
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([[OWNER, positions]]),
        )?;
        state.seed_runtime("SwapRuntimeApi","current_alpha_price_all",json!([
            {"netuid":1,"price":1_000_000_000},{"netuid":2,"price":1_000_000_000},{"netuid":3,"price":1_000_000_000}
        ]))?;
        seed_list_availability(&mut state, principal_rao > 0, locked_rao)?;
        if locked_rao > 0 {
            let lock = json!({"locked_mass":locked_rao,"conviction":{"bits":0},"last_update":0});
            state.seed_python(
                "SubtensorModule",
                "Lock",
                &[json!(OWNER), json!(1), json!(LOCK_TARGET)],
                lock.clone(),
            )?;
            state.seed_python(
                "SubtensorModule",
                "DecayingLock",
                &[json!(OWNER), json!(1)],
                json!(false),
            )?;
            state.seed_runtime("StakeInfoRuntimeApi", "get_coldkey_lock", lock)?;
        }
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_owed",
            json!(accrued_rao),
        )?;
        let (client, _) = client(state).await?;
        for (json_mode, dust, quiet, all_wallets) in [
            (true, false, false, false),
            (true, false, true, true),
            (false, false, false, false),
            (false, true, false, false),
            (false, false, true, true),
        ] {
            let mut argv = vec!["rtao", "stake", "list", "--coldkey", "Owner"];
            if json_mode {
                argv.push("--json");
            }
            if dust {
                argv.push("--dust");
            }
            if quiet {
                argv.push("--quiet");
            }
            let parsed = parse(&argv)?;
            let (mut ctx, sink) = context(&parsed)?;
            ctx.books.addresses.extend([
                json!({"name":"TinyLeaf","address":TINY}),
                json!({"name":"DustSubnet","address":DUST}),
                json!({"name":"LockTarget","address":LOCK_TARGET}),
            ]);
            let request = if all_wallets {
                Request::List {
                    owners: vec![(Some("wallet10".into()), OWNER)],
                    dust,
                    all_wallets: true,
                }
            } else {
                request(&mut ctx, &parsed)?
            };
            stake::execute(
                &mut ctx,
                &client,
                &request,
                Default::default(),
                None,
                &mut PromptHistory::default(),
            )
            .await?;
            let output = sink.text(Stream::Stdout)?;
            if json_mode {
                let records: Value = serde_json::from_str(&output)?;
                let records = records.as_array().ok_or("records")?;
                assert_eq!(records.len(), 5 + usize::from(principal_rao > 0));
                assert_eq!(records[3]["hotkey"], json!(TINY));
                assert_eq!(records[4]["registered"], false);
                assert_eq!(records[0]["locked_amount"], json!(locked_rao as f64 / 1e9));
                assert_eq!(
                    records[0]["available_amount"],
                    json!((101_000_000_001u64 - locked_rao) as f64 / 1e9)
                );
                for record in records {
                    assert_eq!(record["root_staked_tao"], json!(principal_rao as f64 / 1e9));
                    assert_eq!(
                        record["accrued_basket_yield_tao"],
                        json!(accrued_rao as f64 / 1e9)
                    );
                    assert_eq!(
                        record["personal_yield"],
                        if principal_rao > 0 {
                            json!(accrued_rao as f64 / principal_rao as f64)
                        } else {
                            Value::Null
                        }
                    );
                    assert_eq!(record["auto_claim"], false);
                    assert_eq!(
                        record.get("wallet"),
                        all_wallets.then(|| json!("wallet10")).as_ref()
                    );
                    assert_eq!(
                        record.get("coldkey"),
                        all_wallets.then(|| json!(OWNER)).as_ref()
                    );
                    if record["netuid"] == 1 && locked_rao > 0 {
                        assert_eq!(record["lock_hotkey"], json!(LOCK_TARGET));
                        assert_eq!(record["is_perpetual"], true);
                    } else {
                        assert!(record.get("lock_hotkey").is_none());
                        assert!(record.get("is_perpetual").is_none());
                    }
                }
                assert!(sink.bytes(Stream::Stderr)?.is_empty());
            } else {
                let large = output.find("Straße").ok_or("large position")?;
                let middle = output.find("Owner").ok_or("middle position")?;
                let small = output.find("Other").ok_or("small position")?;
                assert!(large < middle && middle < small);
                assert_eq!(output.contains("TinyLeaf"), dust);
                assert_eq!(output.contains("DustSubnet"), dust);
                let header = output
                    .lines()
                    .find(|line| line.contains("netuid") && line.contains("spot value"))
                    .ok_or("stake header")?;
                assert_eq!(header.contains("wallet"), all_wallets);
                assert_eq!(header.contains("locked"), locked_rao > 0);
                if locked_rao > 0 {
                    let index = if all_wallets { 3 } else { 2 };
                    let columns = if all_wallets { 7 } else { 6 };
                    let cells = output
                        .lines()
                        .filter_map(|line| {
                            // UTF8_FULL uses outer │ borders and internal ┆
                            // cell separators. Divider lines are not data rows.
                            let row = line.trim().strip_prefix('│')?.strip_suffix('│')?;
                            let cells = row.split('┆').map(str::trim).collect::<Vec<_>>();
                            (cells.len() == columns && !cells.contains(&"netuid")).then_some(cells)
                        })
                        .map(|cells| cells[index])
                        .collect::<Vec<_>>();
                    assert_eq!(
                        cells.len(),
                        3 + usize::from(principal_rao > 0) + 2 * usize::from(dust),
                        "visible stake rows: {output}"
                    );
                    let amount = DynBalance::from_rao(locked_rao, NetUid(1)).to_python_string();
                    assert_eq!(
                        cells
                            .iter()
                            .filter(|cell| !cell.is_empty())
                            .copied()
                            .collect::<Vec<_>>(),
                        vec![amount.as_str()]
                    );
                    let largest = output
                        .lines()
                        .find(|line| line.contains("Straße"))
                        .ok_or("largest row")?;
                    assert!(largest.contains(&amount));
                    assert!(largest.contains("lock on LockTarget"));
                }
                let has_yield = principal_rao > 0 || accrued_rao > 0;
                assert_eq!(output.contains("accrued basket yield:"), has_yield);
                assert_eq!(output.contains("root principal:"), has_yield);
                assert_eq!(output.contains("rtao root claim"), has_yield);
                if principal_rao > 0 {
                    let ratio = if accrued_rao > 0 {
                        "25.00% of root stake"
                    } else {
                        "0.00% of root stake"
                    };
                    assert!(output.contains(ratio));
                }
                assert_eq!(output.matches("(spot, excl. slippage/fees)").count(), 1);
                if !dust && !quiet {
                    let diagnostic = sink.text(Stream::Stderr)?;
                    assert!(diagnostic.contains("1 dust subnet and 1 dust position hidden"));
                }
                if quiet {
                    assert!(sink.bytes(Stream::Stderr)?.is_empty());
                }
            }
        }
        client.close().await;
    }
    Ok(())
}

struct ScriptTerminal {
    answers: std::collections::VecDeque<String>,
    prompts: Arc<Mutex<Vec<String>>>,
}
impl Terminal for ScriptTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.prompts
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(label.into());
        self.answers.pop_front().ok_or(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn interactive(ctx: &mut AppContext, answers: &[&str]) -> Arc<Mutex<Vec<String>>> {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    ctx.globals.wallet_given = true;
    ctx.terminal = Arc::new(Mutex::new(Box::new(ScriptTerminal {
        answers: answers.iter().map(|value| (*value).into()).collect(),
        prompts: prompts.clone(),
    })));
    prompts
}

#[tokio::test]
async fn generated_source_picker_fills_hotkey_and_netuid_once_and_preserves_replay() -> TestResult {
    use rtao_cli::commands::generated::TxRequest;
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([
            {"hotkey":HOT,"coldkey":OWNER,"netuid":19,"stake":1_000_000_000u64,"is_registered":true}
        ]),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([{"netuid":19,"price":1_000_000_000}]),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_availability_for_coldkeys",
        json!({}),
    )?;
    let (client, _) = client(state).await?;
    let parsed = parse(&[
        "rtao",
        "stake",
        "remove",
        "--amount-alpha",
        "all",
        "--proxy-for",
        "Owner",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let prompts = interactive(&mut ctx, &[]);
    let mut history = PromptHistory::default();
    let leaf = parsed
        .subcommand()
        .ok_or("group")?
        .1
        .subcommand()
        .ok_or("leaf")?
        .1;
    let tx =
        TxRequest::from_matches_with_client(&mut ctx, &client, "remove_stake", leaf, &mut history)
            .await?;
    assert_eq!(tx.args["hotkey_ss58"], json!(HOT));
    assert_eq!(tx.args["netuid"], 19);
    assert!(prompts.lock().map_err(|_| "prompts")?.is_empty());
    let replay = history.replay_command(&["rtao".into(), "stake".into(), "remove".into()]);
    assert!(replay.contains("--netuid 19"));
    assert!(replay.contains(&format!("--hotkey {HOT}")));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn generated_target_and_destination_pickers_record_only_public_selected_accounts()
-> TestResult {
    use rtao_cli::commands::generated::TxRequest;
    let (client, _) = client(FakeState::try_default()?).await?;
    let destination = OWNER.to_string();
    for (argv, op, answer, field, expected) in [
        (
            vec!["rtao", "stake", "add", "--netuid", "1", "--amount", "1"],
            "add_stake",
            "Other",
            "hotkey_ss58",
            OTHER,
        ),
        (
            vec![
                "rtao",
                "stake",
                "transfer",
                "--origin-netuid",
                "1",
                "--dest-netuid",
                "2",
                "--hotkey",
                "Straße",
                "--amount-alpha",
                "all",
            ],
            "transfer_stake",
            destination.as_str(),
            "dest_coldkey_ss58",
            OWNER,
        ),
    ] {
        let parsed = parse(&argv)?;
        let (mut ctx, _) = context(&parsed)?;
        let prompts = interactive(&mut ctx, &[answer]);
        let mut history = PromptHistory::default();
        let leaf = parsed
            .subcommand()
            .ok_or("group")?
            .1
            .subcommand()
            .ok_or("leaf")?
            .1;
        let tx =
            TxRequest::from_matches_with_client(&mut ctx, &client, op, leaf, &mut history).await?;
        assert_eq!(tx.args[field], json!(expected));
        assert_eq!(prompts.lock().map_err(|_| "prompts")?.len(), 1);
        let replay = history.replay_command(&["rtao".into(), "stake".into()]);
        assert!(replay.contains(&expected.to_string()));
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn empty_human_positions_still_show_accrued_yield_and_claim_guidance() -> TestResult {
    for accrued_rao in [0u64, 500_000_000] {
        let mut state = FakeState::try_default()?;
        state.seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([]),
        )?;
        state.seed_runtime("SwapRuntimeApi", "current_alpha_price_all", json!([]))?;
        state.seed_runtime(
            "BetaBasketRuntimeApi",
            "get_root_basket_owed",
            json!(accrued_rao),
        )?;
        let (client, _) = client(state).await?;
        for (json_mode, quiet) in [(false, false), (false, true), (true, false)] {
            let mut argv = vec!["rtao", "stake", "list", "--coldkey", "Owner"];
            if json_mode {
                argv.push("--json");
            }
            if quiet {
                argv.push("--quiet");
            }
            let parsed = parse(&argv)?;
            let (mut ctx, sink) = context(&parsed)?;
            let request = request(&mut ctx, &parsed)?;
            stake::execute(
                &mut ctx,
                &client,
                &request,
                Default::default(),
                None,
                &mut PromptHistory::default(),
            )
            .await?;
            let output = sink.text(Stream::Stdout)?;
            if json_mode {
                assert_eq!(serde_json::from_str::<Value>(&output)?, json!([]));
            } else {
                assert!(output.contains("none"));
                assert_eq!(
                    output.contains("accrued basket yield: τ0.500000000"),
                    accrued_rao > 0
                );
                assert_eq!(output.contains("rtao root claim"), accrued_rao > 0);
                assert_eq!(output.contains("root principal:"), accrued_rao > 0);
                assert!(!output.contains("% of root stake"));
                assert_eq!(output.matches("(spot, excl. slippage/fees)").count(), 1);
                if accrued_rao == 0 {
                    assert!(!output.contains("yield"));
                }
            }
            assert!(sink.bytes(Stream::Stderr)?.is_empty());
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn source_owner_labels_keep_account_reads_and_explicit_show_context() -> TestResult {
    use rtao_cli::commands::generated::TxRequest;
    let root = TempRoot::new();
    let wallet = root.path().join("chosen");
    std::fs::create_dir(&wallet)?;
    std::fs::write(
        wallet.join("coldkeypub.txt"),
        serde_json::to_vec(&json!({"ss58Address":HOT,"cryptoType":1}))?,
    )?;
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([]),
    )?;
    state.seed_runtime("SwapRuntimeApi", "current_alpha_price_all", json!([]))?;
    let (client, chain) = client(state).await?;
    for mode in [
        "wallet",
        "proxy",
        "configured proxy",
        "self",
        "multisig",
        "external",
    ] {
        let parsed = parse(&[
            "rtao",
            "stake",
            "remove",
            "--amount-alpha",
            "all",
            "--netuid",
            "19",
            "--wallet",
            "chosen",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        ctx.globals.wallet_path = root.path().to_owned();
        ctx.passwords.native_provider = None;
        interactive(&mut ctx, &[]);
        let (expected_owner, expected_label) = match mode {
            "proxy" => {
                ctx.globals.proxy_for = Some("Owner".into());
                (OWNER, "Owner".to_owned())
            }
            "configured proxy" => {
                ctx.config.insert("proxy_for".into(), json!("Owner"));
                (OWNER, "Owner".to_owned())
            }
            "self" => {
                ctx.config.insert("proxy_for".into(), json!("Owner"));
                ctx.globals.proxy_for = Some("self".into());
                (HOT, "wallet 'chosen'".to_owned())
            }
            "multisig" => {
                ctx.books.multisigs =
                    vec![json!({"name":"chosen","threshold":2,"signatories":[HOT,OTHER]})];
                let account = ctx
                    .resolve_address("coldkey_ss58", None)?
                    .ok_or("multisig owner")?;
                assert_ne!(account, HOT);
                (account, "multisig 'chosen'".to_owned())
            }
            "external" => {
                ctx.globals.signer_backend = Some("ledger".into());
                ctx.globals.signer_address = Some(OTHER.to_string());
                (OTHER, format!("signer {OTHER}"))
            }
            _ => (HOT, "wallet 'chosen'".to_owned()),
        };
        let start = chain.requests().len();
        let group = parsed.subcommand().ok_or("stake group")?.1;
        let leaf = group.subcommand().ok_or("remove leaf")?.1;
        let error = TxRequest::from_matches_with_client(
            &mut ctx,
            &client,
            "remove_stake",
            leaf,
            &mut PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("empty source must refuse")?;
        assert_eq!(error.code, 1);
        assert_eq!(
            error.message,
            format!("{expected_label} has no stake on netuid 19")
        );
        assert_eq!(
            error.help.as_deref(),
            Some("`rtao stake list` shows every position")
        );
        let reads = chain
            .requests()
            .into_iter()
            .skip(start)
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "StakeInfoRuntimeApi_get_stake_info_for_coldkey"
            })
            .collect::<Vec<_>>();
        assert_eq!(reads.len(), 1);
        assert_eq!(
            reads[0].params[1],
            format!("0x{}", hex::encode(expected_owner.0))
        );
    }
    let owner_address = OWNER.to_string();
    for (raw, expected_owner, expected_label) in [
        (Some("Owner"), OWNER, "Owner"),
        (Some(owner_address.as_str()), OWNER, owner_address.as_str()),
        (None, HOT, "wallet 'chosen'"),
    ] {
        let mut argv = vec![
            "rtao", "stake", "show", "--netuid", "19", "--wallet", "chosen",
        ];
        if let Some(raw) = raw {
            argv.extend(["--coldkey", raw]);
        }
        let parsed = parse(&argv)?;
        let (mut ctx, _) = context(&parsed)?;
        ctx.globals.wallet_path = root.path().to_owned();
        ctx.passwords.native_provider = None;
        interactive(&mut ctx, &[]);
        let request = request(&mut ctx, &parsed)?;
        let start = chain.requests().len();
        let error = stake::execute(
            &mut ctx,
            &client,
            &request,
            Default::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("empty show source must refuse")?;
        assert_eq!(error.code, 1);
        assert_eq!(
            error.message,
            format!("{expected_label} has no stake on netuid 19")
        );
        let reads = chain
            .requests()
            .into_iter()
            .skip(start)
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "StakeInfoRuntimeApi_get_stake_info_for_coldkey"
            })
            .collect::<Vec<_>>();
        assert_eq!(reads.len(), 1);
        assert_eq!(
            reads[0].params[1],
            format!("0x{}", hex::encode(expected_owner.0))
        );
    }
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn stake_list_uses_natural_local_aliases_and_empty_contacts_allow_chain_names() -> TestResult
{
    let wallets = TempRoot::new();
    // File creation order opposes natural wallet and hotkey ordering.
    for (wallet, hotkey) in [
        ("wallet10", "wrong-wallet"),
        ("wallet2", "hot10"),
        ("wallet2", "hot2"),
    ] {
        let directory = wallets.path().join(wallet);
        std::fs::create_dir_all(directory.join("hotkeys"))?;
        std::fs::write(
            directory.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":OWNER,"cryptoType":1}))?,
        )?;
        std::fs::write(
            directory.join("hotkeys").join(hotkey),
            serde_json::to_vec(&json!({"ss58Address":HOT,"cryptoType":1}))?,
        )?;
    }
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkey",
        json!([
            {"hotkey":HOT,"coldkey":OWNER,"netuid":1,"stake":1_000_000_000u64,"is_registered":true},
            {"hotkey":OTHER,"coldkey":OWNER,"netuid":1,"stake":500_000_000u64,"is_registered":true}
        ]),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([
            {"netuid":1,"price":1_000_000_000}
        ]),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_availability_for_coldkeys",
        json!({}),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    state.seed_python("SubtensorModule", "Owner", &[json!(OTHER)], json!(OWNER))?;
    state.seed_python(
        "SubtensorModule",
        "IdentitiesV2",
        &[json!(OWNER)],
        json!({"name":"chain-only"}),
    )?;
    let (client, chain) = client(state).await?;
    let parsed = parse(&["rtao", "stake", "list", "--coldkey", &OWNER.to_string()])?;
    let (mut ctx, sink) = context(&parsed)?;
    ctx.globals.wallet_path = wallets.path().to_owned();
    ctx.books.addresses = vec![
        json!({"name":"contact-must-not-replace-hot2","address":HOT}),
        json!({"name":"","address":OTHER}),
    ];
    let request = request(&mut ctx, &parsed)?;
    let outcome = stake::execute(
        &mut ctx,
        &client,
        &request,
        Default::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await;
    client.close().await;
    outcome?;
    let text = sink.text(Stream::Stdout)?;
    assert!(text.contains("hot2"), "{text}");
    assert!(text.contains("chain-only"), "{text}");
    assert!(!text.contains("hot10"));
    assert!(!text.contains("wrong-wallet"));
    assert!(!text.contains("contact-must-not-replace-hot2"));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    Ok(())
}
