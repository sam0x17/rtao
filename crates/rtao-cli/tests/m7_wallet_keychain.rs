//! Keychain commands use a fake process runner; no native services are started.

use rtao_cli::{
    application,
    books::Books,
    commands::wallet,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    process::{ProcessCommand, ProcessError, ProcessFuture, ProcessOutput, ProcessRunner},
    prompt::{PromptError, PromptHistory, Terminal},
};
use secrecy::SecretString;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    ffi::OsStr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

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

#[derive(Default)]
struct Processes {
    replies: VecDeque<Result<ProcessOutput, ProcessError>>,
    calls: Vec<ProcessCommand>,
}
#[derive(Clone, Default)]
struct Runner(Arc<Mutex<Processes>>);
impl Runner {
    fn replies(replies: Vec<Result<ProcessOutput, ProcessError>>) -> Self {
        Self(Arc::new(Mutex::new(Processes {
            replies: replies.into(),
            calls: Vec::new(),
        })))
    }
}
impl ProcessRunner for Runner {
    fn which(&self, _: &OsStr) -> Option<PathBuf> {
        None
    }
    fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
        let reply = self
            .0
            .lock()
            .map_err(|_| ProcessError::Io)
            .and_then(|mut state| {
                state.calls.push(command);
                state.replies.pop_front().unwrap_or(Err(ProcessError::Io))
            });
        Box::pin(async move { reply })
    }
}
fn reply(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<ProcessOutput, ProcessError> {
    Ok(ProcessOutput {
        success,
        code: Some(if success { 0 } else { 1 }),
        stdout: Zeroizing::new(stdout.to_vec()),
        stderr: Zeroizing::new(stderr.to_vec()),
    })
}

struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
}
impl Fixture {
    fn new(leaf: &str, name: &str) -> Result<Self, Box<dyn Error>> {
        let matches = application::command_tree([wallet::command()])
            .try_get_matches_from(["rtao", "wallet", "keychain", leaf, "-w", name, "--json"])?;
        let selected = matches
            .subcommand()
            .ok_or("wallet")?
            .1
            .subcommand()
            .ok_or("keychain")?
            .1
            .subcommand()
            .ok_or("leaf")?
            .1;
        // This path is only inspected. The fixture creates no files/directories.
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m7-keychain-missing");
        assert!(!root.exists(), "fixture requires an absent wallet root");
        let mut globals = Globals::resolve(&matches, &Default::default(), &root, |_| None)?;
        globals.apply_with_env(selected, &[], |_| None)?;
        globals.wallet_path = root;
        let sink = MemorySink::default();
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode: true,
                quiet: false,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        Ok(Self { matches, ctx, sink })
    }
    async fn run(&mut self, runner: Runner, macos: bool) -> Result<(), rtao_cli::error::CliError> {
        let group = self
            .matches
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing wallet group"))?
            .1
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing keychain group"))?
            .1;
        wallet::run_keychain_with(
            &mut self.ctx,
            group,
            &mut PromptHistory::default(),
            runner,
            macos,
        )
        .await
    }
    fn payload(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_slice(&self.sink.bytes(Stream::Stdout)?)?)
    }
    fn silent(&self) -> TestResult {
        assert!(self.sink.bytes(Stream::Stdout)?.is_empty());
        assert!(self.sink.bytes(Stream::Stderr)?.is_empty());
        Ok(())
    }
}
fn assert_security_call(call: &ProcessCommand, action: &str, wallet: &str, extra: &[&str]) {
    assert_eq!(call.program, PathBuf::from("/usr/bin/security"));
    let expected = [action, "-s", "bittensor.coldkey", "-a", wallet]
        .into_iter()
        .chain(extra.iter().copied())
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    assert_eq!(call.args, expected);
}

#[test]
fn keychain_routes_require_a_leaf_and_offer_no_password_argument() {
    let command = || application::command_tree([wallet::command()]);
    assert!(
        command()
            .try_get_matches_from(["rtao", "wallet", "keychain"])
            .is_err()
    );
    for leaf in ["save", "show", "delete"] {
        assert!(
            command()
                .try_get_matches_from([
                    "rtao", "wallet", "keychain", leaf, "-w", "absent", "--json"
                ])
                .is_ok()
        );
        assert!(
            command()
                .try_get_matches_from(["rtao", "wallet", "keychain", leaf, "--password", "secret"])
                .is_err()
        );
    }
}

#[tokio::test]
async fn save_uses_native_dialog_and_zeroizing_stdin_without_local_keys_or_secret_output()
-> TestResult {
    let mut fixture = Fixture::new("save", "alice")?;
    let runner = Runner::replies(vec![
        reply(true, b"  keychain-fixture-secret  \n", b""),
        reply(false, b"", b"old entry absent"),
        reply(true, b"ignored secret-looking output", b""),
    ]);
    fixture.run(runner.clone(), true).await?;
    assert_eq!(
        fixture.payload()?,
        json!({"wallet":"alice", "service":"bittensor.coldkey", "account":"alice"})
    );
    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    let state = runner.0.lock().map_err(|_| "fixture lock")?;
    assert!(state.replies.is_empty());
    assert_eq!(state.calls.len(), 3);
    assert_eq!(state.calls[0].program, PathBuf::from("osascript"));
    assert_eq!(
        state.calls[0].args,
        vec![
            std::ffi::OsString::from("-e"),
            std::ffi::OsString::from(
                "set dlg to display dialog \"Enter the password for wallet 'alice'. It will be stored in your macOS Keychain.\" with title \"Save alice coldkey password\" default answer \"\" with hidden answer buttons {\"Cancel\", \"OK\"} default button \"OK\"\nreturn text returned of dlg"
            ),
        ]
    );
    assert_security_call(&state.calls[1], "delete-generic-password", "alice", &[]);
    assert_security_call(
        &state.calls[2],
        "add-generic-password",
        "alice",
        &["-U", "-w"],
    );
    assert!(state.calls[0].input.is_none());
    assert!(state.calls[1].input.is_none());
    assert_eq!(
        state.calls[2].input.as_deref().map(Vec::as_slice),
        Some(b"keychain-fixture-secret\nkeychain-fixture-secret\n".as_slice())
    );
    assert!(!format!("{:?}", state.calls).contains("keychain-fixture-secret"));
    Ok(())
}

#[tokio::test]
async fn show_reports_presence_only_and_delete_preserves_existed_boolean() -> TestResult {
    for (success, bytes, stored) in [
        (true, b"opaque-secret\n".as_slice(), true),
        (true, b" \n", false),
        (false, b"not-a-password", false),
    ] {
        let mut fixture = Fixture::new("show", "absent-wallet")?;
        let runner = Runner::replies(vec![reply(success, bytes, b"sensitive diagnostic")]);
        fixture.run(runner.clone(), true).await?;
        assert_eq!(
            fixture.payload()?,
            json!({"wallet":"absent-wallet", "service":"bittensor.coldkey", "stored":stored})
        );
        let state = runner.0.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.calls.len(), 1);
        assert_security_call(
            &state.calls[0],
            "find-generic-password",
            "absent-wallet",
            &["-w"],
        );
        assert!(state.calls[0].input.is_none());
    }
    for existed in [false, true] {
        let mut fixture = Fixture::new("delete", "absent-wallet")?;
        let runner = Runner::replies(vec![reply(existed, b"private output", b"private error")]);
        fixture.run(runner.clone(), true).await?;
        assert_eq!(
            fixture.payload()?,
            json!({"wallet":"absent-wallet", "existed":existed})
        );
        let state = runner.0.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.calls.len(), 1);
        assert_security_call(
            &state.calls[0],
            "delete-generic-password",
            "absent-wallet",
            &[],
        );
    }
    Ok(())
}

#[tokio::test]
async fn non_macos_fails_before_any_process_for_every_leaf() -> TestResult {
    for leaf in ["save", "show", "delete"] {
        let mut fixture = Fixture::new(leaf, "absent-wallet")?;
        let runner = Runner::default();
        let error = fixture
            .run(runner.clone(), false)
            .await
            .err()
            .ok_or("expected macOS requirement")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, "macOS Keychain is only available on darwin");
        assert!(
            runner
                .0
                .lock()
                .map_err(|_| "fixture lock")?
                .calls
                .is_empty()
        );
        fixture.silent()?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn normal_wallet_dispatch_routes_keychain_without_a_chain_connection() -> TestResult {
    for leaf in ["save", "show", "delete"] {
        let mut fixture = Fixture::new(leaf, "absent-wallet")?;
        let group = fixture.matches.subcommand().ok_or("wallet")?.1;
        let error = wallet::run(&mut fixture.ctx, group, &mut PromptHistory::default())
            .await
            .err()
            .ok_or("expected native Keychain unavailable")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, "macOS Keychain is only available on darwin");
        fixture.silent()?;
    }
    Ok(())
}

#[tokio::test]
async fn saved_multisig_is_rejected_before_platform_or_native_operations() -> TestResult {
    for leaf in ["save", "show", "delete"] {
        let mut fixture = Fixture::new(leaf, "collective")?;
        fixture
            .ctx
            .books
            .multisigs
            .push(json!({"name":"collective", "threshold":1, "signatories":["member"]}));
        let runner = Runner::default();
        let error = fixture
            .run(runner.clone(), true)
            .await
            .err()
            .ok_or("expected multisig rejection")?;
        assert_eq!(error.code, 2);
        assert!(error.message.contains("multisig"));
        assert!(
            runner
                .0
                .lock()
                .map_err(|_| "fixture lock")?
                .calls
                .is_empty()
        );
        fixture.silent()?;
    }
    Ok(())
}

#[tokio::test]
async fn save_failures_are_fixed_diagnostics_and_never_emit_success_or_secrets() -> TestResult {
    let cases = [
        (
            vec![reply(
                false,
                b"private-result",
                b"User canceled -128 private-result",
            )],
            "password entry cancelled",
            1,
        ),
        (vec![reply(true, b" \n", b"")], "empty password entered", 1),
        (
            vec![reply(true, b"bad\npassword", b"")],
            "password may not contain newline characters",
            1,
        ),
        (
            vec![
                reply(true, b"private-result", b""),
                reply(true, b"", b""),
                reply(false, b"private-result", b"private-result"),
            ],
            "could not save password to Keychain",
            3,
        ),
        (
            vec![Err(ProcessError::Spawn)],
            "macOS password prompt failed",
            1,
        ),
    ];
    for (replies, message, calls) in cases {
        let mut fixture = Fixture::new("save", "alice")?;
        let runner = Runner::replies(replies);
        let error = fixture
            .run(runner.clone(), true)
            .await
            .err()
            .ok_or("expected save failure")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, message);
        let state = runner.0.lock().map_err(|_| "fixture lock")?;
        assert_eq!(state.calls.len(), calls);
        assert!(state.replies.is_empty());
        fixture.silent()?;
    }
    Ok(())
}

#[tokio::test]
async fn show_and_delete_propagate_process_failure_without_captured_diagnostics() -> TestResult {
    for leaf in ["show", "delete"] {
        let mut fixture = Fixture::new(leaf, "alice")?;
        let runner = Runner::replies(vec![Err(ProcessError::Io)]);
        let error = fixture
            .run(runner, true)
            .await
            .err()
            .ok_or("expected process I/O failure")?;
        assert_eq!(error.message, "could not access macOS Keychain");
        fixture.silent()?;
    }
    let mut fixture = Fixture::new("show", "alice")?;
    let error = fixture
        .run(Runner::replies(vec![reply(true, &[0xff], b"")]), true)
        .await
        .err()
        .ok_or("expected non UTF-8 secret failure")?;
    assert_eq!(error.message, "password subprocess returned invalid UTF-8");
    fixture.silent()?;
    Ok(())
}
