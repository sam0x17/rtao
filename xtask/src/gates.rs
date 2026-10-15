//! Hosted GitHub pr-gates commands; full local gate runs are retired by Q30.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail, ensure};
use toml_edit::DocumentMut;

const JOBS: &[&str] = &["check", "no-host", "msrv", "features", "supply", "semver"];
const PROFILES: &[&str] = &[
    "validator",
    "miner",
    "owner",
    "validator,prometheus",
    "miner,redis",
    "owner,ledger",
    "intents-all",
    "blocking",
    "full",
];

#[derive(Debug, PartialEq, Eq)]
struct Options {
    job: Option<String>,
    list: bool,
}

fn options(args: Vec<String>) -> Result<Options> {
    let mut options = Options {
        job: None,
        list: false,
    };
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--list" if !options.list => options.list = true,
            "--job" if options.job.is_none() => {
                let job = args.next().context("--job needs a job name")?;
                ensure!(JOBS.contains(&job.as_str()), "unknown gate job {job:?}");
                options.job = Some(job);
            }
            _ => bail!(
                "usage: cargo xtask gates [--job check|no-host|msrv|features|supply|semver] [--list]"
            ),
        }
    }
    Ok(options)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    args: Vec<String>,
    tool: Option<&'static str>,
    // Only the semver comparison, after a verified pre-1.0 baseline, is advisory.
    informational: bool,
}

fn cargo(args: &[&str]) -> Step {
    Step {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        tool: None,
        informational: false,
    }
}

fn xtask(args: &[&str]) -> Step {
    let mut command = vec!["run", "--locked", "-p", "xtask", "--"];
    command.extend_from_slice(args);
    cargo(&command)
}

fn tool(name: &'static str, args: &[&str]) -> Step {
    let mut step = cargo(args);
    step.tool = Some(name);
    step
}

fn step_complete(plan: &str, id: &str) -> bool {
    plan.lines()
        .any(|line| line.starts_with(&format!("- [x] **{id}**")))
}

fn examples(root: &Path, plan: &str) -> Result<Vec<Step>> {
    let mut commands = Vec::new();
    for (id, name, profile) in [
        ("M2-18", "validator", "validator,prometheus"),
        ("M3-06", "miner", "miner,redis"),
        ("M4-17", "owner", "owner,ledger"),
    ] {
        let exists = root
            .join(format!("crates/rtao/examples/{name}.rs"))
            .is_file();
        ensure!(
            exists || !step_complete(plan, id),
            "{id} is marked complete but its {name} example is missing"
        );
        if exists {
            commands.push(cargo(&[
                "check",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
                "--features",
                profile,
                "--example",
                name,
            ]));
        }
    }
    Ok(commands)
}

fn steps(root: &Path, job: &str, plan: &str, baseline: Option<&str>) -> Result<Vec<Step>> {
    Ok(match job {
        "check" => {
            let mut steps = vec![
                xtask(&["import-upstream", "--verify-only"]),
                // The upstream core has its own pinned lock. Workspace fetch
                // alone cannot populate every version needed by its projection.
                xtask(&["pin", "--check", "--prepare-cache"]),
                xtask(&["pin", "--check"]),
                cargo(&["fmt", "--all", "--check"]),
                cargo(&[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--locked",
                    // Report independent target failures in one hosted run.
                    "--keep-going",
                    "--",
                    "-D",
                    "warnings",
                ]),
                xtask(&["deps-check"]),
                xtask(&["core-surface", "--check"]),
                xtask(&["staged", "--check"]),
                xtask(&["lint-upstream"]),
            ];
            // M0-27 explicitly defers regen to M1-04. Once introduced it cannot
            // silently disappear, even if its source file is later removed.
            if step_complete(plan, "M1-04") || root.join("xtask/src/regen.rs").is_file() {
                steps.push(xtask(&["regen", "--check"]));
            }
            if step_complete(plan, "M1-13") || root.join("xtask/src/compat.rs").is_file() {
                steps.push(xtask(&["compat", "--check"]));
            }
            if step_complete(plan, "M1-22") || root.join("xtask/src/probe.rs").is_file() {
                steps.push(xtask(&["probe", "pins", "--check"]));
            }
            steps.extend([
                xtask(&["extract", "--check"]),
                xtask(&["parity", "check"]),
                tool(
                    "cargo-nextest",
                    &[
                        "nextest",
                        "run",
                        "--workspace",
                        "--all-features",
                        "--locked",
                        "--profile",
                        "ci",
                    ],
                ),
                cargo(&["test", "--workspace", "--doc", "--all-features", "--locked"]),
            ]);
            steps
        }
        "no-host" => vec![
            cargo(&[
                "build",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
                "--features",
                "validator,prometheus",
            ]),
            cargo(&[
                "build",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
                "--features",
                "miner,redis",
            ]),
            xtask(&["deps-check", "--profile", "no-host"]),
            cargo(&[
                "clippy",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
                "--features",
                "validator,miner,prometheus",
                "--",
                "-D",
                "warnings",
            ]),
        ],
        "msrv" => vec![
            cargo(&[
                "+1.89",
                "check",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
                "--features",
                "validator,miner,prometheus",
            ]),
            cargo(&[
                "+1.89",
                "check",
                "--locked",
                "-p",
                "rtao",
                "--features",
                "full",
            ]),
            cargo(&["+1.89", "check", "--locked", "-p", "rtao-cli"]),
        ],
        "features" => {
            let mut steps = vec![
                tool(
                    "cargo-hack",
                    &[
                        "hack",
                        "check",
                        "--locked",
                        "-p",
                        "rtao",
                        "--each-feature",
                        "--no-dev-deps",
                    ],
                ),
                tool(
                    "cargo-hack",
                    &[
                        "hack",
                        "check",
                        "--locked",
                        "-p",
                        "rtao-intents",
                        "--each-feature",
                        "--no-dev-deps",
                    ],
                ),
                tool(
                    "cargo-hack",
                    &[
                        "hack",
                        "check",
                        "--locked",
                        "-p",
                        "rtao-btauth",
                        "-p",
                        "rtao-signer",
                        "--feature-powerset",
                        "--no-dev-deps",
                    ],
                ),
            ];
            for profile in PROFILES {
                steps.push(cargo(&[
                    "check",
                    "--locked",
                    "-p",
                    "rtao",
                    "--no-default-features",
                    "--features",
                    profile,
                ]));
            }
            steps.push(cargo(&[
                "check",
                "--locked",
                "-p",
                "rtao",
                "--no-default-features",
            ]));
            steps.extend(examples(root, plan)?);
            steps
        }
        "supply" => vec![
            tool(
                "cargo-deny",
                &["deny", "--locked", "--all-features", "check"],
            ),
            {
                let mut step = xtask(&["supply"]);
                step.tool = Some("cargo-audit");
                step
            },
        ],
        "semver" => {
            if let Some(baseline) = baseline {
                let version = semver::Version::parse(
                    baseline
                        .strip_prefix("rtao-v")
                        .context("invalid release tag prefix")?,
                )?;
                let mut step = tool(
                    "cargo-semver-checks",
                    &["semver-checks", "--baseline-rev", baseline],
                );
                step.informational = version.major == 0;
                vec![step]
            } else {
                Vec::new()
            }
        }
        _ => bail!("unknown gate job {job:?}"),
    })
}

struct Environment {
    root: PathBuf,
    temporary: PathBuf,
    path: OsString,
    tools: DocumentMut,
    feature_root: Option<PathBuf>,
}

impl Environment {
    fn new(root: &Path) -> Result<Self> {
        ensure!(
            std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true"),
            "Q30 requires hosted GitHub CI for gates; inspect xtask/src/gates.rs locally instead"
        );
        // GitHub provides an isolated job root through this environment variable.
        let temporary = PathBuf::from(std::env::var_os("RTAO_TEMP_ROOT").context(
            "set RTAO_TEMP_ROOT to the recorded task-scoped temporary root before running gates",
        )?);
        ensure!(temporary.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
        fs::create_dir_all(temporary.join("tmp"))?;
        let tool_bin = temporary.join("ci/tools/bin");
        fs::create_dir_all(&tool_bin)?;
        let mut paths = vec![tool_bin, temporary.join("tools/bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        Ok(Self {
            root: root.to_owned(),
            temporary,
            path: std::env::join_paths(paths)?,
            tools: fs::read_to_string(root.join("ci/tool-versions.toml"))?.parse()?,
            feature_root: None,
        })
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("PATH", &self.path)
            .env("RTAO_TEMP_ROOT", &self.temporary)
            .env("TMPDIR", self.temporary.join("tmp"))
            .env("RTAO_TEST_TMPDIR", self.temporary.join("tmp"))
            .env("CARGO_TARGET_DIR", self.temporary.join("build"))
            .env("CARGO_INCREMENTAL", "0")
            .env("CARGO_PROFILE_DEV_DEBUG", "0")
            .env("RUSTUP_AUTO_INSTALL", "0");
        command
    }

    fn require_msrv(&self) -> Result<()> {
        let output = self
            .command("rustup")
            .args(["run", "1.89", "rustc", "--version"])
            .output()?;
        ensure!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).starts_with("rustc 1.89."),
            "Rust 1.89 is required for msrv; gates never installs toolchains or changes host settings"
        );
        Ok(())
    }

    fn ensure_tool(&self, name: &str) -> Result<()> {
        let version = self.tools["tools"][name]
            .as_str()
            .with_context(|| format!("missing exact version for {name}"))?;
        semver::Version::parse(version)
            .with_context(|| format!("invalid exact tool version for {name}"))?;
        let subcommand = name
            .strip_prefix("cargo-")
            .context("invalid cargo tool name")?;
        let matches = || -> Result<bool> {
            let output = self
                .command("cargo")
                .args([subcommand, "--version"])
                .output()?;
            Ok(output.status.success() && version_matches(&output.stdout, version))
        };
        if !matches()? {
            let install_root = self.temporary.join("ci/tools");
            println!(
                "gates: install {name} {version} under {}",
                install_root.display()
            );
            let status = self
                .command("cargo")
                .args(["install", name, "--version", version, "--locked", "--root"])
                .arg(&install_root)
                .env("CARGO_TARGET_DIR", self.temporary.join("ci/install-build"))
                .status()?;
            ensure!(
                status.success(),
                "installing pinned {name} {version} failed"
            );
            ensure!(
                matches()?,
                "installed {name} does not report pinned version {version}"
            );
        }
        Ok(())
    }

    fn feature_workspace(&mut self) -> Result<PathBuf> {
        if let Some(root) = &self.feature_root {
            return Ok(root.clone());
        }
        let original = fs::read_to_string(self.root.join("Cargo.lock"))?;
        // A standalone feature run must reject invalid dev features and stale
        // locks in the original graph before pruning can hide either error.
        let output = self
            .command("cargo")
            .args([
                "metadata",
                "--offline",
                "--locked",
                "--all-features",
                "--format-version",
                "1",
            ])
            .output()?;
        ensure!(
            output.status.success(),
            "validating original locked feature graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            fs::read_to_string(self.root.join("Cargo.lock"))? == original,
            "source lock changed during original feature graph validation"
        );
        println!("gates: original locked feature graph validated before dev pruning");
        let destination = self
            .temporary
            .join(format!("ci/features-worktree-{}", std::process::id()));
        ensure!(
            !destination.exists(),
            "feature scratch workspace already exists: {}",
            destination.display()
        );
        fs::create_dir_all(&destination)?;
        let files = self
            .command("git")
            .args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ])
            .output()?;
        ensure!(
            files.status.success(),
            "listing feature workspace source files failed"
        );
        for relative in files
            .stdout
            .split(|byte| *byte == 0)
            .filter(|bytes| !bytes.is_empty())
        {
            let relative = Path::new(std::str::from_utf8(relative)?);
            ensure!(
                relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))),
                "invalid source path in feature snapshot"
            );
            let source = self.root.join(relative);
            // git also lists tracked files deleted in the current worktree.
            if !source.exists() {
                continue;
            }
            ensure!(
                fs::symlink_metadata(&source)?.is_file(),
                "feature workspace source must be a regular file: {}",
                source.display()
            );
            let target = destination.join(relative);
            fs::create_dir_all(target.parent().context("snapshot file has no parent")?)?;
            fs::copy(source, &target)?;
            if relative
                .file_name()
                .is_some_and(|name| name == "Cargo.toml")
            {
                let mut manifest: DocumentMut = fs::read_to_string(&target)?.parse()?;
                strip_dev_dependencies(&mut manifest);
                fs::write(target, manifest.to_string())?;
            }
        }
        // cargo-hack's --no-dev-deps temporarily rewrites manifests. Cargo must
        // first prune dev-only lock entries in this disposable copy; the source
        // workspace and its lock remain untouched. Resolving offline preserves
        // every retained exact package, enforced below before any build runs.
        let output = self
            .command("cargo")
            .current_dir(&destination)
            .args([
                "metadata",
                "--offline",
                "--all-features",
                "--format-version",
                "1",
            ])
            .output()?;
        ensure!(
            output.status.success(),
            "preparing locked no-dev feature graph failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        verify_pruned_lock(
            &original,
            &fs::read_to_string(destination.join("Cargo.lock"))?,
        )?;
        println!(
            "gates: no-dev feature snapshot at {}; every retained lock package matches the source lock",
            destination.display()
        );
        self.feature_root = Some(destination.clone());
        Ok(destination)
    }

    fn run(&mut self, step: &Step) -> Result<bool> {
        if let Some(name) = step.tool {
            self.ensure_tool(name)?;
        }
        let args: Vec<OsString> = step
            .args
            .iter()
            .map(|arg| {
                arg.strip_prefix("$RTAO_TEMP_ROOT/").map_or_else(
                    || OsString::from(arg),
                    |relative| self.temporary.join(relative).into_os_string(),
                )
            })
            .collect();
        let mut command = self.command("cargo");
        if step.tool == Some("cargo-hack") {
            command.current_dir(self.feature_workspace()?);
        }
        let status = command.args(args).status()?;
        accept_status(step, status)
    }
}

fn strip_dev_dependencies(manifest: &mut DocumentMut) {
    let mut retained = BTreeSet::new();
    let mut collect = |table: &dyn toml_edit::TableLike| {
        for kind in ["dependencies", "build-dependencies"] {
            if let Some(dependencies) = table.get(kind).and_then(toml_edit::Item::as_table_like) {
                retained.extend(dependencies.iter().map(|(name, _)| name.to_owned()));
            }
        }
    };
    collect(manifest.as_table());
    if let Some(targets) = manifest
        .get("target")
        .and_then(toml_edit::Item::as_table_like)
    {
        for (_, target) in targets.iter() {
            if let Some(target) = target.as_table_like() {
                collect(target);
            }
        }
    }
    let mut removed = BTreeSet::new();
    let mut strip = |table: &mut dyn toml_edit::TableLike| {
        if let Some(dependencies) = table.remove("dev-dependencies")
            && let Some(dependencies) = dependencies.as_table_like()
        {
            for (name, _) in dependencies.iter() {
                if !retained.contains(name) {
                    removed.insert(name.to_owned());
                }
            }
        }
    };
    strip(manifest.as_table_mut());
    if let Some(targets) = manifest
        .get_mut("target")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        for (_, target) in targets.iter_mut() {
            if let Some(target) = target.as_table_like_mut() {
                strip(target);
            }
        }
    }
    if let Some(features) = manifest
        .get_mut("features")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        // Cargo has validated the original graph, including dependency feature
        // names. Only forwarding edges to removed aliases can now be pruned.
        for (_, feature) in features.iter_mut() {
            if let Some(edges) = feature.as_array_mut() {
                edges.retain(|edge| {
                    let Some((dependency, _)) = edge.as_str().and_then(|edge| edge.split_once('/'))
                    else {
                        return true;
                    };
                    let dependency = dependency.strip_suffix('?').unwrap_or(dependency);
                    !removed.contains(dependency)
                });
            }
        }
    }
}

type LockPackage = (String, String, Option<String>, Option<String>);

fn lock_packages(source: &str) -> Result<BTreeSet<LockPackage>> {
    let lock: DocumentMut = source.parse()?;
    lock["package"]
        .as_array_of_tables()
        .context("lockfile has no package records")?
        .iter()
        .map(|package| {
            Ok((
                package["name"]
                    .as_str()
                    .context("lock package name missing")?
                    .to_owned(),
                package["version"]
                    .as_str()
                    .context("lock package version missing")?
                    .to_owned(),
                package
                    .get("source")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_owned),
                package
                    .get("checksum")
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect()
}

fn verify_pruned_lock(original: &str, pruned: &str) -> Result<()> {
    let original = lock_packages(original)?;
    let pruned = lock_packages(pruned)?;
    ensure!(!pruned.is_empty(), "feature snapshot lock is empty");
    if let Some(package) = pruned.difference(&original).next() {
        bail!("no-dev feature resolution changed a pinned package: {package:?}");
    }
    Ok(())
}

fn version_matches(output: &[u8], version: &str) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .next()
        .is_some_and(|line| line.split_whitespace().nth(1) == Some(version))
}

fn accept_status(step: &Step, status: ExitStatus) -> Result<bool> {
    if status.success() {
        return Ok(true);
    }
    if step.informational && status.code().is_some() {
        eprintln!("::warning::semver comparison failed (informational before 1.0)");
        return Ok(false);
    }
    bail!("gate failed ({status}): cargo {}", step.args.join(" "))
}

fn execute(steps: &[Step], mut run: impl FnMut(&Step) -> Result<bool>) -> Result<bool> {
    let mut clean = true;
    for step in steps {
        println!("gates: cargo {}", step.args.join(" "));
        clean &= run(step)?;
    }
    Ok(clean)
}

fn baseline(root: &Path) -> Result<Option<String>> {
    let tags = Command::new("git")
        .current_dir(root)
        .args(["tag", "--list", "rtao-v*"])
        .output()?;
    ensure!(tags.status.success(), "listing release tags failed");
    if tags.stdout.is_empty() {
        return Ok(None);
    }
    let output = Command::new("git")
        .current_dir(root)
        .args(["describe", "--tags", "--abbrev=0", "--match", "rtao-v*"])
        .output()?;
    ensure!(
        output.status.success(),
        "release tags exist but no baseline is reachable; fetch complete tag history"
    );
    Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()))
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    if args == ["--help"] || args == ["-h"] {
        println!(
            "cargo xtask gates [--job check|no-host|msrv|features|supply|semver] [--list]\nRuns Rust-only pr-gates in hosted GitHub CI, stopping on a blocking failure.\nRTAO_TEMP_ROOT must name the isolated hosted job root. Pinned tools install only there; the hosted MSRV job provides Rust 1.89.\n--list prints commands without executing gates or installing tools; invoking Cargo may still compile xtask. Inspect xtask/src/gates.rs locally instead.\nFull local gates are retired by Q30. Push source changes for applicable hosted checks; do not duplicate them locally."
        );
        return Ok(());
    }
    let options = options(args)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("missing workspace root")?;
    let plan = fs::read_to_string(root.join("PLAN.md"))?;
    let jobs: Vec<&str> = options
        .job
        .as_deref()
        .map_or_else(|| JOBS.to_vec(), |job| vec![job]);
    let baseline = if jobs.contains(&"semver") {
        baseline(root)?
    } else {
        None
    };
    let mut environment = if options.list {
        None
    } else {
        Some(Environment::new(root)?)
    };
    let mut clean = true;
    for job in jobs {
        println!("gates: job {job}");
        let steps = steps(root, job, &plan, baseline.as_deref())?;
        if job == "semver" && steps.is_empty() {
            println!("gates: no rtao-v* tag yet; semver starts after the first tag");
        }
        if let Some(environment) = &mut environment {
            if job == "msrv" {
                environment.require_msrv()?;
            }
            clean &= execute(&steps, |step| environment.run(step))?;
        } else {
            for step in steps {
                println!("cargo {}", step.args.join(" "));
            }
        }
    }
    if !options.list {
        if clean {
            println!("gates: all selected blocking checks passed");
        } else {
            println!(
                "gates: all selected blocking checks passed; pre-1.0 semver comparison reported an advisory failure"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURE_FIXTURE: &str = r#"
[package]
name = "gate-feature-fixture"
version = "0.1.0"
edition = "2021"
[workspace]
[features]
only_dev = ["normal_alias/collision"]
retained = ["dep:normal_alias", "normal_alias/flag", "normal_alias?/flag", "build_alias/flag", "shared/flag", "target_alias/flag", "target_build_alias/flag", "only_dev"]
dev_flags = ["only_dev/flag", "only_dev/café", "only_target/flag"]
[dependencies]
normal_alias = { package = "actual-normal", path = "normal", optional = true }
shared = { package = "actual-shared", path = "shared" }
[build-dependencies]
build_alias = { package = "actual-build", path = "build" }
[dev-dependencies]
only_dev = { package = "actual-dev", path = "dev" }
normal_alias = { package = "actual-normal", path = "normal" }
shared = { package = "actual-shared", path = "shared" }
target_alias = { package = "actual-target", path = "target-normal" }
target_build_alias = { package = "actual-target-build", path = "target-build" }
[target.'cfg(unix)'.dependencies]
target_alias = { package = "actual-target", path = "target-normal", optional = true }
[target.'cfg(unix)'.build-dependencies]
target_build_alias = { package = "actual-target-build", path = "target-build" }
[target.'cfg(unix)'.dev-dependencies]
only_target = { package = "actual-target-dev", path = "target-dev" }
"#;

    fn with_feature_fixture(test: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let root = base.join("tmp").join(format!(
            "gate-feature-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.parent().context("fixture parent missing")?)?;
        fs::create_dir(&root)?;
        let result = test(&root);
        fs::remove_dir_all(&root)?;
        result
    }

    fn write_feature_fixture(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("Cargo.toml"), FEATURE_FIXTURE)?;
        fs::write(
            root.join("src/lib.rs"),
            "pub fn check() { normal_alias::enabled(); normal_alias::collision(); shared::enabled(); #[cfg(unix)] target_alias::enabled(); }\n",
        )?;
        fs::write(
            root.join("build.rs"),
            "fn main() { build_alias::enabled(); #[cfg(unix)] target_build_alias::enabled(); }\n",
        )?;
        for (directory, name) in [
            ("normal", "actual-normal"),
            ("shared", "actual-shared"),
            ("build", "actual-build"),
            ("dev", "actual-dev"),
            ("target-normal", "actual-target"),
            ("target-build", "actual-target-build"),
            ("target-dev", "actual-target-dev"),
        ] {
            let dependency = root.join(directory);
            fs::create_dir_all(dependency.join("src"))?;
            fs::write(
                dependency.join("Cargo.toml"),
                format!(
                    "[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2021'\n[features]\nflag = []\ncollision = []\n'café' = []\n"
                ),
            )?;
            fs::write(
                dependency.join("src/lib.rs"),
                "#[cfg(feature = \"flag\")] pub fn enabled() {}\n#[cfg(feature = \"collision\")] pub fn collision() {}\n",
            )?;
        }
        Ok(())
    }

    fn fixture_cargo(root: &Path, args: &[&str]) -> Result<std::process::Output> {
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args(args)
            .env("PYO3_NO_PYTHON", "1")
            .env("CARGO_TARGET_DIR", root.join("build-artifacts"))
            .output()?;
        println!(
            "fixture cargo {}: {}\n{}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    fn fixture_metadata(root: &Path) -> Result<std::process::Output> {
        fixture_cargo(
            root,
            &[
                "metadata",
                "--offline",
                "--all-features",
                "--format-version",
                "1",
            ],
        )
    }

    #[test]
    fn no_dev_snapshot_preserves_real_cargo_features_and_lock_provenance() -> Result<()> {
        with_feature_fixture(|root| {
            let source = root.join("source");
            let snapshot = root.join("snapshot");
            write_feature_fixture(&source)?;
            write_feature_fixture(&snapshot)?;
            ensure!(
                fixture_metadata(&source)?.status.success(),
                "original feature fixture was rejected"
            );
            let source_manifest = fs::read_to_string(source.join("Cargo.toml"))?;
            let source_lock = fs::read_to_string(source.join("Cargo.lock"))?;
            fs::write(snapshot.join("Cargo.lock"), &source_lock)?;
            let mut old: DocumentMut = FEATURE_FIXTURE.parse()?;
            old.remove("dev-dependencies");
            old["target"]["cfg(unix)"]
                .as_table_mut()
                .context("target table missing")?
                .remove("dev-dependencies");
            fs::write(snapshot.join("Cargo.toml"), old.to_string())?;
            let rejected = fixture_metadata(&snapshot)?;
            ensure!(
                !rejected.status.success()
                    && String::from_utf8_lossy(&rejected.stderr).contains("only_dev"),
                "old no-dev harness unexpectedly accepted the graph"
            );
            let mut repaired: DocumentMut = FEATURE_FIXTURE.parse()?;
            strip_dev_dependencies(&mut repaired);
            fs::write(snapshot.join("Cargo.toml"), repaired.to_string())?;
            let accepted = fixture_metadata(&snapshot)?;
            ensure!(
                accepted.status.success(),
                "repaired Cargo metadata rejected the retained feature graph"
            );
            let metadata: serde_json::Value = serde_json::from_slice(&accepted.stdout)?;
            let package = metadata["packages"]
                .as_array()
                .context("packages missing")?
                .iter()
                .find(|package| package["name"] == "gate-feature-fixture")
                .context("fixture package missing")?;
            for name in ["only_dev", "retained", "dev_flags"] {
                ensure!(
                    package["features"].get(name).is_some(),
                    "explicit feature name {name} was removed"
                );
            }
            ensure!(
                fixture_cargo(
                    &snapshot,
                    &[
                        "check",
                        "--offline",
                        "--locked",
                        "--no-default-features",
                        "--features",
                        "retained"
                    ]
                )?
                .status
                .success(),
                "retained normal/build/target features no longer compile"
            );
            let disabled = fixture_cargo(
                &snapshot,
                &["check", "--offline", "--locked", "--no-default-features"],
            )?;
            ensure!(
                !disabled.status.success()
                    && String::from_utf8_lossy(&disabled.stderr).contains("enabled"),
                "fixture did not require the forwarded build feature"
            );
            let pruned = fs::read_to_string(snapshot.join("Cargo.lock"))?;
            verify_pruned_lock(&source_lock, &pruned)?;
            ensure!(
                verify_pruned_lock(
                    &source_lock,
                    &pruned.replace("version = \"0.1.0\"", "version = \"0.1.1\"")
                )
                .is_err(),
                "changed resolved package escaped provenance validation"
            );
            ensure!(
                fs::read_to_string(source.join("Cargo.toml"))? == source_manifest,
                "original manifest changed"
            );
            ensure!(
                fs::read_to_string(source.join("Cargo.lock"))? == source_lock,
                "original lock changed"
            );
            Ok(())
        })
    }

    #[test]
    fn no_dev_snapshot_prunes_removed_weak_forwarding() -> Result<()> {
        with_feature_fixture(|root| {
            write_feature_fixture(root)?;
            let mut manifest: DocumentMut = FEATURE_FIXTURE.parse()?;
            manifest["features"]["dev_flags"]
                .as_array_mut()
                .context("dev features missing")?
                .extend(["only_dev?/flag", "only_target?/flag"]);
            strip_dev_dependencies(&mut manifest);
            fs::write(root.join("Cargo.toml"), manifest.to_string())?;
            ensure!(
                fixture_metadata(root)?.status.success(),
                "removed weak forwarding still invalidates Cargo metadata"
            );
            ensure!(
                fixture_cargo(root, &["check", "--offline", "--locked", "--all-features"])?
                    .status
                    .success(),
                "live weak or target feature forwarding was lost"
            );
            Ok(())
        })
    }

    #[test]
    fn no_dev_snapshot_preserves_unknown_and_malformed_edges() -> Result<()> {
        with_feature_fixture(|root| {
            for (index, edge) in [
                "missing/flag",
                "actual-dev/flag",
                "only_dev//flag",
                "only_dev/flag/extra",
                "only_dev?/flag?",
                "only_dev/",
                "only_dev/flag!",
                "only_dev/flag:bad",
                "only_dev??/flag",
                "dep:only_dev/flag",
                "dep:only_dev",
                "unknown-feature",
            ]
            .into_iter()
            .enumerate()
            {
                reject_original_feature_graph(&root.join(index.to_string()), Some(edge), false)?;
            }
            Ok(())
        })
    }

    fn reject_original_feature_graph(
        root: &Path,
        edge: Option<&str>,
        corrupt_lock: bool,
    ) -> Result<()> {
        let source = root.join("source");
        write_feature_fixture(&source)?;
        fs::write(source.join(".gitignore"), "build-artifacts/\n")?;
        ensure!(
            Command::new("git")
                .current_dir(&source)
                .args(["init", "-q", "--initial-branch=feature-fixture"])
                .status()?
                .success(),
            "fixture Git initialization failed"
        );
        ensure!(
            fixture_metadata(&source)?.status.success(),
            "original baseline fixture invalid"
        );
        if let Some(edge) = edge {
            let mut manifest: DocumentMut = FEATURE_FIXTURE.parse()?;
            let mut invalid = toml_edit::Array::new();
            invalid.push(edge);
            manifest["features"]["invalid"] = toml_edit::value(invalid);
            fs::write(source.join("Cargo.toml"), manifest.to_string())?;
        }
        if corrupt_lock {
            let path = source.join("Cargo.lock");
            let lock = fs::read_to_string(&path)?;
            fs::write(
                path,
                lock.replace("version = \"0.1.0\"", "version = \"0.1.1\""),
            )?;
        }
        let manifest = fs::read(source.join("Cargo.toml"))?;
        let lock = fs::read(source.join("Cargo.lock"))?;
        let original = fixture_cargo(
            &source,
            &[
                "metadata",
                "--offline",
                "--locked",
                "--all-features",
                "--format-version",
                "1",
            ],
        )?;
        ensure!(
            !original.status.success(),
            "invalid original Cargo graph was accepted"
        );
        let temporary = root.join("task");
        fs::create_dir_all(temporary.join("tmp"))?;
        let mut environment = Environment {
            root: source.clone(),
            temporary,
            path: std::env::var_os("PATH").unwrap_or_default(),
            tools: DocumentMut::new(),
            feature_root: None,
        };
        let result = environment.feature_workspace();
        ensure!(
            result.is_err(),
            "standalone preparation accepted invalid original graph {edge:?}"
        );
        let error = result.err().context("original graph rejection missing")?;
        println!("standalone preparation rejected original graph: {error:#}");
        ensure!(
            error
                .to_string()
                .contains("validating original locked feature graph"),
            "original error was masked by later preparation: {error:#}"
        );
        ensure!(
            environment.feature_root.is_none(),
            "invalid source produced a cached feature workspace"
        );
        ensure!(
            !environment
                .temporary
                .join(format!("ci/features-worktree-{}", std::process::id()))
                .exists(),
            "source validation occurred after copying or pruning"
        );
        ensure!(
            fs::read(source.join("Cargo.toml"))? == manifest,
            "original manifest changed during rejection"
        );
        ensure!(
            fs::read(source.join("Cargo.lock"))? == lock,
            "original lock changed during rejection"
        );
        Ok(())
    }

    #[test]
    fn no_dev_snapshot_validates_original_graph_before_pruning() -> Result<()> {
        with_feature_fixture(|root| {
            for (index, edge) in [
                "only_dev/not_defined",
                "only_target/not_defined",
                "only_dev?/flag",
                "dep:only_dev",
            ]
            .into_iter()
            .enumerate()
            {
                reject_original_feature_graph(&root.join(index.to_string()), Some(edge), false)?;
            }
            reject_original_feature_graph(&root.join("lock-drift"), None, true)
        })
    }

    #[test]
    fn no_dev_snapshot_preserves_weak_only_dependency_behavior() -> Result<()> {
        with_feature_fixture(|root| {
            fs::create_dir_all(root.join("src"))?;
            fs::create_dir_all(root.join("dep/src"))?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname='weak-fixture'\nversion='0.1.0'\nedition='2021'\n[workspace]\n[dependencies]\noptional_alias={package='weak-dependency',path='dep',optional=true}\n[dev-dependencies]\noptional_alias={package='weak-dependency',path='dep'}\n[features]\nactivate=['dep:optional_alias','optional_alias/permit']\nweak_forward=['optional_alias?/flag']\n",
            )?;
            fs::write(
                root.join("src/lib.rs"),
                "#[cfg(feature=\"activate\")] pub fn proof() { optional_alias::enabled(); }\n",
            )?;
            fs::write(
                root.join("dep/Cargo.toml"),
                "[package]\nname='weak-dependency'\nversion='0.1.0'\nedition='2021'\n[features]\nflag=[]\npermit=[]\n",
            )?;
            fs::write(
                root.join("dep/src/lib.rs"),
                "#[cfg(not(feature=\"permit\"))] compile_error!(\"optional dependency activated without permission\");\n#[cfg(feature=\"flag\")] pub fn enabled() {}\n",
            )?;
            ensure!(
                fixture_metadata(root)?.status.success(),
                "original weak fixture invalid"
            );
            let mut manifest: DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
            strip_dev_dependencies(&mut manifest);
            fs::write(root.join("Cargo.toml"), manifest.to_string())?;
            ensure!(
                fixture_cargo(
                    root,
                    &[
                        "check",
                        "--offline",
                        "--locked",
                        "--no-default-features",
                        "--features",
                        "weak_forward"
                    ]
                )?
                .status
                .success(),
                "inactive weak dependency failed to compile"
            );
            let disabled = fixture_cargo(
                root,
                &[
                    "check",
                    "--offline",
                    "--locked",
                    "--no-default-features",
                    "--features",
                    "activate",
                ],
            )?;
            ensure!(
                !disabled.status.success()
                    && String::from_utf8_lossy(&disabled.stderr).contains("enabled"),
                "weak feature was not required for compilation"
            );
            ensure!(
                fixture_cargo(
                    root,
                    &[
                        "check",
                        "--offline",
                        "--locked",
                        "--no-default-features",
                        "--features",
                        "activate,weak_forward"
                    ]
                )?
                .status
                .success(),
                "retained weak forwarding no longer enables the dependency feature"
            );
            Ok(())
        })
    }

    #[test]
    fn oracle_readiness_never_removes_or_downgrades_gate_commands() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let pending = "- [ ] **M2-05** pending\n- [ ] **M2-14** pending";
        let complete = "- [x] **M2-05** complete\n- [x] **M2-14** complete";
        for job in JOBS {
            ensure!(
                steps(root, job, pending, None)? == steps(root, job, complete, None)?,
                "oracle staging changed gate commands for {job}"
            );
        }
        let checks = steps(root, "check", pending, None)?;
        for command in [
            vec!["pin", "--check"],
            vec!["extract", "--check"],
            vec!["parity", "check"],
        ] {
            ensure!(
                checks.iter().any(|step| step.args.ends_with(
                    &command
                        .iter()
                        .map(|arg| (*arg).to_owned())
                        .collect::<Vec<_>>()
                ) && !step.informational),
                "initial oracle staging removed mandatory command {command:?}"
            );
        }
        ensure!(
            checks
                .iter()
                .any(|step| step.args.contains(&"nextest".into())
                    && step.args.contains(&"--all-features".into())),
            "ordinary all-feature behavior tests disappeared"
        );
        Ok(())
    }

    #[test]
    fn refuses_unknown_or_ambiguous_job_selectors() -> Result<()> {
        ensure!(options(vec!["--job".into(), "e2e".into()]).is_err());
        ensure!(options(vec!["--job".into()]).is_err());
        ensure!(
            options(vec![
                "--job".into(),
                "check".into(),
                "--job".into(),
                "supply".into()
            ])
            .is_err()
        );
        ensure!(
            options(vec!["--list".into(), "--job".into(), "msrv".into()])?
                == Options {
                    job: Some("msrv".into()),
                    list: true
                }
        );
        Ok(())
    }

    #[test]
    fn blocking_failure_stops_before_next_command() -> Result<()> {
        let steps = [cargo(&["first"]), cargo(&["red"]), cargo(&["unreachable"])];
        let mut observed = Vec::new();
        let result = execute(&steps, |step| {
            observed.push(step.args[0].clone());
            ensure!(step.args != ["red"], "fixture gate failure");
            Ok(true)
        });
        ensure!(result.is_err());
        ensure!(observed == ["first", "red"]);
        Ok(())
    }

    #[test]
    fn keeps_msrv_on_shipped_crates_and_all_role_profiles() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("missing root")?;
        let msrv = steps(root, "msrv", "", None)?;
        ensure!(msrv.len() == 3);
        ensure!(msrv.iter().all(|step| {
            step.args.first().is_some_and(|arg| arg == "+1.89")
                && !step
                    .args
                    .iter()
                    .any(|arg| arg == "--workspace" || arg == "xtask" || arg == "rtao-testkit")
        }));
        let features = steps(root, "features", "", None)?;
        for profile in PROFILES {
            ensure!(features.iter().any(|step| {
                step.args
                    .windows(2)
                    .any(|pair| pair == ["--features", profile])
            }));
        }
        ensure!(features.iter().any(|step| {
            step.args
                .last()
                .is_some_and(|arg| arg == "--no-default-features")
        }));
        Ok(())
    }

    #[test]
    fn release_baseline_controls_only_semver_failure_policy() -> Result<()> {
        let root = Path::new(".");
        ensure!(steps(root, "semver", "", None)?.is_empty());
        ensure!(steps(root, "semver", "", Some("rtao-v0.2.0"))?[0].informational);
        ensure!(!steps(root, "semver", "", Some("rtao-v1.0.0"))?[0].informational);
        ensure!(steps(root, "semver", "", Some("rtao-vbad")).is_err());
        ensure!(
            steps(root, "supply", "", None)?
                .iter()
                .all(|step| !step.informational)
        );
        Ok(())
    }

    #[test]
    fn supply_keeps_deny_and_pins_the_guarded_raw_audit() -> Result<()> {
        let supply = steps(Path::new("."), "supply", "", None)?;
        ensure!(supply.len() == 2);
        ensure!(
            supply[0]
                == tool(
                    "cargo-deny",
                    &["deny", "--locked", "--all-features", "check"]
                )
        );
        ensure!(supply[1].tool == Some("cargo-audit") && supply[1].args == xtask(&["supply"]).args);
        ensure!(
            supply
                .iter()
                .all(|step| !step.informational && !step.args.iter().any(|arg| arg == "--ignore"))
        );
        Ok(())
    }

    #[test]
    fn completed_future_checks_cannot_become_vacuous() -> Result<()> {
        let absent = Path::new("/path-that-is-not-the-rtao-worktree");
        ensure!(examples(absent, "- [x] **M2-18** shipped").is_err());
        ensure!(examples(absent, "- [ ] **M2-18** later")?.is_empty());
        ensure!(
            steps(absent, "check", "- [x] **M1-04** shipped", None)?
                .iter()
                .any(|step| step.args.ends_with(&["regen".into(), "--check".into()]))
        );
        ensure!(
            !steps(absent, "check", "- [ ] **M1-04** later", None)?
                .iter()
                .any(|step| step.args.iter().any(|arg| arg == "regen"))
        );
        ensure!(
            steps(absent, "check", "- [x] **M1-13** shipped", None)?
                .iter()
                .any(|step| step.args.ends_with(&["compat".into(), "--check".into()]))
        );
        ensure!(
            steps(absent, "check", "- [x] **M1-22** shipped", None)?
                .iter()
                .any(|step| step.args.ends_with(&[
                    "probe".into(),
                    "pins".into(),
                    "--check".into()
                ]))
        );
        Ok(())
    }

    #[test]
    fn exact_tool_version_does_not_accept_prefix_or_extra_version() -> Result<()> {
        ensure!(version_matches(
            b"cargo-nextest 0.9.143 (hash)\n",
            "0.9.143"
        ));
        ensure!(version_matches(b"cargo-audit-audit 0.22.2\n", "0.22.2"));
        ensure!(!version_matches(b"cargo-nextest 0.9.1430\n", "0.9.143"));
        ensure!(!version_matches(b"error: require 0.9.143\n", "0.9.143"));
        Ok(())
    }

    #[test]
    fn no_dev_snapshot_may_prune_but_never_change_package_provenance() -> Result<()> {
        let pinned = "[[package]]\nname = 'crypto'\nversion = '1.0.0'\nsource = 'git+https://example.test/core#1234'\nchecksum = 'abcd'\n";
        let original = format!("{pinned}\n[[package]]\nname = 'dev-only'\nversion = '2.0.0'\n");
        verify_pruned_lock(&original, pinned)?;
        for changed in [
            pinned.replace("1.0.0", "1.0.1"),
            pinned.replace("#1234", "#5678"),
            pinned.replace("abcd", "efgh"),
            format!("{pinned}\n[[package]]\nname = 'injected'\nversion = '3.0.0'\n"),
        ] {
            ensure!(verify_pruned_lock(&original, &changed).is_err());
        }
        ensure!(verify_pruned_lock(&original, "package = []").is_err());
        let mut manifest: DocumentMut = "[dependencies]\nnormal = '1'\n[dev-dependencies]\ndev = '2'\n[target.'cfg(unix)'.dependencies]\nplatform = '3'\n[target.'cfg(unix)'.dev-dependencies]\nplatform-dev = '4'\n".parse()?;
        strip_dev_dependencies(&mut manifest);
        ensure!(manifest.get("dev-dependencies").is_none());
        ensure!(
            manifest["target"]["cfg(unix)"]
                .get("dev-dependencies")
                .is_none()
        );
        ensure!(manifest["dependencies"]["normal"].as_str() == Some("1"));
        ensure!(manifest["target"]["cfg(unix)"]["dependencies"]["platform"].as_str() == Some("3"));
        Ok(())
    }
}
