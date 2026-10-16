//! Actual required-argument parsing confirms local signing wallets first.

use rtao::primitives::AccountId;
use rtao_cli::{
    application,
    books::Books,
    commands::{misc, wallet},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions},
    prompt::{PromptContext, PromptError, PromptHistory, SharedTerminal, Terminal},
};
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Trace = Arc<Mutex<Vec<String>>>;
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
    let mut globals = Globals::resolve(parsed, &config, Path::new("/fixture/no-user-home"), &env)?;
    let mut leaf = parsed;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    globals.apply_with_env(leaf, &[], &env)?;
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
