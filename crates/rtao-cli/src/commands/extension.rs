// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/extension.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Explicit browser-extension bridge lifecycle and public account discovery.

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, TableSpec},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::signer::extension::{
    BridgeClient, BridgeOptions, BridgePaths, DEFAULT_BRIDGE_HOST, DEFAULT_BRIDGE_PORT,
    EnsureBridgeOptions, ExtensionAccount, bridge_ws_url, ensure_bridge, run_bridge,
    run_bridge_daemon, stop_bridge_daemon,
};
use serde_json::json;
use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    pin::Pin,
    time::Duration,
};

/// An explicitly requested bridge command operation, injectable without any process or socket.
pub type ExtensionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;

/// Side-effect boundary for the extension commands' daemon, browser and socket access.
pub trait ExtensionBackend: Send + Sync {
    /// Serve until an interrupt, printing public URLs through the shared output.
    fn serve<'a>(
        &'a self,
        options: BridgeOptions,
        paths: BridgePaths,
        daemon: bool,
        output: &'a Output,
    ) -> ExtensionFuture<'a, ()>;
    /// Stop a daemon only after the library's authentication and process-identity checks.
    fn stop<'a>(&'a self, paths: BridgePaths, url: String) -> ExtensionFuture<'a, bool>;
    /// Resolve an explicitly authorized bridge and list public extension accounts.
    fn accounts<'a>(
        &'a self,
        options: EnsureBridgeOptions,
    ) -> ExtensionFuture<'a, Vec<ExtensionAccount>>;
}

struct NativeExtension;
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
impl ExtensionBackend for NativeExtension {
    fn serve<'a>(
        &'a self,
        options: BridgeOptions,
        paths: BridgePaths,
        daemon: bool,
        output: &'a Output,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if daemon {
                return run_bridge_daemon(&options, &paths).await.map_err(failure);
            }
            let mut server = run_bridge(&options).await.map_err(failure)?;
            let _files = paths.publish(&server).map_err(failure)?;
            output
                .message(&format!(
                    "extension bridge listening on {}",
                    server.http_url()
                ))
                .map_err(failure)?;
            output
                .message(&format!("websocket clients: {}", server.ws_url()))
                .map_err(failure)?;
            let signal = wait_for_interrupt().await;
            let stopped = server.stop().await.map_err(failure);
            signal.and(stopped)?;
            output.message("bridge stopped.").map_err(failure)
        })
    }
    fn stop<'a>(&'a self, paths: BridgePaths, url: String) -> ExtensionFuture<'a, bool> {
        Box::pin(async move { stop_bridge_daemon(&paths, &url).await.map_err(failure) })
    }
    fn accounts<'a>(
        &'a self,
        options: EnsureBridgeOptions,
    ) -> ExtensionFuture<'a, Vec<ExtensionAccount>> {
        Box::pin(async move {
            let url = ensure_bridge(&options).await.map_err(failure)?;
            let token = options
                .paths
                .read_token()
                .map_err(failure)?
                .ok_or_else(|| failure("extension bridge client token is missing"))?;
            let client = BridgeClient::new(&url, token.as_str()).map_err(failure)?;
            let rows = client.list_accounts().await.map_err(failure);
            client.close().await;
            rows
        })
    }
}

async fn wait_for_interrupt() -> Result<(), CliError> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(failure)?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(failure),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.map_err(failure)
    }
}

fn listener(command: Command) -> Command {
    command
        .arg(
            Arg::new("host")
                .long("host")
                .default_value(DEFAULT_BRIDGE_HOST)
                .help("Numeric loopback bind address."),
        )
        .arg(
            Arg::new("port")
                .long("port")
                .default_value("39295")
                .allow_negative_numbers(true)
                .value_parser(|raw: &str| {
                    rtao::primitives::pyfmt::parse_integer(raw)
                        .map(|port| port.to_string())
                        .ok_or_else(|| "bridge port must be an integer".to_owned())
                })
                .help("Bridge listen port."),
        )
}

/// Build `misc extension bridge`, `stop` and `accounts`; daemon flags are hidden.
pub fn command() -> Command {
    Command::new("extension")
        .about("Manage browser-extension signing through the local bridge.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            listener(Command::new("bridge").about("Run the extension bridge until interrupted."))
                .arg(
                    Arg::new("no_open")
                        .long("no-open")
                        .action(ArgAction::SetTrue)
                        .help("Do not open the bridge page."),
                )
                .arg(
                    Arg::new("daemon")
                        .long("daemon")
                        .hide(true)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("pid_file")
                        .long("pid-file")
                        .hide(true)
                        .requires("daemon")
                        .value_parser(clap::value_parser!(PathBuf)),
                ),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("stop").about("Stop the authenticated background bridge daemon."),
            Tier::Read,
        ))
        .subcommand(with_globals(
            listener(Command::new("accounts").about(
                "List accounts exposed by connected extensions; may start a bridge and browser.",
            )),
            Tier::Extension,
        ))
}

fn address(leaf: &ArgMatches) -> Result<SocketAddr, CliError> {
    let host = leaf
        .get_one::<String>("host")
        .ok_or_else(|| CliError::input("missing bridge host"))?
        .parse::<IpAddr>()
        .map_err(|_| CliError::input("extension bridge host must be a numeric loopback address"))?;
    if !host.is_loopback() {
        return Err(CliError::input(
            "extension bridge must bind a loopback address",
        ));
    }
    let port = leaf
        .get_one::<String>("port")
        .map(|port| port.parse::<u16>())
        .transpose()
        .map_err(|_| CliError::input("extension bridge port must be between 0 and 65535"))?
        .unwrap_or(DEFAULT_BRIDGE_PORT);
    Ok(SocketAddr::new(host, port))
}

/// Execute an explicitly requested extension command, resolving Python-compatible daemon paths.
pub async fn run(ctx: &AppContext, group: &ArgMatches) -> Result<(), CliError> {
    run_with_backend(
        ctx,
        group,
        BridgePaths::from_environment().map_err(failure)?,
        &NativeExtension,
    )
    .await
}

/// Execute with injected lifecycle and accounts operations; this function itself starts no process or socket.
pub async fn run_with_backend(
    ctx: &AppContext,
    group: &ArgMatches,
    mut paths: BridgePaths,
    backend: &dyn ExtensionBackend,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing extension command"))?;
    match name {
        "bridge" => {
            if let Some(path) = leaf.get_one::<PathBuf>("pid_file") {
                paths.pid_file = path.clone();
            }
            backend
                .serve(
                    BridgeOptions {
                        address: address(leaf)?,
                        open_browser: !leaf.get_flag("no_open") && !leaf.get_flag("daemon"),
                        browser: ctx.globals.extension_browser.clone(),
                    },
                    paths,
                    leaf.get_flag("daemon"),
                    &ctx.output,
                )
                .await
        }
        "stop" => {
            let url = ctx.globals.extension_bridge_url.clone().unwrap_or_else(|| {
                bridge_ws_url(SocketAddr::from(([127, 0, 0, 1], DEFAULT_BRIDGE_PORT)))
            });
            let stopped = backend.stop(paths, url).await?;
            ctx.output
                .message(if stopped {
                    "extension bridge stopped."
                } else {
                    "extension bridge is not running."
                })
                .map_err(failure)
        }
        "accounts" => {
            let url = match &ctx.globals.extension_bridge_url {
                Some(url) if !url.is_empty() => url.clone(),
                _ => bridge_ws_url(address(leaf)?),
            };
            let options = EnsureBridgeOptions {
                url,
                paths,
                allow_spawn: true,
                open_browser: !ctx.output.options().quiet,
                browser: ctx.globals.extension_browser.clone(),
                fresh: true,
                startup_timeout: Duration::from_secs(15),
                extension_timeout: Duration::from_secs(180),
            };
            let rows = backend.accounts(options).await?;
            let records = serde_json::to_value(&rows).map_err(failure)?;
            let mut table = TableSpec::new(
                "extension accounts",
                ["name", "address", "source", "type"]
                    .map(String::from)
                    .to_vec(),
                rows.iter()
                    .map(|row| {
                        vec![
                            json!(row.name),
                            json!(row.address),
                            json!(row.source),
                            json!(row.account_type),
                        ]
                    })
                    .collect(),
            );
            table.records = Some(records);
            ctx.output.table(&table).map_err(failure)
        }
        _ => Err(CliError::input("unknown extension command")),
    }
}
