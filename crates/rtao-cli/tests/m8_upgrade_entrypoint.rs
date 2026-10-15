//! Actual upgrade routes reject bad public artifacts before chain connections.

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
            "m8-upgrade-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root)?;
        Ok(Self(root))
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
        command.stdin(Stdio::null());
        for (index, file) in ConfigFile::ALL.into_iter().enumerate() {
            command.env(
                file.environment(),
                self.0.join(format!("shared-{index}.json")),
            );
        }
        // This endpoint cannot resolve to a real chain. Public artifact failures
        // must occur before its configuration is used to open a connection.
        command.env("BT_NETWORK", "not-a-network");
        command.env("BT_WALLET_PATH", self.0.join("wallets"));
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
        // Only these fixture-owned public files exist; no task launches a
        // service, signer, symlink or virtualenv that could depend on the root.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_artifact_reaches_all_three_real_alias_paths_before_connecting() -> TestResult {
    let root = Root::new()?;
    let hex = root.0.join("invalid.hex");
    std::fs::write(&hex, b"0xzz")?;
    for prefix in [
        &["misc", "upgrade"][..],
        &["utils", "upgrade"][..],
        &["upgrade"][..],
    ] {
        let result = root
            .command()
            .args(prefix)
            .arg("check")
            .arg("--hex-file")
            .arg(&hex)
            .arg("--json")
            .output()?;
        assert_eq!(result.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("call data is not a valid hex string"),
            "{stderr}"
        );
        assert!(!stderr.contains("not-a-network"), "{stderr}");
        assert!(result.stdout.is_empty());
    }
    assert!(!root.0.join("wallets").exists());
    Ok(())
}

#[test]
fn conflicting_sources_and_malformed_repository_fail_in_real_dispatch() -> TestResult {
    let root = Root::new()?;
    for (args, diagnostic) in [
        (
            &[
                "misc",
                "upgrade",
                "check",
                "--hex-url",
                "https://invalid.example/blob",
                "--hex-file",
                "unused.hex",
            ][..],
            "pass exactly one",
        ),
        (
            &["upgrade", "pending", "--repo", "A/B/C"][..],
            "--repo must be OWNER/REPO",
        ),
    ] {
        let result = root.command().args(args).output()?;
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(diagnostic), "{stderr}");
        assert!(!stderr.contains("not-a-network"), "{stderr}");
    }
    Ok(())
}
