// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/metagraph_view.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

// Public fixture files live in the recorded test root and have no process,
// service, symlink, configuration or private-wallet dependencies.
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
fn pow_root_preview_and_feature_gated_gpu_are_validated_before_connection() -> TestResult {
    let hotkey = AccountId([23; 32]).to_string();
    for (options, gpu_request) in [
        (vec!["--netuid", "0", "--pow"], false),
        (vec!["--netuid", "1", "--pow", "--pow-backend", "gpu"], true),
        (vec!["--netuid", "1", "--pow", "--pow-device", "0"], true),
        (vec!["--netuid", "1", "--pow", "--dry-run"], false),
    ] {
        let mut args = vec!["rtao", "subnets", "register", "--hotkey", &hotkey, "--yes"];
        args.extend(options);
        let parsed = parse(&args)?;
        let (mut ctx, _) = context(&parsed)?;
        let result = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        );
        if gpu_request && cfg!(feature = "pow-gpu") {
            assert!(matches!(result?, Request::Pow(_)));
        } else {
            assert_eq!(result.err().ok_or("expected refusal")?.code, 2);
        }
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
        "--pow-backend",
        "cpu",
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
    graph["identity"] = json!({
        "description":"subnet-details", "true_flag":true,
        "nested_value":{"key":["quoted",true]},
        "false_flag":false,"zero_value":0,"empty_string":"",
        "empty_array":[],"empty_object":{},"null_value":null,
    });
    graph["burn"] = json!(1_500_000_000u64);
    graph["min_burn"] = json!(1u64);
    graph["max_burn"] = json!(u64::MAX);
    graph["moving_price"] = json!({"bits":-6_442_450_944i128});
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
    assert!(text.contains("registered_at"));
    assert!(text.contains("1  1 blocks ago"));
    assert!(text.contains("subnet-details"));
    assert!(text.contains("true_flag"));
    assert!(text.contains("{'key': ['quoted', True]}"));
    for omitted in [
        "false_flag",
        "zero_value",
        "empty_string",
        "empty_array",
        "empty_object",
        "null_value",
    ] {
        assert!(!text.contains(omitted), "false identity field {omitted}");
    }
    assert!(text.contains("-1.500000"));
    for key in [
        "subnet_emission",
        "tao_in_emission",
        "alpha_in_emission",
        "alpha_out_emission",
    ] {
        assert!(
            text.lines()
                .any(|line| line.contains(key) && line.ends_with("per block"))
        );
    }
    for key in ["pending_alpha_emission", "pending_root_emission"] {
        assert!(
            text.lines()
                .any(|line| line.contains(key) && line.ends_with("undistributed"))
        );
    }
    for reading in [
        "1500000000  = τ1.500000000",
        "1  = τ0.000000001",
        "18446744073709551615  = τ18,446,744,073.709552765",
    ] {
        assert!(text.contains(reading), "burn reading {reading}");
    }
    assert!(text.contains("rtao subnets hyperparameters 1"));
    graph["total_stake"][0] = json!(u128::from(u64::MAX) + 1);
    assert!(subnets::render_metagraph(&output, NetUid(1), &graph, &local).is_err());
    Ok(())
}

#[tokio::test]
async fn human_metagraph_execution_uses_first_natural_hotkey_then_contacts() -> TestResult {
    // py: sdk/python/bittensor/cli/metagraph_view.py:154-198 (_neurons)
    let root = TempRoot::new();
    for (wallet, hotkey) in [
        ("wallet10", "worker1"),
        ("wallet2", "worker10"),
        ("wallet2", "worker2"),
    ] {
        let directory = root.path().join(wallet).join("hotkeys");
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join(hotkey),
            serde_json::to_vec(&json!({"ss58Address":AccountId([21;32]),"cryptoType":1}))?,
        )?;
    }
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
    let parsed = parse(&["rtao", "subnets", "metagraph", "1"])?;
    let (mut ctx, sink) = context(&parsed)?;
    ctx.globals.wallet_path = root.path().to_owned();
    ctx.books.addresses = vec![
        json!({"address":AccountId([21;32]),"name":"contact-cannot-replace-hotkey"}),
        json!({"address":AccountId([22;32]),"name":"first-contact"}),
        json!({"address":AccountId([22;32]),"name":"second-contact"}),
        json!({"address":AccountId([22;32]),"name":""}),
    ];
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
            &mut PromptHistory::default(),
        )
        .await?
        .is_none()
    );
    let text = sink.text(Stream::Stdout)?;
    assert!(text.contains("(worker2)"));
    assert!(text.contains("(first-contact)"));
    for overridden in [
        "wallet2/",
        "wallet10/",
        "worker10",
        "worker1)",
        "second-contact",
        "contact-cannot-replace-hotkey",
    ] {
        assert!(!text.contains(overridden), "unexpected name {overridden}");
    }
    assert!(
        text.find("first-contact").ok_or("contact")?
            < text.find("worker2").ok_or("wallet hotkey")?
    );
    // Natural-equal wallet names retain their directory order as complete
    // groups. Flattening both groups would wrongly prefer worker1 over worker9.
    let tied_root = TempRoot::new();
    for wallet in ["wallet2", "wallet02"] {
        fs::create_dir(tied_root.path().join(wallet))?;
    }
    let order = fs::read_dir(tied_root.path())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(order.len(), 2);
    for (wallet, hotkey) in [
        (order[0].as_str(), "worker9"),
        (order[1].as_str(), "worker1"),
    ] {
        let directory = tied_root.path().join(wallet).join("hotkeys");
        fs::create_dir(&directory)?;
        fs::write(
            directory.join(hotkey),
            serde_json::to_vec(&json!({"ss58Address":AccountId([21;32]),"cryptoType":1}))?,
        )?;
    }
    let (mut tied_ctx, tied_sink) = context(&parsed)?;
    tied_ctx.globals.wallet_path = tied_root.path().to_owned();
    tied_ctx.books.addresses = ctx.books.addresses.clone();
    assert!(
        subnets::execute(
            &mut tied_ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await?
        .is_none()
    );
    let tied_text = tied_sink.text(Stream::Stdout)?;
    assert!(tied_text.contains("(worker9)"));
    assert!(tied_text.contains("(first-contact)"));
    for overridden in [
        "worker1)",
        "second-contact",
        "contact-cannot-replace-hotkey",
        "chain-only-name",
    ] {
        assert!(
            !tied_text.contains(overridden),
            "unexpected name {overridden}"
        );
    }
    assert!(
        tied_text.find("first-contact").ok_or("contact")?
            < tied_text.find("worker9").ok_or("group-first hotkey")?
    );
    let requests = chain.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "state_call"
                && request.params[0] == "SubnetInfoRuntimeApi_get_metagraph")
            .count(),
        2
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[test]
fn human_metagraph_empty_and_short_columns_keep_summary_and_native_endpoints() -> TestResult {
    // py: sdk/python/bittensor/cli/metagraph_view.py:28-49,154-211
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
    graph["hotkeys"] = json!([]);
    graph["num_uids"] = json!(0);
    graph["max_uids"] = json!(12);
    graph["alpha_in"] = json!(0);
    subnets::render_metagraph(&output, NetUid(1), &graph, &BTreeMap::new())?;
    let text = sink.text(Stream::Stdout)?;
    assert!(text.contains("neurons 0/12"));
    assert!(text.contains("0 uids · 0 validator permits · 0 active"));
    assert!(text.contains("rtao subnets hyperparameters 1"));
    assert!(!text.contains("τ per alpha (spot)"));
    let mut sparse = self::graph();
    for key in [
        "total_stake",
        "emission",
        "incentives",
        "dividends",
        "last_update",
        "validator_permit",
        "active",
        "identities",
        "axons",
    ] {
        sparse[key] = json!([]);
    }
    subnets::render_metagraph(&output, NetUid(1), &sparse, &BTreeMap::new())?;
    assert!(
        sink.text(Stream::Stdout)?
            .contains("2 uids · 0 validator permits · 0 active")
    );
    sparse["axons"] = json!([{"ip":u64::from(u32::MAX)+1,"ip_type":4,"port":80}]);
    assert!(subnets::render_metagraph(&output, NetUid(1), &sparse, &BTreeMap::new()).is_err());
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

#[test]
fn inspection_arguments_match_python_and_hparams_alias_stays_hidden() -> TestResult {
    for mut args in [
        vec!["rtao", "s", "list"],
        vec!["rtao", "subnets", "show", "7"],
        vec!["rtao", "subnets", "hparams", "7", "--name", "kappa"],
        vec!["rtao", "subnets", "burn-cost", "0"],
        vec!["rtao", "subnets", "create-cost"],
        vec!["rtao", "subnets", "price", "--netuid", "7"],
        vec!["rtao", "subnets", "conviction", "--netuid", "7", "--all"],
    ] {
        args.push("--json");
        let parsed = parse(&args)?;
        let (mut ctx, _) = context(&parsed)?;
        assert!(ctx.globals.json);
        let Request::Inspect(request) = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?
        else {
            return Err("expected inspection".into());
        };
        if args.contains(&"hparams") {
            assert_eq!(request.command, "hyperparameters");
            assert_eq!(request.name.as_deref(), Some("kappa"));
        }
        if args.contains(&"--all") {
            assert!(request.all);
        }
    }
    for args in [
        vec!["rtao", "subnets", "show", "--netuid", "7"],
        vec!["rtao", "subnets", "price", "7"],
        vec!["rtao", "subnets", "show", "4096"],
        vec!["rtao", "subnets", "show", "-1"],
    ] {
        assert!(parse(&args).is_err());
    }
    let mut command = subnets::command()?;
    let help = command.render_long_help().to_string();
    assert!(help.contains("hyperparameters"));
    assert!(!help.contains("hparams"));
    let parsed = parse(&[
        "rtao",
        "--network",
        "finney",
        "subnets",
        "show",
        "7",
        "--network",
        "test",
        "--json",
    ])?;
    let (ctx, _) = context(&parsed)?;
    assert_eq!(ctx.globals.network, "test");
    assert!(ctx.globals.json);
    Ok(())
}

#[test]
fn registration_age_and_conviction_eta_keep_gate_boundaries() {
    assert_eq!(subnets::registered_note(10, 10), None);
    assert_eq!(subnets::registered_note(11, 10), None);
    assert_eq!(
        subnets::registered_note(10, 11).as_deref(),
        Some("just now")
    );
    assert_eq!(subnets::registered_note(10, 15).as_deref(), Some("~1m ago"));
    assert_eq!(
        subnets::registered_note(0, 7200).as_deref(),
        Some("~1d ago")
    );
    assert_eq!(subnets::conviction_eta(None), "won't clear 18%");
    assert_eq!(subnets::conviction_eta(Some(0)), "above 18%");
    assert_eq!(subnets::conviction_eta(Some(1)), "18% in ~1m");
    assert_eq!(subnets::conviction_eta(Some(300)), "18% in ~1h");
    assert_eq!(subnets::conviction_eta(Some(7200)), "18% in ~1d");
}

#[tokio::test]
async fn inspection_cost_and_price_use_real_reads_without_author_calls() -> TestResult {
    let mut state = FakeState::try_default()?;
    state.seed_python("SubtensorModule", "NetworksAdded", &[json!(7)], json!(true))?;
    state.seed_python(
        "SubtensorModule",
        "Burn",
        &[json!(7)],
        json!(1_500_000_000u64),
    )?;
    state.seed_runtime(
        "SubnetRegistrationRuntimeApi",
        "get_network_registration_cost",
        json!(2_000_000_001u64),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price",
        json!(9_007_199_254_740_993u64),
    )?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    for (args, expected) in [
        (
            vec!["rtao", "subnets", "burn-cost", "7", "--json"],
            json!({"burn_cost":"τ1.500000000","tao":1.5}),
        ),
        (
            vec!["rtao", "subnets", "create-cost", "--json"],
            json!({"create_cost":"τ2.000000001","tao":2.000000001}),
        ),
        (
            vec!["rtao", "subnets", "price", "--netuid", "7", "--json"],
            json!({"netuid":7,"price_rao":9_007_199_254_740_993u64,"tao_per_alpha":rtao::primitives::pyfmt::rao_to_f64(9_007_199_254_740_993)}),
        ),
    ] {
        let parsed = parse(&args)?;
        let (mut ctx, sink) = context(&parsed)?;
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        subnets::execute(
            &mut ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await?;
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            expected
        );
    }
    assert!(
        !chain
            .requests()
            .iter()
            .any(|r| r.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[test]
fn conviction_json_keeps_dust_and_uses_per_hotkey_gate_records() -> TestResult {
    use rtao::primitives::DynBalance;
    use rtao::reads::reads::m6_locks::{ConvictionEntry, SubnetConvictions};
    let netuid = NetUid(7);
    let balance = |rao| DynBalance::from_rao(rao, netuid);
    let key = AccountId([21; 32]);
    let mut data = SubnetConvictions {
        netuid,
        block: 100,
        alpha_out: balance(10_000_000_000),
        protocol_alpha: balance(0),
        alpha_burned: balance(0),
        eligible_alpha: balance(10_000_000_000),
        threshold_alpha: balance(1_800_000_000),
        total_locked_alpha: balance(1),
        total_conviction_alpha: balance(1),
        leader_hotkey: Some(key),
        leader_blocks_to_threshold: None,
        unlock_rate: 100,
        maturity_rate: 50,
        owner_hotkey: Some(key),
        registered_at: 1,
        ownership_changeable_at_block: 2_628_001,
        hotkeys: vec![ConvictionEntry {
            hotkey: key,
            is_owner: true,
            locked_alpha: balance(1),
            conviction_alpha: balance(1),
            pct_of_threshold: Some(1.0 / 1_800_000_000.0),
            blocks_to_threshold: None,
        }],
    };
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    subnets::render_convictions(&output, &data, false)?;
    let actual: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(actual["hotkeys"][0]["hotkey"], json!(key));
    assert_eq!(
        actual["hotkeys"][0]["locked_alpha"]
            .as_f64()
            .map(f64::to_bits),
        Some(0.000000001_f64.to_bits())
    );
    assert_eq!(actual["hotkeys"][0]["locked_alpha"].to_string(), "1e-09");
    assert_eq!(actual["hotkeys"][0]["blocks_to_threshold"], Value::Null);
    assert!(actual.get("protocol_alpha").is_none());
    let machine = sink.text(Stream::Stdout)?;
    assert!(sink.text(Stream::Stderr)?.is_empty());
    let quiet_json = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: true,
            network: None,
        },
        Arc::new(quiet_json.clone()),
    );
    subnets::render_convictions(&output, &data, false)?;
    assert_eq!(quiet_json.text(Stream::Stdout)?, machine);
    assert!(quiet_json.text(Stream::Stderr)?.is_empty());
    for (block, open) in [(100, false), (2_628_001, true)] {
        data.block = block;
        let mut ordinary = None;
        for quiet in [false, true] {
            let human = MemorySink::default();
            let output = Output::new(
                OutputOptions {
                    json_mode: false,
                    quiet,
                    network: None,
                },
                Arc::new(human.clone()),
            );
            subnets::render_convictions(&output, &data, false)?;
            let text = human.text(Stream::Stdout)?;
            if quiet {
                assert_eq!(Some(&text), ordinary.as_ref());
                assert!(human.text(Stream::Stderr)?.is_empty());
            } else {
                ordinary = Some(text.clone());
                assert_eq!(
                    human.text(Stream::Stderr)?,
                    "1 negligible lock hidden (pass `--all` to show)\n"
                );
            }
            assert!(!text.contains("negligible lock"));
            assert!(text.contains("gate is per-hotkey"));
            assert!(text.contains("won't clear 18%"));
            assert!(
                text.contains("its lock always counts conviction equal to its full locked mass")
            );
            assert!(text.contains("conviction closes half its gap to the locked mass"));
            assert!(text.contains("a decaying lock's mass halves every ~0d"));
            assert!(text.contains("rtao conviction add --netuid 7 --amount <alpha>"));
            assert!(text.contains("score that gates subnet ownership"));
            assert!(text.contains("rates and alpha accounting stay constant"));
            assert!(text.contains(&format!("{}/guides/conviction", rtao_catalog::py::DOCS_URL)));
            if open {
                assert!(text.contains("open — registered at block 1"));
                assert!(text.contains("the one-year age gate has passed"));
                assert!(!text.contains("d away"));
            } else {
                assert!(text.contains("block 2628001 — ~365d away"));
                assert!(text.contains("a subnet must be one year old before ownership can change"));
                assert!(!text.contains("age gate has passed"));
            }
        }
    }
    let all = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: false,
            quiet: false,
            network: None,
        },
        Arc::new(all.clone()),
    );
    subnets::render_convictions(&output, &data, true)?;
    assert!(
        !all.text(Stream::Stdout)?
            .contains("negligible locks hidden")
    );
    assert!(!all.text(Stream::Stderr)?.contains("negligible lock"));
    Ok(())
}

#[tokio::test]
async fn subnet_list_orders_emissions_then_native_netuid_and_root_has_no_pool() -> TestResult {
    let mut state = FakeState::try_default()?;
    for (id, emission) in [(0, 5_000_000u64), (1, 2_000_000), (2, 2_000_000)] {
        for (name, value) in [
            ("NetworksAdded", json!(true)),
            ("Tempo", json!(10)),
            ("Burn", json!(1_000_000_000u64)),
            ("SubnetworkN", json!(id + 1)),
            ("MaxAllowedUids", json!(256)),
            ("SubnetTaoInEmission", json!(emission)),
        ] {
            state.seed_python("SubtensorModule", name, &[json!(id)], value)?;
        }
    }
    state.seed_runtime("SwapRuntimeApi","current_alpha_price_all",json!([{"netuid":0,"price":1_000_000_000},{"netuid":1,"price":500_000_000},{"netuid":2,"price":250_000_000}]))?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let parsed = parse(&["rtao", "subnets", "list", "--json"])?;
    let (mut ctx, sink) = context(&parsed)?;
    let request = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?;
    subnets::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut PromptHistory::default(),
    )
    .await?;
    let records: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(
        records
            .as_array()
            .ok_or("records")?
            .iter()
            .map(|v| v["netuid"].clone())
            .collect::<Vec<_>>(),
        vec![json!(0), json!(1), json!(2)]
    );
    assert_eq!(records[0]["price_tao_per_alpha"], Value::Null);
    assert_eq!(records[0]["emission_tao_per_day"], Value::Null);
    assert_eq!(records[1]["price_tao_per_alpha"], json!(0.5));
    assert_eq!(records[1]["emission_tao_per_day"], json!(14.4));
    assert_eq!(records[1]["url"], Value::Null);
    let machine = sink.text(Stream::Stdout)?;
    assert!(sink.text(Stream::Stderr)?.is_empty());
    let mut ordinary = None;
    for quiet in [false, true] {
        for json_mode in [false, true] {
            let mut args = vec!["rtao", "subnets", "list"];
            if quiet {
                args.push("--quiet");
            }
            if json_mode {
                args.push("--json");
            }
            let parsed = parse(&args)?;
            let (mut ctx, sink) = context(&parsed)?;
            let request = Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("group")?.1,
                &mut PromptHistory::default(),
            )?;
            subnets::execute(
                &mut ctx,
                &client,
                &request,
                SubmissionOptions::default(),
                None,
                &mut PromptHistory::default(),
            )
            .await?;
            let text = sink.text(Stream::Stdout)?;
            assert!(sink.text(Stream::Stderr)?.is_empty());
            if json_mode {
                assert_eq!(text, machine);
            } else {
                if quiet {
                    assert_eq!(Some(&text), ordinary.as_ref());
                } else {
                    ordinary = Some(text.clone());
                }
                assert!(text.contains("3 subnets · 6 neurons · sorted by emission"));
                assert!(text.contains("0.500000"));
                assert!(text.contains(records[1]["symbol"].as_str().ok_or("symbol")?));
                assert!(
                    text.contains("spot price of one alpha token in TAO (root has no alpha pool)")
                );
                assert!(text.contains("TAO injected into the subnet per day at the current rate"));
                assert!(text.contains("smoothed net TAO flow from staking"));
                assert!(text.contains(
                    "positive means TAO is entering the subnet, negative means it is leaving"
                ));
                assert!(text.contains("TAO burned to register a neuron on the subnet right now"));
                assert!(text.contains("alpha token symbol is included with its netuid"));
            }
        }
    }
    assert!(
        !chain
            .requests()
            .iter()
            .any(|r| r.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}
