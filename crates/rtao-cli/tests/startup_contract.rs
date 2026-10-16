// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/main.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Legacy startup compatibility through actual parsing and the offline binary.

use rtao::{client::config::ConfigFile, primitives::AccountId};
use rtao_cli::application::{normalize_legacy_address_argv, warn_legacy_config};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> io::Result<Self> {
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-fixtures")
            })
            .join("tmp");
        std::fs::create_dir_all(&base)?;
        let path = base.join(format!(
            "startup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
        command
            .stdin(Stdio::null())
            .env("HOME", &self.0)
            .env("NO_COLOR", "1")
            .env("CLICOLOR_FORCE", "0");
        for (index, file) in ConfigFile::ALL.into_iter().enumerate() {
            command.env(
                file.environment(),
                self.0.join(format!("shared-{index}.json")),
            );
        }
        for key in [
            "BT_NETWORK",
            "BT_WALLET",
            "BT_WALLET_HOTKEY",
            "BT_WALLET_PATH",
            "BT_RETRY_FOREVER",
            "BT_WALLET_PASSWORD_FILE",
            "BT_WALLET_PASSWORD",
            "BT_SIGNER_ADDRESS",
            "BT_EXTENSION_BROWSER",
            "BT_EXTENSION_BRIDGE",
            "BITTENSOR_LOG",
        ] {
            command.env_remove(key);
        }
        command.env("BT_NETWORK", "ws://127.0.0.1:1");
        command
    }
    fn legacy(&self) -> PathBuf {
        self.0.join(".bittensor/config.yml")
    }
    fn marker(&self) -> PathBuf {
        self.0.join(".bittensor/.btcli_legacy_config_warned")
    }
    fn seed_legacy(&self) -> io::Result<()> {
        std::fs::create_dir(self.0.join(".bittensor"))?;
        std::fs::write(self.legacy(), b"old yaml; must never be parsed\n")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        // All binary invocations have returned; only this fixture's files live
        // below the exact uniquely named root, without services or keyfiles.
        if self.0.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .starts_with(&format!("startup-{}-", std::process::id()))
        }) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(|value| OsString::from(*value)).collect()
}

#[test]
fn legacy_address_rewrite_obeys_every_guard_and_reaches_the_actual_parser() -> TestResult {
    let address = AccountId([61; 32]).to_string();
    for group in ["addr", "addresses"] {
        let mut argv = words(&[
            "rtao", group, "contact", &address, "--note", "kept", "--json",
        ]);
        normalize_legacy_address_argv(&mut argv);
        assert_eq!(
            argv,
            words(&[
                "rtao", group, "add", "contact", &address, "--note", "kept", "--json"
            ])
        );
        let parsed = rtao_cli::commands::command_tree()?.try_get_matches_from(&argv)?;
        let (name, leaf) = parsed
            .subcommand()
            .ok_or("address group")?
            .1
            .subcommand()
            .ok_or("add command")?;
        assert_eq!(name, "add");
        assert_eq!(
            leaf.get_one::<String>("name").map(String::as_str),
            Some("contact")
        );
        assert_eq!(
            leaf.get_one::<String>("ss58").map(String::as_str),
            Some(address.as_str())
        );
    }
    for argv in [
        words(&["rtao", "addr"]),
        words(&["rtao", "addr", "contact"]),
        words(&["rtao", "addr", "contact", "invalid"]),
        words(&["rtao", "addr", "-name", &address]),
        words(&["rtao", "addr", "contact", "--json"]),
        words(&["rtao", "--json", "addr", "contact", &address]),
        words(&["rtao", "wallet", "contact", &address]),
        words(&["rtao", "ADDR", "contact", &address]),
    ]
    .into_iter()
    .chain(
        ["add", "save", "list", "show", "remove"]
            .map(|name| words(&["rtao", "addr", name, &address])),
    ) {
        let mut current = argv.clone();
        normalize_legacy_address_argv(&mut current);
        assert_eq!(current, argv);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut argv = words(&["rtao", "addr", "contact", &address]);
        argv[2] = OsString::from_vec(vec![0xff]);
        let before = argv.clone();
        normalize_legacy_address_argv(&mut argv);
        assert_eq!(argv, before);
    }
    Ok(())
}

#[test]
fn legacy_address_shorthand_saves_through_the_real_offline_entrypoint() -> TestResult {
    let address = AccountId([62; 32]).to_string();
    for group in ["addr", "addresses"] {
        let root = Root::new()?;
        let output = root
            .command()
            .args([
                group, "contact", &address, "--note", "saved", "--json", "--quiet",
            ])
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout)?,
            json!({"entry":{"name":"contact","address":address,"note":"saved"},"path":root.0.join("shared-1.json")})
        );
        let stored: Value = serde_json::from_slice(&std::fs::read(root.0.join("shared-1.json"))?)?;
        assert_eq!(
            stored,
            json!([{"name":"contact","address":address,"note":"saved"}])
        );
        let bad = root
            .command()
            .args([group, "another", "invalid", "--json"])
            .output()?;
        assert_eq!(bad.status.code(), Some(2));
        assert!(bad.stdout.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(root.0.join("shared-1.json"))?)?,
            stored
        );
    }
    Ok(())
}

struct FailedWriter;
impl io::Write for FailedWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn legacy_config_warning_keeps_once_only_and_safe_advisory_file_semantics() -> TestResult {
    let root = Root::new()?;
    let current = root.0.join("shared-0.json");
    let mut output = Vec::new();
    warn_legacy_config(&root.0, &current, &mut output)?;
    assert!(output.is_empty());
    assert!(!root.marker().exists());
    root.seed_legacy()?;
    let original = std::fs::read(root.legacy())?;
    assert_eq!(
        warn_legacy_config(&root.0, &current, &mut FailedWriter)
            .err()
            .ok_or("writer failure")?
            .kind(),
        io::ErrorKind::BrokenPipe
    );
    assert!(!root.marker().exists());
    warn_legacy_config(&root.0, &current, &mut output)?;
    assert!(String::from_utf8(output.clone())?.contains("legacy (v9) btcli config"));
    assert!(root.marker().is_file());
    assert_eq!(std::fs::read(root.marker())?, b"");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.marker())?.permissions().mode() & 0o777,
            0o600
        );
    }
    let first = output.clone();
    warn_legacy_config(&root.0, &current, &mut output)?;
    assert_eq!(output, first);
    let nonregular = Root::new()?;
    std::fs::create_dir(nonregular.0.join(".bittensor"))?;
    std::fs::create_dir(nonregular.legacy())?;
    let mut silent = Vec::new();
    warn_legacy_config(&nonregular.0, &current, &mut silent)?;
    assert!(silent.is_empty());
    assert!(!nonregular.marker().exists());
    std::fs::remove_file(root.marker())?;
    std::fs::create_dir(root.marker())?;
    warn_legacy_config(&root.0, &current, &mut output)?;
    assert_eq!(output.len(), first.len() * 2);
    assert_eq!(std::fs::read(root.legacy())?, original);
    assert!(!current.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let safe = Root::new()?;
        safe.seed_legacy()?;
        let target = safe.0.join("preserved");
        std::fs::write(&target, b"preserve")?;
        symlink(&target, safe.marker())?;
        let mut warning = Vec::new();
        warn_legacy_config(&safe.0, Path::new("injected"), &mut warning)?;
        assert!(!warning.is_empty());
        assert_eq!(std::fs::read(&target)?, b"preserve");
        std::fs::remove_file(safe.marker())?;
        std::fs::remove_file(safe.legacy())?;
        symlink(&target, safe.legacy())?;
        warning.clear();
        warn_legacy_config(&safe.0, Path::new("injected"), &mut warning)?;
        assert!(warning.is_empty());
        let linked = Root::new()?;
        symlink(safe.0.join(".bittensor"), linked.0.join(".bittensor"))?;
        warn_legacy_config(&linked.0, Path::new("injected"), &mut warning)?;
        assert!(warning.is_empty());
    }
    Ok(())
}

#[test]
fn real_startup_warns_once_on_stderr_even_for_help_version_json_and_quiet() -> TestResult {
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["config", "get", "network", "--json", "--quiet"],
    ] {
        let root = Root::new()?;
        root.seed_legacy()?;
        let first = root.command().args(&args).output()?;
        assert!(first.status.success());
        let stderr = String::from_utf8(first.stderr)?;
        assert_eq!(stderr.lines().count(), 1);
        assert!(stderr.contains("legacy (v9) btcli config"));
        assert!(stderr.contains(&root.0.join("shared-0.json").display().to_string()));
        assert!(stderr.contains(&format!("{}/migration", rtao_catalog::py::DOCS_URL)));
        assert!(root.marker().is_file());
        if args[0] == "config" {
            assert_eq!(serde_json::from_slice::<Value>(&first.stdout)?, Value::Null);
        }
        if args[0] == "--help" {
            assert!(
                String::from_utf8(first.stdout)?
                    .contains(&format!("Docs: {}", rtao_catalog::py::DOCS_URL))
            );
        }
        let second = root.command().args(&args).output()?;
        assert!(second.status.success());
        assert!(second.stderr.is_empty());
        assert!(!root.0.join("shared-0.json").exists());
    }
    Ok(())
}
