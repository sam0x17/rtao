// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnet_emission.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_emission_weights.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! `subnets emission` views and checked writes against the encoded fake chain.

use rtao::{
    client::{Client, ClientConfig, Network, View},
    primitives::{
        AccountId, AlphaBalance, CryptoType, DefaultAccount, NetUid, TaoBalance,
        emission_weights::{self as ew, DEFAULT_OWNER, ONE},
    },
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
};
use rtao_cli::{
    application,
    books::Books,
    commands::subnets::{self, Request},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_runtime::storage::{subtensor_module as st, swap};
use rtao_testkit::{
    FakeChain, FakeState, Reply, SubmitScript,
    fixtures::catalog_samples::{
        COLD, EMISSION_ALPHA_ISSUANCE, EMISSION_ROOT_TAO, EMISSION_TAO_WEIGHT, SUBNET,
        emission_root_baseline_share, emission_weights_info, intent_state,
    },
    rpc::RpcClient,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[path = "../../rtao-wallet/tests/support/mod.rs"]
mod support;
use support::TempRoot;

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

/// Public coldkey records only; no secret key material exists in the fixture.
fn wallet() -> Result<TempRoot, Box<dyn std::error::Error>> {
    let root = TempRoot::new();
    let wallet = root.path().join("sample");
    std::fs::create_dir(&wallet)?;
    std::fs::write(
        wallet.join("coldkeypub.txt"),
        serde_json::to_vec(&json!({"ss58Address":COLD,"cryptoType":1}))?,
    )?;
    Ok(root)
}

// py: tests/unit/test_emission_weights.py:363-372 (_client)
fn state() -> Result<FakeState, Box<dyn std::error::Error>> {
    let runtime = FakeState::try_default()?.runtime;
    let mut state = intent_state(runtime, "set_subnet_emission_weights")?;
    state.seed(&st::TAO_WEIGHT, (), EMISSION_TAO_WEIGHT)?;
    state.seed(
        &st::SUBNET_TAO,
        (NetUid::ROOT,),
        TaoBalance(EMISSION_ROOT_TAO),
    )?;
    state.seed(&st::SUBNET_ALPHA_IN, (SUBNET,), AlphaBalance(0))?;
    state.seed(
        &st::SUBNET_ALPHA_OUT,
        (SUBNET,),
        AlphaBalance(EMISSION_ALPHA_ISSUANCE),
    )?;
    state.seed(&swap::BALANCER_ALPHA_RESERVOIR, (SUBNET,), AlphaBalance(0))?;
    Ok(state)
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

struct Outcome {
    result: Result<(), CliError>,
    stdout: String,
    stderr: String,
}

async fn run(
    client: &Client,
    wallet: &Path,
    args: &[&str],
) -> Result<Outcome, Box<dyn std::error::Error>> {
    run_with(client, wallet, args, None).await
}

/// Run one invocation; a signing source routes it as an external signer.
async fn run_with(
    client: &Client,
    wallet: &Path,
    args: &[&str],
    signer: Option<&Factory>,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    run_setup(client, wallet, args, signer, &Setup::default()).await
}

/// A saved multisig wallet `team`, signed by the selected members.
struct Saved {
    multisigs: Value,
    signatories: Vec<String>,
    store: TempRoot,
}

/// Submission deadline and saved-multisig context for one invocation.
#[derive(Default)]
struct Setup {
    deadline: Option<Duration>,
    saved: Option<Saved>,
}

async fn run_setup(
    client: &Client,
    wallet: &Path,
    args: &[&str],
    signer: Option<&Factory>,
    setup: &Setup,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    let mut argv = vec![
        "rtao",
        "--wallet-path",
        wallet.to_str().ok_or("wallet path")?,
        "--wallet",
        "sample",
    ];
    argv.extend(args);
    let parsed = application::command_tree([subnets::command()?]).try_get_matches_from(argv)?;
    let group = parsed.subcommand().ok_or("subnets")?.1;
    let emission = group.subcommand().ok_or("emission")?.1;
    let leaf = emission.subcommand().ok_or("emission leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/emission-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &subnets::reserved(group)?, |_| None)?;
    if let Some(signer) = signer {
        globals.signer_backend = Some("extension".into());
        globals.signer_address = Some(signer.0.key.account().to_string());
    }
    let mut books = Books::default();
    if let Some(saved) = &setup.saved {
        globals.wallet_name = "team".into();
        globals.wallet_given = true;
        globals.signer_backend = None;
        globals.signer_address = None;
        globals.signatories = saved.signatories.clone();
        books = Books::from_values(json!([]), json!([]), saved.multisigs.clone());
    }
    let mut ctx = AppContext::new(
        globals,
        config,
        books,
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    if let Some(saved) = &setup.saved {
        // Saved-call caching stays inside this fixture's root.
        ctx.config_store = Some(rtao::client::config::Store::new(
            rtao::client::config::Paths::new(saved.store.path(), |_| None),
        ));
    }
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let mut history = PromptHistory::default();
    let request = Request::from_matches(&mut ctx, group, &mut history)?;
    assert!(matches!(request, Request::Emission(_)));
    let result = subnets::execute(
        &mut ctx,
        client,
        &request,
        SubmissionOptions {
            deadline: setup.deadline,
            ..Default::default()
        },
        signer.map(|signer| signer as &dyn ExternalSignerFactory),
        &mut history,
    )
    .await
    .map(|_| ());
    if let Err(error) = &result {
        ctx.output.cli_error(error)?;
    }
    Ok(Outcome {
        result,
        stdout: sink.text(Stream::Stdout)?,
        stderr: sink.text(Stream::Stderr)?,
    })
}

fn code(outcome: &Outcome) -> u8 {
    outcome.result.as_ref().err().map_or(0, |error| error.code)
}

fn baseline() -> f64 {
    ew::share_to_float(emission_root_baseline_share())
}

fn min_validator() -> u64 {
    ew::min_validator_parts(emission_root_baseline_share())
}

fn netuid() -> String {
    SUBNET.0.to_string()
}

const SHOW_FIELDS: [&str; 15] = [
    "netuid",
    "block",
    "weights",
    "shares",
    "global_tao_weight",
    "root_share",
    "baseline_root_share",
    "validator_share_after_root",
    "root_capped",
    "root_tao",
    "alpha_issuance",
    "min_validator",
    "created_via_lease",
    "limits",
    "rate_limit",
];

// pyt: unit/test_emission_weights.py::test_cli_show_json_has_stable_fields
// pyt: unit/test_emission_weights.py::test_cli_show_human_output
#[tokio::test]
async fn show_json_has_stable_fields_and_human_output_names_every_range() -> TestResult {
    let wallet = wallet()?;
    let (client, chain) = client(state()?).await?;
    let netuid = netuid();
    let shown = run(
        &client,
        wallet.path(),
        &["--json", "subnets", "emission", "show", "--netuid", &netuid],
    )
    .await?;
    assert_eq!(code(&shown), 0, "{}", shown.stderr);
    let record: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        record
            .as_object()
            .ok_or("record")?
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        SHOW_FIELDS
    );
    assert_eq!(record["weights"], emission_weights_info(SUBNET)["weights"]);
    assert!((record["shares"]["validator"].as_f64().ok_or("share")? - 0.41).abs() < 1e-4);
    assert_eq!(record["root_share"], json!(baseline()));
    assert_eq!(record["baseline_root_share"], json!(baseline()));
    assert_eq!(record["root_capped"], json!(false));
    assert_eq!(record["min_validator"], json!(min_validator()));
    assert_eq!(
        record["limits"]["validator"]["min_parts"],
        json!(min_validator())
    );
    assert_eq!(record["limits"]["validator"]["max"], json!(0.5));
    assert_eq!(record["limits"]["owner"]["min_parts"], json!(0));
    assert_eq!(record["limits"]["owner"]["max_parts"], json!(ew::MAX_OWNER));
    assert_eq!(
        record["limits"]["owner"]["why"],
        json!("0 turns the owner cut off")
    );
    assert_eq!(
        record["rate_limit"],
        json!({
            "limit_blocks": 50_400,
            "last_change_block": 0,
            "next_allowed_block": 100,
            "blocks_until_allowed": 0,
            "owner_can_change_now": true,
        })
    );

    let human = run(
        &client,
        wallet.path(),
        &["subnets", "emission", "show", "--netuid", &netuid],
    )
    .await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    let floor = ew::format_share(min_validator() - 1, 4);
    let lowest = ew::format_share(min_validator(), 4);
    let default_validator = ew::share_to_float(ew::default_weights(DEFAULT_OWNER)[ew::VALIDATOR]);
    for needle in [
        "Vector".to_owned(),
        "burn  0.00%".into(),
        "recycle  0.00%".into(),
        "owner  18.00%".into(),
        "validator  41.00%".into(),
        "miner  41.00%".into(),
        "Root".into(),
        "root share of emission  1.7".into(),
        "paid off the top of the validator slot".into(),
        "root baseline share  1.7".into(),
        "global TaoWeight (5.27%)".into(),
        "recomputed every block".into(),
        format!(
            "validators after root  {:.4}%",
            (default_validator - baseline()) * 100.0
        ),
        format!("validator floor  > {floor}"),
        "root tao  5,443,000 TAO".into(),
        "alpha issuance  6,457,000 alpha".into(),
        "next owner change  now".into(),
        "Allowed".into(),
        format!("validator  {lowest} to 50.0000%"),
        "miner  >= 33.00%".into(),
        "burn+recycle  <= 33.00%".into(),
        "sum  = 100%".into(),
        format!("set-value --netuid {netuid}"),
    ] {
        assert!(human.stdout.contains(&needle), "{needle}\n{}", human.stdout);
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

// pyt: unit/test_emission_weights.py::test_cli_show_reports_a_capped_root_share_and_a_closed_window
#[tokio::test]
async fn show_reports_a_capped_root_share_and_a_closed_window() -> TestResult {
    let wallet = wallet()?;
    let mut state = state()?;
    let low = ew::renormalize(
        &ew::default_weights(DEFAULT_OWNER),
        &[(ew::VALIDATOR, ew::parse_share("5%")?)],
    )?;
    let mut capped = emission_weights_info(SUBNET);
    capped["weights"] = json!(low);
    capped["owner"] = json!(low[ew::OWNER]);
    capped["validator"] = json!(low[ew::VALIDATOR]);
    capped["miner"] = json!(low[ew::MINER]);
    capped["root_baseline_share"] = json!(ew::parse_share("6%")?);
    capped["root_share"] = json!(low[ew::VALIDATOR]);
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_emission_weights",
        capped,
    )?;
    state.seed(
        &st::LAST_RATE_LIMITED_BLOCK,
        (rtao::reads::reads::subnets::emission_weights_rate_limit_key(SUBNET),),
        90u64,
    )?;
    let (client, _) = client(state).await?;
    let netuid = netuid();
    let human = run(
        &client,
        wallet.path(),
        &["subnets", "emission", "show", "--netuid", &netuid],
    )
    .await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    for needle in [
        "root share of emission  5.0000%  the baseline, capped at the validator slot",
        "root baseline share  6.0000%",
        "validators after root  0.0000%  nothing: raise the validator share",
        "validator floor  > 6.0000%",
        "validator  6.0000% to 50.0000%",
        "next owner change  block 50,490",
        "root is exempt",
    ] {
        assert!(human.stdout.contains(needle), "{needle}\n{}", human.stdout);
    }
    let record: Value = serde_json::from_str(
        &run(
            &client,
            wallet.path(),
            &["--json", "subnets", "emission", "show", "--netuid", &netuid],
        )
        .await?
        .stdout,
    )?;
    assert_eq!(record["root_capped"], json!(true));
    assert!((record["root_share"].as_f64().ok_or("root share")? - 0.05).abs() < 1e-9);
    assert!((record["baseline_root_share"].as_f64().ok_or("baseline")? - 0.06).abs() < 1e-9);
    assert_eq!(record["validator_share_after_root"], json!(0.0));
    assert_eq!(record["min_validator"], json!(ew::parse_share("6%")? + 1));
    assert_eq!(record["rate_limit"]["next_allowed_block"], json!(50_490));
    assert_eq!(record["rate_limit"]["owner_can_change_now"], json!(false));
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_cli_show_missing_subnet_exits_1
#[tokio::test]
async fn show_missing_subnet_exits_1() -> TestResult {
    let wallet = wallet()?;
    let mut state = state()?;
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_emission_weights",
        Value::Null,
    )?;
    let (client, _) = client(state).await?;
    let missing = run(
        &client,
        wallet.path(),
        &["--json", "subnets", "emission", "show", "--netuid", "7"],
    )
    .await?;
    assert_eq!(code(&missing), 1);
    let error: Value = serde_json::from_str(&missing.stderr)?;
    assert_eq!(
        error["error"],
        "subnet 7 does not exist or has no emission vector"
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_cli_set_value_dry_run_renormalizes
// pyt: unit/test_emission_weights.py::test_cli_increase_and_decrease_dry_run
// pyt: unit/test_emission_weights.py::test_cli_set_full_vector_dry_run
#[tokio::test]
async fn writes_dry_run_through_the_real_plan_with_structured_previews() -> TestResult {
    let wallet = wallet()?;
    let (client, chain) = client(state()?).await?;
    let netuid = netuid();
    let dry = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "--dry-run",
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "owner",
            "--value",
            "10%",
        ],
    )
    .await?;
    assert_eq!(code(&dry), 0, "{}", dry.stderr);
    let plan: Value = serde_json::from_str(&dry.stdout)?;
    assert_eq!(plan["ok"], json!(true), "{plan}");
    assert_eq!(plan["args"]["renormalize"], json!(true));
    assert_eq!(plan["args"]["slots"], json!([2]));
    let preview = &plan["preview"];
    assert_eq!(
        preview["before"]["weights"],
        emission_weights_info(SUBNET)["weights"]
    );
    assert_eq!(
        preview["after"]["weights"][2],
        json!(ew::parse_share("10%")?)
    );
    assert_eq!(
        preview["after"]["weights"]
            .as_array()
            .ok_or("weights")?
            .iter()
            .filter_map(Value::as_u64)
            .map(u128::from)
            .sum::<u128>(),
        u128::from(ONE)
    );
    assert_eq!(preview["changed"], json!(["owner", "validator", "miner"]));
    assert!((preview["root"]["share_before"].as_f64().ok_or("before")? - baseline()).abs() < 1e-12);
    assert!((preview["root"]["share_after"].as_f64().ok_or("after")? - baseline()).abs() < 1e-12);
    assert_eq!(preview["rate_limit"]["owner_can_change_now"], json!(true));
    assert!(preview["limits"].get("validator").is_some());

    let human = run(
        &client,
        wallet.path(),
        &[
            "--dry-run",
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "owner",
            "--value",
            "10%",
        ],
    )
    .await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    assert!(
        human
            .stdout
            .contains("owner 18.00% / validator 41.00% / miner 41.00% -> owner 10.00%"),
        "{}",
        human.stdout
    );
    assert!(human.stdout.contains(&format!(
        "root keeps its baseline {:.4}%",
        baseline() * 100.0
    )));
    assert!(human.stdout.contains("alpha validators get"));
    // A preview submits nothing, so nothing is read back.
    for preview in [&dry, &human] {
        assert!(!preview.stdout.contains("readback"), "{}", preview.stdout);
        assert!(!preview.stdout.contains("read back"), "{}", preview.stdout);
        assert!(!preview.stdout.contains("read-back"), "{}", preview.stdout);
    }

    for (verb, delta) in [("increase", 1i128), ("decrease", -1)] {
        let adjusted = run(
            &client,
            wallet.path(),
            &[
                "--json",
                "--dry-run",
                "subnets",
                "emission",
                verb,
                "--netuid",
                &netuid,
                "--slot",
                "validator",
                "--by",
                "5%",
            ],
        )
        .await?;
        assert_eq!(code(&adjusted), 0, "{verb}: {}", adjusted.stderr);
        let plan: Value = serde_json::from_str(&adjusted.stdout)?;
        assert_eq!(plan["ok"], json!(true), "{plan}");
        let expected = i128::from(ew::default_weights(DEFAULT_OWNER)[ew::VALIDATOR])
            + delta * i128::from(ew::parse_share("5%")?);
        assert_eq!(plan["args"]["values"][0], json!(expected));
        assert_eq!(
            plan["preview"]["after"]["weights"][ew::VALIDATOR],
            plan["args"]["values"][0]
        );
    }

    let full = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "--dry-run",
            "subnets",
            "emission",
            "set",
            "--netuid",
            &netuid,
            "--owner",
            "0.10",
            "--validator",
            "0.45",
            "--miner",
            "0.45",
        ],
    )
    .await?;
    assert_eq!(code(&full), 0, "{}", full.stderr);
    let plan: Value = serde_json::from_str(&full.stdout)?;
    assert_eq!(plan["ok"], json!(true), "{plan}");
    assert_eq!(plan["args"]["slots"], json!([0, 1, 2, 3, 4]));
    assert_eq!(
        plan["args"]["values"]
            .as_array()
            .ok_or("values")?
            .iter()
            .filter_map(Value::as_u64)
            .map(u128::from)
            .sum::<u128>(),
        u128::from(ONE)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_cli_explains_rule_violations_before_submitting
// pyt: unit/test_emission_weights.py::test_cli_decrease_rejects_more_than_the_slot_holds
#[tokio::test]
async fn rule_violations_and_bad_inputs_exit_2_before_planning() -> TestResult {
    let wallet = wallet()?;
    let (client, chain) = client(state()?).await?;
    let netuid = netuid();
    let owner = run(
        &client,
        wallet.path(),
        &[
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "owner",
            "--value",
            "40%",
        ],
    )
    .await?;
    assert_eq!(code(&owner), 2);
    assert!(
        owner.stderr.contains("OwnerEmissionAboveMaximum"),
        "{}",
        owner.stderr
    );
    assert!(owner.stderr.contains("may be at") && owner.stderr.contains("33.00%"));
    assert!(owner.stderr.contains(&format!(
        "allowed on subnet {netuid} right now: owner 0.00% to 33.00%"
    )));

    let too_low = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "validator",
            "--value",
            "1%",
        ],
    )
    .await?;
    assert_eq!(code(&too_low), 2);
    let error: Value = serde_json::from_str(&too_low.stderr)?;
    let text = error["error"].as_str().ok_or("error")?;
    assert!(
        text.starts_with(
            "the chain would reject this change: ValidatorEmissionTooLowToPreserveRoot"
        )
    );
    assert!(text.contains(&format!(
        "must be above {} or validators would get nothing",
        ew::format_share(min_validator() - 1, 4)
    )));
    assert!(error["note"].as_str().ok_or("note")?.starts_with(&format!(
        "allowed on subnet {netuid} right now: validator {} to 50.0000%",
        ew::format_share(min_validator(), 4)
    )));
    assert_eq!(
        error["help"],
        json!(format!(
            "`rtao subnets emission show --netuid {netuid}` prints the vector and every allowed range"
        ))
    );

    let over_burn = run(
        &client,
        wallet.path(),
        &[
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "burn",
            "--value",
            "40%",
        ],
    )
    .await?;
    assert_eq!(code(&over_burn), 2, "{}", over_burn.stderr);
    assert!(
        over_burn.stderr.contains("BurnRecycleEmissionAboveMaximum"),
        "{}",
        over_burn.stderr
    );
    assert!(
        over_burn.stderr.contains("burn+recycle <= 33.00%"),
        "{}",
        over_burn.stderr
    );

    let burn_ok = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "--dry-run",
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "burn",
            "--value",
            "10%",
        ],
    )
    .await?;
    assert_eq!(code(&burn_ok), 0, "{}", burn_ok.stderr);
    let burn_plan: Value = serde_json::from_str(&burn_ok.stdout)?;
    assert_eq!(burn_plan["ok"], json!(true), "{burn_plan}");
    assert_eq!(
        burn_plan["preview"]["after"]["weights"][ew::BURN],
        json!(ew::parse_share("10%")?)
    );

    let bad = run(
        &client,
        wallet.path(),
        &[
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "owner",
            "--value",
            "lots",
        ],
    )
    .await?;
    assert_eq!(code(&bad), 2);
    assert!(
        bad.stderr
            .contains("invalid value for `--value`: cannot read emission share"),
        "{}",
        bad.stderr
    );

    let decrease = run(
        &client,
        wallet.path(),
        &[
            "subnets", "emission", "decrease", "--netuid", &netuid, "--slot", "owner", "--by",
            "50%",
        ],
    )
    .await?;
    assert_eq!(code(&decrease), 2);
    assert!(decrease.stderr.contains("cannot take"));

    let off = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "subnets",
            "emission",
            "set",
            "--netuid",
            &netuid,
            "--owner",
            "10%",
            "--validator",
            "45%",
            "--miner",
            "40%",
        ],
    )
    .await?;
    assert_eq!(code(&off), 2);
    let error: Value = serde_json::from_str(&off.stderr)?;
    assert!(
        error["error"]
            .as_str()
            .ok_or("error")?
            .contains("must add up to 100%")
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

// pyt: unit/test_emission_weights.py::test_cli_rate_limited_owner_is_blocked_with_exit_1
#[tokio::test]
async fn rate_limited_owner_dry_run_is_blocked_with_exit_1() -> TestResult {
    let wallet = wallet()?;
    let mut state = state()?;
    state.seed(
        &st::LAST_RATE_LIMITED_BLOCK,
        (rtao::reads::reads::subnets::emission_weights_rate_limit_key(SUBNET),),
        90u64,
    )?;
    let (client, chain) = client(state).await?;
    let netuid = netuid();
    let blocked = run(
        &client,
        wallet.path(),
        &[
            "--json",
            "--dry-run",
            "subnets",
            "emission",
            "set-value",
            "--netuid",
            &netuid,
            "--slot",
            "owner",
            "--value",
            "10%",
        ],
    )
    .await?;
    assert_eq!(code(&blocked), 1);
    let plan: Value = serde_json::from_str(&blocked.stdout)?;
    assert_eq!(plan["ok"], json!(false));
    assert!(
        plan["violations"][0]
            .as_str()
            .ok_or("violation")?
            .contains("next owner change at block 50,490")
    );
    assert_eq!(
        plan["preview"]["rate_limit"]["owner_can_change_now"],
        json!(false)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

/// In-memory test key standing in for an external signer; no wallet secret exists.
struct Keys {
    key: KeypairSigner,
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
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            assert_eq!(role, SignerRole::Coldkey);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
struct Factory(Arc<Keys>);
impl Factory {
    fn new() -> Result<Self, SignerError> {
        Ok(Self(Arc::new(Keys {
            key: KeypairSigner::from_seed(&[23; 32], CryptoType::Ed25519)?,
        })))
    }
    fn account(&self) -> AccountId {
        self.0.key.account()
    }
}
impl ExternalSignerFactory for Factory {
    fn keys<'a>(
        &'a self,
        _: &'a Globals,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move { Ok(self.0.clone() as Arc<dyn DynKeySource>) })
    }
}

/// The emission fixture with the test key as the subnet owner.
fn owned_state(owner: AccountId) -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = state()?;
    state.seed(&st::SUBNET_OWNER, (SUBNET,), DefaultAccount::from(owner))?;
    Ok(state)
}

fn emission_info(weights: ew::Weights) -> Value {
    let mut info = emission_weights_info(SUBNET);
    info["weights"] = json!(weights);
    info["owner"] = json!(weights[ew::OWNER]);
    info["validator"] = json!(weights[ew::VALIDATOR]);
    info["miner"] = json!(weights[ew::MINER]);
    info
}

/// How one write lands on the fake chain.
#[derive(Clone, Copy)]
struct Landed {
    /// Weights of the receipt's SubnetEmissionWeightsSet event, if any.
    applied: Option<ew::Weights>,
    /// The vector the runtime API reports at the inclusion block.
    stored: ew::Weights,
    /// Fail every emission read once the extrinsic is included.
    fail_reads: bool,
}

/// Inclusion blocks of the writes, as (number, hash).
type Inclusions = Arc<Mutex<Vec<(u64, String)>>>;

/// Land each submitted extrinsic as `landed` says. Only the inclusion block
/// reports `stored`: every earlier block keeps the fixture's vector, and the
/// head moves one block past the inclusion where it is back to that vector
/// (as if a later change restored it), so a read-back at any block but the
/// inclusion one sees a different vector.
fn land_on_submit(state: &mut FakeState, landed: Landed) -> Inclusions {
    let inclusions = Inclusions::default();
    let seen = inclusions.clone();
    state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
        let mut events = Vec::new();
        if let Some(applied) = landed.applied {
            events.push(json!({"phase": "ApplyExtrinsic", "extrinsic_idx": 0, "module_id": "SubtensorModule", "event_id": "SubnetEmissionWeightsSet", "attributes": {"netuid": SUBNET.0, "weights": applied}, "topics": []}));
        }
        events.push(json!({"phase": "ApplyExtrinsic", "extrinsic_idx": 0, "module_id": "System", "event_id": "ExtrinsicSuccess", "attributes": {"dispatch_info": {"weight": state.weight, "class": "Normal", "pays_fee": "Yes"}}, "topics": []}));
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!(events),
        });
        let reply = state.dispatch(request);
        let number = state.block;
        let Some(hash) = state.block_hash(number) else {
            return Reply::error(-32602, "no inclusion block", None);
        };
        if let Err(error) = state.seed_runtime_at(
            hash.clone(),
            "SubnetInfoRuntimeApi",
            "get_subnet_emission_weights",
            emission_info(landed.stored),
        ) {
            return Reply::error(-32602, error.to_string(), None);
        }
        state.advance(1);
        if landed.fail_reads {
            state.set_handler("state_call", |state, request| {
                if request.params[0] == "SubnetInfoRuntimeApi_get_subnet_emission_weights" {
                    Reply::error(-32000, "emission read unavailable", None)
                } else {
                    state.dispatch(request)
                }
            });
        }
        if let Ok(mut seen) = seen.lock() {
            seen.push((number, hash));
        }
        reply
    });
    inclusions
}

fn landed(applied: ew::Weights) -> Landed {
    Landed {
        applied: Some(applied),
        stored: applied,
        fail_reads: false,
    }
}

fn last_inclusion(inclusions: &Inclusions) -> Result<(u64, String), Box<dyn std::error::Error>> {
    Ok(inclusions
        .lock()
        .map_err(|_| "inclusions")?
        .last()
        .cloned()
        .ok_or("no inclusion")?)
}

fn documents(stdout: &str) -> Result<Vec<Value>, serde_json::Error> {
    serde_json::Deserializer::from_str(stdout)
        .into_iter::<Value>()
        .collect()
}

fn owner_at(share: &str) -> Result<ew::Weights, Box<dyn std::error::Error>> {
    Ok(ew::renormalize(
        &ew::default_weights(DEFAULT_OWNER),
        &[(ew::OWNER, ew::parse_share(share)?)],
    )?)
}

fn set_owner<'a>(netuid: &'a str, value: &'a str) -> Vec<&'a str> {
    vec![
        "--yes",
        "--no-mev-shield",
        "subnets",
        "emission",
        "set-value",
        "--netuid",
        netuid,
        "--slot",
        "owner",
        "--value",
        value,
    ]
}

fn json_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    [&["--json"], args].concat()
}

#[tokio::test]
async fn finalized_write_reads_the_stored_vector_back_at_the_inclusion_block() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let expected = owner_at("10%")?;
    let mut state = owned_state(signer.account())?;
    let inclusions = land_on_submit(&mut state, landed(expected));
    let (client, chain) = client(state).await?;
    let netuid = netuid();
    let write = set_owner(&netuid, "10%");
    let written = run_with(&client, wallet.path(), &json_args(&write), Some(&signer)).await?;
    assert_eq!(code(&written), 0, "{}", written.stderr);
    let documents = documents(&written.stdout)?;
    assert_eq!(documents.len(), 2, "{}", written.stdout);
    let (receipt, readback) = (&documents[0], &documents[1]["readback"]);
    assert_eq!(receipt["success"], json!(true), "{receipt}");
    assert_eq!(readback["status"], json!("match"), "{readback}");
    assert_eq!(readback["matches"], json!(true));
    assert_eq!(readback["differs"], json!([]));
    assert_eq!(readback["netuid"], json!(SUBNET.0));
    assert_eq!(readback["submitted"], json!(expected));
    assert_eq!(readback["submitted_from"], json!("event"));
    assert_eq!(readback["previewed"], json!(expected));
    assert_eq!(readback["base_changed"], json!(false));
    assert_eq!(readback["stored"], json!(expected));
    assert!(
        (readback["stored_shares"]["owner"]
            .as_f64()
            .ok_or("owner share")?
            - 0.10)
            .abs()
            < 1e-9
    );
    // Read at the inclusion block, not at the head past it, which reports the
    // old vector again; reading anywhere else would be a mismatch.
    let (included, hash) = last_inclusion(&inclusions)?;
    assert_eq!(readback["block"], json!(included));
    assert_eq!(readback["block_hash"], json!(hash));
    assert_eq!(receipt["block_hash"], json!(hash));
    assert_eq!(chain.mutate_state(|state| state.block), included + 1);

    // The extrinsic carried the vector its event reports.
    let submissions = chain.mutate_state(|state| state.submissions.clone());
    assert_eq!(submissions.len(), 1);
    let runtime = View::runtime(&client).await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    assert_eq!(
        decoded["call"]["call_function"],
        json!("sudo_set_subnet_emission_weights"),
        "{decoded}"
    );
    assert_eq!(
        decoded["call"]["call_args"],
        json!([
            {"name": "netuid", "type": "NetUid", "value": SUBNET.0},
            {"name": "uids", "type": "Vec<u16>", "value": [0, 1, 2, 3, 4]},
            {"name": "values", "type": "Vec<u64>", "value": expected},
        ]),
        "{decoded}"
    );

    let human = run_with(&client, wallet.path(), &write, Some(&signer)).await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    for needle in [
        format!("subnet {netuid} emission weights read back at finalized block"),
        "Stored".into(),
        format!("owner  10.0000%  {} parts", expected[ew::OWNER]),
        format!(
            "validator  {}  {} parts",
            ew::format_share(expected[ew::VALIDATOR], 4),
            expected[ew::VALIDATOR]
        ),
        format!(
            "miner  {}  {} parts",
            ew::format_share(expected[ew::MINER], 4),
            expected[ew::MINER]
        ),
        "burn  0.0000%  0 parts".into(),
        "recycle  0.0000%  0 parts".into(),
        "result  matches the submitted vector exactly".into(),
    ] {
        assert!(human.stdout.contains(&needle), "{needle}\n{}", human.stdout);
    }
    assert!(!human.stdout.contains("submitted 0 parts"));
    assert!(!human.stdout.contains("base  changed"));
    assert!(!human.stdout.contains("compared with"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn read_back_mismatch_after_finality_exits_1_and_names_the_slots() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let submitted = owner_at("10%")?;
    // Another change landed in the same block after ours: owner ends at 12%.
    let stored = owner_at("12%")?;
    let mut state = owned_state(signer.account())?;
    land_on_submit(
        &mut state,
        Landed {
            applied: Some(submitted),
            stored,
            fail_reads: false,
        },
    );
    let (client, chain) = client(state).await?;
    let netuid = netuid();
    let write = set_owner(&netuid, "10%");
    let written = run_with(&client, wallet.path(), &json_args(&write), Some(&signer)).await?;
    assert_eq!(code(&written), 1, "{}", written.stderr);
    let error = written.result.as_ref().err().ok_or("mismatch error")?;
    assert!(
        error
            .submitted_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.success),
        "the receipt is retained"
    );
    let documents = documents(&written.stdout)?;
    assert_eq!(documents.len(), 2, "{}", written.stdout);
    let readback = &documents[1]["readback"];
    assert_eq!(readback["status"], json!("mismatch"), "{readback}");
    assert_eq!(readback["matches"], json!(false));
    assert_eq!(readback["differs"], json!(["owner", "validator", "miner"]));
    assert_eq!(readback["submitted"], json!(submitted));
    assert_eq!(readback["base_changed"], json!(false));
    assert_eq!(readback["stored"], json!(stored));
    let diagnostic: Value = serde_json::from_str(&written.stderr)?;
    let text = diagnostic["error"].as_str().ok_or("error")?;
    assert!(
        text.starts_with(&format!(
            "the emission vector stored on subnet {netuid} at finalized block"
        )),
        "{text}"
    );
    assert!(text.ends_with("differs from the one submitted (owner, validator, miner)"));
    assert_eq!(
        diagnostic["note"],
        json!(
            "another change to this subnet's emission weights landed in the same block after this one"
        )
    );
    assert!(
        diagnostic["help"]
            .as_str()
            .ok_or("help")?
            .contains(&format!("rtao subnets emission show --netuid {netuid}"))
    );
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);

    let human = run_with(&client, wallet.path(), &write, Some(&signer)).await?;
    assert_eq!(code(&human), 1, "{}", human.stderr);
    for needle in [
        format!(
            "owner  12.0000%  {} parts; submitted {} parts",
            stored[ew::OWNER],
            submitted[ew::OWNER]
        ),
        "result  differs from the submitted vector".into(),
    ] {
        assert!(human.stdout.contains(&needle), "{needle}\n{}", human.stdout);
    }
    assert!(human.stderr.contains("differs from the one submitted"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn read_back_compares_with_the_applied_event_and_reports_a_base_change_apart() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let netuid = netuid();
    let previewed = owner_at("10%")?;
    // The vector moved before the build: owner is still 10%, but the other
    // two were rescaled from a newer validator/miner split.
    let rebuilt = ew::renormalize(
        &ew::renormalize(
            &ew::default_weights(DEFAULT_OWNER),
            &[(ew::VALIDATOR, ew::parse_share("30%")?)],
        )?,
        &[(ew::OWNER, ew::parse_share("10%")?)],
    )?;
    assert_ne!(rebuilt, previewed);
    let mut state = owned_state(signer.account())?;
    land_on_submit(&mut state, landed(rebuilt));
    let (client, _) = client(state).await?;
    let write = set_owner(&netuid, "10%");
    let written = run_with(&client, wallet.path(), &json_args(&write), Some(&signer)).await?;
    assert_eq!(code(&written), 0, "{}", written.stderr);
    let readback = &documents(&written.stdout)?[1]["readback"];
    assert_eq!(readback["status"], json!("match"), "{readback}");
    assert_eq!(readback["submitted"], json!(rebuilt));
    assert_eq!(readback["submitted_from"], json!("event"));
    assert_eq!(readback["previewed"], json!(previewed));
    assert_eq!(readback["base_changed"], json!(true));
    let human = run_with(&client, wallet.path(), &write, Some(&signer)).await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    for needle in [
        "result  matches the submitted vector exactly",
        "base  changed before submission",
        "the change was rebuilt on a newer vector: burn 0.0000% / recycle 0.0000% / owner 10.0000%",
    ] {
        assert!(human.stdout.contains(needle), "{needle}\n{}", human.stdout);
    }
    client.close().await;

    // Without the event the preview is the only reference, and the note
    // cannot tell a moved base from a concurrent change.
    for (stored, status) in [(previewed, "match"), (rebuilt, "mismatch")] {
        let mut state = owned_state(signer.account())?;
        land_on_submit(
            &mut state,
            Landed {
                applied: None,
                stored,
                fail_reads: false,
            },
        );
        let (client, _) = crate::client(state).await?;
        let written = run_with(&client, wallet.path(), &json_args(&write), Some(&signer)).await?;
        let readback = &documents(&written.stdout)?[1]["readback"];
        assert_eq!(readback["status"], json!(status), "{readback}");
        assert_eq!(readback["submitted"], json!(previewed));
        assert_eq!(readback["submitted_from"], json!("preview"));
        assert_eq!(readback["base_changed"], json!(false));
        if status == "match" {
            assert_eq!(code(&written), 0, "{}", written.stderr);
            let human = run_with(&client, wallet.path(), &write, Some(&signer)).await?;
            assert!(
                human
                    .stdout
                    .contains("compared with  the previewed vector  the receipt has no SubnetEmissionWeightsSet event"),
                "{}",
                human.stdout
            );
        } else {
            assert_eq!(code(&written), 1, "{}", written.stderr);
            let diagnostic: Value = serde_json::from_str(&written.stderr)?;
            assert!(
                diagnostic["note"]
                    .as_str()
                    .ok_or("note")?
                    .contains("or the vector moved before submission"),
                "{diagnostic}"
            );
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn lagging_read_endpoint_reads_back_through_the_submit_endpoint() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let expected = owner_at("10%")?;
    let read = FakeChain::from_state(owned_state(signer.account())?);
    let mut node_state = owned_state(signer.account())?;
    let inclusions = land_on_submit(&mut node_state, landed(expected));
    let node = FakeChain::from_state(node_state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc_with_submit(
        RpcClient::new(read.clone()),
        RpcClient::new(node.clone()),
        config,
    )
    .await?;
    let netuid = netuid();
    let written = run_with(
        &client,
        wallet.path(),
        &json_args(&set_owner(&netuid, "10%")),
        Some(&signer),
    )
    .await?;
    assert_eq!(code(&written), 0, "{}", written.stderr);
    let documents = documents(&written.stdout)?;
    let readback = &documents[1]["readback"];
    assert_eq!(readback["status"], json!("match"), "{readback}");
    let (included, hash) = last_inclusion(&inclusions)?;
    assert_eq!(readback["block"], json!(included));
    assert_eq!(readback["block_hash"], json!(hash));
    // The read endpoint never imported the inclusion block, so the vector
    // came from the submission endpoint that reported it.
    assert!(read.mutate_state(|state| state.header(&hash).is_none()));
    assert!(
        node.requests().iter().any(|request| {
            request.method == "state_call"
                && request.params[0] == "SubnetInfoRuntimeApi_get_subnet_emission_weights"
                && request.params[2] == json!(hash)
        }),
        "the inclusion-block read reached the submission endpoint"
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn read_back_that_cannot_run_is_skipped_and_the_command_succeeds() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let netuid = netuid();
    let write = set_owner(&netuid, "10%");
    let applied = owner_at("10%")?;

    // A read error after the successful receipt.
    let mut state = owned_state(signer.account())?;
    land_on_submit(
        &mut state,
        Landed {
            fail_reads: true,
            ..landed(applied)
        },
    );
    let (client, _) = client(state).await?;
    let written = run_with(&client, wallet.path(), &json_args(&write), Some(&signer)).await?;
    assert_eq!(code(&written), 0, "{}", written.stderr);
    let documents = documents(&written.stdout)?;
    assert_eq!(documents.len(), 2, "{}", written.stdout);
    assert_eq!(documents[0]["success"], json!(true));
    let readback = &documents[1]["readback"];
    assert_eq!(readback["status"], json!("skipped"), "{readback}");
    assert_eq!(readback["matches"], Value::Null);
    assert!(
        readback["reason"]
            .as_str()
            .ok_or("reason")?
            .starts_with("the vector could not be read at the inclusion block"),
        "{readback}"
    );
    client.close().await;

    // Finality never reaches the inclusion block within the bound.
    let mut state = owned_state(signer.account())?;
    state.set_finalized(95)?;
    land_on_submit(&mut state, landed(applied));
    let (client, chain) = crate::client(state).await?;
    let bounded = Setup {
        deadline: Some(Duration::from_secs(2)),
        saved: None,
    };
    let written = run_setup(
        &client,
        wallet.path(),
        &json_args(&write),
        Some(&signer),
        &bounded,
    )
    .await?;
    assert_eq!(code(&written), 0, "{}", written.stderr);
    let documents = crate::documents(&written.stdout)?;
    assert_eq!(documents[0]["success"], json!(true), "{}", written.stdout);
    let readback = &documents[1]["readback"];
    assert_eq!(readback["status"], json!("skipped"), "{readback}");
    assert_eq!(
        readback["reason"],
        json!(
            "the inclusion block was not finalized within 2s; the change is included but not verified"
        )
    );
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    let human = run_setup(&client, wallet.path(), &write, Some(&signer), &bounded).await?;
    assert_eq!(code(&human), 0, "{}", human.stderr);
    for needle in [
        format!("subnet {netuid} emission weights read-back skipped"),
        "reason  the inclusion block was not finalized within 2s".into(),
    ] {
        assert!(human.stdout.contains(&needle), "{needle}\n{}", human.stdout);
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_multisig_reads_back_only_once_the_change_executes() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let other = KeypairSigner::from_seed(&[24; 32], CryptoType::Ed25519)?.account();
    let netuid = netuid();
    let applied = owner_at("10%")?;
    for threshold in [1u16, 2] {
        let members = [signer.account(), other];
        let (probe, _) = client(owned_state(signer.account())?).await?;
        let group = rtao::Executor::new(probe.clone())
            .multisig(&members, threshold)
            .await?;
        probe.close().await;
        // The multisig owns the subnet.
        let mut state = owned_state(group.address)?;
        land_on_submit(&mut state, landed(applied));
        let (client, chain) = client(state).await?;
        let setup = Setup {
            deadline: None,
            saved: Some(Saved {
                multisigs: json!([{"name": "team", "threshold": threshold, "signatories": members.iter().map(ToString::to_string).collect::<Vec<_>>()}]),
                signatories: vec![format!("{}=extension", signer.account())],
                store: TempRoot::new(),
            }),
        };
        let written = run_setup(
            &client,
            wallet.path(),
            &json_args(&set_owner(&netuid, "10%")),
            Some(&signer),
            &setup,
        )
        .await?;
        assert_eq!(code(&written), 0, "{threshold}: {}", written.stderr);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        let documents = documents(&written.stdout)?;
        let readback = &documents.last().ok_or("readback")?["readback"];
        if threshold == 1 {
            // as_multi_threshold_1 dispatched the change at once.
            assert_eq!(readback["status"], json!("match"), "{readback}");
        } else {
            assert_eq!(readback["status"], json!("skipped"), "{readback}");
            assert!(
                readback["reason"]
                    .as_str()
                    .ok_or("reason")?
                    .starts_with("the multisig approval is saved but has not executed"),
                "{readback}"
            );
        }
        client.close().await;
    }
    Ok(())
}
