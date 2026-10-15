//! Prepare the isolated crates.io BLS dependency proof without changing workspace pins.

use anyhow::{Context, Result, bail, ensure};
use std::{path::Path, process::Command};

/// PR-U4 proof preparation arguments.
#[derive(Debug)]
pub struct PrU4ProofArgs {
    /// Preparation step to execute.
    pub command: PrU4ProofCommand,
}

/// Operations supported by the dependency proof driver.
#[derive(Debug)]
pub enum PrU4ProofCommand {
    /// Create the pinned worktree, verify the registry BLS graph and build the probe.
    Prepare,
}

/// Parse this command from the repository maintenance driver's arguments.
pub fn entry(args: Vec<String>) -> Result<()> {
    let command = match args.as_slice() {
        [command] if ["help", "--help", "-h"].contains(&command.as_str()) => {
            println!("usage: cargo xtask pr-u4-proof prepare");
            return Ok(());
        }
        [command] if command == "prepare" => PrU4ProofCommand::Prepare,
        _ => bail!("usage: cargo xtask pr-u4-proof prepare"),
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask workspace root")?;
    run(root, PrU4ProofArgs { command })
}

/// Run proof preparation under the caller's recorded task root and Cargo target.
pub fn run(root: &Path, args: PrU4ProofArgs) -> Result<()> {
    match args.command {
        PrU4ProofCommand::Prepare => {
            let script = root.join("ci/pr-u4-probe/prepare.sh");
            ensure!(script.is_file(), "PR-U4 preparation script is missing");
            let status = Command::new("bash")
                .arg(script)
                .current_dir(root)
                .status()
                .context("run isolated PR-U4 proof preparation")?;
            ensure!(status.success(), "PR-U4 preparation failed with {status}");
        }
    }
    Ok(())
}
