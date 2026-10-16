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
    output::{DiagnosticContext, terminal_text},
    prompt::{LiveTerminal, PromptContext, SharedTerminal},
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

async fn run() -> Result<(), CliError> {
    let argv = env::args_os().collect::<Vec<_>>();
    if argv.get(1).is_some_and(|arg| arg == "doctor") {
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
            .map_err(|error| CliError::input(error.to_string()))?;
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
    let terminal: SharedTerminal = Arc::new(Mutex::new(Box::new(LiveTerminal)));
    let mut prompting = terminal.clone();
    let completion_tree = command.clone();
    let replay_args = argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut argv = argv;
    let mut prompt_history = rtao_cli::prompt::PromptHistory::default();
    let mut prepared_context = None;
    if let Some(partial) =
        application::pending_wallet_matches(&command, &argv, &context, &prompting)?
    {
        let mut leaf = &partial;
        while let Some((_, child)) = leaf.subcommand() {
            leaf = child;
        }
        #[allow(deprecated)]
        let prompt_home =
            env::home_dir().ok_or_else(|| CliError::failure("home directory is unavailable"))?;
        // Only the two local-key commands above need this early context. This
        // loads public options/books; confirmation never acquires a signer.
        let mut app = AppContext::from_store(
            &partial,
            leaf,
            &[],
            &store,
            &prompt_home,
            |name| env::var(name).ok(),
            terminal.clone(),
        )
        .await?;
        application::confirm_required_wallet(&mut app, &mut argv, &mut prompt_history)?;
        prepared_context = Some(app);
    }
    let parse_context = prepared_context
        .as_ref()
        .map(AppContext::prompt_context)
        .unwrap_or(context);
    let mut invocation = application::parse_with_prompt_history(
        command,
        argv,
        &parse_context,
        &mut prompting,
        prompt_history,
    )?;
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
        None => {
            AppContext::from_store(
                &invocation.matches,
                leaf,
                &reserved,
                &store,
                &home,
                |name| env::var(name).ok(),
                terminal,
            )
            .await?
        }
    };
    rtao_cli::logs::setup_logging(app.globals.verbosity, app.globals.quiet)
        .map_err(|e| CliError::failure(e.to_string()))?;
    let replay = invocation.history.replay_command(&replay_args);
    let result = async {
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
                let runner = app.clone();
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
                let runner = app.clone();
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
                        let Some(request) =
                            root::prepare_transaction(app_ref, &client, &request, history).await?
                        else {
                            return Ok(());
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
                let runner = app.clone();
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
                let runner = app.clone();
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
                let runner = app.clone();
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
                let runner = app.clone();
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
    }
    .await;
    if let Err(error) = &result
        && !error.message.is_empty()
    {
        app.output
            .error(
                &error.message,
                DiagnosticContext {
                    help: error.help.as_deref(),
                    ..Default::default()
                },
            )
            .map_err(|e| CliError::failure(e.to_string()))?;
    }
    application::finish_prompts(
        &app.output,
        &mut invocation.history,
        &replay_args,
        result.is_ok(),
    )?;
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
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if !error.message.is_empty() {
                if error.code == 0 {
                    print!("{}", error.message);
                } else {
                    eprintln!("rtao: {}", terminal_text(&error.message));
                }
            }
            ExitCode::from(error.code)
        }
    }
}
