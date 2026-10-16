// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/globals.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Global options through the installed parser and public request/policy consumers.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use clap::{ArgMatches, Command, error::ErrorKind};
use rtao::{
    primitives::{AccountId, CryptoType},
    signer::{BoxFuture, MultiSignature, Signer, SignerError, SignerRole, SigningRequest},
};
use rtao_cli::{
    application,
    books::Books,
    call::{RawCallRequest, RawSignerFactory},
    commands::{self, generated, schema},
    context::{AppContext, ExternalSignerFactory},
    error::CliError,
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions},
    prompt::{PromptContext, PromptError, Terminal},
    signer_backends::{BackendRequest, BackendSession, NativeSignerFactory, SigningBackend},
};
use serde_json::{Map, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct UnusedTerminal(Arc<AtomicUsize>);
impl Terminal for UnusedTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

fn leaf(mut matches: &ArgMatches) -> &ArgMatches {
    while let Some((_, child)) = matches.subcommand() {
        matches = child;
    }
    matches
}

fn context(
    parsed: &ArgMatches,
    reserved: &[&str],
    reads: &Arc<AtomicUsize>,
) -> Result<AppContext, Box<dyn std::error::Error>> {
    let config = Map::new();
    let mut globals = Globals::resolve(
        parsed,
        &config,
        Path::new("/isolated/globals-contract-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf(parsed), reserved, |_| None)?;
    let mut app = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(UnusedTerminal(reads.clone())))),
    );
    app.output = Output::new(OutputOptions::default(), Arc::new(MemorySink::default()));
    Ok(app)
}

#[test]
fn every_global_proxy_choice_reaches_the_typed_raw_request() -> TestResult {
    let command = commands::command_tree()?;
    let proxy = AccountId([31; 32]);
    let proxy_address = proxy.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    for choice in rtao_intents::proxy::PROXY_TYPES {
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "call",
            "System.remark",
            "--args",
            r#"{"remark":"0x00"}"#,
            "--proxy-for",
            &proxy_address,
            "--force-proxy-type",
            choice,
            "--key",
            "hotkey",
        ])?;
        let app = context(&parsed, &["signer_backend"], &reads)?;
        let request = RawCallRequest::from_matches(&app, leaf(&parsed))?;
        assert_eq!(request.proxy_for, Some(proxy));
        assert_eq!(
            serde_json::to_value(request.force_proxy_type)?,
            json!(choice)
        );
        assert_eq!(
            request.params,
            json!({"remark":"0x00"}).as_object().ok_or("args")?.clone()
        );
        assert_eq!(request.role, SignerRole::Hotkey);
        assert_eq!(request.backend, None);
        assert_eq!(app.globals.signer_backend, None);
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn invalid_global_proxy_choices_refuse_before_missing_argument_prompts() -> TestResult {
    let command = commands::command_tree()?;
    let reads = Arc::new(AtomicUsize::new(0));
    let mut terminal = UnusedTerminal(reads.clone());
    for invalid in ["", "any", "Any ", "Sudo", "not-a-proxy-type"] {
        for route in [&["call"][..], &["tx", "transfer"][..]] {
            let argv = std::iter::once("rtao")
                .chain(route.iter().copied())
                .chain(["--force-proxy-type", invalid])
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>();
            let error = command
                .clone()
                .try_get_matches_from(&argv)
                .err()
                .ok_or("invalid proxy accepted")?;
            assert_eq!(error.kind(), ErrorKind::InvalidValue);
            let error = application::parse_with_prompts(
                command.clone(),
                argv,
                &PromptContext::default(),
                &mut terminal,
            )
            .err()
            .ok_or("invalid proxy reached prompt round")?;
            assert_eq!(error.code, 2);
            assert!(error.message.contains("--force-proxy-type"));
        }
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn global_proxy_help_and_completion_choices_match_the_live_proxy_catalog() -> TestResult {
    let mut command = commands::command_tree()?;
    command.build();
    let call = command.find_subcommand_mut("call").ok_or("call command")?;
    let help = call.render_long_help().to_string();
    let arg = call
        .get_arguments()
        .find(|arg| arg.get_id() == "force_proxy_type")
        .ok_or("global proxy argument")?;
    let choices = arg
        .get_value_parser()
        .possible_values()
        .ok_or("proxy completions")?
        .map(|choice| choice.get_name().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(choices, rtao_intents::proxy::PROXY_TYPES);
    for choice in rtao_intents::proxy::PROXY_TYPES {
        assert!(help.contains(choice), "help omitted {choice}");
    }
    Ok(())
}

#[test]
fn command_owned_proxy_fields_keep_their_schema_and_stay_out_of_globals() -> TestResult {
    let parsed = commands::command_tree()?.try_get_matches_from([
        "rtao",
        "tx",
        "execute-proxy-announced",
        "--force-proxy-type",
        "command-owned-value",
    ])?;
    let spec = rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == "execute_proxy_announced")
        .ok_or("proxy intent")?;
    let fields = schema::intent_fields(spec)?;
    let reads = Arc::new(AtomicUsize::new(0));
    let mut app = context(&parsed, &generated::intent_reserved(spec.name)?, &reads)?;
    assert_eq!(app.globals.force_proxy_type, None);
    app.globals.force_proxy_type = Some("Any".into());
    app.globals.apply_with_env(
        leaf(&parsed),
        &generated::intent_reserved(spec.name)?,
        |_| None,
    )?;
    assert_eq!(app.globals.force_proxy_type.as_deref(), Some("Any"));
    assert_eq!(
        schema::values(leaf(&parsed), &fields)?["force_proxy_type"],
        json!("command-owned-value")
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn shield_flag_order_at_each_scope_controls_the_real_shield_policy() -> TestResult {
    let command = commands::command_tree()?;
    let reads = Arc::new(AtomicUsize::new(0));
    for (root, local, expected) in [
        (vec!["--mev-shield", "--no-mev-shield"], vec![], false),
        (vec!["--no-mev-shield", "--mev-shield"], vec![], true),
        (vec![], vec!["--mev-shield", "--no-mev-shield"], false),
        (vec![], vec!["--no-mev-shield", "--mev-shield"], true),
        (vec!["--mev-shield"], vec!["--no-mev-shield"], false),
        (vec!["--no-mev-shield"], vec!["--mev-shield"], true),
        (vec![], vec!["--mev-shield", "--mev-shield"], true),
        (vec![], vec!["--no-mev-shield", "--no-mev-shield"], false),
        (
            vec!["--mev-shield", "--no-mev-shield"],
            vec!["--mev-shield", "--no-mev-shield", "--mev-shield"],
            true,
        ),
    ] {
        let argv = std::iter::once("rtao")
            .chain(root)
            .chain(["call", "System.remark"])
            .chain(local);
        let parsed = command.clone().try_get_matches_from(argv)?;
        let app = context(&parsed, &["signer_backend"], &reads)?;
        assert_eq!(app.globals.mev_shield, Some(expected));
        let choice = app.resolve_mev_shield(!expected, false, "fixture")?;
        assert_eq!(choice.enabled, expected);
        assert_eq!(choice.forced, expected);
        let required = app.resolve_mev_shield(false, true, "fixture");
        if expected {
            assert!(required?.enabled);
        } else {
            assert_eq!(required.err().ok_or("required shield bypassed")?.code, 2);
        }
    }
    let parsed = command.try_get_matches_from(["rtao", "call", "System.remark"])?;
    let app = context(&parsed, &["signer_backend"], &reads)?;
    assert_eq!(app.globals.mev_shield, None);
    assert!(app.resolve_mev_shield(true, false, "fixture")?.enabled);
    assert!(!app.resolve_mev_shield(false, false, "fixture")?.enabled);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn tiered_options_override_only_supported_settings_and_inherit_the_root() -> TestResult {
    let command = application::command_tree([
        with_globals(Command::new("read"), Tier::Read),
        with_globals(Command::new("unlock"), Tier::Unlock),
        with_globals(Command::new("bridge"), Tier::Extension),
        with_globals(Command::new("submit"), Tier::Tx),
    ]);
    let env = |name: &str| match name {
        "BT_SIGNER_ADDRESS" => Some("env-account".into()),
        "BT_EXTENSION_BROWSER" => Some("env-browser".into()),
        "BT_EXTENSION_BRIDGE" => Some("ws://env-bridge".into()),
        _ => None,
    };
    for (tier, unlock, bridge, tx) in [
        ("read", false, false, false),
        ("unlock", true, false, false),
        ("bridge", false, true, false),
        ("submit", true, true, true),
    ] {
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "--network",
            "test",
            "--wallet",
            "root-owner",
            "--signer-address",
            "root-account",
            "--extension-browser",
            "root-browser",
            "--extension-bridge",
            "ws://root-bridge",
            "--yes",
            "--dry-run",
            "--no-mev-shield",
            tier,
            "--network",
            "local",
            "--wallet-hotkey",
            "leaf-hotkey",
        ])?;
        let mut globals =
            Globals::resolve(&parsed, &Map::new(), Path::new("/isolated/tier-home"), env)?;
        globals.apply_with_env(leaf(&parsed), &[], env)?;
        assert_eq!(globals.network, "local");
        assert_eq!(globals.wallet_name, "root-owner");
        assert_eq!(globals.hotkey_name, "leaf-hotkey");
        assert!(globals.wallet_given && globals.hotkey_given);
        assert!(globals.assume_yes && globals.dry_run);
        assert_eq!(globals.mev_shield, Some(false));
        assert_eq!(
            globals.signer_address.as_deref(),
            Some(if tx { "env-account" } else { "root-account" })
        );
        assert_eq!(
            globals.extension_browser.as_deref(),
            Some(if bridge {
                "env-browser"
            } else {
                "root-browser"
            })
        );
        assert_eq!(
            globals.extension_bridge_url.as_deref(),
            Some(if bridge {
                "ws://env-bridge"
            } else {
                "ws://root-bridge"
            })
        );
        for (flag, value, allowed) in [
            (
                "--wallet-password-file",
                Some("/fixture/password.txt"),
                unlock,
            ),
            ("--macos-password", None, unlock),
            ("--keychain-password", None, unlock),
            ("--extension-browser", Some("browser"), bridge),
            ("--extension-bridge", Some("ws://bridge"), bridge),
            ("--yes", None, tx),
            ("--no-prompt", None, tx),
            ("--dry-run", None, tx),
            ("--mev-shield", None, tx),
            ("--no-mev-shield", None, tx),
            ("--proxy-for", Some("self"), tx),
            ("--force-proxy-type", Some("Any"), tx),
            ("--signatory", Some("member"), tx),
            ("--signer", Some("wallet"), tx),
            ("--ledger", None, tx),
            ("--ledger-account", Some("0"), tx),
            ("--ledger-index", Some("0"), tx),
            ("--signer-address", Some("account"), tx),
            ("--extension-source", Some("polkadot-js"), tx),
        ] {
            let mut argv = vec!["rtao", tier, flag];
            argv.extend(value);
            let parsed = command.clone().try_get_matches_from(argv);
            assert_eq!(parsed.is_ok(), allowed, "{tier} {flag}");
            if !allowed {
                assert_eq!(
                    parsed.err().ok_or("unsupported flag accepted")?.kind(),
                    ErrorKind::UnknownArgument
                );
            }
        }
    }
    Ok(())
}

type BackendTrace = Arc<Mutex<Vec<String>>>;

struct FixtureBackend {
    account: AccountId,
    trace: BackendTrace,
}
struct FixtureSession {
    account: AccountId,
    trace: BackendTrace,
}
struct FixtureSigner(AccountId);
impl Signer for FixtureSigner {
    fn account(&self) -> AccountId {
        self.0
    }
    fn crypto_type(&self) -> CryptoType {
        CryptoType::Ed25519
    }
    fn sign<'a>(
        &'a self,
        _: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async { Err(SignerError::KeyView("fixture must not sign".into())) })
    }
}
impl BackendSession for FixtureSession {
    fn signer(&self) -> Arc<dyn Signer> {
        Arc::new(FixtureSigner(self.account))
    }
    fn release(&self, success: bool) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async move {
            self.trace
                .lock()
                .map_err(|_| CliError::failure("trace"))?
                .push(format!("release:{success}"));
            Ok(())
        })
    }
}
impl SigningBackend for FixtureBackend {
    fn validate(&self, request: &BackendRequest) -> Result<(), CliError> {
        assert_eq!(request.expected, Some(self.account));
        self.trace
            .lock()
            .map_err(|_| CliError::failure("trace"))?
            .push(format!(
                "validate:{}",
                request.globals.signer_backend.as_deref().unwrap_or("")
            ));
        Ok(())
    }
    fn discover<'a>(&'a self, _: &'a BackendRequest) -> BoxFuture<'a, Result<AccountId, CliError>> {
        Box::pin(async { Err(CliError::failure("pinned account must not use discovery")) })
    }
    fn acquire<'a>(
        &'a self,
        request: &'a BackendRequest,
        reviewed: AccountId,
    ) -> BoxFuture<'a, Result<Arc<dyn BackendSession>, CliError>> {
        Box::pin(async move {
            assert_eq!(reviewed, self.account);
            assert_eq!(request.expected, Some(self.account));
            self.trace
                .lock()
                .map_err(|_| CliError::failure("trace"))?
                .push(format!(
                    "acquire:{}",
                    request.globals.signer_backend.as_deref().unwrap_or("")
                ));
            Ok(Arc::new(FixtureSession {
                account: self.account,
                trace: self.trace.clone(),
            }) as Arc<dyn BackendSession>)
        })
    }
}

#[tokio::test]
async fn python_control_whitespace_signers_reach_actual_lazy_backend_consumers() -> TestResult {
    let command = commands::command_tree()?;
    let account = AccountId([41; 32]);
    let address = account.to_string();
    let reads = Arc::new(AtomicUsize::new(0));
    for backend in ["wallet", "extension", "ledger", "vault"] {
        let raw = format!("\u{1c}\u{1d}{}\u{1e}\u{1f}", backend.to_ascii_uppercase());
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "tx",
            "transfer",
            "--signer",
            &raw,
            "--signer-address",
            &address,
        ])?;
        let app = context(&parsed, &generated::intent_reserved("transfer")?, &reads)?;
        assert_eq!(app.globals.signer_backend.as_deref(), Some(raw.as_str()));
        assert_eq!(app.globals.uses_external_signer(), backend != "wallet");
        assert_eq!(app.prompt_context().external_signer, backend != "wallet");
        assert_eq!(
            app.prompt_context().extension_signer,
            backend == "extension"
        );
        let trace: BackendTrace = Arc::new(Mutex::new(Vec::new()));
        let boundary = Arc::new(FixtureBackend {
            account,
            trace: trace.clone(),
        });
        let factory = NativeSignerFactory::with_backend(&app, boundary.clone());
        let keys = app
            .signing_keys(SignerRole::Coldkey, Some(&factory))
            .await?;
        if backend != "wallet" {
            assert_eq!(keys.bare_signer_account(), Some(account));
            assert_eq!(
                *trace.lock().map_err(|_| "trace")?,
                [format!("validate:{backend}")]
            );
            assert_eq!(
                keys.signer_for(SignerRole::Coldkey).await?.account(),
                account
            );
            ExternalSignerFactory::release(&factory, &app.globals, keys.as_ref(), false).await?;
        }
        assert_eq!(app.globals.signer_backend.as_deref(), Some(raw.as_str()));

        // Raw call's leaf --signer retains exact role/backend choices. Its
        // inherited root setting follows the same Python backend normalization.
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "--signer",
            &raw,
            "call",
            "System.remark",
            "--signer-address",
            &address,
        ])?;
        let app = context(&parsed, &["signer_backend"], &reads)?;
        let request = RawCallRequest::from_matches(&app, leaf(&parsed))?;
        assert_eq!(request.rounds.len(), 1);
        let selection = &request.rounds[0];
        assert_eq!(selection.backend, raw);
        assert_eq!(selection.address, Some(account));
        let factory = NativeSignerFactory::with_backend(&app, boundary);
        let keys = RawSignerFactory::source(&factory, &app, selection).await?;
        if backend != "wallet" {
            assert_eq!(keys.bare_signer_account(), Some(account));
            assert_eq!(
                keys.signer_for(SignerRole::Coldkey).await?.account(),
                account
            );
            RawSignerFactory::release(&factory, selection, keys.as_ref(), false).await?;
        }
        assert_eq!(app.globals.signer_backend.as_deref(), Some(raw.as_str()));
        let expected = if backend == "wallet" {
            Vec::new()
        } else {
            vec![
                format!("validate:{backend}"),
                format!("acquire:{backend}"),
                "release:false".into(),
                format!("validate:{backend}"),
                format!("acquire:{backend}"),
                "release:false".into(),
            ]
        };
        assert_eq!(*trace.lock().map_err(|_| "trace")?, expected);
        let error = command
            .clone()
            .try_get_matches_from(["rtao", "call", "System.remark", "--signer", &raw])
            .err()
            .ok_or("raw owned selector was normalized")?;
        assert_eq!(error.kind(), ErrorKind::InvalidValue);
    }
    for raw in ["\u{1c}unknown\u{1f}", "\u{1c}\u{1d}\u{1e}\u{1f}"] {
        let parsed = command
            .clone()
            .try_get_matches_from(["rtao", "tx", "transfer", "--signer", raw])?;
        let error = context(&parsed, &generated::intent_reserved("transfer")?, &reads)
            .err()
            .ok_or("invalid normalized selector accepted")?;
        assert!(error.to_string().contains("unknown signing backend"));
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    Ok(())
}
