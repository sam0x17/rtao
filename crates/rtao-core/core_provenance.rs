//! Parse the base core identity and optional carried patch provenance.

use toml_edit::{DocumentMut, Item};

pub(crate) fn require_disposable(
    workspace: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RTAO_CORE_CANARY_DISPOSABLE").as_deref() != Ok("1") {
        return Err(
            "core canary builds require RTAO_CORE_CANARY_DISPOSABLE=1 in a disposable checkout"
                .into(),
        );
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        let temporary =
            std::env::var_os("RTAO_TEMP_ROOT").ok_or("local canary build needs RTAO_TEMP_ROOT")?;
        let temporary = std::path::Path::new(&temporary).canonicalize()?;
        let workspace = workspace.canonicalize()?;
        if workspace == temporary || !workspace.starts_with(&temporary) {
            return Err("local core canary build must be strictly inside RTAO_TEMP_ROOT".into());
        }
    }
    Ok(())
}

pub(crate) struct CorePin<'a> {
    pub(crate) tag: &'a str,
    pub(crate) branch: &'a str,
    pub(crate) commit: &'a str,
    pub(crate) selector: &'static str,
    pub(crate) reference: &'a str,
    // This file is also compiled by xtask's BLS checker, which consumes only
    // the base identity. Build output and lock tests consume the patch fields.
    #[allow(dead_code)]
    pub(crate) patch_commit: &'a str,
    #[allow(dead_code)]
    pub(crate) patch_source: String,
}

fn full_commit(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn patch_identity<'a>(
    core: &'a Item,
    tag: &str,
    commit: &str,
) -> Result<(&'a str, String), &'static str> {
    let Some(patch) = core.get("patch") else {
        return Ok(("", String::new()));
    };
    if !patch.is_table() {
        return Err("core.patch must be a table");
    }
    let repo = "https://github.com/sam0x17/subtensor.git";
    if patch.get("repo").and_then(Item::as_str) != Some(repo) {
        return Err("core.patch.repo must be the maintained PR-U17 fork");
    }
    if !tag.strip_prefix('v').is_some_and(|version| {
        !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Err("a patched core tag must have form vN");
    }
    let branch = format!("rtao-core-patch/{tag}");
    if patch.get("branch").and_then(Item::as_str) != Some(branch.as_str()) {
        return Err("core.patch.branch differs from the base release tag");
    }
    if patch.get("base").and_then(Item::as_str) != Some(commit) {
        return Err("core.patch.base differs from core.commit");
    }
    let revision = patch
        .get("rev")
        .and_then(Item::as_str)
        .filter(|revision| full_commit(revision) && *revision != commit)
        .ok_or("core.patch.rev must be a full distinct Git commit")?;
    if patch.get("upstream_pr").and_then(Item::as_str)
        != Some("https://github.com/RaoFoundation/subtensor/pull/3242")
    {
        return Err("core.patch.upstream_pr differs from PR-U17");
    }
    for field in ["review_base", "review_rev"] {
        if !patch
            .get(field)
            .and_then(Item::as_str)
            .is_some_and(full_commit)
        {
            return Err("core.patch review identity must use full Git commits");
        }
    }
    if !patch
        .get("review_diff_sha256")
        .and_then(Item::as_str)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Err("core.patch.review_diff_sha256 must be a full SHA256");
    }
    if patch.get("merge_commit").is_some()
        && !patch
            .get("merge_commit")
            .and_then(Item::as_str)
            .is_some_and(full_commit)
    {
        return Err("core.patch.merge_commit must be a full Git commit");
    }
    for (field, expected) in [
        (
            "paths",
            &[
                "sdk/bittensor-core/src/signers/mod.rs",
                "sdk/bittensor-core/src/signers/ledger.rs",
                "sdk/bittensor-core/src/signers/hid.rs",
            ][..],
        ),
        (
            "probe",
            &[
                "bittensor_core::signers::ApduTransport",
                "bittensor_core::signers::ledger::LedgerDevice::with_transport",
            ][..],
        ),
    ] {
        let values = patch
            .get(field)
            .and_then(Item::as_array)
            .ok_or("core.patch path/probe inventory missing")?;
        if values.len() != expected.len()
            || !expected.iter().all(|expected| {
                values
                    .iter()
                    .filter(|value| value.as_str() == Some(*expected))
                    .count()
                    == 1
            })
        {
            return Err("core.patch path/probe inventory differs from PR-U17");
        }
    }
    Ok((revision, format!("git+{repo}?rev={revision}#{revision}")))
}

pub(crate) fn effective(pins: &DocumentMut) -> Result<CorePin<'_>, &'static str> {
    let core = pins.get("core").ok_or("upstream.toml has no [core]")?;
    let tag = core
        .get("tag")
        .and_then(Item::as_str)
        .ok_or("core.tag must be a string")?;
    let commit = core
        .get("commit")
        .and_then(Item::as_str)
        .ok_or("core.commit must be a string")?;
    if tag.is_empty() || !full_commit(commit) {
        return Err("upstream.toml must contain a nonempty core tag and a full Git commit");
    }
    let Some(canary) = core.get("canary") else {
        let (patch_commit, patch_source) = patch_identity(core, tag, commit)?;
        return Ok(CorePin {
            tag,
            branch: "",
            commit,
            selector: "tag",
            reference: tag,
            patch_commit,
            patch_source,
        });
    };
    if core.get("patch").is_some() {
        return Err("a core canary cannot retain core.patch");
    }
    if canary.get("branch").and_then(Item::as_str) != Some("main") {
        return Err("core.canary.branch must be main");
    }
    let commit = canary
        .get("commit")
        .and_then(Item::as_str)
        .ok_or("core.canary.commit must be a string")?;
    if !full_commit(commit) {
        return Err("core.canary.commit must be a full Git commit");
    }
    let fingerprint = canary
        .get("fingerprint")
        .and_then(Item::as_str)
        .ok_or("core.canary.fingerprint missing")?;
    if !fingerprint
        .strip_prefix("b3:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("core.canary.fingerprint must be a BLAKE3 fingerprint");
    }
    if canary
        .get("toolchain")
        .and_then(Item::as_str)
        .is_none_or(|toolchain| toolchain.is_empty())
    {
        return Err("core.canary.toolchain missing");
    }
    Ok(CorePin {
        tag: "canary:main",
        branch: "main",
        commit,
        selector: "rev",
        reference: commit,
        patch_commit: "",
        patch_source: String::new(),
    })
}
