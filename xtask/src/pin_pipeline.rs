//! Isolated release candidates, guarded adoption, and pinned pipeline replay.

use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
};

type Inventory = BTreeMap<PathBuf, String>;

fn safe_relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "unsafe pipeline source path {}",
        path.display()
    );
    Ok(())
}

/// Git's tracked and untracked inventory includes the user's current source edits.
pub(crate) fn source_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "cannot enumerate candidate source files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut paths = BTreeSet::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
    {
        let path = PathBuf::from(
            std::str::from_utf8(bytes).context("pipeline source paths must be UTF-8")?,
        );
        safe_relative(&path)?;
        if path.starts_with("target") || path.starts_with(".git") {
            continue;
        }
        if root.join(&path).exists() {
            paths.insert(path);
        }
    }
    Ok(paths)
}

fn inventory(root: &Path, files: impl IntoIterator<Item = PathBuf>) -> Result<Inventory> {
    let mut out = Inventory::new();
    for path in files {
        safe_relative(&path)?;
        let absolute = root.join(&path);
        let metadata = fs::symlink_metadata(&absolute)?;
        ensure!(
            metadata.is_file(),
            "pipeline source must be a regular file: {}",
            absolute.display()
        );
        ensure!(
            absolute.canonicalize()?.starts_with(root.canonicalize()?),
            "pipeline source ancestor escapes workspace"
        );
        out.insert(path, format!("{:x}", Sha256::digest(fs::read(absolute)?)));
    }
    Ok(out)
}

pub(crate) fn require_candidate(root: &Path) -> Result<()> {
    let temporary = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT").context("candidate requires RTAO_TEMP_ROOT")?,
    )
    .canonicalize()?;
    let root = root.canonicalize()?;
    ensure!(
        std::env::var("RTAO_PIN_CANDIDATE").as_deref() == Ok("1")
            && root != temporary
            && root.starts_with(&temporary),
        "pin preparation requires an isolated candidate strictly inside RTAO_TEMP_ROOT"
    );
    Ok(())
}

/// Remove only prior pinned copies in the disposable candidate. The importer
/// then writes a complete tree, including deletion of upstream-removed files.
pub(crate) fn prune_imports(root: &Path, previous: &toml_edit::DocumentMut) -> Result<()> {
    require_candidate(root)?;
    for (_, artifact) in previous["artifacts"]
        .as_table_like()
        .context("prior artifact inventory missing")?
        .iter()
    {
        let destination = Path::new(
            artifact
                .get("to")
                .and_then(toml_edit::Item::as_str)
                .context("prior artifact destination missing")?,
        );
        safe_relative(destination)?;
        ensure!(
            destination.starts_with("vendor/upstream")
                && !destination.starts_with("vendor/upstream/extracted"),
            "prior artifact lies outside pinned upstream copies"
        );
        let absolute = root.join(destination);
        ensure!(
            absolute.canonicalize()?.starts_with(root.canonicalize()?)
                && fs::symlink_metadata(&absolute)?.is_file(),
            "prior copied artifact must be a regular candidate file"
        );
        fs::remove_file(absolute)?;
    }
    Ok(())
}

fn unchanged(before: &Inventory, after: &Inventory) -> Result<()> {
    ensure!(
        before == after,
        "source changed during candidate validation; refusing adoption"
    );
    Ok(())
}

fn replay(before: &Inventory, candidate: &Inventory, report: &serde_json::Value) -> Result<()> {
    ensure!(
        before == candidate,
        "pinned replay changed repository bytes or file inventory; refusing adoption"
    );
    ensure!(
        report
            .get("changes")
            .or_else(|| report.get("changed"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
            && report["changed"].as_u64().is_none_or(|count| count == 0),
        "pinned replay requires an explicitly empty porting report"
    );
    Ok(())
}

fn copy_files(from: &Path, to: &Path, files: impl IntoIterator<Item = PathBuf>) -> Result<()> {
    fs::create_dir_all(to)?;
    let destination_root = to.canonicalize()?;
    for path in files {
        safe_relative(&path)?;
        let destination = to.join(&path);
        let mut parent = destination_root.clone();
        for component in path.parent().context("candidate file parent")?.components() {
            parent.push(component);
            if parent.exists() {
                ensure!(
                    fs::symlink_metadata(&parent)?.is_dir(),
                    "candidate/adoption parent must be a regular directory"
                );
            } else {
                ensure!(
                    fs::symlink_metadata(&parent).is_err(),
                    "candidate/adoption parent is a dangling symlink"
                );
                fs::create_dir(&parent)?;
            }
        }
        if let Ok(metadata) = fs::symlink_metadata(&destination) {
            ensure!(
                metadata.is_file(),
                "candidate/adoption destination must be a regular file"
            );
        }
        fs::copy(from.join(path), destination)?;
    }
    Ok(())
}

pub(crate) fn plan(tag: &str, force: bool) -> serde_json::Value {
    json!({"schema":1,"tag":tag,"force":force,"isolated_candidate":true,
        "python_execution":"designated github-hosted Ubuntu oracle/record lanes only",
        "steps":["release finality/image provenance (reuse immutable committed pins for --force)",
            "re-vendor watched tree and artifacts", "extract", "regen", "oracle image/dump and offline verification",
            "current-spec localnet recordings and Rust replay", "conditional core manifest/lock/surface move",
            "parity report --autobless with symbol/table/oracle proof", "all pr-gates",
            "pinned localnet e2e and core-oracle", "adopt validated changes (pinned replay requires identical bytes)"]})
}

pub(crate) fn run(
    root: &Path,
    tag: &str,
    force: bool,
    plan_only: bool,
    dry_run: bool,
    github_output: Option<&Path>,
) -> Result<()> {
    if plan_only {
        println!("{}", serde_json::to_string_pretty(&plan(tag, force))?);
        return Ok(());
    }
    // Refuse before creating a candidate or invoking any container on a Mac.
    crate::oracle::require_lane("oracle")?;
    let task = crate::oracle::task_path(&format!("pin/pipeline-{}", std::process::id()))?;
    ensure!(
        !task.exists(),
        "pipeline scratch already exists: {}",
        task.display()
    );
    fs::create_dir_all(&task)?;
    let candidate = task.join("candidate");
    let report = crate::oracle::task_path("pin/porting-report.json")?;
    let status_path = crate::oracle::task_path("pin/pipeline-status.json")?;
    fs::write(
        &status_path,
        serde_json::to_vec_pretty(
            &json!({"schema":1,"status":"running","stage":"candidate","tag":tag}),
        )?,
    )?;
    let result = (|| -> Result<(String, bool)> {
        let before = inventory(root, source_files(root)?)?;
        // Reuse original commit objects without creating commits, tags, or remote refs.
        let cloned = Command::new("git")
            .args(["clone", "--shared", "--no-checkout", "--no-tags"])
            .arg(root)
            .arg(&candidate)
            .status()?;
        ensure!(cloned.success(), "candidate clone failed");
        copy_files(root, &candidate, before.keys().cloned())?;
        fs::create_dir_all(task.join("tmp"))?;
        let cache = crate::pin::cached_repo(root)?;
        let exit = Command::new("bash")
            .arg(root.join("ci/pin-pipeline.sh"))
            .arg(&candidate)
            .arg(root)
            .arg(tag)
            .arg(if force { "true" } else { "false" })
            .arg(&report)
            .env("RTAO_TEMP_ROOT", &task)
            .env("RTAO_UPSTREAM_CACHE", cache)
            .env("RTAO_PIN_CANDIDATE", "1")
            .env("CARGO_TARGET_DIR", task.join("build"))
            .env("TMPDIR", task.join("tmp"))
            .env("RTAO_PIN_STAGE_FILE", &status_path)
            .status()?;
        ensure!(
            exit.success(),
            "candidate pipeline failed; source pins and fixtures remain unchanged; see {}",
            status_path.display()
        );
        let after = inventory(root, source_files(root)?)?;
        unchanged(&before, &after)?;
        let candidate_files = inventory(&candidate, source_files(&candidate)?)?;
        let porting_report: serde_json::Value = serde_json::from_slice(&fs::read(&report)?)?;
        if force {
            replay(&before, &candidate_files, &porting_report)?;
        }
        let next = crate::pin::read_document(&candidate.join("upstream.toml"))?;
        let selected = next["parity"]["tag"]
            .as_str()
            .context("candidate parity tag missing")?
            .to_owned();
        let core_moved = crate::pin::read_document(&root.join("upstream.toml"))?["core"]["commit"]
            .as_str()
            != next["core"]["commit"].as_str();
        let spec = next["parity"]["spec_version"]
            .as_integer()
            .context("candidate spec missing")?;
        let archive = crate::oracle::task_path("pin/validated-fixtures.tar.gz")?;
        ensure!(
            Command::new("tar")
                .current_dir(&candidate)
                .arg("-czf")
                .arg(archive)
                .args(["vendor/oracle", &format!("vendor/recorded/{spec}")])
                .status()?
                .success(),
            "cannot retain validated candidate fixtures"
        );
        let fresh_archive = crate::oracle::task_path("pin/replay-evidence.tar.gz")?;
        ensure!(
            Command::new("tar")
                .current_dir(&task)
                .arg("-czf")
                .arg(fresh_archive)
                .args(["oracle-output", &format!("recorded-{spec}")])
                .status()?
                .success(),
            "cannot retain fresh replay evidence"
        );
        if !force && !dry_run {
            // A journal contains exact old file bytes until every write succeeds.
            let journal = task.join("adoption-journal");
            fs::create_dir(&journal)?;
            let originals = journal.join("files");
            let changed: BTreeSet<_> = before
                .keys()
                .chain(candidate_files.keys())
                .filter(|path| before.get(*path) != candidate_files.get(*path))
                .cloned()
                .collect();
            fs::write(
                journal.join("manifest.json"),
                serde_json::to_vec_pretty(&json!({
                    "schema":1,"workspace":root,"state":"prepared",
                    "files":changed.iter().map(|path|json!({"path":path,"before_sha256":before.get(path),"candidate_sha256":candidate_files.get(path)})).collect::<Vec<_>>()
                }))?,
            )?;
            copy_files(
                root,
                &originals,
                changed
                    .iter()
                    .filter(|path| before.contains_key(*path))
                    .cloned(),
            )?;
            let adopted = (|| -> Result<()> {
                copy_files(
                    &candidate,
                    root,
                    changed
                        .iter()
                        .filter(|path| candidate_files.contains_key(*path))
                        .cloned(),
                )?;
                for path in changed
                    .iter()
                    .filter(|path| !candidate_files.contains_key(*path))
                {
                    fs::remove_file(root.join(path))?;
                }
                Ok(())
            })();
            if let Err(error) = adopted {
                copy_files(
                    &originals,
                    root,
                    changed
                        .iter()
                        .filter(|path| before.contains_key(*path))
                        .cloned(),
                )?;
                for path in changed.iter().filter(|path| !before.contains_key(*path)) {
                    if root.join(path).exists() {
                        fs::remove_file(root.join(path))?;
                    }
                }
                return Err(error.context("candidate adoption rolled back"));
            }
        }
        Ok((selected, core_moved))
    })();
    if let Err(error) = &result {
        let mut status: serde_json::Value = serde_json::from_slice(&fs::read(&status_path)?)?;
        status["status"] = json!("held");
        status["error"] = json!(format!("{error:#}"));
        fs::write(&status_path, serde_json::to_vec_pretty(&status)?)?;
    }
    // All subprocesses above are synchronous. The shell checks and removes exact
    // owned containers before it exits. Never remove a live journal after an
    // interrupted or failed rollback.
    if result.is_err() && task.join("adoption-journal").exists() {
        return result
            .map(|_| ())
            .context(format!("retained recovery journal {}", task.display()));
    }
    let processes = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()?;
    ensure!(
        !String::from_utf8_lossy(&processes.stdout)
            .lines()
            .any(|line| line.contains(&task.display().to_string())),
        "retained candidate has a live process dependency: {}",
        candidate.display()
    );
    // A Docker bind mount keeps its source directory live even after the
    // process which started it exits. Inspect mount paths without logging envs.
    let containers = Command::new("docker").args(["ps", "--quiet"]).output()?;
    ensure!(
        containers.status.success(),
        "cannot audit running Docker dependencies; retained {}",
        task.display()
    );
    for id in String::from_utf8(containers.stdout)?.lines() {
        let inspected = Command::new("docker")
            .args(["inspect", "--format", "{{json .Mounts}}", id])
            .output()?;
        ensure!(
            inspected.status.success(),
            "cannot audit container {id}; retained {}",
            task.display()
        );
        let mounts: serde_json::Value = serde_json::from_slice(&inspected.stdout)?;
        ensure!(
            !mounts
                .as_array()
                .context("Docker mount inventory missing")?
                .iter()
                .any(|mount| mount["Source"]
                    .as_str()
                    .is_some_and(|source| Path::new(source).starts_with(&task))),
            "container {id} has a live scratch dependency; retained {}",
            task.display()
        );
    }
    fs::remove_dir_all(&task)?;
    match result {
        Ok((selected, core_moved)) => {
            fs::write(
                &status_path,
                serde_json::to_vec_pretty(
                    &json!({"status":"validated","tag":selected,"force":force,"dry_run":dry_run,"core_bump":core_moved,"adopted":!force&&!dry_run}),
                )?,
            )?;
            if let Some(path) = github_output {
                writeln!(
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)?,
                    "tag={selected}\ncore_bump={core_moved}\nvalidated=true"
                )?;
            }
            println!("Validated {selected}; report {}", report.display());
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FIXTURE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Result<Self> {
            let task = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/pin-pipeline-tests")
                });
            let path = task.join(format!(
                "pin/tests/fixture-{}-{}",
                std::process::id(),
                FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            ensure!(!path.exists(), "pipeline fixture already exists");
            fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Filesystem-only synchronous fixture: no child processes, launchd
            // units, outside configuration, or virtual environments are made.
            // Its only symlink targets another synthetic file inside this root.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn adoption_refuses_symlink_ancestors_and_preserves_other_files() -> Result<()> {
        let fixture = Fixture::new()?;
        let source = fixture.0.join("source");
        let destination = fixture.0.join("destination");
        let retained = fixture.0.join("retained");
        fs::create_dir_all(source.join("escape"))?;
        fs::create_dir(&destination)?;
        fs::create_dir(&retained)?;
        fs::write(source.join("escape/sentinel"), b"candidate")?;
        fs::write(retained.join("sentinel"), b"unrelated original")?;
        std::os::unix::fs::symlink(&retained, destination.join("escape"))?;
        assert!(copy_files(&source, &destination, [PathBuf::from("escape/sentinel")]).is_err());
        assert_eq!(fs::read(retained.join("sentinel"))?, b"unrelated original");
        assert_eq!(fs::read(source.join("escape/sentinel"))?, b"candidate");
        Ok(())
    }
    #[test]
    fn source_races_and_nonempty_replays_cannot_adopt() -> Result<()> {
        let original = BTreeMap::from([(PathBuf::from("upstream.toml"), "old".into())]);
        let changed = BTreeMap::from([(PathBuf::from("upstream.toml"), "new".into())]);
        assert!(unchanged(&original, &changed).is_err());
        assert!(replay(&original, &changed, &json!({"changed":[]})).is_err());
        assert!(replay(&original, &original, &json!({"changed":["python-op"]})).is_err());
        assert!(replay(&original, &original, &json!({})).is_err());
        assert!(
            replay(
                &original,
                &original,
                &json!({"changed":[],"changes":["op"]})
            )
            .is_err()
        );
        assert!(replay(&original, &original, &json!({"changed":1,"changes":[]})).is_err());
        replay(&original, &original, &json!({"changed":[]}))
    }
    #[test]
    fn unsafe_source_paths_are_rejected() {
        for path in ["../wallet", "/Users/sam/wallet", "", "source/../wallet"] {
            assert!(safe_relative(Path::new(path)).is_err());
        }
    }
}
