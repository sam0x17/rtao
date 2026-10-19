// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/prompt.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/main.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The rtao command line application.

#![deny(missing_docs)]

mod doctor;

#[cfg(feature = "evm")]
use rtao_cli::commands::evm;
#[cfg(feature = "extension")]
use rtao_cli::commands::extension;
use rtao_cli::{
    application,
    commands::{
        addresses, axon, catalog, collateral, config, conviction, crowd, generated, hparams, misc,
        multisig, proxy, root, stake, subnets, timelock, upgrade, wallet, weights,
    },
    context::AppContext,
    error::CliError,
    output::{DiagnosticContext, Output, OutputOptions, terminal_text},
    prompt::{CancellableTerminal, PromptContext, SharedTerminal, TerminalCancel},
};
use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
};

// Retain the original offline doctor's accepted argv and result text. Other
// command families extend the builder/dispatcher, without replacing its checks.
fn run_doctor(mut args: impl Iterator<Item = OsString>) -> Result<(), CliError> {
    let manifest = match args.next() {
        None => doctor::find_manifest(
            &env::current_dir().map_err(|e| CliError::failure(e.to_string()))?,
        )
        .map_err(CliError::failure)?,
        Some(option) if option == "--manifest-path" => {
            let path =
                PathBuf::from(args.next().ok_or_else(|| {
                    CliError::failure("--manifest-path requires a Cargo.toml path")
                })?);
            if args.next().is_some() {
                return Err(CliError::failure(
                    "usage: rtao doctor [--manifest-path <Cargo.toml>]",
                ));
            }
            path
        }
        Some(_) => {
            return Err(CliError::failure(
                "usage: rtao doctor [--manifest-path <Cargo.toml>]",
            ));
        }
    };
    let check = doctor::check(&manifest).map_err(CliError::failure)?;
    if let Some(warning) = check.warning {
        eprintln!("rtao: warning: {}", terminal_text(&warning));
    }
    println!(
        "Core pin and downstream BLS patch verified for {}",
        terminal_text(&check.manifest.display().to_string())
    );
    Ok(())
}

fn fast_path_parse_error(error: clap::Error) -> CliError {
    CliError {
        code: if error.use_stderr() { 2 } else { 0 },
        ..CliError::failure(error.to_string())
    }
}

// Main owns process signals. Library contexts and tests only receive a latch.
// Tokio does not restore the process default SIGINT handler when this is dropped.
struct InterruptWatcher(tokio::task::JoinHandle<()>);
impl InterruptWatcher {
    fn spawn(future: impl std::future::Future<Output = ()> + Send + 'static) -> Self {
        Self(tokio::spawn(future))
    }
    #[cfg(unix)]
    fn register(cancel: TerminalCancel) -> Result<Self, CliError> {
        // Register synchronously, before any prompt can block this worker.
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .map_err(|_| CliError::failure("could not register interrupt handling"))?;
        Ok(Self::spawn(async move {
            if signal.recv().await.is_some() {
                // Never acquire a terminal/output lock before publishing.
                cancel.cancel();
            }
        }))
    }
    #[cfg(not(unix))]
    fn register(_: TerminalCancel) -> Result<Self, CliError> {
        Err(CliError::failure(
            "invocation interrupt handling requires Unix",
        ))
    }
}
impl Drop for InterruptWatcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run(cancel: TerminalCancel, final_output: &mut Output) -> Result<(), CliError> {
    let mut argv = env::args_os().collect::<Vec<_>>();
    #[allow(deprecated)]
    if let Some(home) = env::home_dir() {
        let paths = rtao::client::config::Paths::new(&home, |key| env::var_os(key));
        application::warn_legacy_config(
            &home,
            paths.path(rtao::client::config::ConfigFile::Config),
            &mut std::io::stderr().lock(),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
    }
    application::normalize_legacy_address_argv(&mut argv);
    if argv.get(1).is_some_and(|arg| arg == "doctor") {
        // Use the installed help definition without loading configuration or
        // running doctor. All other argv retains the original offline parser.
        if argv
            .iter()
            .skip(2)
            .any(|arg| arg == "--help" || arg == "-h")
            && let Err(error) = rtao_cli::commands::command_tree()?.try_get_matches_from(&argv)
            && !error.use_stderr()
        {
            return Err(fast_path_parse_error(error));
        }
        return run_doctor(argv.into_iter().skip(2));
    }
    if argv
        .get(1)
        .is_some_and(|arg| arg == "--version" || arg == "-V")
    {
        println!(
            "rtao {} (bittensor-core {})",
            env!("CARGO_PKG_VERSION"),
            rtao::core::BITTENSOR_CORE_TAG
        );
        return Ok(());
    }
    let mut command = rtao_cli::commands::command_tree()?;
    if argv.len() == 1 || argv.get(1).is_some_and(|arg| arg == "help") {
        print!("{}", command.render_long_help());
        return Ok(());
    }
    if argv.get(1).is_some_and(|arg| arg == "completions") {
        let matches = command
            .clone()
            .try_get_matches_from(&argv)
            .map_err(fast_path_parse_error)?;
        let (_, leaf) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing completion command"))?;
        return rtao_cli::completions::run(
            &command,
            leaf,
            &rtao_cli::output::Output::new(
                Default::default(),
                Arc::new(rtao_cli::output::ProcessSink),
            ),
        );
    }
    let _interrupt_watcher = InterruptWatcher::register(cancel.clone())?;
    let paths = rtao::client::config::Paths::from_process()
        .map_err(|e| CliError::failure(e.to_string()))?;
    let store = rtao::client::config::Store::new(paths);
    let config = store
        .load()
        .await
        .map_err(|e| CliError::failure(e.to_string()))?;
    let context = PromptContext {
        json: config.get("json").is_some_and(truthy),
        quiet: config.get("quiet").is_some_and(truthy),
        ..Default::default()
    };
    *final_output = Output::process(OutputOptions {
        json_mode: context.json || argv.iter().any(|arg| arg == "--json"),
        quiet: context.quiet || argv.iter().any(|arg| arg == "--quiet" || arg == "-q"),
        network: None,
    });
    let terminal: SharedTerminal = Arc::new(Mutex::new(Box::new(CancellableTerminal::new(
        cancel.clone(),
    ))));
    let mut prompting = terminal.clone();
    let completion_tree = command.clone();
    let replay_args = argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let prompt_history = rtao_cli::prompt::PromptHistory::default();
    let mut prepared_context = None;
    if let Some(partial) =
        application::pending_prompt_matches(&command, &argv, &context, &prompting)?
    {
        let mut leaf = &partial;
        while let Some((_, child)) = leaf.subcommand() {
            leaf = child;
        }
        #[allow(deprecated)]
        let prompt_home =
            env::home_dir().ok_or_else(|| CliError::failure("home directory is unavailable"))?;
        // Required public addresses and local-key commands need the same
        // resolved options/books as their handlers, without acquiring a signer.
        let reserved = application::pending_prompt_reserved(&partial)?;
        let app = AppContext::from_store(
            &partial,
            leaf,
            &reserved,
            &store,
            &prompt_home,
            |name| env::var(name).ok(),
            terminal.clone(),
        )
        .await?;
        prepared_context = Some(app.with_interruption(cancel.clone()));
    }
    let parsed_invocation = match prepared_context.as_mut() {
        Some(app) => application::parse_with_context(command, argv, app, prompt_history),
        None => application::parse_with_prompt_history(
            command,
            argv,
            &context,
            &mut prompting,
            prompt_history,
        ),
    };
    let mut invocation = match parsed_invocation {
        Ok(invocation) => invocation,
        Err(mut error) => {
            if error.help.is_some() {
                // Missing-input acquisition owns a diagnostic, including JSON
                // before any AppContext exists. Other clap errors keep their
                // original rendering and exit status.
                let output = rtao_cli::output::Output::new(
                    rtao_cli::output::OutputOptions {
                        json_mode: context.json || replay_args.iter().any(|arg| arg == "--json"),
                        quiet: context.quiet
                            || replay_args
                                .iter()
                                .any(|arg| arg == "--quiet" || arg == "-q"),
                        network: None,
                    },
                    Arc::new(rtao_cli::output::ProcessSink),
                );
                output
                    .error(
                        &error.message,
                        DiagnosticContext {
                            help: error.help.as_deref(),
                            ..Default::default()
                        },
                    )
                    .map_err(|failure| CliError::failure(failure.to_string()))?;
                error.message.clear();
            }
            return Err(error);
        }
    };
    let command_path = application::command_path(&completion_tree, &invocation.matches);
    let Some((name, matches)) = invocation.matches.subcommand() else {
        return Err(CliError::input("missing command"));
    };
    let (name, matches) = if matches!(name, "misc" | "utils") {
        matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing misc command"))?
    } else {
        (name, matches)
    };
    let name = if command_path.first().is_some_and(|part| part == "misc") {
        command_path.get(1).map(String::as_str).unwrap_or(name)
    } else {
        name
    };
    if name == "doctor" {
        // Builder allows root globals before the offline doctor.
        let mut args = Vec::new();
        if let Some(path) = matches.get_one::<String>("manifest_path") {
            args.push(OsString::from("--manifest-path"));
            args.push(OsString::from(path));
        }
        return run_doctor(args.into_iter());
    }
    #[allow(deprecated)]
    let home = env::home_dir().ok_or_else(|| CliError::failure("home directory is unavailable"))?;
    // Nested families attach their global overrides to the selected leaf.
    // Keep the group's matches for dispatch while resolving that leaf's flags.
    let mut leaf = matches;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let reserved = match name {
        "call" => vec!["signer_backend"],
        "stake" => stake::reserved(matches)?,
        "addr" => addresses::reserved(matches),
        "crowd" => crowd::reserved(matches)?,
        "upgrade" => upgrade::reserved(matches)?,
        "convert" | "latency" => misc::reserved(name),
        "proxy" => proxy::reserved(matches)?,
        "multisig" => multisig::reserved(matches)?,
        "tx" => generated::intent_reserved(
            &matches
                .subcommand()
                .ok_or_else(|| CliError::input("missing transaction"))?
                .0
                .replace('-', "_"),
        )?,
        "query" => generated::read_reserved(
            matches
                .subcommand()
                .ok_or_else(|| CliError::input("missing query"))?
                .0
                .replace('-', "_")
                .parse()
                .map_err(|error| CliError::input(format!("{error}")))?,
        ),
        "hparams" => hparams::reserved(matches)?,
        "subnets" => subnets::reserved(matches)?,
        "root" => root::reserved(matches)?,
        "conviction" => conviction::reserved(matches)?,
        "collateral" => collateral::reserved(matches)?,
        #[cfg(feature = "evm")]
        "evm" => evm::reserved(matches)?,
        _ => Vec::new(),
    };
    let mut app = match prepared_context {
        Some(app) => app,
        None => AppContext::from_store(
            &invocation.matches,
            leaf,
            &reserved,
            &store,
            &home,
            |name| env::var(name).ok(),
            terminal,
        )
        .await?
        .with_interruption(cancel.clone()),
    };
    *final_output = app.output.clone();
    if cancel.is_cancelled() {
        return Err(CliError::aborted());
    }
    rtao_cli::logs::setup_logging(app.globals.verbosity, app.globals.quiet)
        .map_err(|e| CliError::failure(e.to_string()))?;
    let replay = invocation.history.replay_command(&replay_args);
    let result = Box::pin(async {
        // Async handlers borrow argv; final success guidance still needs it.
        let replay_args = replay_args.as_slice();
        match name {
            "call" => rtao_cli::call::run(&mut app, matches, &replay).await,
            "completions" => rtao_cli::completions::run(&completion_tree, matches, &app.output),
            "config" => config::run(&store, &app.output, matches).await,
            "tools" => catalog::run_tools(&app.output, &generated::rust_tools()?),
            "explain" => catalog::run_explain(&app.output, matches),
            "wallet" => wallet::run(&mut app, matches, &mut invocation.history).await,
            "addr" => addresses::run(&app, store.paths(), matches).await,
            "convert" | "latency" => {
                misc::validate_before_connect(name, matches)?;
                misc::run(&app, name, matches).await
            }
            "upgrade" => upgrade::run(&mut app, matches, &mut invocation.history).await,
            "crowd" => {
                crowd::validate_before_connect(matches)?;
                let context = &app;
                app.run(|client| async move { crowd::execute(context, &client, matches).await })
                    .await
            }
            "timelock" => timelock::run(&app, matches, &mut invocation.history).await,
            #[cfg(feature = "extension")]
            "extension" => extension::run(&app, matches).await,
            #[cfg(feature = "evm")]
            "evm" => {
                evm::validate_before_connect(&app, matches)?;
                if evm::needs_native(matches)
                    && !(app.globals.dry_run && matches.subcommand_name() == Some("setup-localnet"))
                {
                    let runner = app.clone();
                    let app_ref = &mut app;
                    let history = &mut invocation.history;
                    runner
                        .run(|client| async move {
                            evm::run(
                                app_ref,
                                Some(&client),
                                matches,
                                rtao_cli::context::SubmissionOptions {
                                    replay: Some(&replay),
                                    ..Default::default()
                                },
                                None,
                                history,
                            )
                            .await
                        })
                        .await
                } else {
                    evm::run(
                        &mut app,
                        None,
                        matches,
                        Default::default(),
                        None,
                        &mut invocation.history,
                    )
                    .await
                }
            }
            "multisig" => {
                multisig::run(&mut app, store.paths(), matches, &mut invocation.history).await
            }
            "proxy" if matches.subcommand_name() == Some("book") => {
                let (_, group) = matches
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing proxy book command"))?;
                proxy::book_run(&app, store.paths(), group).await
            }
            "proxy" => {
                let request =
                    proxy::Request::from_matches(&mut app, matches, &mut invocation.history)?;
                let runner = app.clone();
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        let replay = history.replay_command(replay_args);
                        proxy::execute(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "subnets" => {
                let request =
                    subnets::Request::from_matches(&mut app, matches, &mut invocation.history)?;
                let mut runner = app.clone();
                runner.track_heads = !matches!(
                    request,
                    subnets::Request::Inspect(_) | subnets::Request::Metagraph(_)
                );
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        let replay = history.replay_command(replay_args);
                        subnets::execute(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "root" => {
                let request =
                    root::Request::from_matches(&mut app, matches, &mut invocation.history)?;
                let mut runner = app.clone();
                runner.track_heads = !matches!(request, root::Request::List { .. });
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        if matches!(&request, root::Request::List { .. }) {
                            return root::execute(
                                app_ref,
                                &client,
                                &request,
                                Default::default(),
                                None,
                                history,
                            )
                            .await
                            .map(|_| ());
                        }
                        let Some(prepared) =
                            root::prepare_submission(app_ref, &client, &request, history).await?
                        else {
                            return Ok(());
                        };
                        let replay = history.replay_command(replay_args);
                        generated::execute_tx(
                            app_ref,
                            &client,
                            &prepared.request,
                            rtao_cli::context::SubmissionOptions {
                                review: prepared.review.as_ref(),
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "conviction" | "collateral" => {
                enum ReadRequest {
                    Conviction(conviction::ReadRequest),
                    Collateral(collateral::ReadRequest),
                }
                let requests = if name == "conviction" {
                    conviction::tx_requests(&mut app, matches, &mut invocation.history)?
                } else {
                    collateral::tx_requests(&mut app, matches, &mut invocation.history)?
                };
                let read = if requests.is_empty() {
                    Some(if name == "conviction" {
                        ReadRequest::Conviction(conviction::prepare_read(
                            &mut app,
                            matches,
                            &mut invocation.history,
                        )?)
                    } else {
                        ReadRequest::Collateral(collateral::prepare_read(
                            &mut app,
                            matches,
                            &mut invocation.history,
                        )?)
                    })
                } else {
                    None
                };
                let mut runner = app.clone();
                runner.track_heads = read.is_none();
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        if let Some(read) = read {
                            return match read {
                                ReadRequest::Conviction(read) => {
                                    conviction::execute_prepared_read(app_ref, &client, &read).await
                                }
                                ReadRequest::Collateral(read) => {
                                    collateral::execute_prepared_read(app_ref, &client, &read).await
                                }
                            };
                        }
                        let replay = history.replay_command(replay_args);
                        for request in requests {
                            generated::execute_tx(
                                app_ref,
                                &client,
                                &request,
                                rtao_cli::context::SubmissionOptions {
                                    replay: Some(&replay),
                                    ..Default::default()
                                },
                                None,
                                history,
                            )
                            .await?;
                        }
                        Ok(())
                    })
                    .await
            }
            "weights" | "axon" => {
                let request = if name == "weights" {
                    weights::request(matches)?
                } else {
                    axon::request(matches)?
                };
                let runner = app.clone();
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        generated::execute_tx(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "hparams" => {
                let parsed = if !app.prompt_context().interactive(&app.terminal) {
                    Some(hparams::Request::from_matches(
                        &mut app,
                        matches,
                        &mut invocation.history,
                    )?)
                } else {
                    None
                };
                let mut runner = app.clone();
                runner.track_heads = !matches!(
                    parsed,
                    Some(hparams::Request::Hyperparameters { .. } | hparams::Request::Read(_))
                );
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        let request = match parsed {
                            Some(request) => request,
                            None => {
                                hparams::Request::from_matches_with_client(
                                    app_ref, &client, matches, history,
                                )
                                .await?
                            }
                        };
                        let replay = history.replay_command(replay_args);
                        hparams::execute(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "stake" => {
                let request =
                    stake::Request::from_matches(&mut app, matches, &mut invocation.history)?;
                if stake::execute_local(&app, &request)? {
                    return Ok(());
                }
                let mut runner = app.clone();
                runner.track_heads = !matches!(
                    request,
                    stake::Request::Show { .. } | stake::Request::List { .. }
                );
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        let replay = history.replay_command(replay_args);
                        stake::execute(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "tx" => {
                let op = matches
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing transaction"))?
                    .0
                    .replace('-', "_");
                let parsed = if !app.prompt_context().interactive(&app.terminal) {
                    Some(generated::TxRequest::from_matches(
                        &mut app,
                        &op,
                        leaf,
                        &mut invocation.history,
                    )?)
                } else {
                    None
                };
                let runner = app.clone();
                let app_ref = &mut app;
                let history = &mut invocation.history;
                runner
                    .run(|client| async move {
                        let request = match parsed {
                            Some(request) => request,
                            None => {
                                generated::TxRequest::from_matches_with_client(
                                    app_ref, &client, &op, leaf, history,
                                )
                                .await?
                            }
                        };
                        let replay = history.replay_command(replay_args);
                        generated::execute_tx(
                            app_ref,
                            &client,
                            &request,
                            rtao_cli::context::SubmissionOptions {
                                replay: Some(&replay),
                                ..Default::default()
                            },
                            None,
                            history,
                        )
                        .await
                        .map(|_| ())
                    })
                    .await
            }
            "query" => {
                let read = matches
                    .subcommand()
                    .ok_or_else(|| CliError::input("missing query"))?
                    .0
                    .replace('-', "_")
                    .parse()
                    .map_err(|error| CliError::input(format!("{error}")))?;
                let request = generated::QueryRequest::from_matches(
                    &app,
                    read,
                    leaf,
                    &mut invocation.history,
                )?;
                let mut runner = app.clone();
                runner.track_heads = false;
                let output = app.output.clone();
                runner
                    .run(|client| async move {
                        generated::execute_query(&client, &request, &output)
                            .await
                            .map(|_| ())
                    })
                    .await
            }
            _ => Err(CliError::input(format!("unknown command {name:?}"))),
        }
    })
    .await;
    if let Err(error) = &result
        && !error.message.is_empty()
        && !(cancel.is_cancelled() && error.code == 130)
    {
        app.output
            .cli_error(error)
            .map_err(|e| CliError::failure(e.to_string()))?;
    }
    app.finish_prompts(&mut invocation.history, &replay_args, result.is_ok())?;
    // Rendering has already emitted structured diagnostics for command errors.
    result.map_err(|mut error| {
        error.message.clear();
        error
    })
}
fn truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64() != Some(0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cancel = TerminalCancel::default();
    let argv = env::args_os().collect::<Vec<_>>();
    let mut output = Output::process(OutputOptions {
        json_mode: argv.iter().any(|arg| arg == "--json"),
        quiet: argv.iter().any(|arg| arg == "--quiet" || arg == "-q"),
        network: None,
    });
    let result = Box::pin(run(cancel.clone(), &mut output)).await;
    if cancel.is_cancelled() {
        // Human chatter only. Keep any primary recovery diagnostic below;
        // neither this notice nor final disposition replaces the result.
        let _ = output.message("aborted.");
    }
    match result {
        Ok(()) => ExitCode::from(cancel.exit_code(0)),
        Err(error) => {
            if !error.message.is_empty() && !(cancel.is_cancelled() && error.code == 130) {
                if error.code == 0 {
                    print!("{}", error.message);
                } else {
                    eprintln!("rtao: {}", terminal_text(&error.message));
                }
            }
            ExitCode::from(cancel.exit_code(error.code))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod interrupt_tests {
    use super::*;

    struct Dropped(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn invocation_watcher_is_aborted_on_normal_completion() {
        let cancel = TerminalCancel::default();
        let receiver_cancel = cancel.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (dropped, finished) = tokio::sync::oneshot::channel();
        let guard = Dropped(Some(dropped));
        let watcher = InterruptWatcher::spawn(async move {
            let _guard = guard;
            let _ = started.send(());
            std::future::pending::<()>().await;
            receiver_cancel.cancel();
        });
        ready.await.unwrap();
        drop(watcher);
        tokio::time::timeout(std::time::Duration::from_secs(2), finished)
            .await
            .unwrap()
            .unwrap();
        assert!(!cancel.is_cancelled());
    }
}
