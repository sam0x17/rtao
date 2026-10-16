//! Actual required-argument parsing confirms local signing wallets first.

use rtao::{
    primitives::{AccountId, CryptoType},
    signer::{DynKeySource, SignerRole},
    wallet::{
        NativePasswordFuture, NativePasswordProvider, PasswordPrompt, PasswordSources,
        PromptPolicy, RegenerateFrom, SecretString, Wallet, WalletError, WriteOptions,
    },
};
use rtao_cli::{
    application,
    books::Books,
    commands::{addresses, generated::TxRequest, misc, proxy, wallet},
    context::{AppContext, CliWalletKeys},
    error::CliError,
    globals::Globals,
    output::{DiagnosticContext, MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptContext, PromptError, PromptHistory, SharedTerminal, Terminal},
};
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Trace = Arc<Mutex<Vec<String>>>;
const WRONG_PASSWORD: &str = "wrong-password-private-sentinel";
#[derive(Default)]
struct WrongPasswords {
    prompts: AtomicUsize,
    keychain: AtomicUsize,
    dialogs: AtomicUsize,
}
impl PasswordPrompt for WrongPasswords {
    fn prompt(&self, _: &str) -> Result<SecretString, WalletError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        Ok(SecretString::new(WRONG_PASSWORD.into()))
    }
}
impl NativePasswordProvider for WrongPasswords {
    fn keychain_load<'a>(
        &'a self,
        wallet: &'a str,
    ) -> NativePasswordFuture<'a, Option<SecretString>> {
        Box::pin(async move {
            assert_eq!(wallet, "encrypted");
            self.keychain.fetch_add(1, Ordering::SeqCst);
            Ok(Some(SecretString::new(WRONG_PASSWORD.into())))
        })
    }
    fn macos_prompt<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
    ) -> NativePasswordFuture<'a, SecretString> {
        Box::pin(async move {
            assert_eq!(title, "Unlock encrypted coldkey");
            assert_eq!(message, "Enter the password for wallet 'encrypted'.");
            self.dialogs.fetch_add(1, Ordering::SeqCst);
            Ok(SecretString::new(WRONG_PASSWORD.into()))
        })
    }
}
struct Input {
    tty: bool,
    err_tty: bool,
    lines: VecDeque<String>,
    trace: Trace,
}
impl Terminal for Input {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.err_tty
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.trace
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(label.into());
        self.lines.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        self.trace
            .lock()
            .map_err(|_| PromptError::Io)?
            .push("PRIVATE INPUT MUST NOT RUN".into());
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn terminal(lines: &[&str], tty: bool, err_tty: bool) -> (SharedTerminal, Trace) {
    let trace = Trace::default();
    let input = Input {
        tty,
        err_tty,
        lines: lines.iter().map(|line| (*line).into()).collect(),
        trace: trace.clone(),
    };
    (Arc::new(Mutex::new(Box::new(input))), trace)
}
struct Root(PathBuf);
impl Root {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-fixtures")
            });
        let path = base.join("tmp").join(format!(
            "generic-wallet-prompts-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = Self(path);
        std::fs::create_dir_all(&root.0)?;
        for (name, byte) in [("configured", 7), ("chosen", 8)] {
            let wallet = root.0.join(name);
            std::fs::create_dir(&wallet)?;
            std::fs::write(
                wallet.join("coldkeypub.txt"),
                serde_json::to_vec(&json!({"ss58Address":AccountId([byte;32]),"cryptoType":1}))?,
            )?;
            // Public prompt resolution must succeed despite unusable private
            // material. The actual sign/decrypt body will reject this later.
            std::fs::write(
                wallet.join("coldkey"),
                b"private-key acquisition must wait until after prompts",
            )?;
        }
        Ok(root)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tree() -> clap::Command {
    application::command_tree([wallet::command(), misc::command()])
}
fn public_context(
    root: &Root,
    parsed: &clap::ArgMatches,
    terminal: SharedTerminal,
    books: Books,
    config_wallet: &str,
    env_wallet: Option<&str>,
) -> Result<AppContext, Box<dyn std::error::Error>> {
    let config = Map::from_iter([
        ("wallet".into(), json!(config_wallet)),
        ("wallet_path".into(), json!(root.0)),
    ]);
    let env = |name: &str| {
        if name == "BT_WALLET" {
            env_wallet.map(str::to_owned)
        } else {
            None
        }
    };
    let mut globals = Globals::resolve(parsed, &config, Path::new("/fixture/no-user-home"), env)?;
    let mut leaf = parsed;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let reserved = application::pending_prompt_reserved(parsed)?;
    globals.apply_with_env(leaf, &reserved, env)?;
    let mut app = AppContext::new(globals, config, books, terminal);
    app.output = Output::new(OutputOptions::default(), Arc::new(MemorySink::default()));
    Ok(app)
}
fn original(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}
fn strings(words: &[OsString]) -> Vec<String> {
    words
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

#[tokio::test]
async fn actual_sign_and_decrypt_prompt_wallet_before_required_input_and_do_not_repeat_it()
-> TestResult {
    // Run the real resolver with a clean child environment; mutating process
    // variables would race other tests and the wallet's blocking workers.
    if let Some(root) = std::env::var_os("RTAO_PASSWORD_CONTRACT_ROOT") {
        let root = PathBuf::from(root);
        for (source, role, json_mode, quiet, prompts, keychain, dialogs, expected_help) in [
            (
                "prompt",
                SignerRole::Coldkey,
                false,
                false,
                3,
                0,
                0,
                "re-run and enter the coldkey password used when the key was created",
            ),
            (
                "prompt",
                SignerRole::Coldkey,
                true,
                false,
                3,
                0,
                0,
                "re-run and enter the coldkey password used when the key was created",
            ),
            (
                "prompt",
                SignerRole::Coldkey,
                false,
                true,
                3,
                0,
                0,
                "re-run and enter the coldkey password used when the key was created",
            ),
            (
                "prompt",
                SignerRole::Hotkey,
                false,
                false,
                1,
                0,
                0,
                "re-run and enter the hotkey password used when the key was created",
            ),
            (
                "keychain",
                SignerRole::Coldkey,
                false,
                false,
                0,
                1,
                0,
                "re-save with `rtao wallet keychain save`",
            ),
            (
                "dialog",
                SignerRole::Coldkey,
                false,
                false,
                0,
                0,
                1,
                "re-run and enter the coldkey password in the macOS password dialog",
            ),
            (
                "explicit",
                SignerRole::Coldkey,
                false,
                false,
                0,
                0,
                0,
                "supply the coldkey password used when the key was created",
            ),
            (
                "file",
                SignerRole::Coldkey,
                false,
                false,
                0,
                0,
                0,
                "correct the selected password file, then retry",
            ),
        ] {
            let attempts = Arc::new(WrongPasswords::default());
            let sources = PasswordSources {
                password: (source == "explicit").then(|| SecretString::new(WRONG_PASSWORD.into())),
                password_file: (source == "file").then(|| root.join("password.txt")),
                keychain: matches!(source, "keychain" | "explicit" | "file")
                    || role == SignerRole::Hotkey,
                macos_prompt: matches!(source, "dialog" | "explicit" | "file")
                    || role == SignerRole::Hotkey,
                prompt: PromptPolicy::Always,
                prompt_callback: Some(attempts.clone()),
                native_provider: Some(attempts.clone()),
            };
            let sink = MemorySink::default();
            let output = Output::new(
                OutputOptions {
                    json_mode,
                    quiet,
                    ..Default::default()
                },
                Arc::new(sink.clone()),
            );
            let keys = CliWalletKeys::new(
                Wallet::new("encrypted", "worker", &root)?.with_password_sources(sources),
                output.clone(),
            );
            let error = keys
                .signer_for(role)
                .await
                .err()
                .ok_or("wrong password accepted")?;
            assert!(error.to_string().starts_with("wrong password ("));
            assert_eq!(error.help(), Some(expected_help));
            assert!(!format!("{error:?}").contains(WRONG_PASSWORD));
            assert_eq!(attempts.prompts.load(Ordering::SeqCst), prompts);
            assert_eq!(attempts.keychain.load(Ordering::SeqCst), keychain);
            assert_eq!(attempts.dialogs.load(Ordering::SeqCst), dialogs);
            let before = sink.text(Stream::Stderr)?;
            let notices = if prompts == 3 && !json_mode && !quiet {
                2
            } else {
                0
            };
            assert_eq!(before.lines().count(), notices);
            assert!(before.lines().all(|line| line == "error: wrong password"));
            let client_error = CliError::from(rtao::client::ClientError::Signing(error.clone()));
            assert_eq!(client_error.code, 1);
            assert_eq!(client_error.message, error.to_string());
            assert_eq!(client_error.help.as_deref(), Some(expected_help));
            let transaction =
                rtao::tx::TxError::from(rtao::client::ClientError::Signing(error.clone()));
            assert!(matches!(
                transaction,
                rtao::tx::TxError::Signer(ref signer) if signer.help() == Some(expected_help)
            ));
            let error = CliError::from(error);
            assert_eq!(error.code, 1);
            assert!(!error.message.is_empty());
            assert_eq!(error.help.as_deref(), Some(expected_help));
            output.error(
                &error.message,
                DiagnosticContext {
                    help: error.help.as_deref(),
                    ..Default::default()
                },
            )?;
            let rendered = sink.text(Stream::Stderr)?;
            if json_mode {
                let diagnostic: Value = serde_json::from_str(&rendered)?;
                assert_eq!(
                    diagnostic,
                    json!({"error":error.message,"help":expected_help})
                );
            } else {
                assert_eq!(
                    rendered.matches("error: wrong password").count(),
                    notices + 1
                );
                assert!(rendered.contains(expected_help));
            }
            assert!(sink.bytes(Stream::Stdout)?.is_empty());
            assert!(!rendered.contains(WRONG_PASSWORD));
        }
        return Ok(());
    }
    let root = Root::new()?;
    for (family, command, field, use_hotkey) in [
        ("wallet", "sign", "message", false),
        ("w", "sign", "message", false),
        ("wallet", "decrypt", "ciphertext", false),
        ("wallet", "sign", "message", true),
        ("wallet", "decrypt", "ciphertext", true),
    ] {
        let mut words = vec!["rtao", family, command];
        if use_hotkey {
            words.push("--use-hotkey");
        }
        let original = original(&words);
        let mut argv = original.clone();
        let (terminal, trace) = terminal(&["missing", "chosen", "public-payload"], true, true);
        let mut input = terminal.clone();
        let partial =
            application::pending_wallet_matches(&tree(), &argv, &PromptContext::default(), &input)?
                .ok_or("wallet round")?;
        let mut app = public_context(
            &root,
            &partial,
            terminal,
            Books::default(),
            "configured",
            None,
        )?;
        let mut history = PromptHistory::default();
        application::confirm_required_wallet(&mut app, &mut argv, &mut history)?;
        let mut invocation = application::parse_with_prompt_history(
            tree(),
            argv,
            &app.prompt_context(),
            &mut input,
            history,
        )?;
        assert_eq!(app.globals.wallet_name, "chosen");
        assert!(app.globals.wallet_given);
        let reads = trace.lock().map_err(|_| "trace")?.clone();
        assert_eq!(reads.len(), 3);
        assert!(
            reads[0].contains("wallet [configured]") && reads[1].contains("wallet [configured]")
        );
        assert!(reads[2].contains(field));
        assert_eq!(
            invocation.history.replay_command(&strings(&original)),
            format!(
                "rtao {family} {command}{} --wallet chosen --{field} public-payload",
                if use_hotkey { " --use-hotkey" } else { "" }
            )
        );
        let chosen = AccountId([8; 32]).to_string();
        assert_eq!(
            app.output.address_names()?.get(&chosen).map(String::as_str),
            Some("chosen")
        );
        assert!(!app.output.address_names()?.contains_key("chosen"));
        // The ordinary command consumes the same selection and does not ask
        // again. Only now may it inspect the deliberately invalid private file.
        let group = invocation.matches.subcommand().ok_or("wallet group")?.1;
        assert!(
            wallet::run(&mut app, group, &mut invocation.history)
                .await
                .is_err()
        );
        assert_eq!(*trace.lock().map_err(|_| "trace")?, reads);
    }
    // Encrypt once, then reuse the actual keyfile through fresh, uncached
    // wallet handles. The same task root owns every subprocess fixture.
    let correct = "fixture-password-private-correct";
    let encrypted = Wallet::new("encrypted", "worker", &root.0)?;
    let account = encrypted
        .regenerate_coldkey(
            RegenerateFrom::Seed(zeroize::Zeroizing::new(vec![71; 32])),
            CryptoType::Ed25519,
            WriteOptions {
                password: Some(SecretString::new(correct.into())),
                ..WriteOptions::coldkey()
            },
        )
        .await?;
    let ciphertext = rtao::wallet::encrypt_message("private-message-fixture", &account.to_string())
        .await?
        .ciphertext;
    let private = std::fs::read(encrypted.reference().coldkey_path())?;
    assert!(rtao::core::keyfile_json::keyfile_data_is_encrypted_nacl(
        &private
    ));
    std::fs::create_dir_all(root.0.join("encrypted/hotkeys"))?;
    std::fs::write(encrypted.reference().hotkey_path(), &private)?;
    std::fs::write(
        encrypted.reference().hotkeypub_path(),
        std::fs::read(encrypted.reference().coldkeypub_path())?,
    )?;
    let password_file = root.0.join("password.txt");
    std::fs::write(&password_file, WRONG_PASSWORD)?;
    let cold_env = rtao::wallet::env_var_name(&encrypted.reference().coldkey_path());
    let hot_env = rtao::wallet::env_var_name(&encrypted.reference().hotkey_path());
    let mut child = std::process::Command::new(std::env::current_exe()?);
    child
        .args([
            "--exact",
            "actual_sign_and_decrypt_prompt_wallet_before_required_input_and_do_not_repeat_it",
            "--nocapture",
        ])
        .env("RTAO_PASSWORD_CONTRACT_ROOT", &root.0)
        .env_remove("BT_WALLET_PASSWORD")
        .env_remove("BT_WALLET_PASSWORD_FILE")
        .env_remove(&cold_env)
        .env_remove(&hot_env)
        .stdin(std::process::Stdio::null());
    let result = child.output()?;
    let stdout = String::from_utf8(result.stdout)?;
    let stderr = String::from_utf8(result.stderr)?;
    for text in [&stdout, &stderr] {
        assert!(!text.contains(WRONG_PASSWORD) && !text.contains(correct));
    }
    assert!(result.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("1 passed;"),
        "password child did not run: {stdout}"
    );

    // Exercise the installed command's final error renderer as well as the
    // public signer conversion. Earlier env/file sources must beat both native
    // flags even on hosts where those native services are unavailable.
    let encoded =
        rtao::core::keyfile_env::encode_password_for_environment(&cold_env, WRONG_PASSWORD)?;
    let file_value = password_file.to_string_lossy();
    for (operation, mode, variable, value, source, expected_help) in [
        (
            "sign",
            "--json",
            "BT_WALLET_PASSWORD",
            WRONG_PASSWORD,
            "BT_WALLET_PASSWORD",
            "correct or unset BT_WALLET_PASSWORD, then retry",
        ),
        (
            "sign",
            "--quiet",
            cold_env.as_str(),
            encoded.as_str(),
            "keyfile environment",
            "correct or remove the encoded password variable for this keyfile, then retry",
        ),
        (
            "decrypt",
            "--json",
            "BT_WALLET_PASSWORD_FILE",
            file_value.as_ref(),
            "password file",
            "correct the selected password file, then retry",
        ),
    ] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rtao"));
        command
            .args([
                "wallet",
                operation,
                "--wallet",
                "encrypted",
                "--wallet-hotkey",
                "worker",
                "--keychain-password",
                "--macos-password",
                mode,
                "--wallet-path",
            ])
            .arg(&root.0)
            .args(if operation == "sign" {
                ["--message", "public-payload"]
            } else {
                ["--ciphertext", ciphertext.as_str()]
            })
            // Startup's legacy-config warning also consults the child home.
            .env("HOME", &root.0)
            .env_remove("BT_WALLET_PASSWORD")
            .env("BT_WALLET_PASSWORD_FILE", &password_file)
            .env_remove(&cold_env)
            .env_remove(&hot_env)
            .env_remove("RTAO_PASSWORD_CONTRACT_ROOT")
            .env(variable, value)
            .stdin(std::process::Stdio::null());
        for (index, file) in rtao::client::config::ConfigFile::ALL.iter().enumerate() {
            command.env(
                file.environment(),
                root.0.join(format!("config-{index}.json")),
            );
        }
        let result = command.output()?;
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr)?;
        let expected_error = format!("wrong password ({source})");
        if mode == "--json" {
            let diagnostic: Value = serde_json::from_str(&stderr)?;
            assert_eq!(
                diagnostic,
                json!({"error":expected_error,"help":expected_help})
            );
        } else {
            assert_eq!(stderr.matches("error: wrong password").count(), 1);
            assert!(stderr.contains(&expected_error) && stderr.contains(expected_help));
        }
        for secret in [
            WRONG_PASSWORD,
            correct,
            "private-message-fixture",
            encoded.as_str(),
        ] {
            assert!(!stderr.contains(secret));
        }
    }
    assert_eq!(
        std::fs::read(encrypted.reference().coldkey_path())?,
        private
    );
    assert_eq!(std::fs::read(encrypted.reference().hotkey_path())?, private);
    Ok(())
}

#[test]
fn actual_generic_wallet_missing_fields_remain_noninteractive_and_never_touch_keys() -> TestResult {
    for (tty, err_tty, context) in [
        (false, true, PromptContext::default()),
        (true, false, PromptContext::default()),
        (
            true,
            true,
            PromptContext {
                json: true,
                ..Default::default()
            },
        ),
        (
            true,
            true,
            PromptContext {
                quiet: true,
                ..Default::default()
            },
        ),
        (
            true,
            true,
            PromptContext {
                policy: rtao::wallet::PromptPolicy::Never,
                ..Default::default()
            },
        ),
    ] {
        for (command, field) in [("sign", "--message"), ("decrypt", "--ciphertext")] {
            let (terminal, trace) = terminal(&[], tty, err_tty);
            let mut input = terminal;
            let argv = original(&["rtao", "wallet", command]);
            assert!(
                application::pending_wallet_matches(&tree(), &argv, &context, &input)?.is_none()
            );
            let error = application::parse_with_prompts(tree(), argv, &context, &mut input)
                .err()
                .ok_or("missing field")?;
            assert_eq!(error.code, 2);
            assert!(error.message.contains(field));
            assert!(trace.lock().map_err(|_| "trace")?.is_empty());
        }
    }
    Ok(())
}

#[test]
fn given_yes_external_and_environment_wallets_skip_only_the_confirmation() -> TestResult {
    let root = Root::new()?;
    for (words, env_wallet) in [
        (vec!["rtao", "wallet", "sign", "--wallet", "chosen"], None),
        (vec!["rtao", "--yes", "wallet", "sign"], None),
        (vec!["rtao", "--signer", "ledger", "wallet", "sign"], None),
        (vec!["rtao", "wallet", "sign"], Some("chosen")),
    ] {
        let (terminal, trace) = terminal(&["public-payload"], true, true);
        let mut input = terminal.clone();
        let mut argv = original(&words);
        let partial =
            application::pending_wallet_matches(&tree(), &argv, &PromptContext::default(), &input)?
                .ok_or("partial")?;
        let mut app = public_context(
            &root,
            &partial,
            terminal,
            Books::default(),
            "configured",
            env_wallet,
        )?;
        let unchanged = argv.clone();
        let mut history = PromptHistory::default();
        application::confirm_required_wallet(&mut app, &mut argv, &mut history)?;
        assert_eq!(argv, unchanged);
        let invocation = application::parse_with_prompt_history(
            tree(),
            argv,
            &app.prompt_context(),
            &mut input,
            history,
        )?;
        let reads = trace.lock().map_err(|_| "trace")?;
        assert_eq!(reads.len(), 1);
        assert!(reads[0].contains("message"));
        assert!(
            !invocation
                .history
                .replay_command(&strings(&unchanged))
                .contains("--wallet configured")
        );
    }
    Ok(())
}

#[test]
fn saved_multisig_default_reasks_before_actual_sign_message() -> TestResult {
    let root = Root::new()?;
    let (terminal, trace) = terminal(&["", "chosen", "public-payload"], true, true);
    let mut input = terminal.clone();
    let mut argv = original(&["rtao", "wallet", "sign"]);
    let partial =
        application::pending_wallet_matches(&tree(), &argv, &PromptContext::default(), &input)?
            .ok_or("partial")?;
    let books = Books::from_values(
        Value::Null,
        Value::Null,
        json!([{"name":"safe","signatories":["chosen"]}]),
    );
    let mut app = public_context(&root, &partial, terminal, books, "safe", None)?;
    let mut history = PromptHistory::default();
    application::confirm_required_wallet(&mut app, &mut argv, &mut history)?;
    let _invocation = application::parse_with_prompt_history(
        tree(),
        argv,
        &app.prompt_context(),
        &mut input,
        history,
    )?;
    let reads = trace.lock().map_err(|_| "trace")?;
    assert_eq!(reads.len(), 3);
    assert!(reads[0].contains("wallet") && reads[1].contains("wallet"));
    assert!(reads[2].contains("message"));
    assert_eq!(app.globals.wallet_name, "chosen");
    Ok(())
}

#[test]
fn exact_route_guard_leaves_other_commands_and_supplied_input_errors_to_existing_parser()
-> TestResult {
    for words in [
        vec!["rtao", "wallet", "verify"],
        vec!["rtao", "wallet", "encrypt"],
        vec!["rtao", "misc", "convert"],
        vec!["rtao", "wallet", "sign", "--message", "supplied"],
        vec!["rtao", "wallet", "sign", "--unknown"],
    ] {
        let (terminal, trace) = terminal(&[], true, true);
        assert!(
            application::pending_wallet_matches(
                &tree(),
                &original(&words),
                &PromptContext::default(),
                &terminal
            )?
            .is_none()
        );
        assert!(trace.lock().map_err(|_| "trace")?.is_empty());
    }
    Ok(())
}

#[test]
fn actual_wallet_leaf_and_root_json_quiet_flags_never_start_early_confirmation() -> TestResult {
    for (words, field) in [
        (vec!["rtao", "--json", "wallet", "sign"], "--message"),
        (vec!["rtao", "wallet", "sign", "--json"], "--message"),
        (vec!["rtao", "-q", "wallet", "decrypt"], "--ciphertext"),
        (vec!["rtao", "wallet", "decrypt", "--quiet"], "--ciphertext"),
    ] {
        let (terminal, trace) = terminal(&[], true, true);
        let mut input = terminal;
        let argv = original(&words);
        assert!(
            application::pending_wallet_matches(&tree(), &argv, &PromptContext::default(), &input)?
                .is_none()
        );
        let error =
            application::parse_with_prompts(tree(), argv, &PromptContext::default(), &mut input)
                .err()
                .ok_or("missing")?;
        assert_eq!(error.code, 2);
        assert!(error.message.contains(field));
        assert!(trace.lock().map_err(|_| "trace")?.is_empty());
    }
    Ok(())
}

fn picker_tree() -> Result<clap::Command, rtao_cli::error::CliError> {
    let command =
        application::command_tree([wallet::command(), addresses::command(), proxy::command()?]);
    #[cfg(feature = "evm")]
    let command = command.subcommand(rtao_cli::commands::evm::command()?);
    Ok(command)
}
fn picker_books() -> Books {
    Books::from_values(
        json!([
            {"name":"first-contact","address":AccountId([41;32])},
            {"name":"second-contact","address":AccountId([42;32])},
        ]),
        json!([]),
        json!([]),
    )
}
fn pending_context(
    root: &Root,
    command: &clap::Command,
    argv: &[OsString],
    terminal: SharedTerminal,
    books: Books,
) -> Result<AppContext, Box<dyn std::error::Error>> {
    let partial =
        application::pending_prompt_matches(command, argv, &PromptContext::default(), &terminal)?
            .ok_or("expected public context round")?;
    public_context(root, &partial, terminal, books, "configured", None)
}
fn invocation_leaf(matches: &clap::ArgMatches) -> &clap::ArgMatches {
    let mut leaf = matches;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    leaf
}
fn public_hotkey(root: &Root, wallet: &str, account: AccountId) -> TestResult {
    let path = root.0.join(wallet).join("hotkeys");
    std::fs::create_dir_all(&path)?;
    // Picker menus inspect the keyfile address; neither file contains secrets.
    let public_data = serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?;
    std::fs::write(path.join("workerpub.txt"), &public_data)?;
    std::fs::write(path.join("worker"), public_data)?;
    Ok(())
}

#[tokio::test]
async fn required_positional_address_keeps_declaration_order_retries_and_actual_save_handler()
-> TestResult {
    let root = Root::new()?;
    let command = picker_tree()?;
    let argv = original(&["rtao", "addr", "save"]);
    let (input, trace) = terminal(&["saved", "unknown-contact", "1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let invocation =
        application::parse_with_context(command, argv.clone(), &mut app, PromptHistory::default())?;
    let canonical = AccountId([41; 32]).to_string();
    let leaf = invocation_leaf(&invocation.matches);
    assert_eq!(
        leaf.get_one::<String>("name").map(String::as_str),
        Some("saved")
    );
    assert_eq!(leaf.get_one::<String>("ss58"), Some(&canonical));
    let reads = trace.lock().map_err(|_| "trace")?.clone();
    assert_eq!(reads.len(), 3);
    assert!(reads[0].contains("name"));
    assert!(reads[1].contains("ss58") && reads[2].contains("ss58"));
    assert_eq!(
        invocation.history.replay_command(&strings(&argv)),
        format!("rtao addr save saved {canonical}")
    );
    assert!(
        !strings(&invocation.argv)
            .iter()
            .any(|token| token == "--ss58")
    );
    let paths = rtao::client::config::Paths::new(&root.0, |_| None);
    addresses::run(
        &app,
        &paths,
        invocation.matches.subcommand().ok_or("addr group")?.1,
    )
    .await?;
    let saved: Value = serde_json::from_slice(&std::fs::read(
        paths.path(rtao::client::config::ConfigFile::Addresses),
    )?)?;
    let record = saved
        .as_array()
        .ok_or("saved contacts")?
        .iter()
        .find(|row| row["name"] == "saved")
        .ok_or("saved row")?;
    assert_eq!(record["address"], canonical);
    assert_eq!(trace.lock().map_err(|_| "trace")?.len(), reads.len());
    Ok(())
}

#[test]
fn required_hotkey_target_binds_to_confirmed_wallet_before_actual_typed_request() -> TestResult {
    let root = Root::new()?;
    let configured = AccountId([21; 32]);
    let chosen = AccountId([22; 32]);
    let old = AccountId([23; 32]).to_string();
    public_hotkey(&root, "configured", configured)?;
    public_hotkey(&root, "chosen", chosen)?;
    let command = picker_tree()?;
    let argv = original(&["rtao", "w", "swap-hotkey", "--hotkey", &old]);
    let (input, trace) = terminal(&["chosen", "1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let mut invocation =
        application::parse_with_context(command, argv.clone(), &mut app, PromptHistory::default())?;
    assert_eq!(app.globals.wallet_name, "chosen");
    assert!(app.globals.wallet_given);
    let leaf = invocation_leaf(&invocation.matches);
    assert_eq!(
        leaf.get_one::<String>("new_hotkey_ss58"),
        Some(&chosen.to_string())
    );
    let request = TxRequest::from_matches(&mut app, "swap_hotkey", leaf, &mut invocation.history)?;
    assert_eq!(request.args["new_hotkey_ss58"], json!(chosen));
    assert_eq!(request.args["hotkey_ss58"], old);
    request.intent()?;
    let reads = trace.lock().map_err(|_| "trace")?.clone();
    assert_eq!(reads.len(), 2);
    assert!(reads[0].contains("wallet [configured]"));
    assert!(reads[1].contains("new-hotkey"));
    assert_eq!(
        invocation.history.replay_command(&strings(&argv)),
        format!("rtao w swap-hotkey --hotkey {old} --wallet chosen --new-hotkey {chosen}")
    );
    assert!(!reads.iter().any(|line| line.contains("PRIVATE")));
    Ok(())
}

#[test]
fn reviewed_required_address_routes_keep_real_option_spellings_and_canonical_replay() -> TestResult
{
    let root = Root::new()?;
    let address = AccountId([41; 32]).to_string();
    let public_key = hex::encode([41u8; 32]);
    for (words, field, flag) in [
        (
            vec![
                "rtao",
                "wallet",
                "regen-coldkeypub",
                "--public-key",
                &public_key,
            ],
            "ss58",
            "--ss58",
        ),
        (
            vec![
                "rtao",
                "wallet",
                "regen-hotkeypub",
                "--public-key",
                &public_key,
            ],
            "ss58",
            "--ss58",
        ),
        (
            vec![
                "rtao",
                "wallet",
                "verify",
                "--message",
                "payload",
                "--signature",
                "00",
            ],
            "ss58",
            "--ss58",
        ),
        (
            vec!["rtao", "wallet", "swap-coldkey", "--wallet", "configured"],
            "new_coldkey_ss58",
            "--new-coldkey",
        ),
        (
            vec![
                "rtao",
                "wallet",
                "announce-coldkey-swap",
                "--wallet",
                "configured",
            ],
            "new_coldkey_ss58",
            "--new-coldkey",
        ),
        (
            vec!["rtao", "proxy", "add", "--wallet", "configured"],
            "delegate_ss58",
            "--delegate",
        ),
        (
            vec![
                "rtao",
                "proxy",
                "execute",
                "--wallet",
                "configured",
                "--real",
                &address,
                "--inner-op",
                "transfer",
            ],
            "delegate_ss58",
            "--delegate",
        ),
        (
            vec![
                "rtao",
                "proxy",
                "execute",
                "--wallet",
                "configured",
                "--delegate",
                &address,
                "--inner-op",
                "transfer",
            ],
            "real_ss58",
            "--real",
        ),
        (
            vec!["rtao", "proxy", "book", "add", "--name", "saved"],
            "address",
            "--address",
        ),
    ] {
        let command = picker_tree()?;
        let argv = original(&words);
        let (input, trace) = terminal(&["first-contact"], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        let invocation = application::parse_with_context(
            command,
            argv.clone(),
            &mut app,
            PromptHistory::default(),
        )?;
        assert_eq!(
            invocation_leaf(&invocation.matches).get_one::<String>(field),
            Some(&address)
        );
        let tokens = strings(&invocation.argv);
        assert_eq!(
            &tokens[tokens.len() - 2..],
            &[flag.to_owned(), address.clone()]
        );
        assert!(
            invocation
                .history
                .replay_command(&strings(&argv))
                .ends_with(&format!(" {flag} {address}"))
        );
        assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 1);
    }
    Ok(())
}

#[test]
fn canonical_picker_answers_use_real_clap_parser_retries_and_final_relationships() -> TestResult {
    fn only_second(raw: &str) -> Result<String, String> {
        if raw == AccountId([42; 32]).to_string() {
            Ok(raw.into())
        } else {
            Err("this command fixture accepts only the second account".into())
        }
    }
    let root = Root::new()?;
    let command = picker_tree()?.mut_subcommand("wallet", |wallet| {
        wallet.mut_subcommand("verify", |verify| {
            verify.mut_arg("ss58", |arg| arg.value_parser(only_second))
        })
    });
    let argv = original(&[
        "rtao",
        "wallet",
        "verify",
        "--message",
        "payload",
        "--signature",
        "00",
    ]);
    let (input, trace) = terminal(&["1", "2"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let invocation =
        application::parse_with_context(command, argv.clone(), &mut app, PromptHistory::default())?;
    let accepted = AccountId([42; 32]).to_string();
    assert_eq!(
        invocation_leaf(&invocation.matches).get_one::<String>("ss58"),
        Some(&accepted)
    );
    assert_eq!(
        invocation.history.replay_command(&strings(&argv)),
        format!("rtao wallet verify --message payload --signature 00 --ss58 {accepted}")
    );
    assert!(!strings(&invocation.argv).contains(&AccountId([41; 32]).to_string()));
    assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 2);
    let command = picker_tree()?.mut_subcommand("wallet", |wallet| {
        wallet.mut_subcommand("verify", |verify| {
            verify.mut_arg("ss58", |arg| arg.conflicts_with("message"))
        })
    });
    // A supplied conflict waives Clap's required argument, so leave both
    // values missing to exercise collection before final relationship checks.
    let supplied = command.clone().try_get_matches_from(&argv)?;
    assert!(
        invocation_leaf(&supplied)
            .get_one::<String>("ss58")
            .is_none()
    );
    let argv = original(&["rtao", "wallet", "verify", "--signature", "00"]);
    let (input, trace) = terminal(&["payload", "1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let error = application::parse_with_context(command, argv, &mut app, PromptHistory::default())
        .err()
        .ok_or("final relationship rejection")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("cannot be used with"));
    let reads = trace.lock().map_err(|_| "trace")?;
    assert_eq!(reads.len(), 2);
    assert!(reads[0].contains("message"));
    Ok(())
}

#[test]
fn required_tx_context_confirms_all_coldkey_routes_and_preserves_inner_proxy_reservations()
-> TestResult {
    let root = Root::new()?;
    for words in [
        vec!["rtao", "wallet", "swap-coldkey"],
        vec!["rtao", "wallet", "announce-coldkey-swap"],
        vec!["rtao", "proxy", "add"],
    ] {
        let command = picker_tree()?;
        let argv = original(&words);
        let (input, trace) = terminal(&["chosen", "1"], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        application::parse_with_context(command, argv, &mut app, PromptHistory::default())?;
        assert_eq!(app.globals.wallet_name, "chosen");
        let reads = trace.lock().map_err(|_| "trace")?.clone();
        assert_eq!(reads.len(), 2);
        assert!(reads[0].contains("wallet [configured]"));
        assert!(!reads.iter().any(|line| line.contains("PRIVATE")));
    }
    let delegate = AccountId([41; 32]).to_string();
    let real = AccountId([42; 32]).to_string();
    for outer in [None, Some("Any")] {
        let words = vec![
            "rtao",
            "proxy",
            "execute",
            "--delegate",
            &delegate,
            "--real",
            &real,
            "--force-proxy-type",
            "Staking",
        ];
        let argv = original(&words);
        let command = picker_tree()?;
        let (input, trace) = terminal(&["chosen", "transfer"], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        assert_eq!(app.globals.force_proxy_type, None);
        // Root grammar excludes this flag. Model an already chosen outer
        // route, as in the generated-command collision fixture.
        app.globals.force_proxy_type = outer.map(str::to_owned);
        let invocation = application::parse_with_context(
            command,
            argv.clone(),
            &mut app,
            PromptHistory::default(),
        )?;
        assert_eq!(app.globals.force_proxy_type.as_deref(), outer);
        assert_eq!(app.globals.wallet_name, "chosen");
        assert_eq!(
            invocation_leaf(&invocation.matches)
                .get_one::<String>("force_proxy_type")
                .map(String::as_str),
            Some("Staking")
        );
        let reads = trace.lock().map_err(|_| "trace")?.clone();
        assert_eq!(reads.len(), 2);
        assert!(reads[0].contains("wallet [configured]") && reads[1].contains("inner-op"));
        assert!(
            invocation
                .history
                .replay_command(&strings(&argv))
                .ends_with("--wallet chosen --inner-op transfer")
        );
    }
    for extra in [vec!["--yes"], vec!["--signer", "ledger"]] {
        let mut words = vec!["rtao", "proxy", "add"];
        words.extend(extra);
        let argv = original(&words);
        let command = picker_tree()?;
        let (input, trace) = terminal(&["1"], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        application::parse_with_context(command, argv, &mut app, PromptHistory::default())?;
        assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 1);
        assert_eq!(app.globals.wallet_name, "configured");
    }
    let mut books = picker_books();
    books.multisigs = vec![json!({"name":"saved-group","threshold":1,"signatories":[delegate]})];
    let command = picker_tree()?;
    let argv = original(&["rtao", "proxy", "add"]);
    let (input, trace) = terminal(&["saved-group", "1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, books)?;
    application::parse_with_context(command, argv, &mut app, PromptHistory::default())?;
    assert_eq!(app.globals.wallet_name, "saved-group");
    assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 2);
    Ok(())
}

#[test]
fn generic_address_machine_abort_and_excluded_routes_keep_existing_boundaries() -> TestResult {
    for (words, tty, err_tty) in [
        (vec!["rtao", "proxy", "execute", "--json"], true, true),
        (vec!["rtao", "--json", "proxy", "execute"], true, true),
        (vec!["rtao", "wallet", "swap-hotkey", "--quiet"], true, true),
        (vec!["rtao", "wallet", "swap-hotkey"], false, true),
        (vec!["rtao", "wallet", "swap-hotkey"], true, false),
    ] {
        let command = picker_tree()?;
        let argv = original(&words);
        let (input, trace) = terminal(&[], tty, err_tty);
        assert!(
            application::pending_prompt_matches(
                &command,
                &argv,
                &PromptContext::default(),
                &input
            )?
            .is_none()
        );
        let mut parser_input = input;
        let error = application::parse_with_prompts(
            command,
            argv,
            &PromptContext::default(),
            &mut parser_input,
        )
        .err()
        .ok_or("missing-input error")?;
        assert_eq!(error.code, 2);
        assert!(error.message.contains("missing required"));
        if words.contains(&"execute") {
            for field in ["--delegate", "--real", "--inner-op"] {
                assert!(error.message.contains(field));
            }
        }
        assert!(trace.lock().map_err(|_| "trace")?.is_empty());
    }
    let root = Root::new()?;
    for words in [
        vec!["rtao", "addr", "save", "new"],
        vec!["rtao", "wallet", "swap-hotkey"],
    ] {
        let command = picker_tree()?;
        let argv = original(&words);
        let (input, trace) = terminal(&[], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        let error =
            application::parse_with_context(command, argv, &mut app, PromptHistory::default())
                .err()
                .ok_or("abort")?;
        assert_eq!(error.code, 130);
        assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 1);
    }
    let address = AccountId([41; 32]).to_string();
    for words in [
        vec!["rtao", "wallet", "swap-hotkey", "--new-hotkey", &address],
        vec!["rtao", "wallet", "encrypt"],
        vec!["rtao", "wallet", "associate-hotkey"],
        vec!["rtao", "addr", "save", "--unknown"],
    ] {
        let command = picker_tree()?;
        let (input, trace) = terminal(&[], true, true);
        assert!(
            application::pending_prompt_matches(
                &command,
                &original(&words),
                &PromptContext::default(),
                &input
            )?
            .is_none()
        );
        assert!(trace.lock().map_err(|_| "trace")?.is_empty());
    }
    #[cfg(feature = "evm")]
    {
        let command = picker_tree()?;
        let (input, _) = terminal(&[], true, true);
        assert!(
            application::pending_prompt_matches(
                &command,
                &original(&["rtao", "evm", "stake", "add"]),
                &PromptContext::default(),
                &input
            )?
            .is_none()
        );
    }
    Ok(())
}

#[test]
fn generic_transaction_required_fields_confirm_wallet_before_values_and_bind_root_hotkey()
-> TestResult {
    let root = Root::new()?;
    let configured_hotkey = AccountId([21; 32]);
    let chosen_hotkey = AccountId([22; 32]);
    public_hotkey(&root, "configured", configured_hotkey)?;
    public_hotkey(&root, "chosen", chosen_hotkey)?;
    let cases = vec![
        (vec!["rtao", "call"], vec![("target", "System.remark")]),
        (
            vec!["rtao", "root", "swap", "--hotkey", "worker"],
            vec![("--from", "1"), ("--to", "2"), ("--amount", "1")],
        ),
        (
            vec!["rtao", "misc", "weights", "set"],
            vec![("--netuid", "1")],
        ),
        (vec!["rtao", "wt", "commit"], vec![("--netuid", "1")]),
        (
            vec!["rtao", "misc", "weights", "reveal"],
            vec![
                ("--netuid", "1"),
                ("--uids", "0"),
                ("--weights", "1"),
                ("--salt", "1"),
            ],
        ),
        (
            vec!["rtao", "misc", "axon", "set"],
            vec![("--netuid", "1"), ("--ip", "127.0.0.1"), ("--port", "3030")],
        ),
        (vec!["rtao", "axon", "reset"], vec![("--netuid", "1")]),
        (
            vec!["rtao", "misc", "collateral", "add"],
            vec![("--netuid", "1")],
        ),
        (
            vec!["rtao", "collateral", "set-min"],
            vec![("--netuid", "1"), ("--min-alpha", "1")],
        ),
        (vec!["rtao", "stake", "set-auto"], vec![("--netuid", "1")]),
        (
            vec!["rtao", "st", "children", "set"],
            vec![("--netuid", "1"), ("--children", "[]")],
        ),
        (
            vec!["rtao", "stake", "child", "revoke"],
            vec![("--netuid", "1")],
        ),
        (
            vec!["rtao", "stake", "child", "take"],
            vec![("--netuid", "1"), ("--take", "0.1")],
        ),
        (
            vec!["rtao", "hparams", "stake-burn"],
            vec![
                ("--netuid", "1"),
                ("--amount-tao", "1"),
                ("--limit-price", "1"),
            ],
        ),
        (
            vec!["rtao", "hparams", "trim-null"],
            vec![("--netuid", "1")],
        ),
        (
            vec!["rtao", "conviction", "mode", "--perpetual"],
            vec![("--netuid", "1")],
        ),
    ];
    #[cfg(feature = "evm")]
    let cases = {
        let mut cases = cases;
        cases.push((vec!["rtao", "evm", "associate"], vec![("--netuid", "1")]));
        cases
    };
    for (words, fields) in cases {
        let command = rtao_cli::commands::command_tree()?;
        let argv = original(&words);
        let lines = std::iter::once("chosen")
            .chain(fields.iter().map(|(_, answer)| *answer))
            .collect::<Vec<_>>();
        let (input, trace) = terminal(&lines, true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        let mut invocation = application::parse_with_context(
            command,
            argv.clone(),
            &mut app,
            PromptHistory::default(),
        )?;
        assert_eq!(app.globals.wallet_name, "chosen", "{words:?}");
        assert!(app.globals.wallet_given, "{words:?}");
        let reads = trace.lock().map_err(|_| "trace")?.clone();
        assert_eq!(reads.len(), 1 + fields.len(), "{words:?}");
        assert!(reads[0].contains("wallet [configured]"), "{words:?}");
        for (read, (field, _)) in reads.iter().skip(1).zip(&fields) {
            assert!(
                read.contains(field.trim_start_matches('-')),
                "{words:?}: {read}"
            );
        }
        let replay = invocation.history.replay_command(&strings(&argv));
        assert_eq!(replay.matches("--wallet chosen").count(), 1, "{words:?}");
        assert!(!reads.iter().any(|line| line.contains("PRIVATE")));
        if words.get(1) == Some(&"root") {
            let group = invocation.matches.subcommand().ok_or("root group")?.1;
            let rtao_cli::commands::root::Request::Swap {
                hotkey,
                origin,
                dest,
                amount,
                ..
            } = rtao_cli::commands::root::Request::from_matches(
                &mut app,
                group,
                &mut invocation.history,
            )?
            else {
                return Err("actual root swap request".into());
            };
            assert_eq!(hotkey, chosen_hotkey);
            assert_ne!(hotkey, configured_hotkey);
            assert_eq!((origin.0, dest.0), (1, 2));
            assert_eq!(amount, "1");
            assert_eq!(*trace.lock().map_err(|_| "trace")?, reads);
            assert_eq!(
                replay,
                "rtao root swap --hotkey worker --wallet chosen --from 1 --to 2 --amount 1",
            );
        }
    }
    // A raw command's role belongs to that command; the root backend remains
    // authoritative and must not be overwritten by --key hotkey.
    let command = rtao_cli::commands::command_tree()?;
    let argv = original(&["rtao", "--signer", "ledger", "call", "--key", "hotkey"]);
    let (input, trace) = terminal(&["System.remark"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let invocation =
        application::parse_with_context(command, argv, &mut app, PromptHistory::default())?;
    assert_eq!(app.globals.signer_backend.as_deref(), Some("ledger"));
    assert_eq!(
        invocation_leaf(&invocation.matches)
            .get_one::<String>("signer_backend")
            .map(String::as_str),
        Some("hotkey"),
    );
    let reads = trace.lock().map_err(|_| "trace")?;
    assert_eq!(reads.len(), 1);
    assert!(reads[0].contains("target"));
    Ok(())
}

#[cfg(feature = "evm")]
#[tokio::test]
async fn required_evm_pubkey_position_uses_public_picker_and_actual_offline_handler() -> TestResult
{
    let root = Root::new()?;
    let command = rtao_cli::commands::command_tree()?;
    let argv = original(&["rtao", "evm", "pubkey"]);
    let (input, trace) = terminal(&["unknown-contact", "1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let mut invocation =
        application::parse_with_context(command, argv.clone(), &mut app, PromptHistory::default())?;
    let canonical = AccountId([41; 32]).to_string();
    assert_eq!(
        invocation_leaf(&invocation.matches).get_one::<String>("ss58"),
        Some(&canonical)
    );
    assert_eq!(
        strings(&invocation.argv),
        ["rtao", "evm", "pubkey", canonical.as_str()]
    );
    assert_eq!(
        invocation.history.replay_command(&strings(&argv)),
        format!("rtao evm pubkey {canonical}"),
    );
    assert_eq!(app.globals.wallet_name, "configured");
    assert!(!app.globals.wallet_given);
    let reads = trace.lock().map_err(|_| "trace")?.clone();
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|line| line.contains("ss58")));
    let sink = MemorySink::default();
    app.output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    let group = invocation.matches.subcommand().ok_or("evm group")?.1;
    assert!(!rtao_cli::commands::evm::needs_native(group));
    rtao_cli::commands::evm::run(
        &mut app,
        None,
        group,
        Default::default(),
        None,
        &mut invocation.history,
    )
    .await?;
    let payload: Value = serde_json::from_str(&sink.text(rtao_cli::output::Stream::Stdout)?)?;
    assert_eq!(
        payload,
        json!({"ss58":canonical,"pubkey":format!("0x{}",hex::encode([41u8;32]))})
    );
    assert_eq!(*trace.lock().map_err(|_| "trace")?, reads);
    Ok(())
}

#[test]
fn generic_required_typed_inputs_show_shape_retry_and_preserve_exact_public_replay() -> TestResult {
    let command = application::command_tree([clap::Command::new("typed")
        .arg(
            clap::Arg::new("count")
                .long("count")
                .required(true)
                .value_parser(clap::value_parser!(u16).range(1..=5)),
        )
        .arg(
            clap::Arg::new("ratio")
                .long("ratio")
                .required(true)
                .value_parser(clap::value_parser!(f64)),
        )
        .arg(
            clap::Arg::new("enabled")
                .long("enabled")
                .required(true)
                .value_parser(clap::value_parser!(bool)),
        )
        .arg(
            clap::Arg::new("mode")
                .long("mode")
                .required(true)
                .value_parser(["safe", "fast"]),
        )]);
    let argv = original(&["rtao", "typed"]);
    let (mut input, trace) = terminal(
        &["6", "3", "bad", "0.25", "true", "bad", "safe"],
        true,
        true,
    );
    let invocation = application::parse_with_prompts(
        command,
        argv.clone(),
        &PromptContext::default(),
        &mut input,
    )?;
    let leaf = invocation_leaf(&invocation.matches);
    assert_eq!(leaf.get_one::<u16>("count"), Some(&3));
    assert_eq!(leaf.get_one::<f64>("ratio"), Some(&0.25));
    assert_eq!(leaf.get_one::<bool>("enabled"), Some(&true));
    assert_eq!(
        leaf.get_one::<String>("mode").map(String::as_str),
        Some("safe")
    );
    let reads = trace.lock().map_err(|_| "trace")?;
    assert_eq!(reads.len(), 7);
    for (read, cue) in reads.iter().zip([
        "integer",
        "integer",
        "number",
        "number",
        "y/n",
        "safe|fast",
        "safe|fast",
    ]) {
        assert!(read.contains(cue), "{read}");
    }
    assert_eq!(
        invocation.history.replay_command(&strings(&argv)),
        "rtao typed --count 3 --ratio 0.25 --enabled true --mode safe",
    );
    Ok(())
}

#[test]
fn required_conviction_boolean_choices_preserve_signer_order_real_intents_and_canonical_flags()
-> TestResult {
    let root = Root::new()?;
    for (operation, positive, negative, op, enabled_when_yes) in [
        ("mode", "perpetual", "decaying", "set_perpetual_lock", true),
        (
            "accept",
            "allow",
            "reject",
            "set_reject_locked_alpha",
            false,
        ),
    ] {
        for (answer, yes) in [
            ("y", true),
            ("YES", true),
            ("t", true),
            ("true", true),
            ("on", true),
            ("1", true),
            ("n", false),
            ("NO", false),
            ("f", false),
            ("false", false),
            ("off", false),
            ("0", false),
        ] {
            let command = rtao_cli::commands::command_tree()?;
            let argv = original(&["rtao", "conviction", operation]);
            let mut lines = vec!["chosen"];
            if operation == "mode" {
                lines.push("7");
            }
            lines.extend(["maybe", answer]);
            let (input, trace) = terminal(&lines, true, true);
            let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
            let mut invocation = application::parse_with_context(
                command,
                argv.clone(),
                &mut app,
                PromptHistory::default(),
            )?;
            let flag = format!("--{}", if yes { positive } else { negative });
            let opposite = format!("--{}", if yes { negative } else { positive });
            let tokens = strings(&invocation.argv);
            assert_eq!(tokens.last(), Some(&flag));
            assert!(!tokens.contains(&opposite));
            assert!(!tokens.contains(&answer.to_owned()) && !tokens.contains(&"maybe".to_owned()));
            let group = invocation.matches.subcommand().ok_or("conviction group")?.1;
            let requests = rtao_cli::commands::conviction::tx_requests(
                &mut app,
                group,
                &mut invocation.history,
            )?;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].op, op);
            assert_eq!(
                requests[0].args["enabled"],
                json!(if yes {
                    enabled_when_yes
                } else {
                    !enabled_when_yes
                })
            );
            if operation == "mode" {
                assert_eq!(requests[0].args["netuid"], 7);
            }
            let reads = trace.lock().map_err(|_| "trace")?;
            assert_eq!(reads.len(), if operation == "mode" { 4 } else { 3 });
            assert!(reads[0].contains("wallet [configured]"));
            assert!(reads.last().ok_or("boolean prompt")?.contains("y/n"));
            assert_eq!(
                invocation.history.replay_command(&strings(&argv)),
                format!(
                    "rtao conviction {operation} --wallet chosen{} {flag}",
                    if operation == "mode" {
                        " --netuid 7"
                    } else {
                        ""
                    }
                )
            );
        }
        // A supplied boolean half is never asked again or rewritten.
        let command = rtao_cli::commands::command_tree()?;
        let positive_flag = format!("--{positive}");
        let mut words = vec!["rtao", "conviction", operation, positive_flag.as_str()];
        if operation == "mode" {
            words.extend(["--netuid", "7"]);
        }
        let argv = original(&words);
        let (mut input, trace) = terminal(&[], true, true);
        let invocation = application::parse_with_prompts(
            command,
            argv.clone(),
            &PromptContext::default(),
            &mut input,
        )?;
        assert_eq!(strings(&invocation.argv), strings(&argv));
        assert!(trace.lock().map_err(|_| "trace")?.is_empty());
        for flag in ["--json", "--quiet"] {
            let command = rtao_cli::commands::command_tree()?;
            let argv = original(&["rtao", "conviction", operation, flag]);
            let (mut input, trace) = terminal(&[], true, true);
            let error = application::parse_with_prompts(
                command,
                argv,
                &PromptContext::default(),
                &mut input,
            )
            .err()
            .ok_or("missing bool")?;
            assert_eq!(error.code, 2);
            assert!(error.message.contains(&positive_flag));
            if operation == "mode" {
                assert!(error.message.contains("--netuid"));
            }
            assert_eq!(
                error.help.as_deref(),
                Some("pass the options explicitly, or run on a terminal to be prompted")
            );
            assert!(trace.lock().map_err(|_| "trace")?.is_empty());
        }
        let command = rtao_cli::commands::command_tree()?;
        let argv = original(&["rtao", "conviction", operation]);
        let (input, trace) = terminal(&["chosen"], true, true);
        let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
        let error =
            application::parse_with_context(command, argv, &mut app, PromptHistory::default())
                .err()
                .ok_or("aborted choice")?;
        assert_eq!(error.code, 130);
        assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 2);
    }
    Ok(())
}
