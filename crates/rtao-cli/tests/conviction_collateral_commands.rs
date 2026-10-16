// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/lock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/collateral.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Lock/collateral argument semantics without nodes or wallet unlocking.
use rtao_cli::{
    application,
    books::Books,
    commands::{collateral, conviction},
    context::AppContext,
    globals::Globals,
    prompt::{PromptError, PromptHistory, Terminal},
};
use serde_json::{Map, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;
struct NoTerminal;
impl Terminal for NoTerminal {
    fn stdin_is_tty(&self) -> bool {
        false
    }
    fn stderr_is_tty(&self) -> bool {
        false
    }
    fn read_line(&mut self, _: &str) -> std::result::Result<String, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn read_secret(
        &mut self,
        _: &str,
    ) -> std::result::Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> std::result::Result<(), PromptError> {
        Ok(())
    }
}
fn tree() -> std::result::Result<clap::Command, rtao_cli::error::CliError> {
    Ok(application::command_tree([
        conviction::command()?,
        collateral::command()?,
    ]))
}
fn context(
    parsed: &clap::ArgMatches,
) -> std::result::Result<AppContext, Box<dyn std::error::Error>> {
    let (family, group) = parsed.subcommand().ok_or("family")?;
    let leaf = group.subcommand().ok_or("leaf")?.1;
    let config = Map::new();
    let mut globals =
        Globals::resolve(parsed, &config, Path::new("/isolated/no-wallet"), |_| None)?;
    let reserved = if family == "conviction" {
        conviction::reserved(group)?
    } else {
        collateral::reserved(group)?
    };
    globals.apply_with_env(leaf, &reserved, |_| None)?;
    Ok(AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    ))
}
#[test]
fn mode_and_accept_require_one_unambiguous_choice() -> Result {
    for args in [
        vec!["rtao", "conviction", "mode", "--netuid", "3"],
        vec![
            "rtao",
            "conviction",
            "mode",
            "--netuid",
            "3",
            "--perpetual",
            "--decaying",
        ],
        vec!["rtao", "conviction", "accept"],
        vec!["rtao", "conviction", "accept", "--allow", "--reject"],
    ] {
        assert!(tree()?.try_get_matches_from(args).is_err());
    }
    for (name, flag, enabled) in [
        ("mode", "--perpetual", true),
        ("mode", "--decaying", false),
        ("accept", "--allow", false),
        ("accept", "--reject", true),
    ] {
        let mut args = vec!["rtao", "conviction", name, flag];
        if name == "mode" {
            args.extend(["--netuid", "3"])
        }
        let parsed = tree()?.try_get_matches_from(args)?;
        let mut ctx = context(&parsed)?;
        let tx = conviction::tx_requests(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(tx.len(), 1);
        assert_eq!(
            tx[0].op,
            if name == "mode" {
                "set_perpetual_lock"
            } else {
                "set_reject_locked_alpha"
            }
        );
        assert_eq!(tx[0].args["enabled"], json!(enabled));
    }
    Ok(())
}
#[test]
fn perpetual_add_prepares_mode_before_lock_and_preserves_exact_amount() -> Result {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "conviction",
        "add",
        "--netuid",
        "3",
        "--amount",
        "0.000000001",
        "--perpetual",
        "--period",
        "64",
        "--hotkey",
        &rtao::primitives::AccountId([1; 32]).to_string(),
    ])?;
    let mut ctx = context(&parsed)?;
    let tx = conviction::tx_requests(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?;
    assert_eq!(tx.len(), 2);
    assert_eq!(tx[0].op, "set_perpetual_lock");
    assert_eq!(tx[1].op, "lock_stake");
    assert_eq!(
        tx[0].args,
        json!({"netuid":3,"enabled":true})
            .as_object()
            .ok_or("args")?
            .clone()
    );
    assert_eq!(tx[1].args["amount_alpha"], json!("0.000000001"));
    assert!(tx.iter().all(|t| t.period == Some(64)));
    Ok(())
}
#[test]
fn collateral_all_and_floor_alias_use_semantic_intents() -> Result {
    for (args, op, field, value) in [
        (
            vec!["rtao", "collateral", "add", "--netuid", "3", "--all"],
            "add_collateral",
            "amount_alpha",
            "all",
        ),
        (
            vec![
                "rtao",
                "collateral",
                "set-min",
                "--netuid",
                "3",
                "--amount",
                "0",
            ],
            "set_min_collateral",
            "min_alpha",
            "0",
        ),
    ] {
        let mut args = args;
        let hotkey = rtao::primitives::AccountId([1; 32]).to_string();
        args.extend(["--hotkey", hotkey.as_str()]);
        let parsed = tree()?.try_get_matches_from(args)?;
        let mut ctx = context(&parsed)?;
        let tx = collateral::tx_requests(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(tx.len(), 1);
        assert_eq!(tx[0].op, op);
        assert_eq!(tx[0].args[field], json!(value));
    }
    Ok(())
}
#[test]
fn reads_have_required_subnet_and_optional_coldkey() -> Result {
    let parsed = tree()?.try_get_matches_from([
        "rtao",
        "collateral",
        "show",
        "--netuid",
        "3",
        "--hotkey",
        "named",
    ])?;
    let leaf = parsed
        .subcommand()
        .ok_or("group")?
        .1
        .subcommand()
        .ok_or("leaf")?
        .1;
    assert!(leaf.get_one::<String>("coldkey_ss58").is_none());
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "collateral", "list"])
            .is_err()
    );
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "conviction", "targeting"])
            .is_err()
    );
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "conviction", "list", "--dust"])
            .is_ok()
    );
    Ok(())
}

#[test]
fn collateral_required_options_and_owner_default_help_match_curated_commands() -> Result {
    for args in [
        vec!["rtao", "collateral", "add", "--amount", "1"],
        vec!["rtao", "collateral", "set-min", "--netuid", "3"],
        vec!["rtao", "collateral", "set-min", "--amount", "0"],
    ] {
        let error = tree()?
            .try_get_matches_from(args)
            .err()
            .ok_or("required option")?;
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert_eq!(error.exit_code(), 2);
    }
    // Only add's amount remains eligible for the ordinary interactive prompt.
    assert!(
        tree()?
            .try_get_matches_from(["rtao", "collateral", "add", "--netuid", "3"])
            .is_ok()
    );
    let mut group = collateral::command()?;
    assert!(group.render_long_help().to_string().contains(&format!(
        "{}/guides/mining/collateral",
        rtao_catalog::py::DOCS_URL
    )));
    let show = group.find_subcommand("show").ok_or("show command")?;
    let coldkey_help = show
        .get_arguments()
        .find(|arg| arg.get_id() == "coldkey_ss58")
        .and_then(clap::Arg::get_help)
        .ok_or("coldkey help")?
        .to_string();
    assert!(coldkey_help.contains("Defaults to the hotkey owner"));
    assert!(!coldkey_help.contains("Defaults to your wallet's coldkey"));
    Ok(())
}

async fn fake_client()
-> std::result::Result<(rtao::client::Client, rtao_testkit::FakeChain), Box<dyn std::error::Error>>
{
    use rtao::{
        client::{Client, ClientConfig, Network},
        primitives::AccountId,
    };
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    let cold = AccountId([7; 32]);
    let hot = AccountId([8; 32]);
    let other = AccountId([9; 32]);
    let mut state = FakeState::try_default()?;
    for (item, keys, value) in [
        ("TokenSymbol", vec![json!(1)], json!("Ω")),
        ("AccountFlags", vec![json!(cold)], json!(1)),
        ("Owner", vec![json!(hot)], json!(cold)),
        (
            "IdentitiesV2",
            vec![json!(cold)],
            json!({"name":"chain owner","url":""}),
        ),
        (
            "Lock",
            vec![json!(cold), json!(1), json!(hot)],
            json!({"locked_mass":100,"conviction":{"bits":0},"last_update":0}),
        ),
        (
            "LockingColdkeys",
            vec![json!(1), json!(hot), json!(cold)],
            json!([]),
        ),
        ("DecayingLock", vec![json!(cold), json!(1)], json!(false)),
        (
            "MinerCollateral",
            vec![json!(1), json!(hot), json!(cold)],
            json!({"locked":300,"min_locked":100,"earned":17,"drain_ratio":{"bits":0}}),
        ),
        (
            "MinerCollateral",
            vec![json!(1), json!(hot), json!(other)],
            json!({"locked":150,"min_locked":200,"earned":0,"drain_ratio":{"bits":0}}),
        ),
        ("CollateralLockShare", vec![json!(1)], json!(0)),
        ("CollateralDrainRatio", vec![json!(1)], json!({"bits":0})),
    ] {
        state.seed_python("SubtensorModule", item, &keys, value)?;
    }
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_coldkey_lock",
        json!({"locked_mass":100,"conviction":{"bits":0},"last_update":0}),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_hotkey_conviction",
        json!({"bits":42u128<<64}),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([{"netuid":1,"price":1_000_000_000}]),
    )?;
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
fn captured(ctx: &mut AppContext, json_mode: bool) -> rtao_cli::output::MemorySink {
    use rtao_cli::output::{MemorySink, Output, OutputOptions};
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode,
            quiet: ctx.globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    sink
}
#[tokio::test]
async fn public_reads_preserve_records_owner_default_and_never_submit() -> Result {
    use rtao::primitives::AccountId;
    use rtao_cli::output::Stream;
    let (client, chain) = fake_client().await?;
    let cold = AccountId([7; 32]).to_string();
    let hot = AccountId([8; 32]).to_string();
    let other = AccountId([9; 32]).to_string();
    for (family, command, extra) in [
        ("conviction", "list", vec!["--coldkey", cold.as_str()]),
        ("conviction", "show", vec!["--coldkey", cold.as_str()]),
        ("conviction", "targeting", vec!["--hotkey", hot.as_str()]),
        ("collateral", "show", vec!["--hotkey", hot.as_str()]),
        (
            "collateral",
            "show",
            vec!["--hotkey", hot.as_str(), "--coldkey", other.as_str()],
        ),
        ("collateral", "list", vec![]),
    ] {
        let mut args = vec!["rtao", family, command, "--netuid", "1"];
        args.extend(extra);
        let parsed = tree()?.try_get_matches_from(args)?;
        let group = parsed.subcommand().ok_or("group")?.1;
        let mut ctx = context(&parsed)?;
        if family == "conviction" && command == "targeting" {
            // Python ignores empty contact labels; they must not suppress
            // the coldkey's published identity in machine records.
            ctx.books.addresses = vec![json!({"address":cold,"name":""})];
        }
        let sink = captured(&mut ctx, true);
        if family == "conviction" {
            let request = conviction::prepare_read(&mut ctx, group, &mut PromptHistory::default())?;
            conviction::execute_prepared_read(&ctx, &client, &request).await?;
        } else {
            let request = collateral::prepare_read(&mut ctx, group, &mut PromptHistory::default())?;
            collateral::execute_prepared_read(&ctx, &client, &request).await?;
        }
        let payload: serde_json::Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
        if family == "conviction" && command == "list" {
            assert_eq!(
                payload[0]["locked_amount"].as_f64().map(f64::to_bits),
                Some(0.0000001_f64.to_bits())
            );
            assert_eq!(payload[0]["locked_amount"].to_string(), "1e-07");
            assert_eq!(payload[0]["hotkey"], json!(hot));
        }
        if family == "conviction" && command == "targeting" {
            assert_eq!(payload[0]["name"], json!("chain owner"));
        }
        if family == "conviction" && command == "show" {
            assert!(payload["lock"].is_object());
            assert!(payload["conviction"].is_object());
        }
        if family == "collateral" && command == "show" {
            let explicit = group
                .subcommand()
                .ok_or("leaf")?
                .1
                .get_one::<String>("coldkey_ss58")
                .is_some();
            assert_eq!(
                payload["collateral"]["coldkey"],
                json!(if explicit {
                    other.as_str()
                } else {
                    cold.as_str()
                })
            );
            assert!(payload["policy"].is_object());
        }
        // Exercise the real read handlers, including their ReadError wrapper,
        // with a typed refusal at the snapshot boundary before any data output.
        for (retry, expected) in [
            (Some("12"), "wait 12 seconds, then retry"),
            (None, "reduce the request or connection rate, then retry"),
        ] {
            let (refused_client, refused_chain) = fake_client().await?;
            let policy = rtao_rpc::policy::classify_rpc_error(&json!({
                "code":-32005,"message":"slow down",
                "data":{"policy":"rate","retry_after":retry},
            }))
            .ok_or("typed policy fixture")?;
            let expected_message = policy.to_string();
            let start = refused_chain.requests().len();
            refused_chain.queue_transport_error(
                "chain_getBlockHash",
                rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                    policy,
                )))),
            );
            let mut refused_ctx = context(&parsed)?;
            let sink = captured(&mut refused_ctx, true);
            let outcome = if family == "conviction" {
                let request = conviction::prepare_read(
                    &mut refused_ctx,
                    group,
                    &mut PromptHistory::default(),
                )?;
                conviction::execute_prepared_read(&refused_ctx, &refused_client, &request).await
            } else {
                let request = collateral::prepare_read(
                    &mut refused_ctx,
                    group,
                    &mut PromptHistory::default(),
                )?;
                collateral::execute_prepared_read(&refused_ctx, &refused_client, &request).await
            };
            let error = outcome.err().ok_or("policy refusal was accepted")?;
            assert_eq!(error.code, 1);
            assert_eq!(error.message, expected_message);
            assert_eq!(error.help.as_deref(), Some(expected), "{family} {command}");
            let requests = refused_chain.requests();
            assert_eq!(
                requests[start..]
                    .iter()
                    .filter(|request| request.method == "chain_getBlockHash")
                    .count(),
                1
            );
            assert!(
                requests[start..]
                    .iter()
                    .all(|request| !request.method.starts_with("author_"))
            );
            assert!(refused_chain.mutate_state(|state| state.submissions.is_empty()));
            assert!(sink.bytes(Stream::Stdout)?.is_empty());
            assert!(sink.bytes(Stream::Stderr)?.is_empty());
            refused_client.close().await;
        }
    }
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn targeting_keeps_truthy_contact_json_types_and_ignores_falsy_labels() -> Result {
    use rtao::primitives::AccountId;
    use rtao_cli::output::Stream;
    let (client, chain) = fake_client().await?;
    let cold = AccountId([7; 32]).to_string();
    let hot = AccountId([8; 32]).to_string();
    for (name, expected, retained) in [
        (json!(7), json!(7), true),
        (json!(true), json!(true), true),
        (json!(["team", 3]), json!(["team", 3]), true),
        (json!({"team":true}), json!({"team":true}), true),
        (json!(0), json!("chain owner"), false),
        (json!(false), json!("chain owner"), false),
        (json!(null), json!("chain owner"), false),
        (json!(""), json!("chain owner"), false),
        (json!([]), json!("chain owner"), false),
        (json!({}), json!("chain owner"), false),
        (json!("—"), json!(null), true),
    ] {
        let parsed = tree()?.try_get_matches_from([
            "rtao",
            "conviction",
            "targeting",
            "--netuid",
            "1",
            "--hotkey",
            hot.as_str(),
        ])?;
        let group = parsed.subcommand().ok_or("group")?.1;
        let mut ctx = context(&parsed)?;
        ctx.books.addresses = vec![
            json!({"address":cold,"name":0}),
            json!({"address":cold,"name":name}),
        ];
        if retained {
            ctx.books
                .addresses
                .push(json!({"address":cold,"name":"later duplicate"}));
        }
        let sink = captured(&mut ctx, true);
        let request = conviction::prepare_read(&mut ctx, group, &mut PromptHistory::default())?;
        conviction::execute_prepared_read(&ctx, &client, &request).await?;
        let payload: serde_json::Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
        assert_eq!(payload.as_array().ok_or("targeting records")?.len(), 1);
        assert_eq!(payload[0]["name"], expected, "label {name}");
        assert_eq!(payload[0]["coldkey"], cold);
        assert_eq!(
            payload[0]["locked_amount"].as_f64().map(f64::to_bits),
            Some(0.0000001_f64.to_bits())
        );
        assert_eq!(payload[0]["locked_amount"].to_string(), "1e-07");
    }
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

#[tokio::test]
async fn collateral_list_preserves_order_full_json_and_renders_nonempty_and_empty_humans() -> Result
{
    use rtao::primitives::AccountId;
    use rtao_cli::output::Stream;
    let (client, chain) = fake_client().await?;
    let cold = AccountId([7; 32]);
    let hot = AccountId([8; 32]);
    let other = AccountId([9; 32]);
    let retired = AccountId([10; 32]);
    chain.mutate_state(|state| -> Result {
        state.seed_python(
            "SubtensorModule",
            "MinerCollateral",
            &[json!(1), json!(retired), json!(other)],
            json!({"locked":500,"min_locked":400,"earned":23,"drain_ratio":{"bits":2u128<<64}}),
        )?;
        state.seed_python("SubtensorModule", "Uids", &[json!(1), json!(hot)], json!(4))?;
        // FakeState's imported Python defaults return UID 0 for unknown keys.
        // This position explicitly represents a deregistered hotkey.
        let entry = state
            .runtime
            .storage_entry("SubtensorModule", "Uids")
            .ok_or("Uids metadata")?;
        let key = state.runtime.storage_key(
            entry,
            &[
                rtao::core::codec::Value::Uint(1),
                rtao::core::codec::Value::Str(retired.to_string()),
            ],
        )?;
        state.seed_encoded(key, None);
        Ok(())
    })?;
    let expected = json!([
        {"hotkey":retired,"coldkey":other,"netuid":1,"locked_alpha":"0.000000500Ω",
         "min_locked_alpha":"0.000000400Ω","earned_alpha":"0.000000023Ω","drain_ratio":2.0,
         "headroom_alpha":"0.000000100Ω","shortfall_alpha":"0.000000000Ω","releasable_work_alpha":"0.000000050Ω","uid":null},
        {"hotkey":hot,"coldkey":cold,"netuid":1,"locked_alpha":"0.000000300Ω",
         "min_locked_alpha":"0.000000100Ω","earned_alpha":"0.000000017Ω","drain_ratio":0.0,
         "headroom_alpha":"0.000000200Ω","shortfall_alpha":"0.000000000Ω","releasable_work_alpha":"0.000000000Ω","uid":4},
        {"hotkey":hot,"coldkey":other,"netuid":1,"locked_alpha":"0.000000150Ω",
         "min_locked_alpha":"0.000000200Ω","earned_alpha":"0.000000000Ω","drain_ratio":0.0,
         "headroom_alpha":"0.000000000Ω","shortfall_alpha":"0.000000050Ω","releasable_work_alpha":"0.000000000Ω","uid":4},
    ]);
    for (netuid, records) in [("1", expected), ("2", json!([]))] {
        for json_mode in [true, false] {
            let parsed =
                tree()?.try_get_matches_from(["rtao", "collateral", "list", "--netuid", netuid])?;
            let group = parsed.subcommand().ok_or("collateral group")?.1;
            let mut ctx = context(&parsed)?;
            let sink = captured(&mut ctx, json_mode);
            let request = collateral::prepare_read(&mut ctx, group, &mut PromptHistory::default())?;
            collateral::execute_prepared_read(&ctx, &client, &request).await?;
            let output = sink.text(Stream::Stdout)?;
            if json_mode {
                assert_eq!(serde_json::from_str::<serde_json::Value>(&output)?, records);
            } else {
                assert!(output.contains(&format!("collateral on netuid {netuid}")));
                assert!(output.contains("positions"));
                if netuid == "1" {
                    assert!(output.contains("locked_alpha"));
                    assert!(output.contains(&retired.to_string()));
                    let positions = ["0.000000500Ω", "0.000000300Ω", "0.000000150Ω"]
                        .map(|amount| output.find(amount).ok_or("human collateral amount"));
                    let [first, second, third] = positions;
                    let (first, second, third) = (first?, second?, third?);
                    assert!(first < second && second < third);
                } else {
                    assert!(output.contains("none"));
                }
            }
            assert!(sink.text(Stream::Stderr)?.is_empty());
        }
    }
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

#[tokio::test]
async fn empty_human_lock_view_still_reports_acceptance_and_dust() -> Result {
    use rtao_cli::output::Stream;
    let (client, chain) = fake_client().await?;
    let cold = rtao::primitives::AccountId([7; 32]).to_string();
    let mut machine = None;
    for show_dust in [false, true] {
        let mut ordinary = None;
        for quiet in [false, true] {
            for json_mode in [false, true] {
                let mut args = vec!["rtao", "conviction", "list", "--coldkey", cold.as_str()];
                if show_dust {
                    args.push("--dust");
                }
                if quiet {
                    args.push("--quiet");
                }
                if json_mode {
                    args.push("--json");
                }
                let parsed = tree()?.try_get_matches_from(args)?;
                let mut ctx = context(&parsed)?;
                let sink = captured(&mut ctx, json_mode);
                let request = conviction::prepare_read(
                    &mut ctx,
                    parsed.subcommand().ok_or("group")?.1,
                    &mut PromptHistory::default(),
                )?;
                conviction::execute_prepared_read(&ctx, &client, &request).await?;
                let output = sink.text(Stream::Stdout)?;
                let messages = sink.text(Stream::Stderr)?;
                if json_mode {
                    let records: serde_json::Value = serde_json::from_str(&output)?;
                    assert_eq!(records.as_array().ok_or("records")?.len(), 1);
                    assert_eq!(
                        records[0]["hotkey"],
                        json!(rtao::primitives::AccountId([8; 32]))
                    );
                    assert_eq!(
                        records[0]["locked_amount"].as_f64().map(f64::to_bits),
                        Some(0.0000001_f64.to_bits())
                    );
                    if let Some(expected) = &machine {
                        assert_eq!(&output, expected);
                    } else {
                        machine = Some(output);
                    }
                    assert!(messages.is_empty());
                    continue;
                }
                if quiet {
                    assert_eq!(Some(&output), ordinary.as_ref());
                    assert!(messages.is_empty());
                } else {
                    ordinary = Some(output.clone());
                    assert!(messages.contains("incoming locked alpha: accepted"));
                    assert_eq!(messages.contains("1 dust subnet hidden"), !show_dust);
                }
                assert!(!output.contains("incoming locked alpha"));
                assert!(!output.contains("dust subnet hidden"));
                assert!(!messages.contains("total locked value"));
                if show_dust {
                    assert!(!output.contains("  none"));
                    assert!(output.contains("perpetual"));
                    let total =
                        rtao::primitives::DynBalance::from_rao(100, rtao::primitives::NetUid(0));
                    assert!(output.contains(&format!(
                        "total locked value {total} (spot, excl. slippage/fees)"
                    )));
                    assert_eq!(output.matches("total locked value").count(), 1);
                } else {
                    assert!(output.contains("  none"));
                    assert!(!output.contains("total locked value"));
                }
            }
        }
    }
    assert!(chain.mutate_state(|s| s.submissions.is_empty()));
    client.close().await;
    Ok(())
}
