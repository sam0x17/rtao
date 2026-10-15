//! CI-only current-spec upstream recording without weakening its localhost guard.

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use toml_edit::DocumentMut;

fn verify(root: &Path, out: &Path, spec: i64) -> Result<()> {
    let manifest = crate::oracle::read_json(&out.join("manifest.json"))?;
    ensure!(
        manifest["schema"].as_u64() == Some(1) && manifest["spec"].as_i64() == Some(spec),
        "recorded manifest spec/schema differs"
    );
    crate::oracle::check_identity(root, &manifest["image"])?;
    let files = manifest["files"]
        .as_object()
        .context("recorded files missing")?;
    let mut actual = BTreeSet::new();
    crate::oracle::inventory(out, &mut actual, out)?;
    actual.remove("manifest.json");
    ensure!(
        actual == files.keys().cloned().collect::<BTreeSet<_>>(),
        "recorded inventory differs from manifest"
    );
    for name in [
        "golden.json",
        "shape_corpus/corpus.json",
        "dropped.json",
        "image.json",
    ] {
        ensure!(files.contains_key(name), "recording output {name} missing");
    }
    for (name, hash) in files {
        ensure!(
            !name.is_empty()
                && !name.starts_with('/')
                && !name.contains('\\')
                && name
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != ".."),
            "unsafe recording path"
        );
        let path = out.join(name);
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "recording must be regular files"
        );
        ensure!(
            hash.as_str() == Some(format!("{:x}", Sha256::digest(fs::read(path)?)).as_str()),
            "recorded fixture hash differs: {name}"
        );
    }
    rtao_primitives::json::compare(
        &crate::oracle::read_json(&out.join("image.json"))?,
        &manifest["image"],
    )?;
    let golden = crate::oracle::read_json(&out.join("golden.json"))?;
    ensure!(
        golden["network"]["spec_version"].as_i64() == Some(spec),
        "recorded golden spec differs"
    );
    let corpus = crate::oracle::read_json(&out.join("shape_corpus/corpus.json"))?;
    ensure!(
        corpus["spec_version"].as_i64() == Some(spec),
        "recorded corpus spec differs"
    );
    println!(
        "Current-spec fixture identity and checksums verified; Rust wire/corpus replay remains required."
    );
    Ok(())
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = crate::oracle::workspace_root()?;
    let mut values = BTreeMap::new();
    let mut plan = false;
    let mut check = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" | "--built-image-id" | "--localnet" | "--out"
                if !values.contains_key(&arg) =>
            {
                values.insert(
                    arg.clone(),
                    args.next()
                        .with_context(|| format!("{arg} needs a value"))?,
                );
            }
            "--plan" if !plan => plan = true,
            "--check" if !check => check = true,
            _ => bail!(
                "usage: cargo xtask record --image <oracle@sha256> --localnet <owned-container-id> [--out directory] [--plan]"
            ),
        }
    }
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let spec = pins["parity"]["spec_version"]
        .as_integer()
        .context("missing parity spec")?;
    if check {
        ensure!(
            !plan && values.keys().all(|key| key == "--out"),
            "record --check takes only --out"
        );
        let out = values
            .get("--out")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(format!("vendor/recorded/{spec}")));
        return verify(root, &out, spec);
    }
    let image = match (values.get("--image"), values.get("--built-image-id")) {
        (Some(image), None) => {
            crate::oracle::pinned_image(image)?;
            image
        }
        (None, Some(image)) => {
            ensure!(
                image
                    .strip_prefix("sha256:")
                    .is_some_and(|hash| hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
                "built image must be an immutable Docker image ID"
            );
            image
        }
        _ => bail!(
            "record needs exactly one --image GHCR digest or --built-image-id from this hosted build"
        ),
    };
    let localnet = values.get("--localnet").context("--localnet is required")?;
    ensure!(
        localnet.len() == 64
            && localnet
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "localnet must be an exact container ID"
    );
    let out = values
        .get("--out")
        .map(PathBuf::from)
        .unwrap_or(crate::oracle::task_path(&format!("recorded-{spec}"))?);
    let command = vec![
        "run".into(),
        "--rm".into(),
        "--network".into(),
        format!("container:{localnet}"),
        "--env".into(),
        "GITHUB_ACTIONS=true".into(),
        "--env".into(),
        "RTAO_PYTHON_LANE=record".into(),
        "--env".into(),
        format!("RTAO_ORACLE_EXECUTION_IMAGE={image}"),
        "--mount".into(),
        format!("type=bind,src={},dst=/output", out.display()),
        "--entrypoint".into(),
        "/upstream/sdk/python/.venv/bin/python".into(),
        image.clone(),
        "/oracle/record.py".into(),
        "--spec".into(),
        spec.to_string(),
    ];
    if plan {
        println!("{}", serde_json::to_string_pretty(&command)?);
        return Ok(());
    }
    crate::oracle::require_lane("record")?;
    let inspected = Command::new("docker")
        .args(["image", "inspect", image])
        .output()?;
    ensure!(
        inspected.status.success(),
        "cannot inspect recorder execution image"
    );
    let inspected: serde_json::Value = serde_json::from_slice(&inspected.stdout)?;
    let labels = &inspected[0]["Config"]["Labels"];
    let (tag, commit, input_hash) = crate::oracle::execution_identity(root)?;
    ensure!(
        labels["org.opencontainers.image.revision"].as_str() == Some(commit.as_str())
            && labels["io.rtao.oracle.parity-tag"].as_str() == Some(tag.as_str())
            && labels["io.rtao.oracle.input-sha256"].as_str() == Some(input_hash.as_str()),
        "recorder execution image labels differ from current oracle inputs"
    );
    let inspection = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.State.Running}} {{index .Config.Labels \"io.rtao.localnet\"}}",
            localnet,
        ])
        .output()?;
    ensure!(
        inspection.status.success()
            && String::from_utf8(inspection.stdout)?.trim() == "true recording",
        "refusing a container without the designated recording-localnet ownership label"
    );
    crate::oracle::new_task_output(&out)?;
    ensure!(
        Command::new("docker").args(&command).status()?.success(),
        "upstream recording failed; prior recorded fixtures are unchanged"
    );
    verify(root, &out, spec)?;
    println!(
        "Recorded spec {spec} into {}. No committed fixture is overwritten by this command.",
        out.display()
    );
    Ok(())
}
