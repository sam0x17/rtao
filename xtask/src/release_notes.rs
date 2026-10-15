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
    if let Ok(changelog) = fs::read_to_string(root.join("CHANGELOG.md")) {
        output.push('\n');
        output.push_str(&changelog);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    Ok(output)
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
}
