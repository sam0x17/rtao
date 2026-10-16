//! Actual required-argument parsing confirms local signing wallets first.

use rtao::primitives::AccountId;
use rtao_cli::{
    application,
    books::Books,
    commands::{addresses, generated::TxRequest, misc, proxy, wallet},
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
    let (input, trace) = terminal(&["1"], true, true);
    let mut app = pending_context(&root, &command, &argv, input, picker_books())?;
    let error = application::parse_with_context(command, argv, &mut app, PromptHistory::default())
        .err()
        .ok_or("final relationship rejection")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("cannot be used with"));
    assert_eq!(trace.lock().map_err(|_| "trace")?.len(), 1);
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
                &original(&["rtao", "evm", "pubkey"]),
                &PromptContext::default(),
                &input
            )?
            .is_none()
        );
    }
    Ok(())
}
