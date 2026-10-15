//! Exercise the real CLI entry point without network or native password access.
#![allow(clippy::unwrap_used)]
use rtao::client::config::ConfigFile;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Result<Self, std::io::Error> {
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-entrypoint-tests")
            });
        let path = base.join("tmp").join(format!(
            "entrypoint-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
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
        command
            .env_remove("BT_NETWORK")
            .env_remove("BT_WALLET")
            .env_remove("BT_WALLET_HOTKEY")
            .env_remove("BT_RETRY_FOREVER");
        command
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn help_version_and_missing_raw_target_have_real_statuses() -> Result<(), Box<dyn std::error::Error>>
{
    let root = Root::new()?;
    let help = root.command().arg("--help").output()?;
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout)?;
    assert!(text.contains("doctor"));
    assert!(text.contains("call"));
    assert!(!text.contains("allow-raw"));
    let version = root.command().arg("--version").output()?;
    assert!(version.status.success());
    assert!(String::from_utf8(version.stdout)?.contains(rtao::core::BITTENSOR_CORE_TAG));
    let missing = root.command().args(["call", "--json"]).output()?;
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert!(
        String::from_utf8(missing.stderr)?
            .to_ascii_lowercase()
            .contains("target")
    );
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn m5_families_and_hidden_aliases_are_registered_by_the_real_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = Root::new()?;
    for (args, expected) in [
        (
            vec!["stake", "--help"],
            vec!["move-swap", "transfer", "burn"],
        ),
        (
            vec!["st", "add", "--help"],
            vec!["--amount-tao", "--hotkey"],
        ),
        (vec!["proxy", "--help"], vec!["create", "execute", "kill"]),
        (
            vec!["multisig", "--help"],
            vec!["pending", "show", "remove"],
        ),
        (
            vec!["hparams", "set-take", "--help"],
            vec!["--take", "--hotkey"],
        ),
        (
            vec!["hparams", "buyback", "--help"],
            vec!["--limit-price", "--amount"],
        ),
    ] {
        let output = root.command().args(&args).output()?;
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout)?;
        for fragment in expected {
            assert!(text.contains(fragment), "{args:?} lacks {fragment}: {text}");
        }
    }
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}
#[test]
fn invalid_raw_role_is_a_framework_diagnostic_and_never_connects()
-> Result<(), Box<dyn std::error::Error>> {
    let root = Root::new()?;
    let result = root
        .command()
        .args([
            "call",
            "System.remark",
            "--signer",
            "invalid-role",
            "--json",
        ])
        .output()?;
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    let diagnostic = String::from_utf8(result.stderr)?;
    assert!(diagnostic.contains("invalid value"));
    assert!(diagnostic.contains("--signer"));
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn doctor_verifies_the_canonical_workspace_through_the_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = Root::new()?;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../Cargo.toml")
        .canonicalize()?;
    let output = root
        .command()
        .current_dir(&root.0)
        .env("CARGO_NET_OFFLINE", "true")
        .env("CARGO_TERM_COLOR", "always")
        .args(["doctor", "--manifest-path"])
        .arg(&manifest)
        .output()?;
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "Core pin and downstream BLS patch verified for {}\n",
            manifest.display()
        )
    );
    assert!(output.stderr.is_empty());
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[test]
fn doctor_missing_patch_fails_with_the_exact_required_patch_through_the_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = Root::new()?;
    let tag = rtao::core::BITTENSOR_CORE_TAG;
    let repository = "https://github.com/RaoFoundation/subtensor.git";
    let manifest = root.0.join("Cargo.toml");
    std::fs::write(
        &manifest,
        format!(
            "[workspace]\nmembers = [\"consumer\"]\nresolver = \"3\"\n\
             [workspace.dependencies]\n\
             bittensor-core = {{ git = {repository:?}, tag = {tag:?}, default-features = false }}\n"
        ),
    )?;
    std::fs::create_dir_all(root.0.join("consumer/src"))?;
    std::fs::write(
        root.0.join("consumer/Cargo.toml"),
        "[package]\nname = \"doctor-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )?;
    std::fs::write(root.0.join("consumer/src/lib.rs"), "")?;
    std::fs::write(
        root.0.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"doctor-consumer\"\nversion = \"0.0.0\"\n",
    )?;

    // The unused declaration supplies the pin without fetching upstream code.
    // Doctor must reject the root patch before inspecting the consumer graph.
    let output = root
        .command()
        .current_dir(&root.0)
        .env("CARGO_NET_OFFLINE", "true")
        .args(["doctor", "--manifest-path"])
        .arg(&manifest)
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr)?;
    let expected_patch =
        format!("[patch.crates-io]\nw3f-bls = {{ git = {repository:?}, tag = {tag:?} }}");
    assert!(diagnostic.contains("the final workspace needs this exact core-matching patch"));
    assert!(diagnostic.contains(&expected_patch));
    assert!(diagnostic.contains("root Cargo.toml"));
    Ok(())
}
