//! `rtao-remote-signerd`: the reference rtao-remote-signer/1 signing server.

use std::{path::PathBuf, process::ExitCode};

use clap::{ArgAction, Parser};
use rtao_remote_signer::daemon::{Daemon, DaemonConfig};

/// Serve policy-checked remote signing over btauth/1.
#[derive(Parser, Debug)]
#[command(name = "rtao-remote-signerd", version)]
struct Args {
    /// TOML configuration file.
    #[arg(long, short = 'c')]
    config: PathBuf,
    /// Validate the configuration, load keys and exit without serving.
    #[arg(long)]
    check: bool,
    /// Increase log verbosity (-v debug, -vv trace).
    #[arg(long = "verbose", short = 'v', action = ArgAction::Count)]
    verbosity: u8,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let level = match args.verbosity {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .init();
    let result = async {
        let daemon = Daemon::build(DaemonConfig::load(&args.config)?).await?;
        if args.check {
            println!(
                "configuration ok: server key {}, {} account(s)",
                daemon.server.server_key(),
                daemon.server.accounts().len()
            );
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind(daemon.listen)
            .await
            .map_err(|e| {
                rtao_remote_signer::daemon::DaemonError(format!(
                    "cannot listen on {}: {e}",
                    daemon.listen
                ))
            })?;
        let bound = listener.local_addr().map_err(|e| {
            rtao_remote_signer::daemon::DaemonError(format!("cannot read listen address: {e}"))
        })?;
        // One parseable readiness line on stdout for supervisors and tests.
        println!("listening on {bound}");
        daemon.serve(listener).await
    }
    .await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
