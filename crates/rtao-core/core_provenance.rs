//! Parse the effective core identity for build output and lock assertions.

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
}

fn full_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        return Ok(CorePin {
            tag,
            branch: "",
            commit,
            selector: "tag",
            reference: tag,
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
    })
}
