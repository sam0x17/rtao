//! Extension CLI fixtures inject lifecycle operations; no daemons, browser, files or sockets are started.

#![cfg(feature = "extension")]

use rtao::signer::extension::{BridgeOptions, BridgePaths, EnsureBridgeOptions, ExtensionAccount};
use rtao_cli::{
    application,
    books::Books,
    commands::extension::{self, ExtensionBackend, ExtensionFuture},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use secrecy::SecretString;
use serde_json::{Value, json};
use std::{
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
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
    fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn paths() -> BridgePaths {
    BridgePaths {
        token_file: PathBuf::from("/fixture/private/bridge.token"),
        pid_file: PathBuf::from("/fixture/private/bridge.pid"),
    }
}
fn tree() -> clap::Command {
    application::command_tree([clap::Command::new("misc").subcommand(extension::command())])
}
struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
}
impl Fixture {
    fn new(args: &[&str], json_mode: bool, quiet: bool) -> Result<Self, Box<dyn Error>> {
        let matches = tree().try_get_matches_from(args)?;
        let mut leaf = &matches;
        while let Some((_, child)) = leaf.subcommand() {
            leaf = child;
        }
        let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut globals = Globals::resolve(&matches, &Default::default(), &home, |_| None)?;
        globals.apply_with_env(leaf, &[], |_| None)?;
        globals.json = json_mode;
        globals.quiet = quiet;
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode,
                quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        Ok(Self { matches, ctx, sink })
    }
    async fn run(&self, backend: &Fake) -> Result<(), rtao_cli::error::CliError> {
        let (_, group) = self
            .matches
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing group"))?;
        let group = if group.subcommand_name() == Some("extension") {
            group
                .subcommand()
                .ok_or_else(|| rtao_cli::error::CliError::input("missing extension"))?
                .1
        } else {
            group
        };
        extension::run_with_backend(&self.ctx, group, paths(), backend).await
    }
}
#[derive(Default)]
struct State {
    serve: Vec<(BridgeOptions, BridgePaths, bool)>,
    stop: Vec<(BridgePaths, String)>,
    accounts: Vec<EnsureBridgeOptions>,
}
#[derive(Default)]
struct Fake {
    state: Mutex<State>,
    stopped: bool,
    rows: Vec<ExtensionAccount>,
    fail: bool,
}
impl ExtensionBackend for Fake {
    fn serve<'a>(
        &'a self,
        options: BridgeOptions,
        paths: BridgePaths,
        daemon: bool,
        _: &'a Output,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture lock"))?
                .serve
                .push((options, paths, daemon));
            Ok(())
        })
    }
    fn stop<'a>(&'a self, paths: BridgePaths, url: String) -> ExtensionFuture<'a, bool> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture lock"))?
                .stop
                .push((paths, url));
            Ok(self.stopped)
        })
    }
    fn accounts<'a>(
        &'a self,
        options: EnsureBridgeOptions,
    ) -> ExtensionFuture<'a, Vec<ExtensionAccount>> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture lock"))?
                .accounts
                .push(options);
            if self.fail {
                return Err(rtao_cli::error::CliError::failure(
                    "extension fixture failure",
                ));
            }
            Ok(self.rows.clone())
        })
    }
}

#[test]
fn all_three_routes_support_misc_and_v9_aliases_and_hide_daemon_controls() -> TestResult {
    for prefix in [
        vec!["rtao", "misc", "extension"],
        vec!["rtao", "utils", "extension"],
        vec!["rtao", "extension"],
    ] {
        for leaf in ["bridge", "stop", "accounts"] {
            let mut argv = prefix.clone();
            argv.push(leaf);
            let _ = tree().try_get_matches_from(argv)?;
        }
    }
    assert!(
        tree()
            .try_get_matches_from(["rtao", "extension", "bridge", "--pid-file", "/fixture/pid"])
            .is_err()
    );
    let mut command = extension::command();
    let bridge = command.find_subcommand_mut("bridge").ok_or("bridge")?;
    let help = bridge.render_long_help().to_string();
    assert!(!help.contains("--daemon") && !help.contains("--pid-file"));
    Ok(())
}

#[tokio::test]
async fn foreground_and_daemon_forward_only_explicit_lifecycle_authority() -> TestResult {
    let backend = Fake::default();
    Fixture::new(
        &[
            "rtao",
            "extension",
            "bridge",
            "--host",
            "::1",
            "--port",
            "12345",
            "--no-open",
        ],
        false,
        false,
    )?
    .run(&backend)
    .await?;
    Fixture::new(
        &[
            "rtao",
            "extension",
            "bridge",
            "--daemon",
            "--pid-file",
            "/fixture/override.pid",
        ],
        false,
        false,
    )?
    .run(&backend)
    .await?;
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.serve.len(), 2);
    assert_eq!(state.serve[0].0.address.to_string(), "[::1]:12345");
    assert!(!state.serve[0].0.open_browser && !state.serve[0].2);
    assert_eq!(state.serve[0].1, paths());
    assert!(state.serve[1].2);
    assert!(!state.serve[1].0.open_browser);
    assert_eq!(
        state.serve[1].1.pid_file,
        PathBuf::from("/fixture/override.pid")
    );
    assert_eq!(state.serve[1].1.token_file, paths().token_file);
    Ok(())
}

#[tokio::test]
async fn nonloopback_is_rejected_before_lifecycle_or_account_calls() -> TestResult {
    let backend = Fake::default();
    for leaf in ["bridge", "accounts"] {
        let fixture = Fixture::new(
            &["rtao", "extension", leaf, "--host", "0.0.0.0"],
            false,
            false,
        )?;
        assert_eq!(
            fixture
                .run(&backend)
                .await
                .err()
                .ok_or("unsafe host accepted")?
                .code,
            2
        );
    }
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert!(state.serve.is_empty() && state.accounts.is_empty());
    Ok(())
}

#[tokio::test]
async fn explicit_account_bridge_url_ignores_unused_listener_host_and_port() -> TestResult {
    let backend = Fake::default();
    let fixture = Fixture::new(
        &[
            "rtao",
            "extension",
            "accounts",
            "--extension-bridge",
            "ws://127.0.0.1:40000/ws",
            "--host",
            "ignored.example",
            "--port",
            "70000",
        ],
        true,
        false,
    )?;
    fixture.run(&backend).await?;
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.accounts.len(), 1);
    assert_eq!(state.accounts[0].url, "ws://127.0.0.1:40000/ws");
    assert!(state.serve.is_empty());
    drop(state);

    let fixture = Fixture::new(
        &[
            "rtao",
            "extension",
            "accounts",
            "--extension-bridge",
            "",
            "--host",
            "::1",
            "--port",
            "12345",
        ],
        true,
        false,
    )?;
    fixture.run(&backend).await?;
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert_eq!(state.accounts.len(), 2);
    assert_eq!(state.accounts[1].url, "ws://[::1]:12345/ws");
    drop(state);

    for (host, port) in [("ignored.example", "12345"), ("127.0.0.1", "70000")] {
        let fixture = Fixture::new(
            &[
                "rtao",
                "extension",
                "accounts",
                "--host",
                host,
                "--port",
                port,
            ],
            true,
            false,
        )?;
        assert_eq!(
            fixture
                .run(&backend)
                .await
                .err()
                .ok_or("invalid fallback accepted")?
                .code,
            2
        );
    }
    assert_eq!(
        backend
            .state
            .lock()
            .map_err(|_| "fixture lock")?
            .accounts
            .len(),
        2
    );
    assert!(
        tree()
            .try_get_matches_from([
                "rtao",
                "extension",
                "accounts",
                "--extension-bridge",
                "ws://127.0.0.1:40000/ws",
                "--port",
                "not-an-integer"
            ])
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn accounts_json_preserves_python_rows_and_explicit_url_browser_and_quiet_choices()
-> TestResult {
    let rows = vec![ExtensionAccount {
        name: "fixture\u{1b}[31m".into(),
        address: "extension-address".into(),
        source: "polkadot-js".into(),
        account_type: "sr25519".into(),
    }];
    let backend = Fake {
        rows: rows.clone(),
        ..Default::default()
    };
    let fixture = Fixture::new(
        &[
            "rtao",
            "extension",
            "accounts",
            "--extension-bridge",
            "ws://127.0.0.1:40000/ws",
            "--extension-browser",
            "fixture-browser",
        ],
        true,
        true,
    )?;
    fixture.run(&backend).await?;
    let result: Value = serde_json::from_slice(&fixture.sink.bytes(Stream::Stdout)?)?;
    assert_eq!(
        result,
        json!([{"name":rows[0].name,"address":"extension-address","source":"polkadot-js","type":"sr25519"}])
    );
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    let options = state.accounts.first().ok_or("accounts call")?;
    assert_eq!(options.url, "ws://127.0.0.1:40000/ws");
    assert_eq!(options.browser.as_deref(), Some("fixture-browser"));
    assert!(options.allow_spawn && options.fresh);
    assert!(!options.open_browser);
    assert_eq!(options.paths, paths());
    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn account_human_output_escapes_extension_controls_and_errors_emit_no_rows() -> TestResult {
    let backend = Fake {
        rows: vec![ExtensionAccount {
            name: "name\u{1b}[31m\u{202e}".into(),
            address: "address".into(),
            source: "source".into(),
            account_type: "ed25519".into(),
        }],
        ..Default::default()
    };
    let fixture = Fixture::new(&["rtao", "extension", "accounts"], false, false)?;
    fixture.run(&backend).await?;
    let printed = String::from_utf8(fixture.sink.bytes(Stream::Stdout)?)?;
    assert!(!printed.contains('\u{1b}') && !printed.contains('\u{202e}'));
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert!(state.accounts[0].open_browser);
    drop(state);
    let fixture = Fixture::new(&["rtao", "extension", "accounts"], true, false)?;
    assert!(
        fixture
            .run(&Fake {
                fail: true,
                ..Default::default()
            })
            .await
            .is_err()
    );
    assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn stop_keeps_python_messages_and_forwards_identity_paths() -> TestResult {
    for stopped in [true, false] {
        let backend = Fake {
            stopped,
            ..Default::default()
        };
        let fixture = Fixture::new(&["rtao", "extension", "stop"], false, false)?;
        fixture.run(&backend).await?;
        let stderr = String::from_utf8(fixture.sink.bytes(Stream::Stderr)?)?;
        assert!(stderr.contains(if stopped {
            "extension bridge stopped."
        } else {
            "extension bridge is not running."
        }));
        assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
        let state = backend.state.lock().map_err(|_| "fixture lock")?;
        assert_eq!(
            state.stop,
            vec![(paths(), "ws://127.0.0.1:39295/ws".into())]
        );
    }
    Ok(())
}
