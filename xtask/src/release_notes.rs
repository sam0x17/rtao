//! Release notes generated from the same pins and compatibility record as the binaries.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use toml_edit::DocumentMut;

pub(crate) fn tag_version(tag: &str) -> Result<semver::Version> {
    let version = tag
        .strip_prefix("rtao-v")
        .context("release tag must start with rtao-v")?;
    let parsed = semver::Version::parse(version)?;
    ensure!(
        parsed.to_string() == version && parsed.pre.is_empty() && parsed.build.is_empty(),
        "release tag must contain a canonical stable version"
    );
    Ok(parsed)
}

pub(crate) fn render(root: &Path, tag: &str) -> Result<String> {
    let version = tag_version(tag)?;
    let manifest: DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
    ensure!(
        manifest["workspace"]["package"]["version"].as_str() == Some(version.to_string().as_str()),
        "release tag and workspace version differ"
    );
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let compat: Value = serde_json::from_slice(&fs::read(root.join("compat.json"))?)?;
    let pin = |section: &str, key: &str| -> Result<&str> {
        pins.get(section)
            .and_then(|item| item.get(key))
            .and_then(toml_edit::Item::as_str)
            .with_context(|| format!("missing {section}.{key}"))
    };
    let milestone = compat
        .get("milestone")
        .and_then(Value::as_str)
        .unwrap_or("not yet recorded");
    let mut output = format!(
        "# {tag}\n\nMilestone: {milestone}.\n\nBittensor core: `{}` (`{}`). Python parity: `{}` (`{}`).\n\nSee the attached `COMPAT.md` and `compat.json` for recorded validation; an absent validated range does not assert runtime compatibility.\n\n",
        pin("core", "tag")?,
        pin("core", "commit")?,
        pin("parity", "tag")?,
        pin("parity", "commit")?
    );
    output.push_str("Supported CLI archives: Linux x86_64 and ARM64 (glibc 2.35), macOS Apple ARM and Intel. Linux ARM checks use QEMU; macOS Apple ARM builds run locally and Intel binaries are cross-built and checked through Rosetta. The signed local record and GitHub build attestations identify the actual builders. Rustdoc and the book are downloadable assets.\n\n");
    output.push_str("Every subnet workspace using this core pin needs this exact downstream BLS patch:\n\n```toml\n[patch.crates-io]\nw3f-bls = { git = \"https://github.com/RaoFoundation/subtensor.git\", tag = \"");
    output.push_str(pin("core", "tag")?);
    output.push_str("\" }\n```\n\nRun `rtao doctor --manifest-path Cargo.toml` to check the pin and patch. Move the patch line when the core tag changes.\n");
    let curated_path = root.join("docs/releases").join(format!("{tag}.md"));
    let curated = fs::read_to_string(&curated_path).with_context(|| {
        format!(
            "versioned releases need curated notes in {}",
            curated_path.display()
        )
    })?;
    check_curated(&curated)?;
    output.push('\n');
    output.push_str(&curated);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    if let Ok(changelog) = fs::read_to_string(root.join("CHANGELOG.md")) {
        output.push('\n');
        output.push_str(&changelog);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    Ok(output)
}

/// Curated notes map changes to PLAN decisions and state the persisted-format
/// compatibility range, so a rollback decision can be made from them (D-121).
pub(crate) fn check_curated(notes: &str) -> Result<()> {
    for heading in ["## Journal compatibility", "## Changes by decision"] {
        ensure!(
            notes.lines().any(|line| line.trim_end() == heading),
            "curated release notes need a `{heading}` section"
        );
    }
    let decisions = notes
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| {
            word.strip_prefix("D-")
                .is_some_and(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count();
    ensure!(decisions > 0, "curated release notes cite no D-number");
    Ok(())
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root")?;
    let mut tag = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tag" if tag.is_none() => {
                tag = Some(args.next().context("--tag requires a release tag")?)
            }
            _ => bail!("usage: cargo xtask release-notes --tag rtao-vX.Y.Z"),
        }
    }
    print!("{}", render(root, &tag.context("--tag is required")?)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_or_nonrelease_tags() {
        for tag in [
            "v0.1.0",
            "rtao-v01.0.0",
            "rtao-v0.1.0-alpha",
            "rtao-v0.1.0+build",
            "rtao-v0.1.0/other",
        ] {
            assert!(tag_version(tag).is_err(), "accepted {tag}");
        }
        assert!(tag_version("rtao-v0.1.0").is_ok());
    }

    #[test]
    fn curated_notes_need_decisions_and_journal_range() {
        let good = "# x\n\n## Changes by decision\n\n- D-120: journal range.\n\n## Journal compatibility\n\nSchemas 1 to 4.\n";
        assert!(check_curated(good).is_ok());
        assert!(check_curated(&good.replace("## Journal compatibility", "## Journal")).is_err());
        assert!(check_curated(&good.replace("## Changes by decision", "## Changes")).is_err());
        assert!(check_curated(&good.replace("D-120", "D-1200")).is_err());
    }

    #[test]
    fn next_release_has_curated_notes() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let manifest: DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
        let version = manifest["workspace"]["package"]["version"]
            .as_str()
            .context("workspace version")?;
        let tag = format!("rtao-v{version}");
        // Once released, a tag's notes stay; the next version adds its own.
        check_curated(&fs::read_to_string(
            root.join("docs/releases").join(format!("{tag}.md")),
        )?)
    }

    /// Each published package carries the root license texts as regular files,
    /// and every workspace path dependency names the version it points at, so
    /// `cargo publish` writes a registry requirement (D-181).
    #[test]
    fn crates_carry_root_licenses_and_path_versions() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let manifest: DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
        let workspace_version = manifest["workspace"]["package"]["version"]
            .as_str()
            .context("workspace version")?;
        let dependencies = manifest["workspace"]["dependencies"]
            .as_table()
            .context("workspace dependencies")?;
        let mut crates = 0;
        for entry in fs::read_dir(root.join("crates"))? {
            let dir = entry?.path();
            let Ok(text) = fs::read_to_string(dir.join("Cargo.toml")) else {
                continue;
            };
            crates += 1;
            let crate_manifest: DocumentMut = text.parse()?;
            let package = &crate_manifest["package"];
            let name = package["name"].as_str().context("package name")?;
            for file in ["LICENSE-MIT", "LICENSE-APACHE", "NOTICE"] {
                let path = dir.join(file);
                ensure!(
                    fs::symlink_metadata(&path)
                        .is_ok_and(|metadata| metadata.file_type().is_file()),
                    "{name} lacks a regular {file}"
                );
                ensure!(
                    fs::read(&path)? == fs::read(root.join(file))?,
                    "{name}/{file} differs from the root {file}; copy it again"
                );
            }
            let version = package["version"]
                .as_str()
                .or_else(|| {
                    package["version"]
                        .get("workspace")
                        .map(|_| workspace_version)
                })
                .context("package version")?;
            let declared = dependencies
                .get(name)
                .and_then(|item| item.get("version"))
                .and_then(|item| item.as_str())
                .with_context(|| format!("[workspace.dependencies] {name} has no version"))?;
            ensure!(
                declared == version,
                "[workspace.dependencies] {name} is {declared}, the crate is {version}"
            );
        }
        ensure!(crates >= 24, "found only {crates} crates");
        Ok(())
    }
}
