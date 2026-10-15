//! Immutable main-branch core pins for explicitly disposable checkouts.

use std::{collections::BTreeSet, path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::pin::{atomic_write, cached_repo, git, read_document};

const UPSTREAM: &str = "https://github.com/RaoFoundation/subtensor.git";
const FORK: &str = "https://github.com/sam0x17/subtensor.git";

fn full_commit(commit: &str) -> bool {
    commit.len() == 40
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn local_scope(root: &Path, temporary: &Path) -> Result<()> {
    ensure!(
        root != temporary && root.starts_with(temporary),
        "local core canary workspace must be strictly inside RTAO_TEMP_ROOT; the original workspace is not disposable"
    );
    Ok(())
}

fn require_disposable(root: &Path) -> Result<()> {
    ensure!(
        std::env::var("RTAO_CORE_CANARY_DISPOSABLE").as_deref() == Ok("1"),
        "core canary changes require an explicitly disposable checkout; set RTAO_CORE_CANARY_DISPOSABLE=1 only there"
    );
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        let temporary =
            std::env::var_os("RTAO_TEMP_ROOT").context("local canary needs RTAO_TEMP_ROOT")?;
        let temporary = Path::new(&temporary)
            .canonicalize()
            .context("canonical canary task root")?;
        let root = root.canonicalize().context("canonical canary workspace")?;
        local_scope(&root, &temporary)?;
    }
    Ok(())
}

fn resolve_main(repo: &Path) -> Result<String> {
    git(
        repo,
        &[
            "fetch",
            "--filter=blob:none",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    )?;
    let commit = String::from_utf8(git(
        repo,
        &["rev-parse", "--verify", "refs/remotes/origin/main^{commit}"],
    )?)?
    .trim()
    .to_owned();
    ensure!(
        full_commit(&commit),
        "main did not resolve to a full Git commit"
    );
    Ok(commit)
}

fn rewrite_git_dependencies(item: &mut Item, commit: &str) -> Result<usize> {
    let Some(table) = item.as_table_like_mut() else {
        return Ok(0);
    };
    let mut count = 0;
    if table.get("git").and_then(Item::as_str) == Some(UPSTREAM) {
        for selector in ["tag", "branch", "path", "rev"] {
            table.remove(selector);
        }
        table.insert("rev", value(commit));
        count += 1;
    }
    for (_, child) in table.iter_mut() {
        count += rewrite_git_dependencies(child, commit)?;
    }
    Ok(count)
}

fn transformed(
    manifest: &DocumentMut,
    pins: &DocumentMut,
    commit: &str,
    toolchain: &str,
    fingerprint: &str,
) -> Result<(DocumentMut, DocumentMut)> {
    ensure!(
        full_commit(commit),
        "canary requires a full immutable Git commit"
    );
    ensure!(
        pins["monorepo"]["url"].as_str() == Some(UPSTREAM),
        "unsupported canary source"
    );
    let mut manifest = manifest.clone();
    let patch = manifest["patch"]
        .as_table_like_mut()
        .context("root patch table missing")?;
    // Cargo applies these URL patches across refs. Both must disappear before
    // selecting main itself, even if one table currently has no entries.
    patch.remove(UPSTREAM);
    patch.remove(FORK);
    if let Some(fork) = pins["core"]
        .get("patch")
        .and_then(|patch| patch.get("git"))
        .and_then(Item::as_str)
    {
        patch.remove(fork);
    }
    ensure!(
        rewrite_git_dependencies(manifest.as_item_mut(), commit)? >= 4,
        "canary needs the core, macros, linting and BLS dependency declarations"
    );
    let mut pins = pins.clone();
    pins["core"]
        .as_table_like_mut()
        .context("core pin table missing")?
        .remove("patch");
    let mut canary = Table::new();
    canary.insert("branch", value("main"));
    canary.insert("commit", value(commit));
    canary.insert("toolchain", value(toolchain));
    canary.insert("fingerprint", value(fingerprint));
    pins["core"]["canary"] = Item::Table(canary);
    verify_manifest_document(&manifest, &pins)?;
    Ok((manifest, pins))
}

pub(crate) fn commit(pins: &DocumentMut) -> Result<Option<&str>> {
    let Some(canary) = pins.get("core").and_then(|core| core.get("canary")) else {
        return Ok(None);
    };
    ensure!(
        canary.get("branch").and_then(Item::as_str) == Some("main"),
        "only the main core canary is supported"
    );
    let commit = canary
        .get("commit")
        .and_then(Item::as_str)
        .context("core.canary.commit missing")?;
    ensure!(
        full_commit(commit),
        "core.canary.commit must be a full Git commit"
    );
    ensure!(
        pins["core"].get("patch").is_none(),
        "canary must not carry core.patch"
    );
    Ok(Some(commit))
}

fn verify_git_dependencies(item: &Item, commit: &str) -> Result<()> {
    if let Some(table) = item.as_table_like() {
        if table.get("git").and_then(Item::as_str) == Some(UPSTREAM) {
            ensure!(
                table.get("rev").and_then(Item::as_str) == Some(commit)
                    && ["tag", "branch", "path"]
                        .iter()
                        .all(|key| table.get(key).is_none()),
                "monorepo dependency differs from immutable canary {commit}"
            );
        }
        for (_, child) in table.iter() {
            verify_git_dependencies(child, commit)?;
        }
    }
    Ok(())
}

fn verify_manifest_document(manifest: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let commit = commit(pins)?.context("canary pin missing")?;
    ensure!(
        pins["monorepo"]["url"].as_str() == Some(UPSTREAM),
        "unsupported monorepo source"
    );
    let patch = manifest["patch"]
        .as_table_like()
        .context("patch table missing")?;
    ensure!(
        patch.get(UPSTREAM).is_none() && patch.get(FORK).is_none(),
        "core canary must not retain either core URL patch table"
    );
    for name in ["bittensor-core", "subtensor-macros", "subtensor-linting"] {
        ensure!(
            manifest["workspace"]["dependencies"][name]["git"].as_str() == Some(UPSTREAM),
            "{name} must use the upstream source"
        );
    }
    ensure!(
        manifest["workspace"]["dependencies"]["bittensor-core"]["default-features"].as_bool()
            == Some(false),
        "bittensor-core must disable default features"
    );
    ensure!(
        manifest["patch"]["crates-io"]["w3f-bls"]["git"].as_str() == Some(UPSTREAM),
        "canary must retain the downstream BLS patch"
    );
    verify_git_dependencies(manifest.as_item(), commit)
}

fn verify_lock_document(lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let commit = commit(pins)?.context("canary pin missing")?;
    let expected = format!("git+{UPSTREAM}?rev={commit}#{commit}");
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?;
    for name in [
        "bittensor-core",
        "subtensor-macros",
        "subtensor-linting",
        "w3f-bls",
    ] {
        let matching = packages
            .iter()
            .filter(|package| {
                package.get("name").and_then(Item::as_str) == Some(name)
                    && (name != "w3f-bls"
                        || package
                            .get("source")
                            .and_then(Item::as_str)
                            .is_some_and(|source| source.starts_with(&format!("git+{UPSTREAM}"))))
            })
            .collect::<Vec<_>>();
        ensure!(
            matching.len() == 1,
            "Cargo.lock must contain exactly one {name}; found {}",
            matching.len()
        );
        ensure!(
            matching[0].get("source").and_then(Item::as_str) == Some(&expected),
            "Cargo.lock {name} source differs from canary {expected}"
        );
    }
    for package in packages {
        if let Some(source) = package.get("source").and_then(Item::as_str) {
            ensure!(
                !source.starts_with(&format!("git+{UPSTREAM}")) || source == expected,
                "inconsistent monorepo canary source: {source}"
            );
            ensure!(
                !source.starts_with(&format!("git+{FORK}")),
                "canary still resolves a core-patch fork: {source}"
            );
        }
    }
    Ok(())
}

fn verify_graph_tree(tree: &str, lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let commit = commit(pins)?.context("canary pin missing")?;
    let expected = format!("git+{UPSTREAM}?rev={commit}#{commit}");
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?;
    let mut bls = BTreeSet::new();
    for line in tree.lines().filter(|line| !line.trim().is_empty()) {
        let line = line.replace(" (proc-macro)", "");
        let line = line.trim().strip_suffix(" (*)").unwrap_or(line.trim());
        let (name, rest) = line
            .split_once(" v")
            .context("unrecognized Cargo tree package")?;
        let (version, source) = if let Some((version, source)) = rest.split_once(" (") {
            (
                version,
                Some(
                    source
                        .strip_suffix(')')
                        .context("unclosed Cargo tree source")?,
                ),
            )
        } else {
            (rest, None)
        };
        if let Some(source) = source {
            ensure!(
                source.starts_with("https://") || Path::new(source).is_absolute(),
                "unsupported Cargo tree source {source}"
            );
        }
        let matching = packages
            .iter()
            .filter(|package| {
                package.get("name").and_then(Item::as_str) == Some(name)
                    && package.get("version").and_then(Item::as_str) == Some(version)
                    && match source {
                        Some(source) if source.starts_with("https://") => package
                            .get("source")
                            .and_then(Item::as_str)
                            .is_some_and(|locked| {
                                locked
                                    .strip_prefix("git+")
                                    .is_some_and(|locked| locked.starts_with(source))
                            }),
                        _ => !package
                            .get("source")
                            .and_then(Item::as_str)
                            .is_some_and(|locked| locked.starts_with("git+")),
                    }
            })
            .collect::<Vec<_>>();
        ensure!(
            matching.len() == 1,
            "cannot identify canary graph package {name} {version} in Cargo.lock; found {}",
            matching.len()
        );
        let package = matching[0];
        if let Some(source) = package.get("source").and_then(Item::as_str) {
            ensure!(
                !source.starts_with(&format!("git+{UPSTREAM}")) || source == expected,
                "built monorepo package differs from canary: {source}"
            );
            ensure!(
                !source.starts_with(&format!("git+{FORK}")),
                "built canary package resolves the core-patch fork: {source}"
            );
        }
        if name == "w3f-bls" {
            ensure!(
                package.get("source").and_then(Item::as_str) == Some(expected.as_str()),
                "built BLS implementation differs from immutable canary"
            );
            bls.insert((
                package.get("name").and_then(Item::as_str),
                package.get("version").and_then(Item::as_str),
                package.get("source").and_then(Item::as_str),
            ));
        }
    }
    ensure!(
        bls.len() == 1,
        "canary normal/build graph must contain exactly one pinned BLS implementation; found {}",
        bls.len()
    );
    Ok(())
}

fn verify_graph(root: &Path, pins: &DocumentMut) -> Result<()> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args([
            "tree",
            "--locked",
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
        ])
        .output()
        .context("audit resolved canary normal/build graph")?;
    ensure!(
        output.status.success(),
        "canary dependency tree resolution failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    verify_graph_tree(
        &String::from_utf8(output.stdout)?,
        &read_document(&root.join("Cargo.lock"))?,
        pins,
    )
}

pub(crate) fn verify(root: &Path, pins: &DocumentMut) -> Result<()> {
    require_disposable(root)?;
    let repo = cached_repo(root)?;
    let commit = commit(pins)?.context("core canary missing")?;
    ensure!(
        String::from_utf8(git(
            &repo,
            &["rev-parse", "--verify", &format!("{commit}^{{commit}}")]
        )?)?
        .trim()
            == commit,
        "canary source commit missing from cache"
    );
    ensure!(
        crate::fingerprint::fingerprint(&repo, commit)?
            == pins["core"]["canary"]["fingerprint"]
                .as_str()
                .context("canary fingerprint missing")?,
        "canary core-input fingerprint changed"
    );
    ensure!(
        crate::pin::toolchain(&repo, commit)?
            == pins["core"]["canary"]["toolchain"]
                .as_str()
                .context("canary toolchain missing")?,
        "canary toolchain differs from source"
    );
    verify_manifest_document(&read_document(&root.join("Cargo.toml"))?, pins)?;
    verify_lock_document(&read_document(&root.join("Cargo.lock"))?, pins)?;
    verify_graph(root, pins)
}

pub(crate) fn run(root: &Path, pins: &DocumentMut, branch: &str) -> Result<()> {
    ensure!(
        branch == "main",
        "core canary supports --core-branch main only"
    );
    require_disposable(root)?;
    let repo = cached_repo(root)?;
    let commit = resolve_main(&repo)?;
    crate::fingerprint::prepare_source_cache(&repo, &commit)?;
    let fingerprint = crate::fingerprint::fingerprint(&repo, &commit)?;
    let toolchain = crate::pin::toolchain(&repo, &commit)?;
    let (manifest, pins) = transformed(
        &read_document(&root.join("Cargo.toml"))?,
        pins,
        &commit,
        &toolchain,
        &fingerprint,
    )?;
    atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
    atomic_write(&root.join("upstream.toml"), pins.to_string().as_bytes())?;
    let status = Command::new("cargo")
        .current_dir(root)
        .args(["update", "--workspace"])
        .status()
        .context("resolve the disposable core canary lockfile")?;
    ensure!(
        status.success(),
        "canary lock resolution failed; discard this disposable checkout"
    );
    verify(root, &pins)?;
    println!(
        "Disposable core canary main resolved to {commit}; release/parity pins retained, core patches removed, manifest and lock sources verified. No commit created."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "1111111111111111111111111111111111111111";

    #[test]
    fn canary_scope_rejects_original_root_and_accepts_only_nested_checkouts() -> Result<()> {
        let root = Path::new("/workspace/rtao");
        let temporary = root.join("target/recorded-task");
        assert!(local_scope(root, &temporary).is_err());
        assert!(local_scope(&temporary, &temporary).is_err());
        assert!(local_scope(Path::new("/workspace/unrelated"), &temporary).is_err());
        local_scope(&temporary.join("tmp/canary"), &temporary)?;
        Ok(())
    }

    fn documents() -> Result<(DocumentMut, DocumentMut)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace missing")?;
        Ok((
            read_document(&root.join("Cargo.toml"))?,
            read_document(&root.join("upstream.toml"))?,
        ))
    }

    #[test]
    fn canary_rewrites_all_sources_and_removes_both_patch_tables() -> Result<()> {
        let (mut manifest, mut pins) = documents()?;
        let future: DocumentMut = format!(
            "dependency = {{ git = '{UPSTREAM}', branch = 'main', features = ['future'] }}"
        )
        .parse()?;
        manifest["workspace"]["dependencies"]["future-monorepo"] = future["dependency"].clone();
        for source in [UPSTREAM, FORK] {
            manifest["patch"][source] = Item::Table(Table::new());
            manifest["patch"][source]["bittensor-core"] = value("temporary patch");
        }
        pins["core"]["patch"] = Item::Table(Table::new());
        let released = pins["core"]["tag"].clone();
        let released_commit = pins["core"]["commit"].clone();
        let parity = pins["parity"].to_string();
        let (manifest, canary) = transformed(&manifest, &pins, COMMIT, "1.89", "b3:fixture")?;
        assert_eq!(canary["core"]["tag"].as_str(), released.as_str());
        assert_eq!(canary["core"]["commit"].as_str(), released_commit.as_str());
        assert_eq!(canary["parity"].to_string(), parity);
        assert!(canary["core"].get("patch").is_none());
        assert_eq!(
            manifest["workspace"]["dependencies"]["future-monorepo"]["rev"].as_str(),
            Some(COMMIT)
        );
        assert!(
            manifest["workspace"]["dependencies"]["future-monorepo"]
                .get("branch")
                .is_none()
        );
        assert_eq!(
            manifest["workspace"]["dependencies"]["future-monorepo"]["features"]
                .as_array()
                .context("features missing")?
                .len(),
            1
        );
        verify_manifest_document(&manifest, &canary)?;
        Ok(())
    }

    #[test]
    fn canary_lock_rejects_old_refs_and_fork_sources_but_retains_dormant_entries() -> Result<()> {
        let (manifest, pins) = documents()?;
        let (mut manifest, pins) = transformed(&manifest, &pins, COMMIT, "1.89", "b3:fixture")?;
        let source = format!("git+{UPSTREAM}?rev={COMMIT}#{COMMIT}");
        let entries = [
            "bittensor-core",
            "subtensor-macros",
            "subtensor-linting",
            "w3f-bls",
        ]
        .map(|name| format!("[[package]]\nname='{name}'\nversion='0.1.0'\nsource='{source}'\n"))
        .join("\n");
        let lock: DocumentMut = entries.parse()?;
        verify_lock_document(&lock, &pins)?;
        for changed in [
            entries.replace(&source, &format!("git+{UPSTREAM}?tag=v478#{COMMIT}")),
            entries.replace(UPSTREAM, FORK),
        ] {
            assert!(verify_lock_document(&changed.parse()?, &pins).is_err());
        }
        let dormant: DocumentMut = format!("{entries}\n[[package]]\nname='w3f-bls'\nversion='0.1.9'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n").parse()?;
        verify_lock_document(&dormant, &pins)?;
        manifest["workspace"]["dependencies"]["bittensor-core"]["branch"] = value("main");
        assert!(verify_manifest_document(&manifest, &pins).is_err());
        assert!(transformed(&manifest, &pins, "main", "1.89", "b3:fixture").is_err());
        Ok(())
    }

    #[test]
    fn canary_normal_build_graph_rejects_activated_second_bls_and_wrong_sources() -> Result<()> {
        let (manifest, pins) = documents()?;
        let (_, pins) = transformed(&manifest, &pins, COMMIT, "1.89", "b3:fixture")?;
        let source = format!("git+{UPSTREAM}?rev={COMMIT}#{COMMIT}");
        let entries = format!(
            "[[package]]\nname='bittensor-core'\nversion='0.1.0'\nsource='{source}'\n[[package]]\nname='w3f-bls'\nversion='0.1.3'\nsource='{source}'\n[[package]]\nname='w3f-bls'\nversion='0.1.9'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n"
        );
        let lock: DocumentMut = entries.parse()?;
        let tree = format!(
            "bittensor-core v0.1.0 ({UPSTREAM}?rev={COMMIT}#11111111)\nw3f-bls v0.1.3 ({UPSTREAM}?rev={COMMIT}#11111111)\nw3f-bls v0.1.3 ({UPSTREAM}?rev={COMMIT}#11111111) (*)\n"
        );
        verify_graph_tree(&tree, &lock, &pins)?;
        verify_graph_tree(
            &tree.replace(
                "bittensor-core v0.1.0 (",
                "bittensor-core v0.1.0 (proc-macro) (",
            ),
            &lock,
            &pins,
        )?;
        assert!(verify_graph_tree(&format!("{tree}w3f-bls v0.1.9\n"), &lock, &pins).is_err());
        assert!(verify_graph_tree(&tree.replace("https://", "ssh://"), &lock, &pins).is_err());
        assert!(verify_graph_tree(&tree.replace("https://", "http://"), &lock, &pins).is_err());
        for bad in [
            format!("git+{UPSTREAM}?tag=v478#{COMMIT}"),
            format!("git+{FORK}?rev={COMMIT}#{COMMIT}"),
        ] {
            let changed: DocumentMut = entries.replace(&source, &bad).parse()?;
            assert!(verify_graph_tree(&tree, &changed, &pins).is_err());
        }
        assert!(verify_graph_tree("", &lock, &pins).is_err());
        Ok(())
    }
}
