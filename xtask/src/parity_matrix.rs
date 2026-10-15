//! Deterministic inventory of declared parity status and recorded oracle evidence.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};
use toml_edit::{DocumentMut, Item};

use super::{Row, symbols::Index};

fn document(root: &Path, path: &str) -> Result<DocumentMut> {
    Ok(fs::read_to_string(root.join(path))?.parse()?)
}
fn array(document: &DocumentMut, key: &str) -> Result<BTreeSet<String>> {
    document[key]
        .as_array()
        .context("matrix pending array absent")?
        .iter()
        .map(|name| {
            name.as_str()
                .map(str::to_owned)
                .context("matrix pending name invalid")
        })
        .collect()
}
fn records(root: &Path, path: &str) -> Result<BTreeSet<String>> {
    let path = root.join(path);
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let json: Value = serde_json::from_slice(&fs::read(path)?)?;
    json["vectors"]
        .as_array()
        .context("oracle matrix vectors absent")?
        .iter()
        .map(|row| {
            row["name"]
                .as_str()
                .map(str::to_owned)
                .context("oracle vector name absent")
        })
        .collect()
}
fn status(recorded: &BTreeSet<String>, name: &str) -> &'static str {
    if recorded.contains(name) {
        "recorded"
    } else {
        "missing"
    }
}

pub(super) fn generate(root: &Path, files: &[Row], index: &Index) -> Result<Value> {
    let pins = document(root, "upstream.toml")?;
    let pending = document(root, "parity/pending.toml")?;
    let pending_intents = array(&pending, "intents")?;
    let pending_reads = array(&pending, "reads")?;
    let recorded_intents = records(root, "vendor/oracle/intents.json")?;
    let recorded_reads = records(root, "vendor/oracle/reads.json")?;
    let file_rows=files.iter().map(|row|json!({"python":row.py,"rust":row.rs,"kind":row.kind,"rtao":if row.synced_blob=="pending" {"pending"} else {"reconciled"},"synced_blob":row.synced_blob,"milestone":row.milestone,"oracle":if row.kind=="n/a" {"not_required"} else {"unverified"}})).collect::<Vec<_>>();
    let intents=rtao_catalog::INTENTS.iter().map(|spec|json!({"name":spec.name,"python_class":spec.python_class,"rtao":if pending_intents.contains(spec.name) {"pending"} else {"implemented"},"oracle":status(&recorded_intents,spec.name)})).collect::<Vec<_>>();
    let reads=rtao_catalog::READS.iter().map(|spec|json!({"name":spec.name,"rtao":if pending_reads.contains(spec.name) {"pending"} else {"implemented"},"oracle":status(&recorded_reads,spec.name)})).collect::<Vec<_>>();
    let discovered = crate::extract::public_exports(root)?;
    let decisions = document(root, "parity/exports_extra.toml")?;
    let rows = decisions["export"]
        .as_array_of_tables()
        .context("matrix exports decisions absent")?;
    let mut exports = Vec::new();
    for (path, names) in discovered {
        let module = super::symbols::module(&path)?;
        for name in names {
            let symbol = format!("{module}.{name}");
            let decision = rows
                .iter()
                .find(|row| row.get("python").and_then(Item::as_str) == Some(symbol.as_str()));
            let rtao = if index.aliases.contains_key(&symbol) {
                "aliased"
            } else if decision.is_some_and(|row| row.get("pending").is_some()) {
                "pending"
            } else if decision.is_some() {
                "decision"
            } else {
                "module_map"
            };
            exports.push(json!({"python":symbol,"source":path,"rtao":rtao,"oracle":"unverified"}));
        }
    }
    let divergences=document(root,"parity/divergences.toml")?["divergences"].as_array_of_tables().context("matrix divergence register absent")?.iter().map(|row|json!({"id":row["id"].as_str(),"rtao":row.get("status").and_then(Item::as_str).unwrap_or("active"),"tests":row.get("pinning_tests").and_then(Item::as_array).map(|tests|tests.iter().filter_map(|test|test.as_str()).collect::<Vec<_>>()),"oracle":"unverified"})).collect::<Vec<_>>();
    let env = document(root, "parity/env.toml")?;
    let environment=env["env"].as_table_like().context("matrix environment rows absent")?.iter().map(|(name,row)|json!({"name":name,"rtao":if row.get("consumer").is_some(){"implemented"}else if row.get("class").and_then(Item::as_str)==Some("pending"){"pending"}else{"divergence"},"consumer":row.get("consumer").and_then(Item::as_str),"divergence":row.get("divergence").and_then(Item::as_str),"oracle":"unverified"})).collect::<Vec<_>>();
    let e2e = document(root, "parity/e2e_map.toml")?;
    let chain = document(root, "parity/chain_pending.toml")?;
    let chain_assertions=chain["assertion"].as_table_like().context("matrix chain assertion register absent")?.iter().map(|(name,row)|json!({"name":name,"rtao":"pending","milestone":row.get("milestone").and_then(Item::as_str),"plan":row.get("plan").and_then(Item::as_str),"oracle":"unverified"})).collect::<Vec<_>>();
    let e2e=e2e["tests"].as_table_like().context("matrix e2e rows absent")?.iter().map(|(name,row)|json!({"name":name,"rtao":if row.get("rtao").is_some(){"mapped"}else if row.get("na").is_some(){"not_required"}else{"pending"},"test":row.get("rtao").and_then(Item::as_str),"oracle":"unverified"})).collect::<Vec<_>>();
    Ok(
        json!({"schema":1,"pin":{"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"status_semantics":{"rtao":"Declared implementation status; parity check independently validates evidence. Aliases/module-map decisions do not certify behavior.","oracle":"recorded means a named immutable vector exists; this inventory does not claim a successful replay."},"families":{"files":file_rows,"intents":intents,"reads":reads,"semantic_errors":rtao_catalog::SEMANTIC_ERRORS.iter().map(|spec|json!({"name":spec.code.as_str(),"rtao":"generated","oracle":"unverified"})).collect::<Vec<_>>(),"chain_errors":rtao_catalog::CHAIN_ERRORS.iter().map(|spec|json!({"name":spec.name,"rtao":"generated","oracle":"unverified"})).collect::<Vec<_>>(),"exports":exports,"divergences":divergences,"environment":environment,"e2e":e2e,"chain_assertions":chain_assertions}}),
    )
}

fn bytes(value: &Value) -> Result<Vec<u8>> {
    let mut value = value.clone();
    value.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
pub(super) fn run(root: &Path, index: &Index, check: bool) -> Result<Value> {
    let files = super::read_map(&fs::read_to_string(root.join("parity/map.toml"))?)?;
    let matrix = generate(root, &files, index)?;
    let bytes = bytes(&matrix)?;
    let path = root.join("parity/matrix.json");
    if check {
        ensure!(
            fs::read(&path).context("parity matrix missing; run cargo xtask parity matrix")?
                == bytes,
            "parity matrix stale; run cargo xtask parity matrix"
        );
    } else {
        fs::write(path, bytes)?;
    }
    Ok(
        json!({"families":matrix["families"].as_object().context("matrix families absent")?.len(),"rows":matrix["families"].as_object().context("matrix families absent")?.values().filter_map(Value::as_array).map(Vec::len).sum::<usize>()}),
    )
}
