//! The rtao command line application.

#![deny(missing_docs)]

mod doctor;

use std::{env, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("doctor") => {
            let manifest = match args.next().as_deref() {
                None => {
                    doctor::find_manifest(&env::current_dir().map_err(|error| error.to_string())?)?
                }
                Some("--manifest-path") => {
                    let path = PathBuf::from(
                        args.next()
                            .ok_or("--manifest-path requires a Cargo.toml path")?,
                    );
                    if args.next().is_some() {
                        return Err("usage: rtao doctor [--manifest-path <Cargo.toml>]".into());
                    }
                    path
                }
                Some(_) => return Err("usage: rtao doctor [--manifest-path <Cargo.toml>]".into()),
            };
            let check = doctor::check(&manifest)?;
            if let Some(warning) = check.warning {
                eprintln!("rtao: warning: {warning}");
            }
            println!(
                "Core pin and downstream BLS patch verified for {}",
                check.manifest.display()
            );
            Ok(())
        }
        Some("--help" | "-h" | "help") | None => {
            println!(
                "rtao\n\n  rtao doctor [--manifest-path <Cargo.toml>]\n\nCheck the workspace's core pin, BLS patch and lockfile."
            );
            Ok(())
        }
        Some("--version" | "-V") => {
            println!(
                "rtao {} (bittensor-core {})",
                env!("CARGO_PKG_VERSION"),
                rtao::core::BITTENSOR_CORE_TAG
            );
            Ok(())
        }
        Some(command) => Err(format!("unknown command {command:?}; run rtao --help")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rtao: {error}");
            ExitCode::FAILURE
        }
    }
}
