// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Timelock command fixtures inject crypto and beacon replies; no network or real encryption runs.

use rtao::tx::timelock::{OpenOptions, Reveal, TimelockError, Timelocked};
use rtao_cli::{
    application,
    books::Books,
    commands::timelock::{
        self, MAX_CIPHERTEXT_BYTES, MAX_PLAINTEXT_BYTES, TimelockBackend, TimelockFuture,
    },
    context::AppContext,
    globals::Globals,
    output::{DiagnosticContext, MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use secrecy::SecretString;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

type TestResult = Result<(), Box<dyn Error>>;

struct TerminalFixture {
    tty: bool,
    answers: VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}
impl Terminal for TerminalFixture {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.answers.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
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
struct Fixture {
    matches: clap::ArgMatches,
    ctx: AppContext,
    sink: MemorySink,
    history: PromptHistory,
    prompt_messages: Arc<Mutex<Vec<String>>>,
}
impl Fixture {
    fn new(args: &[&str], json_mode: bool, answers: &[&str]) -> Result<Self, Box<dyn Error>> {
        let matches = tree().try_get_matches_from(args)?;
        let mut leaf = &matches;
        while let Some((_, child)) = leaf.subcommand() {
            leaf = child;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut globals = Globals::resolve(&matches, &Default::default(), &root, |_| None)?;
        globals.apply_with_env(leaf, &[], |_| None)?;
        globals.json = json_mode;
        let sink = MemorySink::default();
        let prompt_messages = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(TerminalFixture {
                tty: !answers.is_empty(),
                answers: answers.iter().map(|text| (*text).into()).collect(),
                messages: prompt_messages.clone(),
            }))),
        );
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode,
                quiet: false,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        Ok(Self {
            matches,
            ctx,
            sink,
            history: PromptHistory::default(),
            prompt_messages,
        })
    }
    async fn run(&mut self, backend: &Fake) -> Result<(), rtao_cli::error::CliError> {
        let (_, group) = self
            .matches
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing group"))?;
        let group = if group.subcommand_name() == Some("timelock") {
            group
                .subcommand()
                .ok_or_else(|| rtao_cli::error::CliError::input("missing timelock"))?
                .1
        } else {
            group
        };
        timelock::run_with_backend(&self.ctx, group, &mut self.history, backend).await
    }
    fn payload(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_slice(&self.sink.bytes(Stream::Stdout)?)?)
    }
}
fn tree() -> clap::Command {
    application::command_tree([clap::Command::new("misc").subcommand(timelock::command())])
}
fn sealed(size: usize, round: u64) -> Result<Timelocked, TimelockError> {
    let mut bytes = vec![0x7f; size.saturating_sub(16)];
    bytes.extend_from_slice(b"AES_GCM_");
    bytes.extend_from_slice(&round.to_le_bytes());
    Timelocked::parse(bytes)
}
#[derive(Default)]
struct State {
    seals: Vec<(usize, Reveal)>,
    opens: Vec<(u64, OpenOptions)>,
}
struct Fake {
    state: Mutex<State>,
    sealed_size: usize,
    plaintext: Vec<u8>,
    not_ready: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            sealed_size: 20,
            plaintext: b"fixture plaintext".to_vec(),
            not_ready: false,
        }
    }
}
impl TimelockBackend for Fake {
    fn seal<'a>(&'a self, payload: &'a [u8], reveal: Reveal) -> TimelockFuture<'a, Timelocked> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| TimelockError::Input("fixture lock".into()))?
                .seals
                .push((payload.len(), reveal));
            sealed(self.sealed_size, 1)
        })
    }
    fn open<'a>(
        &'a self,
        sealed: &'a Timelocked,
        options: OpenOptions,
    ) -> TimelockFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.state
                .lock()
                .map_err(|_| TimelockError::Input("fixture lock".into()))?
                .opens
                .push((sealed.reveal_round(), options));
            if self.not_ready {
                return Err(TimelockError::NotReady {
                    reveal_round: sealed.reveal_round(),
                    reveal_at: sealed.reveal_at()?,
                    remaining: Duration::from_secs(4),
                });
            }
            Ok(self.plaintext.clone())
        })
    }
}

#[test]
fn routes_preserve_misc_and_hidden_v9_aliases_and_native_reveal_boundaries() -> TestResult {
    for path in [
        vec!["rtao", "misc", "timelock"],
        vec!["rtao", "utils", "timelock"],
        vec!["rtao", "timelock"],
    ] {
        let mut args = path;
        args.extend(["encrypt", "text", "--in", "1h30m"]);
        let _ = tree().try_get_matches_from(args)?;
    }
    for args in [
        vec![
            "rtao", "timelock", "encrypt", "text", "--in", "1s", "--round", "3",
        ],
        vec![
            "rtao",
            "timelock",
            "encrypt",
            "text",
            "--round",
            "18446744073709551616",
        ],
    ] {
        assert!(tree().try_get_matches_from(args).is_err());
    }
    let mut group = timelock::command();
    let help = group.render_long_help().to_string();
    assert!(help.contains("Guide: https://www.bittensor.com/docs/guides/timelock"));
    for (name, expected) in [
        (
            "encrypt",
            vec![
                "Seal text or a file until a future moment; prints the ciphertext hex.",
                "The reveal round is embedded in the ciphertext, so the hex is all anyone needs to decrypt later -- no key, no metadata, no account.",
                "Text to seal, or a path to a file whose contents to seal.",
                "Read the payload from this file instead of the argument.",
                "Write raw ciphertext bytes here instead of printing hex.",
                "Duration until the reveal: '30s', '15m', '1h30m', '2d', or plain seconds.",
                "Absolute reveal moment, ISO-8601: '2026-07-08T12:00Z' (no offset = UTC).",
                "Explicit drand quicknet round number to encrypt to.",
                "Regular files only; plaintext must be just under 8 MiB.",
                "Nominal 12-second blocks until reveal (rtao addition D-033).",
            ],
        ),
        (
            "decrypt",
            vec![
                "Open a timelocked ciphertext once its reveal moment has passed.",
                "Ciphertext hex (from encrypt), or a path to a ciphertext file.",
                "Read the ciphertext from this file (raw bytes or hex).",
                "Write the decrypted bytes here instead of printing.",
                "Sleep until the reveal moment instead of failing early.",
                "Give up after this many seconds (with --wait).",
                "Total decryption deadline, including beacon access and crypto. Must be finite and non-negative.",
                "Regular files only; ciphertext input is limited to 16 MiB.",
            ],
        ),
        (
            "show",
            vec![
                "Show when a ciphertext unlocks, without decrypting it.",
                "Ciphertext hex (from encrypt), or a path to a ciphertext file.",
                "Read the ciphertext from this file (raw bytes or hex).",
                "Regular files only; ciphertext input is limited to 16 MiB.",
            ],
        ),
    ] {
        let help = group
            .find_subcommand_mut(name)
            .ok_or("timelock help command")?
            .render_long_help()
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for expected in expected {
            assert!(
                help.contains(expected),
                "{name}: missing {expected:?} in {help}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn conflicting_sources_keep_python_callback_errors_in_human_and_json_modes() -> TestResult {
    let backend = Fake::default();
    for json_mode in [false, true] {
        for name in ["encrypt", "show", "decrypt"] {
            for (data, file) in [("text", "missing"), ("", "")] {
                // Encrypt has no reveal option: the source conflict must win before prompts.
                let mut fixture = Fixture::new(
                    &["rtao", "timelock", name, data, "--file", file],
                    json_mode,
                    &["must not be prompted"],
                )?;
                let error = fixture
                    .run(&backend)
                    .await
                    .err()
                    .ok_or("conflicting sources unexpectedly succeeded")?;
                let (message, help) = if name == "encrypt" {
                    (
                        "give the payload exactly one way",
                        "pass text or a file path as an argument, or use --file",
                    )
                } else {
                    (
                        "give the ciphertext exactly one way",
                        "pass the hex or a file path as an argument, or use --file",
                    )
                };
                assert_eq!(error.code, 1);
                assert_eq!(error.message, message);
                assert_eq!(error.help.as_deref(), Some(help));
                assert!(
                    fixture
                        .prompt_messages
                        .lock()
                        .map_err(|_| "prompt messages")?
                        .is_empty()
                );
                // Render through the same shared Output call as main's error boundary.
                fixture.ctx.output.error(
                    &error.message,
                    DiagnosticContext {
                        help: error.help.as_deref(),
                        ..Default::default()
                    },
                )?;
                assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
                let rendered = fixture.sink.text(Stream::Stderr)?;
                if json_mode {
                    assert_eq!(
                        serde_json::from_str::<Value>(&rendered)?,
                        json!({"error":message,"help":help})
                    );
                } else {
                    assert_eq!(rendered, format!("error: {message}\n  help: {help}\n"));
                }
            }
        }
    }
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert!(state.seals.is_empty() && state.opens.is_empty());
    Ok(())
}

#[tokio::test]
async fn encrypt_json_has_python_fields_and_native_reveal_selection() -> TestResult {
    let mut fixture = Fixture::new(
        &["rtao", "timelock", "encrypt", "hi", "--blocks", "5"],
        true,
        &[],
    )?;
    let backend = Fake::default();
    fixture.run(&backend).await?;
    let result = fixture.payload()?;
    let expected = sealed(20, 1)?;
    assert_eq!(
        result,
        json!({"ciphertext":expected.hex(),"reveal_round":1,"reveal_at":expected.reveal_at()?.to_rfc3339(),"reveal_in_seconds":0,"bytes":20})
    );
    assert_eq!(
        backend.state.lock().map_err(|_| "fixture lock")?.seals,
        vec![(2, Reveal::InBlocks(5))]
    );
    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn empty_output_paths_print_the_same_payload_as_omitted_paths() -> TestResult {
    let sealed = sealed(20, 1)?;
    let encoded = sealed.hex();
    for json_mode in [true, false] {
        for name in ["encrypt", "decrypt"] {
            let mut args = vec!["rtao", "timelock", name];
            if name == "encrypt" {
                args.extend(["text", "--in", "1s"]);
            } else {
                args.push(&encoded);
            }
            args.extend(["--out", ""]);
            let mut fixture = Fixture::new(&args, json_mode, &[])?;
            let backend = Fake::default();
            fixture.run(&backend).await?;
            if json_mode {
                let expected = if name == "encrypt" {
                    json!({"ciphertext":encoded,"reveal_round":1,"reveal_at":sealed.reveal_at()?.to_rfc3339(),"reveal_in_seconds":0,"bytes":20})
                } else {
                    json!({"plaintext":"fixture plaintext","plaintext_hex":hex::encode("fixture plaintext"),"reveal_round":1})
                };
                assert_eq!(fixture.payload()?, expected);
                assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
            } else {
                let expected = if name == "encrypt" {
                    encoded.as_str()
                } else {
                    "fixture plaintext"
                };
                assert_eq!(fixture.sink.text(Stream::Stdout)?, format!("{expected}\n"));
                if name == "encrypt" {
                    assert_eq!(
                        fixture.sink.text(Stream::Stderr)?,
                        format!(
                            "revealed since {} (round 1)\n",
                            sealed.reveal_at()?.to_rfc3339()
                        )
                    );
                } else {
                    assert!(fixture.sink.bytes(Stream::Stderr)?.is_empty());
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn show_is_offline_and_preserves_json_and_human_field_difference() -> TestResult {
    let expected = sealed(20, 1)?;
    let encoded = expected.hex();
    let mut fixture = Fixture::new(&["rtao", "timelock", "show", &encoded], true, &[])?;
    let backend = Fake::default();
    fixture.run(&backend).await?;
    assert_eq!(
        fixture.payload()?,
        json!({"reveal_round":1,"reveal_at":expected.reveal_at()?.to_rfc3339(),"revealed":true,"remaining_seconds":0,"bytes":20})
    );
    {
        let state = backend.state.lock().map_err(|_| "fixture lock")?;
        assert!(state.seals.is_empty() && state.opens.is_empty());
    }
    let mut human = Fixture::new(&["rtao", "timelock", "show", &encoded], false, &[])?;
    human.run(&backend).await?;
    let text = String::from_utf8(human.sink.bytes(Stream::Stdout)?)?;
    assert!(text.contains("remaining") && !text.contains("remaining_seconds"));
    Ok(())
}

#[tokio::test]
async fn decrypt_has_explicit_wait_deadline_and_null_binary_plaintext() -> TestResult {
    let encoded = sealed(20, 1)?.hex();
    let mut fixture = Fixture::new(
        &[
            "rtao",
            "timelock",
            "decrypt",
            &encoded,
            "--wait",
            "--timeout",
            "2.5",
        ],
        true,
        &[],
    )?;
    let backend = Fake {
        plaintext: vec![0xff, 0, 1],
        ..Default::default()
    };
    fixture.run(&backend).await?;
    assert_eq!(
        fixture.payload()?,
        json!({"plaintext":null,"plaintext_hex":"ff0001","reveal_round":1})
    );
    assert_eq!(
        backend.state.lock().map_err(|_| "fixture lock")?.opens,
        vec![(
            1,
            OpenOptions {
                wait: true,
                timeout: Some(Duration::from_millis(2500))
            }
        )]
    );
    Ok(())
}

#[tokio::test]
async fn decrypt_text_is_escaped_for_humans_and_value_equal_in_json() -> TestResult {
    let encoded = sealed(20, 1)?.hex();
    let plaintext = "hello\u{1b}[31m\n\u{202e}";
    let backend = Fake {
        plaintext: plaintext.as_bytes().to_vec(),
        ..Default::default()
    };
    for json_mode in [true, false] {
        let mut fixture = Fixture::new(&["rtao", "timelock", "decrypt", &encoded], json_mode, &[])?;
        fixture.run(&backend).await?;
        if json_mode {
            assert_eq!(
                fixture.payload()?,
                json!({"plaintext":plaintext,"plaintext_hex":hex::encode(plaintext),"reveal_round":1})
            );
        } else {
            let printed = String::from_utf8(fixture.sink.bytes(Stream::Stdout)?)?;
            assert!(!printed.contains('\u{1b}') && !printed.contains('\u{202e}'));
            assert!(printed.contains("hello"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn missing_inputs_and_invalid_deadlines_fail_before_crypto() -> TestResult {
    let backend = Fake::default();
    let mut missing = Fixture::new(&["rtao", "timelock", "encrypt"], true, &[])?;
    let error = missing
        .run(&backend)
        .await
        .err()
        .ok_or("missing input unexpectedly succeeded")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("data") && error.message.contains("--in"));
    let encoded = sealed(20, 1)?.hex();
    for timeout in ["NaN", "inf", "-1", "not-a-number"] {
        let mut fixture = Fixture::new(
            &[
                "rtao",
                "timelock",
                "decrypt",
                &encoded,
                "--timeout",
                timeout,
            ],
            true,
            &[],
        )?;
        assert_eq!(
            fixture
                .run(&backend)
                .await
                .err()
                .ok_or("invalid deadline succeeded")?
                .code,
            2
        );
    }
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert!(state.seals.is_empty() && state.opens.is_empty());
    Ok(())
}

#[tokio::test]
async fn input_acquisition_errors_precede_reveal_and_deadline_validation() -> TestResult {
    let root = FileRoot::new()?;
    let missing = root.0.join("missing-payload.bin");
    let path = missing.to_str().ok_or("missing path utf8")?;
    let backend = Fake::default();
    // Python reads the file before encrypt validates a supplied reveal moment.
    for (reveal_args, answers) in [
        (vec!["--in", "not-a-duration"], vec![]),
        (vec![], vec!["0"]),
    ] {
        let mut argv = vec!["rtao", "timelock", "encrypt", "--file", path];
        argv.extend(reveal_args);
        let mut fixture = Fixture::new(&argv, false, &answers)?;
        let error = fixture
            .run(&backend)
            .await
            .err()
            .ok_or("missing payload unexpectedly succeeded")?;
        assert_eq!(error.code, 1);
        assert!(error.message.contains(path), "{}", error.message);
        assert!(!error.message.contains("duration"));
        assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    }
    // A syntactically valid hex value with no trailer is rejected before timeout policy.
    for timeout in ["2.5", "NaN", "-1"] {
        let mut fixture = Fixture::new(
            &["rtao", "timelock", "decrypt", "00", "--timeout", timeout],
            true,
            &[],
        )?;
        let error = fixture
            .run(&backend)
            .await
            .err()
            .ok_or("malformed ciphertext unexpectedly succeeded")?;
        assert_eq!(error.code, 1);
        assert_eq!(
            error.message,
            "not a timelock ciphertext (missing the reveal-round trailer)"
        );
        assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    }
    let state = backend.state.lock().map_err(|_| "fixture lock")?;
    assert!(state.seals.is_empty() && state.opens.is_empty());
    Ok(())
}

#[tokio::test]
async fn duration_prompt_accepts_python_grammar_before_checked_encryption() -> TestResult {
    let backend = Fake::default();
    for answer in ["0", "0s", "-1", "-1.5", "-0.0"] {
        let mut fixture = Fixture::new(
            &["rtao", "timelock", "encrypt", "payload"],
            false,
            &[answer],
        )?;
        let error = fixture
            .run(&backend)
            .await
            .err()
            .ok_or("nonpositive reveal unexpectedly succeeded")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, "reveal_in must be a positive duration");
        assert_eq!(
            fixture
                .prompt_messages
                .lock()
                .map_err(|_| "prompt messages")?
                .as_slice(),
            ["--in: Duration until the reveal: '30s', '15m', '1h30m', '2d', or plain seconds."]
        );
        assert_eq!(
            fixture.history.replay_command(&[
                "rtao".into(),
                "timelock".into(),
                "encrypt".into(),
                "payload".into()
            ]),
            format!("rtao timelock encrypt payload --in {answer}")
        );
        assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    }
    {
        let state = backend.state.lock().map_err(|_| "fixture lock")?;
        assert!(state.seals.is_empty() && state.opens.is_empty());
    }
    let mut fixture = Fixture::new(
        &["rtao", "timelock", "encrypt", "payload"],
        false,
        &["-1s", "1h30m"],
    )?;
    fixture.run(&backend).await?;
    assert_eq!(
        fixture
            .prompt_messages
            .lock()
            .map_err(|_| "prompt messages")?
            .as_slice(),
        [
            "--in: Duration until the reveal: '30s', '15m', '1h30m', '2d', or plain seconds.",
            "cannot parse duration '-1s'; use seconds or units like '90s', '15m', '1h30m', '2d'"
        ]
    );
    assert_eq!(
        backend.state.lock().map_err(|_| "fixture lock")?.seals,
        vec![(7, Reveal::In(Duration::from_secs(5400)))]
    );
    assert_eq!(
        fixture.history.replay_command(&[
            "rtao".into(),
            "timelock".into(),
            "encrypt".into(),
            "payload".into()
        ]),
        "rtao timelock encrypt payload --in 1h30m"
    );
    Ok(())
}

#[tokio::test]
async fn not_ready_error_has_reveal_remediation_and_no_plaintext_output() -> TestResult {
    let encoded = sealed(20, 1)?.hex();
    let mut fixture = Fixture::new(&["rtao", "timelock", "decrypt", &encoded], true, &[])?;
    let backend = Fake {
        not_ready: true,
        ..Default::default()
    };
    let error = fixture
        .run(&backend)
        .await
        .err()
        .ok_or("not-ready unexpectedly succeeded")?;
    assert_eq!(error.code, 1);
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--wait"))
    );
    assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn boundary_ciphertext_roundtrips_as_prefixed_hex_and_oversized_output_is_not_emitted()
-> TestResult {
    let backend = Fake {
        sealed_size: MAX_CIPHERTEXT_BYTES,
        ..Default::default()
    };
    let mut encrypt = Fixture::new(
        &["rtao", "timelock", "encrypt", "text", "--in", "1s"],
        true,
        &[],
    )?;
    encrypt.run(&backend).await?;
    let payload = encrypt.payload()?;
    let encoded = format!(
        "0x{}",
        payload["ciphertext"].as_str().ok_or("missing ciphertext")?
    );
    assert_eq!(encoded.len(), rtao::client::config::MAX_JSON_FILE_BYTES);
    let mut show = Fixture::new(&["rtao", "timelock", "show", &encoded], true, &[])?;
    show.run(&backend).await?;
    assert_eq!(show.payload()?["bytes"], json!(MAX_CIPHERTEXT_BYTES));
    let backend = Fake {
        sealed_size: MAX_CIPHERTEXT_BYTES + 1,
        ..Default::default()
    };
    let mut rejected = Fixture::new(
        &["rtao", "timelock", "encrypt", "text", "--in", "1s"],
        true,
        &[],
    )?;
    assert!(rejected.run(&backend).await.is_err());
    assert!(rejected.sink.bytes(Stream::Stdout)?.is_empty());
    let large = "x".repeat(MAX_PLAINTEXT_BYTES + 1);
    let backend = Fake::default();
    let mut rejected = Fixture::new(
        &["rtao", "timelock", "encrypt", &large, "--in", "1s"],
        true,
        &[],
    )?;
    assert!(rejected.run(&backend).await.is_err());
    assert!(
        backend
            .state
            .lock()
            .map_err(|_| "fixture lock")?
            .seals
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn prompted_large_hex_uses_same_decode_path_and_plaintext_is_omitted_from_replay()
-> TestResult {
    let encoded = sealed(40 * 1024, 1)?.hex();
    let mut show = Fixture::new(&["rtao", "timelock", "show"], false, &[&encoded])?;
    show.run(&Fake::default()).await?;
    assert_eq!(
        show.prompt_messages
            .lock()
            .map_err(|_| "prompt messages")?
            .as_slice(),
        ["ciphertext: Ciphertext hex (from encrypt), or a path to a ciphertext file."]
    );
    let mut decrypt = Fixture::new(&["rtao", "timelock", "decrypt"], false, &[&encoded])?;
    decrypt.run(&Fake::default()).await?;
    assert_eq!(
        decrypt
            .prompt_messages
            .lock()
            .map_err(|_| "prompt messages")?
            .as_slice(),
        ["ciphertext: Ciphertext hex (from encrypt), or a path to a ciphertext file."]
    );
    for (name, fixture) in [("show", &show), ("decrypt", &decrypt)] {
        let replay =
            fixture
                .history
                .replay_command(&["rtao".into(), "timelock".into(), name.into()]);
        assert_eq!(replay, format!("rtao timelock {name} {encoded}"));
        let argv = replay.split_whitespace().collect::<Vec<_>>();
        let mut replayed = Fixture::new(&argv, false, &[])?;
        replayed.run(&Fake::default()).await?;
        assert_eq!(
            replayed.sink.bytes(Stream::Stdout)?,
            fixture.sink.bytes(Stream::Stdout)?
        );
    }
    // Prompted public ciphertext uses the ordinary shell quoting path. An invalid
    // answer is re-prompted and must never enter the replay command.
    let short = sealed(20, 1)?.hex();
    let wrapped = format!("0x{} {}", &short[..2], &short[2..]);
    for name in ["show", "decrypt"] {
        let mut fixture = Fixture::new(
            &["rtao", "timelock", name],
            false,
            &["invalid-ciphertext", &wrapped],
        )?;
        fixture.run(&Fake::default()).await?;
        let replay =
            fixture
                .history
                .replay_command(&["rtao".into(), "timelock".into(), name.into()]);
        let prefix = format!("rtao timelock {name} '");
        assert_eq!(replay, format!("{prefix}{wrapped}'"));
        let replayed_data = replay
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix('\''))
            .ok_or("missing quoted ciphertext replay")?;
        let mut replayed = Fixture::new(&["rtao", "timelock", name, replayed_data], false, &[])?;
        replayed.run(&Fake::default()).await?;
        assert_eq!(
            replayed.sink.bytes(Stream::Stdout)?,
            fixture.sink.bytes(Stream::Stdout)?
        );
    }
    let secret = "do-not-replay-this-plaintext";
    let mut encrypt = Fixture::new(&["rtao", "timelock", "encrypt"], false, &[secret, "1h"])?;
    encrypt.run(&Fake::default()).await?;
    assert_eq!(
        encrypt
            .prompt_messages
            .lock()
            .map_err(|_| "prompt messages")?
            .as_slice(),
        [
            "data: Text to seal, or a path to a file whose contents to seal.",
            "--in: Duration until the reveal: '30s', '15m', '1h30m', '2d', or plain seconds."
        ]
    );
    let replay =
        encrypt
            .history
            .replay_command(&["rtao".into(), "timelock".into(), "encrypt".into()]);
    assert!(!replay.contains(secret));
    assert!(replay.contains("--in") && replay.contains("1h"));
    Ok(())
}

struct FileRoot(PathBuf);
impl FileRoot {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-fixtures")
            });
        let path = base.join("tmp").join(format!(
            "m7-timelock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for FileRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn files_accept_raw_or_wrapped_hex_and_output_preserves_binary_bytes() -> TestResult {
    let root = FileRoot::new()?;
    let raw = root.0.join("raw.bin");
    let wrapped = root.0.join("wrapped.hex");
    let out = root.0.join("plaintext.bin");
    let sealed = sealed(70 * 1024, 1)?;
    std::fs::write(&raw, sealed.ciphertext())?;
    let encoded = sealed.hex();
    std::fs::write(
        &wrapped,
        format!(
            "0x{}\t\n {}\r\n{}\x1c{}\x1d{}\x1e{}\x1f{}\x0b\x0c",
            &encoded[..32],
            &encoded[32..64],
            &encoded[64..97],
            &encoded[97..128],
            &encoded[128..160],
            &encoded[160..192],
            &encoded[192..],
        ),
    )?;
    for (path, explicit) in [(&raw, true), (&wrapped, false)] {
        let path = path.to_str().ok_or("fixture path utf8")?;
        let argv = if explicit {
            vec!["rtao", "timelock", "show", "--file", path]
        } else {
            vec!["rtao", "timelock", "show", path]
        };
        let mut fixture = Fixture::new(&argv, true, &[])?;
        fixture.run(&Fake::default()).await?;
        assert_eq!(fixture.payload()?["bytes"], json!(70 * 1024));
    }
    let backend = Fake {
        plaintext: vec![0xff, 0, 1],
        ..Default::default()
    };
    let mut fixture = Fixture::new(
        &[
            "rtao",
            "timelock",
            "decrypt",
            "--file",
            raw.to_str().ok_or("raw path")?,
            "--out",
            out.to_str().ok_or("out path")?,
        ],
        true,
        &[],
    )?;
    fixture.run(&backend).await?;
    assert_eq!(std::fs::read(&out)?, vec![0xff, 0, 1]);
    assert_eq!(
        fixture.payload()?,
        json!({"path":out.to_str().ok_or("out path")?,"bytes":3,"reveal_round":1})
    );
    Ok(())
}

#[tokio::test]
async fn oversized_sparse_ciphertext_is_rejected_before_beacon_and_left_unchanged() -> TestResult {
    let root = FileRoot::new()?;
    let path = root.0.join("oversized.bin");
    let file = std::fs::File::create(&path)?;
    let size = rtao::client::config::MAX_JSON_FILE_BYTES as u64 + 1;
    file.set_len(size)?;
    drop(file);
    let backend = Fake::default();
    let mut fixture = Fixture::new(
        &[
            "rtao",
            "timelock",
            "decrypt",
            "--file",
            path.to_str().ok_or("fixture path")?,
        ],
        true,
        &[],
    )?;
    assert!(fixture.run(&backend).await.is_err());
    assert_eq!(std::fs::metadata(&path)?.len(), size);
    assert!(
        backend
            .state
            .lock()
            .map_err(|_| "fixture lock")?
            .opens
            .is_empty()
    );
    assert!(fixture.sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn encryption_reads_file_bytes_and_writes_the_exact_raw_envelope() -> TestResult {
    let root = FileRoot::new()?;
    let input = root.0.join("input.bin");
    let output = root.0.join("ciphertext.bin");
    std::fs::write(&input, [0xff, 0, 1, 2])?;
    let backend = Fake::default();
    let mut fixture = Fixture::new(
        &[
            "rtao",
            "timelock",
            "encrypt",
            "--file",
            input.to_str().ok_or("input path")?,
            "--out",
            output.to_str().ok_or("out path")?,
            "--round",
            "123",
        ],
        true,
        &[],
    )?;
    fixture.run(&backend).await?;
    assert_eq!(std::fs::read(&output)?, sealed(20, 1)?.ciphertext());
    assert_eq!(
        backend.state.lock().map_err(|_| "fixture lock")?.seals,
        vec![(4, Reveal::Round(123))]
    );
    let result = fixture.payload()?;
    assert_eq!(result["path"], json!(output.to_str().ok_or("out path")?));
    assert_eq!(result["bytes"], 20);
    assert!(result.get("ciphertext").is_none());
    Ok(())
}
