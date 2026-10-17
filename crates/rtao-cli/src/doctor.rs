//! Offline checks for final workspaces consuming the SDK.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value as Json;
use toml_edit::{DocumentMut, Item};

use rtao::core::dependency_policy::{LockedPackage, validate_bls_lock, validate_bls_tree};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GitPin {
    repository: String,
    reference: Option<(String, String)>,
}

impl GitPin {
    fn from_dependency(dependency: &Item) -> Result<Self, String> {
        let repository = dependency
            .get("git")
            .and_then(Item::as_str)
            .ok_or("bittensor-core must declare a git repository")?
            .to_owned();
        let references = ["tag", "rev", "branch"]
            .iter()
            .filter_map(|kind| dependency.get(kind).map(|value| (*kind, value)))
            .map(|(kind, value)| {
                value
                    .as_str()
                    .map(|value| (kind.to_owned(), value.to_owned()))
                    .ok_or_else(|| format!("git dependency {kind} must be a string"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if references.len() > 1 || dependency.get("path").is_some() {
            return Err("git dependency must use one reference kind and no path".into());
        }
        Ok(Self {
            repository,
            reference: references.into_iter().next(),
        })
    }

    fn from_source(source: &str) -> Result<Self, String> {
        let source = source
            .strip_prefix("git+")
            .ok_or("bittensor-core must use a git source")?;
        let source = source.split('#').next().ok_or("empty git source")?;
        let (repository, reference) = match source.rsplit_once('?') {
            Some((repository, query)) => {
                let (kind, value) = query
                    .split_once('=')
                    .ok_or("git source has no reference value")?;
                if !["tag", "rev", "branch"].contains(&kind) || value.contains('&') {
                    return Err(format!("unsupported git source reference {query:?}"));
                }
                (
                    repository,
                    Some((kind.to_owned(), decode_reference(value)?)),
                )
            }
            None => (source, None),
        };
        Ok(Self {
            repository: repository.to_owned(),
            reference,
        })
    }

    fn patch_line(&self) -> String {
        let reference = self
            .reference
            .as_ref()
            .map(|(kind, value)| format!(", {kind} = {}", toml_edit::Value::from(value.as_str())))
            .unwrap_or_default();
        format!(
            "[patch.crates-io]\nw3f-bls = {{ git = {}{reference} }}",
            toml_edit::Value::from(self.repository.as_str())
        )
    }

    fn warning(&self) -> Option<String> {
        let binary_tag = rtao::core::BITTENSOR_CORE_TAG;
        if self
            .reference
            .as_ref()
            .is_some_and(|(kind, value)| kind == "tag" && value == binary_tag)
        {
            None
        } else {
            let reference = self
                .reference
                .as_ref()
                .map(|(kind, value)| format!("{kind} = {}", toml_edit::Value::from(value.as_str())))
                .unwrap_or_else(|| "default git branch".into());
            Some(format!(
                "workspace bittensor-core {reference} differs from this rtao binary's tag {binary_tag}; the workspace's own pin was verified"
            ))
        }
    }
}

fn decode_reference(value: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes
                .next()
                .and_then(|digit| char::from(digit).to_digit(16));
            let low = bytes
                .next()
                .and_then(|digit| char::from(digit).to_digit(16));
            match (high, low) {
                (Some(high), Some(low)) => decoded.push((high * 16 + low) as u8),
                _ => return Err("git reference contains invalid percent encoding".into()),
            }
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).map_err(|error| format!("git reference is not UTF-8: {error}"))
}

fn document(path: &Path) -> Result<DocumentMut, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .parse()
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn cargo_output(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("run Cargo to inspect the workspace: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cargo could not inspect the workspace offline:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("Cargo output is not UTF-8: {error}"))
}

/// Ask Cargo to identify the package selected by the current directory.
pub(super) fn find_manifest(directory: &Path) -> Result<PathBuf, String> {
    let output = cargo_output(Command::new("cargo").current_dir(directory).args([
        "locate-project",
        "--offline",
        "--message-format",
        "plain",
    ]))?;
    Ok(PathBuf::from(output.trim()))
}

fn workspace_manifest(manifest: &Path) -> Result<PathBuf, String> {
    if manifest.file_name().is_none_or(|name| name != "Cargo.toml") {
        return Err("--manifest-path must name Cargo.toml".into());
    }
    let output = cargo_output(
        Command::new("cargo")
            .args([
                "locate-project",
                "--workspace",
                "--offline",
                "--message-format",
                "plain",
                "--manifest-path",
            ])
            .arg(manifest),
    )?;
    Ok(PathBuf::from(output.trim()))
}

fn metadata(manifest: &Path, no_deps: bool) -> Result<Json, String> {
    let mut command = Command::new("cargo");
    command.current_dir(
        manifest
            .parent()
            .ok_or("Cargo.toml has no parent directory")?,
    );
    command
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest);
    if no_deps {
        command.arg("--no-deps");
    }
    serde_json::from_str(&cargo_output(&mut command)?)
        .map_err(|error| format!("parse Cargo workspace metadata: {error}"))
}

fn metadata_pins(metadata: &Json) -> Result<BTreeSet<GitPin>, String> {
    let packages = metadata
        .get("packages")
        .and_then(Json::as_array)
        .ok_or("Cargo metadata has no packages")?;
    let mut pins = BTreeSet::new();
    for package in packages {
        let dependencies = package
            .get("dependencies")
            .and_then(Json::as_array)
            .ok_or("Cargo package metadata has no dependencies")?;
        for dependency in dependencies {
            if dependency.get("name").and_then(Json::as_str) == Some("bittensor-core") {
                let source = dependency
                    .get("source")
                    .and_then(Json::as_str)
                    .ok_or("bittensor-core declaration must use a git source")?;
                pins.insert(GitPin::from_source(source)?);
            }
        }
    }
    Ok(pins)
}

fn expected_pin(manifest_path: &Path, manifest: &DocumentMut) -> Result<GitPin, String> {
    let mut pins = metadata_pins(&metadata(manifest_path, true)?)?;
    if let Some(dependencies) = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table_like)
    {
        for (name, dependency) in dependencies.iter() {
            if dependency
                .get("package")
                .and_then(Item::as_str)
                .unwrap_or(name)
                == "bittensor-core"
            {
                pins.insert(GitPin::from_dependency(dependency)?);
            }
        }
    }
    if pins.is_empty() {
        // A subnet may depend only on the facade. Full metadata finds the
        // declaration in rtao-core without compiling the downstream program.
        pins = metadata_pins(&metadata(manifest_path, false).map_err(|error| format!("no workspace bittensor-core declaration was found; inspecting the transitive declaration requires dependencies cached for offline Cargo metadata:\n{error}"))?)?;
    }
    if pins.len() != 1 {
        return Err(format!(
            "workspace must declare exactly one bittensor-core git pin; found {} distinct pins",
            pins.len()
        ));
    }
    pins.into_iter()
        .next()
        .ok_or("workspace has no bittensor-core declaration".into())
}

fn verify_manifest(manifest: &DocumentMut, expected: &GitPin) -> Result<(), String> {
    let patch = manifest
        .get("patch")
        .and_then(|item| item.get("crates-io"))
        .and_then(|item| item.get("w3f-bls"));
    let right_patch = patch.is_some_and(|patch| {
        GitPin::from_dependency(patch).is_ok_and(|pin| pin == *expected)
            && patch
                .get("package")
                .and_then(Item::as_str)
                .is_none_or(|name| name == "w3f-bls")
    });
    if !right_patch {
        return Err(format!(
            "the final workspace needs this exact core-matching patch:\n\n{}\n\nCargo applies patches only from the final workspace's root Cargo.toml.",
            expected.patch_line()
        ));
    }
    Ok(())
}

struct CorePatch {
    core: GitPin,
    bls_source: String,
}

fn core_replacement<'a>(manifest: &'a DocumentMut, expected: &GitPin) -> Option<&'a Item> {
    manifest
        .get("patch")
        .and_then(|patch| patch.get(expected.repository.as_str()))
        .and_then(|patch| patch.get("bittensor-core"))
}

fn recorded_core_patch(
    manifest: &DocumentMut,
    expected: &GitPin,
    record: &DocumentMut,
) -> Result<CorePatch, String> {
    let core = record
        .get("core")
        .ok_or("core patch record has no core pin")?;
    let tag = core
        .get("tag")
        .and_then(Item::as_str)
        .ok_or("core patch record has no upstream tag")?;
    let base = core
        .get("commit")
        .and_then(Item::as_str)
        .ok_or("core patch record has no upstream commit")?;
    let repository = record
        .get("monorepo")
        .and_then(|monorepo| monorepo.get("url"))
        .and_then(Item::as_str)
        .ok_or("core patch record has no upstream repository")?;
    let original = GitPin {
        repository: repository.to_owned(),
        reference: Some(("tag".into(), tag.to_owned())),
    };
    if original != *expected {
        return Err("core patch record differs from the workspace's upstream declaration".into());
    }
    let patch = core
        .get("patch")
        .ok_or("core patch record has no replacement")?;
    let revision = patch
        .get("rev")
        .and_then(Item::as_str)
        .ok_or("core patch record has no replacement revision")?;
    if patch.get("base").and_then(Item::as_str) != Some(base)
        || !full_commit(base)
        || !full_commit(revision)
    {
        return Err(
            "core patch record requires matching full upstream and replacement commits".into(),
        );
    }
    let recorded = GitPin {
        repository: patch
            .get("repo")
            .and_then(Item::as_str)
            .ok_or("core patch record has no replacement repository")?
            .to_owned(),
        reference: Some(("rev".into(), revision.to_owned())),
    };
    let replacement = GitPin::from_dependency(
        core_replacement(manifest, expected).ok_or("workspace core replacement is absent")?,
    )?;
    if replacement != recorded {
        return Err("workspace core replacement differs from its recorded immutable pin".into());
    }
    // Only core is replaced. BLS must retain the original upstream identity,
    // even when Cargo resolves core to the recorded Ledger transport fork.
    Ok(CorePatch {
        core: recorded,
        bls_source: format!("git+{repository}?tag={tag}#{base}"),
    })
}

fn full_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_lock(lock: &DocumentMut, expected: &GitPin) -> Result<String, String> {
    verify_resolved_lock(lock, expected, None, &expected.patch_line())
}

fn verify_resolved_lock(
    lock: &DocumentMut,
    expected: &GitPin,
    bls_source: Option<&str>,
    required_patch: &str,
) -> Result<String, String> {
    let packages = lock
        .get("package")
        .and_then(Item::as_array_of_tables)
        .ok_or("Cargo.lock has no package array")?;
    let core: Vec<_> = packages
        .iter()
        .filter(|package| package.get("name").and_then(Item::as_str) == Some("bittensor-core"))
        .collect();
    if core.len() != 1 {
        return Err(format!(
            "Cargo.lock must contain exactly one bittensor-core; found {}.\n\n{}",
            core.len(),
            required_patch
        ));
    }
    let core_source = core[0]
        .get("source")
        .and_then(Item::as_str)
        .ok_or("Cargo.lock bittensor-core has no git source")?;
    let matches_pin = GitPin::from_source(core_source).is_ok_and(|pin| pin == *expected);
    let has_commit = core_source.rsplit_once('#').is_some_and(|(_, commit)| {
        full_commit(commit)
            && expected.reference.as_ref().is_none_or(|(kind, revision)| {
                kind != "rev" || !full_commit(revision) || revision == commit
            })
    });
    if !matches_pin || !has_commit {
        return Err(format!(
            "Cargo.lock bittensor-core source differs from the workspace declaration or resolved commit.\n\n{}",
            required_patch
        ));
    }
    let bls = packages
        .iter()
        .filter(|package| package.get("name").and_then(Item::as_str) == Some("w3f-bls"))
        .map(|package| {
            Ok(LockedPackage {
                name: "w3f-bls",
                version: package
                    .get("version")
                    .and_then(Item::as_str)
                    .ok_or("Cargo.lock w3f-bls has no version")?,
                source: package.get("source").and_then(Item::as_str),
                checksum: package.get("checksum").and_then(Item::as_str),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let bls_source = bls_source.unwrap_or(core_source);
    validate_bls_lock(&bls, bls_source).map_err(|error| format!("{error}\n\n{required_patch}"))?;
    Ok(bls_source.to_owned())
}

fn built_tree_command(manifest: &Path) -> Command {
    let mut command = Command::new("cargo");
    if let Some(root) = manifest.parent() {
        command.current_dir(root);
    }
    // Cargo must resolve the consumer's complete build graph. `--locked` and
    // `--offline` keep this inspection from updating its lock or fetching code.
    command
        // Cargo decorates repeated-node markers when the caller forces color;
        // dependency identities must be parsed from plain captured output.
        .env("CARGO_TERM_COLOR", "never")
        .args([
            "tree",
            "--locked",
            "--offline",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--manifest-path",
        ])
        .arg(manifest);
    command
}

fn verify_built(manifest: &Path, core_source: &str) -> Result<(), String> {
    let tree = cargo_output(&mut built_tree_command(manifest))?;
    validate_bls_tree(&tree, core_source)
}

/// The verified workspace and any difference from the inspecting binary's pin.
pub(super) struct Check {
    pub(super) manifest: PathBuf,
    pub(super) warning: Option<String>,
}

pub(super) fn check(manifest_path: &Path) -> Result<Check, String> {
    let manifest_path = workspace_manifest(manifest_path)?;
    let manifest = document(&manifest_path)?;
    let expected = expected_pin(&manifest_path, &manifest)?;
    verify_manifest(&manifest, &expected)?;
    let root = manifest_path
        .parent()
        .ok_or("Cargo.toml has no parent directory")?;
    let lock = document(&root.join("Cargo.lock"))?;
    let bls_source = if core_replacement(&manifest, &expected).is_some() {
        let patch = recorded_core_patch(
            &manifest,
            &expected,
            &document(&root.join("upstream.toml"))?,
        )?;
        verify_resolved_lock(
            &lock,
            &patch.core,
            Some(&patch.bls_source),
            &expected.patch_line(),
        )?
    } else {
        verify_lock(&lock, &expected)?
    };
    verify_built(&manifest_path, &bls_source)?;
    Ok(Check {
        manifest: manifest_path,
        warning: expected.warning(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const MONOREPO: &str = "https://github.com/RaoFoundation/subtensor.git";
    const COMMIT: &str = "850f2755bb05a3851f948489f0ab27b0645cebce";

    fn pin(kind: &str, value: &str) -> GitPin {
        GitPin {
            repository: MONOREPO.into(),
            reference: Some((kind.into(), value.into())),
        }
    }

    fn parse(value: &str) -> Result<DocumentMut, String> {
        value
            .parse()
            .map_err(|error: toml_edit::TomlError| error.to_string())
    }

    fn lock(core_source: &str, bls_source: &str) -> Result<DocumentMut, String> {
        parse(&format!(
            "version = 4\n[[package]]\nname = \"bittensor-core\"\nversion = \"0.1.0\"\nsource = {core_source:?}\n[[package]]\nname = \"w3f-bls\"\nversion = \"0.1.3\"\nsource = {bls_source:?}\n"
        ))
    }

    #[test]
    fn patch_uses_workspace_declaration_and_reference_kind() -> Result<(), String> {
        let expected = pin("tag", "v477");
        verify_manifest(&parse(&expected.patch_line())?, &expected)?;
        for wrong in [
            pin("tag", "v478"),
            pin("rev", "v477"),
            pin("branch", "v477"),
        ] {
            let error = verify_manifest(&parse(&wrong.patch_line())?, &expected)
                .err()
                .ok_or("wrong patch accepted")?;
            assert!(error.contains(&expected.patch_line()));
        }
        let error = verify_manifest(&parse("[workspace]\n")?, &expected)
            .err()
            .ok_or("missing patch accepted")?;
        assert!(error.contains(&expected.patch_line()));
        Ok(())
    }

    #[test]
    fn stale_lock_does_not_override_declared_reference() -> Result<(), String> {
        let source = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
        assert!(verify_lock(&lock(&source, &source)?, &pin("tag", "v477")).is_err());
        let previous_tag = format!("git+{MONOREPO}?tag=v477#{COMMIT}");
        assert!(verify_lock(&lock(&previous_tag, &source)?, &pin("tag", "v477")).is_err());
        let rev = format!("git+{MONOREPO}?rev={COMMIT}#{COMMIT}");
        assert!(verify_lock(&lock(&source, &rev)?, &pin("tag", "v478")).is_err());
        let other_commit = source.replace(COMMIT, "0000000000000000000000000000000000000000");
        assert!(verify_lock(&lock(&source, &other_commit)?, &pin("tag", "v478")).is_err());
        Ok(())
    }

    #[test]
    fn recorded_core_replacement_keeps_bls_at_its_upstream_base() -> Result<(), String> {
        const FORK: &str = "https://github.com/sam0x17/subtensor.git";
        const REVISION: &str = "626f2a4f1f67ab731c1fc3884828751496bc125d";
        let expected = pin("tag", "v478");
        let required_patch = expected.patch_line();
        let manifest = parse(&format!(
            "{required_patch}\n[patch.{MONOREPO:?}]\nbittensor-core = {{ git = {FORK:?}, rev = {REVISION:?} }}\n"
        ))?;
        let record = parse(&format!(
            "[monorepo]\nurl = {MONOREPO:?}\n[core]\ntag = \"v478\"\ncommit = {COMMIT:?}\n[core.patch]\nrepo = {FORK:?}\nrev = {REVISION:?}\nbase = {COMMIT:?}\n"
        ))?;
        verify_manifest(&manifest, &expected)?;
        let patch = recorded_core_patch(&manifest, &expected, &record)?;
        let upstream = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
        let replaced = format!("git+{FORK}?rev={REVISION}#{REVISION}");
        assert_eq!(patch.bls_source, upstream);
        assert_eq!(
            verify_resolved_lock(
                &lock(&replaced, &upstream)?,
                &patch.core,
                Some(&patch.bls_source),
                &required_patch,
            )?,
            upstream
        );
        // The normal downstream path still rejects an undeclared replacement.
        assert!(verify_lock(&lock(&replaced, &upstream)?, &expected).is_err());
        for (core_source, bls_source) in [
            (upstream.clone(), upstream.clone()),
            (replaced.clone(), replaced.clone()),
            (
                replaced.replace(&format!("#{REVISION}"), &format!("#{COMMIT}")),
                upstream.clone(),
            ),
            (replaced.clone(), upstream.replace(COMMIT, REVISION)),
            (replaced.replace(FORK, MONOREPO), upstream.clone()),
        ] {
            let error = verify_resolved_lock(
                &lock(&core_source, &bls_source)?,
                &patch.core,
                Some(&patch.bls_source),
                &required_patch,
            )
            .err()
            .ok_or("changed core or BLS identity accepted")?;
            assert!(error.contains(&required_patch));
        }
        for changed in [
            record
                .to_string()
                .replace("tag = \"v478\"", "tag = \"v477\""),
            record.to_string().replace(
                &format!("base = {COMMIT:?}"),
                &format!("base = {REVISION:?}"),
            ),
            record.to_string().replace(REVISION, COMMIT),
            record.to_string().replace(FORK, MONOREPO),
            "[core]\n".to_owned(),
        ] {
            assert!(recorded_core_patch(&manifest, &expected, &parse(&changed)?).is_err());
        }
        for changed in [
            manifest.to_string().replace(REVISION, COMMIT),
            manifest.to_string().replace(FORK, MONOREPO),
        ] {
            assert!(recorded_core_patch(&parse(&changed)?, &expected, &record).is_err());
        }
        Ok(())
    }

    #[test]
    fn valid_other_pin_warns_without_failing() -> Result<(), String> {
        let expected = pin("tag", "v477");
        let source = format!("git+{MONOREPO}?tag=v477#{COMMIT}");
        verify_manifest(&parse(&expected.patch_line())?, &expected)?;
        verify_lock(&lock(&source, &source)?, &expected)?;
        assert!(expected.warning().is_some());
        assert!(
            pin("tag", rtao::core::BITTENSOR_CORE_TAG)
                .warning()
                .is_none()
        );
        assert!(pin("rev", COMMIT).warning().is_some());
        Ok(())
    }

    #[test]
    fn unrecorded_duplicate_bls_is_rejected() -> Result<(), String> {
        let expected = pin("tag", "v478");
        let source = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
        verify_lock(&lock(&source, &source)?, &expected)?;
        let duplicate = parse(&format!(
            "{}\n[[package]]\nname = \"w3f-bls\"\nversion = \"0.1.9\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
            lock(&source, &source)?
        ))?;
        assert!(verify_lock(&duplicate, &expected).is_err());
        Ok(())
    }

    fn with_dormant(lock: &DocumentMut, checksum: Option<&str>) -> Result<DocumentMut, String> {
        let checksum = checksum
            .map(|value| format!("checksum = {value:?}\n"))
            .unwrap_or_default();
        parse(&format!(
            "{lock}\n[[package]]\nname = \"w3f-bls\"\nversion = \"0.1.9\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n{checksum}"
        ))
    }

    #[test]
    fn recorded_dormant_lock_requires_exact_checksum_and_primary_pin() -> Result<(), String> {
        use rtao::core::dependency_policy::DORMANT_BLS_CHECKSUM;

        let expected = pin("tag", "v478");
        let source = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
        let primary = lock(&source, &source)?;
        let approved = with_dormant(&primary, Some(DORMANT_BLS_CHECKSUM))?;
        assert_eq!(verify_lock(&approved, &expected)?, source);
        for checksum in [None, Some("tampered")] {
            assert!(verify_lock(&with_dormant(&primary, checksum)?, &expected).is_err());
        }
        let duplicate = with_dormant(&approved, Some(DORMANT_BLS_CHECKSUM))?;
        assert!(verify_lock(&duplicate, &expected).is_err());
        for changed in [
            approved
                .to_string()
                .replace("version = \"0.1.9\"", "version = \"0.1.10\""),
            approved
                .to_string()
                .replace("crates.io-index", "different-index"),
            approved
                .to_string()
                .replace("version = \"0.1.3\"", "version = \"0.1.4\""),
        ] {
            assert!(verify_lock(&parse(&changed)?, &expected).is_err());
        }
        Ok(())
    }

    #[test]
    fn dormant_activation_fails_even_with_an_approved_lock() -> Result<(), String> {
        use rtao::core::dependency_policy::DORMANT_BLS_CHECKSUM;

        let expected = pin("tag", "v478");
        let source = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
        let source = verify_lock(
            &with_dormant(&lock(&source, &source)?, Some(DORMANT_BLS_CHECKSUM))?,
            &expected,
        )?;
        let built = format!("w3f-bls v0.1.3 ({MONOREPO}?tag=v478#{})\n", &COMMIT[..8]);
        validate_bls_tree(&built, &source)?;
        assert!(validate_bls_tree(&format!("{built}w3f-bls v0.1.9\n"), &source).is_err());
        assert!(validate_bls_tree("w3f-bls v0.1.9\n", &source).is_err());
        assert!(validate_bls_tree(&built.replace(&COMMIT[..8], "00000000"), &source).is_err());
        Ok(())
    }

    #[test]
    fn build_inspection_uses_consumer_workspace_without_lock_updates() -> Result<(), String> {
        let manifest = Path::new("/consumer/workspace/Cargo.toml");
        let command = built_tree_command(manifest);
        assert_eq!(command.get_current_dir(), manifest.parent());
        assert!(command.get_envs().any(|(name, value)| {
            name == "CARGO_TERM_COLOR" && value == Some(std::ffi::OsStr::new("never"))
        }));
        let arguments = command
            .get_args()
            .map(|argument| argument.to_str().ok_or("non-UTF8 argument"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            arguments,
            [
                "tree",
                "--locked",
                "--offline",
                "--workspace",
                "--all-features",
                "--target",
                "all",
                "--edges",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}",
                "--manifest-path",
                "/consumer/workspace/Cargo.toml"
            ]
        );
        Ok(())
    }

    #[test]
    fn percent_encoded_branch_and_renamed_dependencies_are_preserved() -> Result<(), String> {
        assert_eq!(
            GitPin::from_source(&format!("git+{MONOREPO}?branch=feature%2Fdoctor"))?,
            pin("branch", "feature/doctor")
        );
        assert!(GitPin::from_source(&format!("git+{MONOREPO}?branch=bad%GG")).is_err());
        let metadata = serde_json::json!({"packages": [{"dependencies": [{"name": "bittensor-core", "rename": "core", "source": format!("git+{MONOREPO}?rev={COMMIT}")}]}]});
        assert_eq!(
            metadata_pins(&metadata)?,
            BTreeSet::from([pin("rev", COMMIT)])
        );
        Ok(())
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Result<Self, String> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let base = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!(
                "doctor-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).map_err(|error| error.to_string())?;
            Ok(Self(path))
        }

        fn write(&self, path: &str, contents: &str) -> Result<(), String> {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().ok_or("fixture path has no parent")?)
                .map_err(|error| error.to_string())?;
            fs::write(path, contents).map_err(|error| error.to_string())
        }

        fn package(
            &self,
            path: &str,
            name: &str,
            workspace: &str,
            extra: &str,
        ) -> Result<(), String> {
            self.write(&format!("{path}/Cargo.toml"), &format!("[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n{workspace}\n{extra}"))?;
            self.write(&format!("{path}/src/lib.rs"), "")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // These fixtures contain only manifests and empty Rust sources;
            // all child Cargo commands have exited before the guard is dropped.
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("retain doctor fixture {}: {error}", self.0.display());
            }
        }
    }

    #[test]
    fn member_manifest_patch_is_ignored_and_diagnostic_names_root_pin() -> Result<(), String> {
        let fixture = Fixture::new()?;
        fixture.write("Cargo.toml", &format!("[workspace]\nmembers = [\"member\"]\n[workspace.dependencies]\nbittensor-core = {{ git = {MONOREPO:?}, tag = \"v477\" }}\n"))?;
        fixture.package(
            "member",
            "doctor-member",
            "",
            &pin("tag", "v477").patch_line(),
        )?;
        let member = fixture.0.join("member/Cargo.toml");
        assert_eq!(workspace_manifest(&member)?, fixture.0.join("Cargo.toml"));
        let error = check(&member)
            .err()
            .ok_or("member patch unexpectedly accepted")?;
        assert!(error.contains(&pin("tag", "v477").patch_line()));
        assert!(error.contains("root Cargo.toml"));
        Ok(())
    }

    #[test]
    fn actual_consumer_tree_exposes_optional_and_cross_target_build_bls() -> Result<(), String> {
        let fixture = Fixture::new()?;
        fixture.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"consumer\"]\nexclude = [\"bls\"]\nresolver = \"2\"\n",
        )?;
        fixture.package("bls", "w3f-bls", "", "[workspace]\n")?;
        // The local package is intentionally an unexpected BLS source. It must
        // appear in the inspected graph and be rejected, including when only an
        // optional feature or a non-host build edge makes it reachable.
        for extra in [
            "[features]\nactivate = [\"dep:w3f-bls\"]\n[dependencies]\nw3f-bls = { path = \"../bls\", optional = true }\n",
            "[target.'cfg(target_os = \"none\")'.build-dependencies]\nw3f-bls = { path = \"../bls\" }\n",
        ] {
            fixture.package("consumer", "doctor-consumer", "", extra)?;
            let manifest = fixture.0.join("Cargo.toml");
            cargo_output(
                Command::new("cargo")
                    .current_dir(&fixture.0)
                    .args(["generate-lockfile", "--offline", "--manifest-path"])
                    .arg(&manifest),
            )?;
            let before =
                fs::read(fixture.0.join("Cargo.lock")).map_err(|error| error.to_string())?;
            let tree = cargo_output(&mut built_tree_command(&manifest))?;
            assert!(tree.lines().any(|line| line.starts_with("w3f-bls ")));
            let expected = format!("git+{MONOREPO}?tag=v478#{COMMIT}");
            assert!(validate_bls_tree(&tree, &expected).is_err());
            assert_eq!(
                before,
                fs::read(fixture.0.join("Cargo.lock")).map_err(|error| error.to_string())?
            );
        }
        Ok(())
    }

    #[test]
    fn cargo_resolves_explicit_workspace_and_excluded_package() -> Result<(), String> {
        let fixture = Fixture::new()?;
        fixture.write(
            "parent/Cargo.toml",
            "[workspace]\nmembers = [\"member\", \"../detached\"]\nexclude = [\"excluded\"]\n",
        )?;
        fixture.package("parent/member", "doctor-member", "", "")?;
        fixture.package("parent/excluded", "doctor-excluded", "", "")?;
        fixture.package(
            "detached",
            "doctor-detached",
            "workspace = \"../parent\"",
            "",
        )?;
        assert_eq!(
            workspace_manifest(&fixture.0.join("detached/Cargo.toml"))?,
            fixture.0.join("parent/Cargo.toml")
        );
        // Without a standalone workspace this nested fixture is also beneath
        // the repository workspace, so Cargo correctly rejects ownership.
        assert!(workspace_manifest(&fixture.0.join("parent/excluded/Cargo.toml")).is_err());
        fixture.package("parent/excluded", "doctor-excluded", "", "[workspace]\n")?;
        assert_eq!(
            workspace_manifest(&fixture.0.join("parent/excluded/Cargo.toml"))?,
            fixture.0.join("parent/excluded/Cargo.toml")
        );
        let selected = find_manifest(&fixture.0.join("parent/member/src"))?;
        assert_eq!(selected, fixture.0.join("parent/member/Cargo.toml"));
        assert_eq!(
            workspace_manifest(&selected)?,
            fixture.0.join("parent/Cargo.toml")
        );
        Ok(())
    }
}
