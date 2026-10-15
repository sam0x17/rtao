// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/sudo.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Curated owner commands preserve real SDK arguments and lossless read output.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::NetUid,
};
use rtao_cli::{
    application,
    books::Books,
    commands::hparams::{self, Request},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
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
    Ok(application::command_tree([hparams::command()?]).try_get_matches_from(args)?)
}

fn context(
    parsed: &clap::ArgMatches,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let group = parsed.subcommand().ok_or("hparams")?.1;
    let mut leaf = group;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/hparams-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &hparams::reserved(group)?, |_| None)?;
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

#[test]
fn owner_spellings_select_actual_intents_and_keep_exact_protocol_values() -> TestResult {
    for (name, input, expected) in [
        ("max_difficulty", "18446744073709551615", json!(u64::MAX)),
        ("registration_allowed", "false", json!(0)),
        ("epoch_consensus", "Null", json!("Null")),
    ] {
        let parsed = parse(&[
            "rtao", "su", "set", "--netuid", "1", "--name", name, "--value", input, "--yes",
        ])?;
        let (mut ctx, _) = context(&parsed)?;
        let Request::Transaction(request) = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?
        else {
            return Err("expected transaction".into());
        };
        assert_eq!(request.op, "set_hyperparameter");
        assert_eq!(request.args["value"], json!(input));
        assert_eq!(request.intent()?.to_json()["value"], expected);
    }
    Ok(())
}

#[test]
fn identity_python_flags_map_to_native_fields_and_default_other_fields_to_empty() -> TestResult {
    let parsed = parse(&[
        "rtao",
        "hparams",
        "set_identity",
        "--netuid",
        "2",
        "--name",
        "τ two",
        "--url",
        "https://example.test",
        "--description",
        "example",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Transaction(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected transaction".into());
    };
    let intent = request.intent()?.to_json();
    assert_eq!(intent["op"], "set_subnet_identity");
    assert_eq!(intent["subnet_name"], "τ two");
    assert_eq!(intent["subnet_url"], "https://example.test");
    assert_eq!(intent["description"], "example");
    assert_eq!(intent["github_repo"], "");
    assert_eq!(intent["additional"], "");
    Ok(())
}

#[test]
fn count_without_value_stays_a_read_and_explicit_zero_is_not_omission() -> TestResult {
    let parsed = parse(&[
        "rtao", "hparams", "mech", "count", "--netuid", "1", "--json",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    let Request::Read(query) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("expected read".into());
    };
    assert_eq!(query.name, rtao::reads::ReadName::MechanismCount);
    let parsed = parse(&[
        "rtao",
        "hparams",
        "mechanisms",
        "count",
        "--netuid",
        "1",
        "--value",
        "0",
        "--yes",
    ])?;
    let (mut ctx, _) = context(&parsed)?;
    // Zero is supplied as a transaction, never silently changed into the read
    // branch. The chain retains responsibility for semantic count bounds.
    let Request::Transaction(request) = Request::from_matches(
        &mut ctx,
        parsed.subcommand().ok_or("group")?.1,
        &mut PromptHistory::default(),
    )?
    else {
        return Err("explicit value became a read".into());
    };
    assert_eq!(request.op, "set_mechanism_count");
    assert_eq!(request.args["mechanism_count"], 0);
    Ok(())
}

#[tokio::test]
async fn count_and_emissions_use_the_real_read_dispatch_without_wallet_or_signer() -> TestResult {
    for (command, storage, value, expected) in [
        (
            "count",
            "MechanismCountCurrent",
            json!(3),
            json!({"netuid": 2, "mechanism_count": 3}),
        ),
        (
            "emissions",
            "MechanismEmissionSplit",
            json!([8, 2, 6]),
            json!({"netuid": 2, "split": [8, 2, 6]}),
        ),
    ] {
        let parsed = parse(&[
            "rtao",
            "hparams",
            "mechanisms",
            command,
            "--netuid",
            "2",
            "--json",
        ])?;
        let (mut ctx, sink) = context(&parsed)?;
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("group")?.1,
            &mut PromptHistory::default(),
        )?;
        let mut state = FakeState::try_default()?;
        state.seed_python("SubtensorModule", storage, &[json!(2)], value)?;
        let (client, chain) = client(state).await?;
        let result = hparams::execute(
            &mut ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            None,
            &mut PromptHistory::default(),
        )
        .await?;
        assert!(result.is_none());
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            expected
        );
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn parameter_listing_keeps_runtime_values_and_augments_missing_fixed128_storage() -> TestResult
{
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "SubnetInfoRuntimeApi",
        "get_subnet_hyperparams_v3",
        json!([
            {"name":"rho","value":{"U16":7}},
            {"name":"future_name","value":{"U128":u128::MAX}}
        ]),
    )?;
    let item = rtao_catalog::hyperparams::TABLE
        .storage
        .iter()
        .find(|item| item.parameter == "burn_increase_mult")
        .ok_or("fixed128 storage")?;
    state.seed_python(
        item.container,
        item.name,
        &[json!(1)],
        json!({"bits": (1u128 << 64) + 1}),
    )?;
    let (client, chain) = client(state).await?;
    let params = hparams::fetch_hyperparameters(&client, NetUid(1))
        .await?
        .ok_or("parameters")?;
    assert_eq!(params["rho"], 7);
    assert_eq!(params["burn_increase_mult"], json!((1u128 << 64) + 1));
    assert_eq!(params["future_name"], json!(u128::MAX));
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    hparams::render_hyperparameters(&output, NetUid(1), &Some(params.clone()), None)?;
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        Value::Object(params)
    );
    let state_hashes = chain
        .requests()
        .into_iter()
        .filter_map(|request| match request.method.as_str() {
            "state_getStorage" | "state_queryStorageAt" => request.params.get(1).cloned(),
            "state_call" => request.params.get(2).cloned(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!state_hashes.is_empty());
    assert!(state_hashes.iter().all(|hash| hash == &state_hashes[0]));
    client.close().await;
    Ok(())
}

#[test]
fn single_parameter_machine_record_is_exact_and_missing_names_refuse() -> TestResult {
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let params = Some(serde_json::Map::from_iter([(
        "max_difficulty".into(),
        json!(u64::MAX),
    )]));
    hparams::render_hyperparameters(&output, NetUid(1), &params, Some("max_difficulty"))?;
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(payload["value"], json!(u64::MAX));
    assert_eq!(payload["name"], "max_difficulty");
    assert!(payload["normalized"].is_null());
    assert_eq!(payload["owner_settable"], true);
    assert_eq!(
        hparams::render_hyperparameters(&output, NetUid(1), &params, Some("missing"))
            .err()
            .ok_or("missing refusal")?
            .code,
        2
    );
    assert_eq!(
        hparams::render_hyperparameters(&output, NetUid(1), &None, None)
            .err()
            .ok_or("missing subnet refusal")?
            .code,
        1
    );
    Ok(())
}
