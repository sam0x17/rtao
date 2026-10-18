// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/misc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Source-authored misc helper fixtures; endpoint probes use fake transports.

use rtao::{
    client::{Client, ClientConfig, Network},
    signer::BoxFuture,
};
use rtao_cli::{
    application,
    books::Books,
    commands::misc::{self, LatencyBackend, LatencyMeasurement, NetworkLatencyBackend},
    context::{AppContext, ConnectionFactory},
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, fakes::Reply, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

type TestResult = Result<(), Box<dyn Error>>;

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

fn tree() -> clap::Command {
    application::command_tree([misc::command()])
}

struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
}

impl Fixture {
    fn new(args: &[&str]) -> Result<Self, Box<dyn Error>> {
        let matches = tree().try_get_matches_from(args)?;
        let (name, leaf) = matches
            .subcommand()
            .and_then(|(_, group)| group.subcommand())
            .ok_or("missing misc helper")?;
        let mut globals = Globals::resolve(
            &matches,
            &Default::default(),
            Path::new("/fixture/no-user-home"),
            |_| None,
        )?;
        globals.apply_with_env(leaf, &misc::reserved(name), |_| None)?;
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode: ctx.globals.json,
                quiet: ctx.globals.quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        Ok(Self { matches, ctx, sink })
    }

    fn command(&self) -> Result<(&str, &clap::ArgMatches), CliError> {
        self.matches
            .subcommand()
            .and_then(|(_, group)| group.subcommand())
            .ok_or_else(|| CliError::input("missing misc helper"))
    }

    async fn run(&self, backend: &dyn LatencyBackend) -> Result<(), CliError> {
        let (name, leaf) = self.command()?;
        misc::run_with_backend(&self.ctx, name, leaf, backend).await
    }

    fn payload(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_str(&self.sink.text(Stream::Stdout)?)?)
    }
}

#[derive(Default)]
struct FakeProbe {
    results: BTreeMap<String, (Duration, Option<String>)>,
    calls: Mutex<Vec<(String, String)>>,
    barrier: Option<tokio::sync::Barrier>,
}

impl FakeProbe {
    fn with(mut self, network: &str, micros: u64, error: Option<&str>) -> Self {
        self.results.insert(
            network.into(),
            (Duration::from_micros(micros), error.map(str::to_owned)),
        );
        self
    }

    fn calls(&self) -> Result<Vec<(String, String)>, Box<dyn Error>> {
        Ok(self.calls.lock().map_err(|_| "probe lock")?.clone())
    }
}

impl LatencyBackend for FakeProbe {
    fn probe<'a>(&'a self, network: &'a Network) -> BoxFuture<'a, LatencyMeasurement> {
        Box::pin(async move {
            if self
                .calls
                .lock()
                .map(|mut calls| {
                    calls.push((network.label().to_owned(), network.endpoint().to_owned()));
                })
                .is_err()
            {
                return LatencyMeasurement {
                    elapsed: Duration::ZERO,
                    outcome: Err(CliError::failure("probe lock")),
                };
            }
            if let Some(barrier) = &self.barrier {
                barrier.wait().await;
            }
            let Some((elapsed, error)) = self.results.get(network.label()) else {
                return LatencyMeasurement {
                    elapsed: Duration::ZERO,
                    outcome: Err(CliError::failure("unexpected endpoint probe")),
                };
            };
            LatencyMeasurement {
                elapsed: *elapsed,
                outcome: match error {
                    Some(error) => Err(CliError::failure(error)),
                    None => Ok(()),
                },
            }
        })
    }
}

#[tokio::test]
async fn converts_offline_to_python_shaped_json_and_uses_context_symbols() -> TestResult {
    let backend = FakeProbe::default();
    for (argv, expected) in [
        (
            vec!["rtao", "misc", "convert", "1.5", "--json"],
            json!({"amount":1.5,"currency":"TAO","rao":1_500_000_000_u64,"display":"τ1.500000000"}),
        ),
        (
            vec![
                "rtao", "utils", "convert", "2.25", "--netuid", "7", "--json",
            ],
            json!({"amount":2.25,"currency":"alpha (netuid 7)","rao":2_250_000_000_u64,"display":"2.250000000γ"}),
        ),
        (
            vec!["rtao", "misc", "convert", "1e-9", "--netuid", "8", "--json"],
            json!({"amount":1e-9,"currency":"alpha (netuid 8)","rao":1,"display":"0.000000001α₈"}),
        ),
    ] {
        let fixture = Fixture::new(&argv)?;
        fixture.ctx.output.update_token_symbols([(7, "γ".into())])?;
        fixture.run(&backend).await?;
        let actual = fixture.payload()?;
        // Python emits 1e-09 while serde_json's float literal uses 1e-9.
        // The shared comparison keeps the shape and integer/float token kinds.
        rtao::primitives::json::compare(&expected, &actual)?;
        assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    }
    assert!(backend.calls()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn rao_conversion_uses_python_float_coercion_truncation_and_native_limits() -> TestResult {
    let backend = FakeProbe::default();
    let fixture = Fixture::new(&[
        "rtao",
        "misc",
        "convert",
        "1234567890.75",
        "--rao",
        "--json",
    ])?;
    fixture.run(&backend).await?;
    assert_eq!(
        fixture.payload()?,
        json!({"rao":1_234_567_890_u64,"amount":1.23456789,"currency":"TAO","display":"τ1.234567890"})
    );
    let fixture = Fixture::new(&[
        "rtao",
        "misc",
        "convert",
        "9007199254740993",
        "--rao",
        "--json",
    ])?;
    fixture.run(&backend).await?;
    // The Python CLI float argument already rounds this integer to 2^53.
    assert_eq!(fixture.payload()?["rao"], json!(9_007_199_254_740_992_u64));
    assert!(backend.calls()?.is_empty());
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_nonfinite_negative_precision_and_overflow_before_network() -> TestResult {
    let backend = FakeProbe::default();
    for args in [
        vec!["nope"],
        vec!["nan"],
        vec!["inf"],
        vec!["-1"],
        vec!["-0.1", "--rao"],
        vec!["0.0000000005"],
        vec!["18446744073.709553"],
        vec!["18446744073709551615", "--rao"],
    ] {
        let mut argv = vec!["rtao", "misc", "convert"];
        argv.extend(args);
        argv.push("--json");
        let fixture = Fixture::new(&argv)?;
        let (name, leaf) = fixture.command()?;
        assert!(misc::validate_before_connect(name, leaf).is_err());
        let error = fixture
            .run(&backend)
            .await
            .err()
            .ok_or("expected refusal")?;
        assert_eq!(error.code, 2);
        assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    }
    assert!(backend.calls()?.is_empty());
    assert!(
        tree()
            .try_get_matches_from(["rtao", "misc", "convert", "1", "--netuid", "4096"])
            .is_err()
    );
    Ok(())
}

#[test]
fn targets_expand_sorted_presets_keep_duplicates_and_apply_injected_local_endpoint() -> TestResult {
    let fixture = Fixture::new(&["rtao", "misc", "latency", "--networks", ", ,"])?;
    let (_, leaf) = fixture.command()?;
    let targets = misc::targets_with_env(leaf, |key| {
        (key == "BT_CHAIN_ENDPOINT").then(|| "ws://127.0.0.1:12345".into())
    })?;
    assert_eq!(
        targets.iter().map(Network::label).collect::<Vec<_>>(),
        ["archive", "devnet", "finney", "local", "test"]
    );
    assert_eq!(targets[3].endpoint(), "ws://127.0.0.1:12345");
    let fixture = Fixture::new(&[
        "rtao",
        "misc",
        "latency",
        "--networks",
        " test ,test, wss://custom.invalid/rpc ",
    ])?;
    let targets = misc::targets_with_env(fixture.command()?.1, |_| None)?;
    assert_eq!(
        targets.iter().map(Network::label).collect::<Vec<_>>(),
        ["test", "test", "wss://custom.invalid/rpc"]
    );
    Ok(())
}

#[tokio::test]
async fn invalid_later_network_starts_no_earlier_probe_and_emits_no_partial_output() -> TestResult {
    let fixture = Fixture::new(&[
        "rtao",
        "misc",
        "latency",
        "--networks",
        "finney,not-a-network",
        "--json",
    ])?;
    let backend = FakeProbe::default();
    assert!(fixture.run(&backend).await.is_err());
    assert!(backend.calls()?.is_empty());
    assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn probes_concurrently_and_sorts_success_before_failures_by_rounded_latency() -> TestResult {
    let fixture = Fixture::new(&[
        "rtao",
        "misc",
        "latency",
        "--networks",
        "test,finney,archive",
        "--json",
        "--quiet",
    ])?;
    let mut backend = FakeProbe::default()
        .with("test", 12_250, None)
        .with("finney", 1_200, Some("offline"))
        .with("archive", 5_180, None);
    backend.barrier = Some(tokio::sync::Barrier::new(3));
    tokio::time::timeout(Duration::from_secs(1), fixture.run(&backend)).await??;
    assert_eq!(
        fixture.payload()?,
        json!([
            {"network":"archive","endpoint":"wss://archive.chain.opentensor.ai:443","latency_ms":5.2,"ok":true},
            {"network":"test","endpoint":"wss://test.chain.opentensor.ai:443","latency_ms":12.2,"ok":true},
            {"network":"finney","endpoint":"wss://entrypoint-finney.opentensor.ai:443","latency_ms":1.2,"ok":false,"error":"offline"},
        ])
    );
    assert_eq!(backend.calls()?.len(), 3);
    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn latency_retains_duplicates_stable_ties_and_failure_rows_without_failing_command()
-> TestResult {
    let fixture = Fixture::new(&[
        "rtao",
        "utils",
        "latency",
        "--networks",
        "test,finney,test",
        "--json",
    ])?;
    let backend =
        FakeProbe::default()
            .with("test", 10_010, Some(""))
            .with("finney", 10_040, Some("refused"));
    fixture.run(&backend).await?;
    let payload = fixture.payload()?;
    let rows = payload.as_array().ok_or("expected array")?;
    assert_eq!(
        rows.iter()
            .map(|row| row["network"].clone())
            .collect::<Vec<_>>(),
        [json!("test"), json!("finney"), json!("test")]
    );
    assert!(rows.iter().all(|row| row["latency_ms"] == json!(10.0)));
    assert!(rows.iter().all(|row| row["ok"] == json!(false)));
    assert_eq!(rows[0]["error"], json!(""));
    Ok(())
}

#[tokio::test]
async fn latency_human_table_preserves_fields_footer_grouping_and_escapes_error_controls()
-> TestResult {
    let fixture = Fixture::new(&["rtao", "misc", "latency", "--networks", "test,finney"])?;
    let backend = FakeProbe::default().with("test", 1_234_560, None).with(
        "finney",
        2_000,
        Some("bad\u{1b}[31mendpoint\nnext"),
    );
    fixture.run(&backend).await?;
    let text = fixture.sink.text(Stream::Stdout)?;
    assert!(text.contains("latency (websocket connect + one block query)"));
    for column in ["network", "ms", "endpoint", "status"] {
        assert!(text.contains(column));
    }
    assert!(text.contains("1,234.6"));
    assert!(text.contains("fastest test  1,234.6 ms · 1/2 reachable"));
    assert!(text.contains("bad\\x1b[31mendpoint\\nnext"));
    assert!(!text.contains('\u{1b}'));
    Ok(())
}

struct FakeConnector {
    chain: FakeChain,
    response: Option<Reply>,
    retained: Mutex<Option<Client>>,
    before_query: Mutex<usize>,
}

impl ConnectionFactory for FakeConnector {
    fn connect<'a>(&'a self, config: ClientConfig) -> BoxFuture<'a, Result<Client, CliError>> {
        Box::pin(async move {
            assert!(!config.track_heads);
            assert!(config.runtime_cache_dir.is_none());
            assert!(config.config_store.is_none());
            let client = Client::from_rpc(RpcClient::new(self.chain.clone()), config)
                .await
                .map_err(|error| CliError::failure(error.to_string()))?;
            *self
                .retained
                .lock()
                .map_err(|_| CliError::failure("retained client lock"))? = Some(client.clone());
            *self
                .before_query
                .lock()
                .map_err(|_| CliError::failure("request count lock"))? =
                self.chain.requests().len();
            if let Some(response) = self.response.clone() {
                self.chain
                    .mutate_state(|state| state.queue_reply("chain_getHeader", response));
            }
            Ok(client)
        })
    }
}

#[tokio::test]
async fn production_probe_queries_current_header_once_and_closes_on_success_or_failure()
-> TestResult {
    for (response, succeeds) in [
        (None, true),
        (Some(Reply::result(Value::Null)), false),
        (Some(Reply::result(json!({"number":"garbage"}))), false),
        (Some(Reply::error(-32000, "unavailable", None)), false),
    ] {
        let connector = FakeConnector {
            chain: FakeChain::from_state(FakeState::try_default()?),
            response,
            retained: Mutex::new(None),
            before_query: Mutex::new(0),
        };
        let backend = NetworkLatencyBackend {
            connector: &connector,
        };
        let network = Network::resolve_with("local", |_| None)?;
        let measurement = backend.probe(&network).await;
        assert_eq!(measurement.outcome.is_ok(), succeeds);
        let retained = connector.retained.lock().map_err(|_| "retained lock")?;
        assert!(retained.as_ref().ok_or("missing client")?.is_closed());
        let before = *connector.before_query.lock().map_err(|_| "count lock")?;
        let requests = connector.chain.requests();
        let after = requests.get(before..).ok_or("request range")?;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].method, "chain_getHeader");
        assert_eq!(after[0].params, json!([]));
    }
    Ok(())
}

#[derive(Default)]
struct PendingConnector {
    started: AtomicBool,
    cancelled: AtomicBool,
}
struct CancelledOnDrop<'a>(&'a AtomicBool);
impl Drop for CancelledOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl ConnectionFactory for PendingConnector {
    fn connect<'a>(&'a self, _: ClientConfig) -> BoxFuture<'a, Result<Client, CliError>> {
        Box::pin(async move {
            self.started.store(true, Ordering::SeqCst);
            let _cancelled = CancelledOnDrop(&self.cancelled);
            futures::future::pending().await
        })
    }
}

#[tokio::test]
async fn production_probe_connection_timeout_cancels_the_pending_factory_without_waiting()
-> TestResult {
    let connector = PendingConnector::default();
    let backend = NetworkLatencyBackend {
        connector: &connector,
    };
    let network = Network::resolve_with("local", |_| None)?;
    let measurement = backend
        .probe_with_timeouts(&network, Duration::ZERO, Duration::from_secs(30))
        .await;
    assert_eq!(
        measurement
            .outcome
            .err()
            .ok_or("connection timeout")?
            .message,
        "connection deadline expired after 0s"
    );
    assert!(connector.started.load(Ordering::SeqCst));
    assert!(connector.cancelled.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn production_probe_query_timeout_closes_the_open_client_without_waiting() -> TestResult {
    let connector = FakeConnector {
        chain: FakeChain::from_state(FakeState::try_default()?),
        response: Some(Reply::silent()),
        retained: Mutex::new(None),
        before_query: Mutex::new(0),
    };
    let backend = NetworkLatencyBackend {
        connector: &connector,
    };
    let network = Network::resolve_with("local", |_| None)?;
    let measurement = backend
        .probe_with_timeouts(&network, Duration::from_secs(30), Duration::ZERO)
        .await;
    assert_eq!(
        measurement.outcome.err().ok_or("query timeout")?.message,
        "block query deadline expired after 0s"
    );
    assert!(
        connector
            .retained
            .lock()
            .map_err(|_| "retained lock")?
            .as_ref()
            .ok_or("opened client")?
            .is_closed()
    );
    let before = *connector.before_query.lock().map_err(|_| "request count")?;
    let requests = connector.chain.requests();
    let query = requests.get(before..).ok_or("query range")?;
    assert_eq!(query.len(), 1);
    assert_eq!(query[0].method, "chain_getHeader");
    assert_eq!(query[0].params, json!([]));
    assert!(
        connector
            .chain
            .mutate_state(|state| state.submissions.is_empty())
    );
    Ok(())
}
