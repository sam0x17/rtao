//! Offline upstream file coverage and attribution checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use toml_edit::{Array, DocumentMut, Item, Table, value};

#[path = "parity_cli_psf.rs"]
mod cli_psf;

const PACKAGE: &str = "sdk/python/bittensor/";
const ORIGINAL_LICENSE: &str = "MIT OR Apache-2.0";
const DERIVED_LICENSE: &str = "(MIT OR Apache-2.0) AND Apache-2.0";
const ATTRIBUTION: &str = "// Derived from RaoFoundation/subtensor ";
const ATTRIBUTION_END: &str = " (Apache-2.0); modified by rtao.";
const THIRD_PARTY_ATTRIBUTION: &str = "// Derived from ";
const THIRD_PARTY_END: &str = "; modified by rtao.";

/// Licenses of non-subtensor upstreams named by `// Derived from` headers.
///
/// Every such header must end with `(<license>); modified by rtao.` and name
/// either MIT or Apache-2.0, so a package's declared license can include it.
fn third_party_licenses(source: &str) -> Result<BTreeSet<&'static str>> {
    let mut licenses = BTreeSet::new();
    for line in source.lines().take(12) {
        if !line.starts_with(THIRD_PARTY_ATTRIBUTION) || line.starts_with(ATTRIBUTION) {
            continue;
        }
        let body = line
            .strip_suffix(THIRD_PARTY_END)
            .with_context(|| format!("third-party attribution lacks its ending: {line}"))?;
        let license = body
            .rsplit_once('(')
            .and_then(|(_, license)| license.strip_suffix(')'))
            .with_context(|| format!("third-party attribution lacks a license: {line}"))?;
        licenses.insert(match license {
            "MIT" => "MIT",
            "Apache-2.0" => "Apache-2.0",
            // CPython ports carry PSF-2.0, whose exact notice and retained
            // license file have their own dedicated checks.
            "PSF-2.0" => continue,
            other => bail!("unsupported third-party attribution license {other:?}: {line}"),
        });
    }
    Ok(licenses)
}
const UNICODE_TABLE: &str = "crates/rtao-core-ext/src/ops/python_printable.rs";
const UNICODE_LICENSE: &str = "Unicode-DFS-2016";
const UNICODE_NOTICE_SHA256: &str =
    "68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844";
const UNICODE_PROVENANCE: &str = "// @generated from official UnicodeData-15.0.0.txt; do not hand-edit ranges.\n// UCD15 SHA256 806e9aed65037197f1ec85e12be6e8cd870fc5608b4de0fffd990f689f376a73.\n// CPython v3.12.11 uses Unicode 15: commit 55fee9cf216abe4ec0d1139f94b1930fbd0c7644.\n";
const KINDS: &[&str] = &[
    "generated",
    "extracted",
    "vendored",
    "ported",
    "oracle-only",
    "n/a",
];

#[path = "parity_catalog.rs"]
mod catalog;
#[path = "parity_state.rs"]
mod state;
#[path = "parity_symbols.rs"]
mod symbols;

#[path = "parity_matrix.rs"]
mod matrix;

#[path = "parity_report.rs"]
mod report;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    py: String,
    rs: Vec<String>,
    kind: String,
    synced_blob: String,
    synced_tag: String,
    milestone: String,
    notes: String,
}

/// Run the M0 file parity and license gate, or create its initial seed.
pub fn run(args: &[String]) -> Result<()> {
    let root = repository_root()?;
    match args.first().map(String::as_str) {
        Some("report") => report::run(&root, &args[1..]),
        Some("bless") => {
            ensure!(
                args.len() > 1,
                "usage: cargo xtask parity bless <py-path>..."
            );
            bless(&root, &args[1..])
        }
        Some("matrix") => {
            ensure!(
                args.len() == 1 || (args.len() == 2 && args[1] == "--check"),
                "usage: cargo xtask parity matrix [--check]"
            );
            println!(
                "{}",
                matrix::run(&root, &symbols::index(&root)?, args.len() == 2)?
            );
            Ok(())
        }
        Some("check") => {
            let (strict, base) = check_arguments(&args[1..])?;
            let matrix = check_all(&root, strict, base.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&matrix)?);
            Ok(())
        }
        Some("seed-exports") => {
            ensure!(args.len() == 1, "usage: cargo xtask parity seed-exports");
            ensure!(
                !root.join("parity/exports_extra.toml").exists(),
                "exports_extra.toml already exists; refusing to discard reviewed decisions"
            );
            state::seed_exports(&root, &symbols::index(&root)?)
        }
        Some("seed-divergences") => {
            ensure!(
                args.len() == 1,
                "usage: cargo xtask parity seed-divergences"
            );
            state::seed_divergences(&root)
        }
        Some("seed") => {
            ensure!(args.len() == 1, "usage: cargo xtask parity seed");
            let destination = root.join("parity/map.toml");
            ensure!(
                !destination.exists(),
                "{} already exists; refusing to discard reconciliation state",
                destination.display()
            );
            let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
            let tag = required_string(&pins["parity"], "tag")?;
            let document = fs::read_to_string(root.join("docs/plan/python-module-map.md"))?;
            let listing = fs::read_to_string(root.join("vendor/upstream/tree.txt"))?;
            fs::create_dir_all(root.join("parity"))?;
            fs::write(destination, seed_map(&document, &listing, tag)?)?;
            println!("parity: seeded all 362 documented rows as pending");
            Ok(())
        }
        _ => {
            bail!(
                "usage: cargo xtask parity check [--strict] [--base <rev>] | matrix [--check] | report --base <workspace> --out <json> [--autobless] | bless <py-path>... | seed | seed-exports | seed-divergences"
            )
        }
    }
}

pub(crate) fn python_symbols(
    path: &str,
    source: &str,
) -> Result<BTreeMap<String, serde_json::Value>> {
    symbols::python_symbols(path, source)
}

pub(crate) fn prior_oracle_artifacts(root: &Path, base: Option<&str>) -> Result<Vec<String>> {
    state::prior_oracle_artifacts(root, base)
}

#[cfg(test)]
pub(crate) fn oracle_history_for_test(
    root: &Path,
    actual: Option<&str>,
    base: Option<&str>,
) -> Result<Vec<String>> {
    state::oracle_history(root, actual, base)
}

#[cfg(test)]
pub(crate) fn dispatch_baseline_for_test(root: &Path, sha: &str) -> Result<String> {
    state::dispatch_baseline(root, sha)
}

#[cfg(test)]
pub(crate) fn oracle_history_for_event(
    root: &Path,
    event: &serde_json::Value,
    base: Option<&str>,
) -> Result<Vec<String>> {
    state::oracle_history_for_event(root, event, base)
}

fn bless(root: &Path, paths: &[String]) -> Result<()> {
    let destination = root.join("parity/map.toml");
    let contents = fs::read_to_string(&destination)?;
    let rows = read_map(&contents)?;
    let mut document = contents.parse::<DocumentMut>()?;
    let tree = read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
    let tag = required_string(&pins["parity"], "tag")?;
    let paths = paths.iter().cloned().collect::<BTreeSet<_>>();
    let mut candidates = rows;
    for path in &paths {
        ensure!(valid_path(path), "unsafe parity bless path");
        let row = candidates
            .iter_mut()
            .find(|row| &row.py == path)
            .with_context(|| format!("no parity row for {path}"))?;
        let blob = tree
            .get(path)
            .with_context(|| format!("{path} absent from authenticated upstream tree"))?;
        if path.ends_with(".py") || path.ends_with(".rs") {
            let bytes = fs::read(root.join("vendor/upstream/src").join(path))?;
            ensure!(
                crate::extract::git_blob(&bytes) == *blob,
                "{path} bytes do not match authenticated upstream blob"
            );
        }
        row.synced_blob.clone_from(blob);
        row.synced_tag = tag.to_owned();
    }
    check_files(&candidates, &tree, false)?;
    check_licenses(root, &candidates)?;
    let tables = document
        .get_mut("file")
        .and_then(Item::as_array_of_tables_mut)
        .context("parity file rows missing")?;
    for row in tables.iter_mut() {
        let path = row
            .get("py")
            .and_then(Item::as_str)
            .context("parity row path missing")?;
        if paths.contains(path) {
            let blob = tree[path].clone();
            row["synced_blob"] = value(blob);
            row["synced_tag"] = value(tag);
        }
    }
    fs::write(destination, document.to_string())?;
    println!(
        "parity: explicitly reconciled {} reviewed file rows; regenerate parity matrix",
        paths.len()
    );
    Ok(())
}

fn check_arguments(args: &[String]) -> Result<(bool, Option<String>)> {
    let mut strict = false;
    let mut base = None;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--strict" if !strict => strict = true,
            "--base" if base.is_none() => {
                base = Some(
                    arguments
                        .next()
                        .context("--base requires revision")?
                        .clone(),
                )
            }
            _ => bail!("usage: cargo xtask parity check [--strict] [--base <rev>]"),
        }
    }
    Ok((strict, base))
}

fn layer(
    matrix: &mut serde_json::Map<String, serde_json::Value>,
    findings: &mut Vec<String>,
    name: &str,
    result: Result<serde_json::Value>,
) {
    match result {
        Ok(value) => {
            matrix.insert(
                name.into(),
                serde_json::json!({"status":"checked","details":value}),
            );
        }
        Err(error) => {
            let error = format!("{error:#}");
            findings.push(format!("{name}: {error}"));
            matrix.insert(
                name.into(),
                serde_json::json!({"status":"failed","error":error}),
            );
        }
    }
}

fn check_all(root: &Path, strict: bool, base: Option<&str>) -> Result<serde_json::Value> {
    let rows = read_map(&fs::read_to_string(root.join("parity/map.toml"))?)?;
    let tree = read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    let index = symbols::index(root)?;
    let mut matrix = serde_json::Map::new();
    let mut findings = Vec::new();
    layer(&mut matrix,&mut findings,"files",check_files(&rows,&tree,strict).map(|()|serde_json::json!({"watched":rows.len(),"pending":rows.iter().filter(|row|row.synced_blob=="pending").count()})));
    layer(
        &mut matrix,
        &mut findings,
        "licenses",
        check_licenses(root, &rows).map(|()| serde_json::json!({"attribution":"checked"})),
    );
    layer(
        &mut matrix,
        &mut findings,
        "symbols",
        symbols::changed(root, &rows, &tree, &index)
            .map(|changes| serde_json::json!({"aliases":index.aliases.len(),"changed":changes})),
    );
    layer(
        &mut matrix,
        &mut findings,
        "catalog",
        catalog::catalogs(root),
    );
    layer(
        &mut matrix,
        &mut findings,
        "extraction",
        crate::extract::check_at(root).map(|()| serde_json::json!({"all_outputs":"checked"})),
    );
    layer(
        &mut matrix,
        &mut findings,
        "chain_source",
        catalog::chain(root, strict),
    );
    layer(
        &mut matrix,
        &mut findings,
        "coverage",
        catalog::coverage(root),
    );
    layer(
        &mut matrix,
        &mut findings,
        "environment",
        (|| -> Result<serde_json::Value> {
            let inventory = catalog::json_file(root, "vendor/upstream/extracted/env_vars.json")?;
            ensure!(
                inventory["parity"]["status"].as_str() == Some("matches"),
                "environment map must exactly cover extracted names"
            );
            ensure!(
                !strict || inventory["parity"]["pending"].as_u64() == Some(0),
                "strict parity refuses pending environment behavior"
            );
            Ok(inventory["parity"].clone())
        })(),
    );
    layer(
        &mut matrix,
        &mut findings,
        "pending",
        state::pending(root, &index, strict, base),
    );
    layer(
        &mut matrix,
        &mut findings,
        "exports",
        state::exports(root, &index, strict),
    );
    layer(
        &mut matrix,
        &mut findings,
        "divergences",
        state::divergences(root, &index, strict),
    );
    layer(
        &mut matrix,
        &mut findings,
        "e2e",
        state::e2e(root, &index, strict),
    );
    match crate::oracle::check_committed(root, strict, base) {
        Ok(status) => {
            matrix.insert("oracle".into(), status.report());
        }
        Err(error) => layer(&mut matrix, &mut findings, "oracle", Err(error)),
    }
    layer(
        &mut matrix,
        &mut findings,
        "matrix",
        matrix::run(root, &index, true),
    );
    let output = serde_json::json!({"schema":1,"strict":strict,"layers":matrix});
    ensure!(
        findings.is_empty(),
        "parity failed:\n{}\n\n{}",
        findings.join("\n"),
        serde_json::to_string_pretty(&output)?
    );
    Ok(output)
}

fn repository_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    cwd.ancestors()
        .find(|path| path.join("docs/plan/python-module-map.md").is_file())
        .map(Path::to_path_buf)
        .context("run parity from the rtao repository")
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\t', '\n', '\r'])
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn read_tree(listing: &str) -> Result<BTreeMap<String, String>> {
    let mut tree = BTreeMap::new();
    for (line_number, line) in listing.lines().enumerate() {
        let (metadata, path) = line.split_once('\t').with_context(|| {
            format!("tree.txt:{}: expected git ls-tree record", line_number + 1)
        })?;
        let fields: Vec<_> = metadata.split_whitespace().collect();
        ensure!(
            fields.len() == 3
                && matches!(fields[0], "100644" | "100755" | "120000")
                && fields[1] == "blob"
                && valid_sha(fields[2]),
            "tree.txt:{}: invalid blob record",
            line_number + 1
        );
        ensure!(
            valid_path(path),
            "tree.txt:{}: invalid upstream path",
            line_number + 1
        );
        ensure!(
            tree.insert(path.to_owned(), fields[2].to_owned()).is_none(),
            "tree.txt: duplicate path {path}"
        );
    }
    ensure!(!tree.is_empty(), "tree.txt has no watched files");
    Ok(tree)
}

fn required_string<'a>(item: &'a Item, key: &str) -> Result<&'a str> {
    item.get(key)
        .and_then(Item::as_str)
        .with_context(|| format!("missing or non-string {key}"))
}

fn read_map(contents: &str) -> Result<Vec<Row>> {
    let document: DocumentMut = contents.parse().context("parse parity/map.toml")?;
    let tables = document
        .get("file")
        .and_then(Item::as_array_of_tables)
        .context("parity/map.toml requires [[file]] rows")?;
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for table in tables {
        let item = Item::Table(table.clone());
        let py = required_string(&item, "py")?.to_owned();
        ensure!(valid_path(&py), "invalid map path {py:?}");
        ensure!(seen.insert(py.clone()), "duplicate map row {py}");
        let kind = required_string(&item, "kind")?.to_owned();
        ensure!(
            KINDS.contains(&kind.as_str()),
            "{py}: invalid parity kind {kind:?}"
        );
        let synced_blob = required_string(&item, "synced_blob")?.to_owned();
        ensure!(
            synced_blob == "pending" || valid_sha(&synced_blob),
            "{py}: synced_blob must be pending or a lowercase git blob SHA"
        );
        let rs = item
            .get("rs")
            .and_then(Item::as_array)
            .context("rs must be an array of Rust homes")?
            .iter()
            .map(|home| {
                home.as_str()
                    .map(str::to_owned)
                    .context("rs homes must be strings")
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            !rs.is_empty(),
            "{py}: rs must preserve the documented home, including explicit n/a reasons"
        );
        rows.push(Row {
            py,
            rs,
            kind,
            synced_blob,
            synced_tag: required_string(&item, "synced_tag")?.to_owned(),
            milestone: required_string(&item, "milestone")?.to_owned(),
            notes: required_string(&item, "notes")?.to_owned(),
        });
    }
    ensure!(!rows.is_empty(), "parity/map.toml has no file rows");
    Ok(rows)
}

fn check_files(rows: &[Row], tree: &BTreeMap<String, String>, strict: bool) -> Result<()> {
    let mapped: BTreeSet<_> = rows.iter().map(|row| row.py.as_str()).collect();
    let mut findings = Vec::new();
    for path in tree.keys() {
        if !mapped.contains(path.as_str()) {
            findings.push(format!("unmapped upstream file: {path}"));
        }
    }
    for row in rows {
        match tree.get(&row.py) {
            None => findings.push(format!("removed upstream file: {}", row.py)),
            Some(_) if row.synced_blob == "pending" => {
                if strict {
                    findings.push(format!(
                        "pending parity row: {} ({})",
                        row.py, row.milestone
                    ));
                }
            }
            Some(blob) if blob != &row.synced_blob => findings.push(format!(
                "changed blessed file: {}\n  git diff {} {}",
                row.py, row.synced_blob, blob
            )),
            Some(_) => {}
        }
    }
    ensure!(
        findings.is_empty(),
        "file parity failed:\n{}",
        findings.join("\n")
    );
    Ok(())
}

/// Expand the documented package rows and grouped watched roots into the initial map.
/// Existing reconciliation state is never used as a source for this seed.
pub fn seed_map(document: &str, listing: &str, tag: &str) -> Result<String> {
    let tree = read_tree(listing)?;
    let mut rows = BTreeMap::new();
    let mut other_roots = false;
    let mut package_count = 0;
    for line in document.lines() {
        if line.starts_with("## Other watched roots") {
            other_roots = true;
        }
        if line.starts_with("## Top-level exports") {
            break;
        }
        if !line.starts_with("| `") {
            continue;
        }
        let cells = markdown_cells(line);
        if !other_roots && cells.len() == 7 && KINDS.contains(&cells[4].trim_matches('`')) {
            let py = format!("{PACKAGE}{}", cells[0].trim_matches('`'));
            let row = pending_row(
                &py,
                &cells[2],
                cells[4].trim_matches('`'),
                &cells[5],
                &cells[6],
                tag,
            );
            ensure!(
                rows.insert(py, row).is_none(),
                "duplicate documented package row"
            );
            package_count += 1;
        } else if other_roots && cells.len() == 5 {
            let expected: usize = cells[1]
                .parse()
                .context("invalid documented watched-root count")?;
            let patterns = documented_patterns(&cells[0])?;
            let paths: Vec<_> = tree
                .keys()
                .filter(|path| {
                    patterns
                        .iter()
                        .any(|pattern| matches_pattern(pattern, path))
                })
                .collect();
            ensure!(
                paths.len() == expected,
                "documented watched group {} expects {expected} files, found {}",
                cells[0],
                paths.len()
            );
            for py in paths {
                let mut kind = cells[2]
                    .split('`')
                    .nth(1)
                    .context("watched-root group has no parity kind")?
                    .to_owned();
                if py.starts_with("sdk/python/tests/unit/") && is_ported_test(py) {
                    kind = "ported".to_owned();
                }
                let home = if cells[3].is_empty() {
                    "none needed"
                } else {
                    &cells[3]
                };
                let mut row = pending_row(
                    py,
                    home,
                    &kind,
                    &cells[4],
                    &format!("Documented group {}: {}", cells[0], cells[3]),
                    tag,
                );
                binding_home(&mut row);
                ensure!(
                    rows.insert(py.to_owned(), row).is_none(),
                    "overlapping documented group for {py}"
                );
            }
        }
    }
    ensure!(
        package_count == 223,
        "expected all 223 explicit package rows, found {package_count}"
    );
    ensure!(
        rows.len() == 362,
        "expected all 362 documented map rows, found {}",
        rows.len()
    );
    let rows: Vec<_> = rows.into_values().collect();
    check_files(&rows, &tree, false)?;
    Ok(write_map(&rows))
}

fn pending_row(py: &str, home: &str, kind: &str, milestone: &str, notes: &str, tag: &str) -> Row {
    Row {
        py: py.to_owned(),
        rs: vec![home.to_owned()],
        kind: kind.to_owned(),
        synced_blob: "pending".to_owned(),
        synced_tag: tag.to_owned(),
        milestone: milestone.to_owned(),
        notes: notes.to_owned(),
    }
}

fn markdown_cells(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in line.trim().trim_matches('|').chars() {
        if ch == '|' && !escaped {
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(ch);
        }
        escaped = ch == '\\' && !escaped;
    }
    cells.push(cell.trim().to_owned());
    cells
}

fn documented_patterns(cell: &str) -> Result<Vec<String>> {
    let mut patterns = Vec::new();
    for (index, segment) in cell.split('`').enumerate() {
        if index % 2 == 1 {
            // The fixture cell abbreviates its second path relative to fixtures/.
            let path = if segment == "shape_corpus/corpus.json" {
                "sdk/python/tests/fixtures/shape_corpus/corpus.json"
            } else {
                segment
            };
            patterns.extend(expand_braces(path)?);
        }
    }
    ensure!(
        !patterns.is_empty(),
        "watched group has no path patterns: {cell}"
    );
    Ok(patterns)
}

fn expand_braces(pattern: &str) -> Result<Vec<String>> {
    let Some(start) = pattern.find('{') else {
        return Ok(vec![pattern.to_owned()]);
    };
    let end = pattern[start..]
        .find('}')
        .map(|offset| start + offset)
        .context("unclosed watched-path brace")?;
    let mut paths = Vec::new();
    for choice in pattern[start + 1..end].split(',') {
        paths.extend(expand_braces(&format!(
            "{}{choice}{}",
            &pattern[..start],
            &pattern[end + 1..]
        ))?);
    }
    Ok(paths)
}

fn matches_pattern(pattern: &str, path: &str) -> bool {
    if let Some(root) = pattern.strip_suffix("/**") {
        return path.starts_with(&format!("{root}/"));
    }
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return path
            .strip_prefix(prefix)
            .and_then(|tail| tail.strip_suffix(suffix))
            .is_some_and(|middle| !middle.contains('/'));
    }
    pattern == path
}

fn is_ported_test(path: &str) -> bool {
    let name = path
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".py"))
        .unwrap_or("");
    [
        "test_rpc",
        "test_http_auth",
        "test_balance_properties",
        "test_null_weights",
        "test_slippage_protection",
        "test_root_claim_fee",
        "test_lock_ownership_gate",
        "test_hyperparams_units",
        "test_fee_filters",
        "test_multisig_safety",
        "test_prepare_attach",
        "test_wallet_compat",
        "test_v446_accounting_and_commitments",
        "test_pow_registration",
        "test_cli_null_pruning",
        "test_natural_name_key",
        "test_facades",
        "test_ledger_signer",
        "test_extension_bridge",
        "test_extension_signer",
        "test_evm",
    ]
    .contains(&name)
}

fn binding_home(row: &mut Row) {
    // Architecture §4.9 requires these four oracle binding rows to name mirrored shapes.
    match row.py.as_str() {
        "sdk/bittensor-core-py/src/values.rs" => row
            .rs
            .push("rtao_primitives::IntoCoreValue; rtao_core::DynValue".to_owned()),
        "sdk/bittensor-core-py/src/runtime.rs" => row
            .rs
            .push("rtao_runtime; rtao_core_ext::ir::MetadataIr".to_owned()),
        "sdk/bittensor-core-py/src/timelock.rs" => row
            .rs
            .push("rtao_tx::timelock; rtao_primitives::drand".to_owned()),
        "sdk/bittensor-core-py/src/keys.rs" => {
            row.rs.push("rtao_signer; rtao_core::keys".to_owned())
        }
        _ => {}
    }
}

fn write_map(rows: &[Row]) -> String {
    let mut tables = toml_edit::ArrayOfTables::new();
    for row in rows {
        let mut table = Table::new();
        table["py"] = value(&row.py);
        let mut homes = Array::new();
        for home in &row.rs {
            homes.push(home.as_str());
        }
        table["rs"] = value(homes);
        table["kind"] = value(&row.kind);
        table["synced_blob"] = value(&row.synced_blob);
        table["synced_tag"] = value(&row.synced_tag);
        table["milestone"] = value(&row.milestone);
        table["notes"] = value(&row.notes);
        tables.push(table);
    }
    let mut document = DocumentMut::new();
    document["file"] = Item::ArrayOfTables(tables);
    format!(
        "# Seeded from docs/plan/python-module-map.md; all 362 rows initially pending.\n# rs preserves each documented Rust home; milestone and notes preserve its scope.\n# tree.txt is the complete watched listing from architecture §4.9, not a list\n# derived from this map. Every listing entry must have a row; none is ignored.\n# Package, tests/{{unit,harness,fixtures,e2e}}, codegen, completions, both core\n# crates, rpc_info and eco-tests are recursive roots. Only the three named\n# Python record/basket scripts and the named chain sources/catalogs are watched.\n# Python tests/__init__.py and conftest.py, other scripts, and pallet lib.rs\n# are outside that scope (lib.rs constants are separate extraction inputs).\n# A pending home is prospective. Blessed ported/generated/extracted rows plus\n# actual attribution headers determine which packages contain derived material.\n\n{document}"
    )
}

fn rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            rust_files(&entry.path(), output)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
        {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn attribution(source: &str) -> Result<Option<(&str, &str)>> {
    if !source.lines().any(|line| {
        line.trim_start().strip_prefix("//").is_some_and(|comment| {
            comment
                .trim_start()
                .starts_with("Derived from RaoFoundation/subtensor")
        })
    }) {
        return Ok(None);
    }
    let first = source.lines().next().context("empty derived source")?;
    let body = first
        .strip_prefix(ATTRIBUTION)
        .and_then(|body| body.strip_suffix(ATTRIBUTION_END))
        .context("derived Rust file must start with the canonical attribution header")?;
    let (path, commit) = body
        .split_once(" @ ")
        .context("attribution header requires upstream path @ commit")?;
    ensure!(
        valid_path(path) && valid_sha(commit),
        "attribution header requires a monorepo path and full lowercase commit SHA"
    );
    Ok(Some((path, commit)))
}

fn attributions(source: &str) -> Result<BTreeSet<(&str, &str)>> {
    let Some(primary) = attribution(source)? else {
        return Ok(BTreeSet::new());
    };
    let mut result = BTreeSet::from([primary]);
    for line in source
        .lines()
        .skip(1)
        .filter(|line| line.starts_with(ATTRIBUTION))
    {
        let body = line
            .strip_prefix(ATTRIBUTION)
            .and_then(|body| body.strip_suffix(ATTRIBUTION_END))
            .context("secondary attribution must use the complete canonical header")?;
        let (path, commit) = body
            .split_once(" @ ")
            .context("secondary attribution requires upstream path @ commit")?;
        ensure!(
            valid_path(path) && valid_sha(commit),
            "secondary attribution requires a safe path and full lowercase commit"
        );
        result.insert((path, commit));
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum IndependentProducer {
    Metadata,
    Descriptions,
}

fn independent_producer(row: &Row) -> Result<Option<IndependentProducer>> {
    let frozen_model = match row.py.as_str() {
        "pallets/subtensor/src/rpc_info/basket_info.rs" => Some("basket_info"),
        "pallets/subtensor/src/rpc_info/delegate_info.rs" => Some("delegate_info"),
        "pallets/subtensor/src/rpc_info/dynamic_info.rs" => Some("dynamic_info"),
        "pallets/subtensor/src/rpc_info/metagraph.rs" => Some("metagraph"),
        "pallets/subtensor/src/rpc_info/neuron_info.rs" => Some("neuron_info"),
        "pallets/subtensor/src/rpc_info/show_subnet.rs" => Some("show_subnet"),
        "pallets/subtensor/src/rpc_info/stake_info.rs" => Some("stake_info"),
        "pallets/subtensor/src/rpc_info/subnet_info.rs" => Some("subnet_info"),
        _ => None,
    };
    if let Some(module) = frozen_model {
        ensure!(
            row.kind == "extracted"
                && row.rs
                    == [
                        "codegen/frozen_models.toml".to_owned(),
                        format!(
                            "rtao_runtime::models (metadata projections and authenticated {module} freeze_struct layout tripwires)"
                        ),
                    ],
            "{}: frozen model reconciliation must retain its exact checked projection/tripwire homes",
            row.py
        );
        return Ok(Some(IndependentProducer::Metadata));
    }
    let metadata = match row.py.as_str() {
        "sdk/python/bittensor/_generated/calls.py" => Some("calls"),
        "sdk/python/bittensor/_generated/constants.py" => Some("constants"),
        "sdk/python/bittensor/_generated/errors.py" => Some("errors::MODULE_ERRORS"),
        "sdk/python/bittensor/_generated/runtime_apis.py" => Some("runtime_apis"),
        "sdk/python/bittensor/_generated/storage.py" => Some("storage"),
        _ => None,
    };
    if let Some(module) = metadata {
        ensure!(
            row.kind == "generated" && row.rs == [format!("`rtao_runtime::generated::{module}`")],
            "{}: metadata cross-check row must retain its exact independently generated Rust home",
            row.py
        );
        return Ok(Some(IndependentProducer::Metadata));
    }
    const DESCRIPTIONS: &[&str] = &[
        "__init__",
        "admin_utils",
        "balances",
        "commitments",
        "contracts",
        "crowdloan",
        "drand",
        "ethereum",
        "evm",
        "grandpa",
        "limit_orders",
        "mev_shield",
        "multisig",
        "preimage",
        "proxy",
        "safe_mode",
        "scheduler",
        "subtensor",
        "sudo",
        "swap",
        "system",
        "utility",
    ];
    if DESCRIPTIONS
        .iter()
        .any(|module| row.py == format!("sdk/python/bittensor/error_descriptions/{module}.py"))
    {
        ensure!(
            row.kind == "generated"
                && row.rs == ["`rtao_catalog::CHAIN_ERRORS` (description field)"],
            "{}: description cross-check row must retain its exact checked catalog home",
            row.py
        );
        return Ok(Some(IndependentProducer::Descriptions));
    }
    Ok(None)
}

fn independent_producers(root: &Path, rows: &[Row]) -> Result<BTreeSet<String>> {
    let mut kinds = BTreeSet::new();
    let mut checked = BTreeSet::new();
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let tag = required_string(&pins["parity"], "tag")?;
    let tree = read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
    for row in rows.iter().filter(|row| row.synced_blob != "pending") {
        let Some(producer) = independent_producer(row)? else {
            continue;
        };
        ensure!(
            row.synced_tag == tag && tree.get(&row.py) == Some(&row.synced_blob),
            "{}: producer reconciliation requires the current exact source blob and tag",
            row.py
        );
        let source = fs::read(root.join("vendor/upstream/src").join(&row.py))?;
        ensure!(
            crate::extract::git_blob(&source) == row.synced_blob,
            "{}: independent producer source bytes differ from authenticated tree",
            row.py
        );
        kinds.insert(producer);
        checked.insert(row.py.clone());
    }
    if kinds.contains(&IndependentProducer::Metadata) {
        // This recomputes every generated Rust byte from authenticated V15/V16
        // and independently checks the five Python wire surfaces against it.
        crate::regen::check_at(root)?;
    }
    if kinds.contains(&IndependentProducer::Descriptions) {
        // Recompute all leaf tables, exact assembly grammar, per-entry source
        // provenance and the 406 catalog descriptions, then the Rust output.
        crate::extract::check_at(root)?;
        catalog::catalogs(root)?;
    }
    Ok(checked)
}

fn mapped_crates(home: &str, names: &BTreeSet<String>) -> BTreeSet<String> {
    names
        .iter()
        .filter(|name| {
            [name.as_str(), &name.replace('-', "_")]
                .iter()
                .any(|spelling| {
                    home.match_indices(spelling).any(|(offset, matched)| {
                        let before = home[..offset].chars().next_back();
                        let after = home[offset + matched.len()..].chars().next();
                        !before
                            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                            && !after.is_some_and(|ch| {
                                ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
                            })
                    })
                })
        })
        .cloned()
        .collect()
}

fn read_unicode_file(root: &Path, relative: &str) -> Result<String> {
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "invalid Unicode attribution path {relative}"
        );
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path)
                .with_context(|| format!("inspect Unicode attribution path {}", path.display()))?
                .file_type()
                .is_symlink(),
            "Unicode attribution path must not be a symlink: {}",
            path.display()
        );
    }
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "Unicode attribution path must be a regular file: {}",
        path.display()
    );
    fs::read_to_string(&path).with_context(|| format!("read Unicode attribution file {relative}"))
}

fn check_unicode_attribution(root: &Path) -> Result<bool> {
    match fs::symlink_metadata(root.join(UNICODE_TABLE)) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("inspect Unicode printable table"),
    }
    let table = read_unicode_file(root, UNICODE_TABLE)?;
    ensure!(
        table.starts_with(UNICODE_PROVENANCE),
        "{UNICODE_TABLE}: retain the pinned UCD and CPython provenance header"
    );
    let notice = read_unicode_file(root, "crates/rtao-core-ext/LICENSE-UNICODE")?;
    ensure!(
        format!("{:x}", Sha256::digest(notice.as_bytes())) == UNICODE_NOTICE_SHA256,
        "rtao-core-ext/LICENSE-UNICODE must retain the exact full Unicode notice"
    );
    let documentation = read_unicode_file(root, "docs/licenses/Unicode-DFS-2016.txt")?;
    ensure!(
        format!("{:x}", Sha256::digest(documentation.as_bytes())) == UNICODE_NOTICE_SHA256,
        "docs/licenses/Unicode-DFS-2016.txt must retain the exact full Unicode notice"
    );
    let commented_notice = notice
        .lines()
        .map(|line| {
            if line.is_empty() {
                "//\n".to_owned()
            } else {
                format!("// {line}\n")
            }
        })
        .collect::<String>();
    ensure!(
        table.contains(&commented_notice),
        "{UNICODE_TABLE}: retain the full Unicode notice in its source header"
    );
    let attribution = read_unicode_file(root, "NOTICE")?;
    let (_, section) = attribution
        .split_once("Unicode 15 printable data\n")
        .context("NOTICE must attribute the Unicode 15 printable data")?;
    ensure!(
        section.contains("rtao-core-ext")
            && section.contains("UnicodeData-15.0.0.txt")
            && section.contains(UNICODE_LICENSE)
            && section.contains(&notice),
        "NOTICE must name the Unicode printable table's package/source/license and retain its full notice"
    );
    Ok(true)
}

fn check_licenses(root: &Path, rows: &[Row]) -> Result<()> {
    let independent = if rows.iter().any(|row| {
        row.synced_blob != "pending"
            && independent_producer(row).is_ok_and(|producer| producer.is_some())
    }) {
        independent_producers(root, rows)?
    } else {
        // Even a malformed narrow producer mapping must fail, not silently
        // fall back to ordinary attribution through the prefilter above.
        for row in rows.iter().filter(|row| row.synced_blob != "pending") {
            independent_producer(row)?;
        }
        BTreeSet::new()
    };
    let workspace: DocumentMut = fs::read_to_string(root.join("Cargo.toml"))?.parse()?;
    let workspace_license = workspace["workspace"]["package"]["license"]
        .as_str()
        .context("workspace license is missing")?;
    ensure!(
        workspace_license == ORIGINAL_LICENSE,
        "workspace license must be {ORIGINAL_LICENSE:?}"
    );
    let mut packages = BTreeMap::new();
    for entry in fs::read_dir(root.join("crates"))? {
        let directory = entry?.path();
        if directory.join("Cargo.toml").is_file() {
            packages.insert(directory, ());
        }
    }
    if root.join("xtask/Cargo.toml").is_file() {
        packages.insert(root.join("xtask"), ());
    }
    let mut manifests = BTreeMap::new();
    for directory in packages.keys() {
        let document: DocumentMut = fs::read_to_string(directory.join("Cargo.toml"))?.parse()?;
        let name = required_string(&document["package"], "name")?.to_owned();
        ensure!(
            manifests.insert(name, (directory, document)).is_none(),
            "duplicate package name during license check"
        );
    }
    let names = manifests.keys().cloned().collect();
    let mut derived = BTreeSet::new();
    for row in rows.iter().filter(|row| {
        row.synced_blob != "pending"
            && matches!(row.kind.as_str(), "ported" | "generated" | "extracted")
    }) {
        for home in &row.rs {
            derived.extend(mapped_crates(home, &names));
        }
    }
    let mut attributed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut third_party: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    let mut cli_psf = false;
    for (name, (directory, _)) in &manifests {
        let mut files = Vec::new();
        rust_files(directory, &mut files)?;
        for path in files {
            let source = fs::read_to_string(&path)?;
            cli_psf |= cli_psf::source(root, name, directory, &path, &source)?;
            let licenses = third_party_licenses(&source)
                .with_context(|| format!("{}: invalid attribution", path.display()))?;
            if !licenses.is_empty() {
                third_party
                    .entry(name.to_owned())
                    .or_default()
                    .extend(licenses);
            }
            for (upstream_path, _) in attributions(&source)
                .with_context(|| format!("{}: invalid attribution", path.display()))?
            {
                derived.insert(name.to_owned());
                attributed
                    .entry(name.to_owned())
                    .or_default()
                    .insert(upstream_path.to_owned());
            }
        }
    }
    for row in rows.iter().filter(|row| {
        row.synced_blob != "pending"
            && matches!(row.kind.as_str(), "ported" | "generated" | "extracted")
    }) {
        if independent.contains(&row.py) {
            continue;
        }
        for home in &row.rs {
            for name in mapped_crates(home, &names) {
                ensure!(
                    attributed
                        .get(&name)
                        .is_some_and(|sources| sources.contains(&row.py)),
                    "{}: blessed derived home {name} contains no attribution header naming its upstream source",
                    row.py
                );
            }
            // Exact mapped Rust paths additionally prove that a particular file
            // is derived. Symbolic homes require attribution for that same upstream
            // file in their containing package, rather than an unrelated header.
            for token in home.split(['`', ' ', ',', ';', '(', ')']) {
                if token.ends_with(".rs") && valid_path(token) {
                    let relative = if token.starts_with("crates/") || token.starts_with("xtask/") {
                        token.to_owned()
                    } else if names
                        .iter()
                        .any(|name| token.starts_with(&format!("{name}/")))
                    {
                        format!("crates/{token}")
                    } else {
                        continue;
                    };
                    let path = root.join(&relative);
                    ensure!(
                        path.is_file(),
                        "{}: missing implemented Rust home {token}",
                        row.py
                    );
                    ensure!(
                        attributions(&fs::read_to_string(&path)?)?
                            .iter()
                            .any(|(upstream_path, _)| *upstream_path == row.py),
                        "{token}: mapped derived Rust file is missing its attribution header"
                    );
                }
            }
        }
    }
    let unicode = check_unicode_attribution(root)?;
    if unicode {
        ensure!(
            manifests
                .get("rtao-core-ext")
                .is_some_and(|(directory, _)| { **directory == root.join("crates/rtao-core-ext") }),
            "the Unicode printable table must belong to rtao-core-ext at its exact crate path"
        );
    }
    for (name, (directory, document)) in &manifests {
        let license = &document["package"]["license"];
        let actual = if license.get("workspace").and_then(Item::as_bool) == Some(true) {
            workspace_license
        } else {
            license
                .as_str()
                .with_context(|| format!("{name}: missing package license"))?
        };
        let foreign = third_party.get(name.as_str());
        let base = if derived.contains(name)
            || foreign.is_some_and(|licenses| licenses.contains("Apache-2.0"))
        {
            DERIVED_LICENSE
        } else {
            ORIGINAL_LICENSE
        };
        let psf = name == "rtao-primitives"
            && fs::read_to_string(directory.join("src/drand.rs"))?
                .lines()
                .take(6)
                .any(|line| line.contains("CPython v3.12.12 Modules/_datetimemodule.c (PSF-2.0)"));
        let mut expected = if psf {
            let license = fs::read_to_string(directory.join("LICENSE-PSF"))?;
            ensure!(
                license.contains("PYTHON SOFTWARE FOUNDATION LICENSE VERSION 2"),
                "CPython port requires its retained PSF license"
            );
            format!("{base} AND PSF-2.0")
        } else {
            base.to_owned()
        };
        if foreign.is_some_and(|licenses| licenses.contains("MIT")) {
            if expected == ORIGINAL_LICENSE {
                expected = format!("({ORIGINAL_LICENSE})");
            }
            expected.push_str(" AND MIT");
        }
        if cli_psf && name == "rtao-cli" {
            expected.push_str(" AND PSF-2.0");
        }
        if unicode && name == "rtao-core-ext" {
            expected.push_str(" AND ");
            expected.push_str(UNICODE_LICENSE);
        }
        ensure!(
            actual == expected,
            "{name}: license {actual:?}; expected {expected:?} from parity homes and attribution headers"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_attributions_name_a_supported_license() -> Result<()> {
        let source = "// Derived from opentensor/bittensor bittensor/core/axon.py @ v10.5.0 (MIT); modified by rtao.\n// Derived from RaoFoundation/polkadot-sdk substrate/primitives/core/src/crypto.rs @ cacb (Apache-2.0); modified by rtao.\n//! Code.\n";
        assert_eq!(
            third_party_licenses(source)?,
            BTreeSet::from(["Apache-2.0", "MIT"])
        );
        let subtensor =
            format!("{ATTRIBUTION}sdk/python/bittensor/axon.py @ abc{ATTRIBUTION_END}\n");
        assert!(third_party_licenses(&subtensor)?.is_empty());
        assert!(
            third_party_licenses("// Derived from x/y z.py @ v1 (GPL-3.0); modified by rtao.\n")
                .is_err()
        );
        assert!(third_party_licenses("// Derived from x/y z.py @ v1 (MIT)\n").is_err());
        Ok(())
    }
    use std::sync::atomic::{AtomicU64, Ordering};

    const SHA: &str = "1111111111111111111111111111111111111111";
    const OTHER_SHA: &str = "2222222222222222222222222222222222222222";

    fn row(path: &str) -> Row {
        pending_row(
            path,
            "rtao_example::module",
            "ported",
            "M0",
            "fixture",
            "v478",
        )
    }

    fn listing(path: &str) -> String {
        format!("100644 blob {SHA}\t{path}\n")
    }

    #[test]
    fn detects_unknown_upstream_file_and_unknown_map_row() -> Result<()> {
        let tree = read_tree(&listing("sdk/python/bittensor/new.py"))?;
        let error = check_files(&[row("sdk/python/bittensor/old.py")], &tree, false)
            .err()
            .context("expected coverage failure")?
            .to_string();
        assert!(error.contains("unmapped upstream file: sdk/python/bittensor/new.py"));
        assert!(error.contains("removed upstream file: sdk/python/bittensor/old.py"));
        Ok(())
    }

    #[test]
    fn pending_checks_presence_and_strict_refuses_pending() -> Result<()> {
        let path = "sdk/python/bittensor/example.py";
        let rows = vec![row(path)];
        let tree = read_tree(&listing(path))?;
        check_files(&rows, &tree, false)?;
        assert!(check_files(&rows, &tree, true).is_err());
        assert!(check_files(&rows, &BTreeMap::new(), false).is_err());
        Ok(())
    }

    #[test]
    fn blessed_blob_tampering_reports_both_shas() -> Result<()> {
        let path = "sdk/python/bittensor/example.py";
        let mut mapped = row(path);
        mapped.synced_blob = OTHER_SHA.to_owned();
        let error = check_files(&[mapped], &read_tree(&listing(path))?, false)
            .err()
            .context("expected changed blob failure")?
            .to_string();
        assert!(error.contains(&format!("git diff {OTHER_SHA} {SHA}")));
        Ok(())
    }

    #[test]
    fn composite_provenance_retains_primary_grammar_and_every_secondary_source() -> Result<()> {
        let first =
            format!("{ATTRIBUTION}sdk/python/bittensor/settings.py @ {SHA}{ATTRIBUTION_END}");
        let second = format!(
            "{ATTRIBUTION}sdk/python/bittensor/hyperparams.py @ {OTHER_SHA}{ATTRIBUTION_END}"
        );
        let source = format!("{first}\n// @generated\n{second}\n//! Shared source table.\n");
        assert_eq!(
            attributions(&source)?,
            BTreeSet::from([
                ("sdk/python/bittensor/settings.py", SHA),
                ("sdk/python/bittensor/hyperparams.py", OTHER_SHA),
            ])
        );
        assert!(attributions(&source.replacen(OTHER_SHA, "short", 1)).is_err());
        assert!(attributions(&format!("//! Unattributed primary.\n{second}\n")).is_err());
        assert!(
            attributions(&source.replace("sdk/python/bittensor/hyperparams.py", "../escape.py"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn independent_producer_exceptions_require_exact_kind_and_actual_home() -> Result<()> {
        let mut metadata = row("sdk/python/bittensor/_generated/storage.py");
        metadata.kind = "generated".into();
        metadata.rs = vec!["`rtao_runtime::generated::storage`".into()];
        assert_eq!(
            independent_producer(&metadata)?,
            Some(IndependentProducer::Metadata)
        );
        metadata.rs.push("rtao_example::unchecked".into());
        assert!(independent_producer(&metadata).is_err());
        metadata.rs.pop();
        metadata.kind = "ported".into();
        assert!(independent_producer(&metadata).is_err());
        let mut descriptions = row("sdk/python/bittensor/error_descriptions/proxy.py");
        descriptions.kind = "generated".into();
        descriptions.rs = vec!["`rtao_catalog::CHAIN_ERRORS` (description field)".into()];
        assert_eq!(
            independent_producer(&descriptions)?,
            Some(IndependentProducer::Descriptions)
        );
        descriptions.rs = vec!["rtao_catalog::unreviewed".into()];
        assert!(independent_producer(&descriptions).is_err());
        descriptions.py = "sdk/python/bittensor/error_descriptions/new_unreviewed.py".into();
        assert_eq!(independent_producer(&descriptions)?, None);
        let mut frozen = row("pallets/subtensor/src/rpc_info/basket_info.rs");
        frozen.kind = "extracted".into();
        frozen.rs = vec![
            "codegen/frozen_models.toml".into(),
            "rtao_runtime::models (metadata projections and authenticated basket_info freeze_struct layout tripwires)".into(),
        ];
        assert_eq!(
            independent_producer(&frozen)?,
            Some(IndependentProducer::Metadata)
        );
        frozen.rs[0] = "codegen/unreviewed_models.toml".into();
        assert!(independent_producer(&frozen).is_err());
        frozen.py = "pallets/subtensor/src/rpc_info/unreviewed.rs".into();
        assert_eq!(independent_producer(&frozen)?, None);
        Ok(())
    }

    #[test]
    fn rows_and_listing_reject_duplicates_and_malformed_blobs() -> Result<()> {
        let path = "sdk/python/bittensor/example.py";
        assert!(read_tree(&format!("{}{}", listing(path), listing(path))).is_err());
        assert!(read_tree(&listing(path).replace(SHA, "not-a-blob")).is_err());
        assert!(read_tree(&listing("../escape.py")).is_err());
        assert!(read_map(&write_map(&[row(path), row(path)])).is_err());
        assert!(read_map(&write_map(&[row(path)]).replace("pending", "invalid-blob")).is_err());
        Ok(())
    }

    #[test]
    fn no_listing_entries_are_silently_excluded() -> Result<()> {
        let path = "sdk/python/bittensor/example.py";
        let listing = format!(
            "{}{}",
            listing(path),
            listing("sdk/python/tests/conftest.py")
        );
        assert!(check_files(&[row(path)], &read_tree(&listing)?, false).is_err());
        Ok(())
    }

    #[test]
    fn seed_preserves_all_documented_rows_and_coverage() -> Result<()> {
        let root = repository_root()?;
        let document = fs::read_to_string(root.join("docs/plan/python-module-map.md"))?;
        let committed = read_map(&fs::read_to_string(root.join("parity/map.toml"))?)?;
        // This seed regression test remains runnable before import-upstream. The
        // independently committed explicit key set supplies its synthetic blobs;
        // `parity check` verifies coverage against the actual imported listing.
        let listing = committed
            .iter()
            .map(|row| self::listing(&row.py))
            .collect::<String>();
        let seed = seed_map(&document, &listing, "v478")?;
        let rows = read_map(&seed)?;
        assert_eq!(rows.len(), 362);
        assert_eq!(
            rows.iter()
                .filter(|row| row.py.starts_with(PACKAGE))
                .count(),
            223
        );
        for (kind, expected) in [
            ("extracted", 6),
            ("vendored", 24),
            ("generated", 29),
            ("n/a", 18),
            ("ported", 146),
        ] {
            assert_eq!(
                rows.iter()
                    .filter(|row| row.py.starts_with(PACKAGE) && row.kind == kind)
                    .count(),
                expected,
                "documented package kind {kind}"
            );
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.py.starts_with("sdk/python/tests/unit/") && row.kind == "ported")
                .count(),
            21
        );
        assert!(rows.iter().all(|row| row.synced_blob == "pending"));
        check_files(&rows, &read_tree(&listing)?, false)?;
        // The seed retains the original prospective homes; the audited map
        // subsequently records reviewed actual homes/reasons and source blobs.
        // Its complete watched-file key set must still equal the seed exactly.
        assert_eq!(
            rows.iter().map(|row| &row.py).collect::<Vec<_>>(),
            committed.iter().map(|row| &row.py).collect::<Vec<_>>()
        );
        // Do not let a missing map row or a new upstream file disappear during seeding.
        assert!(
            seed_map(
                &document.replacen("| `balance.py`", "| balance.py", 1),
                &listing,
                "v478"
            )
            .is_err()
        );
        assert!(
            seed_map(
                &document,
                &format!("{listing}{}", self::listing("sdk/python/bittensor/new.py")),
                "v478"
            )
            .is_err()
        );
        Ok(())
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            // An explicit task root wins when set; ordinary CI and local
            // Cargo runs keep their disposable fixtures inside the repo target.
            let base = if let Some(base) = std::env::var_os("RTAO_TEMP_ROOT") {
                PathBuf::from(base).join("license-gate/fixtures")
            } else if let Some(base) = std::env::var_os("TMPDIR") {
                PathBuf::from(base)
            } else {
                repository_root()?.join("target")
            };
            let directory = base.join(format!(
                "parity-license-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(directory.join("crates/rtao-example/src"))?;
            fs::write(
                directory.join("Cargo.toml"),
                "[workspace.package]\nlicense = 'MIT OR Apache-2.0'\n",
            )?;
            let fixture = Self(directory);
            fixture.license(ORIGINAL_LICENSE)?;
            fs::write(
                fixture.0.join("crates/rtao-example/src/lib.rs"),
                "//! Original Rust code.\n",
            )?;
            Ok(fixture)
        }

        fn license(&self, license: &str) -> Result<()> {
            fs::write(
                self.0.join("crates/rtao-example/Cargo.toml"),
                format!("[package]\nname = 'rtao-example'\nlicense = '{license}'\n"),
            )?;
            Ok(())
        }

        fn derived(&self) -> Result<()> {
            fs::write(
                self.0.join("crates/rtao-example/src/lib.rs"),
                format!(
                    "{ATTRIBUTION}sdk/python/bittensor/example.py @ {SHA}{ATTRIBUTION_END}\n//! Derived Rust code.\n"
                ),
            )?;
            Ok(())
        }

        fn unicode(&self) -> Result<()> {
            let package = self.0.join("crates/rtao-core-ext");
            fs::create_dir_all(package.join("src/ops"))?;
            fs::create_dir_all(self.0.join("docs/licenses"))?;
            self.unicode_license(&format!("{DERIVED_LICENSE} AND {UNICODE_LICENSE}"))?;
            fs::write(
                package.join("src/lib.rs"),
                format!("{ATTRIBUTION}sdk/python/bittensor/example.py @ {SHA}{ATTRIBUTION_END}\n"),
            )?;
            fs::write(
                self.0.join(UNICODE_TABLE),
                include_str!("../../crates/rtao-core-ext/src/ops/python_printable.rs"),
            )?;
            let notice = include_str!("../../crates/rtao-core-ext/LICENSE-UNICODE");
            fs::write(package.join("LICENSE-UNICODE"), notice)?;
            fs::write(self.0.join("docs/licenses/Unicode-DFS-2016.txt"), notice)?;
            fs::write(
                self.0.join("NOTICE"),
                format!(
                    "Unicode 15 printable data\n\nrtao-core-ext data from UnicodeData-15.0.0.txt under {UNICODE_LICENSE}.\n\n{notice}"
                ),
            )?;
            Ok(())
        }

        fn unicode_license(&self, license: &str) -> Result<()> {
            fs::write(
                self.0.join("crates/rtao-core-ext/Cargo.toml"),
                format!("[package]\nname = 'rtao-core-ext'\nlicense = '{license}'\n"),
            )?;
            Ok(())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Fixtures contain only owned files and self-contained symlinks.
            // No child, launchd unit, config or environment depends on them.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn independent_producers_refuse_wrong_tag_blob_source_and_missing_producer_prerequisites()
    -> Result<()> {
        let fixture = Fixture::new()?;
        let path = "sdk/python/bittensor/_generated/storage.py";
        let source = b"# authenticated generated fixture\n";
        let blob = crate::extract::git_blob(source);
        fs::create_dir_all(
            fixture
                .0
                .join("vendor/upstream/src/sdk/python/bittensor/_generated"),
        )?;
        fs::write(fixture.0.join("upstream.toml"), "[parity]\ntag='v478'\n")?;
        fs::write(
            fixture.0.join("vendor/upstream/tree.txt"),
            format!("100644 blob {blob}\t{path}\n"),
        )?;
        let file = fixture.0.join("vendor/upstream/src").join(path);
        fs::write(&file, source)?;
        let mut mapped = row(path);
        mapped.kind = "generated".into();
        mapped.rs = vec!["`rtao_runtime::generated::storage`".into()];
        mapped.synced_blob = blob;
        mapped.synced_tag = "old-tag".into();
        assert!(independent_producers(&fixture.0, &[mapped.clone()]).is_err());
        mapped.synced_tag = "v478".into();
        let good_blob = mapped.synced_blob.clone();
        mapped.synced_blob = OTHER_SHA.into();
        assert!(independent_producers(&fixture.0, &[mapped.clone()]).is_err());
        mapped.synced_blob = good_blob;
        fs::write(&file, b"# changed bytes\n")?;
        assert!(independent_producers(&fixture.0, &[mapped.clone()]).is_err());
        fs::write(file, source)?;
        // Matching Python bytes do not authorize the exception by themselves:
        // absent metadata-pin prerequisites still fail producer validation.
        assert!(independent_producers(&fixture.0, &[mapped]).is_err());
        Ok(())
    }

    #[test]
    fn pending_homes_do_not_relicense_original_crates() -> Result<()> {
        let fixture = Fixture::new()?;
        check_licenses(&fixture.0, &[row("sdk/python/bittensor/example.py")])?;
        fixture.license(DERIVED_LICENSE)?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn workspace_inheritance_does_not_hide_derived_or_invalid_license() -> Result<()> {
        let fixture = Fixture::new()?;
        fs::write(
            fixture.0.join("crates/rtao-example/Cargo.toml"),
            "[package]\nname = 'rtao-example'\nlicense.workspace = true\n",
        )?;
        check_licenses(&fixture.0, &[])?;
        fixture.derived()?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.license(DERIVED_LICENSE)?;
        fs::write(
            fixture.0.join("Cargo.toml"),
            "[workspace.package]\nlicense = 'MIT'\n",
        )?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn actual_headers_require_compound_license_even_without_map() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.derived()?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.license(DERIVED_LICENSE)?;
        check_licenses(&fixture.0, &[])?;
        fixture.license("MIT")?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn blessed_homes_require_attribution_and_compound_license() -> Result<()> {
        let fixture = Fixture::new()?;
        let mut mapped = row("sdk/python/bittensor/example.py");
        mapped.synced_blob = SHA.to_owned();
        fixture.license(DERIVED_LICENSE)?;
        assert!(check_licenses(&fixture.0, &[mapped.clone()]).is_err());
        fixture.derived()?;
        check_licenses(&fixture.0, &[mapped])?;
        Ok(())
    }

    #[test]
    fn unrelated_header_cannot_cover_another_blessed_home() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.derived()?;
        fixture.license(DERIVED_LICENSE)?;
        let mut mapped = row("sdk/python/bittensor/other.py");
        mapped.synced_blob = SHA.to_owned();
        assert!(check_licenses(&fixture.0, &[mapped]).is_err());
        Ok(())
    }

    #[test]
    fn exact_documented_paths_require_their_own_headers() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.derived()?;
        fixture.license(DERIVED_LICENSE)?;
        let mut mapped = row("sdk/python/bittensor/example.py");
        mapped.synced_blob = SHA.to_owned();
        mapped.rs = vec!["`rtao-example/src/generated/example.rs`, generated by xtask".to_owned()];
        fs::create_dir_all(fixture.0.join("crates/rtao-example/src/generated"))?;
        let destination = fixture
            .0
            .join("crates/rtao-example/src/generated/example.rs");
        fs::write(
            &destination,
            "//! Header was removed from this derived file.\n",
        )?;
        assert!(check_licenses(&fixture.0, &[mapped.clone()]).is_err());
        fs::write(
            &destination,
            format!("{ATTRIBUTION}sdk/python/bittensor/example.py @ {SHA}{ATTRIBUTION_END}\n"),
        )?;
        check_licenses(&fixture.0, &[mapped])?;
        Ok(())
    }

    #[test]
    fn unicode_clause_requires_its_table_and_preserves_base_attribution() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.unicode()?;
        check_licenses(&fixture.0, &[])?;
        for unsupported in [
            DERIVED_LICENSE.to_owned(),
            format!("{ORIGINAL_LICENSE} AND {UNICODE_LICENSE}"),
            format!("{DERIVED_LICENSE} AND {UNICODE_LICENSE} AND BSD-3-Clause"),
        ] {
            fixture.unicode_license(&unsupported)?;
            assert!(check_licenses(&fixture.0, &[]).is_err());
        }
        fixture.unicode_license(&format!("{DERIVED_LICENSE} AND {UNICODE_LICENSE}"))?;
        fs::remove_file(fixture.0.join(UNICODE_TABLE))?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn unrelated_packages_cannot_claim_the_unicode_clause() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.license(&format!("{ORIGINAL_LICENSE} AND {UNICODE_LICENSE}"))?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.unicode()?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.license(ORIGINAL_LICENSE)?;
        fs::write(
            fixture.0.join("crates/rtao-core-ext/Cargo.toml"),
            format!(
                "[package]\nname = 'different-package'\nlicense = '{DERIVED_LICENSE} AND {UNICODE_LICENSE}'\n"
            ),
        )?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn unicode_standalone_notices_and_root_attribution_reject_missing_or_tampered_files()
    -> Result<()> {
        for relative in [
            "crates/rtao-core-ext/LICENSE-UNICODE",
            "docs/licenses/Unicode-DFS-2016.txt",
            "NOTICE",
        ] {
            let fixture = Fixture::new()?;
            fixture.unicode()?;
            let path = fixture.0.join(relative);
            let original = fs::read(&path)?;
            fs::remove_file(&path)?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "missing {relative}"
            );
            fs::write(
                &path,
                "UNICODE, INC. LICENSE AGREEMENT - DATA FILES AND SOFTWARE\n",
            )?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "truncated {relative}"
            );
            let changed = String::from_utf8(original.clone())?
                .replace("Copyright © 1991-2022 Unicode, Inc.", "Copyright removed");
            fs::write(&path, changed)?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "changed {relative}"
            );
            fs::write(&path, original)?;
            check_licenses(&fixture.0, &[])?;
        }
        Ok(())
    }

    #[test]
    fn unicode_table_requires_pinned_provenance_and_the_full_inline_notice() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.unicode()?;
        let path = fixture.0.join(UNICODE_TABLE);
        let original = fs::read_to_string(&path)?;
        for changed in [
            original.replace("UnicodeData-15.0.0.txt", "UnicodeData-16.0.0.txt"),
            original.replace("806e9aed65037197", "0000000000000000"),
            original.replace("55fee9cf216abe4e", "0000000000000000"),
            original.replace(
                "// Copyright © 1991-2022 Unicode, Inc. All rights reserved.\n",
                "",
            ),
        ] {
            fs::write(&path, changed)?;
            assert!(check_licenses(&fixture.0, &[]).is_err());
        }
        fs::write(&path, original)?;
        check_licenses(&fixture.0, &[])?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unicode_attribution_rejects_file_and_parent_directory_symlinks() -> Result<()> {
        use std::os::unix::fs::symlink;

        for relative in [
            UNICODE_TABLE,
            "crates/rtao-core-ext/LICENSE-UNICODE",
            "docs/licenses/Unicode-DFS-2016.txt",
            "NOTICE",
            "docs/licenses",
        ] {
            let fixture = Fixture::new()?;
            fixture.unicode()?;
            let path = fixture.0.join(relative);
            let target = fixture.0.join("symlink-target");
            fs::rename(&path, &target)?;
            symlink(&target, &path)?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "symlink {relative}"
            );
        }
        Ok(())
    }

    #[test]
    fn unicode_addition_keeps_the_existing_psf_guard() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.unicode()?;
        let package = fixture.0.join("crates/rtao-primitives");
        fs::create_dir_all(package.join("src"))?;
        let manifest = package.join("Cargo.toml");
        fs::write(
            &manifest,
            format!(
                "[package]\nname = 'rtao-primitives'\nlicense = '{DERIVED_LICENSE} AND PSF-2.0'\n"
            ),
        )?;
        fs::write(
            package.join("src/drand.rs"),
            format!(
                "{ATTRIBUTION}sdk/python/bittensor/timelock.py @ {SHA}{ATTRIBUTION_END}\n// CPython v3.12.12 Modules/_datetimemodule.c (PSF-2.0)\n"
            ),
        )?;
        fs::write(
            package.join("LICENSE-PSF"),
            "PYTHON SOFTWARE FOUNDATION LICENSE VERSION 2\n",
        )?;
        check_licenses(&fixture.0, &[])?;
        fs::write(
            &manifest,
            format!("[package]\nname = 'rtao-primitives'\nlicense = '{DERIVED_LICENSE}'\n"),
        )?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fs::write(
            &manifest,
            format!(
                "[package]\nname = 'rtao-primitives'\nlicense = '{DERIVED_LICENSE} AND PSF-2.0'\n"
            ),
        )?;
        fs::write(package.join("LICENSE-PSF"), "missing PSF notice\n")?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn attribution_requires_first_line_path_full_commit_and_license() -> Result<()> {
        assert!(attribution("//! Original code.\nconst PREFIX: &str = \"// Derived from RaoFoundation/subtensor \";\n")?.is_none());
        let good =
            format!("{ATTRIBUTION}sdk/python/bittensor/example.py @ {SHA}{ATTRIBUTION_END}\n");
        assert!(attribution(&good)?.is_some());
        assert!(attribution(&format!("//! Later attribution\n{good}")).is_err());
        assert!(attribution(&good.replace(SHA, "deadbeef")).is_err());
        assert!(attribution(&good.replace("Apache-2.0", "MIT")).is_err());
        assert!(
            attribution(&good.replace("sdk/python/bittensor/example.py", "../example.py")).is_err()
        );
        Ok(())
    }
    impl Fixture {
        fn cli_psf(&self) -> Result<()> {
            let package = self.0.join("crates/rtao-cli");
            fs::create_dir_all(package.join("src"))?;
            fs::write(
                package.join("Cargo.toml"),
                include_str!("../tests/fixtures/cli_psf/Cargo.toml"),
            )?;
            let source = if let Some(path) = std::env::var_os("RTAO_CLI_PSF_COMPOSITE_SOURCE") {
                fs::read_to_string(path)?
            } else {
                include_str!("../tests/fixtures/cli_psf/books-provenance.txt").to_owned()
            };
            fs::write(package.join("src/books.rs"), source)?;
            fs::write(
                package.join("LICENSE-PSF"),
                include_bytes!("../../crates/rtao-primitives/LICENSE-PSF"),
            )?;
            let mut notice = if self.0.join("NOTICE").is_file() {
                fs::read_to_string(self.0.join("NOTICE"))?
            } else {
                include_str!("../../NOTICE").to_owned()
            };
            if !notice.contains(cli_psf::NOTICE) {
                notice.push('\n');
                notice.push_str(cli_psf::NOTICE);
            }
            fs::write(self.0.join("NOTICE"), notice)?;
            Ok(())
        }
    }

    #[test]
    fn cli_psf_identified_port_retains_exact_base_and_release_notice() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.cli_psf()?;
        check_licenses(&fixture.0, &[])?;
        let notices: Vec<_> = crate::release::LICENSES
            .iter()
            .filter(|license| license.name == "LICENSE-PSF")
            .collect();
        assert_eq!(notices.len(), 1);
        crate::release::validate_license_bytes(
            notices[0],
            &fs::read(fixture.0.join("crates/rtao-cli/LICENSE-PSF"))?,
        )?;
        Ok(())
    }

    #[test]
    fn cli_psf_missing_or_unsupported_license_clauses_are_rejected() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.cli_psf()?;
        let path = fixture.0.join("crates/rtao-cli/Cargo.toml");
        let original = fs::read_to_string(&path)?;
        for unsupported in [
            DERIVED_LICENSE.to_owned(),
            ORIGINAL_LICENSE.to_owned(),
            format!("{ORIGINAL_LICENSE} AND PSF-2.0"),
            format!("{DERIVED_LICENSE} AND PSF-2.0 AND BSD-3-Clause"),
            format!("{DERIVED_LICENSE} AND PSF-2.0 AND Unicode-DFS-2016"),
        ] {
            let mut manifest: DocumentMut = original.parse()?;
            manifest["package"]["license"] = value(unsupported);
            fs::write(&path, manifest.to_string())?;
            assert!(check_licenses(&fixture.0, &[]).is_err());
        }
        fs::write(&path, original)?;
        check_licenses(&fixture.0, &[])?;
        Ok(())
    }

    #[test]
    fn cli_psf_requires_full_exact_regular_notice_and_retained_root_attribution() -> Result<()> {
        for relative in ["crates/rtao-cli/LICENSE-PSF", "NOTICE"] {
            let fixture = Fixture::new()?;
            fixture.cli_psf()?;
            check_licenses(&fixture.0, &[])?;
            let path = fixture.0.join(relative);
            let original = fs::read(&path)?;
            if relative == "NOTICE" {
                assert_eq!(
                    std::str::from_utf8(&original)?
                        .matches(cli_psf::NOTICE)
                        .count(),
                    1,
                    "fixture must contain exactly one CLI notice for mutation"
                );
            }
            fs::remove_file(&path)?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "missing {relative}"
            );
            fs::write(&path, "PYTHON SOFTWARE FOUNDATION LICENSE VERSION 2\n")?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "truncated {relative}"
            );
            let mut changed = original.clone();
            let changed_at = if relative == "NOTICE" {
                std::str::from_utf8(&original)?
                    .find(cli_psf::NOTICE)
                    .context("CLI NOTICE section")?
            } else {
                0
            };
            changed[changed_at] = b'X';
            fs::write(&path, &changed)?;
            assert!(
                check_licenses(&fixture.0, &[]).is_err(),
                "changed {relative}"
            );
            fs::write(&path, &original)?;
            check_licenses(&fixture.0, &[])?;
            #[cfg(unix)]
            {
                let retained = fixture.0.join("retained-notice");
                fs::rename(&path, &retained)?;
                std::os::unix::fs::symlink(&retained, &path)?;
                assert!(
                    check_licenses(&fixture.0, &[]).is_err(),
                    "symlink {relative}"
                );
            }
        }
        let fixture = Fixture::new()?;
        fixture.cli_psf()?;
        let path = fixture.0.join("NOTICE");
        let original = fs::read_to_string(&path)?;
        for token in [
            "CPython difflib SequenceMatcher",
            "CPython 3.12.12 Lib/difflib.py",
            "4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5",
            "3eb2d371b4a171b063921de6bbf115108bc39de8bf72866315868ce2599213b7",
            "crates/rtao-cli/src/books.rs",
            "crates/rtao-cli/LICENSE-PSF",
            "Modified by rtao:",
        ] {
            fs::write(&path, original.replace(token, "removed"))?;
            assert!(check_licenses(&fixture.0, &[]).is_err(), "NOTICE {token}");
        }
        fs::write(&path, original)?;
        check_licenses(&fixture.0, &[])?;
        Ok(())
    }

    #[test]
    fn cli_psf_rejects_missing_stale_or_detached_source_provenance() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.cli_psf()?;
        let path = fixture.0.join("crates/rtao-cli/src/books.rs");
        let original = fs::read_to_string(&path)?;
        for changed in [
            original.replace(cli_psf::PROVENANCE, ""),
            original.lines().skip(1).collect::<Vec<_>>().join("\n"),
            original.replacen(
                "sdk/python/bittensor/cli/context.py",
                "sdk/python/bittensor/cli/other.py",
                1,
            ),
            original.replace("4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5", OTHER_SHA),
            original.replace(
                "CPython 3.12.12 Lib/difflib.py",
                "CPython 3.12.13 Lib/difflib.py",
            ),
            original.replace("Lib/difflib.py", "Lib/other.py"),
            original.replace(
                cli_psf::PROVENANCE,
                &format!("{}\n{}", cli_psf::PROVENANCE, cli_psf::PROVENANCE),
            ),
            original.replace(
                "pub(crate) fn sequence_ratio(",
                "pub(crate) fn unrelated_ratio(",
            ),
            original.replace(
                cli_psf::PROVENANCE,
                &format!("{}\nconst UNRELATED: bool = true;", cli_psf::PROVENANCE),
            ),
        ] {
            fs::write(&path, changed)?;
            assert!(check_licenses(&fixture.0, &[]).is_err());
        }
        fs::write(&path, original)?;
        check_licenses(&fixture.0, &[])?;
        Ok(())
    }

    #[test]
    fn cli_psf_rejects_unrelated_packages_and_wrong_source_paths() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.cli_psf()?;
        let manifest = fixture.0.join("crates/rtao-cli/Cargo.toml");
        let original = fs::read_to_string(&manifest)?;
        let mut altered: DocumentMut = original.parse()?;
        altered["package"]["name"] = value("unrelated-package");
        fs::write(&manifest, altered.to_string())?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fs::write(&manifest, original)?;
        let books = fixture.0.join("crates/rtao-cli/src/books.rs");
        let moved = fixture.0.join("crates/rtao-cli/src/other.rs");
        fs::rename(&books, &moved)?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fs::rename(&moved, &books)?;
        fixture.license(&format!("{ORIGINAL_LICENSE} AND PSF-2.0"))?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.license(ORIGINAL_LICENSE)?;
        fs::rename(
            fixture.0.join("crates/rtao-cli"),
            fixture.0.join("crates/other-path"),
        )?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        Ok(())
    }

    #[test]
    fn cli_psf_keeps_existing_datetime_and_unicode_license_obligations() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.unicode()?;
        fixture.cli_psf()?;
        let primitive = fixture.0.join("crates/rtao-primitives");
        fs::create_dir_all(primitive.join("src"))?;
        fs::write(
            primitive.join("Cargo.toml"),
            format!("[package]\nname='rtao-primitives'\nlicense='{DERIVED_LICENSE} AND PSF-2.0'\n"),
        )?;
        fs::write(
            primitive.join("src/drand.rs"),
            format!(
                "{ATTRIBUTION}sdk/python/bittensor/timelock.py @ {SHA}{ATTRIBUTION_END}\n// CPython v3.12.12 Modules/_datetimemodule.c (PSF-2.0)\n"
            ),
        )?;
        fs::write(
            primitive.join("LICENSE-PSF"),
            include_bytes!("../../crates/rtao-primitives/LICENSE-PSF"),
        )?;
        check_licenses(&fixture.0, &[])?;
        fs::write(primitive.join("LICENSE-PSF"), "missing legacy PSF notice\n")?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fs::write(
            primitive.join("LICENSE-PSF"),
            include_bytes!("../../crates/rtao-primitives/LICENSE-PSF"),
        )?;
        fixture.unicode_license(DERIVED_LICENSE)?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        fixture.unicode_license(&format!("{DERIVED_LICENSE} AND {UNICODE_LICENSE}"))?;
        check_licenses(&fixture.0, &[])?;
        Ok(())
    }

    #[test]
    fn cli_psf_no_port_keeps_canonical_cli_expression_and_strict_apache_headers() -> Result<()> {
        let fixture = Fixture::new()?;
        let package = fixture.0.join("crates/rtao-cli");
        fs::create_dir_all(package.join("src"))?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname='rtao-cli'\nlicense.workspace=true\n",
        )?;
        fs::write(package.join("src/main.rs"), "fn main() {}\n")?;
        check_licenses(&fixture.0, &[])?;
        fixture.cli_psf()?;
        let books = package.join("src/books.rs");
        let source = fs::read_to_string(&books)?;
        let primary = source.lines().next().context("primary attribution")?;
        let multiline = primary.replace(" @ ", " @\n// ");
        fs::write(&books, source.replacen(primary, &multiline, 1))?;
        assert!(check_licenses(&fixture.0, &[]).is_err());
        assert!(attribution(&fs::read_to_string(books)?).is_err());
        Ok(())
    }
    #[test]
    fn cli_psf_frozen_apache_headers_do_not_change_shared_attribution_grammar() -> Result<()> {
        let headers: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/cli_psf/apache-headers.json"
        ))?;
        for file in ["books.rs", "call_names.rs"] {
            assert!(
                attribution(headers[file].as_str().context("frozen header")?).is_err(),
                "{file}"
            );
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace")?;
        let tree = read_tree(&fs::read_to_string(root.join("vendor/upstream/tree.txt"))?)?;
        for file in ["globals.rs", "prompt.rs", "process.rs", "multisig.rs"] {
            let (path, _) = attribution(headers[file].as_str().context("frozen header")?)?
                .context("syntactic combined header")?;
            assert!(
                !tree.contains_key(path),
                "combined path is not an authenticated single source: {file}: {path}"
            );
        }
        Ok(())
    }
}
