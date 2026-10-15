//! Real auxiliary routes use isolated config paths and never start native or network services.

use rtao::client::config::ConfigFile;
use serde_json::Value;
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
            });
        let path = base.join("tmp").join(format!(
            "m7-auxiliary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
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
fn auxiliary_help_reaches_misc_utils_and_top_level_aliases_without_side_effects() -> TestResult {
    let root = Root::new()?;
    for (args, expected) in [
        (
            vec!["wallet", "keychain", "--help"],
            vec!["save", "show", "delete"],
        ),
        (
            vec!["w", "keychain", "save", "--help"],
            vec!["--wallet", "--json"],
        ),
        (
            vec!["misc", "timelock", "encrypt", "--help"],
            vec!["--in", "--at", "--round", "--blocks"],
        ),
        (
            vec!["utils", "timelock", "decrypt", "--help"],
            vec!["--wait", "--timeout", "--out"],
        ),
        (vec!["timelock", "show", "--help"], vec!["--file", "--json"]),
    ] {
        let result = root.command().args(&args).output()?;
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout)?;
        for fragment in expected {
            assert!(text.contains(fragment), "{args:?} missing {fragment}");
        }
    }
    #[cfg(feature = "extension")]
    for path in [
        vec!["misc", "extension"],
        vec!["utils", "extension"],
        vec!["extension"],
    ] {
        for leaf in ["bridge", "stop", "accounts"] {
            let mut args = path.clone();
            args.extend([leaf, "--help"]);
            let result = root.command().args(&args).output()?;
            assert!(
                result.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let text = String::from_utf8(result.stdout)?;
            assert!(!text.contains("--daemon") && !text.contains("--pid-file"));
        }
    }
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn timelock_show_executes_real_offline_dispatch_and_bad_input_returns_no_payload() -> TestResult {
    let root = Root::new()?;
    let mut bytes = vec![0x7f; 4];
    bytes.extend_from_slice(b"AES_GCM_");
    bytes.extend_from_slice(&1u64.to_le_bytes());
    let hex = hex::encode(bytes);
    for path in [
        vec!["misc", "timelock"],
        vec!["utils", "timelock"],
        vec!["timelock"],
    ] {
        let mut args = path;
        args.extend(["show", &hex, "--json"]);
        let result = root.command().args(&args).output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stderr.is_empty());
        let payload: Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(payload["reveal_round"], 1);
        assert_eq!(payload["bytes"], 20);
        assert_eq!(payload["revealed"], true);
        assert_eq!(payload["remaining_seconds"], 0);
        assert_eq!(payload.as_object().ok_or("timelock object")?.len(), 5);
    }
    let result = root
        .command()
        .args(["timelock", "show", "not-a-ciphertext", "--json"])
        .output()?;
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let error: Value = serde_json::from_slice(&result.stderr)?;
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|text| text.contains("ciphertext"))
    );
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}
