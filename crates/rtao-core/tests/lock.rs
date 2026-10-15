//! Prove the single BLS source and generated provenance match the canonical pin.

use std::{error::Error, fs, path::Path, process::Command};

use rtao_core::dependency_policy::{LockedPackage, validate_bls_lock, validate_bls_tree};
use rtao_core::{BITTENSOR_CORE_BRANCH, BITTENSOR_CORE_COMMIT, BITTENSOR_CORE_TAG};
use toml_edit::{DocumentMut, Item};

#[path = "../core_provenance.rs"]
mod core_provenance;

#[test]
fn single_bls_source_matches_core_pin() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let pin = core_provenance::effective(&pins)?;
    if !pin.branch.is_empty() {
        core_provenance::require_disposable(&root)?;
    }
    let monorepo = pins.get("monorepo").ok_or("missing [monorepo]")?;
    let repo = monorepo
        .get("url")
        .and_then(Item::as_str)
        .ok_or("missing monorepo.url")?;
    assert_eq!(BITTENSOR_CORE_TAG, pin.tag);
    assert_eq!(BITTENSOR_CORE_BRANCH, pin.branch);
    assert_eq!(BITTENSOR_CORE_COMMIT, pin.commit);
    let expected = format!(
        "git+{repo}?{}={}#{}",
        pin.selector, pin.reference, pin.commit
    );

    let lock = fs::read_to_string(root.join("Cargo.lock"))?.parse::<DocumentMut>()?;
    let packages = lock
        .get("package")
        .and_then(Item::as_array_of_tables)
        .ok_or("missing locked packages")?;
    let identities: Vec<_> = packages
        .iter()
        .map(|package| {
            Ok::<_, Box<dyn Error>>(LockedPackage {
                name: package
                    .get("name")
                    .and_then(Item::as_str)
                    .ok_or("missing locked name")?,
                version: package
                    .get("version")
                    .and_then(Item::as_str)
                    .ok_or("missing locked version")?,
                source: package.get("source").and_then(Item::as_str),
                checksum: package.get("checksum").and_then(Item::as_str),
            })
        })
        .collect::<Result<_, _>>()?;
    validate_bls_lock(&identities, &expected)?;
    let tree = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(&root)
        .args([
            "tree",
            "--locked",
            "--offline",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .env("CARGO_TERM_COLOR", "never")
        .output()?;
    assert!(
        tree.status.success(),
        "BLS built graph inspection failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    validate_bls_tree(&String::from_utf8(tree.stdout)?, &expected)?;

    if !pin.branch.is_empty() {
        for name in ["bittensor-core", "subtensor-macros", "subtensor-linting"] {
            let matching: Vec<_> = packages
                .iter()
                .filter(|package| package.get("name").and_then(Item::as_str) == Some(name))
                .collect();
            assert_eq!(matching.len(), 1, "canary must resolve exactly one {name}");
            assert_eq!(
                matching[0].get("source").and_then(Item::as_str),
                Some(expected.as_str())
            );
        }
        for package in packages {
            if let Some(source) = package.get("source").and_then(Item::as_str) {
                if source.starts_with(&format!("git+{repo}")) {
                    assert_eq!(
                        source, expected,
                        "all monorepo packages must use the effective canary commit"
                    );
                }
                assert!(
                    !source.starts_with("git+https://github.com/sam0x17/subtensor.git"),
                    "a canary cannot retain the core-patch fork"
                );
            }
        }
    }

    let manifest = fs::read_to_string(root.join("Cargo.toml"))?.parse::<DocumentMut>()?;
    let patch = manifest
        .get("patch")
        .and_then(|patch| patch.get("crates-io"))
        .and_then(|patch| patch.get("w3f-bls"))
        .ok_or("missing downstream BLS patch")?;
    assert_eq!(patch.get("git").and_then(Item::as_str), Some(repo));
    assert_eq!(
        patch.get(pin.selector).and_then(Item::as_str),
        Some(pin.reference)
    );
    if !pin.branch.is_empty() {
        assert!(manifest["patch"].get(repo).is_none());
        assert!(
            manifest["patch"]
                .get("https://github.com/sam0x17/subtensor.git")
                .is_none()
        );
        for selector in ["tag", "branch", "path"] {
            assert!(
                patch.get(selector).is_none(),
                "BLS canary patch must use only rev"
            );
        }
    }
    Ok(())
}

#[test]
fn canary_provenance_identifies_effective_commit_without_claiming_a_release()
-> Result<(), Box<dyn Error>> {
    let mut pins: DocumentMut = "[core]\ntag='v478'\ncommit='1111111111111111111111111111111111111111'\n[core.canary]\nbranch='main'\ncommit='2222222222222222222222222222222222222222'\ntoolchain='1.89'\nfingerprint='b3:3333333333333333333333333333333333333333333333333333333333333333'\n".parse()?;
    let pin = core_provenance::effective(&pins)?;
    assert_eq!(pin.tag, "canary:main");
    assert_eq!(pin.branch, "main");
    assert_eq!(pin.commit, "2222222222222222222222222222222222222222");
    assert_eq!(pin.selector, "rev");
    assert_eq!(pins["core"]["tag"].as_str(), Some("v478"));
    pins["core"]["canary"]["commit"] = toml_edit::value("main");
    assert!(core_provenance::effective(&pins).is_err());
    Ok(())
}
