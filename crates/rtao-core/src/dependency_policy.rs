//! Shared dependency provenance checks used by maintenance tools and consumers.

use std::collections::BTreeSet;

/// One package identity read from Cargo.lock.
#[derive(Clone, Copy, Debug)]
pub struct LockedPackage<'a> {
    /// Cargo package name.
    pub name: &'a str,
    /// Resolved package version.
    pub version: &'a str,
    /// Registry or immutable Git source, if present.
    pub source: Option<&'a str>,
    /// Registry source checksum, if present.
    pub checksum: Option<&'a str>,
}

/// The only additional BLS lock entry admitted by PLAN.md Q26.
pub const DORMANT_BLS_VERSION: &str = "0.1.9";
/// Exact registry source for the recorded dormant BLS package.
pub const DORMANT_BLS_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// Checksum of the recorded dormant BLS package.
pub const DORMANT_BLS_CHECKSUM: &str =
    "e6bfb937b3d12077654a9e43e32a4e9c20177dd9fea0f3aba673e7840bb54f32";

/// Require one pinned BLS lock identity and at most the recorded dormant entry.
///
/// The caller supplies the fully resolved core Git source. This validates lock
/// inventory only; [`validate_bls_tree`] must separately prove build provenance.
pub fn validate_bls_lock(packages: &[LockedPackage<'_>], expected: &str) -> Result<(), String> {
    let mut pinned = 0;
    let mut dormant = 0;
    for package in packages.iter().filter(|package| package.name == "w3f-bls") {
        if package.version == "0.1.3" && package.source == Some(expected) {
            pinned += 1;
        } else if package.version == DORMANT_BLS_VERSION
            && package.source == Some(DORMANT_BLS_SOURCE)
            && package.checksum == Some(DORMANT_BLS_CHECKSUM)
        {
            dormant += 1;
        } else {
            return Err(format!(
                "unapproved w3f-bls lock identity: version {} source {:?}; expected pinned 0.1.3 at {expected} or the recorded dormant 0.1.9 checksum",
                package.version, package.source,
            ));
        }
    }
    if pinned != 1 || dormant > 1 {
        return Err(format!(
            "BLS lock inventory requires one pinned identity and at most one recorded dormant entry; found pinned={pinned}, dormant={dormant}",
        ));
    }
    Ok(())
}

/// Require one resolved built BLS package at the full expected core source.
pub fn validate_bls_built(packages: &[LockedPackage<'_>], expected: &str) -> Result<(), String> {
    let built: Vec<_> = packages
        .iter()
        .filter(|package| package.name == "w3f-bls")
        .collect();
    if built.len() != 1 || built[0].version != "0.1.3" || built[0].source != Some(expected) {
        return Err("require exactly one built w3f-bls 0.1.3 at the immutable core pin".into());
    }
    Ok(())
}

/// Require one built BLS identity at the full core pin.
///
/// Input is `cargo tree --prefix none --format '{p}'` output with normal/build
/// edges. Repeated paths to the same package are permitted. Registry BLS,
/// alternate Git locations and commits, and a missing BLS package fail closed.
/// Validate the full lock source first, since Cargo abbreviates displayed hashes.
pub fn validate_bls_tree(output: &str, expected: &str) -> Result<(), String> {
    let expected = expected
        .strip_prefix("git+")
        .ok_or("core source is not Git")?;
    let (location, commit) = expected
        .rsplit_once('#')
        .ok_or("core source has no commit")?;
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("core source must name a full immutable commit".into());
    }
    let mut seen = BTreeSet::new();
    for line in output.lines() {
        let line = line.trim().strip_suffix(" (*)").unwrap_or(line.trim());
        if line.split_whitespace().next() != Some("w3f-bls") {
            continue;
        }
        let source = line
            .strip_prefix("w3f-bls v0.1.3 (")
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| format!("unapproved built BLS implementation: {line}"))?;
        let (actual_location, actual_commit) = source
            .rsplit_once('#')
            .ok_or_else(|| format!("built BLS has no immutable commit: {line}"))?;
        if actual_location != location
            || !(8..=40).contains(&actual_commit.len())
            || !actual_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !commit.starts_with(actual_commit)
        {
            return Err(format!("built BLS differs from core pin: {line}"));
        }
        seen.insert((actual_location, actual_commit));
    }
    if seen.len() != 1 {
        return Err(format!(
            "require exactly one built BLS identity; found {}",
            seen.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const SOURCE: &str = "git+https://github.com/RaoFoundation/subtensor.git?tag=v478#1111111111111111111111111111111111111111";
    const TREE: &str =
        "w3f-bls v0.1.3 (https://github.com/RaoFoundation/subtensor.git?tag=v478#11111111)\n";

    fn pinned() -> LockedPackage<'static> {
        LockedPackage {
            name: "w3f-bls",
            version: "0.1.3",
            source: Some(SOURCE),
            checksum: None,
        }
    }
    fn dormant() -> LockedPackage<'static> {
        LockedPackage {
            name: "w3f-bls",
            version: DORMANT_BLS_VERSION,
            source: Some(DORMANT_BLS_SOURCE),
            checksum: Some(DORMANT_BLS_CHECKSUM),
        }
    }

    #[test]
    fn recorded_dormant_lock_entry_requires_one_pinned_copy() {
        validate_bls_lock(&[pinned()], SOURCE).unwrap();
        validate_bls_lock(&[pinned(), dormant()], SOURCE).unwrap();
        assert!(validate_bls_lock(&[], SOURCE).is_err());
        assert!(validate_bls_lock(&[dormant()], SOURCE).is_err());
        assert!(validate_bls_lock(&[pinned(), pinned()], SOURCE).is_err());
        assert!(validate_bls_lock(&[pinned(), dormant(), dormant()], SOURCE).is_err());
    }

    #[test]
    fn unrecorded_versions_sources_and_checksums_are_rejected() {
        for unexpected in [
            LockedPackage {
                version: "0.1.10",
                ..dormant()
            },
            LockedPackage {
                source: Some(SOURCE),
                ..dormant()
            },
            LockedPackage {
                checksum: None,
                ..dormant()
            },
            LockedPackage {
                checksum: Some("changed"),
                ..dormant()
            },
            LockedPackage {
                source: Some(DORMANT_BLS_SOURCE),
                ..pinned()
            },
        ] {
            assert!(validate_bls_lock(&[pinned(), unexpected], SOURCE).is_err());
        }
    }

    #[test]
    fn dormant_activation_and_missing_primary_are_rejected() {
        validate_bls_built(&[pinned()], SOURCE).unwrap();
        assert!(validate_bls_built(&[pinned(), dormant()], SOURCE).is_err());
        assert!(validate_bls_built(&[dormant()], SOURCE).is_err());
        assert!(validate_bls_built(&[], SOURCE).is_err());
        validate_bls_tree(TREE, SOURCE).unwrap();
        validate_bls_tree(&format!("{TREE}{TREE}"), SOURCE).unwrap();
        validate_bls_tree(&format!("{TREE}{} (*)\n", TREE.trim()), SOURCE).unwrap();
        assert!(validate_bls_tree("", SOURCE).is_err());
        assert!(validate_bls_tree("w3f-bls v0.1.9\n", SOURCE).is_err());
        assert!(validate_bls_tree(&format!("{TREE}w3f-bls v0.1.9\n"), SOURCE).is_err());
    }

    #[test]
    fn alternate_built_source_commit_and_version_are_rejected() {
        for changed in [
            TREE.replace("11111111", "22222222"),
            TREE.replace("RaoFoundation", "sam0x17"),
            TREE.replace("v478", "v479"),
            TREE.replace("v0.1.3", "v0.1.4"),
            TREE.replace("11111111", "1111"),
        ] {
            assert!(validate_bls_tree(&changed, SOURCE).is_err());
        }
    }
}
