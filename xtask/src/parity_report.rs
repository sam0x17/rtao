//! Offline pin-bump reports, with conservative evidence for cosmetic reconciliation.

use anyhow::{Context, Result, ensure};
use rustpython_parser::{Mode, lexer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, value};

fn normalize(value: &mut Value) {
    match value {
        Value::Object(values) => {
            let metadata = values.get("provenance").is_some_and(|row| {
                row["path"].is_string()
                    && (row["blob"].as_str().is_some_and(super::valid_sha)
                        || row["sha256"].as_str().is_some_and(|sha| {
                            sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                        }))
            });
            if metadata {
                values.remove("provenance");
            }
            for (key, value) in values.iter_mut() {
                if !matches!(
                    key.as_str(),
                    "value"
                        | "schema"
                        | "input_schema"
                        | "output_schema"
                        | "properties"
                        | "params"
                        | "args"
                        | "intent"
                        | "plan"
                ) {
                    normalize(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize(value);
            }
        }
        _ => {}
    }
}
fn json_file(root: &Path, path: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(root.join(path))?)?)
}
fn extracted(root: &Path) -> Result<Value> {
    let directory = root.join("vendor/upstream/extracted");
    let mut output = serde_json::Map::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        ensure!(
            entry.file_type()?.is_file(),
            "extract report input must be a flat regular file"
        );
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let bytes = fs::read(&path)?;
            ensure!(
                bytes.len() <= 64 * 1024 * 1024,
                "extracted report input exceeds bound"
            );
            let mut value: Value = serde_json::from_slice(&bytes)?;
            normalize(&mut value);
            output.insert(entry.file_name().to_string_lossy().into_owned(), value);
        }
    }
    ensure!(!output.is_empty(), "extracted report proof absent");
    Ok(Value::Object(output))
}
fn oracle(root: &Path) -> Result<Value> {
    let directory = root.join("vendor/oracle");
    crate::oracle::check(root, &directory)?;
    let manifest = json_file(root, "vendor/oracle/manifest.json")?;
    let mut output = serde_json::Map::new();
    for (path, _) in manifest["files"]
        .as_object()
        .context("oracle manifest inventory absent")?
    {
        if path == "image.json" {
            continue;
        }
        ensure!(super::valid_path(path), "unsafe oracle report path");
        let mut value: Value = serde_json::from_slice(&fs::read(directory.join(path))?)?;
        if let Some(vectors) = value.get("vectors") {
            value = vectors.clone();
        }
        output.insert(path.clone(), value);
    }
    ensure!(!output.is_empty(), "oracle comparison evidence absent");
    Ok(Value::Object(output))
}
fn unchanged(left: Result<Value>, right: Result<Value>) -> (bool, Option<String>) {
    match (left, right) {
        (Ok(left), Ok(right)) => match rtao_primitives::json::compare(&left, &right) {
            Ok(()) => (true, None),
            Err(error) => (false, Some(format!("behavior differs: {error:#}"))),
        },
        (Err(error), _) | (_, Err(error)) => (false, Some(format!("proof unavailable: {error:#}"))),
    }
}
fn source(root: &Path, path: &str, blob: &str) -> Result<String> {
    let bytes = fs::read(root.join("vendor/upstream/src").join(path))?;
    ensure!(
        crate::extract::git_blob(&bytes) == blob,
        "report source differs from authenticated tree: {path}"
    );
    String::from_utf8(bytes).context("report source not UTF-8")
}
fn normalized_catalog(root: &Path, path: &str) -> Result<Value> {
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let artifact = &pins["artifacts"][path];
    let destination = artifact["to"]
        .as_str()
        .context("catalog report artifact destination absent")?;
    ensure!(
        super::valid_path(destination),
        "unsafe catalog report destination"
    );
    let bytes = fs::read(root.join(destination))?;
    ensure!(
        artifact["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
        "catalog report input differs from pin"
    );
    let mut value: Value = serde_json::from_slice(&bytes)?;
    normalize_catalog(&mut value);
    Ok(value)
}
fn body_symbols(root: &Path, path: &str, blob: &str) -> Result<Value> {
    let source = source(root, path, blob)?;
    semantic_python(path, &source)
}
fn semantic_python(path: &str, source: &str) -> Result<Value> {
    let symbols = super::python_symbols(path, source)?
        .into_iter()
        .map(|(name, row)| (name, row["tokens"].clone()))
        .collect::<serde_json::Map<_, _>>();
    let tokens = lexer::lex(source, Mode::Module)
        .map(|token| {
            token
                .map(|(token, _)| format!("{token:?}"))
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"symbols":symbols,"module_tokens":tokens}))
}

fn normalize_catalog(value: &mut Value) {
    match value {
        Value::Object(values) => {
            if let Some(Value::Array(sources)) = values.get_mut("sources") {
                for source in sources {
                    if let Value::Object(source) = source {
                        for key in ["line", "end_line", "url"] {
                            source.remove(key);
                        }
                    }
                }
            }
            for (key, value) in values.iter_mut() {
                if !matches!(
                    key.as_str(),
                    "schema" | "input_schema" | "output_schema" | "properties"
                ) {
                    normalize_catalog(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_catalog(value);
            }
        }
        _ => {}
    }
}

fn names(root: &Path, path: &str) -> Result<BTreeSet<String>> {
    let catalog = normalized_catalog(root, path)?;
    catalog
        .as_array()
        .context("operation catalog absent")?
        .iter()
        .map(|row| {
            row["name"]
                .as_str()
                .map(str::to_owned)
                .context("catalog operation name missing")
        })
        .collect()
}
fn reconcile_pending(
    old: &BTreeSet<String>,
    current: &BTreeSet<String>,
    pending: &Item,
) -> Result<(BTreeSet<String>, Value)> {
    let pending = pending
        .as_array()
        .context("pending operation array absent")?
        .iter()
        .map(|name| {
            name.as_str()
                .map(str::to_owned)
                .context("pending name missing")
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let added = current.difference(old).cloned().collect::<BTreeSet<_>>();
    let removed = old.difference(current).cloned().collect::<BTreeSet<_>>();
    let merged = pending
        .intersection(current)
        .cloned()
        .chain(added.iter().cloned())
        .collect::<BTreeSet<_>>();
    Ok((
        merged,
        json!({"added":added,"removed":removed,"pending_before":pending.len()}),
    ))
}

pub(super) fn run(root: &Path, args: &[String]) -> Result<()> {
    let mut base = None;
    let mut out = None;
    let mut autobless = false;
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--base" if base.is_none() => {
                base = Some(PathBuf::from(
                    args.next().context("report --base needs workspace")?,
                ))
            }
            "--out" if out.is_none() => {
                out = Some(PathBuf::from(
                    args.next().context("report --out needs JSON path")?,
                ))
            }
            "--autobless" if !autobless => autobless = true,
            _ => anyhow::bail!(
                "usage: cargo xtask parity report --base <workspace> --out <json> [--autobless]"
            ),
        }
    }
    let base = base.context("report requires --base workspace")?;
    ensure!(
        base.join("upstream.toml").is_file(),
        "report baseline workspace missing"
    );
    ensure!(
        base.canonicalize()? != root.canonicalize()?,
        "report baseline must differ from candidate workspace"
    );
    let out = out.context("report requires --out")?;
    let before = super::read_tree(&fs::read_to_string(base.join("vendor/upstream/tree.txt"))?)?;
    let after = super::read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    let contents = fs::read_to_string(root.join("parity/map.toml"))?;
    let mut map = contents.parse::<DocumentMut>()?;
    let mapped = super::read_map(&contents)?;
    let index = super::symbols::index(root)?;
    let mut pending =
        fs::read_to_string(root.join("parity/pending.toml"))?.parse::<DocumentMut>()?;
    let mut catalog_deltas = serde_json::Map::new();
    for family in ["intents", "reads"] {
        let path = format!("website/apps/bittensor-website/public/catalog/{family}.json");
        let before = names(&base, &path)?;
        let after = names(root, &path)?;
        let (merged, mut delta) = reconcile_pending(&before, &after, &pending[family])?;
        if autobless && before != after {
            let old = fs::read_to_string(base.join("upstream.toml"))?.parse::<DocumentMut>()?;
            let new = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
            ensure!(
                old["parity"]["commit"].as_str() != new["parity"]["commit"].as_str(),
                "new or vanished catalog operations require a parity pin bump"
            );
            let mut array = toml_edit::Array::new();
            for name in &merged {
                array.push(name.as_str());
            }
            pending[family] = value(array);
        }
        delta["pending_after"] = json!(if autobless {
            merged.len()
        } else {
            pending[family]
                .as_array()
                .context("pending family absent")?
                .len()
        });
        catalog_deltas.insert(family.into(), delta);
    }
    let (extraction_unchanged, extraction_error) = unchanged(extracted(&base), extracted(root));
    let (oracle_unchanged, oracle_error) = unchanged(oracle(&base), oracle(root));
    let mut changes = Vec::new();
    let mut blessed = BTreeSet::new();
    for path in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        if before.get(path) == after.get(path) {
            continue;
        }
        let row = mapped.iter().find(|row| &row.py == path);
        let (symbols_unchanged, symbol_error) = if path.ends_with(".py") {
            match (before.get(path), after.get(path)) {
                (Some(old), Some(new)) => unchanged(
                    body_symbols(&base, path, old),
                    body_symbols(root, path, new),
                ),
                _ => (false, Some("added or removed Python module".into())),
            }
        } else if path.starts_with("website/") && path.ends_with(".json") {
            unchanged(
                normalized_catalog(&base, path),
                normalized_catalog(root, path),
            )
        } else {
            (
                false,
                Some("non-Python change needs explicit consumer test evidence".into()),
            )
        };
        let eligible = row
            .is_some_and(|row| row.synced_blob != "pending" && row.kind != "vendored")
            && after.contains_key(path)
            && symbols_unchanged
            && extraction_unchanged
            && oracle_unchanged;
        if autobless && eligible {
            blessed.insert(path.clone());
        }
        let symbol_changes = if path.ends_with(".py") {
            match(before.get(path),after.get(path)){
                (Some(old),Some(new))=>match(super::python_symbols(path,&source(&base,path,old)?),super::python_symbols(path,&source(root,path,new)?)){
                    (Ok(old),Ok(new))=>old.keys().chain(new.keys()).collect::<BTreeSet<_>>().into_iter().filter(|name|old.get(*name).map(|row|&row["tokens"])!=new.get(*name).map(|row|&row["tokens"])).map(|name|json!({"python":name,"rust":index.aliases.get(name).map(|locations|locations.iter().map(|location|json!({"path":location.path,"line":location.line,"item":location.item})).collect::<Vec<_>>())})).collect::<Vec<_>>(),
                    _=>Vec::new()
                },_=>Vec::new()
            }
        } else {
            Vec::new()
        };
        changes.push(json!({"path":path,"py":path,"old_blob":before.get(path),"new_blob":after.get(path),"rust":row.map(|row|&row.rs),"symbols":symbol_changes,"symbols_unchanged":symbols_unchanged,"symbol_error":symbol_error,"extraction_unchanged":extraction_unchanged,"oracle_unchanged":oracle_unchanged,"auto_bless_candidate":eligible,"auto_blessed":autobless&&eligible}));
    }
    if !blessed.is_empty() {
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let tag = super::required_string(&pins["parity"], "tag")?;
        for row in map
            .get_mut("file")
            .and_then(Item::as_array_of_tables_mut)
            .context("report file rows absent")?
            .iter_mut()
        {
            let path = row["py"].as_str().context("report row path absent")?;
            if blessed.contains(path) {
                row["synced_blob"] = value(after[path].clone());
                row["synced_tag"] = value(tag);
            }
        }
        let proposed = super::read_map(&map.to_string())?;
        super::check_files(&proposed, &after, false)?;
        super::check_licenses(root, &proposed)?;
        fs::write(root.join("parity/map.toml"), map.to_string())?;
    }
    if autobless {
        fs::write(root.join("parity/pending.toml"), pending.to_string())?;
    }
    let report = json!({"schema":1,"changed":changes.len(),"changes":changes,"extraction":{"unchanged":extraction_unchanged,"error":extraction_error},"oracle":{"unchanged":oracle_unchanged,"error":oracle_error},"auto_blessed":blessed,"catalog_deltas":catalog_deltas,"pending_intents":pending["intents"].as_array().context("pending intents absent")?.len(),"pending_reads":pending["reads"].as_array().context("pending reads absent")?.len(),"proof_policy":"Only previously reconciled non-vendored files qualify; unchanged symbols, complete extracted behavior and authenticated passing oracle evidence are required. This report never marks a pending file implemented."});
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    let mut markdown = format!(
        "# Parity porting report\n\n{} watched files changed. Pending behavior: {} intents, {} reads. Automatically reconciled {} files.\n\n| Python or chain path | Status | Rust home |\n|---|---|---|\n",
        report["changed"],
        report["pending_intents"],
        report["pending_reads"],
        blessed.len()
    );
    for change in report["changes"]
        .as_array()
        .context("report changes absent")?
    {
        let path = change["path"]
            .as_str()
            .context("report path absent")?
            .replace('|', "\\|");
        markdown.push_str(&format!(
            "| `{path}` | {} | {} |\n",
            if change["auto_blessed"].as_bool() == Some(true) {
                "automatically reconciled"
            } else {
                "review required"
            },
            change["rust"].to_string().replace('|', "\\|")
        ));
    }
    markdown.push_str(&format!("\nCatalog changes and pending deltas:\n\n```json\n{}\n```\n\nExtraction proof: {}. Oracle proof: {}. Missing evidence prevents automatic reconciliation.\n",serde_json::to_string_pretty(&report["catalog_deltas"])?,report["extraction"],report["oracle"]));
    fs::write(out.with_extension("md"), markdown)?;
    println!(
        "parity: {} changed files; report {}",
        report["changed"],
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cosmetic_python_fingerprint_ignores_locations_and_detects_unindexed_effects() -> Result<()> {
        let path = "sdk/python/bittensor/example.py";
        let baseline = semantic_python(path, "VALUE = 1\ndef f(x=2):\n    return x\n")?;
        let cosmetic = semantic_python(
            path,
            "# comment\n\nVALUE = 1\n\ndef f(x=2):\n    return x\n",
        )?;
        rtao_primitives::json::compare(&baseline, &cosmetic)?;
        for source in [
            "VALUE = 2\ndef f(x=2):\n    return x\n",
            "VALUE = 1\ndef f(x=3):\n    return x\n",
            "VALUE = 1\ndef f(x=2):\n    return x\nf()\n",
        ] {
            assert!(
                rtao_primitives::json::compare(&baseline, &semantic_python(path, source)?).is_err()
            );
        }
        Ok(())
    }
    #[test]
    fn catalog_normalization_removes_only_source_locations() {
        let mut baseline = json!({"schema":{"properties":{"line":{"type":"integer"},"url":{"type":"string"}}},"docs_url":"https://docs","sources":[{"path":"x","line":1,"end_line":2,"url":"https://old"}]});
        let mut cosmetic = baseline.clone();
        cosmetic["sources"][0]["line"] = json!(99);
        cosmetic["sources"][0]["url"] = json!("https://new");
        normalize_catalog(&mut baseline);
        normalize_catalog(&mut cosmetic);
        assert_eq!(baseline, cosmetic);
        assert!(baseline["schema"]["properties"].get("line").is_some());
        assert!(baseline["schema"]["properties"].get("url").is_some());
        assert_eq!(baseline["docs_url"], json!("https://docs"));
        cosmetic["schema"]["properties"]["line"]["type"] = json!("number");
        assert_ne!(baseline, cosmetic);
    }
    #[test]
    fn behavioral_schema_and_oracle_keys_survive_normalization() -> Result<()> {
        let value = json!({"input_schema":{"properties":{"url":{"type":"string"},"provenance":{"path":"behavior","blob":"a".repeat(40)}}},"vectors":[{"tag":"tag-a","commit":"commit-a","blob":"blob-a","url":"https://behavior","args":{"provenance":{"path":"value","blob":"a".repeat(40)}}}]});
        let mut normalized = value.clone();
        normalize(&mut normalized);
        normalize_catalog(&mut normalized);
        rtao_primitives::json::compare(&normalized, &value)?;
        for key in ["url", "tag", "commit", "blob"] {
            let mut changed = value.clone();
            changed["vectors"][0][key] = json!("changed");
            normalize(&mut changed);
            assert!(rtao_primitives::json::compare(&normalized, &changed).is_err());
        }
        Ok(())
    }
    #[test]
    fn pending_delta_adds_only_new_operations_and_preserves_implemented_existing() -> Result<()> {
        let old = BTreeSet::from(["implemented".into(), "pending".into(), "vanished".into()]);
        let current = BTreeSet::from(["implemented".into(), "pending".into(), "new".into()]);
        let document = "intents=['pending','vanished']".parse::<DocumentMut>()?;
        let (pending, delta) = reconcile_pending(&old, &current, &document["intents"])?;
        assert_eq!(pending, BTreeSet::from(["pending".into(), "new".into()]));
        assert_eq!(delta["added"], json!(["new"]));
        assert_eq!(delta["removed"], json!(["vanished"]));
        Ok(())
    }
}
