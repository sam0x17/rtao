//! Actual misc helper routing remains offline for conversion and bad selections.

use rtao::client::config::ConfigFile;
use serde_json::{Value, json};
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
            "m8-misc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&root)?;
        Ok(Self(root))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
        command.env("HOME", &self.0);
        command.stdin(Stdio::null());
        for (index, file) in ConfigFile::ALL.into_iter().enumerate() {
            command.env(
                file.environment(),
                self.0.join(format!("shared-{index}.json")),
            );
        }
        // Offline routes must succeed even when the global chain is unavailable.
        command.env("BT_NETWORK", "ws://127.0.0.1:1");
        for variable in [
            "BT_WALLET",
            "BT_WALLET_PATH",
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn conversion_reaches_real_misc_and_utils_routes_without_a_chain_or_cache_writes() -> TestResult {
    let root = Root::new()?;
    for group in ["misc", "utils"] {
        let result = root
            .command()
            .args([group, "convert", "1.25", "--netuid", "7", "--json"])
            .output()?;
        assert!(
            result.status.success(),
            "{group}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout)?,
            json!({"amount":1.25,"currency":"alpha (netuid 7)","rao":1_250_000_000_u64,"display":"1.250000000α₇"})
        );
        assert!(result.stderr.is_empty());
    }
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn malformed_later_latency_target_fails_without_connecting_or_emitting_partial_json() -> TestResult
{
    let root = Root::new()?;
    let result = root
        .command()
        .args([
            "misc",
            "latency",
            "--networks",
            "ws://127.0.0.1:1,not-a-network",
            "--json",
        ])
        .output()?;
    assert_eq!(result.status.code(), Some(2));
    let diagnostic = String::from_utf8(result.stderr)?;
    assert!(diagnostic.contains("Unknown network"));
    assert!(String::from_utf8(result.stdout)?.trim().is_empty());
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn offline_m8_groups_and_completions_reach_the_real_entrypoint() -> TestResult {
    let root = Root::new()?;
    for group in ["addr", "addresses"] {
        let result = root.command().args([group, "list", "--json"]).output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout)?["addresses"],
            json!([])
        );
    }
    let result = root
        .command()
        .args(["proxy", "book", "list", "--verify", "--json"])
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout)?["proxies"],
        json!([])
    );
    for group in ["stake", "st"] {
        let result = root.command().args([group, "wizard", "--json"]).output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            serde_json::from_slice::<Value>(&result.stdout)?["move"]
                .as_str()
                .is_some_and(|guide| guide.contains("stake move"))
        );
    }
    let result = root
        .command()
        .args(["wallet", "list", "--json"])
        .env("BT_WALLET_PATH", root.0.join("absent-wallets"))
        .output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _: Value = serde_json::from_slice(&result.stdout)?;
    let result = root.command().args(["completions", "bash"]).output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let script = String::from_utf8(result.stdout)?;
    assert!(script.contains("rtao"));
    assert!(script.contains("wallet"));
    assert!(script.contains("wizard"));
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}
