// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generated query prompts and defaults through real requests and fake transport.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::AccountId,
    reads::ReadName,
};
use rtao_cli::{
    application,
    books::Books,
    commands::generated::{self, QueryRequest},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct QueryTerminal {
    interactive: bool,
    answers: VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}

impl Terminal for QueryTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.interactive
    }

    fn stderr_is_tty(&self) -> bool {
        self.interactive
    }

    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(label.into());
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

fn request(
    argv: &[&str],
    interactive: bool,
    answers: &[&str],
    books: Books,
    messages: Arc<Mutex<Vec<String>>>,
    history: &mut PromptHistory,
) -> TestResult<(QueryRequest, MemorySink, Output)> {
    let parsed = application::command_tree([generated::query_command()?])
        .try_get_matches_from(argv.iter().copied())?;
    let (command, leaf) = parsed
        .subcommand()
        .ok_or("query group")?
        .1
        .subcommand()
        .ok_or("query leaf")?;
    let name: ReadName = command.replace('-', "_").parse()?;
    let config = Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/query-contract-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, &generated::read_reserved(name), |_| None)?;
    let terminal = QueryTerminal {
        interactive,
        answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
        messages,
    };
    let sink = MemorySink::default();
    let output = Output::new(
        OutputOptions {
            json_mode: globals.json,
            quiet: globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    let mut ctx = AppContext::new(
        globals,
        config,
        books,
        Arc::new(Mutex::new(Box::new(terminal))),
    );
    ctx.output = output.clone();
    let request = QueryRequest::from_matches(&ctx, name, leaf, history)?;
    Ok((request, sink, output))
}

async fn client(state: FakeState) -> TestResult<(Client, FakeChain)> {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}

#[test]
fn query_prompts_show_input_shapes_retry_bad_numbers_and_record_resolved_answers() -> TestResult {
    let alice = AccountId([51; 32]);
    let bob = AccountId([52; 32]);
    let books = Books::from_values(
        json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
        json!([]),
        json!([]),
    );
    for (argv, answers, field, expected, label, retry) in [
        (
            vec!["rtao", "query", "immunity-period"],
            vec!["3.5", "3"],
            "netuid",
            json!(3),
            "  netuid (integer): ",
            true,
        ),
        (
            vec!["rtao", "query", "quote-stake", "--netuid", "3"],
            vec!["not-a-number", "0.25"],
            "amount_tao",
            json!(0.25),
            "  amount-tao (number): ",
            true,
        ),
        (
            vec!["rtao", "query", "balances"],
            vec![" bob, alice, bob "],
            "coldkey_ss58s",
            json!([bob, alice, bob]),
            "  coldkeys (comma-separated): ",
            false,
        ),
    ] {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let mut history = PromptHistory::default();
        let (request, _, _) = request(
            &argv,
            true,
            &answers,
            books.clone(),
            messages.clone(),
            &mut history,
        )?;
        assert_eq!(request.params[field], expected);
        let messages = messages.lock().map_err(|_| "prompt messages")?;
        assert_eq!(
            messages.iter().filter(|message| *message == label).count(),
            if retry { 2 } else { 1 }
        );
        if retry {
            assert!(messages.iter().any(|message| message.contains("expected")));
        }
        let argv = argv
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        let replay = history.replay_command(&argv);
        assert!(replay.contains(answers.last().ok_or("answer")?.trim()));
        if retry {
            assert!(!replay.contains(answers[0]));
        }
    }
    Ok(())
}

#[test]
fn missing_query_parameters_refuse_without_reading_the_terminal_in_machine_mode() -> TestResult {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let error = request(
        &["rtao", "query", "immunity-period", "--json"],
        true,
        &["3"],
        Books::default(),
        messages.clone(),
        &mut PromptHistory::default(),
    )
    .err()
    .ok_or("missing netuid must refuse")?;
    let error = error
        .downcast_ref::<rtao_cli::error::CliError>()
        .ok_or("CLI input error")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("--netuid"));
    assert!(messages.lock().map_err(|_| "prompt messages")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn query_boolean_default_and_negated_twin_select_the_real_runtime_method() -> TestResult {
    let mut state = FakeState::try_default()?;
    for method in ["get_neurons_lite", "get_neurons"] {
        state.seed_runtime("NeuronInfoRuntimeApi", method, json!([]))?;
    }
    let (client, chain) = client(state).await?;
    let result: TestResult = async {
        for (flag, lite, method) in [
            (None, true, "NeuronInfoRuntimeApi_get_neurons_lite"),
            (Some("--no-lite"), false, "NeuronInfoRuntimeApi_get_neurons"),
            (
                Some("--lite"),
                true,
                "NeuronInfoRuntimeApi_get_neurons_lite",
            ),
        ] {
            let mut argv = vec!["rtao", "query", "neurons", "--netuid", "3", "--json"];
            argv.extend(flag);
            let (request, sink, output) = request(
                &argv,
                false,
                &[],
                Books::default(),
                Arc::new(Mutex::new(Vec::new())),
                &mut PromptHistory::default(),
            )?;
            assert_eq!(request.params["lite"], json!(lite));
            let observed = chain.requests().len();
            let payload = generated::execute_query(&client, &request, &output).await?;
            assert_eq!(payload, json!([]));
            assert_eq!(
                serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
                payload
            );
            let calls = chain
                .requests()
                .into_iter()
                .skip(observed)
                .filter(|request| request.method == "state_call")
                .map(|request| request.params[0].clone())
                .collect::<Vec<_>>();
            assert_eq!(calls, vec![json!(method)]);
        }
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "fixture state")?
                .submissions
                .is_empty()
        );
        Ok(())
    }
    .await;
    client.close().await;
    result
}

#[tokio::test]
async fn omitted_query_owner_keeps_null_and_dispatches_against_the_hotkey_owner() -> TestResult {
    let hot = AccountId([53; 32]);
    let owner = AccountId([54; 32]);
    let mut state = FakeState::try_default()?;
    state.seed_python("SubtensorModule", "Owner", &[json!(hot)], json!(owner))?;
    state.seed_python(
        "SubtensorModule",
        "MinerCollateral",
        &[json!(3), json!(hot), json!(owner)],
        json!({"locked":300,"min_locked":100,"earned":17,"drain_ratio":{"bits":2u128<<64}}),
    )?;
    let hot = hot.to_string();
    let (request, sink, output) = request(
        &[
            "rtao",
            "query",
            "miner-collateral",
            "--netuid",
            "3",
            "--hotkey",
            &hot,
            "--json",
        ],
        false,
        &[],
        Books::default(),
        Arc::new(Mutex::new(Vec::new())),
        &mut PromptHistory::default(),
    )?;
    assert_eq!(request.params["coldkey_ss58"], Value::Null);
    let (client, chain) = client(state).await?;
    let result: TestResult = async {
        let payload = generated::execute_query(&client, &request, &output).await?;
        assert_eq!(payload["coldkey"], json!(owner));
        assert_eq!(
            serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
            payload
        );
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "fixture state")?
                .submissions
                .is_empty()
        );
        Ok(())
    }
    .await;
    client.close().await;
    result
}

#[tokio::test]
async fn actual_grouped_and_matrix_queries_keep_machine_shape_and_project_human_rows() -> TestResult
{
    let hot = AccountId([55; 32]);
    let mut state = FakeState::try_default()?;
    state.seed_python(
        "SubtensorModule",
        "TimelockedWeightCommits",
        &[json!(1), json!(9)],
        json!([[hot, 90, "0xff0001", 1]]),
    )?;
    state.seed_python(
        "SubtensorModule",
        "TimelockedWeightCommits",
        &[json!(1), json!(2)],
        json!([]),
    )?;
    state.seed_python(
        "SubtensorModule",
        "Bonds",
        &[json!(1), json!(3)],
        json!([[2, 32768]]),
    )?;
    let (client, chain) = client(state).await?;
    let result: TestResult = async {
        for (command, headings) in [
            (
                "timelocked-weight-commits",
                vec!["epoch", "hotkey", "reveals_at"],
            ),
            ("bonds", vec!["validator", "miner", "bond"]),
        ] {
            let (request, sink, output) = request(
                &["rtao", "query", command, "--netuid", "1", "--json"],
                false,
                &[],
                Books::default(),
                Arc::new(Mutex::new(Vec::new())),
                &mut PromptHistory::default(),
            )?;
            assert_eq!(request.params["mechid"], json!(0));
            let payload = generated::execute_query(&client, &request, &output).await?;
            assert_eq!(
                serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
                payload
            );
            if command == "timelocked-weight-commits" {
                assert_eq!(payload["2"], json!([]));
                assert_eq!(payload["9"][0]["hotkey"], json!(hot));
                assert_eq!(
                    payload["9"][0]["reveals_at"],
                    json!("2023-08-23T15:09:27+00:00")
                );
                assert_eq!(payload["9"][0]["ciphertext"], json!("0xff0001"));
                assert!(payload["9"][0].get("epoch").is_none());
            } else {
                assert!(payload["3"]["2"].is_number());
                assert!(payload.get("validator").is_none());
            }
            let human_sink = MemorySink::default();
            let human = Output::new(OutputOptions::default(), Arc::new(human_sink.clone()));
            generated::render_query(&human, request.name, &payload)?;
            let text = human_sink.text(Stream::Stdout)?;
            for heading in headings {
                assert!(text.contains(heading), "missing {heading}: {text}");
            }
            assert!(!text.contains("none"));
        }
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "fixture state")?
                .submissions
                .is_empty()
        );
        Ok(())
    }
    .await;
    client.close().await;
    result
}
