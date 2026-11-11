//! Prove the single BLS source and generated provenance match the canonical pin.

use std::{error::Error, fs, path::Path, process::Command};

use rtao_core::dependency_policy::{LockedPackage, validate_bls_lock, validate_bls_tree};
use rtao_core::{
    BITTENSOR_CORE_BRANCH, BITTENSOR_CORE_COMMIT, BITTENSOR_CORE_PATCH_COMMIT,
    BITTENSOR_CORE_PATCH_SOURCE, BITTENSOR_CORE_TAG, UPSTREAM_BLS_VERSION, UPSTREAM_CORE_VERSION,
};
use toml_edit::{DocumentMut, Item};

#[path = "../core_provenance.rs"]
mod core_provenance;

/// The republished core graph (D-183): the core and every package it took from
/// Git resolve from the workspace's upstream/ directory (no source), the
/// monorepo contributes only xtask's linting crate at the base identity, and
/// nothing resolves from the core-patch fork.
fn verify_core_lock_sources(
    lock: &DocumentMut,
    repo: &str,
    pin: &core_provenance::CorePin<'_>,
    core_version: &str,
) -> Result<(), Box<dyn Error>> {
    let base = format!(
        "git+{repo}?{}={}#{}",
        pin.selector, pin.reference, pin.commit
    );
    let packages = lock
        .get("package")
        .and_then(Item::as_array_of_tables)
        .ok_or("locked packages missing")?;
    for (name, expected, version) in [
        ("rtao-upstream-core", None, Some(core_version)),
        ("subtensor-linting", Some(base.as_str()), None),
    ] {
        let matching = packages
            .iter()
            .filter(|package| package.get("name").and_then(Item::as_str) == Some(name))
            .collect::<Vec<_>>();
        if matching.len() != 1
            || matching
                .first()
                .and_then(|package| package.get("source"))
                .and_then(Item::as_str)
                != expected
            || version.is_some_and(|version| {
                matching
                    .first()
                    .and_then(|package| package.get("version"))
                    .and_then(Item::as_str)
                    != Some(version)
            })
        {
            return Err(format!("lock must contain exactly one {name} from {expected:?}").into());
        }
    }
    for package in packages {
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .ok_or("locked name missing")?;
        let Some(source) = package.get("source").and_then(Item::as_str) else {
            continue;
        };
        if [
            "bittensor-core",
            "subtensor-macros",
            "subtensor-hashed",
            "fips204",
            "tle",
        ]
        .contains(&name)
            && source.starts_with("git+")
        {
            return Err(format!("the core graph resolves {name} from Git: {source}").into());
        }
        if source.starts_with(&format!("git+{repo}")) && source != base {
            return Err(
                format!("monorepo source differs from base identity: {name} {source}").into(),
            );
        }
        if source
            .to_ascii_lowercase()
            .contains("github.com/sam0x17/subtensor")
        {
            return Err(format!("unexpected core-patch fork identity: {name} {source}").into());
        }
    }
    Ok(())
}

#[test]
fn single_bls_source_matches_core_pin() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The crate's own copy, from which the constants are built, equals the
    // workspace's record (D-183).
    let pins_text = fs::read_to_string(root.join("upstream.toml"))?;
    assert_eq!(
        pins_text,
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("upstream.toml"))?
    );
    let pins = pins_text.parse::<DocumentMut>()?;
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
    assert_eq!(BITTENSOR_CORE_PATCH_COMMIT, pin.patch_commit);
    assert_eq!(BITTENSOR_CORE_PATCH_SOURCE, pin.patch_source);
    let (core_version, bls_version) = core_provenance::republished(&pins)?;
    assert_eq!(UPSTREAM_CORE_VERSION, core_version);
    assert_eq!(UPSTREAM_BLS_VERSION, bls_version);
    if !pin.branch.is_empty() {
        // A disposable canary builds upstream's packages from Git; its graph
        // is checked by `cargo xtask pin --check` in that checkout.
        return Ok(());
    }

    let lock = fs::read_to_string(root.join("Cargo.lock"))?.parse::<DocumentMut>()?;
    verify_core_lock_sources(&lock, repo, &pin, core_version)?;
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
    validate_bls_lock(&identities, bls_version)?;
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
    validate_bls_tree(&String::from_utf8(tree.stdout)?, bls_version)?;

    // The published graph builds without any [patch].
    let manifest = fs::read_to_string(root.join("Cargo.toml"))?.parse::<DocumentMut>()?;
    assert!(manifest.get("patch").is_none(), "the root manifest patches");
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
    assert_eq!(pin.patch_commit, "");
    assert_eq!(pin.patch_source, "");
    assert_eq!(pins["core"]["tag"].as_str(), Some("v478"));
    pins["core"]["canary"]["commit"] = toml_edit::value("main");
    assert!(core_provenance::effective(&pins).is_err());
    Ok(())
}

fn patched_fixture() -> Result<DocumentMut, Box<dyn Error>> {
    Ok("[core]\ntag='v478'\ncommit='1111111111111111111111111111111111111111'\n[core.patch]\nrepo='https://github.com/sam0x17/subtensor.git'\nbranch='rtao-core-patch/v478'\nbase='1111111111111111111111111111111111111111'\nrev='2222222222222222222222222222222222222222'\nupstream_pr='https://github.com/RaoFoundation/subtensor/pull/3242'\nupstream_pr_rev='2222222222222222222222222222222222222222'\nsecurity_issue='https://github.com/RaoFoundation/subtensor/issues/3251'\nreview_base='1111111111111111111111111111111111111111'\nreview_rev='2222222222222222222222222222222222222222'\nreview_diff_sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\npaths=['sdk/bittensor-core/src/signers/mod.rs','sdk/bittensor-core/src/signers/ledger.rs','sdk/bittensor-core/src/signers/hid.rs','sdk/bittensor-core/Cargo.toml','sdk/bittensor-core/src/keys/mod.rs','sdk/bittensor-core/src/keys/encrypted_json.rs','sdk/bittensor-core/src/keys/hashed.rs']\nprobe=['bittensor_core::signers::ApduTransport','bittensor_core::signers::ledger::LedgerDevice::with_transport']\n".parse()?)
}

#[test]
fn patch_provenance_preserves_base_identity_and_names_the_actual_core_source()
-> Result<(), Box<dyn Error>> {
    let pins = patched_fixture()?;
    let pin = core_provenance::effective(&pins)?;
    assert_eq!(pin.tag, "v478");
    assert_eq!(pin.branch, "");
    assert_eq!(pin.commit, "1111111111111111111111111111111111111111");
    assert_eq!((pin.selector, pin.reference), ("tag", "v478"));
    assert_eq!(pin.patch_commit, "2222222222222222222222222222222222222222");
    assert_eq!(
        pin.patch_source,
        "git+https://github.com/sam0x17/subtensor.git?rev=2222222222222222222222222222222222222222#2222222222222222222222222222222222222222"
    );
    let mut without_patch = pins.clone();
    without_patch["core"]
        .as_table_like_mut()
        .ok_or("fixture core table missing")?
        .remove("patch");
    let without_patch = core_provenance::effective(&without_patch)?;
    assert_eq!(without_patch.patch_commit, "");
    assert_eq!(without_patch.patch_source, "");
    Ok(())
}

#[test]
fn patch_provenance_rejects_alternate_sources_and_incomplete_identities()
-> Result<(), Box<dyn Error>> {
    let pins = patched_fixture()?;
    for (field, changed) in [
        ("repo", "https://example.invalid/fork.git"),
        ("branch", "rtao-core-patch/v477"),
        ("base", "3333333333333333333333333333333333333333"),
        ("rev", "main"),
        ("rev", "1111111111111111111111111111111111111111"),
        (
            "upstream_pr",
            "https://github.com/RaoFoundation/subtensor/pull/1",
        ),
        ("upstream_pr_rev", "main"),
        (
            "security_issue",
            "https://github.com/RaoFoundation/subtensor/issues/1",
        ),
        ("review_base", "v478"),
        ("review_rev", "main"),
        ("review_diff_sha256", "abc"),
        ("merge_commit", "main"),
    ] {
        let mut changed_pins = pins.clone();
        changed_pins["core"]["patch"][field] = toml_edit::value(changed);
        assert!(
            core_provenance::effective(&changed_pins).is_err(),
            "accepted invalid patch field {field}"
        );
    }
    for field in [
        "repo",
        "branch",
        "base",
        "rev",
        "upstream_pr",
        "upstream_pr_rev",
        "security_issue",
        "review_base",
        "review_rev",
        "review_diff_sha256",
        "paths",
        "probe",
    ] {
        let mut changed = pins.clone();
        changed["core"]["patch"]
            .as_table_like_mut()
            .ok_or("fixture patch missing")?
            .remove(field);
        assert!(
            core_provenance::effective(&changed).is_err(),
            "accepted missing patch field {field}"
        );
    }
    for field in ["paths", "probe"] {
        let mut changed = pins.clone();
        let mut values = toml_edit::Array::new();
        values.push("unexpected");
        changed["core"]["patch"][field] = toml_edit::value(values);
        assert!(core_provenance::effective(&changed).is_err());
    }
    let mut canary = pins;
    canary["core"]["canary"] = toml_edit::Item::Table(toml_edit::Table::new());
    assert!(core_provenance::effective(&canary).is_err());
    Ok(())
}

#[test]
fn republished_core_lock_excludes_git_core_graph_and_the_fork() -> Result<(), Box<dyn Error>> {
    let repo = "https://github.com/RaoFoundation/subtensor.git";
    let pins = patched_fixture()?;
    let pin = core_provenance::effective(&pins)?;
    let base = format!("git+{repo}?tag=v478#{}", pin.commit);
    let lock = format!(
        "[[package]]\nname='rtao-upstream-core'\nversion='0.478.0'\n[[package]]\nname='subtensor-linting'\nsource='{base}'\n"
    );
    verify_core_lock_sources(&lock.parse()?, repo, &pin, "0.478.0")?;
    assert!(verify_core_lock_sources(&lock.parse()?, repo, &pin, "0.478.1").is_err());
    for changed in [
        lock.replace("0.478.0", "0.477.0"),
        format!("{lock}\n[[package]]\nname='rtao-upstream-core'\nversion='0.478.0'\n"),
        format!(
            "{lock}\n[[package]]\nname='bittensor-core'\nsource='{}'\n",
            pin.patch_source
        ),
        format!("{lock}\n[[package]]\nname='subtensor-macros'\nsource='{base}'\n"),
        format!(
            "{lock}\n[[package]]\nname='unexpected'\nsource='{}'\n",
            pin.patch_source.replace("sam0x17", "Sam0x17")
        ),
        lock.replace(&base, &base.replace("v478", "v477")),
    ] {
        assert!(verify_core_lock_sources(&changed.parse()?, repo, &pin, "0.478.0").is_err());
    }
    Ok(())
}
