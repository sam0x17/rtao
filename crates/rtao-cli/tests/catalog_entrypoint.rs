//! Config and catalog routes through the real binary with isolated file paths.

use rtao::client::config::ConfigFile;
use serde_json::{Value, json};
use std::{
    error::Error,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-catalog-tests")
            });
        let path = base.join("tmp").join(format!(
            "catalog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn config_path(&self) -> PathBuf {
        self.0.join("shared-0.json")
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
        // These commands must complete without connecting, even when the
        // configured endpoint cannot serve RPC. Shared books are redirected.
        command.env("BT_NETWORK", "ws://127.0.0.1:1");
        for variable in [
            "BT_WALLET",
            "BT_WALLET_HOTKEY",
            "BT_WALLET_PATH",
            "BT_WALLET_PASSWORD_FILE",
            "BT_RETRY_FOREVER",
            "BITTENSOR_LOG",
        ] {
            command.env_remove(variable);
        }
        command
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        // All child commands have returned; these paths contain only fixtures.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn success_payload(output: Output) -> Result<Value, Box<dyn Error>> {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn config_binary_routes_all_five_commands_and_leaf_globals() -> TestResult {
    let root = Root::new()?;
    let value = success_payload(
        root.command()
            .args(["c", "set", "network", "test", "--json", "--quiet"])
            .output()?,
    )?;
    assert_eq!(
        value,
        json!({"key":"network","value":"test","path":root.config_path()})
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(root.config_path())?)?;
    assert_eq!(stored, json!({"network":"test"}));
    assert_eq!(
        success_payload(
            root.command()
                .args(["--json", "conf", "get", "network"])
                .output()?
        )?,
        json!("test")
    );
    assert_eq!(
        success_payload(root.command().args(["config", "path", "--json"]).output()?)?,
        json!(root.config_path())
    );
    assert_eq!(
        success_payload(
            root.command()
                .args(["config", "unset", "network", "--json"])
                .output()?
        )?,
        json!({"key":"network","existed":true})
    );
    success_payload(
        root.command()
            .args(["config", "set", "wallet", "fixture", "--json"])
            .output()?,
    )?;
    assert_eq!(
        success_payload(
            root.command()
                .args(["config", "clear", "--json"])
                .output()?
        )?,
        json!({"path":root.config_path()})
    );
    let cleared: Value = serde_json::from_slice(&std::fs::read(root.config_path())?)?;
    assert_eq!(cleared, json!({}));
    Ok(())
}

#[test]
fn tools_binary_emits_complete_json_with_unavailable_commands_unadvertised() -> TestResult {
    let root = Root::new()?;
    let value = success_payload(root.command().args(["tools", "--quiet"]).output()?)?;
    let rows = value.as_array().ok_or("tools array")?;
    assert_eq!(rows.len(), rtao_catalog::INTENTS.len());
    for (row, source) in rows.iter().zip(&rtao_catalog::INTENTS) {
        assert_eq!(row["name"], source.name);
        assert_eq!(row.as_object().ok_or("tool object")?.len(), 10);
        let metadata = rtao_intents::interface_metadata()
            .into_iter()
            .find(|metadata| metadata.name == source.name)
            .ok_or("intent metadata")?;
        if metadata.compiled {
            assert!(row["rust"]["type"].is_string());
            assert!(row["rust"]["role"].is_string());
            assert!(row["rust"]["output"].is_string());
        } else {
            assert!(row["rust"]["type"].is_null());
            assert!(row["rust"]["role"].is_null());
            assert!(row["rust"]["output"].is_null());
        }
        if metadata.compiled && metadata.implemented {
            assert_eq!(
                row["rust"]["cli"],
                format!("rtao tx {}", source.name.replace('_', "-"))
            );
        } else {
            assert!(row["rust"]["cli"].is_null());
        }
    }
    let transfer = rows
        .iter()
        .find(|row| row["name"] == "transfer")
        .ok_or("transfer tool")?;
    assert_eq!(
        transfer["rust"]["type"],
        std::any::type_name::<rtao_intents::Transfer>()
    );
    assert_eq!(
        transfer["rust"]["role"],
        std::any::type_name::<<rtao_intents::Transfer as rtao::tx::IntentInterface>::Role>()
    );
    assert_eq!(
        transfer["rust"]["output"],
        std::any::type_name::<<rtao_intents::Transfer as rtao::tx::IntentInterface>::Output>()
    );
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn generated_binary_registers_operations_from_actual_availability() -> TestResult {
    let root = Root::new()?;
    for (family, present) in [
        ("tx", "set-weights"),
        ("query", "balances"),
        ("weights", "reveal"),
        ("axon", "reset"),
        ("hparams", "mechanisms"),
    ] {
        let output = root.command().args([family, "--help"]).output()?;
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(String::from_utf8(output.stdout)?.contains(present));
    }
    let metadata = rtao_intents::interface_metadata();
    assert_eq!(metadata.len(), rtao_catalog::INTENTS.len());
    for operation in metadata {
        let name = operation.name.replace('_', "-");
        let output = root.command().args(["tx", &name, "--help"]).output()?;
        if operation.compiled && operation.implemented {
            assert_eq!(output.status.code(), Some(0), "{name}");
            assert!(output.stderr.is_empty(), "{name}");
            assert!(
                String::from_utf8(output.stdout)?.contains(&format!("rtao tx {name}")),
                "{name}"
            );
        } else {
            assert_eq!(output.status.code(), Some(2), "{name}");
            assert!(output.stdout.is_empty(), "{name}");
            assert!(String::from_utf8(output.stderr)?.contains(&name), "{name}");
        }
    }
    let unknown = root
        .command()
        .args(["tx", "not-a-catalog-operation", "--help"])
        .output()?;
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
    assert!(String::from_utf8(unknown.stderr)?.contains("not-a-catalog-operation"));
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn generated_binary_missing_inputs_emit_one_json_error_before_connection() -> TestResult {
    let root = Root::new()?;
    let output = root
        .command()
        .args(["tx", "transfer", "--json", "--quiet"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr)?;
    assert!(
        diagnostic["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn explain_binary_routes_semantic_and_chain_modes_without_connecting() -> TestResult {
    let root = Root::new()?;
    let value = success_payload(
        root.command()
            .args(["--json", "explain", "insufficient_balance"])
            .output()?,
    )?;
    assert_eq!(value["code"], "insufficient_balance");
    assert_eq!(value.as_object().ok_or("semantic explanation")?.len(), 4);
    assert!(
        value["explanation"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    let value = success_payload(
        root.command()
            .args(["explain", "--chain", "bAlAnCeS", "--json", "--quiet"])
            .output()?,
    )?;
    let records = value.as_array().ok_or("chain error array")?;
    assert!(!records.is_empty());
    assert!(records.iter().all(|record| record["pallet"] == "Balances"));
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn offline_command_errors_emit_one_structured_diagnostic_with_status_one() -> TestResult {
    let root = Root::new()?;
    for args in [
        vec!["config", "get", "invalid-key", "--json"],
        vec!["explain", "invalid-code", "--json"],
    ] {
        let output = root.command().args(args).output()?;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        // Parsing the full stream rejects repeated rendering or plain text.
        let diagnostic: Value = serde_json::from_slice(&output.stderr)?;
        assert!(
            diagnostic["error"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert!(
            diagnostic["help"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}
