//! Criterion measurement collection and an explicit, comparable regression gate.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const SUITES: &[(&str, &str)] = &[
    ("rtao-core-ext", "protocol"),
    ("rtao-runtime", "runtime"),
    ("rtao-metagraph", "metagraph"),
    ("rtao-btauth", "btauth"),
    ("rtao-intents", "weights"),
];

fn output(program: &str, args: &[&str], root: &Path) -> Result<String> {
    let result = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    ensure!(
        result.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn machine() -> Result<String> {
    if let Ok(value) = std::env::var("RTAO_BENCH_MACHINE") {
        ensure!(!value.trim().is_empty(), "RTAO_BENCH_MACHINE is empty");
        return Ok(value);
    }
    if cfg!(target_os = "macos") {
        return output(
            "sysctl",
            &["-n", "machdep.cpu.brand_string"],
            Path::new("."),
        );
    }
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .context("set RTAO_BENCH_MACHINE to identify benchmark hardware")?;
    cpu.lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|v| v.split_once(':'))
                .map(|(_, v)| v.trim().to_owned())
        })
        .context("set RTAO_BENCH_MACHINE to identify benchmark hardware")
}

fn source_snapshot(root: &Path) -> Result<String> {
    let result = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    ensure!(result.status.success(), "git source inventory failed");
    let mut files: Vec<_> = result
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(std::str::from_utf8)
        .collect::<std::result::Result<_, _>>()?;
    files.sort();
    files.dedup();
    let mut hash = Sha256::new();
    for file in files {
        let source = file.ends_with(".rs")
            || file.ends_with(".scale")
            || file == "Cargo.lock"
            || file == "upstream.toml"
            || file == "rust-toolchain.toml"
            || file.ends_with("Cargo.toml");
        if source {
            ensure!(root.join(file).is_file(), "source file disappeared: {file}");
            hash.update(file.as_bytes());
            hash.update([0]);
            hash.update(Sha256::digest(fs::read(root.join(file))?));
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn collect(directory: &Path, measurements: &mut BTreeMap<String, Value>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if entry.file_name() == "new" {
                let benchmark: Value =
                    serde_json::from_slice(&fs::read(entry.path().join("benchmark.json"))?)?;
                let estimate: Value =
                    serde_json::from_slice(&fs::read(entry.path().join("estimates.json"))?)?;
                let samples: Value =
                    serde_json::from_slice(&fs::read(entry.path().join("sample.json"))?)?;
                let id = benchmark["full_id"]
                    .as_str()
                    .context("Criterion benchmark ID missing")?;
                let mean = estimate["mean"]["point_estimate"]
                    .as_f64()
                    .context("Criterion mean missing")?;
                ensure!(
                    mean.is_finite() && mean > 0.0,
                    "invalid Criterion mean: {id}"
                );
                ensure!(measurements.insert(id.into(), json!({
                    "mean_ns": mean, "mean_confidence_interval": estimate["mean"]["confidence_interval"],
                    "median_ns": estimate["median"]["point_estimate"],
                    "samples": samples["times"].as_array().context("Criterion sample times missing")?.len(),
                })).is_none(), "duplicate benchmark {id}");
            } else {
                collect(&entry.path(), measurements)?;
            }
        }
    }
    Ok(())
}

fn expected() -> Vec<String> {
    let mut ids = vec![
        "runtime/v15_parse".into(),
        "storage/keys_10000".into(),
        "storage/map_page_decode_256".into(),
        "extrinsic/sign_and_assemble".into(),
        "weights/conform_256".into(),
        "weights/conform_2500".into(),
    ];
    for group in ["metagraph_decode", "metagraph_projection"] {
        for path in ["view", "typed", "dynamic"] {
            for fixture in ["finney-1", "finney-3", "finney-64", "synthetic-2500"] {
                ids.push(format!("{group}/{path}/{fixture}"));
            }
        }
    }
    for operation in ["sign", "authenticate", "verify_and_commit"] {
        for crypto in ["sr25519", "ed25519"] {
            ids.push(format!("btauth/{operation}_{crypto}"));
        }
    }
    ids.sort();
    ids
}

fn compare(baseline: &Value, current: &Value, percent: f64) -> Result<()> {
    ensure!(
        percent.is_finite() && percent >= 0.0,
        "regression threshold must be finite and nonnegative"
    );
    ensure!(
        baseline["schema"] == 1 && current["schema"] == 1,
        "unknown benchmark record schema"
    );
    // Absolute results across platforms or toolchains are not a regression
    // test. CI measures the reference source and current source on one host.
    ensure!(
        baseline["context"] == current["context"],
        "benchmark contexts differ; measure both source revisions on the same machine with the same toolchain, fixtures and sample settings"
    );
    let old = baseline["benchmarks"]
        .as_object()
        .context("baseline benchmarks missing")?;
    let new = current["benchmarks"]
        .as_object()
        .context("current benchmarks missing")?;
    ensure!(
        !old.is_empty() && old.keys().eq(new.keys()),
        "benchmark IDs changed or missing; a new baseline must be measured explicitly"
    );
    let mut failures = Vec::new();
    for (id, old) in old {
        let before = old["mean_ns"].as_f64().context("baseline mean missing")?;
        let after = new[id]["mean_ns"]
            .as_f64()
            .context("current mean missing")?;
        ensure!(
            before.is_finite() && before > 0.0 && after.is_finite() && after > 0.0,
            "invalid means for {id}"
        );
        let change = (after / before - 1.0) * 100.0;
        println!("bench: {id}: {before:.2} -> {after:.2} ns ({change:+.2}%)");
        if after > before * (1.0 + percent / 100.0) {
            failures.push(format!("{id}: {change:+.2}%"));
        }
    }
    ensure!(
        failures.is_empty(),
        "benchmark regressions exceed {percent}%: {}",
        failures.join(", ")
    );
    Ok(())
}

/// Execute every M1 benchmark and retain bounded, provenance-bearing results.
pub fn run(args: Vec<String>) -> Result<()> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("repository root")?;
    let mut workspace = repository.to_owned();
    let mut baseline = None;
    let mut save = None;
    let mut compare_baseline = false;
    let mut percent: f64 = 30.0;
    let mut sample_size: usize = 30;
    let mut measurement: f64 = 1.0;
    let mut warmup: f64 = 1.0;
    let mut list = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--workspace" => {
                workspace = PathBuf::from(args.next().context("--workspace requires a directory")?)
            }
            "--baseline" => {
                baseline = Some(PathBuf::from(
                    args.next().context("--baseline requires a file")?,
                ))
            }
            "--save-baseline" => {
                save = Some(PathBuf::from(
                    args.next().context("--save-baseline requires a file")?,
                ))
            }
            "--compare-baseline" => compare_baseline = true,
            "--fail-over-percent" => {
                percent = args
                    .next()
                    .context("--fail-over-percent requires a number")?
                    .parse()?
            }
            "--sample-size" => {
                sample_size = args
                    .next()
                    .context("--sample-size requires a number")?
                    .parse()?
            }
            "--measurement-seconds" => {
                measurement = args
                    .next()
                    .context("--measurement-seconds requires a number")?
                    .parse()?
            }
            "--warmup-seconds" => {
                warmup = args
                    .next()
                    .context("--warmup-seconds requires a number")?
                    .parse()?
            }
            "--list" => list = true,
            _ => bail!(
                "usage: bench [--workspace PATH] [--compare-baseline] [--baseline JSON] [--save-baseline JSON] [--fail-over-percent 30] [--sample-size 30] [--measurement-seconds 1] [--warmup-seconds 1] [--list]"
            ),
        }
    }
    ensure!(
        sample_size >= 10
            && measurement.is_finite()
            && measurement > 0.0
            && warmup.is_finite()
            && warmup > 0.0
            && percent.is_finite()
            && percent >= 0.0,
        "invalid Criterion sample settings"
    );
    if list {
        for id in expected() {
            println!("{id}");
        }
        return Ok(());
    }
    let temporary = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT")
            .context("set RTAO_TEMP_ROOT to the recorded task-scoped temporary root")?,
    );
    ensure!(temporary.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
    let workspace = workspace.canonicalize()?;
    let source_before = source_snapshot(&workspace)?;
    let compiler = output("rustc", &["-Vv"], repository)?;
    let active_toolchain = output("rustup", &["show", "active-toolchain"], repository)?;
    let active_toolchain = active_toolchain
        .split_whitespace()
        .next()
        .context("active toolchain missing")?;
    let host = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("rustc host missing")?;
    let default_baseline = repository
        .join("vendor/bench/baselines")
        .join(format!("m2-{host}.json"));
    let baseline = baseline.unwrap_or(default_baseline);
    let directory = {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = temporary.join("bench").join(format!("criterion-{nonce}"));
        fs::create_dir_all(&directory)?;
        let mut cargo = Command::new("cargo");
        cargo.current_dir(&workspace).arg("bench").arg("--locked");
        cargo.args(["--features", "rtao-intents/weights"]);
        for (package, _) in SUITES {
            cargo.args(["-p", package]);
        }
        for (_, name) in SUITES {
            cargo.args(["--bench", name]);
        }
        cargo
            .args([
                "--",
                "--sample-size",
                &sample_size.to_string(),
                "--measurement-time",
                &measurement.to_string(),
                "--warm-up-time",
                &warmup.to_string(),
                "--noplot",
            ])
            .env("CRITERION_HOME", &directory)
            .env("RUSTUP_TOOLCHAIN", active_toolchain)
            .env("TMPDIR", temporary.join("tmp"));
        ensure!(
            cargo.status()?.success(),
            "Criterion benchmark execution failed; results at {}",
            directory.display()
        );
        directory
    };
    let mut benchmarks = BTreeMap::new();
    collect(&directory, &mut benchmarks)?;
    ensure!(
        benchmarks.keys().cloned().collect::<Vec<_>>() == expected(),
        "Criterion benchmark suite incomplete or stale"
    );
    ensure!(
        benchmarks
            .values()
            .all(|entry| entry["samples"] == sample_size),
        "Criterion sample count differs from requested context"
    );
    let source_after = source_snapshot(&workspace)?;
    ensure!(
        source_before == source_after,
        "source changed during measurement; rerun on a stable source tree"
    );
    let mut fixtures = BTreeMap::new();
    for path in [
        "vendor/metadata/finney/478.v15.scale",
        "vendor/bench/finney-1.scale",
        "vendor/bench/finney-3.scale",
        "vendor/bench/finney-64.scale",
        "vendor/bench/synthetic-2500.scale",
    ] {
        fixtures.insert(
            path,
            format!("{:x}", Sha256::digest(fs::read(workspace.join(path))?)),
        );
    }
    let record = json!({
        "schema": 1,
        "context": { "compiler": compiler, "os": std::env::consts::OS, "machine": machine()?,
            "sample_size": sample_size, "measurement_seconds": measurement, "warmup_seconds": warmup,
            "criterion_version": "0.7.0", "fixtures_sha256": fixtures,
            "rustflags": std::env::var("RUSTFLAGS").unwrap_or_default(), "profile": "release",
            "cargo_features": ["rtao-intents/weights"], "suites": SUITES },
        "source": { "commit": output("git", &["rev-parse", "HEAD"], &workspace)?,
            "dirty": !output("git", &["status", "--porcelain", "--untracked-files=normal"], &workspace)?.is_empty(),
            "snapshot_sha256": source_after,
            "core_pin_sha256": format!("{:x}", Sha256::digest(fs::read(workspace.join("upstream.toml"))?)),
            "lock_sha256": format!("{:x}", Sha256::digest(fs::read(workspace.join("Cargo.lock"))?)) },
        "measured_unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "benchmarks": benchmarks,
    });
    let result_path = temporary.join("bench/latest.json");
    fs::create_dir_all(result_path.parent().context("result parent")?)?;
    fs::write(
        &result_path,
        format!("{}\n", serde_json::to_string_pretty(&record)?),
    )?;
    if let Some(save) = save {
        if let Some(parent) = save.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            save,
            format!("{}\n", serde_json::to_string_pretty(&record)?),
        )?;
    }
    println!(
        "bench: {} measurements retained at {}",
        expected().len(),
        result_path.display()
    );
    if compare_baseline {
        compare(
            &serde_json::from_slice(&fs::read(&baseline).with_context(|| {
                format!(
                    "baseline {} missing; measure a reference source on this host first",
                    baseline.display()
                )
            })?)?,
            &record,
            percent,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(ns: f64) -> Value {
        json!({"schema":1,"context":{"machine":"same-host"},"benchmarks":{"a":{"mean_ns":ns}}})
    }
    #[test]
    fn threshold_is_strict_and_regressions_fail() {
        assert!(compare(&record(100.0), &record(130.0), 30.0).is_ok());
        assert!(compare(&record(100.0), &record(130.001), 30.0).is_err());
        assert!(compare(&record(100.0), &record(90.0), 30.0).is_ok());
    }
    #[test]
    fn mismatched_context_and_missing_benchmarks_fail() {
        let mut changed = record(100.0);
        changed["context"]["machine"] = json!("other-host");
        assert!(compare(&record(100.0), &changed, 30.0).is_err());
        let mut changed = record(100.0);
        changed["benchmarks"] = json!({});
        assert!(compare(&record(100.0), &changed, 30.0).is_err());
        assert!(compare(&record(0.0), &record(100.0), 30.0).is_err());
    }
    #[test]
    fn implemented_suite_has_36_distinct_measurements() {
        let ids = expected();
        assert_eq!(ids.len(), 36);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }
    #[test]
    fn nonfinite_cli_settings_fail_before_criterion_can_panic() {
        for argument in [
            "--measurement-seconds",
            "--warmup-seconds",
            "--fail-over-percent",
        ] {
            assert!(run(vec![argument.into(), "inf".into(), "--list".into()]).is_err());
        }
    }
}
