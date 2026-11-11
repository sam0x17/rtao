//! Copies of workspace files that published crates read at build time.
//!
//! A `.crate` holds only its own directory, so a crate cannot `include_str!`,
//! `include_bytes!` or read in its build script a file above it (D-183). Each
//! such input is copied into the crate that reads it. The workspace file stays
//! the source: `cargo xtask crate-inputs` refreshes the copies and
//! `cargo xtask crate-inputs --check` (and the test below) fails when one
//! differs. Inputs that only tests read stay where they are, since tests do
//! not run from a downloaded crate.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

/// Single files: `(source, copy)`, relative to the workspace root.
pub(crate) const FILES: [(&str, &str); 7] = [
    // rtao-core's build script records the core pin in `rtao_core::*`.
    ("upstream.toml", "crates/rtao-core/upstream.toml"),
    // rtao-testkit's `Localnet::start` runs the pinned image.
    ("upstream.toml", "crates/rtao-testkit/upstream.toml"),
    // rtao-testkit's `FakeState::load_python_defaults`.
    (
        "vendor/upstream/extracted/named_tables.json",
        "crates/rtao-testkit/named_tables.json",
    ),
    // rtao-client's compiled compatibility report.
    ("compat.json", "crates/rtao-client/compat.json"),
    // rtao-signer's vault and extension pages.
    (
        "vendor/upstream/vault/static/vault.html",
        "crates/rtao-signer/static/vault.html",
    ),
    (
        "vendor/upstream/vault/static/bittensor-chain-specs.png",
        "crates/rtao-signer/static/bittensor-chain-specs.png",
    ),
    (
        "vendor/upstream/extension/static/bridge.html",
        "crates/rtao-signer/static/bridge.html",
    ),
];

/// Directories copied file for file (no subdirectories): `(source, copy)`.
pub(crate) const DIRECTORIES: [(&str, &str); 1] = [
    // rtao-evm's precompile ABIs.
    ("vendor/upstream/evm/abi", "crates/rtao-evm/abi"),
];

/// Every `(source, copy)` file pair.
fn pairs(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut pairs: Vec<_> = FILES
        .iter()
        .map(|(source, copy)| (root.join(source), root.join(copy)))
        .collect();
    for (source, copy) in DIRECTORIES {
        let mut names = Vec::new();
        for entry in fs::read_dir(root.join(source)).with_context(|| format!("read {source}"))? {
            let entry = entry?;
            ensure!(
                entry.file_type()?.is_file(),
                "{source} must hold only files"
            );
            names.push(entry.file_name());
        }
        names.sort();
        for name in names {
            pairs.push((root.join(source).join(&name), root.join(copy).join(&name)));
        }
    }
    Ok(pairs)
}

/// Files in copied directories that no longer have a source.
fn stale(root: &Path) -> Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    for (source, copy) in DIRECTORIES {
        let copy = root.join(copy);
        if !copy.exists() {
            continue;
        }
        for entry in fs::read_dir(&copy)? {
            let entry = entry?;
            if !root.join(source).join(entry.file_name()).is_file() {
                stale.push(entry.path());
            }
        }
    }
    Ok(stale)
}

/// Refresh every copy from its source. A root without rtao's crates (a test
/// fixture) has nothing to refresh.
pub(crate) fn sync(root: &Path) -> Result<()> {
    if !FILES
        .iter()
        .chain(DIRECTORIES.iter())
        .all(|(source, _)| root.join(source).exists())
        || !root.join("crates/rtao-core").is_dir()
    {
        return Ok(());
    }
    for (source, copy) in pairs(root)? {
        let bytes = fs::read(&source).with_context(|| format!("read {}", source.display()))?;
        if fs::read(&copy).ok().as_deref() != Some(bytes.as_slice()) {
            fs::create_dir_all(copy.parent().context("copy has no parent")?)?;
            fs::write(&copy, bytes)?;
        }
    }
    for path in stale(root)? {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Fail when a copy differs from its source.
pub(crate) fn check(root: &Path) -> Result<()> {
    let mut differing = BTreeMap::new();
    for (source, copy) in pairs(root)? {
        let expected = fs::read(&source).with_context(|| format!("read {}", source.display()))?;
        if fs::read(&copy).ok().as_deref() != Some(expected.as_slice()) {
            differing.insert(copy, source);
        }
    }
    for path in stale(root)? {
        differing.insert(path, PathBuf::from("(no source)"));
    }
    ensure!(
        differing.is_empty(),
        "crate input copies differ from their workspace sources; run cargo xtask crate-inputs:\n{}",
        differing
            .iter()
            .map(|(copy, source)| format!("  {} <- {}", copy.display(), source.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

pub(crate) fn run(arguments: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    match arguments.as_slice() {
        [] => {
            sync(root)?;
            println!("crate input copies refreshed");
        }
        [flag] if flag == "--check" => {
            check(root)?;
            println!("crate input copies equal their workspace sources");
        }
        _ => bail!("usage: cargo xtask crate-inputs [--check]"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_input_copies_equal_their_sources() -> Result<()> {
        check(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .context("workspace root missing")?,
        )
    }
}
