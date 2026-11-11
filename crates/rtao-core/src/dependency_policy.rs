//! Shared dependency provenance checks used by maintenance tools and consumers.
//!
//! The core takes the chain's BLS fork (the monorepo's vendored `w3f-bls`)
//! through rtao's republished package `rtao-upstream-w3f-bls` (D-183). A consumer's
//! graph must build exactly that package, at the version this rtao revision
//! records, and no other `w3f-bls`.

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

/// Package name of rtao's republish of the chain's BLS fork.
pub const BLS_PACKAGE: &str = "rtao-upstream-w3f-bls";
/// Package name of rtao's republish of the core.
pub const CORE_PACKAGE: &str = "rtao-upstream-core";
/// Cargo's source identity of crates.io.
pub const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// The only `w3f-bls` lock entry admitted by PLAN.md Q26: an optional
/// dependency of `sp-core` that no build enables.
pub const DORMANT_BLS_VERSION: &str = "0.1.9";
/// Exact registry source for the recorded dormant BLS package.
pub const DORMANT_BLS_SOURCE: &str = CRATES_IO;
/// Checksum of the recorded dormant BLS package.
pub const DORMANT_BLS_CHECKSUM: &str =
    "e6bfb937b3d12077654a9e43e32a4e9c20177dd9fea0f3aba673e7840bb54f32";

fn republished_source(source: Option<&str>) -> bool {
    // A workspace path (rtao's own checkout, or a project that depends on it
    // by path or Git) or crates.io.
    source.is_none_or(|source| source == CRATES_IO || source.starts_with("git+"))
}

/// Require one republished BLS lock identity at `expected` and at most the
/// recorded dormant `w3f-bls` entry.
///
/// This validates lock inventory only; [`validate_bls_tree`] must separately
/// prove what is built.
pub fn validate_bls_lock(packages: &[LockedPackage<'_>], expected: &str) -> Result<(), String> {
    let mut republished = 0;
    let mut dormant = 0;
    for package in packages {
        if package.name == BLS_PACKAGE {
            if package.version != expected || !republished_source(package.source) {
                return Err(format!(
                    "unapproved {BLS_PACKAGE} lock identity: version {} source {:?}; expected {expected}",
                    package.version, package.source,
                ));
            }
            republished += 1;
        } else if package.name == "w3f-bls" {
            if package.version == DORMANT_BLS_VERSION
                && package.source == Some(DORMANT_BLS_SOURCE)
                && package.checksum == Some(DORMANT_BLS_CHECKSUM)
            {
                dormant += 1;
            } else {
                return Err(format!(
                    "unapproved w3f-bls lock identity: version {} source {:?}; the core builds {BLS_PACKAGE} {expected}, and only the recorded dormant w3f-bls 0.1.9 may be locked",
                    package.version, package.source,
                ));
            }
        }
    }
    if republished != 1 || dormant > 1 {
        return Err(format!(
            "BLS lock inventory requires one {BLS_PACKAGE} and at most one recorded dormant w3f-bls; found {republished} and {dormant}",
        ));
    }
    Ok(())
}

/// Require exactly one built BLS package, the republished one at `expected`.
pub fn validate_bls_built(packages: &[LockedPackage<'_>], expected: &str) -> Result<(), String> {
    let built: Vec<_> = packages
        .iter()
        .filter(|package| package.name == "w3f-bls" || package.name == BLS_PACKAGE)
        .collect();
    if built.len() != 1
        || built[0].name != BLS_PACKAGE
        || built[0].version != expected
        || !republished_source(built[0].source)
    {
        return Err(format!(
            "require exactly one built {BLS_PACKAGE} {expected} and no w3f-bls"
        ));
    }
    Ok(())
}

/// Require one built BLS identity, the republished package at `expected`.
///
/// Input is `cargo tree --prefix none --format '{p}'` output with normal/build
/// edges. Repeated paths to the same package are permitted. Any `w3f-bls`
/// (registry, Git or path), another version or a missing BLS package fails.
pub fn validate_bls_tree(output: &str, expected: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let version = format!("v{expected}");
    for line in output.lines() {
        let line = line.trim().strip_suffix(" (*)").unwrap_or(line.trim());
        let mut words = line.split_whitespace();
        match words.next() {
            Some("w3f-bls") => return Err(format!("unapproved built BLS implementation: {line}")),
            Some(BLS_PACKAGE) => {
                if words.next() != Some(version.as_str()) {
                    return Err(format!(
                        "built BLS differs from {BLS_PACKAGE} {expected}: {line}"
                    ));
                }
                seen.insert(line);
            }
            _ => {}
        }
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

    const VERSION: &str = "0.490.0";
    const TREE: &str = "rtao-upstream-w3f-bls v0.490.0\n";

    fn republished() -> LockedPackage<'static> {
        LockedPackage {
            name: BLS_PACKAGE,
            version: VERSION,
            source: Some(CRATES_IO),
            checksum: Some("checksum"),
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
    fn recorded_dormant_lock_entry_requires_one_republished_copy() {
        validate_bls_lock(&[republished()], VERSION).unwrap();
        validate_bls_lock(
            &[
                LockedPackage {
                    source: None,
                    ..republished()
                },
                dormant(),
            ],
            VERSION,
        )
        .unwrap();
        assert!(validate_bls_lock(&[], VERSION).is_err());
        assert!(validate_bls_lock(&[dormant()], VERSION).is_err());
        assert!(validate_bls_lock(&[republished(), republished()], VERSION).is_err());
        assert!(validate_bls_lock(&[republished(), dormant(), dormant()], VERSION).is_err());
    }

    #[test]
    fn unrecorded_versions_sources_and_checksums_are_rejected() {
        for unexpected in [
            LockedPackage {
                version: "0.1.10",
                ..dormant()
            },
            LockedPackage {
                source: Some("git+https://github.com/RaoFoundation/subtensor.git?tag=v490#1"),
                version: "0.1.3",
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
        ] {
            assert!(validate_bls_lock(&[republished(), unexpected], VERSION).is_err());
        }
        for changed in [
            LockedPackage {
                version: "0.490.1",
                ..republished()
            },
            LockedPackage {
                source: Some("registry+https://example.invalid/index"),
                ..republished()
            },
        ] {
            assert!(validate_bls_lock(&[changed], VERSION).is_err());
        }
    }

    #[test]
    fn dormant_activation_and_missing_primary_are_rejected() {
        validate_bls_built(&[republished()], VERSION).unwrap();
        assert!(validate_bls_built(&[republished(), dormant()], VERSION).is_err());
        assert!(validate_bls_built(&[dormant()], VERSION).is_err());
        assert!(validate_bls_built(&[], VERSION).is_err());
        validate_bls_tree(TREE, VERSION).unwrap();
        validate_bls_tree(&format!("{TREE}{TREE}"), VERSION).unwrap();
        validate_bls_tree(&format!("{TREE}{} (*)\n", TREE.trim()), VERSION).unwrap();
        validate_bls_tree(
            "rtao-upstream-w3f-bls v0.490.0 (/work/rtao/upstream/w3f-bls)\n",
            VERSION,
        )
        .unwrap();
        assert!(validate_bls_tree("", VERSION).is_err());
        assert!(validate_bls_tree("w3f-bls v0.1.9\n", VERSION).is_err());
        assert!(validate_bls_tree(&format!("{TREE}w3f-bls v0.1.9\n"), VERSION).is_err());
    }

    #[test]
    fn alternate_built_versions_and_sources_are_rejected() {
        for changed in [
            TREE.replace("v0.490.0", "v0.490.1"),
            format!("{TREE}rtao-upstream-w3f-bls v0.490.0 (/elsewhere)\n"),
            "w3f-bls v0.1.3 (https://github.com/RaoFoundation/subtensor.git?tag=v490#d967d135)\n"
                .to_owned(),
        ] {
            assert!(validate_bls_tree(&changed, VERSION).is_err());
        }
    }
}
