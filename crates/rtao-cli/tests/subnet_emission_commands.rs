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
    FakeChain, FakeState, Reply,
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
    let mut history = PromptHistory::default();
    let request = Request::from_matches(&mut ctx, group, &mut history)?;
    assert!(matches!(request, Request::Emission(_)));
    let result = subnets::execute(
        &mut ctx,
        client,
        &request,
        SubmissionOptions::default(),
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
        "owner  18.00%".into(),
        "validator  41.00%".into(),
        "miner  41.00%".into(),
        "reserved, pinned to 0".into(),
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

    let pinned = run(
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
            "1%",
        ],
    )
    .await?;
    assert_eq!(code(&pinned), 2);
    assert!(pinned.stderr.contains("pinned to 0"));

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

/// Once an extrinsic is submitted the runtime API reports `stored`, as the
/// chain would after applying a change (ours, or another that landed with it).
fn store_on_submit(state: &mut FakeState, stored: ew::Weights) {
    state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
        let mut info = emission_weights_info(SUBNET);
        info["weights"] = json!(stored);
        info["owner"] = json!(stored[ew::OWNER]);
        info["validator"] = json!(stored[ew::VALIDATOR]);
        info["miner"] = json!(stored[ew::MINER]);
        match state.seed_runtime("SubnetInfoRuntimeApi", "get_subnet_emission_weights", info) {
            Ok(()) => state.dispatch(request),
            Err(error) => Reply::error(-32602, error.to_string(), None),
        }
    });
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

#[tokio::test]
async fn finalized_write_reads_the_stored_vector_back_and_reports_a_match() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let expected = owner_at("10%")?;
    let mut state = owned_state(signer.account())?;
    store_on_submit(&mut state, expected);
    let (client, chain) = client(state).await?;
    let netuid = netuid();
    let write = [
        "--yes",
        "--no-mev-shield",
        "subnets",
        "emission",
        "set-value",
        "--netuid",
        &netuid,
        "--slot",
        "owner",
        "--value",
        "10%",
    ];
    let json_args = [&["--json"], &write[..]].concat();
    let written = run_with(&client, wallet.path(), &json_args, Some(&signer)).await?;
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
    assert_eq!(readback["stored"], json!(expected));
    assert!(
        (readback["stored_shares"]["owner"]
            .as_f64()
            .ok_or("owner share")?
            - 0.10)
            .abs()
            < 1e-9
    );
    // Read at the finalized inclusion block the receipt names.
    assert_eq!(readback["block_hash"], receipt["block_hash"]);
    let included = chain.mutate_state(|state| state.block);
    assert_eq!(readback["block"], json!(included));

    // The vector read back is compared with the one the extrinsic carried.
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
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn read_back_mismatch_after_finality_exits_1_and_names_the_slots() -> TestResult {
    let wallet = wallet()?;
    let signer = Factory::new()?;
    let submitted = owner_at("10%")?;
    // Another change landed with ours: the chain keeps owner at 12%.
    let stored = owner_at("12%")?;
    let mut state = owned_state(signer.account())?;
    store_on_submit(&mut state, stored);
    let (client, chain) = client(state).await?;
    let netuid = netuid();
    let written = run_with(
        &client,
        wallet.path(),
        &[
            "--json",
            "--yes",
            "--no-mev-shield",
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
        Some(&signer),
    )
    .await?;
    assert_eq!(code(&written), 1, "{}", written.stderr);
    let error = written.result.as_ref().err().ok_or("mismatch error")?;
    assert!(
        error
            .submitted_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.success),
        "the finalized receipt is retained"
    );
    let documents = documents(&written.stdout)?;
    assert_eq!(documents.len(), 2, "{}", written.stdout);
    let readback = &documents[1]["readback"];
    assert_eq!(readback["status"], json!("mismatch"), "{readback}");
    assert_eq!(readback["matches"], json!(false));
    assert_eq!(readback["differs"], json!(["owner", "validator", "miner"]));
    assert_eq!(readback["submitted"], json!(submitted));
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
    assert!(
        diagnostic["note"]
            .as_str()
            .ok_or("note")?
            .contains("another change")
    );
    assert!(
        diagnostic["help"]
            .as_str()
            .ok_or("help")?
            .contains(&format!("rtao subnets emission show --netuid {netuid}"))
    );
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);

    let human = run_with(
        &client,
        wallet.path(),
        &[
            "--yes",
            "--no-mev-shield",
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
        Some(&signer),
    )
    .await?;
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
