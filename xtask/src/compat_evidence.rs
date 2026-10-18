//! Produce hosted-lane validation reports and adopt a run's reports as the ledger.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail, ensure};
use blake2::{Blake2b, digest::consts::U32};
use serde_json::{Value as Json, json};
use sha2::Digest;
use toml_edit::DocumentMut;

use super::{
    EVIDENCE, MAX_REPORT_BYTES, RUN_URL_PREFIX, candidates, canonical, canonical_hash,
    checked_path, image_digests, json_text, members, read, report_run, runtime_identity, sha256,
    spec, toml, validate,
};

const REPORTS: &str = "docs/compat/reports";
const MAX_ADOPTED_FILES: usize = 256;
const USAGE: &str = "usage: cargo xtask compat report --kind e2e|interop --image-digest <sha256:...> [--image-digest ...] [--runtime-spec <n>] --out <file>\n       cargo xtask compat report --kind drift [--runtime-spec <n>] [--endpoint <https url>] --out <directory>\n       cargo xtask compat adopt --dir <downloaded report directory>";

/// The GitHub Actions run that produced a report.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Run {
    url: String,
    head_commit: String,
}

/// Require the real hosted rtao run and the checked-out commit it names.
///
/// Reports record only the environment of the run that executed the tests;
/// there is deliberately no command-line override.
fn hosted_run(root: &Path) -> Result<Run> {
    let variable = |name: &str| std::env::var(name).unwrap_or_default();
    ensure!(
        variable("GITHUB_ACTIONS") == "true",
        "compatibility reports are produced only by hosted GitHub Actions lanes"
    );
    let base = format!(
        "{}/{}/actions/runs/",
        variable("GITHUB_SERVER_URL"),
        variable("GITHUB_REPOSITORY")
    );
    ensure!(
        base == RUN_URL_PREFIX,
        "compatibility reports require the sam0x17/rtao GitHub repository"
    );
    let id = variable("GITHUB_RUN_ID");
    let head_commit = variable("GITHUB_SHA");
    let run = Run {
        url: format!("{base}{id}"),
        head_commit,
    };
    // Reuse the ledger's own provenance checks for the recorded fields.
    report_run(&json!({"run": {"url": run.url, "head_commit": run.head_commit}}))?;
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()?;
        ensure!(output.status.success(), "git {args:?} failed");
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    ensure!(
        git(&["rev-parse", "HEAD"])? == run.head_commit,
        "checked-out commit differs from GITHUB_SHA"
    );
    ensure!(
        git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty(),
        "tracked files changed during the compatibility lane"
    );
    Ok(run)
}

/// Pinned-network runtime identities at one spec, V15 then V16.
fn runtimes_at(inputs: &Json, runtime_spec: u32) -> Result<Vec<Json>> {
    let runtimes: Vec<_> = candidates(inputs)?
        .into_iter()
        .filter(|runtime| spec(runtime).is_ok_and(|value| value == runtime_spec))
        .collect();
    ensure!(
        !runtimes.is_empty(),
        "spec {runtime_spec} has no immutable metadata pin"
    );
    Ok(runtimes)
}

fn header(inputs: &Json, kind: &str, run: &Run) -> Result<Json> {
    Ok(json!({
        "schema": 1,
        "kind": kind,
        "status": "passed",
        "inputs_sha256": sha256(&serde_json::to_vec(&canonical(inputs))?),
        "run": {"url": run.url, "head_commit": run.head_commit},
    }))
}

/// A passed e2e or interop report for runtimes exercised on image digests.
fn test_report(
    inputs: &Json,
    kind: &str,
    runtimes: &[Json],
    digests: &[String],
    run: &Run,
) -> Result<Json> {
    ensure!(
        matches!(kind, "e2e" | "interop"),
        "test reports are e2e or interop"
    );
    ensure!(!runtimes.is_empty(), "a test report needs runtimes");
    let available = candidates(inputs)?;
    for runtime in runtimes {
        ensure!(
            available.contains(runtime),
            "report runtime is absent from immutable metadata pins"
        );
    }
    let pinned = image_digests(inputs)?;
    let digests: BTreeSet<_> = digests.iter().cloned().collect();
    ensure!(!digests.is_empty(), "a test report needs an image digest");
    for digest in &digests {
        ensure!(
            pinned.contains(digest),
            "image digest {digest} is not in the pinned localnet matrix"
        );
    }
    let mut report = header(inputs, kind, run)?;
    report["runtimes"] = json!(runtimes);
    report["image_digests"] = json!(digests);
    Ok(canonical(&report))
}

/// A passed self-comparison drift report for one runtime identity.
fn drift_report(inputs: &Json, runtime: &Json, observed: &Json, run: &Run) -> Result<Json> {
    ensure!(
        candidates(inputs)?.contains(runtime),
        "drift runtime is absent from immutable metadata pins"
    );
    let mut report = header(inputs, "drift", run)?;
    report["from"] = runtime.clone();
    report["to"] = runtime.clone();
    report["classification"] = json!("surface-unchanged");
    report["observed"] = observed.clone();
    Ok(canonical(&report))
}

fn metadata_hash(bytes: &[u8]) -> String {
    format!("0x{:x}", Blake2b::<U32>::digest(bytes))
}

/// Compare the live finalized runtime with the pinned input at one spec.
///
/// Returns the pinned V15/V16 identities, which are exact only when the live
/// genesis, spec, `:code` and both metadata blobs equal the immutable inputs.
fn live_identities(
    root: &Path,
    inputs: &Json,
    pins: &DocumentMut,
    runtime_spec: u32,
    endpoint: Option<&str>,
) -> Result<(Vec<Json>, Json)> {
    let network = crate::metadata_pin::pinned_network(pins)?;
    let endpoint = endpoint.unwrap_or_else(|| crate::drift::public_endpoint(network));
    ensure!(
        endpoint.starts_with("https://"),
        "drift reports read a public network over HTTPS"
    );
    let pin = members(inputs, "metadata_pins")?
        .iter()
        .find(|pin| spec(pin).is_ok_and(|value| value == runtime_spec))
        .with_context(|| format!("spec {runtime_spec} has no immutable metadata pin"))?;
    let live = crate::drift::finalized(endpoint)?;
    ensure!(
        pins["networks"][network]["genesis"].as_str() == Some(live.genesis.as_str())
            && json_text(pin, "genesis")? == live.genesis,
        "{network} genesis differs from the pinned network"
    );
    ensure!(
        live.spec == runtime_spec,
        "{network} finalized spec {} differs from pinned spec {runtime_spec}",
        live.spec
    );
    ensure!(
        json_text(pin, "code_hash")? == live.code_hash,
        "{network} finalized :code differs from the pinned spec {runtime_spec} runtime"
    );
    let pinned15 = read(root, json_text(&pin["v15"], "path")?, 16 * 1024 * 1024)?;
    let pinned16 = read(root, json_text(&pin["v16"], "path")?, 16 * 1024 * 1024)?;
    let class = crate::drift::classify(&pinned15, &pinned16, &live.v15, &live.v16)?;
    ensure!(
        class == "surface-unchanged",
        "{network} finalized runtime drifted from the pinned input: {class}"
    );
    for (version, bytes) in [("v15", &live.v15), ("v16", &live.v16)] {
        ensure!(
            json_text(&pin[version]["metadata_hash"], "value")? == metadata_hash(bytes),
            "{network} finalized {version} metadata differs from the pinned blob"
        );
    }
    let observed = json!({
        "endpoint": endpoint,
        "finalized_head": live.head,
        "code_hash": live.code_hash,
    });
    Ok((
        vec![runtime_identity(pin, "v15"), runtime_identity(pin, "v16")],
        observed,
    ))
}

fn report_bytes(report: &Json) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&canonical(report))?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= MAX_REPORT_BYTES,
        "compatibility report exceeds its size limit"
    );
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn report(root: &Path, args: Vec<String>) -> Result<()> {
    let mut kind = None;
    let mut out = None;
    let mut digests = Vec::new();
    let mut runtime_spec = None;
    let mut endpoint = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .with_context(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--kind" => kind = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--image-digest" => digests.push(value()?),
            "--runtime-spec" => runtime_spec = Some(value()?.parse::<u32>()?),
            "--endpoint" => endpoint = Some(value()?),
            _ => bail!("unknown compat report option {arg:?}\n{USAGE}"),
        }
    }
    let kind = kind.context(USAGE)?;
    let out = out.context(USAGE)?;
    let run = hosted_run(root)?;
    let inputs = super::inputs(root)?;
    let pins = toml(root, "upstream.toml")?;
    let runtime_spec = match runtime_spec {
        Some(value) => value,
        None => u32::try_from(
            crate::metadata_pin::pinned_metadata(&pins)?["spec"]
                .as_integer()
                .context("pinned metadata spec missing")?,
        )?,
    };
    match kind.as_str() {
        "e2e" | "interop" => {
            ensure!(endpoint.is_none(), "--endpoint applies only to drift");
            let runtimes = runtimes_at(&inputs, runtime_spec)?;
            let report = test_report(&inputs, &kind, &runtimes, &digests, &run)?;
            write_new(&out, &report_bytes(&report)?)?;
            println!("Wrote {kind} report {}", out.display());
        }
        "drift" => {
            ensure!(
                digests.is_empty(),
                "drift reports compare runtimes, not images"
            );
            let (runtimes, observed) =
                live_identities(root, &inputs, &pins, runtime_spec, endpoint.as_deref())?;
            fs::create_dir_all(&out)?;
            for (runtime, version) in runtimes.iter().zip(["v15", "v16"]) {
                let path = out.join(format!("drift-{runtime_spec}-{version}.json"));
                write_new(
                    &path,
                    &report_bytes(&drift_report(&inputs, runtime, &observed, &run)?)?,
                )?;
                println!("Wrote surface-unchanged drift report {}", path.display());
            }
        }
        _ => bail!("unknown compatibility report kind {kind:?}\n{USAGE}"),
    }
    Ok(())
}

/// Every regular file below a downloaded artifact directory.
fn downloaded(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![(directory.to_owned(), 0)];
    while let Some((path, depth)) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "downloaded reports must not contain symlinks"
        );
        if metadata.is_dir() {
            ensure!(depth <= 4, "downloaded report directory is too deep");
            for entry in fs::read_dir(&path)? {
                pending.push((entry?.path(), depth + 1));
            }
        } else {
            ensure!(
                metadata.is_file(),
                "downloaded report is not a regular file"
            );
            ensure!(
                path.extension()
                    .is_some_and(|extension| extension == "json"),
                "unexpected non-report file {}",
                path.display()
            );
            files.push(path);
            ensure!(
                files.len() <= MAX_ADOPTED_FILES,
                "too many downloaded reports"
            );
        }
    }
    files.sort();
    Ok(files)
}

/// The deterministic committed path for one report.
fn report_path(report: &Json, run_id: &str, bytes: &[u8]) -> Result<String> {
    let kind = json_text(report, "kind")?;
    let suffix = match kind {
        "e2e" | "interop" => {
            let digests = members(report, "image_digests")?;
            match digests.as_slice() {
                [digest] => {
                    let digest = digest.as_str().context("image digest must be a string")?;
                    ensure!(
                        canonical_hash(digest, "sha256:", 64),
                        "invalid report image digest"
                    );
                    digest["sha256:".len().."sha256:".len() + 12].to_owned()
                }
                _ => format!("multi-{}", &sha256(bytes)[..12]),
            }
        }
        "drift" => {
            let from = spec(&report["from"])?;
            let to = spec(&report["to"])?;
            let hash = json_text(&report["to"]["metadata_hash"], "value")?;
            ensure!(
                canonical_hash(hash, "0x", 64),
                "invalid drift metadata hash"
            );
            format!("{from}-{to}-{}", &hash[2..14])
        }
        kind => bail!("unknown compatibility report kind {kind:?}"),
    };
    Ok(format!("{REPORTS}/{run_id}-{kind}-{suffix}.json"))
}

/// Ledger runtimes: every identity exercised by a test report, ordered by spec.
fn ledger_runtimes(reports: &BTreeMap<String, (Json, Vec<u8>)>) -> Result<Vec<Json>> {
    let mut runtimes = BTreeMap::new();
    for (report, _) in reports.values() {
        if matches!(json_text(report, "kind")?, "e2e" | "interop") {
            for runtime in members(report, "runtimes")? {
                runtimes.insert(
                    (spec(runtime)?, serde_json::to_string(&canonical(runtime))?),
                    canonical(runtime),
                );
            }
        }
    }
    ensure!(
        !runtimes.is_empty(),
        "downloaded reports exercised no runtimes"
    );
    Ok(runtimes.into_values().collect())
}

static WRITES: AtomicU64 = AtomicU64::new(0);

/// Replace a file through a sibling temporary file on the same filesystem.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("output path has no file name")?;
    let temporary = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    write_new(&temporary, bytes)?;
    fs::rename(&temporary, path).with_context(|| format!("write {}", path.display()))
}

/// Validate a run's downloaded reports and record them as the evidence ledger.
///
/// Nothing is written unless the reports together satisfy [`validate`].
fn adopt_into(root: &Path, inputs: &Json, directory: &Path) -> Result<Json> {
    let input_hash = sha256(&serde_json::to_vec(&canonical(inputs))?);
    let mut reports = BTreeMap::new();
    let mut runs = BTreeSet::new();
    for file in downloaded(directory)? {
        let bytes = fs::read(&file)?;
        ensure!(
            bytes.len() <= MAX_REPORT_BYTES,
            "{} exceeds the report size limit",
            file.display()
        );
        let report: Json =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", file.display()))?;
        ensure!(
            report["schema"] == 1 && report["status"] == "passed",
            "{} is not a passed schema 1 report",
            file.display()
        );
        ensure!(
            json_text(&report, "inputs_sha256")? == input_hash,
            "{} was produced for different compatibility inputs",
            file.display()
        );
        let run = report_run(&report)?;
        let path = report_path(&report, &run.0, &bytes)?;
        runs.insert(run);
        ensure!(
            reports.insert(path.clone(), (report, bytes)).is_none(),
            "two downloaded reports map to {path}"
        );
    }
    ensure!(
        runs.len() == 1,
        "adopt the reports of exactly one GitHub Actions run"
    );
    let runtimes = ledger_runtimes(&reports)?;
    let specs: Vec<_> = runtimes.iter().map(spec).collect::<Result<_>>()?;
    let ledger = canonical(&json!({
        "schema": 1,
        "inputs_sha256": input_hash,
        "validated_spec_range": {
            "min": specs.iter().min(),
            "max": specs.iter().max(),
        },
        "runtimes": runtimes,
        "reports": reports
            .iter()
            .map(|(path, (_, bytes))| json!({"path": path, "sha256": sha256(bytes)}))
            .collect::<Vec<_>>(),
    }));
    validate(inputs, &ledger, |path| {
        reports
            .get(path)
            .map(|(_, bytes)| bytes.clone())
            .with_context(|| format!("missing adopted report {path}"))
    })?;
    fs::create_dir_all(root.join(REPORTS))?;
    // Reports of the previous ledger are replaced by this run's reports.
    let previous = if checked_path(root, EVIDENCE, true)?.exists() {
        let ledger: Json = serde_json::from_slice(&read(root, EVIDENCE, MAX_REPORT_BYTES)?)?;
        members(&ledger, "reports")?
            .iter()
            .map(|reference| Ok(json_text(reference, "path")?.to_owned()))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    for (path, (_, bytes)) in &reports {
        replace(&checked_path(root, path, true)?, bytes)?;
    }
    for path in previous {
        if path.starts_with(&format!("{REPORTS}/")) && !reports.contains_key(&path) {
            let stale = checked_path(root, &path, true)?;
            if stale.exists() {
                fs::remove_file(stale)?;
            }
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&ledger)?;
    bytes.push(b'\n');
    replace(&checked_path(root, EVIDENCE, true)?, &bytes)?;
    Ok(ledger)
}

fn adopt(root: &Path, args: Vec<String>) -> Result<()> {
    let [option, directory] = args.as_slice() else {
        bail!(USAGE);
    };
    ensure!(option == "--dir", "{USAGE}");
    let ledger = adopt_into(root, &super::inputs(root)?, Path::new(directory))?;
    println!(
        "Adopted {} reports for specs {}-{} into {EVIDENCE}; run cargo xtask compat",
        members(&ledger, "reports")?.len(),
        ledger["validated_spec_range"]["min"],
        ledger["validated_spec_range"]["max"],
    );
    Ok(())
}

/// Dispatch `compat report` and `compat adopt`.
pub(super) fn run(root: &Path, command: &str, args: Vec<String>) -> Result<()> {
    match command {
        "report" => report(root, args),
        "adopt" => adopt(root, args),
        _ => bail!(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_hash(spec: u32, version: &str) -> String {
        format!("0x{}", sha256(format!("{spec}.{version}").as_bytes()))
    }

    fn fixture_inputs() -> Json {
        let genesis = format!("0x{}", "1".repeat(64));
        let pin = |spec: u32| {
            json!({
                "network": "testnet", "spec_version": spec, "genesis": genesis,
                "v15": {"metadata_hash": {"kind": super::super::HASH_KIND, "value": fixture_hash(spec, "v15")}},
                "v16": {"metadata_hash": {"kind": super::super::HASH_KIND, "value": fixture_hash(spec, "v16")}},
            })
        };
        let digest = |byte: char| json!({"tag": "t", "digest": format!("sha256:{}", byte.to_string().repeat(64))});
        json!({
            "rtao_version": "0.1.0",
            "metadata_pins": [pin(478), pin(481)],
            "localnet": {
                "image": "ghcr.io/raofoundation/subtensor-localnet",
                "pinned": digest('a'), "testnet": digest('a'),
                "main": digest('b'), "devnet": digest('c'), "mainnet": digest('d'),
            },
        })
    }

    fn fixture_run() -> Run {
        Run {
            url: format!("{RUN_URL_PREFIX}4242"),
            head_commit: "e".repeat(40),
        }
    }

    /// Reports the hosted lanes would write for spec 481 on every distinct digest.
    fn lane_reports(inputs: &Json, run: &Run) -> Result<BTreeMap<String, Json>> {
        let runtimes = runtimes_at(inputs, 481)?;
        let mut reports = BTreeMap::new();
        for digest in image_digests(inputs)? {
            for kind in ["e2e", "interop"] {
                reports.insert(
                    format!("{kind}-{}.json", &digest[7..19]),
                    test_report(inputs, kind, &runtimes, std::slice::from_ref(&digest), run)?,
                );
            }
        }
        for (runtime, version) in runtimes.iter().zip(["v15", "v16"]) {
            reports.insert(
                format!("drift-481-{version}.json"),
                drift_report(inputs, runtime, &json!({"code_hash": "0x00"}), run)?,
            );
        }
        Ok(reports)
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Result<Self> {
            let base = std::env::var_os("RTAO_TEST_TMPDIR")
                .or_else(|| std::env::var_os("RTAO_TEMP_ROOT"))
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!(
                "compat-evidence-{name}-{}-{}",
                std::process::id(),
                WRITES.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("root"))?;
            fs::create_dir_all(path.join("download/compat-validation"))?;
            Ok(Self(path))
        }
        fn root(&self) -> PathBuf {
            self.0.join("root")
        }
        fn download(&self) -> PathBuf {
            self.0.join("download")
        }
        fn store(&self, reports: &BTreeMap<String, Json>) -> Result<()> {
            for (name, report) in reports {
                fs::write(
                    self.download().join("compat-validation").join(name),
                    report_bytes(report)?,
                )?;
            }
            Ok(())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn generated_reports_validate_once_every_digest_and_kind_is_covered() -> Result<()> {
        let inputs = fixture_inputs();
        let scratch = Scratch::new("complete")?;
        scratch.store(&lane_reports(&inputs, &fixture_run())?)?;
        let ledger = adopt_into(&scratch.root(), &inputs, &scratch.download())?;
        ensure!(ledger["validated_spec_range"] == json!({"min": 481, "max": 481}));
        let expected: BTreeSet<_> = runtimes_at(&inputs, 481)?
            .iter()
            .map(|runtime| canonical(runtime).to_string())
            .collect();
        ensure!(
            members(&ledger, "runtimes")?
                .iter()
                .map(|runtime| canonical(runtime).to_string())
                .collect::<BTreeSet<_>>()
                == expected
        );
        // Four distinct digests, two test kinds, and two drift identities.
        ensure!(members(&ledger, "reports")?.len() == 10);
        let paths: Vec<_> = members(&ledger, "reports")?
            .iter()
            .map(|reference| json_text(reference, "path").map(str::to_owned))
            .collect::<Result<_>>()?;
        ensure!(paths.contains(&format!("{REPORTS}/4242-e2e-aaaaaaaaaaaa.json")));
        ensure!(paths.contains(&format!(
            "{REPORTS}/4242-drift-481-481-{}.json",
            &fixture_hash(481, "v16")[2..14]
        )));
        // The committed ledger and reports validate exactly as `cargo xtask compat` reads them.
        let root = scratch.root();
        let stored: Json = serde_json::from_slice(&fs::read(root.join(EVIDENCE))?)?;
        ensure!(stored == ledger);
        validate(&inputs, &stored, |path| read(&root, path, MAX_REPORT_BYTES))?;
        // Adopting again from a later run replaces the earlier run's reports.
        let later = Run {
            url: format!("{RUN_URL_PREFIX}4243"),
            head_commit: "f".repeat(40),
        };
        let again = Scratch::new("later")?;
        again.store(&lane_reports(&inputs, &later)?)?;
        adopt_into(&root, &inputs, &again.download())?;
        let names: Vec<_> = fs::read_dir(root.join(REPORTS))?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_>>()?;
        ensure!(names.len() == 10 && names.iter().all(|name| name.starts_with("4243-")));
        Ok(())
    }

    #[test]
    fn adoption_refuses_missing_digest_coverage_and_writes_nothing() -> Result<()> {
        let inputs = fixture_inputs();
        for missing in [
            "e2e-dddddddddddd.json",
            "interop-bbbbbbbbbbbb.json",
            "drift-481-v16.json",
        ] {
            let scratch = Scratch::new("missing")?;
            let mut reports = lane_reports(&inputs, &fixture_run())?;
            reports.remove(missing).context("fixture report missing")?;
            scratch.store(&reports)?;
            ensure!(
                adopt_into(&scratch.root(), &inputs, &scratch.download()).is_err(),
                "adopted without {missing}"
            );
            ensure!(!scratch.root().join(EVIDENCE).exists());
            ensure!(!scratch.root().join(REPORTS).exists());
        }
        Ok(())
    }

    #[test]
    fn adoption_refuses_stale_inputs_mixed_runs_and_failed_reports() -> Result<()> {
        let inputs = fixture_inputs();
        let mut changed = inputs.clone();
        changed["rtao_version"] = json!("0.2.0");
        let scratch = Scratch::new("stale")?;
        scratch.store(&lane_reports(&changed, &fixture_run())?)?;
        ensure!(adopt_into(&scratch.root(), &inputs, &scratch.download()).is_err());

        let scratch = Scratch::new("mixed")?;
        let mut reports = lane_reports(&inputs, &fixture_run())?;
        let other = Run {
            url: format!("{RUN_URL_PREFIX}9"),
            head_commit: "e".repeat(40),
        };
        reports.insert(
            "drift-481-v15.json".into(),
            drift_report(&inputs, &runtimes_at(&inputs, 481)?[0], &json!({}), &other)?,
        );
        scratch.store(&reports)?;
        ensure!(adopt_into(&scratch.root(), &inputs, &scratch.download()).is_err());

        let scratch = Scratch::new("failed")?;
        let mut reports = lane_reports(&inputs, &fixture_run())?;
        reports
            .get_mut("e2e-aaaaaaaaaaaa.json")
            .context("fixture report missing")?["status"] = json!("failed");
        scratch.store(&reports)?;
        ensure!(adopt_into(&scratch.root(), &inputs, &scratch.download()).is_err());
        Ok(())
    }

    #[test]
    fn reports_refuse_unpinned_digests_and_runtimes() -> Result<()> {
        let inputs = fixture_inputs();
        let runtimes = runtimes_at(&inputs, 481)?;
        ensure!(runtimes.len() == 2);
        ensure!(runtimes_at(&inputs, 480).is_err());
        let unknown = format!("sha256:{}", "f".repeat(64));
        ensure!(test_report(&inputs, "e2e", &runtimes, &[unknown], &fixture_run()).is_err());
        ensure!(test_report(&inputs, "e2e", &runtimes, &[], &fixture_run()).is_err());
        ensure!(
            test_report(
                &inputs,
                "drift",
                &runtimes,
                &[format!("sha256:{}", "a".repeat(64))],
                &fixture_run()
            )
            .is_err()
        );
        let mut foreign = runtimes[0].clone();
        foreign["genesis"] = json!(format!("0x{}", "2".repeat(64)));
        ensure!(drift_report(&inputs, &foreign, &json!({}), &fixture_run()).is_err());
        Ok(())
    }

    #[test]
    fn reports_require_the_hosted_rtao_run() {
        // Test processes never run as the hosted lane, so no report can be written.
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            assert!(hosted_run(Path::new(".")).is_err());
        }
    }
}
