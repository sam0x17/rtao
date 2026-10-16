// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/help_theme.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! No-argument group help is informational; actual usage errors remain failures.

use rtao::client::config::ConfigFile;
use std::{
    error::Error,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn Error>>;

struct Root(PathBuf);
impl Root {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-fixtures")
            })
            .join("tmp");
        std::fs::create_dir_all(&base)?;
        let root = base.join(format!(
            "m8-help-theme-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root)?;
        Ok(Self(root))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
        command.env("HOME", &self.0);
        command.stdin(Stdio::null());
        command.env("NO_COLOR", "1");
        command.env("CLICOLOR_FORCE", "0");
        command.env("BT_NETWORK", "ws://127.0.0.1:1");
        command.env("BT_WALLET_PATH", self.0.join("wallets"));
        for (index, file) in ConfigFile::ALL.into_iter().enumerate() {
            command.env(
                file.environment(),
                self.0.join(format!("shared-{index}.json")),
            );
        }
        for variable in [
            "BT_WALLET",
            "BT_WALLET_HOTKEY",
            "BT_RETRY_FOREVER",
            "BT_WALLET_PASSWORD",
            "BT_WALLET_PASSWORD_FILE",
            "BITTENSOR_LOG",
        ] {
            command.env_remove(variable);
        }
        command
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // Exact fixture-owned config root; these parser-only subprocesses have
        // finished and never launch services, wallets, signers or virtualenvs.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn group_paths(command: &clap::Command, prefix: &[String], result: &mut Vec<Vec<String>>) {
    if command.is_arg_required_else_help_set() && command.get_subcommands().next().is_some() {
        result.push(prefix.to_vec());
    }
    for child in command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
    {
        let mut path = prefix.to_vec();
        path.push(child.get_name().into());
        group_paths(child, &path, result);
    }
}

#[test]
fn bare_installed_groups_and_historical_aliases_print_help_and_exit_zero() -> TestResult {
    let root = Root::new()?;
    let command = rtao_cli::commands::command_tree()?;
    let mut paths = Vec::new();
    group_paths(&command, &[], &mut paths);
    // Include the reported route explicitly so a future command-tree change
    // cannot accidentally remove its regression coverage.
    assert!(paths.contains(&vec!["addr".into()]));
    assert!(paths.contains(&vec!["proxy".into(), "book".into()]));
    assert!(paths.contains(&vec!["wallet".into(), "keychain".into()]));
    for alias in [
        &["addresses"][..],
        &["w"][..],
        &["conf"][..],
        &["utils"][..],
        &["utils", "upgrade"][..],
        &["upgrade"][..],
        &["crowdloan"][..],
        &["su", "mechanisms"][..],
    ] {
        paths.push(alias.iter().map(|part| (*part).to_owned()).collect());
    }
    for path in paths {
        let result = root.command().args(&path).output()?;
        assert_eq!(
            result.status.code(),
            Some(0),
            "{}: {}",
            path.join(" "),
            String::from_utf8_lossy(&result.stderr)
        );
        let help = String::from_utf8_lossy(&result.stdout);
        assert!(help.contains("Usage:"), "{}: {help}", path.join(" "));
        let mut group = &command;
        for name in &path {
            group = group.find_subcommand(name).ok_or("installed help path")?;
        }
        // Generated tx/query help groups commands by category rather than
        // using Clap's generic Commands heading. Require the actual entries.
        for child in group.get_subcommands().filter(|child| !child.is_hide_set()) {
            assert!(
                help.lines()
                    .any(|line| line.split_whitespace().next() == Some(child.get_name())),
                "{}: missing {} in {help}",
                path.join(" "),
                child.get_name()
            );
        }
        assert!(!help.contains("error:"), "{}: {help}", path.join(" "));
        assert!(
            result.stderr.is_empty(),
            "{}: {}",
            path.join(" "),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert!(!root.0.join("wallets").exists());
    Ok(())
}

#[test]
fn malformed_input_and_missing_required_leaf_values_still_exit_two() -> TestResult {
    let root = Root::new()?;
    for args in [
        &["--unknown"][..],
        &["addr", "unknown"][..],
        &["addr", "--unknown"][..],
        &["addr", "show"][..],
        &["addr", "remove"][..],
        &["config", "set", "network"][..],
        &["misc", "convert"][..],
        &["misc", "crowd", "info"][..],
        &["misc", "convert", "1", "--netuid", "invalid"][..],
        &["misc", "convert", "1", "--netuid"][..],
    ] {
        let result = root.command().args(args).output()?;
        assert_eq!(
            result.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.stdout.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(!result.stderr.is_empty(), "{args:?}");
    }
    let result = root.command().args(["addr", "--help"]).output()?;
    assert_eq!(result.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&result.stdout).contains("Usage:"));
    assert!(result.stderr.is_empty());
    Ok(())
}
