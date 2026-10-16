// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/crowd.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Pinned crowdloan CLI shapes over metadata-backed SCALE state and fake RPCs.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::AccountId,
};
use rtao_cli::{
    application,
    books::Books,
    commands::crowd,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
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
        Err(PromptError::Unavailable)
    }
}

fn tree() -> clap::Command {
    application::command_tree([clap::Command::new("misc").subcommand(crowd::command())])
}

fn parse(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn std::error::Error>> {
    Ok(tree().try_get_matches_from(args)?)
}

fn group(parsed: &clap::ArgMatches) -> Result<&clap::ArgMatches, Box<dyn std::error::Error>> {
    let (name, group) = parsed.subcommand().ok_or("missing root command")?;
    Ok(if matches!(name, "misc" | "utils") {
        group.subcommand().ok_or("missing misc command")?.1
    } else {
        group
    })
}

fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = group(parsed)?;
    crowd::validate_before_connect(group)?;
    let leaf = group.subcommand().ok_or("missing crowd leaf")?.1;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/crowd-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &crowd::reserved(group)?, |_| None)?;
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

fn fixture() -> Result<FakeState, Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    for (id, target, finalized) in [(11u32, Some(HOT), false), (2, None, true)] {
        state.seed_python(
            "Crowdloan",
            "Crowdloans",
            &[json!(id)],
            json!({
                "creator": COLD,
                "deposit": u64::MAX,
                "min_contribution": 0,
                "end": u32::MAX,
                "cap": u64::MAX,
                "funds_account": AccountId::ZERO,
                "raised": 1_000_000_001u64,
                "target_address": target,
                "call": null,
                "finalized": finalized,
                "contributors_count": 3
            }),
        )?;
    }
    for (id, contributor, amount) in [
        (11u32, COLD, 0),
        (11, HOT, 1_000_000_001),
        (11, OTHER, u64::MAX),
        // A different crowdloan must not leak into the prefix-scoped query.
        (12, COLD, 9_999_999_999),
    ] {
        state.seed_python(
            "Crowdloan",
            "Contributions",
            &[json!(id), json!(contributor)],
            json!(amount),
        )?;
    }
    Ok(state)
}

fn assert_read_only(chain: &FakeChain) {
    assert!(chain.requests().iter().all(|request| {
        !request.method.starts_with("author_")
            && request.method != "payment_queryInfo"
            && request.method != "system_accountNextIndex"
    }));
}

#[test]
fn canonical_and_hidden_routes_share_read_flags_and_native_id_boundaries() -> TestResult {
    let tree = tree();
    for prefix in [
        vec!["rtao", "misc", "crowd"],
        vec!["rtao", "utils", "crowd"],
        vec!["rtao", "cr"],
        vec!["rtao", "crowdloan"],
        vec!["rtao", "crowd"],
    ] {
        for (leaf, expected_reserved) in [
            (vec!["list"], vec![]),
            (vec!["info", "0"], vec!["crowdloan_id"]),
            (vec!["contributors", "4294967295"], vec!["crowdloan_id"]),
        ] {
            let mut args = prefix.clone();
            args.extend(leaf.iter().copied());
            args.extend(["--network", "test", "--json", "--wallet", "public-view"]);
            let parsed = tree.clone().try_get_matches_from(args)?;
            let (ctx, _) = context(&parsed)?;
            assert_eq!(ctx.globals.network, "test");
            assert_eq!(ctx.globals.wallet_name, "public-view");
            assert!(ctx.globals.json);
            assert_eq!(crowd::reserved(group(&parsed)?)?, expected_reserved);
            assert_eq!(
                application::command_path(&tree, &parsed),
                ["misc", "crowd", leaf[0]]
            );
        }
    }
    for leaf in ["info", "contributors"] {
        for invalid in ["-1", "4294967296", "1.5", "not-an-id"] {
            assert!(parse(&["rtao", "misc", "crowd", leaf, invalid]).is_err());
        }
        assert!(parse(&["rtao", "misc", "crowd", leaf]).is_err());
    }
    for unavailable in ["--yes", "--keychain-password", "--dry-run", "--ledger"] {
        assert!(parse(&["rtao", "misc", "crowd", "list", unavailable]).is_err());
    }
    let help = tree.clone().render_long_help().to_string();
    assert!(!help.contains("crowdloan"));
    assert!(!help.contains("utils"));
    Ok(())
}

#[tokio::test]
async fn empty_list_and_contributors_retain_the_distinct_python_objects() -> TestResult {
    let (client, chain) = client(FakeState::try_default()?).await?;
    for (args, expected) in [
        (
            vec!["rtao", "misc", "crowd", "list", "--json"],
            json!({"count": 0}),
        ),
        (
            vec!["rtao", "misc", "crowd", "contributors", "0", "--json"],
            json!({"crowdloan_id": 0, "contributors": []}),
        ),
        (
            vec!["rtao", "cr", "contributors", "4294967295", "--json"],
            json!({"crowdloan_id": u32::MAX, "contributors": []}),
        ),
    ] {
        let parsed = parse(&args)?;
        let (ctx, sink) = context(&parsed)?;
        crowd::execute(&ctx, &client, group(&parsed)?).await?;
        assert_eq!(
            serde_json::from_slice::<Value>(&sink.bytes(Stream::Stdout)?)?,
            expected
        );
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn list_keeps_python_field_names_numeric_sort_and_native_maximum_amounts() -> TestResult {
    let (client, chain) = client(fixture()?).await?;
    let parsed = parse(&["rtao", "misc", "crowd", "list", "--json"])?;
    let (ctx, sink) = context(&parsed)?;
    crowd::execute(&ctx, &client, group(&parsed)?).await?;
    // These are records, rather than the human table's raised/cap column names.
    assert_eq!(
        serde_json::from_slice::<Value>(&sink.bytes(Stream::Stdout)?)?,
        json!([
            {"id":2,"creator":COLD,"raised_tao":1.000000001,"cap_tao":18_446_744_073.709_553,"finalized":true},
            {"id":11,"creator":COLD,"raised_tao":1.000000001,"cap_tao":18_446_744_073.709_553,"finalized":false}
        ])
    );
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn info_retains_balance_strings_optional_target_and_every_python_field() -> TestResult {
    let (client, chain) = client(fixture()?).await?;
    for (id, target, finalized) in [("11", json!(HOT), false), ("2", Value::Null, true)] {
        let parsed = parse(&["rtao", "misc", "crowd", "info", id, "--json"])?;
        let (ctx, sink) = context(&parsed)?;
        crowd::execute(&ctx, &client, group(&parsed)?).await?;
        assert_eq!(
            serde_json::from_slice::<Value>(&sink.bytes(Stream::Stdout)?)?,
            json!({
                "creator": COLD,
                "deposit": "τ18,446,744,073.709552765",
                "min_contribution": "τ0.000000000",
                "cap": "τ18,446,744,073.709552765",
                "raised": "τ1.000000001",
                "end": u32::MAX,
                "finalized": finalized,
                "target_address": target,
                "funds_account": AccountId::ZERO
            })
        );
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn contributors_are_prefix_scoped_with_full_records_in_map_order() -> TestResult {
    let (client, chain) = client(fixture()?).await?;
    let parsed = parse(&["rtao", "crowdloan", "contributors", "11", "--json"])?;
    let (ctx, sink) = context(&parsed)?;
    crowd::execute(&ctx, &client, group(&parsed)?).await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&sink.bytes(Stream::Stdout)?)?,
        json!([
            {"contributor": COLD, "amount_tao": 0.0},
            {"contributor": HOT, "amount_tao": 1.000000001},
            {"contributor": OTHER, "amount_tao": 18_446_744_073.709_553}
        ])
    );
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn missing_info_is_status_one_and_rpc_failure_is_never_reported_as_empty() -> TestResult {
    let (client, chain) = client(FakeState::try_default()?).await?;
    let parsed = parse(&["rtao", "misc", "crowd", "info", "4294967295", "--json"])?;
    let (ctx, sink) = context(&parsed)?;
    let error = crowd::execute(&ctx, &client, group(&parsed)?)
        .await
        .err()
        .ok_or("missing crowdloan was accepted")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "crowdloan 4294967295 not found");
    assert!(error.help.is_none());
    assert!(sink.bytes(Stream::Stdout)?.is_empty());

    chain.mutate_state(|state| {
        for method in ["state_getStorage", "state_queryStorageAt"] {
            state.set_handler(method, |_, _| {
                Reply::error(-32602, "crowdloan fixture unavailable", None)
            });
        }
    });
    let parsed = parse(&["rtao", "cr", "info", "11", "--json"])?;
    let (ctx, sink) = context(&parsed)?;
    let error = crowd::execute(&ctx, &client, group(&parsed)?)
        .await
        .err()
        .ok_or("RPC failure was accepted")?;
    assert_eq!(error.code, 1);
    assert!(
        error.message.contains("crowdloan fixture unavailable"),
        "{}",
        error.message
    );
    assert!(!error.message.contains("not found"));
    assert!(error.help.is_none());
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    for args in [
        vec!["rtao", "misc", "crowd", "list", "--json"],
        vec!["rtao", "misc", "crowd", "info", "11", "--json"],
        vec!["rtao", "misc", "crowd", "contributors", "11", "--json"],
    ] {
        let policy = rtao_rpc::policy::classify_rpc_error(&json!({
            "code":-32005,"message":"slow down",
            "data":{"policy":"rate","retry_after":"12"},
        }))
        .ok_or("typed crowd policy")?;
        let expected_message = policy.to_string();
        let start = chain.requests().len();
        chain.queue_transport_error(
            "chain_getBlockHash",
            rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                policy,
            )))),
        );
        let parsed = parse(&args)?;
        let (ctx, sink) = context(&parsed)?;
        let error = crowd::execute(&ctx, &client, group(&parsed)?)
            .await
            .err()
            .ok_or("typed crowd refusal was accepted")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, expected_message);
        assert_eq!(error.help.as_deref(), Some("wait 12 seconds, then retry"));
        assert_eq!(
            chain.requests()[start..]
                .iter()
                .filter(|request| request.method == "chain_getBlockHash")
                .count(),
            1
        );
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn human_views_keep_pinned_titles_columns_and_detail_order() -> TestResult {
    let (client, chain) = client(fixture()?).await?;
    for (args, title, fields) in [
        (
            vec!["rtao", "misc", "crowd", "list"],
            "crowdloans",
            vec!["id", "creator", "raised", "cap", "finalized"],
        ),
        (
            vec!["rtao", "misc", "crowd", "contributors", "11"],
            "contributors",
            vec!["contributor", "amount_tao"],
        ),
        (
            vec!["rtao", "misc", "crowd", "info", "11"],
            "crowdloan 11",
            vec![
                "creator",
                "deposit",
                "min_contribution",
                "cap",
                "raised",
                "end",
                "finalized",
                "target_address",
                "funds_account",
            ],
        ),
    ] {
        let parsed = parse(&args)?;
        let (ctx, sink) = context(&parsed)?;
        crowd::execute(&ctx, &client, group(&parsed)?).await?;
        let text = sink.text(Stream::Stdout)?;
        assert!(text.starts_with(title));
        let mut offset = 0;
        for field in fields {
            offset += text[offset..].find(field).ok_or("missing human field")? + field.len();
        }
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert_read_only(&chain);
    client.close().await;
    Ok(())
}
