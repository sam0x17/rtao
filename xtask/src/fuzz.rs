//! Run every committed robustness target with instrumented nightly libFuzzer.

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const TARGETS: &[&str] = &[
    "btauth_headers",
    "btauth_server",
    "btauth_verify",
    "event_splitting",
    "json_rpc_frame",
    "keyfile_detection",
    "keyfile_json",
    "metagraph_view",
];

fn seed(directory: &Path, repository: &Path, target: &str) -> Result<()> {
    fs::create_dir_all(directory)?;
    let committed = repository.join("fuzz/seeds").join(target);
    if committed.is_dir() {
        for entry in fs::read_dir(committed)? {
            let entry = entry?;
            ensure!(
                entry.file_type()?.is_file(),
                "fuzz seeds must be ordinary files"
            );
            fs::copy(entry.path(), directory.join(entry.file_name()))?;
        }
    }
    match target {
        "metagraph_view" => {
            for name in [
                "finney-1.scale",
                "finney-3.scale",
                "finney-64.scale",
                "synthetic-2500.scale",
            ] {
                fs::copy(
                    repository.join("vendor/bench").join(name),
                    directory.join(name),
                )?;
            }
            fs::write(directory.join("none"), [0])?;
        }
        "btauth_verify" => {
            // Selector 9 leaves valid signed fixtures unchanged; exercise both
            // algorithms and raw-only vs wrapped-compatible policy immediately.
            for selector in [36, 37, 38, 39, 164, 165, 166, 167] {
                fs::write(directory.join(format!("valid-{selector}")), [selector])?;
            }
            fs::write(
                directory.join("future-boundary"),
                [32, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            )?;
        }
        "btauth_server" => {
            for selector in [16, 17, 18, 19, 20, 21, 22, 23] {
                fs::write(directory.join(format!("server-{selector}")), [selector])?;
            }
            fs::write(
                directory.join("encoded-target"),
                b"\x0e/generate%2Fprompt?text=a%20b",
            )?;
        }
        "btauth_headers" => {
            let values = [b"1".as_slice(), b"5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
                b"1752076800000000000", b"sr25519", b"", b"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"];
            let mut bytes = Vec::new();
            for value in values {
                bytes.extend_from_slice(&u16::try_from(value.len())?.to_le_bytes());
                bytes.extend_from_slice(value);
            }
            fs::write(directory.join("complete-fields"), bytes)?;
        }
        _ => {}
    }
    Ok(())
}

/// Build and execute cargo-fuzz; a stable, uninstrumented smoke is insufficient.
pub fn run(args: Vec<String>) -> Result<()> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("repository root")?;
    let mut toolchain = "nightly".to_owned();
    let mut seconds: u32 = 120;
    let mut selected = Vec::new();
    let mut list = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--toolchain" => {
                toolchain = args
                    .next()
                    .context("--toolchain requires a nightly toolchain")?
            }
            "--seconds-per-target" => {
                seconds = args
                    .next()
                    .context("--seconds-per-target requires a positive integer")?
                    .parse()?
            }
            "--target" => selected.push(args.next().context("--target requires a name")?),
            "--list" => list = true,
            _ => bail!(
                "usage: fuzz [--toolchain nightly] [--seconds-per-target 120] [--target NAME] [--list]"
            ),
        }
    }
    ensure!(
        toolchain == "nightly" || toolchain.starts_with("nightly-"),
        "fuzz requires a nightly compiler"
    );
    ensure!(seconds > 0, "fuzz duration must be positive");
    if selected.is_empty() {
        selected = TARGETS.iter().map(|target| (*target).to_owned()).collect();
    }
    ensure!(
        selected
            .iter()
            .all(|target| TARGETS.contains(&target.as_str())),
        "unknown fuzz target"
    );
    if list {
        for target in selected {
            println!("{target}");
        }
        return Ok(());
    }
    let temporary = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT")
            .context("set RTAO_TEMP_ROOT to the recorded task-scoped temporary root")?,
    );
    ensure!(temporary.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
    let compiler = Command::new("rustc")
        .args([&format!("+{toolchain}"), "-Vv"])
        .output()?;
    ensure!(
        compiler.status.success() && String::from_utf8_lossy(&compiler.stdout).contains("nightly"),
        "selected nightly toolchain unavailable"
    );
    let version = Command::new("cargo")
        .args([&format!("+{toolchain}"), "fuzz", "--version"])
        .output()?;
    ensure!(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).trim() == "cargo-fuzz 0.13.2",
        "install pinned cargo-fuzz 0.13.2 under RTAO_TEMP_ROOT/tools and add tools/bin to PATH"
    );
    // cargo-fuzz 0.13.2 has no --locked flag. Resolve with --locked first,
    // execute offline and reject any lock mutation rather than ignoring it.
    let metadata = Command::new("cargo")
        .current_dir(repository)
        .args([
            &format!("+{toolchain}"),
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            "fuzz/Cargo.toml",
        ])
        .output()?;
    ensure!(
        metadata.status.success(),
        "fuzz lock resolution failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let locked = fs::read(repository.join("fuzz/Cargo.lock"))?;
    fs::create_dir_all(temporary.join("fuzz"))?;
    let mut results = Vec::new();
    for target in selected {
        let corpus = temporary.join("fuzz/corpus").join(&target);
        let artifacts = temporary.join("fuzz/artifacts").join(&target);
        seed(&corpus, repository, &target)?;
        fs::create_dir_all(&artifacts)?;
        println!(
            "fuzz: instrumented {target} for {seconds}s; corpus {}",
            corpus.display()
        );
        let started = std::time::Instant::now();
        let status = Command::new("cargo")
            .current_dir(repository)
            .args([
                &format!("+{toolchain}"),
                "fuzz",
                "run",
                "--fuzz-dir",
                "fuzz",
                "--codegen-units",
                "16",
                "--target-dir",
            ])
            .arg(temporary.join("fuzz/build"))
            .arg(&target)
            .arg(&corpus)
            .args([
                "--",
                &format!("-max_total_time={seconds}"),
                "-timeout=10",
                "-rss_limit_mb=4096",
                "-max_len=1048576",
            ])
            .arg(format!("-artifact_prefix={}/", artifacts.display()))
            .env("CARGO_TARGET_DIR", temporary.join("fuzz/build"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("TMPDIR", temporary.join("tmp"))
            .status()?;
        ensure!(
            fs::read(repository.join("fuzz/Cargo.lock"))? == locked,
            "cargo-fuzz mutated its lock"
        );
        results.push(json!({"target":target,"seconds_requested":seconds,"elapsed_seconds":started.elapsed().as_secs_f64(),"exit_code":status.code(),"passed":status.success()}));
        fs::write(
            temporary.join("fuzz/results.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&json!({
                    "schema":1,"compiler":String::from_utf8_lossy(&compiler.stdout),"cargo_fuzz":"0.13.2",
                    "sanitizer":"address","coverage_instrumented":true,"targets":results,
                }))?
            ),
        )?;
        ensure!(
            status.success(),
            "instrumented fuzz target {target} failed; retain crash artifacts at {}",
            artifacts.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m1_targets_exist_and_use_libfuzzer() -> Result<()> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("repository root")?;
        let manifest = fs::read_to_string(repository.join("fuzz/Cargo.toml"))?;
        let versions: toml_edit::DocumentMut =
            fs::read_to_string(repository.join("ci/tool-versions.toml"))?.parse()?;
        ensure!(
            versions["tools"]["cargo-fuzz"].as_str() == Some("0.13.2"),
            "fuzz driver and tool pin differ"
        );
        for target in TARGETS {
            ensure!(
                manifest.contains(&format!("name = \"{target}\"")),
                "missing manifest target"
            );
            let source = fs::read_to_string(
                repository
                    .join("fuzz/fuzz_targets")
                    .join(format!("{target}.rs")),
            )?;
            ensure!(
                source.contains("fuzz_target!"),
                "target has no libFuzzer entry"
            );
        }
        Ok(())
    }
}
