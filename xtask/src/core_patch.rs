//! Maintain the narrowly scoped PR-U17 patch and prove it remains test-only.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value as Json;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::pin::{atomic_write, cached_repo, git, read_document};

const UPSTREAM: &str = "https://github.com/RaoFoundation/subtensor.git";
const FORK: &str = "https://github.com/sam0x17/subtensor.git";
const PR: &str = "https://github.com/RaoFoundation/subtensor/pull/3242";
const PATHS: [&str; 3] = [
    "sdk/bittensor-core/src/signers/mod.rs",
    "sdk/bittensor-core/src/signers/ledger.rs",
    "sdk/bittensor-core/src/signers/hid.rs",
];
const PROBES: [&str; 2] = [
    "bittensor_core::signers::ApduTransport",
    "bittensor_core::signers::ledger::LedgerDevice::with_transport",
];

fn hash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn tag(value: &str) -> bool {
    value.strip_prefix('v').is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn field<'a>(item: &'a Item, name: &str) -> Result<&'a str> {
    item.get(name)
        .and_then(Item::as_str)
        .with_context(|| format!("core patch needs {name}"))
}

fn strings(item: &Item, name: &str) -> Result<BTreeSet<String>> {
    let array = item
        .get(name)
        .and_then(Item::as_array)
        .with_context(|| format!("core patch needs {name} array"))?;
    let result: BTreeSet<_> = array
        .iter()
        .map(|value| value.as_str().map(str::to_owned).context("expected string"))
        .collect::<Result<_>>()?;
    ensure!(result.len() == array.len(), "duplicate core patch {name}");
    Ok(result)
}

fn patch(pins: &DocumentMut) -> Result<Option<&Item>> {
    let Some(patch) = pins.get("core").and_then(|core| core.get("patch")) else {
        return Ok(None);
    };
    validate_patch(patch)?;
    let core = pins.get("core").context("core pin missing")?;
    let core_tag = field(core, "tag")?;
    ensure!(
        field(patch, "branch")? == format!("rtao-core-patch/{core_tag}"),
        "core patch branch differs from core.tag"
    );
    ensure!(
        field(patch, "base")? == field(core, "commit")?,
        "core patch base differs from core.commit"
    );
    Ok(Some(patch))
}

fn validate_patch(patch: &Item) -> Result<()> {
    ensure!(patch.is_table(), "core.patch must be a table");
    ensure!(field(patch, "repo")? == FORK, "unexpected core patch fork");
    ensure!(
        field(patch, "upstream_pr")? == PR,
        "unexpected core patch PR"
    );
    let core_tag = field(patch, "branch")?
        .strip_prefix("rtao-core-patch/")
        .context("unexpected core patch branch prefix")?;
    ensure!(tag(core_tag), "invalid core patch release tag");
    ensure!(
        field(patch, "branch")? == format!("rtao-core-patch/{core_tag}"),
        "core patch branch differs from core.tag"
    );
    ensure!(
        hash(field(patch, "rev")?) && hash(field(patch, "base")?),
        "core patch commits must be full immutable Git hashes"
    );
    ensure!(
        strings(patch, "paths")? == PATHS.into_iter().map(str::to_owned).collect(),
        "core patch path allowlist changed"
    );
    ensure!(
        strings(patch, "probe")? == PROBES.into_iter().map(str::to_owned).collect(),
        "core patch obsolescence probes changed"
    );
    for key in ["review_base", "review_rev", "review_diff_sha256"] {
        let value = field(patch, key)?;
        ensure!(
            if key == "review_diff_sha256" {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            } else {
                hash(value)
            },
            "invalid PR-U17 {key}"
        );
    }
    Ok(())
}

/// Lock source expected for the core, which is the fork only during a patch.
pub(crate) fn expected_core_source(pins: &DocumentMut) -> Result<String> {
    if let Some(patch) = patch(pins)? {
        let rev = field(patch, "rev")?;
        Ok(format!("git+{FORK}?rev={rev}#{rev}"))
    } else {
        let core = pins.get("core").context("core pin missing")?;
        Ok(format!(
            "git+{UPSTREAM}?tag={}#{}",
            field(core, "tag")?,
            field(core, "commit")?
        ))
    }
}

/// No package other than the single pinned core may resolve from the fork.
pub(crate) fn verify_lock(lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let expected = expected_core_source(pins)?;
    let patched = patch(pins)?.is_some();
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?;
    let mut cores = 0;
    for package in packages {
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .context("package name missing")?;
        let source = package.get("source").and_then(Item::as_str).unwrap_or("");
        if name == "bittensor-core" {
            ensure!(
                source == expected,
                "core lock source differs from core patch/pin"
            );
            cores += 1;
        }
        if source.contains("github.com/sam0x17/subtensor") {
            ensure!(
                patched && name == "bittensor-core" && source == expected,
                "unexpected fork package/source in lock: {name} {source}"
            );
        }
    }
    ensure!(cores == 1, "lock must contain exactly one bittensor-core");
    Ok(())
}

fn dependency(repo: &str, selector: &str, reference: &str) -> Item {
    let mut dependency = InlineTable::new();
    dependency.insert("git", Value::from(repo));
    dependency.insert(selector, Value::from(reference));
    Item::Value(Value::InlineTable(dependency))
}

fn remove_tables(manifest: &mut DocumentMut) -> Result<()> {
    let tables = manifest["patch"]
        .as_table_like_mut()
        .context("patch table missing")?;
    tables.remove(UPSTREAM);
    tables.remove(FORK);
    Ok(())
}

fn patch_tables(manifest: &mut DocumentMut, core_tag: &str, rev: &str) -> Result<()> {
    remove_tables(manifest)?;
    let mut core = Table::new();
    core.insert("bittensor-core", dependency(FORK, "rev", rev));
    manifest["patch"][UPSTREAM] = Item::Table(core);
    let mut other = Table::new();
    for name in ["w3f-bls", "subtensor-macros"] {
        other.insert(name, dependency(UPSTREAM, "tag", core_tag));
    }
    manifest["patch"][FORK] = Item::Table(other);
    Ok(())
}

fn verify_tables(manifest: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let actual = manifest["patch"]
        .as_table_like()
        .context("patch table missing")?;
    if let Some(patch) = patch(pins)? {
        let mut expected = manifest.clone();
        patch_tables(
            &mut expected,
            field(&pins["core"], "tag")?,
            field(patch, "rev")?,
        )?;
        for repo in [UPSTREAM, FORK] {
            let left = actual.get(repo).context("core patch Cargo table missing")?;
            let right = &expected["patch"][repo];
            let left = left
                .as_table_like()
                .context("core patch entry must be a table")?;
            let right = right.as_table_like().context("expected core patch table")?;
            ensure!(
                left.len() == right.len(),
                "unexpected core patch dependency count: {repo}"
            );
            for (name, expected) in right.iter() {
                let actual = left
                    .get(name)
                    .and_then(Item::as_table_like)
                    .context("core patch dependency missing")?;
                let expected = expected
                    .as_table_like()
                    .context("expected core patch dependency")?;
                ensure!(
                    actual.len() == expected.len(),
                    "unexpected core patch dependency fields: {name}"
                );
                for (key, expected) in expected.iter() {
                    ensure!(
                        actual.get(key).and_then(Item::as_str) == expected.as_str(),
                        "core patch dependency differs: {name}.{key}"
                    );
                }
            }
        }
    } else {
        ensure!(
            actual.get(UPSTREAM).is_none() && actual.get(FORK).is_none(),
            "core patch Cargo tables exist without core.patch"
        );
    }
    Ok(())
}

fn verify_clippy(root: &Path) -> Result<()> {
    let config = read_document(&root.join("clippy.toml"))?;
    for (list, path) in [
        ("disallowed-types", PROBES[0]),
        ("disallowed-methods", PROBES[1]),
    ] {
        let entries = config[list]
            .as_array()
            .with_context(|| format!("missing Clippy {list}"))?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.get("path").and_then(Value::as_str) == Some(path))
            .collect();
        ensure!(
            matches.len() == 1
                && matches[0].get("allow-invalid").and_then(Value::as_bool) == Some(true),
            "Clippy must ban {path} with allow-invalid = true"
        );
    }
    Ok(())
}

fn git_text(repo: &Path, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(repo, arguments)?)?.trim().to_owned())
}

fn ancestor(repo: &Path, base: &str, rev: &str) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", base, rev])
        .output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "cannot inspect patch ancestry: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn verify_diff(repo: &Path, base: &str, rev: &str) -> Result<()> {
    ensure!(
        ancestor(repo, base, rev)?,
        "core patch does not descend from core.commit"
    );
    let names = git(
        repo,
        &["diff", "--no-renames", "--name-status", "-z", base, rev],
    )?;
    let records: Vec<_> = names
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .collect();
    ensure!(
        !records.is_empty() && records.len() % 2 == 0,
        "empty or malformed core patch diff"
    );
    for pair in records.chunks_exact(2) {
        ensure!(
            pair.first() == Some(&b"M".as_slice()),
            "core patch must only modify existing regular sources"
        );
        let path = std::str::from_utf8(pair.get(1).context("diff path missing")?)?;
        ensure!(
            PATHS.contains(&path),
            "core patch changed forbidden path {path}"
        );
        for reference in [base, rev] {
            let mode = git_text(repo, &["ls-tree", reference, "--", path])?;
            ensure!(
                mode.starts_with("100644 blob "),
                "core patch path is not a regular source: {path}"
            );
        }
    }
    Ok(())
}

fn diff_digest(repo: &Path, base: &str, rev: &str) -> Result<String> {
    use sha2::Digest;
    // Each path is a separate argument. Keep the exact diff bytes as provenance
    // while the path/regular-file allowlist is verified independently.
    let mut arguments = vec![
        "diff",
        "--binary",
        "--full-index",
        "--no-ext-diff",
        "--no-renames",
        base,
        rev,
        "--",
    ];
    arguments.extend(PATHS);
    Ok(hex::encode(sha2::Sha256::digest(git(repo, &arguments)?)))
}

fn verify_review(repo: &Path, entry: &Item) -> Result<()> {
    let review_base = field(entry, "review_base")?;
    let review_rev = field(entry, "review_rev")?;
    verify_diff(repo, review_base, review_rev)?;
    ensure!(
        diff_digest(repo, review_base, review_rev)? == field(entry, "review_diff_sha256")?,
        "recorded PR-U17 diff changed"
    );
    let base = field(entry, "base")?;
    let expected = if base == review_base {
        git_text(repo, &["rev-parse", &format!("{review_rev}^{{tree}}")])?
    } else {
        applied_review_tree(repo, base, review_base, review_rev)?
    };
    let actual = git_text(
        repo,
        &["rev-parse", &format!("{}^{{tree}}", field(entry, "rev")?)],
    )?;
    ensure!(
        expected == actual,
        "carried core tree differs from applying the recorded PR-U17 diff; review the conflict resolution"
    );
    Ok(())
}

fn applied_review_tree(
    repo: &Path,
    base: &str,
    review_base: &str,
    review_rev: &str,
) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;
    let temporary = hosted_root()?.join("core-patch");
    fs::create_dir_all(&temporary)?;
    let index = temporary.join("review-verify.index");
    let lock = temporary.join("review-verify.index.lock");
    ensure!(
        !index.exists() && !lock.exists(),
        "review verification index already exists; use a fresh hosted task root"
    );
    // An independent index computes the exact tree that a three-way application
    // produces, including whitespace and string/byte literal contents. It never
    // touches the checkout index or relies on whitespace-insensitive patch IDs.
    let result = (|| -> Result<String> {
        let command = || {
            let mut command = Command::new("git");
            command
                .arg("-C")
                .arg(repo)
                .args(["-c", "core.splitIndex=false"])
                .env("GIT_INDEX_FILE", &index);
            command
        };
        let output = command().args(["read-tree", base]).output()?;
        ensure!(
            output.status.success(),
            "cannot initialize isolated review index"
        );
        let diff = git(
            repo,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-renames",
                review_base,
                review_rev,
            ],
        )?;
        ensure!(diff.len() <= 1_048_576, "PR-U17 diff exceeds 1 MiB");
        let mut child = command()
            .args(["apply", "--cached", "--3way", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("review apply stdin missing")?
            .write_all(&diff)?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "core-patch-conflict: recorded PR-U17 does not apply to core base: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = command().arg("write-tree").output()?;
        ensure!(
            output.status.success(),
            "cannot derive exact reviewed core tree"
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    })();
    for path in [&index, &lock] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    result
}

fn hosted_root() -> Result<PathBuf> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Linux"),
        "core-patch builds and rebases run only in bounded hosted Linux CI (PLAN Q30)"
    );
    let root =
        PathBuf::from(std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?);
    ensure!(root.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
    fs::create_dir_all(&root)?;
    root.canonicalize().context("canonical hosted task root")
}

fn missing_symbols(output: &[u8], directory: &Path) -> Result<bool> {
    let source = directory.join("src/main.rs");
    let manifest = directory.join("Cargo.toml");
    let mut errors = 0;
    for line in std::str::from_utf8(output)?.lines() {
        let Ok(record) = serde_json::from_str::<Json>(line) else {
            continue;
        };
        if record["reason"] != "compiler-message" || record["message"]["level"] != "error" {
            continue;
        }
        let own_package = record["package_id"]
            .as_str()
            .and_then(|identity| identity.split_once('#'))
            .is_some_and(|(url, name)| {
                let url = url
                    .strip_prefix("path+")
                    .and_then(|url| url::Url::parse(url).ok());
                name == "rtao-core-patch-probe@0.0.0"
                    && url.and_then(|url| url.to_file_path().ok()).as_deref() == Some(directory)
            });
        let own_target = record["manifest_path"].as_str().map(Path::new)
            == Some(manifest.as_path())
            && record["target"]["name"] == "rtao-core-patch-probe"
            && record["target"]["kind"] == serde_json::json!(["bin"])
            && record["target"]["src_path"].as_str().map(Path::new) == Some(source.as_path());
        let own_span = record["message"]["spans"].as_array().is_some_and(|spans| {
            spans.iter().any(|span| {
                span["is_primary"] == true
                    && span["file_name"].as_str().is_some_and(|name| {
                        let name = Path::new(name);
                        name == source || (!name.is_absolute() && directory.join(name) == source)
                    })
            })
        });
        let missing = record["message"]["code"]["code"]
            .as_str()
            .is_some_and(|code| {
                ["E0405", "E0412", "E0425", "E0432", "E0433", "E0599"].contains(&code)
            });
        if !(own_package && own_target && own_span && missing) {
            return Ok(false);
        }
        errors += 1;
    }
    Ok(errors > 0)
}

fn probe(root: &Path, pins: &DocumentMut, patched: bool) -> Result<Output> {
    let temporary = hosted_root()?;
    let directory = temporary.join(if patched {
        "core-patch/probe/patched"
    } else {
        "core-patch/probe/unpatched"
    });
    ensure!(
        !directory.exists(),
        "core patch probe already exists; use a fresh hosted task root"
    );
    fs::create_dir_all(directory.join("src"))?;
    let core = &pins["core"];
    let core_tag = field(core, "tag")?;
    let mut manifest: DocumentMut = format!(
        "[workspace]\n[package]\nname = \"rtao-core-patch-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nbittensor-core = {{ git = {UPSTREAM:?}, tag = {core_tag:?}, default-features = false, features = [\"ledger\"] }}\n[patch.crates-io]\nw3f-bls = {{ git = {UPSTREAM:?}, tag = {core_tag:?} }}\n"
    ).parse()?;
    if patched {
        patch_tables(
            &mut manifest,
            core_tag,
            field(patch(pins)?.context("patch missing")?, "rev")?,
        )?;
    }
    fs::write(directory.join("Cargo.toml"), manifest.to_string())?;
    // Reuse only the small lock inventory, never compiled output. This keeps
    // isolated probes on reviewed dependency versions while Cargo adjusts the
    // single core source for the unpatched comparison.
    fs::copy(root.join("Cargo.lock"), directory.join("Cargo.lock"))?;
    // The generic function names both exact probe items, so the constructor's
    // inferred transport type cannot produce an unrelated inference error.
    fs::write(
        directory.join("src/main.rs"),
        "fn injected<T: bittensor_core::signers::ApduTransport + 'static>(transport: T) -> bittensor_core::signers::ledger::LedgerDevice { bittensor_core::signers::ledger::LedgerDevice::with_transport(transport) }\nfn main() {}\n",
    )?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(&directory)
        .args(["check", "--message-format=json"])
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_TARGET_DIR", temporary.join("core-patch/build"))
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .context("compile exact core patch API probe")?;
    let missing = missing_symbols(&output.stdout, &directory.canonicalize()?)?;
    if patched {
        ensure!(
            output.status.success() && !missing,
            "patched core API probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    } else if output.status.success() {
        bail!("PR-U17 shipped: drop the core patch");
    } else {
        ensure!(
            missing,
            "unpatched probe failed for a reason other than missing PR-U17 symbols:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // The workflow owns the one task root; remove only our known source dirs.
    fs::remove_dir_all(&directory)?;
    Ok(output)
}

/// Source and identity checks used by `pin --check`.
pub(crate) fn verify(root: &Path, pins: &DocumentMut) -> Result<()> {
    verify_tables(&read_document(&root.join("Cargo.toml"))?, pins)?;
    verify_clippy(root)?;
    verify_lock(&read_document(&root.join("Cargo.lock"))?, pins)?;
    if let Some(patch) = patch(pins)? {
        hosted_root()?;
        let repo = cached_repo(root)?;
        let rev = field(patch, "rev")?;
        git(&repo, &["cat-file", "-e", &format!("{rev}^{{commit}}")])
            .context("patch source cache missing; prepare it explicitly in hosted CI")?;
        verify_diff(&repo, field(patch, "base")?, rev)?;
        verify_review(&repo, patch)?;
        // This merge commit is populated only by the maintained command when
        // GitHub reports the PR merged; no branch or comparison heuristic is used.
        if let Some(merged) = patch.get("merge_commit").and_then(Item::as_str) {
            ensure!(hash(merged), "invalid PR-U17 merge commit");
            ensure!(
                !ancestor(&repo, merged, field(&pins["core"], "commit")?)?,
                "PR-U17 shipped: drop the core patch"
            );
        }
        probe(root, pins, true)?;
        probe(root, pins, false)?;
    }
    Ok(())
}

/// Populate only immutable patch commits in the existing hosted source cache.
pub(crate) fn prepare_cache(root: &Path, pins: &DocumentMut) -> Result<()> {
    let Some(entry) = patch(pins)? else {
        return Ok(());
    };
    hosted_root()?;
    let repo = cached_repo(root)?;
    for revision in [field(entry, "rev")?, field(entry, "review_rev")?] {
        git(&repo, &["fetch", "--filter=blob:none", FORK, revision])?;
    }
    if let Some(merge) = entry.get("merge_commit").and_then(Item::as_str) {
        ensure!(hash(merge), "invalid PR-U17 merge commit");
        git(&repo, &["fetch", "--filter=blob:none", "origin", merge])?;
    }
    Ok(())
}

fn gh(arguments: &[&str]) -> Result<Json> {
    let output = Command::new("gh").args(arguments).output()?;
    ensure!(
        output.status.success(),
        "GitHub PR query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.stdout.len() <= 1_048_576,
        "GitHub PR response exceeds 1 MiB"
    );
    serde_json::from_slice(&output.stdout).context("GitHub response is not JSON")
}

fn pr_commits(repo: &Path, pr: &Json) -> Result<(String, String, Vec<String>)> {
    let records = gh(&["api", "repos/RaoFoundation/subtensor/pulls/3242/commits"])?;
    let records = records.as_array().context("PR commits missing")?;
    ensure!(
        !records.is_empty() && records.len() <= 20,
        "PR-U17 needs 1..=20 commits"
    );
    let mut previous = None;
    let mut base = None;
    let mut commits = Vec::new();
    for record in records {
        let sha = record["sha"].as_str().context("PR commit SHA missing")?;
        ensure!(hash(sha), "invalid PR commit SHA");
        let parents = record["parents"]
            .as_array()
            .context("PR commit parents missing")?;
        ensure!(parents.len() == 1, "PR-U17 must be a linear commit series");
        let parent = parents
            .first()
            .and_then(|parent| parent["sha"].as_str())
            .context("PR parent missing")?;
        ensure!(hash(parent), "invalid PR parent");
        if let Some(previous) = previous {
            ensure!(parent == previous, "PR-U17 commit series is not contiguous");
        } else {
            base = Some(parent.to_owned());
        }
        previous = Some(sha);
        commits.push(sha.to_owned());
    }
    let head = commits.last().context("PR head missing")?.clone();
    ensure!(
        Some(head.as_str()) == pr["head"]["sha"].as_str(),
        "PR changed while querying commits; retry the same operation"
    );
    git(repo, &["fetch", "--filter=blob:none", FORK, &head])?;
    let base = base.context("PR base missing")?;
    verify_diff(repo, &base, &head)?;
    Ok((base, head, commits))
}

fn git_with_input(repo: &Path, arguments: &[&str], bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    command
        .stdin
        .take()
        .context("Git stdin missing")?
        .write_all(bytes)?;
    let output = command.wait_with_output()?;
    ensure!(
        output.status.success(),
        "core-patch-conflict: retained {}: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Rebase a prepared release candidate while preserving its previous patch identity.
/// The pin pipeline calls this after updating candidate core identity, before
/// publishing pins or rebuilding the candidate lock.
pub(crate) fn rebase_candidate(
    root: &Path,
    previous: &DocumentMut,
    next: &mut DocumentMut,
) -> Result<()> {
    let target = field(&next["core"], "tag")?.to_owned();
    ensure!(tag(&target), "core-patch rebase needs a release tag vN");
    let temporary = hosted_root()?;
    let repo = cached_repo(root)?;
    let previous_patch = patch(previous)?;
    let base = git_text(
        &repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{target}^{{commit}}"),
        ],
    )?;
    ensure!(
        base == field(&next["core"], "commit")?,
        "core tag identity changed"
    );
    let pr = gh(&["api", "repos/RaoFoundation/subtensor/pulls/3242"])?;
    ensure!(
        pr["head"]["repo"]["full_name"] == "sam0x17/subtensor",
        "PR-U17 changed fork"
    );
    ensure!(
        pr["state"] == "open" || pr["merged"] == true,
        "PR-U17 was closed without merging; user decision required (PLAN rule 6)"
    );
    if pr["merged"] == true {
        let merge = pr["merge_commit_sha"]
            .as_str()
            .context("merged PR missing commit")?;
        ensure!(hash(merge), "invalid PR merge commit");
        git(&repo, &["fetch", "--filter=blob:none", "origin", merge])?;
        if ancestor(&repo, merge, &base)? {
            next["core"]
                .as_table_like_mut()
                .context("core table missing")?
                .remove("patch");
            let mut manifest = read_document(&root.join("Cargo.toml"))?;
            remove_tables(&mut manifest)?;
            atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
            println!("PR-U17 shipped: dropped the core patch from the release candidate.");
            return Ok(());
        }
    }
    let (review_base, review_rev, commits) = pr_commits(&repo, &pr)?;
    let review_diff = diff_digest(&repo, &review_base, &review_rev)?;
    let branch = format!("rtao-core-patch/{target}");
    let remote = git_text(
        &repo,
        &[
            "ls-remote",
            "--heads",
            FORK,
            &format!("refs/heads/{branch}"),
        ],
    )?;
    let remote = if remote.is_empty() {
        None
    } else {
        let rows = remote.lines().collect::<Vec<_>>();
        ensure!(rows.len() == 1, "ambiguous patch branch identity");
        let (sha, reference) = rows
            .first()
            .context("remote branch missing")?
            .split_once('\t')
            .context("malformed patch ref")?;
        ensure!(
            hash(sha) && reference == format!("refs/heads/{branch}"),
            "unexpected patch branch identity"
        );
        Some(sha.to_owned())
    };
    let previous_same =
        previous_patch.filter(|entry| field(entry, "branch").is_ok_and(|name| name == branch));
    let entry_for = |rev: &str| -> Result<Item> {
        let mut entry = Table::new();
        for (key, contents) in [
            ("repo", FORK),
            ("branch", &branch),
            ("rev", rev),
            ("base", &base),
            ("upstream_pr", PR),
            ("review_base", &review_base),
            ("review_rev", &review_rev),
            ("review_diff_sha256", &review_diff),
        ] {
            entry.insert(key, value(contents));
        }
        for (key, values) in [("paths", &PATHS[..]), ("probe", &PROBES[..])] {
            let mut array = Array::new();
            for item in values {
                array.push(*item);
            }
            entry.insert(key, value(array));
        }
        if pr["merged"] == true {
            entry.insert(
                "merge_commit",
                value(
                    pr["merge_commit_sha"]
                        .as_str()
                        .context("merge commit missing")?,
                ),
            );
        }
        Ok(Item::Table(entry))
    };
    if let Some(remote) = &remote {
        git(&repo, &["fetch", "--filter=blob:none", FORK, remote])?;
        verify_diff(&repo, &base, remote)?;
        if let Some(previous) = previous_same {
            ensure!(
                ancestor(&repo, field(previous, "rev")?, remote)?,
                "remote patch branch rewrote pinned history"
            );
        }
        let entry = entry_for(remote)?;
        if verify_review(&repo, &entry).is_ok() {
            // This includes the initial v478 branch, which can point directly
            // at PR-U17's existing commit. Hosted CI only reads the fork.
            let mut prepared = next.clone();
            prepared["core"]["patch"] = entry;
            let mut manifest = read_document(&root.join("Cargo.toml"))?;
            patch_tables(&mut manifest, &target, remote)?;
            verify_tables(&manifest, &prepared)?;
            atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
            *next = prepared;
            println!(
                "Adopted existing append-only PR-U17 branch {branch} at {remote}; validate the hosted candidate before source adoption."
            );
            return Ok(());
        }
        let previous = previous_same.context(
            "existing patch branch differs from current PR-U17; inspect it before adopting",
        )?;
        ensure!(
            field(previous, "rev")? == remote,
            "unrecorded patch branch differs from current PR-U17; inspect it before adopting"
        );
        verify_review(&repo, previous)?;
    } else {
        ensure!(
            previous_same.is_none(),
            "recorded patch branch disappeared; do not recreate or delete pinned history"
        );
        if base == review_base {
            // No new Git object is needed: the authorized publisher can create
            // this ref at the existing reviewed commit using their own access.
            let entry = entry_for(&review_rev)?;
            write_candidate(&temporary, &repo, &base, &base, &review_rev, &entry)?;
            bail!(
                "core-patch-publish-required: create refs/heads/{branch} at existing reviewed commit {review_rev}, then rerun rebase; hosted CI does not push the fork"
            );
        }
    }
    let start = remote.as_deref().unwrap_or(&base);
    let directory = temporary.join("core-patch/rebase");
    ensure!(
        !directory.exists(),
        "core-patch rebase directory already exists; retained conflict must be resolved first"
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            directory.to_str().context("non-UTF-8 task root")?,
            start,
        ],
    )?;
    if remote.is_some() {
        // Revert the previous net delta, then append the current review series.
        // Publication uses an ordinary fast-forward; no history is overwritten.
        let reverse = git(
            &directory,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                start,
                &base,
            ],
        )?;
        git_with_input(&directory, &["apply", "--index", "--3way", "-"], &reverse)?;
        git(
            &directory,
            &[
                "-c",
                "user.name=rtao core patch",
                "-c",
                "user.email=core-patch@users.noreply.github.com",
                "commit",
                "-m",
                "Replace the previous PR-U17 delta with its reviewed revision",
            ],
        )?;
    }
    for commit in commits {
        let output = Command::new("git")
            .arg("-C")
            .arg(&directory)
            .args([
                "-c",
                "user.name=rtao core patch",
                "-c",
                "user.email=core-patch@users.noreply.github.com",
                "cherry-pick",
                "--keep-redundant-commits",
                &commit,
            ])
            .output()?;
        ensure!(
            output.status.success(),
            "core-patch-conflict: retained {}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let rev = git_text(&directory, &["rev-parse", "HEAD"])?;
    verify_diff(&directory, &base, &rev)?;
    let entry = entry_for(&rev)?;
    verify_review(&directory, &entry)?;
    write_candidate(&temporary, &directory, &base, start, &rev, &entry)?;
    git(
        &repo,
        &[
            "worktree",
            "remove",
            directory.to_str().context("non-UTF-8 task root")?,
        ],
    )?;
    bail!(
        "core-patch-publish-required: bounded candidate files are in {}/core-patch/candidate; publish with an ordinary fast-forward using existing authorized access, then rerun rebase",
        temporary.display()
    );
}

fn write_candidate(
    temporary: &Path,
    repo: &Path,
    base: &str,
    publication_parent: &str,
    rev: &str,
    entry: &Item,
) -> Result<()> {
    use sha2::Digest;
    let output = temporary.join("core-patch/candidate");
    fs::create_dir_all(&output)?;
    let patch = git(
        repo,
        &[
            "format-patch",
            "--stdout",
            "--binary",
            &format!("{publication_parent}..{rev}"),
        ],
    )?;
    ensure!(
        patch.len() <= 1_048_576,
        "core patch candidate exceeds 1 MiB; inspect the source before publication"
    );
    let mut pins = DocumentMut::new();
    pins["core"] = Item::Table(Table::new());
    pins["core"]["patch"] = entry.clone();
    fs::write(output.join("core.patch"), &patch)?;
    fs::write(output.join("identity.toml"), pins.to_string())?;
    let report = serde_json::json!({
        "schema": 1, "status":"publication-required", "repo":FORK,
        "branch":field(entry,"branch")?, "base":base, "publication_parent":publication_parent,
        "candidate_rev":rev, "candidate_rev_is_advisory":true,
        "review_base":field(entry,"review_base")?, "review_rev":field(entry,"review_rev")?,
        "review_diff_sha256":field(entry,"review_diff_sha256")?,
        "patch_sha256":hex::encode(sha2::Sha256::digest(&patch)), "patch_bytes":patch.len(),
        "tests_executed":false, "emulator_evidence":false
    });
    fs::write(
        output.join("candidate.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "Retained bounded core patch candidate at {}",
        output.display()
    );
    Ok(())
}

fn rebase(root: &Path, pins: &DocumentMut, target: &str) -> Result<()> {
    if field(&pins["core"], "tag")? != target {
        // A new core release needs its complete pin/metadata/fingerprint
        // candidate. The pin pipeline calls rebase_candidate with both old and
        // new identities, then runs all candidate gates before adoption.
        return crate::pin::run(vec!["--parity".into(), target.into()]);
    }
    let mut next = pins.clone();
    rebase_candidate(root, pins, &mut next)?;
    atomic_write(&root.join("upstream.toml"), next.to_string().as_bytes())?;
    Ok(())
}

pub(crate) fn run(arguments: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    match arguments.as_slice() {
        [command, target] if command == "rebase" => {
            rebase(root, &read_document(&root.join("upstream.toml"))?, target)
        }
        [command] if command == "strip" => {
            hosted_root()?;
            ensure!(
                std::env::var("RTAO_CORE_UNPATCHED_DISPOSABLE").as_deref() == Ok("1"),
                "core-patch strip requires RTAO_CORE_UNPATCHED_DISPOSABLE=1 in a disposable hosted checkout"
            );
            let mut manifest = read_document(&root.join("Cargo.toml"))?;
            remove_tables(&mut manifest)?;
            atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
            let mut pins = read_document(&root.join("upstream.toml"))?;
            pins["core"]
                .as_table_like_mut()
                .context("core table missing")?
                .remove("patch");
            atomic_write(&root.join("upstream.toml"), pins.to_string().as_bytes())?;
            Ok(())
        }
        _ => bail!("usage: cargo xtask core-patch rebase <vN> | strip"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_lock_bind_only_the_core_to_the_fork() -> Result<()> {
        let base = "1".repeat(40);
        let rev = "2".repeat(40);
        let mut pins: DocumentMut = format!("[core]\ntag='v478'\ncommit='{base}'\n").parse()?;
        let mut entry = Table::new();
        for (key, contents) in [
            ("repo", FORK),
            ("branch", "rtao-core-patch/v478"),
            ("rev", &rev),
            ("base", &base),
            ("upstream_pr", PR),
            ("review_base", &base),
            ("review_rev", &rev),
            ("review_diff_sha256", &"a".repeat(64)),
        ] {
            entry.insert(key, value(contents));
        }
        for (key, values) in [("paths", &PATHS[..]), ("probe", &PROBES[..])] {
            let mut array = Array::new();
            for path in values {
                array.push(*path);
            }
            entry.insert(key, value(array));
        }
        pins["core"]["patch"] = Item::Table(entry);
        let mut manifest: DocumentMut =
            "[patch.crates-io]\nw3f-bls={git='upstream',tag='v478'}\n".parse()?;
        patch_tables(&mut manifest, "v478", &rev)?;
        verify_tables(&manifest, &pins)?;
        let source = expected_core_source(&pins)?;
        let lock: DocumentMut =
            format!("[[package]]\nname='bittensor-core'\nsource='{source}'\n").parse()?;
        verify_lock(&lock, &pins)?;
        let wrong: DocumentMut = format!(
            "{}\n[[package]]\nname='subtensor-macros'\nsource='{source}'\n",
            lock
        )
        .parse()?;
        assert!(verify_lock(&wrong, &pins).is_err());
        manifest["patch"][FORK]["w3f-bls"]["tag"] = value("v477");
        assert!(verify_tables(&manifest, &pins).is_err());
        pins["core"]["patch"]["base"] = value("3".repeat(40));
        assert!(expected_core_source(&pins).is_err());
        Ok(())
    }

    #[test]
    fn only_missing_symbols_in_the_exact_probe_count_as_absence() -> Result<()> {
        use serde_json::json;
        let directory = Path::new("/task/probe");
        let record = json!({
            "reason": "compiler-message",
            "package_id": "path+file:///task/probe#rtao-core-patch-probe@0.0.0",
            "manifest_path": "/task/probe/Cargo.toml",
            "target": {"name":"rtao-core-patch-probe","kind":["bin"],"src_path":"/task/probe/src/main.rs"},
            "message": {"level":"error","code":{"code":"E0405"},"spans":[{"file_name":"src/main.rs","is_primary":true}]}
        });
        assert!(missing_symbols(record.to_string().as_bytes(), directory)?);
        for (pointer, value) in [
            ("/message/code/code", json!("E0277")),
            (
                "/package_id",
                json!("path+file:///task/dependency#dependency@0.0.0"),
            ),
            ("/manifest_path", json!("/task/dependency/Cargo.toml")),
            ("/target/src_path", json!("/task/dependency/src/main.rs")),
            ("/target/kind", json!(["custom-build"])),
            (
                "/message/spans/0/file_name",
                json!("/cargo/dep/src/main.rs"),
            ),
            ("/message/spans/0/is_primary", json!(false)),
        ] {
            let mut changed = record.clone();
            *changed
                .pointer_mut(pointer)
                .context("fixture pointer missing")? = value;
            assert!(!missing_symbols(changed.to_string().as_bytes(), directory)?);
            let combined = format!("{record}\n{changed}");
            assert!(!missing_symbols(combined.as_bytes(), directory)?);
        }
        assert!(!missing_symbols(b"download failed", directory)?);
        Ok(())
    }

    #[test]
    fn hosted_git_fixture_proves_exact_trees_path_modes_and_append_only_history() -> Result<()> {
        let root = hosted_root()?.join("core-patch/source-fixture");
        ensure!(!root.exists(), "hosted source fixture already exists");
        fs::create_dir_all(&root)?;
        let result = (|| -> Result<()> {
            git(&root, &["init", "--quiet"])?;
            git(&root, &["config", "user.name", "rtao source fixture"])?;
            git(
                &root,
                &[
                    "config",
                    "user.email",
                    "source-fixture@users.noreply.github.com",
                ],
            )?;
            let source = root.join(PATHS[1]);
            fs::create_dir_all(source.parent().context("fixture source parent")?)?;
            fs::write(&source, "pub fn open() {}\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "base"])?;
            let base = git_text(&root, &["rev-parse", "HEAD"])?;
            fs::write(
                &source,
                "pub fn open() {}\npub const LABEL: &str = \"APDU bytes\";\n",
            )?;
            git(&root, &["commit", "--quiet", "-am", "reviewed patch"])?;
            let review = git_text(&root, &["rev-parse", "HEAD"])?;
            let digest = diff_digest(&root, &base, &review)?;
            let entry = |base: &str, revision: &str| {
                let mut entry = Table::new();
                for (key, value) in [
                    ("base", base),
                    ("rev", revision),
                    ("review_base", base),
                    ("review_rev", review.as_str()),
                    ("review_diff_sha256", digest.as_str()),
                ] {
                    entry.insert(key, super::value(value));
                }
                Item::Table(entry)
            };
            verify_diff(&root, &base, &review)?;
            verify_review(&root, &entry(&base, &review))?;
            // patch-id ignores this doubled space inside a string literal;
            // exact tree equality must reject the changed runtime value.
            fs::write(
                &source,
                "pub fn open() {}\npub const LABEL: &str = \"APDU  bytes\";\n",
            )?;
            git(&root, &["commit", "--quiet", "-am", "altered literal"])?;
            let altered = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_review(&root, &entry(&base, &altered)).is_err());
            assert!(ancestor(&root, &review, &altered)?);
            git(&root, &["checkout", "--quiet", "--detach", &base])?;
            fs::write(root.join("release.txt"), "unrelated new core release\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "new release base"])?;
            let next_base = git_text(&root, &["rev-parse", "HEAD"])?;
            git(&root, &["cherry-pick", &review])?;
            let rebased = git_text(&root, &["rev-parse", "HEAD"])?;
            let mut rebased_entry = entry(&base, &rebased);
            rebased_entry["base"] = value(&next_base);
            verify_diff(&root, &next_base, &rebased)?;
            verify_review(&root, &rebased_entry)?;
            assert!(!ancestor(&root, &review, &rebased)?);
            assert!(ancestor(&root, &next_base, &rebased)?);
            fs::write(root.join("forbidden.txt"), "unexpected patch content\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "forbidden path"])?;
            let forbidden = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_diff(&root, &next_base, &forbidden).is_err());
            git(&root, &["checkout", "--quiet", "--detach", &rebased])?;
            git(&root, &["update-index", "--chmod=+x", PATHS[1]])?;
            git(
                &root,
                &["commit", "--quiet", "-m", "forbidden executable mode"],
            )?;
            let executable = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_diff(&root, &next_base, &executable).is_err());
            Ok(())
        })();
        // Every child above is synchronous and has exited. Only this exact
        // fixture directory was created, and it contains no external links.
        fs::remove_dir_all(&root)?;
        result
    }
}
