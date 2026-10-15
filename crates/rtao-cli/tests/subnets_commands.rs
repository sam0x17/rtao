// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Registration routing and metagraph output preserve real SDK semantics.

use rtao::{
    client::{Client, ClientConfig, HeadProvider, Network},
    primitives::{AccountId, NetUid},
};
use rtao_cli::{
    application,
    books::Books,
    commands::subnets::{self, Request},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_intents::pow::registration_prefix;
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
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
fn parse(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(application::command_tree([subnets::command()?]).try_get_matches_from(args)?)
}
fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("subnets")?.1;
    let leaf = group.subcommand().ok_or("subnets leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/subnets-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &subnets::reserved(group)?, |_| None)?;
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

#[test]
fn registration_selects_root_or_burn_and_creation_keeps_the_real_registered_output() -> TestResult {
    let hotkey = AccountId([23; 32]).to_string();
    for (args, expected) in [
        (
            vec![
                "rtao", "s", "register", "--netuid", "0", "--hotkey", &hotkey, "--yes",
            ],
            "root_register",
        ),
        (
            vec![
                "rtao", "subnets", "register", "--netuid", "2", "--hotkey", &hotkey, "--yes",
            ],
            "burned_register",
        ),
        (
            vec!["rtao", "subnet", "create", "--hotkey", &hotkey, "--yes"],
            "register_subnet",
        ),
    ] {
        let parsed = parse(&args)?;
        let (mut ctx, _) = context(&parsed)?;
        let Request::Transaction(request) = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?
        else {
            return Err("expected transaction".into());
        };
        assert_eq!(request.op, expected);
        assert_eq!(request.intent()?.to_json()["hotkey_ss58"], hotkey);
        if expected == "root_register" {
            assert!(!request.args.contains_key("netuid"));
        }
    }
    Ok(())
}

#[test]
fn public_pow_inputs_keep_explicit_hotkey_limits_and_period() -> TestResult {
    let hotkey = AccountId([23; 32]).to_string();
    let parsed = parse(&[
        "rtao",
        "subnets",
        "register",
        "--netuid",
        "2",
        "--hotkey",
        &hotkey,
        "--pow",
        "--pow-workers",
        "1",
        "--pow-timeout",
        "1.25",
        "--pow-backend",
        "cpu",
        "--period",
        "16",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Pow(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected public proof request".into());
    };
    assert_eq!(request.netuid, NetUid(2));
    assert_eq!(request.hotkey, AccountId([23; 32]));
    assert_eq!(request.config.workers, 1);
    assert_eq!(request.config.timeout, Duration::from_millis(1250));
    assert_eq!(request.period, Some(16));
    Ok(())
}

#[test]
fn unsupported_pow_root_gpu_and_mining_preview_refuse_before_connection() -> TestResult {
    let hotkey = AccountId([23; 32]).to_string();
    for options in [
        vec!["--netuid", "0", "--pow"],
        vec!["--netuid", "1", "--pow", "--pow-backend", "gpu"],
        vec!["--netuid", "1", "--pow", "--pow-device", "0"],
        vec!["--netuid", "1", "--pow", "--dry-run"],
    ] {
        let mut args = vec!["rtao", "subnets", "register", "--hotkey", &hotkey, "--yes"];
        args.extend(options);
        let parsed = parse(&args)?;
        let (mut ctx, _) = context(&parsed)?;
        let error = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )
        .err()
        .ok_or("expected refusal")?;
        assert_eq!(error.code, 2);
    }
    for (flag, value) in [
        ("--pow-workers", "0"),
        ("--pow-workers", "33"),
        ("--pow-timeout", "NaN"),
        ("--pow-timeout", "3601"),
    ] {
        assert!(
            parse(&[
                "rtao", "subnets", "register", "--netuid", "1", "--hotkey", &hotkey, "--pow", flag,
                value
            ])
            .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn public_pow_preparation_binds_the_explicit_hotkey_and_proxy_owner_without_unlocking()
-> TestResult {
    let hotkey = AccountId([23; 32]);
    let owner = AccountId([9; 32]);
    let hotkey_text = hotkey.to_string();
    let parsed = parse(&[
        "rtao",
        "subnets",
        "register",
        "--netuid",
        "2",
        "--hotkey",
        &hotkey_text,
        "--pow",
        "--pow-workers",
        "1",
        "--pow-timeout",
        "2",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    ctx.config.insert("proxy_for".into(), json!(owner));
    let Request::Pow(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected proof request".into());
    };
    let mut state = FakeState::try_default()?;
    for (storage, value) in [
        ("NetworksAdded", json!(true)),
        ("NetworkPowRegistrationAllowed", json!(true)),
        ("Difficulty", json!(1)),
        ("MinDifficulty", json!(1)),
        ("MaxDifficulty", json!(u64::MAX)),
        ("MaxAllowedUids", json!(256)),
        ("MaxRegistrationsPerBlock", json!(1)),
        ("RegistrationsThisBlock", json!(0)),
    ] {
        state.seed_python("SubtensorModule", storage, &[json!(2)], value)?;
    }
    // Python's permissive fake defaults Uids to zero, which is a registered
    // uid. This preparation fixture needs a real optional-map miss instead.
    let uids = state
        .runtime
        .storage_entry("SubtensorModule", "Uids")
        .ok_or("Uids storage")?;
    let key = state.runtime.storage_key(
        uids,
        &[
            rtao::core::codec::Value::Int(2),
            rtao::core::codec::Value::Bytes(hotkey.0.to_vec()),
        ],
    )?;
    state.seed_encoded(key, None);
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let prepared = subnets::prepare_pow(&ctx, &client, &request).await?;
    assert_eq!(prepared.op, "pow_register");
    assert_eq!(prepared.args["hotkey_ss58"], json!(hotkey));
    assert_eq!(prepared.args["mining_workers"], 1);
    assert_eq!(prepared.args["mining_timeout"], 2.0);
    let block = prepared.args["work_block"].as_u64().ok_or("work block")?;
    let nonce = prepared.args["nonce"].as_u64().ok_or("nonce")?;
    let hash = client
        .rpc()
        .block_hash(block)
        .await?
        .ok_or("challenge hash")?;
    let prefix = registration_prefix(NetUid(2), hash, hotkey, owner)?;
    let seal = rtao::core::pow::seal(&prefix, nonce)?;
    assert_eq!(
        prepared.args["work_hex"],
        json!(format!("0x{}", hex::encode(seal)))
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

fn graph() -> Value {
    let mut graph = serde_json::Map::new();
    for key in [
        "network_registered_at",
        "block",
        "tempo",
        "last_step",
        "blocks_since_last_step",
        "tao_in",
        "alpha_in",
        "alpha_out",
        "subnet_volume",
        "subnet_emission",
        "tao_in_emission",
        "pending_root_emission",
        "alpha_in_emission",
        "alpha_out_emission",
        "pending_alpha_emission",
        "registration_allowed",
        "pow_registration_allowed",
        "burn",
        "min_burn",
        "max_burn",
        "difficulty",
        "min_difficulty",
        "max_difficulty",
        "immunity_period",
        "adjustment_interval",
        "adjustment_alpha",
        "target_regs_per_interval",
        "max_regs_per_block",
        "serving_rate_limit",
        "rho",
        "kappa",
        "min_allowed_weights",
        "max_weights_limit",
        "weights_version",
        "weights_rate_limit",
        "activity_cutoff",
        "max_validators",
        "commit_reveal_weights_enabled",
        "commit_reveal_period",
        "liquid_alpha_enabled",
        "alpha_high",
        "alpha_low",
        "bonds_moving_avg",
    ] {
        graph.insert(key.into(), json!(1));
    }
    graph.extend([
        ("name".into(), json!("τ subnet")),
        ("symbol".into(), json!("β")),
        ("owner_hotkey".into(), json!(AccountId([3; 32]))),
        ("owner_coldkey".into(), json!(AccountId([4; 32]))),
        ("block".into(), json!(100)),
        ("registration_allowed".into(), json!(true)),
        ("pow_registration_allowed".into(), json!(true)),
        ("commit_reveal_weights_enabled".into(), json!(true)),
        ("liquid_alpha_enabled".into(), json!(true)),
        (
            "hotkeys".into(),
            json!([AccountId([21; 32]), AccountId([22; 32])]),
        ),
        (
            "total_stake".into(),
            json!([9_007_199_254_740_992u64, 9_007_199_254_740_993u64]),
        ),
        ("emission".into(), json!([1, 2])),
        ("incentives".into(), json!([0, 65535])),
        ("dividends".into(), json!([65535, 0])),
        ("validator_permit".into(), json!([true, false])),
        ("active".into(), json!([true, true])),
        ("last_update".into(), json!([90, 101])),
        (
            "identities".into(),
            json!([{"name":"chain-only-name"}, {"name":"overridden-chain-name"}]),
        ),
        (
            "axons".into(),
            json!([{"ip":2130706433u64,"port":8091,"ip_type":4},{"ip":1,"port":8092,"ip_type":6}]),
        ),
        ("trust".into(), json!([42, 43])),
    ]);
    Value::Object(graph)
}

#[test]
fn metagraph_json_keeps_the_raw_record_and_native_integer_precision() -> TestResult {
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let graph = graph();
    subnets::render_metagraph(&output, NetUid(1), &graph, &BTreeMap::new())?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        graph
    );
    assert!(subnets::render_metagraph(&output, NetUid(1), &Value::Null, &BTreeMap::new()).is_err());
    Ok(())
}

#[test]
fn human_metagraph_sorts_exact_stake_and_prefers_local_names_with_ipv4_and_ipv6_axons() -> TestResult
{
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: false,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let local = BTreeMap::from([(AccountId([22; 32]).to_string(), "higher-stake-local".into())]);
    let mut graph = graph();
    graph["subnet_volume"] = json!(u128::MAX);
    subnets::render_metagraph(&output, NetUid(1), &graph, &local)?;
    let text = sink.text(Stream::Stdout)?;
    assert!(
        text.find("higher-stake-local").ok_or("local name")?
            < text.find("chain-only-name").ok_or("chain name")?
    );
    assert!(!text.contains("overridden-chain-name"));
    assert!(text.contains("127.0.0.1:8091"));
    assert!(text.contains("[::1]:8092"));
    assert!(text.contains("1 validator permits · 2 active"));
    assert!(text.contains("340282366920938463463374607431.768211455"));
    graph["total_stake"][0] = json!(u128::from(u64::MAX) + 1);
    assert!(subnets::render_metagraph(&output, NetUid(1), &graph, &local).is_err());
    Ok(())
}

#[tokio::test]
async fn metagraph_command_uses_the_actual_typed_read_and_never_submits() -> TestResult {
    let mut state = FakeState::try_default()?;
    let mut wire = graph();
    wire["name"] = json!("τ subnet".as_bytes());
    wire["symbol"] = json!("β".as_bytes());
    state.seed_runtime("SubnetInfoRuntimeApi", "get_metagraph", wire)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let parsed = parse(&["rtao", "subnets", "metagraph", "1", "--json"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?;
    assert!(
        subnets::execute(
            &mut ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            None,
            &mut PromptHistory::default()
        )
        .await?
        .is_none()
    );
    let actual: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(actual["name"], "τ subnet");
    assert_eq!(actual["symbol"], "β");
    assert_eq!(actual["trust"], json!([42, 43]));
    assert_eq!(actual["total_stake"][1], json!(9_007_199_254_740_993u64));
    assert!(
        chain
            .requests()
            .iter()
            .any(|request| request.method == "state_call"
                && request.params[0] == "SubnetInfoRuntimeApi_get_metagraph")
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

#[test]
fn human_metagraph_spot_price_rounds_the_integer_quotient_once() -> TestResult {
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: false,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let mut graph = graph();
    graph["tao_in"] = json!(9_007_199_254_740_995_u64);
    graph["alpha_in"] = json!(3_u64);
    subnets::render_metagraph(&output, NetUid(1), &graph, &BTreeMap::new())?;
    // The exact quotient is 3_002_399_751_580_331 + 2/3. Binary64 has a
    // half-unit spacing here, so the nearest result ends in .5. Converting
    // the numerator first rounds it upward and incorrectly yields ...332.
    assert!(
        sink.text(Stream::Stdout)?
            .contains("3002399751580331.500000 τ per alpha (spot)")
    );
    Ok(())
}
