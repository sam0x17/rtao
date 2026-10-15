//! The rtao command line application.

#![deny(missing_docs)]

mod doctor;

use rtao_cli::{
    application,
    commands::{axon, catalog, config, generated, hparams, weights},
    context::AppContext,
    error::CliError,
    output::DiagnosticContext,
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
        eprintln!("rtao: warning: {warning}");
    }
    println!(
        "Core pin and downstream BLS patch verified for {}",
        check.manifest.display()
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
    let mut command = application::command_tree([
        rtao_cli::call::command(),
        config::command(),
        catalog::tools_command(),
        catalog::explain_command(),
        generated::tx_command()?,
        generated::query_command()?,
        weights::command(),
        axon::command(),
        hparams::command()?,
    ]);
    if argv.len() == 1 || argv.get(1).is_some_and(|arg| arg == "help") {
        print!("{}", command.render_long_help());
        return Ok(());
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
    let mut invocation = application::parse_with_prompts(command, argv, &context, &mut prompting)?;
    let Some((name, matches)) = invocation.matches.subcommand() else {
        return Err(CliError::input("missing command"));
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
        _ => Vec::new(),
    };
    let mut app = AppContext::from_store(
        &invocation.matches,
        leaf,
        &reserved,
        &store,
        &home,
        |name| env::var(name).ok(),
        terminal,
    )
    .await?;
    rtao_cli::logs::setup_logging(app.globals.verbosity, app.globals.quiet)
        .map_err(|e| CliError::failure(e.to_string()))?;
    let replay_args = invocation
        .argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let replay = invocation.history.replay_command(&replay_args);
    let result = async {
        match name {
            "call" => rtao_cli::call::run(&mut app, matches, &replay).await,
            "config" => config::run(&store, &app.output, matches).await,
            "tools" => catalog::run_tools(&app.output, &generated::rust_tools()?),
            "explain" => catalog::run_explain(&app.output, matches),
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
                        let replay = history.replay_command(&replay_args);
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
                        let replay = history.replay_command(&replay_args);
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
                    eprintln!("rtao: {}", error.message);
                }
            }
            ExitCode::from(error.code)
        }
    }
}
