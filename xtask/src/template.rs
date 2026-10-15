//! Generate and exercise the subnet workspace against this exact checkout.

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};
use toml_edit::DocumentMut;

const GENERATOR_VERSION: &str = "0.23.12";
const SDK_SOURCE: &str = "https://github.com/sam0x17/rtao";

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let native_only = match args.as_slice() {
        [command] if command == "check" => false,
        [command, option] if command == "check" && option == "--native-only" => true,
        _ => bail!("usage: cargo xtask template check [--native-only]"),
    };
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?
        .canonicalize()?;
    check_source(&source)?;
    let task_root = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT")
            .context("RTAO_TEMP_ROOT must name the recorded task root")?,
    );
    ensure!(
        task_root.is_absolute()
            && !task_root
                .components()
                .any(|c| matches!(c, Component::ParentDir)),
        "RTAO_TEMP_ROOT must be absolute without parent traversal"
    );
    fs::create_dir_all(&task_root)?;
    let task_root = task_root.canonicalize()?;
    fs::create_dir_all(task_root.join("tmp"))?;
    let run_root = task_root.join(format!("template-check-{}", std::process::id()));
    fs::create_dir(&run_root)
        .context("template check directory already exists; audit it before retrying")?;
    fs::write(
        run_root.join(".template-check-owned"),
        b"rtao generated localnet development fixture\n",
    )?;
    let evidence = task_root.join(format!("template-evidence-{}", std::process::id()));
    fs::create_dir(&evidence)?;
    let result = check_project(&source, &run_root, &evidence, native_only);
    // All child commands have exited and the generated check's EXIT trap has
    // removed its compose project. Do not silently delete any live dependency.
    let cleanup = cleanup(&run_root);
    let report = serde_json::json!({
        "schema": 1, "source": source, "generator": GENERATOR_VERSION,
        "native_only": native_only, "passed": result.is_ok(),
        "error": result.as_ref().err().map(|error| format!("{error:#}")),
        "milestone_acceptance": !native_only && result.is_ok() && cleanup.is_ok(),
        "cleanup_passed": cleanup.is_ok(),
        "cleanup_error": cleanup.as_ref().err().map(|error| format!("{error:#}")),
        "retained_project": cleanup.as_ref().err().map(|_| &run_root)
    });
    fs::write(
        evidence.join("result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if let Err(error) = &cleanup {
        eprintln!("retain generated project {}: {error:#}", run_root.display());
    }
    result?;
    cleanup?;
    if native_only {
        println!(
            "template native checks passed; Docker/localnet acceptance remains pending ({})",
            evidence.display()
        );
    } else {
        println!(
            "generated template passed its native, image and real compose-localnet checks ({})",
            evidence.display()
        );
    }
    Ok(())
}

fn check_project(source: &Path, run_root: &Path, evidence: &Path, native_only: bool) -> Result<()> {
    let generator =
        std::env::var_os("RTAO_CARGO_GENERATE").unwrap_or_else(|| "cargo-generate".into());
    let mut version = Command::new(&generator);
    version.arg("--version");
    let output = version
        .output()
        .context("install cargo-generate 0.23.12 or set RTAO_CARGO_GENERATE")?;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim()
                == format!("cargo generate {GENERATOR_VERSION}"),
        "template gate requires cargo-generate {GENERATOR_VERSION}"
    );
    let project = run_root.join("project");
    fs::create_dir(&project)?;
    let mut generate = Command::new(generator);
    generate
        .current_dir(&project)
        .args(["generate", "--path"])
        .arg(source.join("templates/subnet"))
        .args([
            "--name",
            "rtao-template-check",
            "--init",
            "--silent",
            "--no-workspace",
        ]);
    step(generate, evidence, "generate")?;
    let mut patch = Command::new("bash");
    patch
        .current_dir(&project)
        .arg("ci/patch-source.sh")
        .arg(source);
    step(patch, evidence, "source-patch")?;
    let mut lock = Command::new("cargo");
    lock.current_dir(&project).arg("generate-lockfile");
    step(lock, evidence, "lock")?;
    let mut fetch = Command::new("cargo");
    fetch.current_dir(&project).args(["fetch", "--locked"]);
    step(fetch, evidence, "fetch")?;
    fs::copy(project.join("Cargo.lock"), evidence.join("Cargo.lock"))?;
    let fixture = run_root.join("fixture");
    fs::create_dir(&fixture)?;
    let mut check = Command::new("bash");
    check
        .current_dir(&project)
        .arg("ci/check.sh")
        .env("RTAO_TEMP_ROOT", &fixture);
    let mut build_doctor = Command::new("cargo");
    build_doctor
        .current_dir(source)
        .args(["build", "--locked", "-p", "rtao-cli"]);
    step(build_doctor, evidence, "build-doctor")?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || source.join("target"),
        |path| {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                source.join(path)
            }
        },
    );
    check.env("RTAO_DOCTOR", target.join("debug/rtao"));
    if native_only {
        check.arg("native");
    }
    let result = step(check, evidence, "project-ci");
    for entry in fs::read_dir(&fixture)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path.extension().is_some_and(|extension| {
                extension == "json" || extension == "log" || extension == "txt"
            })
        {
            fs::copy(&path, evidence.join(entry.file_name()))?;
        }
    }
    result
}

fn step(mut command: Command, evidence: &Path, name: &str) -> Result<()> {
    command
        // Cargo/rustup can export the parent workspace's active toolchain.
        // Each command must honor the toolchain file of its own working dir.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env("PYO3_NO_PYTHON", "1")
        .env(
            "TMPDIR",
            evidence
                .parent()
                .context("evidence has no task root")?
                .join("tmp"),
        )
        .stdin(Stdio::null());
    println!("template: {name}");
    let log = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(evidence.join(format!("{name}.log")))?;
    let status = command
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?))
        .status()
        .with_context(|| format!("start template {name}"))?;
    log.sync_all()?;
    ensure!(
        status.success(),
        "template {name} failed; see {}",
        evidence.join(format!("{name}.log")).display()
    );
    Ok(())
}

fn check_source(root: &Path) -> Result<()> {
    let template = root.join("templates/subnet");
    check_container_licenses(root)?;
    let manifest: DocumentMut = fs::read_to_string(template.join("Cargo.toml"))?.parse()?;
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let bls = &manifest["patch"]["crates-io"]["w3f-bls"];
    ensure!(
        bls["git"].as_str() == pins["monorepo"]["url"].as_str(),
        "template BLS repository differs from the core pin"
    );
    ensure!(
        bls["tag"].as_str() == pins["core"]["tag"].as_str(),
        "template BLS tag differs from the core pin"
    );
    ensure!(
        manifest["patch"]
            .as_table()
            .is_some_and(|table| table.len() == 1),
        "template must carry only the root crates.io patch table"
    );
    ensure!(
        manifest["patch"]["crates-io"]
            .as_table()
            .is_some_and(|table| table.len() == 1),
        "template must carry only the BLS patch, never a core patch"
    );
    for role in ["validator", "miner"] {
        let member: DocumentMut =
            fs::read_to_string(template.join(role).join("Cargo.toml"))?.parse()?;
        ensure!(
            member.get("patch").is_none(),
            "template member {role} must not carry a Cargo patch"
        );
        ensure!(
            manifest["workspace"]["dependencies"]["rtao"]["default-features"].as_bool()
                == Some(false),
            "template neuron must disable default SDK features"
        );
        ensure!(
            member["dependencies"]["rtao"]["features"]
                .as_array()
                .is_some_and(|features| features.len() == 1
                    && features.get(0).and_then(toml_edit::Value::as_str) == Some(role)),
            "template {role} must select only its no-host SDK profile"
        );
        check_ring_first(&fs::read_to_string(
            template.join(role).join("src/main.rs"),
        )?)?;
    }
    for package in ["rtao", "rtao-testkit", "rtao-runtime"] {
        ensure!(
            manifest["workspace"]["dependencies"][package]["git"].as_str() == Some(SDK_SOURCE),
            "template {package} git source differs"
        );
    }
    Ok(())
}

fn check_container_licenses(root: &Path) -> Result<()> {
    for license in &crate::release::LICENSES {
        let path = root.join(license.source);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file()
                && metadata.len() == license.bytes.len() as u64
                && metadata.len() <= 1048576,
            "container notice {} must be an ordinary file of the source size",
            license.name
        );
        crate::release::validate_license_bytes(license, &fs::read(path)?)?;
    }
    for (file, source, runtime, direct) in [
        (
            "templates/subnet/Dockerfile",
            "/rtao-src/",
            "/usr/share/licenses/subnet",
            false,
        ),
        (
            "ci/validator/Dockerfile",
            "",
            "/usr/share/licenses/rtao",
            true,
        ),
    ] {
        check_container_notice_guards(
            &fs::read_to_string(root.join(file))?,
            source,
            runtime,
            direct,
        )?;
    }
    Ok(())
}

fn check_container_notice_guards(
    docker: &str,
    source: &str,
    runtime: &str,
    direct: bool,
) -> Result<()> {
    for license in &crate::release::LICENSES {
        let source_path = format!("{source}{}", license.source);
        let runtime_path = format!("{runtime}/{}", license.name);
        for path in [&source_path, &runtime_path] {
            ensure!(
                docker.contains(&format!("test -f {path} \\\n"))
                    && docker.contains(&format!("test ! -L {path} \\\n")),
                "container lacks regular-file or symlink guards for {path}"
            );
            if license.sha256.is_some() {
                ensure!(
                    docker.contains(&format!(
                        "test \"$(stat -c %s {path})\" = {}",
                        license.bytes.len()
                    )),
                    "container lacks the exact size guard for {path}"
                );
            }
        }
        if let Some(expected) = license.sha256 {
            ensure!(
                docker.matches(expected).count() == 2,
                "container must verify {} in both stages",
                license.name
            );
        }
        let copy = if direct {
            format!("/workspace/{}", license.source)
        } else {
            ensure!(
                docker.contains(&format!(
                    "install -Dm644 {source_path} /out/{}",
                    license.name
                )),
                "container omits notice staging for {}",
                license.name
            );
            format!("/out/{}", license.name)
        };
        ensure!(
            docker.contains(&format!("COPY --from=build {copy} {runtime_path}")),
            "container omits the runtime copy of {}",
            license.name
        );
    }
    let notice = format!("{source}NOTICE");
    ensure!(
        docker.contains(&format!("notice_size=\"$(stat -c %s {notice})\""))
            && docker.contains(&format!("notice_size=\"$(stat -c %s {runtime}/NOTICE)\""))
            && docker.matches("test \"$notice_size\" -gt 0 && test \"$notice_size\" -le 1048576").count() == 2
            && docker.contains("printf '%s\\n' \"$notice_size\" > /out/NOTICE.size")
            && docker.contains(&format!("printf '%s  %s\\n' \"$(sha256sum {notice} | cut -d' ' -f1)\" {runtime}/NOTICE > /out/NOTICE.sha256"))
            && docker.contains(&format!("test \"$notice_size\" = \"$(cat {runtime}/NOTICE.size)\""))
            && docker.contains(&format!("sha256sum --check --strict {runtime}/NOTICE.sha256")),
        "container NOTICE must be bounded and tied to the exact build source size/hash"
    );
    for metadata in ["NOTICE.size", "NOTICE.sha256"] {
        ensure!(
            docker.contains(&format!(
                "COPY --from=build /out/{metadata} {runtime}/{metadata}"
            )) && docker.contains(&format!("test -f {runtime}/{metadata}"))
                && docker.contains(&format!("test ! -L {runtime}/{metadata}")),
            "container omits source NOTICE identity metadata or its guards"
        );
    }
    if !direct {
        ensure!(
            docker.contains(&format!("cmp {notice} /out/NOTICE")),
            "container NOTICE staging must retain source bytes"
        );
    }
    Ok(())
}

fn check_ring_first(source: &str) -> Result<()> {
    let file = syn::parse_file(source)?;
    let main = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "main" => Some(function),
            _ => None,
        })
        .context("template binary has no main")?;
    let first = main
        .block
        .stmts
        .first()
        .context("template main is empty")?
        .to_token_stream()
        .to_string();
    ensure!(
        first.starts_with("rustls :: crypto :: ring :: default_provider () . install_default ()"),
        "template binary must install ring first in main"
    );
    Ok(())
}

fn cleanup(root: &Path) -> Result<()> {
    ensure!(
        fs::read(root.join(".template-check-owned"))?
            == b"rtao generated localnet development fixture\n",
        "generated ownership marker changed"
    );
    let compose_identity = root.join("fixture/compose-project.txt");
    if compose_identity.is_file() {
        let project = fs::read_to_string(compose_identity)?;
        let project = project.trim();
        ensure!(
            project.starts_with("rtao-template-")
                && project
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "invalid compose cleanup identity"
        );
        let containers = Command::new("docker")
            .args([
                "ps",
                "--all",
                "--quiet",
                "--filter",
                &format!("label=com.docker.compose.project={project}"),
            ])
            .output()?;
        ensure!(
            containers.status.success() && containers.stdout.is_empty(),
            "generated compose containers remain live; retain their wallet/state mounts"
        );
    }
    let output = Command::new("lsof")
        .arg("-w")
        .arg("+D")
        .arg(root)
        .output()
        .context("lsof is required to audit generated-project cleanup")?;
    ensure!(
        output.status.code() == Some(1) && output.stdout.is_empty() && output.stderr.is_empty(),
        "generated directory still has open files or its lsof audit failed"
    );
    fn no_links(path: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "generated project contains a symlink: {}",
            path.display()
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                no_links(&entry?.path())?;
            }
        }
        Ok(())
    }
    no_links(root)?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_license_bytes_and_both_stage_guards_match() -> Result<()> {
        check_container_licenses(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .context("root missing")?,
        )
    }

    #[test]
    fn container_notice_guards_reject_each_omission() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("root missing")?;
        for (file, source, runtime, direct) in [
            (
                "templates/subnet/Dockerfile",
                "/rtao-src/",
                "/usr/share/licenses/subnet",
                false,
            ),
            (
                "ci/validator/Dockerfile",
                "",
                "/usr/share/licenses/rtao",
                true,
            ),
        ] {
            let docker = fs::read_to_string(root.join(file))?;
            for license in &crate::release::LICENSES {
                let source_path = format!("{source}{}", license.source);
                let runtime_path = format!("{runtime}/{}", license.name);
                let copy = if direct {
                    format!("/workspace/{}", license.source)
                } else {
                    format!("/out/{}", license.name)
                };
                let mut guards = vec![format!("COPY --from=build {copy} {runtime_path}")];
                for path in [source_path, runtime_path] {
                    guards.push(format!("test -f {path}"));
                    guards.push(format!("test ! -L {path}"));
                    if license.sha256.is_some() {
                        guards.push(format!(
                            "test \"$(stat -c %s {path})\" = {}",
                            license.bytes.len()
                        ));
                    }
                }
                if let Some(hash) = license.sha256 {
                    guards.push(hash.to_owned());
                }
                for guard in guards {
                    let changed = docker.replacen(&guard, "REMOVED", 1);
                    assert_ne!(changed, docker);
                    assert!(
                        check_container_notice_guards(&changed, source, runtime, direct).is_err(),
                        "missing guard {guard}"
                    );
                }
            }
            for guard in [
                "test \"$notice_size\" -le 1048576".to_owned(),
                "printf '%s\\n' \"$notice_size\" > /out/NOTICE.size".to_owned(),
                format!("sha256sum {source}NOTICE | cut -d' ' -f1"),
                format!("COPY --from=build /out/NOTICE.sha256 {runtime}/NOTICE.sha256"),
                format!("test ! -L {runtime}/NOTICE.size"),
                format!("sha256sum --check --strict {runtime}/NOTICE.sha256"),
            ] {
                let changed = docker.replacen(&guard, "REMOVED", 1);
                assert_ne!(changed, docker);
                assert!(
                    check_container_notice_guards(&changed, source, runtime, direct).is_err(),
                    "missing guard {guard}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn template_binaries_install_ring_before_any_other_work() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("root missing")?;
        check_source(root)?;
        for role in ["validator", "miner"] {
            let source =
                fs::read_to_string(root.join("templates/subnet").join(role).join("src/main.rs"))?;
            let changed = source.replacen(
                "rustls::crypto::ring::default_provider()",
                "rustls::crypto::aws_lc_rs::default_provider()",
                1,
            );
            ensure!(
                check_ring_first(&changed).is_err(),
                "a different TLS provider passed the template gate"
            );
            let late = source.replacen(
                "async fn main() -> anyhow::Result<()> {",
                "async fn main() -> anyhow::Result<()> { let _config = ();",
                1,
            );
            ensure!(
                check_ring_first(&late).is_err(),
                "provider installation after configuration passed the gate"
            );
        }
        Ok(())
    }

    #[test]
    fn generated_cleanup_preserves_work_failures_and_propagates_down_failure() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("root missing")?;
        let script = fs::read_to_string(root.join("templates/subnet/ci/check.sh"))?;
        let start = script
            .find("cleanup() {")
            .context("cleanup function missing")?;
        let end = script[start..]
            .find("trap cleanup EXIT")
            .context("cleanup trap missing")?;
        let cleanup = &script[start..start + end];
        let task_root = PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT")
                .context("RTAO_TEMP_ROOT required for cleanup proof")?,
        );
        for (work, down, expected) in [(0, 0, 0), (0, 41, 1), (23, 0, 23), (23, 41, 23)] {
            let fixture = task_root.join(format!(
                "template-trap-{}-{work}-{down}",
                std::process::id()
            ));
            fs::create_dir(&fixture)?;
            // Only docker's failure status is injected; execute the actual
            // shipped Bash cleanup function and EXIT-trap control flow.
            let proof = format!(
                "set -euo pipefail\ndocker() {{ if [[ \" $* \" = *\" down \"* ]]; then return \"$TEMPLATE_DOWN_EXIT\"; fi; return 0; }}\n{cleanup}\ntrap cleanup EXIT\nexit \"$TEMPLATE_WORK_EXIT\"\n"
            );
            let result = Command::new("bash")
                .args(["-c", &proof])
                .env("RTAO_TEMP_ROOT", &fixture)
                .env("TEMPLATE_WORK_EXIT", work.to_string())
                .env("TEMPLATE_DOWN_EXIT", down.to_string())
                .status();
            // The exited Bash wrote only this bounded log. No real docker,
            // launchd unit, config, symlink, wallet or virtualenv was created.
            let log = fixture.join("compose.log");
            if log.try_exists()? {
                ensure!(
                    !fs::symlink_metadata(&log)?.file_type().is_symlink(),
                    "cleanup proof log became a symlink"
                );
                fs::remove_file(log)?;
            }
            fs::remove_dir(fixture)?;
            ensure!(
                result?.code() == Some(expected),
                "work={work}, down={down} did not preserve the required exit status"
            );
        }
        Ok(())
    }
}
