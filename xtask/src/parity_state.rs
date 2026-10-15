//! Shrink-only pending operations, public exports, divergences and e2e evidence.

use super::symbols::Index;
use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use rustpython_parser::{Parse, ast};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};
use toml_edit::{DocumentMut, Item, Table, value};

fn document(root: &Path, path: &str) -> Result<DocumentMut> {
    fs::read_to_string(root.join(path))?
        .parse()
        .with_context(|| format!("parse {path}"))
}
pub(super) fn identifier(body: &str, name: &str) -> bool {
    struct Identifiers(BTreeSet<String>);
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        while !input.is_empty() {
            if input.peek(syn::Ident) {
                names.insert(input.parse::<syn::Ident>()?.to_string());
            } else if input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in input);
                names.extend(parse(&content)?);
            } else if input.peek(syn::token::Brace) {
                let content;
                syn::braced!(content in input);
                names.extend(parse(&content)?);
            } else if input.peek(syn::token::Bracket) {
                let content;
                syn::bracketed!(content in input);
                names.extend(parse(&content)?);
            } else {
                input.step(|cursor| {
                    cursor
                        .token_tree()
                        .map(|(_, next)| ((), next))
                        .ok_or_else(|| cursor.error("test token absent"))
                })?;
            }
        }
        Ok(names)
    }
    impl syn::parse::Parse for Identifiers {
        fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
            Ok(Self(parse(input)?))
        }
    }
    syn::parse_str::<Identifiers>(body).is_ok_and(|names| names.0.contains(name))
}
fn strings(document: &DocumentMut, name: &str) -> Result<BTreeSet<String>> {
    let values = document
        .get(name)
        .and_then(Item::as_array)
        .with_context(|| format!("pending {name} array absent"))?;
    let names = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("pending names must be strings")
        })
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(names.len() == values.len(), "duplicate pending operation");
    Ok(names)
}
fn git(root: &Path, revision: &str, path: &str) -> Result<Option<String>> {
    ensure!(
        !revision.starts_with('-')
            && revision.bytes().all(|byte| byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'_' | b'.' | b'-' | b'^' | b'~')),
        "unsafe parity baseline revision"
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{revision}:{path}")])
        .output()?;
    if output.status.success() {
        Ok(Some(String::from_utf8(output.stdout)?))
    } else {
        Ok(None)
    }
}
fn baseline(root: &Path, explicit: Option<&str>) -> Result<Option<String>> {
    if let Some(revision) = explicit {
        return Ok(Some(revision.into()));
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        let event_path = std::env::var_os("GITHUB_EVENT_PATH")
            .context("CI parity needs its event file for shrink-only validation")?;
        let event: Value = serde_json::from_slice(&fs::read(event_path)?)?;
        return event_baseline(root, &event);
    }
    // HEAD compares working-tree edits. A clean committed local check compares
    // the previous commit; CI always uses its event's actual change baseline.
    let head = git(root, "HEAD", "parity/pending.toml")?;
    if head.as_deref() != Some(&fs::read_to_string(root.join("parity/pending.toml"))?) {
        Ok(head.map(|_| "HEAD".into()))
    } else {
        Ok(git(root, "HEAD^", "parity/pending.toml")?.map(|_| "HEAD^".into()))
    }
}

fn event_baseline(root: &Path, event: &Value) -> Result<Option<String>> {
    ensure!(
        !(event.get("pull_request").is_some() && event.get("merge_group").is_some()),
        "CI event has ambiguous parity baseline types"
    );
    // A malformed PR/merge-group baseline cannot fall back to a push field.
    let pointer = if event.get("pull_request").is_some() {
        "/pull_request/base/sha"
    } else if event.get("merge_group").is_some() {
        "/merge_group/base_sha"
    } else {
        "/before"
    };
    let sha = event
        .pointer(pointer)
        .and_then(Value::as_str)
        .context("CI event lacks a string parity baseline; refusing unchecked pending growth")?;
    ensure!(super::valid_sha(sha), "CI parity baseline SHA invalid");
    if sha.bytes().all(|byte| byte == b'0') {
        ensure!(
            pointer == "/before",
            "only a first-push before SHA may be all zero"
        );
        return Ok(None);
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .output()?;
    ensure!(
        output.status.success(),
        "CI parity baseline commit unavailable; checkout full history for shrink-only validation"
    );
    Ok(Some(sha.into()))
}

pub(super) fn prior_oracle_artifacts(root: &Path, explicit: Option<&str>) -> Result<Vec<String>> {
    let actual = baseline(root, None)?;
    oracle_history(root, actual.as_deref(), explicit)
}

fn oracle_history_git(root: &Path) -> Command {
    let mut command = Command::new("git");
    // Replacement refs and legacy grafts do not describe committed ancestry.
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .arg("-C")
        .arg(root);
    command
}

pub(super) fn oracle_history(
    root: &Path,
    actual: Option<&str>,
    explicit: Option<&str>,
) -> Result<Vec<String>> {
    let shallow = oracle_history_git(root)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()?;
    ensure!(
        shallow.status.success() && String::from_utf8(shallow.stdout)?.trim() == "false",
        "initial oracle staging requires complete Git history"
    );
    // An explicit base supplements the actual event/local baseline and HEAD.
    let revisions = [Some("HEAD"), actual, explicit]
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
    let mut artifacts = BTreeSet::new();
    for revision in revisions {
        ensure!(
            !revision.is_empty()
                && !revision.starts_with('-')
                && revision.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b'_' | b'.' | b'-' | b'^' | b'~')),
            "unsafe parity baseline revision"
        );
        let commit = oracle_history_git(root)
            .args(["cat-file", "-e", &format!("{revision}^{{commit}}")])
            .output()?;
        ensure!(
            commit.status.success(),
            "oracle removal baseline commit unavailable: {revision}"
        );
        // Removed historical evidence cannot become a first bootstrap again.
        let output = oracle_history_git(root)
            .args([
                "log",
                "--root",
                "--full-history",
                "--no-show-signature",
                "--no-decorate",
                "--format=",
                "--name-only",
                "--no-renames",
                revision,
                "--",
                "vendor/oracle",
            ])
            .output()?;
        ensure!(
            output.status.success() && output.stdout.len() <= 2 * 1024 * 1024,
            "oracle artifact history unavailable or exceeds bound"
        );
        for path in String::from_utf8(output.stdout)?
            .lines()
            .filter(|path| !path.is_empty())
        {
            if !matches!(
                path,
                "vendor/oracle/lock.json"
                    | "vendor/oracle/runtime-lock.json"
                    | "vendor/oracle/samples_extra.json"
            ) {
                artifacts.insert(format!("{revision}:{path}"));
            }
        }
    }
    Ok(artifacts.into_iter().collect())
}

#[cfg(test)]
pub(super) fn oracle_history_for_event(
    root: &Path,
    event: &Value,
    explicit: Option<&str>,
) -> Result<Vec<String>> {
    let actual = event_baseline(root, event)?;
    oracle_history(root, actual.as_deref(), explicit)
}

fn shrink(
    old: &DocumentMut,
    current: &DocumentMut,
    old_intents: &BTreeSet<String>,
    old_reads: &BTreeSet<String>,
    new_intents: &BTreeSet<String>,
    new_reads: &BTreeSet<String>,
    pin_changed: bool,
) -> Result<()> {
    for (family, old_known, new_known) in [
        ("intents", old_intents, new_intents),
        ("reads", old_reads, new_reads),
    ] {
        let old = strings(old, family)?;
        let current = strings(current, family)?;
        let additions = current.difference(&old).cloned().collect::<BTreeSet<_>>();
        ensure!(
            additions.is_empty()
                || (pin_changed
                    && additions.is_subset(&new_known.difference(old_known).cloned().collect())),
            "pending {family} may only shrink; additions {additions:?} need a parity pin bump and genuinely new upstream operations"
        );
    }
    Ok(())
}

pub(super) fn pending(
    root: &Path,
    index: &Index,
    strict: bool,
    base: Option<&str>,
) -> Result<Value> {
    let current = document(root, "parity/pending.toml")?;
    let intents = strings(&current, "intents")?;
    let reads = strings(&current, "reads")?;
    let all_intents = rtao_catalog::INTENTS
        .iter()
        .map(|spec| spec.name.to_owned())
        .collect::<BTreeSet<_>>();
    let all_reads = rtao_catalog::READS
        .iter()
        .map(|spec| spec.name.to_owned())
        .collect::<BTreeSet<_>>();
    ensure!(
        intents.is_subset(&all_intents) && reads.is_subset(&all_reads),
        "pending register has unknown/removed operations"
    );
    let mut files = Vec::new();
    super::rust_files(&root.join("crates/rtao-intents/src"), &mut files)?;
    let mut implemented = BTreeSet::new();
    for path in files
        .into_iter()
        .filter(|path| !path.ends_with("generated/pending.rs"))
    {
        let file = syn::parse_file(&fs::read_to_string(&path)?)?;
        for item in file.items {
            if let syn::Item::Impl(implementation) = item {
                if !implementation.trait_.as_ref().is_some_and(|(_, path, _)| {
                    path.segments
                        .last()
                        .is_some_and(|part| part.ident == "Intent")
                }) {
                    continue;
                }
                let syn::Type::Path(ty) = &*implementation.self_ty else {
                    continue;
                };
                let class = ty
                    .path
                    .segments
                    .last()
                    .context("intent type absent")?
                    .ident
                    .to_string();
                if let Some(spec) = rtao_catalog::INTENTS
                    .iter()
                    .find(|spec| spec.python_class == class)
                {
                    let method = implementation
                        .items
                        .iter()
                        .find_map(|item| match item {
                            syn::ImplItem::Fn(method) if method.sig.ident == "build" => {
                                Some(method)
                            }
                            _ => None,
                        })
                        .context("implemented intent lacks build")?;
                    ensure!(
                        !method
                            .block
                            .to_token_stream()
                            .to_string()
                            .contains("NotYetPorted"),
                        "nonpending intent {} still returns NotYetPorted",
                        spec.name
                    );
                    ensure!(
                        index
                            .feature_test_bodies
                            .iter()
                            .any(|(name, body)| name.starts_with("rtao_intents::")
                                && identifier(body, &class)
                                && body.contains("assert")),
                        "intent {} lacks an ordinary assertion-bearing test of its class",
                        spec.name
                    );
                    ensure!(
                        implemented.insert(spec.name.to_owned()),
                        "intent {} has duplicate behavior implementations",
                        spec.name
                    );
                }
            }
        }
    }
    ensure!(
        implemented == all_intents.difference(&intents).cloned().collect(),
        "pending intents and actual tested build implementations differ: actual={implemented:?}, declared={:?}",
        all_intents.difference(&intents).collect::<Vec<_>>()
    );
    let mut files = Vec::new();
    super::rust_files(&root.join("crates/rtao-reads/src/reads"), &mut files)?;
    let mut implemented_reads = BTreeSet::new();
    for path in files {
        let file = syn::parse_file(&fs::read_to_string(path)?)?;
        for item in file.items {
            if let syn::Item::Fn(function) = item {
                let name = function.sig.ident.to_string();
                if all_reads.contains(&name)
                    && !function
                        .block
                        .to_token_stream()
                        .to_string()
                        .contains("NotYetPorted")
                    && index.feature_test_bodies.iter().any(|(test, body)| {
                        let variant = name
                            .split('_')
                            .map(|word| {
                                let mut chars = word.chars();
                                chars.next().map_or_else(String::new, |first| {
                                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                                })
                            })
                            .collect::<String>();
                        test.starts_with("rtao_reads::")
                            && (identifier(body, &name)
                                || identifier(body, &variant)
                                || body.contains(&format!("{name:?}")))
                            && body.contains("assert")
                    })
                {
                    ensure!(
                        implemented_reads.insert(name),
                        "read has duplicate tested functions"
                    );
                }
            }
        }
    }
    ensure!(
        implemented_reads == all_reads.difference(&reads).cloned().collect(),
        "pending reads and actual tested read functions differ: actual={implemented_reads:?}, declared={:?}",
        all_reads.difference(&reads).collect::<Vec<_>>()
    );
    if let Some(revision) = baseline(root, base)? {
        let old = document_from_git(root, &revision, "parity/pending.toml")?;
        let old_pins = document_from_git(root, &revision, "upstream.toml")?;
        let pins = document(root, "upstream.toml")?;
        let names = |path: &str| -> Result<BTreeSet<String>> {
            let value: Value = serde_json::from_str(
                &git(root, &revision, path)?.context("baseline catalog missing")?,
            )?;
            value
                .as_array()
                .context("baseline catalog must be array")?
                .iter()
                .map(|row| {
                    row["name"]
                        .as_str()
                        .map(str::to_owned)
                        .context("baseline name missing")
                })
                .collect()
        };
        shrink(
            &old,
            &current,
            &names("vendor/upstream/catalog/intents.json")?,
            &names("vendor/upstream/catalog/reads.json")?,
            &all_intents,
            &all_reads,
            old_pins["parity"].to_string() != pins["parity"].to_string(),
        )?;
    }
    ensure!(
        !strict || (intents.is_empty() && reads.is_empty()),
        "strict parity refuses pending intent/read behavior"
    );
    Ok(
        json!({"pending_intents":intents.len(),"pending_reads":reads.len(),"implemented_intents":implemented.len(),"implemented_reads":all_reads.len()-reads.len()}),
    )
}
fn document_from_git(root: &Path, revision: &str, path: &str) -> Result<DocumentMut> {
    git(root, revision, path)?
        .with_context(|| format!("baseline {path} unavailable"))?
        .parse()
        .map_err(Into::into)
}

pub(super) fn exports(root: &Path, index: &Index, strict: bool) -> Result<Value> {
    let discovered = crate::extract::public_exports(root)?;
    let extra = document(root, "parity/exports_extra.toml")?;
    let mut decisions = BTreeMap::new();
    if let Some(rows) = extra.get("export").and_then(Item::as_array_of_tables) {
        for row in rows {
            let symbol = row
                .get("python")
                .and_then(Item::as_str)
                .context("extra export Python symbol absent")?;
            ensure!(
                row.get("reason")
                    .and_then(Item::as_str)
                    .is_some_and(|reason| reason.trim().len() >= 12),
                "export {symbol} needs reason"
            );
            ensure!(
                row.get("home")
                    .and_then(Item::as_str)
                    .is_some_and(|home| !home.trim().is_empty()),
                "export {symbol} needs an explicit Rust/CLI/n-a home"
            );
            let pending = row.get("pending").and_then(Item::as_str);
            ensure!(
                !strict || pending.is_none(),
                "strict export parity refuses {symbol} pending"
            );
            ensure!(
                decisions
                    .insert(symbol.to_owned(), pending.is_some())
                    .is_none(),
                "duplicate extra export {symbol}"
            );
        }
    }
    let mut names = BTreeSet::new();
    let mut mapped = 0;
    let root_names = documented_root_names(root)?;
    for (path, exports) in &discovered {
        let module = super::symbols::module(path)?;
        for name in exports {
            let symbol = format!("{module}.{name}");
            names.insert(symbol.clone());
            ensure!(
                index.aliases.contains_key(&symbol)
                    || decisions.contains_key(&symbol)
                    || (module == "bittensor" && root_names.contains(name)),
                "public export {symbol} lacks a Rust doc alias or reasoned parity/exports_extra.toml row"
            );
            if index.aliases.contains_key(&symbol) {
                mapped += 1;
            }
        }
    }
    ensure!(
        decisions.keys().all(|name| names.contains(name)),
        "export decision names vanished Python API"
    );
    Ok(
        json!({"lists":discovered.len(),"names":names.len(),"aliased":mapped,"decisions":decisions.len(),"pending":decisions.values().filter(|pending|**pending).count()}),
    )
}

pub(super) fn divergences(root: &Path, index: &Index, strict: bool) -> Result<Value> {
    let map = document(root, "parity/divergences.toml")?;
    let rows = map
        .get("divergences")
        .and_then(Item::as_array_of_tables)
        .context("divergence rows missing")?;
    let mut seen = BTreeSet::new();
    let mut pending = 0;
    let mut active = 0;
    for row in rows {
        let string = |name: &str| {
            row.get(name)
                .and_then(Item::as_str)
                .with_context(|| format!("divergence {name} missing"))
        };
        let id = string("id")?;
        ensure!(seen.insert(id.to_owned()), "duplicate divergence {id}");
        let status = row.get("status").and_then(Item::as_str).unwrap_or("active");
        if status == "withdrawn" {
            ensure!(
                id == "D-011" && string("rationale")?.len() >= 12,
                "only documented D-011 is withdrawn"
            );
            continue;
        }
        for field in [
            "upstream_symbol",
            "python_behavior",
            "rtao_behavior",
            "rationale",
        ] {
            ensure!(
                !string(field)?.trim().is_empty(),
                "divergence {id} empty {field}"
            );
        }
        ensure!(
            matches!(
                string("class")?,
                "stricter" | "improved" | "kept" | "addition" | "typed-only"
            ),
            "divergence {id} unknown class"
        );
        let tests = row
            .get("pinning_tests")
            .and_then(Item::as_array)
            .context("divergence pinning_tests missing")?;
        ensure!(!tests.is_empty(), "divergence {id} has no pinning tests");
        let link = string("upstream_link")?;
        ensure!(
            link.starts_with("https://github.com/RaoFoundation/subtensor/")
                || link.starts_with("https://github.com/"),
            "divergence {id} lacks upstream link"
        );
        match status {
            "pending" => {
                pending += 1;
                ensure!(!strict, "strict divergence parity refuses {id} pending");
                ensure!(
                    string("milestone")?.starts_with('M'),
                    "pending divergence milestone missing"
                );
            }
            "active" => {
                active += 1;
                for test in tests {
                    let test = test.as_str().context("divergence test must be string")?;
                    ensure!(
                        index.tests.contains(test),
                        "divergence {id} pinning test does not qualify: {test}: {}",
                        index
                            .rejected_tests
                            .get(test)
                            .map_or("test does not exist", String::as_str)
                    );
                }
            }
            _ => bail!("divergence {id} unknown state"),
        }
    }
    let expected = (1..=35)
        .map(|number| format!("D-{number:03}"))
        .collect::<BTreeSet<_>>();
    ensure!(
        expected.is_subset(&seen),
        "Appendix C divergence register incomplete: {:?}",
        expected.difference(&seen).collect::<Vec<_>>()
    );
    Ok(json!({"total":seen.len(),"active":active,"pending":pending,"withdrawn":1}))
}

pub(super) fn e2e(root: &Path, index: &Index, strict: bool) -> Result<Value> {
    let pins = document(root, "upstream.toml")?;
    let pin = &pins["artifacts"]["sdk/bittensor-core/tests/e2e-manifest.json"];
    let path = pin["to"]
        .as_str()
        .context("e2e manifest destination absent")?;
    ensure!(super::valid_path(path), "e2e manifest unsafe destination");
    let bytes = fs::read(root.join(path))?;
    ensure!(
        pin["sha256"].as_str() == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
        "e2e manifest differs from artifact pin"
    );
    let manifest: Value = serde_json::from_slice(&bytes)?;
    let mut expected = BTreeSet::new();
    collect_manifest_names(&manifest, &mut expected)?;
    ensure!(
        expected.len() == 112,
        "upstream e2e manifest names changed; review complete map"
    );
    let tree = super::read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    for (path, blob) in tree
        .iter()
        .filter(|(path, _)| path.starts_with("sdk/python/tests/e2e/") && path.ends_with(".py"))
    {
        let source = fs::read_to_string(root.join("vendor/upstream/src").join(path))?;
        ensure!(
            crate::extract::git_blob(source.as_bytes()) == *blob,
            "Python e2e source differs from watched blob"
        );
        for statement in
            ast::Suite::parse(&source, path).map_err(|error| anyhow::anyhow!("{error}"))?
        {
            let name = match statement {
                ast::Stmt::FunctionDef(function) => function.name.to_string(),
                ast::Stmt::AsyncFunctionDef(function) => function.name.to_string(),
                _ => continue,
            };
            if name.starts_with("test_") {
                expected.insert(format!(
                    "pyt:e2e/{}::{name}",
                    path.strip_prefix("sdk/python/tests/e2e/")
                        .context("e2e path missing prefix")?
                ));
            }
        }
    }
    let map = document(root, "parity/e2e_map.toml")?;
    let rows = map
        .get("tests")
        .and_then(Item::as_table_like)
        .context("e2e tests map missing")?;
    ensure!(
        rows.iter()
            .map(|(name, _)| name.to_owned())
            .collect::<BTreeSet<_>>()
            == expected,
        "e2e map has missing or vanished authenticated keys"
    );
    let mut pending = 0;
    let mut mapped = 0;
    let mut na = 0;
    for (name, row) in rows.iter() {
        let present = ["rtao", "pending", "na"]
            .iter()
            .filter(|key| row.get(key).is_some())
            .copied()
            .collect::<Vec<_>>();
        ensure!(
            present.len() == 1,
            "e2e {name} needs exactly one rtao/pending/na state"
        );
        let value = row[present[0]]
            .as_str()
            .context("e2e state must be string")?;
        match present[0] {
            "rtao" => {
                ensure!(
                    value.starts_with("e2e::") || value.starts_with("interop::"),
                    "e2e mapped target must be e2e or interop"
                );
                ensure!(
                    index
                        .feature_tests
                        .contains(&format!("rtao_testkit::{value}")),
                    "e2e mapped test does not qualify: {value}: {}",
                    index
                        .rejected_feature_tests
                        .get(&format!("rtao_testkit::{value}"))
                        .map_or("test does not exist", String::as_str)
                );
                mapped += 1;
            }
            "pending" => {
                ensure!(
                    value.starts_with('M') && value[1..].bytes().all(|byte| byte.is_ascii_digit()),
                    "e2e pending milestone invalid"
                );
                ensure!(!strict, "strict e2e parity refuses {name} pending");
                pending += 1;
            }
            "na" => {
                ensure!(value.trim().len() >= 12, "e2e n/a needs reviewed reason");
                na += 1;
            }
            _ => bail!("unknown e2e state"),
        }
    }
    Ok(json!({"keys":expected.len(),"mapped":mapped,"pending":pending,"na":na}))
}
fn collect_manifest_names(value: &Value, names: &mut BTreeSet<String>) -> Result<()> {
    match value {
        Value::String(name) if name.starts_with("intent_") || name.starts_with("test_") => {
            ensure!(
                names.insert(name.clone()),
                "duplicate e2e manifest name {name}"
            );
        }
        Value::Array(values) => {
            for value in values {
                collect_manifest_names(value, names)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if key.starts_with("intent_") || key.starts_with("test_") {
                    ensure!(
                        names.insert(key.clone()),
                        "duplicate e2e manifest name {key}"
                    );
                }
                collect_manifest_names(value, names)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn documented_root_names(root: &Path) -> Result<BTreeSet<String>> {
    let document = fs::read_to_string(root.join("docs/plan/python-module-map.md"))?;
    let section = document
        .split_once("## Top-level exports")
        .context("root public export module-map table missing")?
        .1
        .split("\n## ")
        .next()
        .context("root exports section missing")?;
    let mut names = rtao_catalog::INTENTS
        .iter()
        .map(|spec| spec.python_class.to_owned())
        .collect::<BTreeSet<_>>();
    for line in section.lines().filter(|line| line.starts_with("| `")) {
        let cells = super::markdown_cells(line);
        if let Some(cell) = cells.first() {
            for name in cell.split('`').skip(1).step_by(2) {
                if !name.contains(|character: char| character.is_whitespace())
                    && name != "_REMOVED_V10_HINTS"
                {
                    names.insert(name.to_owned());
                }
            }
        }
    }
    Ok(names)
}

pub(super) fn seed_exports(root: &Path, index: &Index) -> Result<()> {
    let exports = crate::extract::public_exports(root)?;
    let mut rows = toml_edit::ArrayOfTables::new();
    let root_names = documented_root_names(root)?;
    for (path, names) in exports {
        let module = super::symbols::module(&path)?;
        for name in names {
            let symbol = format!("{module}.{name}");
            if index.aliases.contains_key(&symbol)
                || (module == "bittensor" && root_names.contains(&name))
            {
                continue;
            }
            let mut row = Table::new();
            row["python"] = value(symbol);
            row["home"] = value(if module.contains(".evm") {
                "rtao_evm"
            } else if module.contains(".extension")
                || module.contains(".vault")
                || module.ends_with(".ledger")
            {
                "rtao_signer"
            } else if module.contains(".intents") {
                "rtao_intents"
            } else if module.contains(".reads") {
                "rtao_reads"
            } else if module.ends_with(".wallet") || module.ends_with(".keyfiles") {
                "rtao_wallet"
            } else if module.ends_with(".http_auth") {
                "rtao_btauth"
            } else {
                "rtao facade/public export review"
            });
            row["reason"] = value(
                "Public Python export has no exact Rust doc alias yet; its existing or planned home requires an explicit API review before this row leaves pending.",
            );
            row["pending"] = value("M8");
            row["source"] = value(&path);
            rows.push(row);
        }
    }
    let mut document = DocumentMut::new();
    document["schema"] = value(1);
    document["export"] = Item::ArrayOfTables(rows);
    fs::write(
        root.join("parity/exports_extra.toml"),
        format!(
            "# Explicit API completeness decisions; pending rows do not claim behavior parity.\n{document}"
        ),
    )?;
    Ok(())
}

pub(super) fn seed_divergences(root: &Path) -> Result<()> {
    let mut document = document(root, "parity/divergences.toml")?;
    let pins = document_pins(root)?;
    let commit = super::required_string(&pins["parity"], "commit")?.to_owned();
    let rows = document
        .get_mut("divergences")
        .and_then(Item::as_array_of_tables_mut)
        .context("existing divergence register missing")?;
    let existing = rows
        .iter()
        .filter_map(|row| row.get("id").and_then(Item::as_str))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for row in rows.iter_mut() {
        if row.get("upstream_link").is_none() {
            let path = if row.get("id").and_then(Item::as_str) == Some("D-035") {
                "sdk/python/bittensor/balance.py"
            } else {
                "pallets/subtensor/src/coinbase/tempo_control.rs"
            };
            row["upstream_link"] = value(format!(
                "https://github.com/RaoFoundation/subtensor/blob/{commit}/{path}"
            ));
        }
    }
    let architecture = fs::read_to_string(root.join("docs/plan/architecture.md"))?;
    let section = architecture
        .split_once("## Appendix C.")
        .context("Appendix C missing")?
        .1
        .split("\n## Appendix D.")
        .next()
        .context("Appendix C end missing")?;
    for line in section.lines().filter(|line| line.starts_with("| D-")) {
        let cells = super::markdown_cells(line);
        ensure!(cells.len() == 7, "Appendix C row schema changed");
        let id = &cells[0];
        if existing.contains(id) {
            continue;
        }
        let mut row = Table::new();
        row["id"] = value(id);
        row["upstream_symbol"] = value(cells[1].trim_matches('`'));
        row["python_behavior"] = value(&cells[2]);
        row["rtao_behavior"] = value(&cells[3]);
        row["rationale"] = value(format!(
            "Explicit design decision recorded in docs/plan/architecture.md Appendix C ({id}); pending means the behavior/test is not claimed implemented."
        ));
        if id == "D-011" {
            row["status"] = value("withdrawn");
        } else {
            row["status"] = value("pending");
            row["class"] = value(
                cells[4]
                    .split_whitespace()
                    .next()
                    .context("divergence class missing")?,
            );
            row["milestone"] = value(if id == "D-031" { "M10" } else { "M8" });
            let mut tests = toml_edit::Array::new();
            tests.push(cells[5].as_str());
            row["pinning_tests"] = value(tests);
            row["upstream_link"] = value(format!(
                "https://github.com/RaoFoundation/subtensor/blob/{commit}/sdk/python/bittensor"
            ));
            row["upstream_report"] = value(&cells[6]);
        }
        rows.push(row);
    }
    fs::write(
        root.join("parity/divergences.toml"),
        format!(
            "# Appendix C decisions. Pending entries do not claim implemented behavior or tests.\n{document}"
        ),
    )?;
    Ok(())
}
fn document_pins(root: &Path) -> Result<DocumentMut> {
    document(root, "upstream.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn behavior_citations_accept_macro_identifiers_and_refuse_literal_spoofs() {
        assert!(identifier(
            "{assert_eq!(TransferAll { keep_alive:false }.summary(),expected);}",
            "TransferAll"
        ));
        assert!(!identifier(
            "{assert_eq!(\"TransferAll\",\"TransferAll\");}",
            "TransferAll"
        ));
        assert!(!identifier(
            "{assert!(OtherTransferAll::new());}",
            "TransferAll"
        ));
    }
    #[test]
    fn pending_only_grows_for_new_catalog_operations_in_a_pin_bump() -> Result<()> {
        let old = "intents=['old']\nreads=['read']".parse::<DocumentMut>()?;
        let changed = "intents=['old','existing']\nreads=['read']".parse::<DocumentMut>()?;
        let original = BTreeSet::from(["old".into(), "existing".into()]);
        let reads = BTreeSet::from(["read".into()]);
        assert!(shrink(&old, &changed, &original, &reads, &original, &reads, false).is_err());
        assert!(shrink(&old, &changed, &original, &reads, &original, &reads, true).is_err());
        let prior = BTreeSet::from(["old".into()]);
        shrink(&old, &changed, &prior, &reads, &original, &reads, true)?;
        let removed = "intents=[]\nreads=[]".parse::<DocumentMut>()?;
        shrink(&old, &removed, &original, &reads, &original, &reads, false)?;
        Ok(())
    }
}
