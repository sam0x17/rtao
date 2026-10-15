//! Initial committed-oracle staging; existing evidence always requires verification.

use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{fs, io::ErrorKind, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommittedStatus {
    Verified,
    InitialPending,
    OptionalUnverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Readiness {
    CheckExisting,
    InitialAbsent,
    OptionalAbsent,
}

impl CommittedStatus {
    pub(crate) fn report(self) -> Value {
        match self {
            Self::Verified => {
                json!({"status":"checked","details":{"integrity_pure_intent_read_behavior":"checked","native_replay":"passed"}})
            }
            Self::InitialPending => {
                json!({"status":"pending","milestone":"M2-05","reason":"First hosted oracle manifest has not been produced; M2-05 refresh has not run and M2-14 oracle behavioral parity remains unverified. No current or previously committed oracle output is omitted."})
            }
            Self::OptionalUnverified => {
                json!({"status":"optional-unverified","reason":"Q30 limits acceptance to client development and hosted tests. No first oracle manifest exists; independent Python oracle parity remains unverified. Existing or historically committed oracle outputs cannot use this status."})
            }
        }
    }
}

fn has_output(dir: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir(),
        "committed oracle directory must be a regular directory"
    );
    for (count, entry) in fs::read_dir(dir)?.enumerate() {
        ensure!(
            count < 4096,
            "committed oracle directory exceeds entry bound"
        );
        let entry = entry?;
        let name = entry.file_name();
        // These three files are reviewed generation inputs, not oracle outputs.
        if !matches!(
            name.to_str(),
            Some("lock.json" | "runtime-lock.json" | "samples_extra.json")
        ) {
            return Ok(true);
        }
        ensure!(
            entry.file_type()?.is_file(),
            "oracle generation inputs must be regular files"
        );
    }
    Ok(false)
}

fn readiness_with_history(
    root: &Path,
    strict: bool,
    base: Option<&str>,
    history: impl FnOnce(&Path, Option<&str>) -> Result<Vec<String>>,
) -> Result<Readiness> {
    if has_output(&root.join("vendor/oracle"))? {
        return Ok(Readiness::CheckExisting);
    }
    let plan = fs::read_to_string(root.join("PLAN.md"))?;
    let optional_policy = plan
        .lines()
        .any(|line| line.starts_with("- **Q30 optional oracle policy.**"))
        && plan
            .lines()
            .any(|line| line.starts_with("- **M2-05 optional hosted refresh.**"));
    for step in ["M2-05", "M2-14"] {
        let unchecked = format!("- [ ] **{step}**");
        let checked = format!("- [x] **{step}**");
        // Q30 permits source-parity completion without inventing independent
        // oracle evidence. A historical generic completed M2-14 still fails.
        let optional = (step == "M2-05").then_some("- **M2-05 optional hosted refresh.**");
        let rows = plan
            .lines()
            .filter(|line| {
                line.starts_with(&unchecked)
                    || line.starts_with(&checked)
                    || optional.is_some_and(|prefix| line.starts_with(prefix))
            })
            .collect::<Vec<_>>();
        ensure!(
            rows.len() == 1,
            "initial oracle staging requires one documented {step} status row"
        );
        ensure!(
            rows[0].starts_with(&unchecked)
                || optional.is_some_and(|prefix| rows[0].starts_with(prefix))
                || (optional_policy
                    && step == "M2-14"
                    && rows[0]
                        .starts_with("- [x] **M2-14** Strict client-source parity machinery:")),
            "{step} is complete but committed oracle manifest/vectors are absent"
        );
    }
    ensure!(
        !strict || optional_policy,
        "strict oracle parity refuses missing first committed manifest"
    );
    let prior = history(root, base)?;
    ensure!(
        prior.is_empty(),
        "committed oracle evidence was removed; initial staging cannot hide prior artifacts: {prior:?}"
    );
    Ok(if optional_policy {
        Readiness::OptionalAbsent
    } else {
        Readiness::InitialAbsent
    })
}

fn check_with(
    root: &Path,
    strict: bool,
    base: Option<&str>,
    verify: impl FnOnce(&Path, &Path) -> Result<()>,
) -> Result<CommittedStatus> {
    check_with_history(
        root,
        strict,
        base,
        verify,
        crate::parity::prior_oracle_artifacts,
    )
}

fn check_with_history(
    root: &Path,
    strict: bool,
    base: Option<&str>,
    verify: impl FnOnce(&Path, &Path) -> Result<()>,
    history: impl FnOnce(&Path, Option<&str>) -> Result<Vec<String>>,
) -> Result<CommittedStatus> {
    match readiness_with_history(root, strict, base, history)? {
        Readiness::CheckExisting => {
            verify(root, &root.join("vendor/oracle"))?;
            Ok(CommittedStatus::Verified)
        }
        Readiness::InitialAbsent => Ok(CommittedStatus::InitialPending),
        Readiness::OptionalAbsent => Ok(CommittedStatus::OptionalUnverified),
    }
}

pub(crate) fn check_committed(
    root: &Path,
    strict: bool,
    base: Option<&str>,
) -> Result<CommittedStatus> {
    check_with(root, strict, base, super::check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use std::{
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Result<Self> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let temporary = PathBuf::from(
                std::env::var_os("RTAO_TEMP_ROOT")
                    .context("oracle readiness fixtures require recorded RTAO_TEMP_ROOT")?,
            );
            ensure!(
                temporary.is_absolute() && temporary.is_dir(),
                "recorded fixture root must exist"
            );
            let path = temporary.join(format!(
                "oracle-readiness-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            ensure!(!path.exists(), "oracle readiness fixture already exists");
            let fixture = Self(path);
            fs::create_dir_all(fixture.0.join("vendor/oracle"))?;
            fs::create_dir_all(fixture.0.join("parity"))?;
            fs::write(
                fixture.0.join("PLAN.md"),
                "- [ ] **M2-05** pending\n- [ ] **M2-14** pending\n",
            )?;
            fs::write(
                fixture.0.join("parity/pending.toml"),
                "intents=[]\nreads=[]\n",
            )?;
            for name in ["lock.json", "runtime-lock.json", "samples_extra.json"] {
                fs::write(fixture.0.join("vendor/oracle").join(name), "{}")?;
            }
            fixture.git(&["init", "--quiet"])?;
            fixture.git(&["add", "."])?;
            fixture.commit("initial reviewed inputs")?;
            Ok(fixture)
        }
        fn git(&self, args: &[&str]) -> Result<String> {
            let output = Command::new("git")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .arg("-C")
                .arg(&self.0)
                .args(["-c", "core.hooksPath=/dev/null", "-c", "init.templateDir="])
                .args(args)
                .output()?;
            ensure!(
                output.status.success(),
                "fixture git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).context("fixture git output UTF-8")
        }
        fn commit(&self, message: &str) -> Result<()> {
            self.git(&[
                "-c",
                "user.name=Oracle readiness fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                message,
            ])?;
            Ok(())
        }
        fn readiness(&self, strict: bool, base: Option<&str>) -> Result<Readiness> {
            readiness_with_history(&self.0, strict, base, |root, base| {
                crate::parity::oracle_history_for_test(root, None, base)
            })
        }
        fn check(
            &self,
            strict: bool,
            base: Option<&str>,
            verify: impl FnOnce(&Path, &Path) -> Result<()>,
        ) -> Result<CommittedStatus> {
            check_with_history(&self.0, strict, base, verify, |root, base| {
                crate::parity::oracle_history_for_test(root, None, base)
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Exact task-owned source-only Git fixture; no spawned service,
            // launchd job, venv, symlink or external configuration depends on it.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn initial_absence_is_pending_and_never_reports_verified() -> Result<()> {
        let fixture = Fixture::new()?;
        let status = fixture.check(false, None, |_, _| {
            anyhow::bail!("missing evidence must not invoke verification")
        })?;
        assert_eq!(status, CommittedStatus::InitialPending);
        assert_eq!(status.report()["status"], json!("pending"));
        assert_eq!(status.report()["milestone"], json!("M2-05"));
        assert!(fixture.readiness(true, None).is_err());
        let plan = fs::read_to_string(fixture.0.join("PLAN.md"))?;
        for step in ["M2-05", "M2-14"] {
            fs::write(
                fixture.0.join("PLAN.md"),
                plan.replace(&format!("- [ ] **{step}**"), &format!("- [x] **{step}**")),
            )?;
            assert!(fixture.readiness(false, None).is_err());
        }
        for plan in [
            "",
            "- [ ] **M2-05** missing other step\n",
            "- [ ] **M2-05** pending\n- [ ] **M2-05** duplicate\n- [ ] **M2-14** pending\n",
        ] {
            fs::write(fixture.0.join("PLAN.md"), plan)?;
            assert!(fixture.readiness(false, None).is_err());
        }
        Ok(())
    }

    #[test]
    fn q30_strict_source_parity_reports_optional_oracle_as_unverified() -> Result<()> {
        let fixture = Fixture::new()?;
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let plan = fs::read_to_string(workspace.join("PLAN.md"))?;
        ensure!(
            plan.contains("- **M2-05 optional hosted refresh.**"),
            "fixture must exercise the current optional hosted refresh declaration"
        );
        fs::write(fixture.0.join("PLAN.md"), &plan)?;
        assert_eq!(
            fixture.check(false, None, |_, _| {
                anyhow::bail!("optional refresh cannot invent oracle evidence")
            })?,
            CommittedStatus::OptionalUnverified
        );
        let status = fixture.check(true, None, |_, _| {
            anyhow::bail!("strict source checks must not invent oracle execution")
        })?;
        assert_eq!(status, CommittedStatus::OptionalUnverified);
        assert_eq!(status.report()["status"], "optional-unverified");
        fs::write(
            fixture.0.join("PLAN.md"),
            plan.replace("- [ ] **M2-14**", "- [x] **M2-14**"),
        )?;
        assert_eq!(fixture.readiness(true, None)?, Readiness::OptionalAbsent);
        for invalid in [
            plan.replace(
                "- [ ] **M2-14** Strict client-source parity machinery:",
                "- [x] **M2-14** historical oracle parity claim:",
            ),
            format!("{plan}\n- [x] **M2-05** completed\n"),
            format!("{plan}\n- [ ] **M2-05** duplicate\n"),
            plan.replace("- **M2-05 optional hosted refresh.**", "- [x] **M2-05**"),
        ] {
            fs::write(fixture.0.join("PLAN.md"), invalid)?;
            assert!(fixture.readiness(false, None).is_err());
        }
        Ok(())
    }

    #[test]
    fn q30_optional_policy_cannot_hide_removed_or_corrupt_evidence() -> Result<()> {
        let fixture = Fixture::new()?;
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?;
        let plan = fs::read_to_string(workspace.join("PLAN.md"))?;
        fs::write(fixture.0.join("PLAN.md"), &plan)?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "corrupt optional evidence")?;
        assert!(
            fixture
                .check(true, None, |_, directory| {
                    let _: Value =
                        serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
                    Ok(())
                })
                .is_err()
        );
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("optional evidence still belongs to history")?;
        fs::remove_file(manifest)?;
        assert!(fixture.readiness(true, None).is_err());
        fs::write(
            fixture.0.join("PLAN.md"),
            plan.replace(
                "- **Q30 optional oracle policy.**",
                "- Oracle refresh is optional.",
            ),
        )?;
        assert!(fixture.readiness(true, None).is_err());
        Ok(())
    }

    #[test]
    fn existing_corrupt_or_partial_outputs_never_become_initial_pending() -> Result<()> {
        let fixture = Fixture::new()?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "{ corrupt manifest")?;
        assert_eq!(fixture.readiness(false, None)?, Readiness::CheckExisting);
        let result = fixture.check(false, None, |_, directory| {
            let _: Value = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
            Ok(())
        });
        assert!(result.is_err());
        fs::remove_file(manifest)?;
        fs::write(fixture.0.join("vendor/oracle/reads.json"), "partial output")?;
        assert_eq!(fixture.readiness(false, None)?, Readiness::CheckExisting);
        assert!(
            fixture
                .check(false, None, |_, directory| {
                    fs::read(directory.join("manifest.json"))?;
                    Ok(())
                })
                .is_err()
        );
        // The real standalone verifier is still fail-closed too.
        assert!(super::super::check(&fixture.0, &fixture.0.join("vendor/oracle")).is_err());
        Ok(())
    }

    #[test]
    fn explicit_old_base_cannot_hide_head_evidence_or_its_removal_commit() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.git(&["rev-parse", "HEAD"])?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "corrupt historical artifact")?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("historical evidence")?;
        fs::remove_file(&manifest)?;
        assert!(fixture.readiness(false, Some(initial.trim())).is_err());
        fixture.git(&["add", "-u"])?;
        fixture.commit("removed evidence")?;
        assert!(fixture.readiness(false, Some(initial.trim())).is_err());
        Ok(())
    }

    #[test]
    fn previously_committed_partial_outputs_also_prevent_bootstrap() -> Result<()> {
        let fixture = Fixture::new()?;
        let output = fixture.0.join("vendor/oracle/intents.json");
        fs::write(&output, "partial historical output")?;
        fixture.git(&["add", "vendor/oracle/intents.json"])?;
        fixture.commit("partial evidence")?;
        fs::remove_file(output)?;
        assert!(fixture.readiness(false, None).is_err());
        assert!(fixture.readiness(false, Some("unknown-revision")).is_err());
        assert!(fixture.readiness(false, Some("--unsafe")).is_err());
        Ok(())
    }

    #[test]
    fn actual_event_base_is_validated_even_with_an_explicit_older_base() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.git(&["rev-parse", "HEAD"])?;
        fs::write(
            fixture.0.join("vendor/oracle/manifest.json"),
            "corrupt baseline artifact",
        )?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("event baseline evidence")?;
        let evidence = fixture.git(&["rev-parse", "HEAD"])?;
        fixture.git(&["checkout", "--quiet", "--detach", initial.trim()])?;
        for event in [
            json!({"before":evidence.trim()}),
            json!({"pull_request":{"base":{"sha":evidence.trim()}}}),
            json!({"merge_group":{"base_sha":evidence.trim()}}),
        ] {
            let prior =
                crate::parity::oracle_history_for_event(&fixture.0, &event, Some(initial.trim()))?;
            assert!(
                prior
                    .iter()
                    .any(|path| path.ends_with("vendor/oracle/manifest.json"))
            );
            assert!(
                readiness_with_history(&fixture.0, false, Some(initial.trim()), |root, base| {
                    crate::parity::oracle_history_for_event(root, &event, base)
                })
                .is_err()
            );
        }
        for event in [
            json!({}),
            json!({"before":""}),
            json!({"before":"000"}),
            json!({"before":"--unsafe"}),
            json!({"before":"f".repeat(40)}),
            json!({"pull_request":{"base":{"sha":"0".repeat(40)}}}),
            json!({"merge_group":{"base_sha":"0".repeat(40)}}),
            json!({"pull_request":{"base":{"sha":null}},"before":"0".repeat(40)}),
            json!({"pull_request":{},"before":initial.trim()}),
            json!({"merge_group":{"base_sha":42},"before":"0".repeat(40)}),
            json!({"merge_group":{},"before":initial.trim()}),
            json!({"pull_request":{"base":{"sha":initial.trim()}},"merge_group":{"base_sha":initial.trim()}}),
        ] {
            assert!(
                crate::parity::oracle_history_for_event(&fixture.0, &event, Some(initial.trim()))
                    .is_err()
            );
        }
        assert!(
            crate::parity::oracle_history_for_event(
                &fixture.0,
                &json!({"before":"0".repeat(40)}),
                Some(initial.trim())
            )?
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn manual_ci_baseline_is_the_selected_commits_parent_and_retains_history() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.git(&["rev-parse", "HEAD"])?;
        // The initial commit has no parent and cannot silently skip a baseline.
        assert!(crate::parity::dispatch_baseline_for_test(&fixture.0, initial.trim()).is_err());
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "historical evidence")?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("manual baseline evidence")?;
        let evidence = fixture.git(&["rev-parse", "HEAD"])?;
        fs::remove_file(&manifest)?;
        fixture.git(&["add", "-u"])?;
        fixture.commit("selected manual commit removes evidence")?;
        let selected = fixture.git(&["rev-parse", "HEAD"])?;
        let baseline = crate::parity::dispatch_baseline_for_test(&fixture.0, selected.trim())?;
        assert_eq!(baseline, evidence.trim());
        // Local replacement refs must not rewrite the selected commit's parent.
        fixture.git(&["replace", selected.trim(), evidence.trim()])?;
        assert_eq!(
            crate::parity::dispatch_baseline_for_test(&fixture.0, selected.trim())?,
            evidence.trim()
        );
        assert!(
            readiness_with_history(&fixture.0, false, Some(initial.trim()), |root, base| {
                crate::parity::oracle_history_for_test(root, Some(&baseline), base)
            })
            .is_err()
        );
        for invalid in [
            "",
            "--unsafe",
            "main",
            &"0".repeat(40),
            &"f".repeat(40),
            initial.trim(),
        ] {
            assert!(crate::parity::dispatch_baseline_for_test(&fixture.0, invalid).is_err());
        }
        fixture.git(&["checkout", "--quiet", "--detach", evidence.trim()])?;
        assert!(crate::parity::dispatch_baseline_for_test(&fixture.0, selected.trim()).is_err());
        Ok(())
    }

    #[test]
    fn shallow_history_cannot_claim_first_manifest_absence() -> Result<()> {
        let fixture = Fixture::new()?;
        let head = fixture.git(&["rev-parse", "HEAD"])?;
        fs::write(fixture.0.join(".git/shallow"), head)?;
        let failure = fixture
            .readiness(false, None)
            .err()
            .context("shallow history falsely accepted initial staging")?;
        assert!(failure.to_string().contains("complete Git history"));
        Ok(())
    }

    #[test]
    fn merged_then_removed_evidence_cannot_be_pruned_from_history() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.git(&["rev-parse", "HEAD"])?;
        fixture.git(&["checkout", "--quiet", "-b", "evidence"])?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "corrupt historical output")?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("side-branch evidence")?;
        fs::remove_file(manifest)?;
        fixture.git(&["add", "-u"])?;
        fixture.commit("side-branch evidence removed")?;
        fixture.git(&["checkout", "--quiet", "-b", "work", initial.trim()])?;
        fs::write(fixture.0.join("work.txt"), "independent work")?;
        fixture.git(&["add", "work.txt"])?;
        fixture.commit("independent source")?;
        fixture.git(&[
            "-c",
            "user.name=Oracle readiness fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "merge",
            "--quiet",
            "--no-ff",
            "--no-edit",
            "evidence",
        ])?;
        assert!(!fixture.0.join("vendor/oracle/manifest.json").exists());
        assert!(fixture.readiness(false, Some(initial.trim())).is_err());
        Ok(())
    }

    #[test]
    fn root_output_is_detected_even_if_git_config_hides_root_diffs() -> Result<()> {
        let fixture = Fixture::new()?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "corrupt output introduced in root commit")?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.git(&[
            "-c",
            "user.name=Oracle readiness fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--amend",
            "--no-edit",
        ])?;
        fs::remove_file(manifest)?;
        fixture.git(&["config", "log.showRoot", "false"])?;
        assert!(fixture.readiness(false, None).is_err());
        Ok(())
    }

    #[test]
    fn replacement_refs_and_grafts_cannot_hide_committed_ancestry() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.git(&["rev-parse", "HEAD"])?;
        let manifest = fixture.0.join("vendor/oracle/manifest.json");
        fs::write(&manifest, "corrupt historical output")?;
        fixture.git(&["add", "vendor/oracle/manifest.json"])?;
        fixture.commit("historical output")?;
        fs::remove_file(manifest)?;
        fixture.git(&["add", "-u"])?;
        fixture.commit("removed output")?;
        let head = fixture.git(&["rev-parse", "HEAD"])?;
        fixture.git(&["replace", head.trim(), initial.trim()])?;
        assert!(fixture.readiness(false, None).is_err());
        fixture.git(&["replace", "--delete", head.trim()])?;
        fs::create_dir_all(fixture.0.join(".git/info"))?;
        fs::write(fixture.0.join(".git/info/grafts"), &head)?;
        assert!(fixture.readiness(false, None).is_err());
        Ok(())
    }
}
