//! CI-only oracle execution and Rust-only offline integrity/behavior checks.

mod hyperparams;
mod replay;

#[path = "oracle_readiness.rs"]
mod readiness;
pub(crate) use readiness::check_committed;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use rustpython_parser::{Parse, ast};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

pub(crate) const M2_INTENTS: [&str; 9] = [
    "set_weights",
    "commit_weights",
    "reveal_weights",
    "serve_axon",
    "serve_axon_tls",
    "serve_prometheus",
    "reset_axon",
    "transfer",
    "transfer_all",
];
pub(crate) const M2_READS: [&str; 15] = [
    "block_time",
    "timestamp",
    "blocks_since_last_step",
    "blocks_since_last_update",
    "epoch_status",
    "max_weight_limit",
    "min_allowed_weights",
    "reveal_period",
    "weights_rate_limit",
    "uid",
    "commit_reveal_enabled",
    "mechanism_count",
    "weights",
    "metagraph",
    "token_symbols",
];
const SCRIPTS: [&str; 11] = [
    "common.py",
    "dump_intents.py",
    "dump_reads.py",
    "dump_pure.py",
    "dump_cli.py",
    "image_identity.py",
    "run.py",
    "record.py",
    "wait_localnet.py",
    "pins.env",
    "build-requirements.txt",
];
const IMAGE: &str = "ghcr.io/sam0x17/rtao-oracle";
const MAX_JSON: u64 = 64 * 1024 * 1024;

fn root() -> Result<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root")
}

pub(crate) fn workspace_root() -> Result<&'static Path> {
    root()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn parity_row(source: &str) -> Result<String> {
    let map: DocumentMut = fs::read_to_string(root()?.join("parity/map.toml"))?.parse()?;
    let rows = map
        .get("file")
        .and_then(toml_edit::Item::as_array_of_tables)
        .context("parity map rows missing")?;
    let row = rows
        .iter()
        .find(|row| row.get("py").and_then(toml_edit::Item::as_str) == Some(source))
        .with_context(|| format!("parity map has no source row {source}"))?;
    Ok(format!("parity/map.toml row: {}", row))
}

pub(crate) fn read_json(path: &Path) -> Result<Value> {
    ensure!(
        fs::symlink_metadata(path)?.is_file() && fs::metadata(path)?.len() <= MAX_JSON,
        "oracle JSON must be a bounded regular file"
    );
    serde_json::from_slice(&fs::read(path)?).with_context(|| format!("parse {}", path.display()))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing oracle string {key}"))
}

fn pin(root: &Path) -> Result<(String, String, u32)> {
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let parity = pins.get("parity").context("missing parity pin")?;
    let tag = parity
        .get("tag")
        .and_then(toml_edit::Item::as_str)
        .context("missing parity tag")?;
    let commit = parity
        .get("commit")
        .and_then(toml_edit::Item::as_str)
        .context("missing parity commit")?;
    ensure!(
        tag.strip_prefix('v').is_some_and(
            |value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
        ),
        "noncanonical parity tag"
    );
    ensure!(
        commit.len() == 40
            && commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "noncanonical parity commit"
    );
    let spec = u32::try_from(
        parity
            .get("spec_version")
            .and_then(toml_edit::Item::as_integer)
            .context("missing parity spec")?,
    )?;
    Ok((tag.to_owned(), commit.to_owned(), spec))
}

fn inputs(root: &Path) -> Result<(String, BTreeMap<String, Vec<u8>>)> {
    let mut files = BTreeMap::new();
    for name in SCRIPTS.into_iter().chain(["Dockerfile"]) {
        let path = root.join("ci/oracle").join(name);
        ensure!(
            fs::symlink_metadata(&path)?.is_file(),
            "oracle source must be a regular file"
        );
        let bytes = fs::read(path)?;
        if name.ends_with(".py") {
            ast::Suite::parse(std::str::from_utf8(&bytes)?, name)
                .with_context(|| format!("Rust AST parser rejected oracle script {name}"))?;
        }
        files.insert(name.to_owned(), bytes);
    }
    files.insert(
        "oracle-lock.json".into(),
        fs::read(root.join("vendor/oracle/lock.json"))?,
    );
    files.insert(
        "oracle-runtime-lock.json".into(),
        fs::read(root.join("vendor/oracle/runtime-lock.json"))?,
    );
    let mut hash = Sha256::new();
    for (name, bytes) in &files {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok((format!("{:x}", hash.finalize()), files))
}

/// Source identity used by both the dump and recording image checks.
pub(crate) fn execution_identity(root: &Path) -> Result<(String, String, String)> {
    let (tag, commit, _) = pin(root)?;
    let (input_hash, _) = inputs(root)?;
    Ok((tag, commit, input_hash))
}

pub(crate) fn require_lane(lane: &str) -> Result<()> {
    ensure!(
        std::env::consts::OS == "linux"
            && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Linux")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("GITHUB_REPOSITORY").as_deref() == Ok("sam0x17/rtao")
            && std::env::var("RTAO_PYTHON_LANE").as_deref() == Ok(lane),
        "Python execution is restricted to the designated hosted Ubuntu {lane} lane; local execution and local containers are prohibited"
    );
    Ok(())
}

pub(crate) fn task_path(name: &str) -> Result<PathBuf> {
    let root =
        PathBuf::from(std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?);
    ensure!(
        root.is_dir() && !fs::symlink_metadata(&root)?.file_type().is_symlink(),
        "task root must be an existing regular directory"
    );
    Ok(root.canonicalize()?.join(name))
}

pub(crate) fn new_task_output(path: &Path) -> Result<()> {
    let temporary = task_path("output-placeholder")?
        .parent()
        .context("task root")?
        .to_owned();
    ensure!(
        path.is_absolute()
            && path.starts_with(&temporary)
            && path != temporary
            && path
                .components()
                .all(|component| matches!(component, Component::RootDir | Component::Normal(_))),
        "oracle/record output must be strictly inside the recorded task root without dot components"
    );
    let mut parent = temporary;
    let relative = path
        .parent()
        .context("output parent")?
        .strip_prefix(&parent)?
        .to_owned();
    for component in relative.components() {
        parent.push(component);
        if parent.exists() {
            ensure!(
                fs::symlink_metadata(&parent)?.is_dir(),
                "output ancestor must be a regular directory"
            );
        } else {
            fs::create_dir(&parent)?;
        }
    }
    ensure!(
        !path.exists() && fs::symlink_metadata(path).is_err(),
        "output must be a fresh task directory"
    );
    fs::create_dir(path)?;
    Ok(())
}

fn execute(args: &[String]) -> Result<()> {
    let status = Command::new("docker")
        .args(args)
        .status()
        .context("Docker is required on the designated hosted lane")?;
    ensure!(status.success(), "oracle Docker command failed");
    Ok(())
}

pub(crate) fn pinned_image(value: &str) -> Result<()> {
    ensure!(
        value
            .strip_prefix(&format!("{IMAGE}@sha256:"))
            .is_some_and(|hash| hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))),
        "oracle image must be the GHCR namespace pinned by SHA-256 digest"
    );
    Ok(())
}

fn relative(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && value
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "unsafe oracle manifest path"
    );
    Ok(path)
}

pub(crate) fn check_identity(root: &Path, value: &Value) -> Result<()> {
    let (tag, commit, _) = pin(root)?;
    let (hash, _) = inputs(root)?;
    ensure!(
        value["schema"].as_u64() == Some(1)
            && text(value, "parity_tag")? == tag
            && text(value, "parity_commit")? == commit
            && text(value, "oracle_input_sha256")? == hash,
        "oracle image/source identity is stale"
    );
    ensure!(
        text(value, "python")? == "3.12.11"
            && text(value, "implementation")? == "cpython"
            && text(value, "uv")? == "uv 0.8.24",
        "oracle interpreter/tool pin differs"
    );
    for field in [
        "binding_sha256",
        "upstream_cargo_lock_sha256",
        "upstream_uv_lock_sha256",
    ] {
        let hash = text(value, field)?;
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "invalid oracle {field}"
        );
    }
    rtao_primitives::json::compare(
        &read_json(&root.join("vendor/oracle/runtime-lock.json"))?,
        &value["runtime_lock"],
    )?;
    rtao_primitives::json::compare(
        &read_json(&root.join("vendor/oracle/runtime-lock.json"))?["build_backends"],
        &value["build_backends"],
    )?;
    ensure!(
        text(value, "build_requirements_sha256")?
            == sha(&fs::read(root.join("ci/oracle/build-requirements.txt"))?),
        "oracle build-backend requirements differ"
    );
    Ok(())
}

fn hits(root: &Path, name: &str) -> Result<BTreeSet<(String, String, String)>> {
    let extracted = read_json(&root.join("vendor/upstream/extracted/read_hits.json"))?;
    let (tag, commit, _) = pin(root)?;
    ensure!(
        extracted["format"] == 1
            && extracted["parity"]["tag"] == tag
            && extracted["parity"]["commit"] == commit,
        "stale extracted read-hit identity"
    );
    let row = extracted["reads"]
        .as_array()
        .context("extracted read hits must be an array")?
        .iter()
        .find(|row| row["name"].as_str() == Some(name))
        .with_context(|| format!("unknown oracle read {name}"))?;
    let provenance = &row["provenance"];
    let source = text(provenance, "path")?;
    let bytes = fs::read(root.join("vendor/upstream/src").join(relative(source)?))?;
    ensure!(
        text(provenance, "sha256")? == sha(&bytes),
        "read-hit source authentication failed for {name}"
    );
    let mut blob = sha1::Sha1::new();
    blob.update(format!("blob {}\0", bytes.len()).as_bytes());
    blob.update(&bytes);
    ensure!(
        text(provenance, "blob")? == format!("{:x}", blob.finalize()),
        "read-hit Git blob authentication failed for {name}"
    );
    let mut result = BTreeSet::new();
    for hit in row
        .get("hits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        result.insert((
            text(hit, "kind")?.into(),
            text(hit, "container")?.into(),
            text(hit, "name")?.into(),
        ));
    }
    let extra = root.join("parity/read_hits_extra.toml");
    if extra.exists() {
        let extra: DocumentMut = fs::read_to_string(extra)?.parse()?;
        if let Some(rows) = extra
            .get("read")
            .and_then(|reads| reads.get(name))
            .and_then(|row| row.get("hits"))
            .and_then(toml_edit::Item::as_array)
        {
            for row in rows {
                let row = row
                    .as_inline_table()
                    .context("reviewed read hits must be inline tables")?;
                let value = |key: &str| -> Result<String> {
                    Ok(row
                        .get(key)
                        .and_then(toml_edit::Value::as_str)
                        .with_context(|| format!("extra read hit missing {key}"))?
                        .into())
                };
                result.insert((value("kind")?, value("container")?, value("name")?));
            }
        }
    }
    Ok(result)
}

fn check_reads(root: &Path, document: &Value, scope: &str) -> Result<()> {
    let rows = document["vectors"]
        .as_array()
        .context("missing read vectors")?;
    let mut found = BTreeMap::<String, BTreeSet<(String, String, String)>>::new();
    let mut cases = BTreeSet::new();
    for row in rows {
        for field in [
            "name", "sample", "block", "params", "source", "result", "touched",
        ] {
            ensure!(row.get(field).is_some(), "read vector lacks {field}");
        }
        let name = text(row, "name")?;
        let sample = text(row, "sample")?;
        let block = match &row["block"] {
            Value::Null => "head",
            value if value.as_u64() == Some(90) => "90",
            _ => bail!("read vector must run at head or block 90"),
        };
        ensure!(
            cases.insert((name.to_owned(), sample.to_owned(), block)),
            "duplicated read case"
        );
        let allowed = hits(root, name)?;
        let touched = row["touched"]
            .as_array()
            .context("read vector must record touched items")?;
        for hit in touched {
            let item = (
                text(hit, "kind")?.into(),
                text(hit, "container")?.into(),
                text(hit, "name")?.into(),
            );
            ensure!(
                allowed.contains(&item),
                "Python oracle read {name} touches {:?} outside reviewed H; source {} and parity/map.toml",
                item,
                text(row, "source")?
            );
            found.entry(name.into()).or_default().insert(item);
        }
        found.entry(name.into()).or_default();
    }
    let expected: BTreeSet<_> = if scope == "M2" {
        M2_READS.iter().map(|name| (*name).to_owned()).collect()
    } else {
        read_json(&root.join("vendor/upstream/catalog/reads.json"))?
            .as_array()
            .context("read catalog")?
            .iter()
            .map(|row| Ok(text(row, "name")?.to_owned()))
            .collect::<Result<_>>()?
    };
    ensure!(
        found.keys().cloned().collect::<BTreeSet<_>>() == expected,
        "oracle read scope is incomplete"
    );
    for (name, actual) in found {
        ensure!(
            actual == hits(root, &name)?,
            "oracle samples do not cover every reviewed hit of {name}"
        );
        ensure!(
            cases.contains(&(name.clone(), "upstream".into(), "head"))
                && cases.contains(&(name, "upstream".into(), "90")),
            "read lacks both upstream head and pinned cases"
        );
    }
    Ok(())
}

/// Require every reviewed input exactly once, with an exact one-to-one seed.
fn check_cases(
    root: &Path,
    dir: &Path,
    document: &Value,
    kind: &str,
    names: &BTreeSet<String>,
) -> Result<()> {
    ensure!(document["schema"] == 1, "unsupported {kind} vector schema");
    let extras = read_json(&root.join("vendor/oracle/samples_extra.json"))?;
    let rows = document["vectors"].as_array().context("oracle vectors")?;
    let mut expected = BTreeSet::new();
    for name in names {
        let mut samples = BTreeSet::from(["upstream".to_owned()]);
        for sample in extras[kind]
            .get(name)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = text(sample, "id")?;
            ensure!(
                id != "upstream" && !id.is_empty() && samples.insert(id.to_owned()),
                "duplicated/reserved extra sample {name}/{id}"
            );
            ensure!(
                sample["review"]
                    .as_str()
                    .is_some_and(|review| !review.is_empty()),
                "extra sample {name}/{id} needs a source review"
            );
        }
        let seed = read_json(&dir.join("seeds").join(format!("{name}.json")))?;
        let singular = if kind == "reads" { "read" } else { "intent" };
        ensure!(
            seed["schema"] == 1 && seed["name"] == name.as_str() && seed["kind"] == singular,
            "seed identity differs for {name}"
        );
        let mut seed_cases = BTreeSet::new();
        for row in seed["samples"].as_array().context("seed samples missing")? {
            let case = (
                name.clone(),
                text(row, "sample")?.to_owned(),
                if kind == "reads" {
                    row["block"].to_string()
                } else {
                    String::new()
                },
            );
            ensure!(seed_cases.insert(case), "duplicated seed case for {name}");
        }
        let mut name_cases = BTreeSet::new();
        for sample in samples {
            for block in if kind == "reads" {
                vec!["null", "90"]
            } else {
                vec![""]
            } {
                name_cases.insert((name.clone(), sample.clone(), block.to_owned()));
            }
        }
        ensure!(
            seed_cases == name_cases,
            "seed case inventory differs for {name}"
        );
        expected.extend(name_cases);
    }
    let mut actual = BTreeSet::new();
    for row in rows {
        let name = text(row, "name")?;
        let sample = text(row, "sample")?;
        ensure!(names.contains(name), "unexpected {kind} operation {name}");
        ensure!(
            actual.insert((
                name.to_owned(),
                sample.to_owned(),
                if kind == "reads" {
                    row["block"].to_string()
                } else {
                    String::new()
                }
            )),
            "duplicated {kind} case {name}/{sample}"
        );
        if sample != "upstream" {
            let input = extras[kind][name]
                .as_array()
                .context("unreviewed oracle operation")?
                .iter()
                .find(|input| input["id"] == sample)
                .context("unreviewed oracle sample")?;
            let field = if kind == "reads" { "params" } else { "args" };
            rtao_primitives::json::compare(&input[field], &row[field])
                .context("reviewed sample arguments changed")?;
        }
        parity_row(text(row, "source")?)?;
    }
    ensure!(
        actual == expected,
        "{kind} case inventory differs from the upstream and reviewed samples"
    );
    Ok(())
}

fn replay_native(root: &Path, dir: &Path, reads: &Value, intents: &Value) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            replay::timelock(&read_json(&dir.join("pure/timelock.json"))?)
                .await
                .with_context(|| {
                    format!(
                        "sdk/python/bittensor/timelock.py; {}",
                        parity_row("sdk/python/bittensor/timelock.py").unwrap_or_default()
                    )
                })?;
            let mut union = BTreeMap::<String, replay::Hits>::new();
            for (kind, document) in [("reads", reads), ("intents", intents)] {
                for row in document["vectors"].as_array().context("native vectors")? {
                    let name = text(row, "name")?;
                    let sample = text(row, "sample")?;
                    let seeds = read_json(&dir.join("seeds").join(format!("{name}.json")))?;
                    let seed = seeds["samples"]
                        .as_array()
                        .context("seed samples")?
                        .iter()
                        .find(|seed| {
                            seed["sample"] == sample
                                && (kind != "reads" || seed["block"] == row["block"])
                        })
                        .context("matching native seed")?;
                    let result = if kind == "reads" {
                        replay::read(row, &seed["state"]).await.map(|observed| {
                            union.entry(name.to_owned()).or_default().extend(observed);
                        })
                    } else {
                        replay::intent(row, &seed["state"]).await
                    };
                    result.with_context(|| {
                        format!(
                            "native {kind} {name}/{sample}/{}; {}; {}",
                            row["block"],
                            row["source"],
                            parity_row(text(row, "source").unwrap_or_default()).unwrap_or_default()
                        )
                    })?;
                }
            }
            for (name, actual) in union {
                ensure!(
                    actual == hits(root, &name)?,
                    "native sample union differs from authenticated H for {name}"
                );
            }
            Ok(())
        })
}

fn check_pure(dir: &Path) -> Result<()> {
    let balance_row = parity_row("sdk/python/bittensor/balance.py")?;
    let error_row = parity_row("sdk/python/bittensor/result.py")?;
    let balances = read_json(&dir.join("pure/balance.json"))?;
    ensure!(
        text(&balances, "domain")? == "u64-rao",
        "balance vectors must use the native chain domain"
    );
    let rows = balances["vectors"]
        .as_array()
        .context("balance vectors missing")?;
    ensure!(
        rows.len() == 10_000,
        "oracle must contain exactly 10,000 native balance vectors"
    );
    let mut rao_seen = BTreeSet::new();
    for row in rows {
        let rao = row["rao"]
            .as_u64()
            .context("balance vector is outside u64 rao")?;
        ensure!(rao_seen.insert(rao), "duplicate balance vector");
        let actual =
            rtao_primitives::DynBalance::from_rao(rao, rtao_primitives::NetUid(0)).to_string();
        ensure!(
            text(row, "display")? == actual,
            "Python Balance display mismatch at {rao} rao: expected {}, actual {actual}; sdk/python/bittensor/balance.py; {balance_row}",
            text(row, "display")?
        );
    }
    for edge in [
        0,
        1,
        (1_u64 << 53) - 1,
        1_u64 << 53,
        (1_u64 << 53) + 1,
        u64::MAX,
    ] {
        ensure!(rao_seen.contains(&edge), "balance edge coverage missing");
    }
    let errors = read_json(&dir.join("pure/classification.json"))?;
    for row in errors["messages"]
        .as_array()
        .context("classifier messages missing")?
    {
        let expected = text(row, "code")?;
        let actual =
            rtao_catalog::errors::classify_error(text(row, "text")?, row["name"].as_str()).as_str();
        ensure!(
            expected == actual,
            "Python error classifier differs: expected {expected}, actual {actual}; input {row}; sdk/python/bittensor/result.py; {error_row}"
        );
    }
    check_rpc_classification(&errors)?;
    check_conform(&read_json(&dir.join("pure/conform.json"))?)?;
    Ok(())
}

fn check_rpc_classification(document: &Value) -> Result<()> {
    let rows = document["rpc"]
        .as_array()
        .context("RPC classification vectors")?;
    ensure!(
        rows.len() >= 12,
        "RPC classification branch coverage missing"
    );
    for row in rows {
        let payload = &row["payload"];
        let native = rtao_rpc::classify_response(&json!({"error":payload}))
            .err()
            .context("native RPC classifier accepted an error")?;
        let (kind, retry, policy, reason, block) = match &native {
            rtao_rpc::RpcError::Policy(policy) => (
                "RpcPolicyRejection",
                policy.retry_after.clone(),
                policy.policy.clone(),
                policy.reason.clone(),
                None,
            ),
            rtao_rpc::RpcError::StateDiscarded { block } => {
                ("StateDiscardedError", None, None, None, Some(block.clone()))
            }
            rtao_rpc::RpcError::JsonRpc { .. } => {
                ("SubstrateRequestException", None, None, None, None)
            }
            error => bail!("unexpected pure RPC classifier variant: {error:?}"),
        };
        let actual = json!({"type":kind,"message":native.to_string(),"retry_after":retry,"policy":policy,"reason":reason,"block_hash":block});
        let source = "sdk/python/bittensor/_transport/rpc.py";
        rtao_primitives::json::compare(&row["rpc"], &actual).with_context(|| {
            format!(
                "RPC diagnostic differs for {payload}: expected {}, actual {actual}; {source}; {}",
                row["rpc"],
                parity_row(source).unwrap_or_default()
            )
        })?;
        let pool = rtao_client::PoolRejection::from_rpc(
            payload["code"].as_i64().context("RPC code")?,
            text(payload, "message")?,
            payload.get("data"),
        );
        let actual = rtao_tx::ChainError::from_pool(&pool).to_json();
        let source = "sdk/python/bittensor/result.py";
        rtao_primitives::json::compare(&row["chain"],&actual).with_context(||format!("chain classification differs for {payload}: expected {}, actual {actual}; {source}; {}",row["chain"],parity_row(source).unwrap_or_default()))?;
    }
    Ok(())
}

fn weights_result(
    value: Result<(Vec<rtao_primitives::Uid>, Vec<u16>), rtao_intents::weights::WeightsError>,
) -> Value {
    match value {
        Ok((uids, values)) => {
            json!({"ok": [uids.into_iter().map(|uid|uid.0).collect::<Vec<_>>(),values]})
        }
        Err(error) => {
            json!({"error":{"type":if error.code().is_some(){"ChainError"}else{"BittensorError"},"message":error.to_string()}})
        }
    }
}

fn check_conform(document: &Value) -> Result<()> {
    let parity_row = parity_row("sdk/python/bittensor/intents/weights.py")?;
    use rtao_intents::weights::{ConformState, clip_to_max_weight, conform, normalize};
    use rtao_primitives::{NetUid, Uid};
    let rows = document["vectors"]
        .as_array()
        .context("conform vectors missing")?;
    ensure!(
        rows.len() >= 180,
        "conform corpus does not cover required seeded/edge cases"
    );
    let mut large_raw = false;
    let mut compensated = false;
    for (index, row) in rows.iter().enumerate() {
        let weights = row["weights_f64_bits"]
            .as_array()
            .context("weight bit inputs missing")?
            .iter()
            .map(|value| {
                Ok(f64::from_bits(
                    value.as_u64().context("f64 input bits outside u64")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let raw_uids = row["uids"]
            .as_array()
            .context("UID inputs missing")?
            .iter()
            .map(|value| value.as_u64().context("invalid UID input"))
            .collect::<Result<Vec<_>>>()?;
        let uids = raw_uids
            .iter()
            .map(|uid| u16::try_from(*uid).map(Uid))
            .collect::<std::result::Result<Vec<_>, _>>();
        let limit = u16::try_from(
            row["limit_u16"]
                .as_u64()
                .context("missing max-weight limit")?,
        )?;
        let state = ConformState {
            uid: Uid(u16::try_from(
                row["uid"].as_u64().context("missing self UID")?,
            )?),
            min_allowed_weights: u16::try_from(
                row["min_allowed_weights"]
                    .as_u64()
                    .context("missing min count")?,
            )?,
            max_weight_limit: limit,
        };
        let netuid = NetUid(u16::try_from(
            row["netuid"].as_u64().context("missing netuid")?,
        )?);
        let normalized = match &uids {
            Ok(uids) => weights_result(normalize(uids, &weights)),
            Err(_) => {
                json!({"error":{"type":"BittensorError","message":"UIDs must be in 0..65535."}})
            }
        };
        let clipped = match clip_to_max_weight(&weights, f64::from(limit) / f64::from(u16::MAX)) {
            Ok(values) if values.iter().all(|value| value.is_finite()) => json!({"ok":values}),
            Ok(values) => {
                json!({"nonfinite_f64_bits":values.into_iter().map(f64::to_bits).collect::<Vec<_>>()})
            }
            Err(error) => json!({"error":{"type":"BittensorError","message":error.to_string()}}),
        };
        let conformed = match &uids {
            Ok(uids) => weights_result(conform(uids, &weights, state, netuid, false)),
            Err(_) => {
                json!({"error":{"type":"BittensorError","message":"UIDs must be in 0..65535."}})
            }
        };
        let raw = match &uids {
            Ok(uids) => weights_result(conform(uids, &weights, state, netuid, true)),
            Err(_) => {
                json!({"error":{"type":"BittensorError","message":"Raw weights require equal-length lists and u16 UIDs."}})
            }
        };
        for (name, actual) in [
            ("normalize", normalized),
            ("clip", clipped),
            ("conform", conformed),
            ("conform_raw_u16", raw),
        ] {
            let expected = row
                .get(name)
                .with_context(|| format!("conform result {name} missing"))?;
            rtao_primitives::json::compare(expected,&actual).with_context(||format!("Python pure {name} differs at vector {index}; sdk/python/bittensor/intents/weights.py; {parity_row}; expected {expected}, actual {actual}"))?;
        }
        compensated |= weights == [1e16, 1.0, 1.0];
        large_raw |= weights.len() == 2500
            && weights.iter().all(|weight| {
                weight.is_finite() && *weight >= 0.0 && *weight <= 65535.0 && weight.fract() == 0.0
            })
            && row["conform_raw_u16"].get("ok").is_some();
    }
    ensure!(
        compensated && large_raw,
        "conform corpus omits compensated-sum or valid 2500-UID raw row"
    );
    Ok(())
}

pub(crate) fn inventory(dir: &Path, out: &mut BTreeSet<String>, root: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let kind = fs::symlink_metadata(&path)?.file_type();
        ensure!(!kind.is_symlink(), "oracle output contains a symlink");
        if kind.is_dir() {
            inventory(&path, out, root)?;
        } else {
            ensure!(kind.is_file(), "oracle output contains a special file");
            out.insert(
                path.strip_prefix(root)?
                    .to_str()
                    .context("oracle path must be UTF8")?
                    .to_owned(),
            );
        }
    }
    Ok(())
}

pub(crate) fn check(root: &Path, dir: &Path) -> Result<()> {
    // Recompute authenticated AST extractions before trusting read_hits.json.
    crate::extract::check_at(root).context("oracle source extraction authentication")?;
    let manifest = read_json(&dir.join("manifest.json"))?;
    ensure!(
        manifest["schema"].as_u64() == Some(1),
        "unsupported oracle manifest"
    );
    let scope = text(&manifest, "scope")?;
    ensure!(scope == "M2" || scope == "all", "unknown oracle scope");
    check_identity(root, &manifest["image"])?;
    let files = manifest["files"]
        .as_object()
        .context("oracle manifest files missing")?;
    let mut actual = BTreeSet::new();
    inventory(dir, &mut actual, dir)?;
    actual.remove("manifest.json");
    ensure!(
        actual == files.keys().cloned().collect::<BTreeSet<_>>(),
        "oracle output inventory differs from manifest"
    );
    for (name, expected) in files {
        let path = dir.join(relative(name)?);
        ensure!(
            expected.as_str() == Some(sha(&fs::read(path)?).as_str()),
            "oracle hash mismatch: {name}"
        );
    }
    for name in [
        "intents.json",
        "reads.json",
        "cli_tree.json",
        "pure/balance.json",
        "pure/conform.json",
        "pure/classification.json",
        "pure/timelock.json",
        "pure/hyperparams.json",
        "image.json",
        "lock.json",
        "samples_extra.json",
        "runtime-lock.json",
    ] {
        ensure!(
            files.contains_key(name),
            "oracle required file missing: {name}"
        );
    }
    ensure!(
        fs::read(dir.join("lock.json"))? == fs::read(root.join("vendor/oracle/lock.json"))?,
        "oracle lock differs from parity lock"
    );
    rtao_primitives::json::compare(
        &read_json(&root.join("vendor/oracle/runtime-lock.json"))?,
        &read_json(&dir.join("runtime-lock.json"))?,
    )?;
    rtao_primitives::json::compare(
        &read_json(&root.join("vendor/oracle/samples_extra.json"))?,
        &read_json(&dir.join("samples_extra.json"))?,
    )
    .context("oracle reviewed extra samples differ from the committed inputs")?;
    rtao_primitives::json::compare(&read_json(&dir.join("image.json"))?, &manifest["image"])?;
    let reads = read_json(&dir.join("reads.json"))?;
    ensure!(reads["scope"] == scope, "read scope differs from manifest");
    check_reads(root, &reads, scope)?;
    let intents = read_json(&dir.join("intents.json"))?;
    ensure!(
        intents["scope"] == scope,
        "intent scope differs from manifest"
    );
    let mut names = BTreeSet::new();
    for row in intents["vectors"]
        .as_array()
        .context("intent vectors missing")?
    {
        let name = text(row, "name")?;
        names.insert(name.to_owned());
        ensure!(
            files.contains_key(&format!("seeds/{name}.json")),
            "intent seed file missing"
        );
        for field in [
            "schema",
            "intent",
            "call",
            "preflight",
            "plan",
            "fee_rao",
            "spend",
            "extras",
            "touched",
        ] {
            ensure!(
                row.get(field).is_some(),
                "intent vector {name} lacks {field}"
            );
        }
    }
    let expected: BTreeSet<_> = if scope == "M2" {
        M2_INTENTS.iter().map(|name| (*name).to_owned()).collect()
    } else {
        read_json(&root.join("vendor/upstream/catalog/intents.json"))?
            .as_array()
            .context("intent catalog")?
            .iter()
            .map(|row| Ok(text(row, "name")?.to_owned()))
            .collect::<Result<_>>()?
    };
    ensure!(names == expected, "oracle intent scope is incomplete");
    let read_names = reads["vectors"]
        .as_array()
        .context("read vectors")?
        .iter()
        .map(|row| Ok(text(row, "name")?.to_owned()))
        .collect::<Result<BTreeSet<_>>>()?;
    check_cases(root, dir, &reads, "reads", &read_names)?;
    check_cases(root, dir, &intents, "intents", &expected)?;
    check_pure(dir)?;
    hyperparams::check(root, &read_json(&dir.join("pure/hyperparams.json"))?)?;
    replay_native(root, dir, &reads, &intents)?;
    println!(
        "Oracle pins, complete sample/seed inventory, exact Balance/classification/conform/hyperparameter behavior, Quicknet arithmetic and native intent/read replay with authenticated H verified."
    );
    Ok(())
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = root()?;
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "--check".into());
    let mut values = BTreeMap::new();
    let mut plan = false;
    let mut push = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" | "--built-image-id" | "--out" | "--scope" if !values.contains_key(&arg) => {
                let value = args
                    .next()
                    .with_context(|| format!("{arg} needs a value"))?;
                values.insert(arg, value);
            }
            "--plan" if !plan => plan = true,
            "--push" if !push => push = true,
            _ => bail!(
                "usage: cargo xtask oracle inputs|image|dump|--check [--plan] [--push] [--image ghcr.io/sam0x17/rtao-oracle@sha256:...] [--out directory] [--scope M2|all]"
            ),
        }
    }
    let (tag, commit, _) = pin(root)?;
    let (input_hash, files) = inputs(root)?;
    match command.as_str() {
        "inputs" => {
            ensure!(
                values.is_empty() && !push && !plan,
                "inputs takes no options"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"tag":tag,"commit":commit,"input_sha256":input_hash,"image_tag":format!("{IMAGE}:{tag}-{}",&input_hash[..12]),"python":"3.12.11","scripts_parsed_by":"rustpython-parser (no Python execution)"})
                )?
            );
        }
        "--check" => {
            ensure!(
                !plan && !push && values.keys().all(|key| key == "--out"),
                "check is offline and takes no execution options"
            );
            let dir = values
                .get("--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("vendor/oracle"));
            check(root, &dir)?;
        }
        "image" => {
            ensure!(values.is_empty(), "image takes only --plan and --push");
            let context = task_path("oracle-image-context")?;
            let metadata = task_path("oracle-image-metadata.json")?;
            let image = format!("{IMAGE}:{tag}-{}", &input_hash[..12]);
            let mut command = vec![
                "buildx".into(),
                "build".into(),
                "--platform".into(),
                "linux/amd64".into(),
                if push {
                    "--push".into()
                } else {
                    "--load".into()
                },
                "--tag".into(),
                image,
                "--build-arg".into(),
                format!("PARITY_COMMIT={commit}"),
                "--build-arg".into(),
                format!("PARITY_TAG={tag}"),
                "--build-arg".into(),
                format!("ORACLE_INPUT_SHA256={input_hash}"),
                "--metadata-file".into(),
                metadata.display().to_string(),
            ];
            command.extend([
                "--cache-from".into(),
                format!("type=registry,ref={IMAGE}:{tag}"),
            ]);
            if push {
                command.extend([
                    "--tag".into(),
                    format!("{IMAGE}:{tag}"),
                    "--cache-to".into(),
                    "type=inline".into(),
                ]);
            }
            command.push(context.display().to_string());
            if plan {
                println!("{}", serde_json::to_string_pretty(&command)?);
            } else {
                require_lane("oracle")?;
                ensure!(!context.exists(), "oracle context already exists");
                fs::create_dir(&context)?;
                for (name, bytes) in files {
                    fs::write(context.join(name), bytes)?;
                }
                execute(&command)?;
            }
        }
        "dump" => {
            ensure!(!push, "dump cannot push an image");
            let image = match (values.get("--image"), values.get("--built-image-id")) {
                (Some(image), None) => {
                    pinned_image(image)?;
                    image
                }
                (None, Some(image)) => {
                    ensure!(
                        image.strip_prefix("sha256:").is_some_and(|hash| {
                            hash.len() == 64
                                && hash.bytes().all(|byte| {
                                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                                })
                        }),
                        "built image must be an immutable Docker image ID"
                    );
                    image
                }
                _ => bail!(
                    "dump needs exactly one --image GHCR digest or --built-image-id from this hosted build"
                ),
            };
            let scope = values.get("--scope").map(String::as_str).unwrap_or("M2");
            ensure!(scope == "M2" || scope == "all", "scope must be M2 or all");
            let out = values
                .get("--out")
                .map(PathBuf::from)
                .unwrap_or(task_path("oracle-output")?);
            let command = vec![
                "run".into(),
                "--rm".into(),
                "--network".into(),
                "none".into(),
                "--env".into(),
                "GITHUB_ACTIONS=true".into(),
                "--env".into(),
                "RTAO_PYTHON_LANE=oracle".into(),
                "--env".into(),
                format!("RTAO_ORACLE_EXECUTION_IMAGE={image}"),
                "--mount".into(),
                format!(
                    "type=bind,src={},dst=/inputs,readonly",
                    root.join("vendor/oracle").display()
                ),
                "--mount".into(),
                format!("type=bind,src={},dst=/output", out.display()),
                image.clone(),
                "--scope".into(),
                scope.into(),
            ];
            if plan {
                println!("{}", serde_json::to_string_pretty(&command)?);
            } else {
                require_lane("oracle")?;
                if values.contains_key("--image") {
                    execute(&["pull".into(), image.clone()])?;
                }
                let inspected = Command::new("docker")
                    .args(["image", "inspect", image])
                    .output()?;
                ensure!(
                    inspected.status.success(),
                    "cannot inspect the oracle execution image"
                );
                let inspected: Value = serde_json::from_slice(&inspected.stdout)?;
                let labels = &inspected[0]["Config"]["Labels"];
                ensure!(
                    text(labels, "org.opencontainers.image.revision")? == commit
                        && text(labels, "io.rtao.oracle.parity-tag")? == tag
                        && text(labels, "io.rtao.oracle.input-sha256")? == input_hash,
                    "execution image labels do not match the committed oracle inputs"
                );
                new_task_output(&out)?;
                execute(&command)?;
                check(root, &out)?;
            }
        }
        _ => bail!("unknown oracle command {command:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_read_table_runtime_seeds_encode_without_python() -> Result<()> {
        fn literal(expression: &ast::Expr, suite: &ast::Suite) -> Result<Value> {
            Ok(match expression {
                ast::Expr::Constant(value) => match &value.value {
                    ast::Constant::None => Value::Null,
                    ast::Constant::Str(value) => json!(value),
                    ast::Constant::Int(value) => serde_json::from_str(&value.to_string())?,
                    _ => bail!("unsupported seed literal"),
                },
                ast::Expr::List(values) => Value::Array(
                    values
                        .elts
                        .iter()
                        .map(|value| literal(value, suite))
                        .collect::<Result<_>>()?,
                ),
                ast::Expr::Dict(values) => {
                    let mut result = serde_json::Map::new();
                    for (key, value) in values.keys.iter().zip(&values.values) {
                        let key = literal(key.as_ref().context("expanded seed dict")?, suite)?;
                        result.insert(
                            key.as_str().context("seed dict string key")?.to_owned(),
                            literal(value, suite)?,
                        );
                    }
                    Value::Object(result)
                }
                ast::Expr::Name(name) => {
                    let value=suite.iter().find_map(|statement|match statement {
                        ast::Stmt::Assign(assign) if assign.targets.iter().any(|target|matches!(target,ast::Expr::Name(target) if target.id==name.id))=>Some(assign.value.as_ref()),_=>None,
                    }).context("read-table seed named constant")?;
                    literal(value, suite)?
                }
                ast::Expr::BinOp(operation) if matches!(operation.op, ast::Operator::Pow) => {
                    let base = literal(&operation.left, suite)?
                        .as_u64()
                        .context("seed power base")?;
                    let exponent = u32::try_from(
                        literal(&operation.right, suite)?
                            .as_u64()
                            .context("seed power exponent")?,
                    )?;
                    ensure!(exponent <= 32, "seed power exponent limit");
                    json!(base.checked_pow(exponent).context("seed power overflow")?)
                }
                _ => bail!("read-table runtime seed changed its literal form"),
            })
        }
        let path = "sdk/python/tests/unit/test_reads_table.py";
        let source = fs::read_to_string(root()?.join("vendor/upstream/src").join(path))?;
        let suite = ast::Suite::parse(&source, path)?;
        let function = suite
            .iter()
            .find_map(|statement| match statement {
                ast::Stmt::FunctionDef(function)
                    if function.name.as_str() == "seeded_substrate" =>
                {
                    Some(function)
                }
                _ => None,
            })
            .context("upstream seeded_substrate function")?;
        let cases = function
            .body
            .iter()
            .find_map(|statement| match statement {
                ast::Stmt::For(statement) => Some(statement.iter.as_ref()),
                _ => None,
            })
            .context("upstream runtime seed loop")?;
        let ast::Expr::List(cases) = cases else {
            bail!("upstream seed loop must be literal")
        };
        let mut rows = Vec::new();
        for case in &cases.elts {
            let ast::Expr::Tuple(case) = case else {
                bail!("runtime seed tuple")
            };
            ensure!(case.elts.len() == 3, "runtime seed tuple arity");
            rows.push(json!({"container":literal(&case.elts[0],&suite)?,"name":literal(&case.elts[1],&suite)?,"value":literal(&case.elts[2],&suite)?}));
        }
        ensure!(rows.len() >= 24, "upstream read table seed coverage shrank");
        replay::seeded_state(
            &json!({"storage":[],"storage_defaults":[],"storage_maps":[],"constants":[],"runtime":rows,"spec_version":300,"block":100,"finalized":100,"fee_rao":124414,"weight":{"ref_time":1000000,"proof_size":3593},"token_symbols":{},"events":{},"nonces":{}}),
            &json!({"name":"metagraph","params":{"netuid":1}}),
        )?;
        Ok(())
    }

    #[test]
    fn oracle_sources_parse_without_running_python() -> Result<()> {
        let (hash, files) = inputs(root()?)?;
        assert_eq!(hash.len(), 64);
        assert_eq!(files.len(), 14);
        Ok(())
    }
    #[test]
    fn rejects_unpinned_images_and_unsafe_paths() {
        for image in [
            "python:latest",
            "ghcr.io/sam0x17/rtao-oracle:v478",
            "ghcr.io/other/oracle@sha256:aaa",
        ] {
            assert!(pinned_image(image).is_err());
        }
        assert!(pinned_image(&format!("{IMAGE}@sha256:{}", "a".repeat(64))).is_ok());
        for path in ["../key", "/abs", "a/../key", "a//b", "a\\b"] {
            assert!(relative(path).is_err());
        }
        assert!(relative("pure/balance.json").is_ok());
    }
    #[test]
    fn local_execution_fails_before_docker() {
        if std::env::consts::OS != "linux" {
            assert!(require_lane("oracle").is_err());
            assert!(require_lane("record").is_err());
        }
    }

    #[test]
    fn committed_vectors_replay_without_python() -> Result<()> {
        let root = root()?;
        let status = check_committed(root, false, None)?;
        if root.join("vendor/oracle/manifest.json").exists() {
            assert_eq!(status, readiness::CommittedStatus::Verified);
        } else {
            assert_eq!(status, readiness::CommittedStatus::InitialPending);
        }
        println!("{}", status.report());
        Ok(())
    }
}
