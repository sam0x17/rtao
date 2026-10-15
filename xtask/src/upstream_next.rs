//! Read-only previews of watched Python/catalog/core changes on upstream branches.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn normalize(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize),
        Value::Object(object) => {
            if let Some(sources) = object.get_mut("sources").and_then(Value::as_array_mut) {
                for source in sources {
                    if let Some(source) = source.as_object_mut() {
                        for key in ["line", "end_line", "url"] {
                            source.remove(key);
                        }
                    }
                }
            }
            object.values_mut().for_each(normalize);
        }
        _ => {}
    }
}

fn tree(repo: &Path, commit: &str) -> Result<BTreeMap<String, String>> {
    let mut args = vec!["ls-tree", "-r", commit, "--"];
    args.extend_from_slice(crate::import_upstream::WATCHED_ROOTS);
    let bytes = crate::pin::git(repo, &args)?;
    let mut entries = BTreeMap::new();
    for line in std::str::from_utf8(&bytes)?.lines() {
        let (identity, path) = line
            .split_once('\t')
            .context("malformed upstream tree row")?;
        let fields: Vec<_> = identity.split_whitespace().collect();
        ensure!(
            fields.len() == 3 && fields[1] == "blob",
            "watched source is not a Git blob"
        );
        entries.insert(path.to_owned(), fields[2].to_owned());
    }
    Ok(entries)
}

fn preview(root: &Path, branch: &str) -> Result<Value> {
    ensure!(
        ["main", "testnet"].contains(&branch),
        "upstream-next branch must be main or testnet"
    );
    let repo = crate::pin::cached_repo(root)?;
    let reference = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
    crate::pin::git(
        &repo,
        &["fetch", "--filter=blob:none", "origin", &reference],
    )?;
    let next = String::from_utf8(crate::pin::git(
        &repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/origin/{branch}^{{commit}}"),
        ],
    )?)?
    .trim()
    .to_owned();
    ensure!(
        next.len() == 40 && next.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "branch did not resolve to an immutable commit"
    );
    let pins = crate::pin::read_document(&root.join("upstream.toml"))?;
    let previous = pins["parity"]["commit"]
        .as_str()
        .context("parity commit missing")?;
    let old = tree(&repo, previous)?;
    let new = tree(&repo, &next)?;
    let map = crate::pin::read_document(&root.join("parity/map.toml"))?;
    let mapped: BTreeMap<_, _> = map["file"]
        .as_array_of_tables()
        .context("parity files missing")?
        .iter()
        .map(|row| {
            let path = row["py"].as_str().context("parity row path missing")?;
            let rust: Vec<_> = row["rs"]
                .as_array()
                .context("parity Rust paths missing")?
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .map(str::to_owned)
                .collect();
            Ok((path.to_owned(), rust))
        })
        .collect::<Result<_>>()?;
    let paths: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    let mut changed = Vec::new();
    for path in paths
        .into_iter()
        .filter(|path| old.get(*path) != new.get(*path))
    {
        let mut symbols = Vec::new();
        if path.ends_with(".py") && path.starts_with("sdk/python/") {
            let parse = |blob: Option<&String>| -> Result<BTreeMap<String, Value>> {
                if let Some(blob) = blob {
                    let source =
                        String::from_utf8(crate::pin::git(&repo, &["cat-file", "blob", blob])?)?;
                    crate::parity::python_symbols(path, &source)
                } else {
                    Ok(BTreeMap::new())
                }
            };
            let old_symbols = parse(old.get(path))?;
            let new_symbols = parse(new.get(path))?;
            for symbol in old_symbols
                .keys()
                .chain(new_symbols.keys())
                .collect::<BTreeSet<_>>()
            {
                if old_symbols.get(symbol).map(|value| &value["tokens"])
                    != new_symbols.get(symbol).map(|value| &value["tokens"])
                {
                    let status = match (
                        old_symbols.contains_key(symbol),
                        new_symbols.contains_key(symbol),
                    ) {
                        (false, true) => "added",
                        (true, false) => "removed",
                        _ => "changed",
                    };
                    symbols.push(json!({"python":symbol,"status":status,
                        "old_line":old_symbols.get(symbol).map(|value|&value["line"]),
                        "new_line":new_symbols.get(symbol).map(|value|&value["line"])}));
                }
            }
        }
        changed.push(json!({"py":path,"old_blob":old.get(path),"new_blob":new.get(path),"rust":mapped.get(path).cloned().unwrap_or_default(),"symbols":symbols}));
    }
    let mut catalogs = Vec::new();
    for catalog in ["intents", "reads", "errors"] {
        let path = format!("website/apps/bittensor-website/public/catalog/{catalog}.json");
        let parse = |revision: &str| -> Result<Value> {
            let mut value: Value = serde_json::from_slice(&crate::pin::git(
                &repo,
                &["show", &format!("{revision}:{path}")],
            )?)?;
            normalize(&mut value);
            Ok(value)
        };
        let old_catalog = parse(previous)?;
        let new_catalog = parse(&next)?;
        let ids = |value: &Value| -> BTreeSet<String> {
            value
                .as_array()
                .or_else(|| value.get("entries").and_then(Value::as_array))
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["name"].as_str().or_else(|| entry["id"].as_str()))
                .map(str::to_owned)
                .collect()
        };
        let old_ids = ids(&old_catalog);
        let new_ids = ids(&new_catalog);
        catalogs.push(json!({"catalog":catalog,"semantic_change":old_catalog!=new_catalog,"added":new_ids.difference(&old_ids).collect::<Vec<_>>(),"removed":old_ids.difference(&new_ids).collect::<Vec<_>>() }));
    }
    crate::fingerprint::prepare_source_cache(&repo, &next)?;
    let fingerprint = crate::fingerprint::fingerprint(&repo, &next)?;
    let core_changed = pins["core"]["fingerprint"].as_str() != Some(fingerprint.as_str());
    let diff = String::from_utf8(crate::pin::git(
        &repo,
        &[
            "diff",
            "--stat",
            previous,
            &next,
            "--",
            "sdk/python",
            "sdk/bittensor-core",
            "sdk/bittensor-core-py",
            "website/apps/bittensor-website/public/catalog",
        ],
    )?)?;
    let extrinsic = String::from_utf8(crate::pin::git(
        &repo,
        &[
            "diff",
            "--name-only",
            previous,
            &next,
            "--",
            "sdk/bittensor-core/src/codec/extrinsic.rs",
        ],
    )?)?;
    Ok(
        json!({"schema":1,"branch":branch,"base_commit":previous,"commit":next,"changed":changed,"catalogs":catalogs,
        "core":{"fingerprint":fingerprint,"changed":core_changed,"extrinsic_changed":!extrinsic.trim().is_empty()},"diff_stat":diff,
        "has_changes":!changed.is_empty()||core_changed||catalogs.iter().any(|value|value["semantic_change"]==true),"pins_modified":false}),
    )
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    let mut branch = "main".to_owned();
    let mut output = None;
    let mut plan = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--branch" => branch = args.next().context("--branch needs main or testnet")?,
            "--out" if output.is_none() => {
                output = Some(PathBuf::from(
                    args.next().context("--out needs a report path")?,
                ))
            }
            "--plan" if !plan => plan = true,
            _ => bail!(
                "usage: cargo xtask upstream-next [--branch main|testnet] [--out json] [--plan]"
            ),
        }
    }
    ensure!(
        ["main", "testnet"].contains(&branch.as_str()),
        "branch must be main or testnet"
    );
    let report = if plan {
        json!({"branch":branch,"read_only":true,"steps":["resolve branch to commit","diff watched tree joined to parity map","normalize catalogs","compare core-input fingerprint"],"python_execution":false})
    } else {
        preview(root, &branch)?
    };
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    if let Some(output) = output {
        let task = crate::oracle::task_path("upstream-next")?
            .parent()
            .context("task root")?
            .to_owned();
        ensure!(
            output.is_absolute()
                && output.starts_with(&task)
                && output != task
                && output.components().all(|component| matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )),
            "upstream-next output must be inside RTAO_TEMP_ROOT"
        );
        let parent = output.parent().context("report parent")?;
        let mut checked = task.clone();
        for component in parent.strip_prefix(&task)?.components() {
            checked.push(component);
            if checked.exists() {
                ensure!(
                    fs::symlink_metadata(&checked)?.is_dir(),
                    "report parent must be a regular task directory"
                );
            } else {
                ensure!(
                    fs::symlink_metadata(&checked).is_err(),
                    "report parent must not be a dangling symlink"
                );
                fs::create_dir(&checked)?;
            }
        }
        ensure!(
            parent.canonicalize()?.starts_with(&task),
            "report parent escapes task root"
        );
        ensure!(
            !output.exists() || fs::symlink_metadata(&output)?.is_file(),
            "report output must be a regular file"
        );
        fs::write(output, bytes)?;
    } else {
        print!("{}", String::from_utf8(bytes)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_location_changes_do_not_report_behavior_changes() {
        let mut first = json!({"entries":[{"name":"transfer","sources":[{"line":1,"end_line":9,"url":"#L1","path":"pallet.rs"}],"amount":1}]});
        let mut second = json!({"entries":[{"name":"transfer","sources":[{"line":20,"end_line":28,"url":"#L20","path":"pallet.rs"}],"amount":1}]});
        normalize(&mut first);
        normalize(&mut second);
        assert_eq!(first, second);
        second["entries"][0]["amount"] = json!(2);
        assert_ne!(first, second);
        let mut schema = json!({"input_schema":{"properties":{"url":{"type":"string"}}}});
        normalize(&mut schema);
        assert_eq!(
            schema["input_schema"]["properties"]["url"]["type"],
            "string"
        );
    }
}
