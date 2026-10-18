//! Byte-for-byte imports and offline integrity checks for pinned upstream inputs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

const TREE_KEY: &str = "git:watched-tree";
const TREE_DEST: &str = "vendor/upstream/tree.txt";
const NOTICE_BEGIN: &str = "BEGIN cargo xtask import-upstream attribution\n";
const NOTICE_END: &str = "END cargo xtask import-upstream attribution\n";
const CANONICAL_ABI_COMMIT: &str = "850f2755bb05a3851f948489f0ab27b0645cebce";
const UPSTREAM_URL: &str = "https://github.com/RaoFoundation/subtensor.git";
const SDK_ABI_ROOT: &str = "sdk/python/bittensor/evm/abi/";
const CANONICAL_ABI_ROOT: &str = "precompiles/src/solidity/";
const MAX_CANONICAL_ABIS: usize = 64;
const MAX_CANONICAL_ABI_BYTES: usize = 64 * 1024;
const MAX_CANONICAL_TOTAL_BYTES: usize = 1024 * 1024;
const CURRENT_ABI_STEMS: &[&str] = &[
    "addressMapping",
    "alpha",
    "balance",
    "balanceTransfer",
    "crowdloan",
    "drand",
    "ed25519Verify",
    "leasing",
    "metagraph",
    "neuron",
    "proxy",
    "registry",
    "runtimeConfiguration",
    "scheduler",
    "sr25519Verify",
    "staking",
    "stakingV2",
    "subnet",
    "timestamp",
    "uidLookup",
    "votingPower",
];

/// Exactly the roots specified by architecture §4.9 and the module map.
/// Test package setup files outside these roots are deliberately not watched.
pub const WATCHED_ROOTS: &[&str] = &[
    "sdk/python/bittensor",
    "sdk/python/pyproject.toml",
    "sdk/python/completions",
    "sdk/python/scripts/record_golden.py",
    "sdk/python/scripts/record_shape_corpus.py",
    "sdk/python/scripts/build_basket_index.py",
    "sdk/python/codegen",
    "sdk/python/tests/unit",
    "sdk/python/tests/harness",
    "sdk/python/tests/fixtures",
    "sdk/python/tests/e2e",
    "sdk/bittensor-core",
    "sdk/bittensor-core-py",
    "common/src/transaction_error.rs",
    "common/src/proxy.rs",
    "runtime/src/fee_filters.rs",
    "runtime/src/check_mortality.rs",
    "runtime/src/transaction_payment_wrapper.rs",
    "pallets/subtensor/src/rpc_info",
    "pallets/subtensor/src/coinbase/run_coinbase.rs",
    "pallets/subtensor/src/coinbase/reveal_commits.rs",
    "pallets/subtensor/src/coinbase/tempo_control.rs",
    "pallets/subtensor/src/subnets/serving.rs",
    "pallets/subtensor/src/subnets/weights.rs",
    "pallets/subtensor/src/subnets/subnet.rs",
    "pallets/subtensor/src/utils/misc.rs",
    "pallets/subtensor/src/utils/rate_limiting.rs",
    "pallets/admin-utils/src/lib.rs",
    "docs/guides/signed-requests.mdx",
    "website/apps/bittensor-website/public/catalog/intents.json",
    "website/apps/bittensor-website/public/catalog/reads.json",
    "website/apps/bittensor-website/public/catalog/errors.json",
    "eco-tests",
];

// Constants and pruning provenance are authenticated inputs outside the parity
// file map. Preserve the upstream license alongside these complete source files.
const EXTRA_INPUTS: &[&str] = &[
    "pallets/subtensor/src/lib.rs",
    "pallets/subtensor/src/subnets/registration.rs",
    "pallets/subtensor/src/subnets/uids.rs",
    "LICENSE",
];

#[derive(Clone, Debug)]
struct Artifact {
    to: String,
    sha256: String,
}

#[derive(Clone, Debug)]
struct TreeEntry {
    mode: String,
    blob: String,
}

/// Import from the pin tool's cached clone, or verify committed inputs offline.
pub fn run(args: &[String]) -> Result<()> {
    let mut verify_only = false;
    let mut canonical_only = false;
    let mut explicit_cache = false;
    let mut cache = std::env::var_os("RTAO_UPSTREAM_CACHE").map_or_else(
        || {
            std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .map_or_else(
                    || PathBuf::from("target/upstream.git"),
                    |root| root.join("pin/upstream.git"),
                )
        },
        PathBuf::from,
    );
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--verify-only" => verify_only = true,
            "--canonical-abis" => canonical_only = true,
            "--cache" => {
                cache = PathBuf::from(iter.next().context("--cache requires a path")?);
                explicit_cache = true;
            }
            "--help" | "-h" => {
                println!(
                    "cargo xtask import-upstream [--cache <clone>] [--verify-only]\n\
                     cargo xtask import-upstream --canonical-abis --cache <exact-pin-bare-cache>\n\n\
                     Imports parity.commit from the pin cache. Verification reads only committed files.\n\
                     Canonical-only import accepts the reviewed v478 pin and preserves existing artifacts."
                );
                return Ok(());
            }
            _ => bail!("unknown import-upstream argument {arg:?}"),
        }
    }
    let root = std::env::current_dir()?.canonicalize()?;
    if canonical_only {
        ensure!(
            explicit_cache && !verify_only,
            "--canonical-abis requires explicit --cache and cannot use --verify-only"
        );
        let count = import_canonical_abis(&root, &cache, CANONICAL_ABI_COMMIT, CURRENT_ABI_STEMS)?;
        println!("Imported {count} canonical ABI artifacts");
        return Ok(());
    }
    let count = if verify_only {
        verify(&root)?
    } else {
        import(&root, &cache)?
    };
    println!(
        "{} {count} upstream artifacts",
        if verify_only { "Verified" } else { "Imported" }
    );
    Ok(())
}

fn manifest(root: &Path) -> Result<DocumentMut> {
    checked_path(root, "upstream.toml")?;
    fs::read_to_string(root.join("upstream.toml"))?
        .parse()
        .context("parse upstream.toml")
}

fn parity_commit(doc: &DocumentMut) -> Result<&str> {
    let commit = doc["parity"]["commit"]
        .as_str()
        .context("[parity].commit is missing")?;
    ensure!(
        is_hex(commit, 40),
        "parity.commit must be a full lowercase git commit SHA"
    );
    Ok(commit)
}

fn artifacts(doc: &DocumentMut, need_hash: bool) -> Result<BTreeMap<String, Artifact>> {
    let table = doc["artifacts"]
        .as_table()
        .context("[artifacts] is missing")?;
    let mut parsed = BTreeMap::new();
    let mut destinations = BTreeSet::new();
    for (source, item) in table.iter() {
        if source != TREE_KEY {
            relative_path(source)?;
        }
        let to = item
            .get("to")
            .and_then(Item::as_str)
            .context("artifact.to is missing")?;
        relative_path(to)?;
        ensure!(
            to.starts_with("vendor/upstream/"),
            "artifact destination must be under vendor/upstream: {to}"
        );
        ensure!(
            to != "vendor/upstream/extracted" && !to.starts_with("vendor/upstream/extracted/"),
            "extracted outputs are owned by xtask extract, not import-upstream"
        );
        ensure!(
            destinations.insert(to.to_owned()),
            "duplicate artifact destination {to}"
        );
        let sha256 = item
            .get("sha256")
            .and_then(Item::as_str)
            .unwrap_or("")
            .to_owned();
        ensure!(
            !need_hash || is_hex(&sha256, 64),
            "invalid or missing artifact sha256 for {source}"
        );
        parsed.insert(
            source.to_owned(),
            Artifact {
                to: to.to_owned(),
                sha256,
            },
        );
    }
    if let Some(tree) = parsed.get(TREE_KEY) {
        ensure!(
            tree.to == TREE_DEST,
            "watched tree must be copied to {TREE_DEST}"
        );
    }
    Ok(parsed)
}

fn import(root: &Path, cache: &Path) -> Result<usize> {
    let mut doc = manifest(root)?;
    let commit = parity_commit(&doc)?.to_owned();
    let mut imports = artifacts(&doc, false)?;
    let mut args = vec!["ls-tree", "-r", "--full-tree", &commit, "--"];
    args.extend_from_slice(WATCHED_ROOTS);
    let tree_bytes = git(cache, &args)?;
    let tree = parse_tree(&tree_bytes)?;
    ensure!(!tree.is_empty(), "pinned watched tree is empty");
    let canonical = canonical_abi_inputs(&tree)?;

    // Keep canonical artifact destinations supplied by pin. Every other watched
    // input is retained under src, so Rust-native extraction never needs Python
    // or the cached git repository on an ordinary push.
    for source in tree
        .keys()
        .map(String::as_str)
        .chain(EXTRA_INPUTS.iter().copied())
        .chain(canonical.values().map(String::as_str))
    {
        imports
            .entry(source.to_owned())
            .or_insert_with(|| Artifact {
                to: format!("vendor/upstream/src/{source}"),
                sha256: String::new(),
            });
    }
    imports.insert(
        TREE_KEY.to_owned(),
        Artifact {
            to: TREE_DEST.to_owned(),
            sha256: String::new(),
        },
    );
    validate_scope(&imports, &tree)?;
    let mut extra_args = vec!["ls-tree", "-r", "--full-tree", &commit, "--"];
    extra_args.extend_from_slice(EXTRA_INPUTS);
    extra_args.extend(canonical.values().map(String::as_str));
    let extra = parse_tree_unscoped(&git(cache, &extra_args)?)?;
    ensure!(
        extra.len() == EXTRA_INPUTS.len() + canonical.len()
            && EXTRA_INPUTS.iter().all(|path| extra.contains_key(*path))
            && canonical.values().all(|path| extra.contains_key(path)),
        "required extraction/attribution inputs are absent or not regular files"
    );
    prefetch_blobs(cache, tree.values().chain(extra.values()))?;

    // Validate and fetch everything before touching the committed copies.
    let mut payloads = BTreeMap::new();
    for (source, artifact) in &mut imports {
        checked_path(root, &artifact.to)?;
        let bytes = if source == TREE_KEY {
            tree_bytes.clone()
        } else {
            let object = format!("{commit}:{source}");
            git(cache, &["show", &object])?
        };
        if let Some(entry) = tree.get(source).or_else(|| extra.get(source)) {
            ensure!(
                blob_sha(&bytes) == entry.blob,
                "upstream blob mismatch for {source}"
            );
        }
        artifact.sha256 = sha256(&bytes);
        payloads.insert(source.clone(), bytes);
    }
    let destination_set = imports.values().map(|a| a.to.clone()).collect();
    ensure_no_extra_copies(root, &destination_set)?;
    let notice = notice(root, &commit, &imports)?;

    for (source, artifact) in &imports {
        let bytes = payloads
            .get(source)
            .context("missing staged artifact bytes")?;
        write_if_changed(root, &artifact.to, bytes)?;
    }
    let mut table = Table::new();
    table.set_dotted(false);
    for (source, artifact) in &imports {
        let mut value = InlineTable::new();
        value.insert("to", Value::from(artifact.to.clone()));
        value.insert("sha256", Value::from(artifact.sha256.clone()));
        table.insert(source, Item::Value(Value::InlineTable(value)));
    }
    doc["artifacts"] = Item::Table(table);
    write_if_changed(root, "upstream.toml", doc.to_string().as_bytes())?;
    write_if_changed(root, "NOTICE", notice.as_bytes())?;
    verify(root)
}

fn verify(root: &Path) -> Result<usize> {
    verify_with_canonical(root, true)
}

fn verify_with_canonical(root: &Path, require_canonical: bool) -> Result<usize> {
    let doc = manifest(root)?;
    let commit = parity_commit(&doc)?;
    let imports = artifacts(&doc, true)?;
    let tree_artifact = imports
        .get(TREE_KEY)
        .context("watched tree artifact is missing; run import-upstream")?;
    checked_path(root, &tree_artifact.to)?;
    let tree = parse_tree(&fs::read(root.join(&tree_artifact.to))?)?;
    ensure!(!tree.is_empty(), "watched tree is empty");
    validate_scope_with_canonical(&imports, &tree, require_canonical)?;
    for (source, artifact) in &imports {
        checked_path(root, &artifact.to)?;
        let bytes = fs::read(root.join(&artifact.to))
            .with_context(|| format!("read artifact {source} -> {}", artifact.to))?;
        ensure!(
            sha256(&bytes) == artifact.sha256,
            "sha256 mismatch for {source} -> {}",
            artifact.to
        );
        if let Some(entry) = tree.get(source) {
            ensure!(
                blob_sha(&bytes) == entry.blob,
                "git blob mismatch for {source}; tree.txt and copied bytes disagree"
            );
        }
    }
    let destination_set = imports.values().map(|a| a.to.clone()).collect();
    ensure_no_extra_copies(root, &destination_set)?;
    checked_path(root, "NOTICE")?;
    let actual_notice = fs::read_to_string(root.join("NOTICE"))?;
    ensure!(
        actual_notice == notice(root, commit, &imports)?,
        "NOTICE import attribution is stale"
    );
    Ok(imports.len())
}

fn validate_scope(
    imports: &BTreeMap<String, Artifact>,
    tree: &BTreeMap<String, TreeEntry>,
) -> Result<()> {
    validate_scope_with_canonical(imports, tree, true)
}

fn validate_scope_with_canonical(
    imports: &BTreeMap<String, Artifact>,
    tree: &BTreeMap<String, TreeEntry>,
    require_canonical: bool,
) -> Result<()> {
    let canonical: BTreeSet<_> = canonical_abi_inputs(tree)?.into_values().collect();
    let mut destinations = BTreeSet::new();
    for artifact in imports.values() {
        ensure!(
            destinations.insert(artifact.to.clone()),
            "duplicate artifact destination {}",
            artifact.to
        );
    }
    for destination in &destinations {
        let mut parent = destination.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            ensure!(
                !destinations.contains(prefix),
                "artifact destination is an ancestor of another destination: {prefix}"
            );
            parent = prefix;
        }
    }
    // Every whole watched root must have at least one entry; all individually
    // watched source files must be present. This also catches an accidentally
    // truncated listing before any import mutates committed files.
    for root in WATCHED_ROOTS {
        ensure!(
            tree.keys().any(|path| path == root
                || path
                    .strip_prefix(root)
                    .is_some_and(|suffix| suffix.starts_with('/'))),
            "watched root has vanished or is missing from tree.txt: {root}"
        );
    }
    for path in tree.keys() {
        let artifact = imports
            .get(path)
            .with_context(|| format!("watched source has no artifact: {path}"))?;
        if !is_asset(path) {
            ensure!(
                artifact.to == format!("vendor/upstream/src/{path}"),
                "source input must be copied under vendor/upstream/src: {path}"
            );
        }
    }
    for path in EXTRA_INPUTS {
        let artifact = imports
            .get(*path)
            .with_context(|| format!("required extraction/attribution input is missing: {path}"))?;
        ensure!(
            artifact.to == format!("vendor/upstream/src/{path}"),
            "required input destination differs: {path}"
        );
    }
    for path in &canonical {
        if !require_canonical && !imports.contains_key(path) {
            continue;
        }
        let artifact = imports
            .get(path)
            .with_context(|| format!("required canonical ABI input is missing: {path}"))?;
        ensure!(
            artifact.to == format!("vendor/upstream/src/{path}"),
            "canonical ABI destination differs: {path}"
        );
    }
    for source in imports.keys() {
        ensure!(
            source == TREE_KEY
                || tree.contains_key(source)
                || EXTRA_INPUTS.contains(&source.as_str())
                || canonical.contains(source),
            "artifact outside declared watched/extraction scope: {source}"
        );
    }
    Ok(())
}

fn canonical_abi_inputs(tree: &BTreeMap<String, TreeEntry>) -> Result<BTreeMap<String, String>> {
    let mut inputs = BTreeMap::new();
    for source in tree.keys() {
        let Some(file) = source.strip_prefix(SDK_ABI_ROOT) else {
            continue;
        };
        if file.contains('/') || !file.ends_with(".json") {
            continue;
        }
        let stem = file.strip_suffix(".json").context("ABI suffix")?;
        ensure!(!stem.is_empty(), "SDK ABI stem is empty");
        inputs.insert(source.clone(), format!("{CANONICAL_ABI_ROOT}{stem}.abi"));
    }
    ensure!(
        inputs.len() <= MAX_CANONICAL_ABIS,
        "canonical ABI inventory exceeds bound"
    );
    Ok(inputs)
}

// Candidate preparation never changes a pin or refetches the watched source
// inventory. A hosted caller supplies a shallow, blob-filtered bare cache of
// this exact commit; only the independently named canonical blobs are read.
fn import_canonical_abis(
    root: &Path,
    cache: &Path,
    expected_commit: &str,
    expected_stems: &[&str],
) -> Result<usize> {
    let mut doc = manifest(root)?;
    let commit = parity_commit(&doc)?.to_owned();
    ensure!(
        commit == expected_commit,
        "canonical-only import requires the reviewed exact parity pin"
    );
    ensure!(
        !fs::symlink_metadata(cache)?.file_type().is_symlink()
            && git(cache, &["rev-parse", "--is-bare-repository"])? == b"true\n"
            && git(cache, &["remote", "get-url", "origin"])?
                == format!("{UPSTREAM_URL}\n").as_bytes(),
        "canonical-only cache must be bare with the exact pinned upstream origin"
    );
    ensure!(
        git(
            cache,
            &["rev-parse", "--verify", &format!("{commit}^{{commit}}")]
        )? == format!("{commit}\n").as_bytes()
            && git(cache, &["rev-parse", "--verify", "FETCH_HEAD"])?
                == format!("{commit}\n").as_bytes(),
        "canonical-only cache must have fetched the exact parity commit"
    );
    // This migration-only baseline accepts missing canonical additions, while
    // still verifying every existing source/hash/destination and NOTICE. The
    // ordinary verifier below requires the complete canonical inventory.
    verify_with_canonical(root, false)?;
    let tree_bytes = fs::read(root.join(TREE_DEST))?;
    let tree = parse_tree(&tree_bytes)?;
    let mut watched_args = vec!["ls-tree", "-r", "--full-tree", &commit, "--"];
    watched_args.extend_from_slice(WATCHED_ROOTS);
    ensure!(
        git(cache, &watched_args)? == tree_bytes,
        "canonical-only cache watched tree differs from committed source identity"
    );
    let canonical = canonical_abi_inputs(&tree)?;
    let actual: BTreeSet<_> = canonical
        .keys()
        .map(|source| source.strip_prefix(SDK_ABI_ROOT).unwrap_or(source.as_str()))
        .collect();
    let expected: BTreeSet<_> = expected_stems
        .iter()
        .map(|stem| format!("{stem}.json"))
        .collect();
    ensure!(
        actual.len() == expected.len()
            && actual
                .iter()
                .copied()
                .eq(expected.iter().map(String::as_str)),
        "canonical-only SDK ABI inventory differs from the reviewed exact pin"
    );
    let mut canonical_args = vec!["ls-tree", "-r", "--full-tree", &commit, "--"];
    canonical_args.extend(canonical.values().map(String::as_str));
    let canonical_tree = parse_tree_unscoped(&git(cache, &canonical_args)?)?;
    ensure!(
        canonical_tree.len() == canonical.len()
            && canonical
                .values()
                .all(|path| canonical_tree.contains_key(path)),
        "canonical counterparts are absent or not regular blobs"
    );
    prefetch_blobs(cache, canonical_tree.values())?;
    let mut imports = artifacts(&doc, true)?;
    let mut payloads = BTreeMap::new();
    let mut total = 0usize;
    for (sdk_source, source) in canonical {
        let entry = canonical_tree
            .get(&source)
            .context("canonical tree entry")?;
        let bytes = git(cache, &["show", &format!("{commit}:{source}")])?;
        total = total
            .checked_add(bytes.len())
            .context("canonical ABI size overflow")?;
        ensure!(
            bytes.len() <= MAX_CANONICAL_ABI_BYTES && total <= MAX_CANONICAL_TOTAL_BYTES,
            "canonical ABI bytes exceed candidate bounds"
        );
        ensure!(
            blob_sha(&bytes) == entry.blob,
            "canonical input differs from exact-pin Git blob: {source}"
        );
        let sdk = imports.get(&sdk_source).context("SDK ABI artifact")?;
        let sdk_bytes = fs::read(root.join(&sdk.to))?;
        let sdk_json: serde_json::Value = serde_json::from_slice(&sdk_bytes)?;
        let canonical_json: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            sdk_json.is_array() && canonical_json.is_array() && sdk_json == canonical_json,
            "SDK ABI differs from canonical input: {sdk_source}"
        );
        let destination = format!("vendor/upstream/src/{source}");
        checked_path(root, &destination)?;
        let artifact = Artifact {
            to: destination,
            sha256: sha256(&bytes),
        };
        if let Some(existing) = imports.get(&source) {
            ensure!(
                existing.to == artifact.to && existing.sha256 == artifact.sha256,
                "canonical-only import would replace an existing artifact: {source}"
            );
        }
        imports.insert(source.clone(), artifact);
        payloads.insert(source, bytes);
    }
    validate_scope(&imports, &tree)?;
    let destinations = imports
        .values()
        .map(|artifact| artifact.to.clone())
        .collect();
    ensure_no_extra_copies(root, &destinations)?;
    let generated_notice = notice(root, &commit, &imports)?;
    let table = doc["artifacts"].as_table_mut().context("artifacts")?;
    for source in payloads.keys() {
        if table.contains_key(source) {
            continue;
        }
        let artifact = imports.get(source).context("staged canonical artifact")?;
        let mut row = InlineTable::new();
        row.insert("to", Value::from(artifact.to.clone()));
        row.insert("sha256", Value::from(artifact.sha256.clone()));
        table.insert(source, Item::Value(Value::InlineTable(row)));
    }
    for (source, bytes) in &payloads {
        write_if_changed(
            root,
            &imports.get(source).context("canonical artifact")?.to,
            bytes,
        )?;
    }
    write_if_changed(root, "upstream.toml", doc.to_string().as_bytes())?;
    write_if_changed(root, "NOTICE", generated_notice.as_bytes())?;
    verify(root)?;
    Ok(payloads.len())
}

fn is_asset(path: &str) -> bool {
    path.starts_with("website/apps/bittensor-website/public/catalog/")
        || path.starts_with("sdk/python/tests/fixtures/")
        || path.starts_with("sdk/python/bittensor/evm/abi/")
        || path.starts_with("sdk/python/bittensor/vault/static/")
        || path.starts_with("sdk/python/bittensor/extension/static/")
        || path == "sdk/bittensor-core/fixtures/ledger_proof_vector.json"
        || path == "sdk/bittensor-core/tests/e2e-manifest.json"
        || path == "docs/guides/signed-requests.mdx"
}

fn watched(path: &str) -> bool {
    WATCHED_ROOTS.iter().any(|root| {
        path == *root
            || path
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn parse_tree(bytes: &[u8]) -> Result<BTreeMap<String, TreeEntry>> {
    let tree = parse_tree_unscoped(bytes)?;
    for path in tree.keys() {
        ensure!(
            watched(path),
            "tree.txt contains a path outside watched roots: {path}"
        );
    }
    Ok(tree)
}

fn parse_tree_unscoped(bytes: &[u8]) -> Result<BTreeMap<String, TreeEntry>> {
    let text = std::str::from_utf8(bytes).context("tree.txt is not UTF-8")?;
    ensure!(
        text.ends_with('\n') || text.is_empty(),
        "tree.txt must end in a newline"
    );
    let mut tree = BTreeMap::new();
    let mut last: Option<&str> = None;
    for line in text.lines() {
        let (header, path) = line
            .split_once('\t')
            .context("tree.txt row must have a tab-separated path")?;
        relative_path(path)?;
        let fields: Vec<_> = header.split(' ').collect();
        ensure!(fields.len() == 3, "malformed tree row for {path}");
        ensure!(
            matches!(fields[0], "100644" | "100755") && fields[1] == "blob",
            "tree path must be a regular blob, not a symlink or submodule: {path}"
        );
        ensure!(is_hex(fields[2], 40), "invalid git blob SHA for {path}");
        ensure!(
            last.is_none_or(|previous| previous < path),
            "tree.txt rows must be sorted and unique: {path}"
        );
        last = Some(path);
        tree.insert(
            path.to_owned(),
            TreeEntry {
                mode: fields[0].to_owned(),
                blob: fields[2].to_owned(),
            },
        );
    }
    // Require canonical ls-tree spelling rather than accepting normalized input.
    let canonical: String = tree
        .iter()
        .map(|(path, entry)| format!("{} blob {}\t{path}\n", entry.mode, entry.blob))
        .collect();
    ensure!(
        canonical.as_bytes() == bytes,
        "tree.txt is not canonical git ls-tree output"
    );
    Ok(tree)
}

fn relative_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains(['\\', ':', '\t', '\n', '\r', '\0']),
        "unsafe relative path: {path:?}"
    );
    ensure!(
        path.split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "unsafe relative path: {path:?}"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "path is not relative: {path:?}"
    );
    Ok(())
}

fn checked_path(root: &Path, path: &str) -> Result<PathBuf> {
    relative_path(path)?;
    let mut current = root.to_path_buf();
    let count = path.split('/').count();
    for (index, part) in path.split('/').enumerate() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "symlink in import path: {}",
                    current.display()
                );
                ensure!(
                    if index + 1 == count {
                        metadata.is_file()
                    } else {
                        metadata.is_dir()
                    },
                    "non-regular import path: {}",
                    current.display()
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", current.display()));
            }
        }
    }
    Ok(current)
}

fn ensure_no_extra_copies(root: &Path, expected: &BTreeSet<String>) -> Result<()> {
    let vendor = root.join("vendor/upstream");
    if !vendor.exists() {
        return Ok(());
    }
    let mut pending = vec![vendor];
    while let Some(directory) = pending.pop() {
        let metadata = fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "non-directory or symlink in vendor/upstream"
        );
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.path().symlink_metadata()?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "symlink in vendor/upstream: {}",
                entry.path().display()
            );
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(metadata.is_file(), "non-regular file in vendor/upstream");
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF8 vendored path")?;
                // Generated tables are checked independently by xtask extract --check.
                ensure!(
                    expected.contains(relative)
                        || relative.starts_with("vendor/upstream/extracted/"),
                    "untied vendored copy: {relative}"
                );
            }
        }
    }
    Ok(())
}

fn git(cache: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cache)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "run git in {}; populate it with xtask pin first",
                cache.display()
            )
        })?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn prefetch_blobs<'a>(cache: &Path, entries: impl Iterator<Item = &'a TreeEntry>) -> Result<()> {
    let config = Command::new("git")
        .arg("-C")
        .arg(cache)
        .args(["config", "--bool", "remote.origin.promisor"])
        .output()?;
    if !config.status.success() || config.stdout != b"true\n" {
        return Ok(());
    }
    // Fetch promised blobs together. Reading each missing blob separately from a
    // partial clone otherwise starts hundreds of network requests on a cold pin.
    // Explicit wants are retained even with blob:none; refs/FETCH_HEAD are untouched.
    let blobs: BTreeSet<_> = entries.map(|entry| entry.blob.as_str()).collect();
    let mut args = vec![
        "fetch",
        "--no-tags",
        "--no-write-fetch-head",
        "--filter=blob:none",
        "origin",
    ];
    args.extend(blobs.iter().copied());
    if git(cache, &args).is_ok() {
        return Ok(());
    }
    // Newer Git rejects blob-only wants in its post-fetch connectivity check.
    // Resolve them through the promisor remote instead, then require every one.
    lazy_fetch_blobs(cache, &blobs)
}

fn lazy_fetch_blobs(cache: &Path, blobs: &BTreeSet<&str>) -> Result<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cache)
        .args(["cat-file", "--batch-check"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("run git cat-file for promised blobs")?;
    let mut input = child.stdin.take().context("cat-file stdin unavailable")?;
    let request: String = blobs.iter().map(|blob| format!("{blob}\n")).collect();
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        input.write_all(request.as_bytes())
    });
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("cat-file writer panicked"))??;
    ensure!(
        output.status.success(),
        "git cat-file --batch-check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved = String::from_utf8(output.stdout)?;
    let kinds: BTreeSet<_> = resolved
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next(), fields.next())
        })
        .collect();
    for blob in blobs {
        ensure!(
            kinds.contains(&(Some(*blob), Some("blob"))),
            "promised blob {blob} is unavailable from the upstream remote"
        );
    }
    Ok(())
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn blob_sha(bytes: &[u8]) -> String {
    let mut sha = Sha1::new();
    sha.update(format!("blob {}\0", bytes.len()).as_bytes());
    sha.update(bytes);
    format!("{:x}", sha.finalize())
}

fn write_if_changed(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    let path = checked_path(root, relative)?;
    if fs::read(&path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().context("import path has no parent")?)?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
}

fn notice(root: &Path, commit: &str, imports: &BTreeMap<String, Artifact>) -> Result<String> {
    checked_path(root, "NOTICE")?;
    let original = fs::read_to_string(root.join("NOTICE"))?;
    let mut prefix = if let Some((prefix, remainder)) = original.split_once(NOTICE_BEGIN) {
        ensure!(
            remainder.contains(NOTICE_END),
            "incomplete generated NOTICE attribution"
        );
        ensure!(
            remainder.ends_with(NOTICE_END),
            "unexpected text after generated NOTICE attribution"
        );
        prefix.to_owned()
    } else {
        original
    };
    let placeholder = "Vendored assets and their source commits and hashes will be listed here when\nimported by cargo xtask import-upstream. No upstream assets have been vendored\nat this step.\n";
    prefix = prefix.replace(placeholder, "");
    if !prefix.ends_with("\n\n") {
        prefix.push('\n');
    }
    prefix.push_str(NOTICE_BEGIN);
    prefix.push_str(&format!("Copied byte for byte from RaoFoundation/subtensor @ {commit}\nhttps://github.com/RaoFoundation/subtensor\nLicense: Apache-2.0 (upstream LICENSE preserved in vendor/upstream/src/LICENSE).\nAll watched files are retained for offline checks and Rust-native extraction;\nthe additional pallet lib.rs contains extraction inputs excluded from the file map.\nGenerated tree.txt records the pinned watched-tree listing.\n\nSource -> copied path; SHA-256\n"));
    for (source, artifact) in imports {
        prefix.push_str(&format!(
            "{source} -> {}; {}\n",
            artifact.to, artifact.sha256
        ));
    }
    prefix.push_str(NOTICE_END);
    Ok(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        cache: PathBuf,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let base = std::env::var_os("RTAO_IMPORT_TEST_ROOT").map_or_else(
                || {
                    std::env::var_os("RTAO_TEMP_ROOT")
                        .map(PathBuf::from)
                        .map_or_else(
                            || {
                                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                    .join("../target/xtask-import-tests")
                            },
                            |root| root.join("import-upstream/tests"),
                        )
                },
                PathBuf::from,
            );
            fs::create_dir_all(&base)?;
            let root = base.join(format!(
                "fixture-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root)?;
            let cache = root.join("upstream.git");
            fs::create_dir(&cache)?;
            git(&cache, &["init", "--initial-branch=main"])?;
            write_if_changed(
                &cache,
                "sdk/python/bittensor/settings.py",
                b"RAO_PER_TAO = 10**9\n",
            )?;
            for watched in WATCHED_ROOTS
                .iter()
                .copied()
                .filter(|root| *root != "sdk/python/bittensor")
            {
                let path = if watched
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.contains('.'))
                {
                    watched.to_owned()
                } else {
                    format!("{watched}/fixture.py")
                };
                write_if_changed(&cache, &path, b"fixture\n")?;
            }
            write_if_changed(
                &cache,
                "pallets/subtensor/src/lib.rs",
                b"pub const MAX: u16 = 4095;\n",
            )?;
            write_if_changed(
                &cache,
                "pallets/subtensor/src/subnets/registration.rs",
                b"pub fn get_pruning_uid() {}\n",
            )?;
            write_if_changed(
                &cache,
                "pallets/subtensor/src/subnets/uids.rs",
                b"pub fn get_pruning_score() {}\n",
            )?;
            write_if_changed(&cache, "LICENSE", b"Apache License, Version 2.0\n")?;
            git(&cache, &["add", "."])?;
            git(
                &cache,
                &[
                    "-c",
                    "user.name=rtao test",
                    "-c",
                    "user.email=test@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    "fixture",
                ],
            )?;
            let commit = String::from_utf8(git(&cache, &["rev-parse", "HEAD"])?)?;
            write_if_changed(
                &root,
                "upstream.toml",
                format!("[parity]\ncommit = {:?}\n\n[artifacts]\n", commit.trim()).as_bytes(),
            )?;
            write_if_changed(
                &root,
                "NOTICE",
                b"rtao\n\nApache-2.0 upstream attribution.\n",
            )?;
            Ok(Self { root, cache })
        }

        fn import(&self) -> Result<usize> {
            import(&self.root, &self.cache)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Fixture git subprocesses are synchronous, no daemon/venv is created,
            // and every path is confined to this exact task-scoped fixture root.
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn canonical_fixture(drift: bool) -> Result<(Fixture, PathBuf, String)> {
        let fixture = Fixture::new()?;
        let sdk = br#"[{"type":"function","name":"alpha","inputs":[],"outputs":[]}]"#;
        let canonical = if drift {
            br#"[{"type":"function","name":"changed","inputs":[],"outputs":[]}]"#.as_slice()
        } else {
            sdk.as_slice()
        };
        write_if_changed(&fixture.cache, &format!("{SDK_ABI_ROOT}alpha.json"), sdk)?;
        write_if_changed(
            &fixture.cache,
            &format!("{CANONICAL_ABI_ROOT}alpha.abi"),
            canonical,
        )?;
        git(&fixture.cache, &["add", "."])?;
        git(
            &fixture.cache,
            &[
                "-c",
                "user.name=rtao test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "canonical fixture",
            ],
        )?;
        let commit = String::from_utf8(git(&fixture.cache, &["rev-parse", "HEAD"])?)?
            .trim()
            .to_owned();
        let mut doc = manifest(&fixture.root)?;
        doc["parity"]["commit"] = toml_edit::value(&commit);
        fs::write(fixture.root.join("upstream.toml"), doc.to_string())?;
        fixture.import()?;
        // Reconstruct the previously valid manifest without the newly required
        // independent input. All baseline watched files and hashes stay intact.
        let canonical_source = format!("{CANONICAL_ABI_ROOT}alpha.abi");
        let mut imports = artifacts(&manifest(&fixture.root)?, true)?;
        let removed = imports
            .remove(&canonical_source)
            .context("canonical fixture artifact")?;
        fs::remove_file(fixture.root.join(removed.to))?;
        let mut legacy = manifest(&fixture.root)?;
        legacy["artifacts"]
            .as_table_mut()
            .context("artifacts")?
            .remove(&canonical_source);
        fs::write(fixture.root.join("upstream.toml"), legacy.to_string())?;
        fs::write(
            fixture.root.join("NOTICE"),
            notice(&fixture.root, &commit, &imports)?,
        )?;
        verify_with_canonical(&fixture.root, false)?;
        ensure!(
            verify(&fixture.root).is_err(),
            "strict verification accepted missing canonical input"
        );
        let bare = fixture.root.join("canonical-cache.git");
        git(
            &fixture.cache,
            &[
                "clone",
                "--bare",
                ".",
                bare.to_str().context("fixture cache path")?,
            ],
        )?;
        git(&bare, &["remote", "set-url", "origin", UPSTREAM_URL])?;
        // Local-only fixture fetch establishes FETCH_HEAD without contacting
        // the pinned public origin or starting a service.
        git(
            &bare,
            &[
                "fetch",
                "--no-tags",
                fixture.cache.to_str().context("fixture source path")?,
                &commit,
            ],
        )?;
        Ok((fixture, bare, commit))
    }

    #[test]
    fn canonical_abis_import_preserves_existing_pins_and_artifacts() -> Result<()> {
        let (fixture, cache, commit) = canonical_fixture(false)?;
        let before = manifest(&fixture.root)?;
        let previous: BTreeMap<_, _> = artifacts(&before, true)?
            .into_iter()
            .map(|(source, artifact)| {
                let bytes = fs::read(fixture.root.join(&artifact.to))?;
                Ok((source, (artifact.to, bytes)))
            })
            .collect::<Result<_>>()?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"])? == 1);
        let after = manifest(&fixture.root)?;
        for (section, value) in before.iter().filter(|(key, _)| *key != "artifacts") {
            assert_eq!(value.to_string(), after[section].to_string());
        }
        for (source, (destination, bytes)) in previous {
            assert_eq!(
                before["artifacts"][&source].to_string(),
                after["artifacts"][&source].to_string()
            );
            assert_eq!(bytes, fs::read(fixture.root.join(destination))?);
        }
        let manifest_bytes = fs::read(fixture.root.join("upstream.toml"))?;
        let notice_bytes = fs::read(fixture.root.join("NOTICE"))?;
        import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"])?;
        assert_eq!(
            manifest_bytes,
            fs::read(fixture.root.join("upstream.toml"))?
        );
        assert_eq!(notice_bytes, fs::read(fixture.root.join("NOTICE"))?);
        verify(&fixture.root)?;
        Ok(())
    }

    #[test]
    fn canonical_abis_import_refuses_wrong_pin_cache_inventory_and_existing_changes() -> Result<()>
    {
        let (fixture, cache, commit) = canonical_fixture(false)?;
        let before = fs::read(fixture.root.join("upstream.toml"))?;
        let notice_before = fs::read(fixture.root.join("NOTICE"))?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &"0".repeat(40), &["alpha"]).is_err());
        git(
            &cache,
            &[
                "remote",
                "set-url",
                "origin",
                "https://example.invalid/subtensor.git",
            ],
        )?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"]).is_err());
        git(&cache, &["remote", "set-url", "origin", UPSTREAM_URL])?;
        let fetch_head = fs::read(cache.join("FETCH_HEAD"))?;
        fs::write(cache.join("FETCH_HEAD"), format!("{}\n", "0".repeat(40)))?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"]).is_err());
        fs::write(cache.join("FETCH_HEAD"), fetch_head)?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &commit, &["missing"]).is_err());
        let doc = manifest(&fixture.root)?;
        let imported = artifacts(&doc, true)?;
        let sdk = imported
            .get(&format!("{SDK_ABI_ROOT}alpha.json"))
            .context("SDK artifact")?;
        fs::write(fixture.root.join(&sdk.to), b"[]")?;
        ensure!(import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"]).is_err());
        assert_eq!(before, fs::read(fixture.root.join("upstream.toml"))?);
        assert_eq!(notice_before, fs::read(fixture.root.join("NOTICE"))?);
        ensure!(
            !fixture
                .root
                .join(format!("vendor/upstream/src/{CANONICAL_ABI_ROOT}alpha.abi"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn canonical_abis_import_refuses_drift_before_publishing() -> Result<()> {
        let (fixture, cache, commit) = canonical_fixture(true)?;
        let before = fs::read(fixture.root.join("upstream.toml"))?;
        let notice_before = fs::read(fixture.root.join("NOTICE"))?;
        let error = import_canonical_abis(&fixture.root, &cache, &commit, &["alpha"])
            .err()
            .context("canonical content drift was accepted")?;
        ensure!(
            error.to_string().contains("SDK ABI differs from canonical"),
            "wrong failure: {error:#}"
        );
        assert_eq!(before, fs::read(fixture.root.join("upstream.toml"))?);
        assert_eq!(notice_before, fs::read(fixture.root.join("NOTICE"))?);
        ensure!(
            !fixture
                .root
                .join(format!("vendor/upstream/src/{CANONICAL_ABI_ROOT}alpha.abi"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn imports_are_deterministic_and_verify_without_the_git_cache() -> Result<()> {
        let fixture = Fixture::new()?;
        let expected_count = WATCHED_ROOTS.len() + EXTRA_INPUTS.len() + 1;
        ensure!(fixture.import()? == expected_count);
        let manifest = fs::read(fixture.root.join("upstream.toml"))?;
        let notice = fs::read(fixture.root.join("NOTICE"))?;
        let tree = fs::read(fixture.root.join(TREE_DEST))?;
        fixture.import()?;
        assert_eq!(manifest, fs::read(fixture.root.join("upstream.toml"))?);
        assert_eq!(notice, fs::read(fixture.root.join("NOTICE"))?);
        assert_eq!(tree, fs::read(fixture.root.join(TREE_DEST))?);
        fs::rename(&fixture.cache, fixture.root.join("offline-cache"))?;
        assert_eq!(verify(&fixture.root)?, expected_count);
        Ok(())
    }

    #[test]
    fn offline_verification_rejects_source_tree_and_manifest_tampering() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.import()?;
        let source = "vendor/upstream/src/sdk/python/bittensor/settings.py";
        fs::write(fixture.root.join(source), b"RAO_PER_TAO = 1\n")?;
        ensure!(
            verify(&fixture.root).is_err(),
            "tampered source was accepted"
        );
        fixture.import()?;
        let mut tree = fs::read(fixture.root.join(TREE_DEST))?;
        tree.extend_from_slice(b"not a git tree row\n");
        fs::write(fixture.root.join(TREE_DEST), tree)?;
        ensure!(verify(&fixture.root).is_err(), "tampered tree was accepted");
        fixture.import()?;
        let mut doc = manifest(&fixture.root)?;
        doc["artifacts"]
            .as_table_mut()
            .context("artifacts")?
            .remove("sdk/python/bittensor/settings.py");
        fs::write(fixture.root.join("upstream.toml"), doc.to_string())?;
        ensure!(
            verify(&fixture.root).is_err(),
            "missing manifest source was accepted"
        );
        Ok(())
    }

    #[test]
    fn offline_verification_checks_blob_identity_even_after_sha256_changes() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.import()?;
        let mut doc = manifest(&fixture.root)?;
        let bytes = b"RAO_PER_TAO = 1\n";
        fs::write(
            fixture
                .root
                .join("vendor/upstream/src/sdk/python/bittensor/settings.py"),
            bytes,
        )?;
        doc["artifacts"]["sdk/python/bittensor/settings.py"]["sha256"] =
            toml_edit::value(sha256(bytes));
        fs::write(fixture.root.join("upstream.toml"), doc.to_string())?;
        let error = verify(&fixture.root)
            .err()
            .context("tampered blob was accepted")?;
        ensure!(
            error.to_string().contains("git blob mismatch"),
            "wrong failure: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn importer_refuses_unsafe_and_duplicate_destinations() -> Result<()> {
        for destination in [
            "../escape",
            "/tmp/escape",
            "vendor/upstream/../escape",
            "vendor/upstream//escape",
            "vendor/upstream/./escape",
            "vendor/upstream/evil\\path",
            "vendor/upstream/extracted",
            "vendor/upstream/extracted/owned.json",
        ] {
            let mut doc: DocumentMut = "[artifacts]\n".parse()?;
            let mut item = InlineTable::new();
            item.insert("to", Value::from(destination));
            doc["artifacts"]["sdk/python/bittensor/settings.py"] =
                Item::Value(Value::InlineTable(item));
            ensure!(
                artifacts(&doc, false).is_err(),
                "unsafe destination accepted: {destination}"
            );
        }
        let doc: DocumentMut =
            "[artifacts]\n'a' = {to = 'vendor/upstream/a'}\n'b' = {to = 'vendor/upstream/a'}\n"
                .parse()?;
        ensure!(
            artifacts(&doc, false).is_err(),
            "duplicate destination accepted"
        );
        Ok(())
    }

    #[test]
    fn verification_rejects_untied_copies_and_accepts_extraction_outputs() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.import()?;
        write_if_changed(
            &fixture.root,
            "vendor/upstream/extracted/goldens.json",
            b"{}\n",
        )?;
        verify(&fixture.root)?;
        write_if_changed(&fixture.root, "vendor/upstream/unlisted.json", b"{}\n")?;
        ensure!(verify(&fixture.root).is_err(), "untied copy was accepted");
        ensure!(
            fixture.import().is_err(),
            "import silently left an untied copy"
        );
        Ok(())
    }

    #[test]
    fn malformed_notice_does_not_mutate_imports_or_manifest() -> Result<()> {
        let fixture = Fixture::new()?;
        let before = fs::read(fixture.root.join("upstream.toml"))?;
        fs::write(
            fixture.root.join("NOTICE"),
            format!("rtao\n{NOTICE_BEGIN}unfinished\n"),
        )?;
        ensure!(fixture.import().is_err(), "malformed NOTICE accepted");
        assert_eq!(before, fs::read(fixture.root.join("upstream.toml"))?);
        ensure!(
            !fixture.root.join("vendor/upstream").exists(),
            "import mutated copies before NOTICE validation"
        );
        Ok(())
    }

    #[test]
    fn importer_rejects_destinations_that_are_ancestors_of_other_artifacts() -> Result<()> {
        let fixture = Fixture::new()?;
        let mut doc = manifest(&fixture.root)?;
        let mut artifact = InlineTable::new();
        artifact.insert("to", Value::from("vendor/upstream/src"));
        doc["artifacts"]["sdk/python/tests/fixtures/fixture.py"] =
            Item::Value(Value::InlineTable(artifact));
        fs::write(fixture.root.join("upstream.toml"), doc.to_string())?;
        ensure!(
            fixture.import().is_err(),
            "destination ancestor conflict accepted"
        );
        ensure!(
            !fixture.root.join("vendor/upstream").exists(),
            "import mutated copies before destination validation"
        );
        Ok(())
    }

    #[test]
    fn tree_parser_refuses_symlinks_path_escapes_and_duplicate_rows() -> Result<()> {
        let blob = "0".repeat(40);
        let symlink = format!("120000 blob {blob}\tsdk/python/bittensor/alias.py\n");
        ensure!(parse_tree(symlink.as_bytes()).is_err());
        let escape = format!("100644 blob {blob}\tsdk/python/bittensor/../alias.py\n");
        ensure!(parse_tree(escape.as_bytes()).is_err());
        let row = format!("100644 blob {blob}\tsdk/python/bittensor/alias.py\n");
        ensure!(parse_tree(format!("{row}{row}").as_bytes()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn importing_and_verifying_refuse_symlinked_destinations() -> Result<()> {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new()?;
        fixture.import()?;
        let imported = fixture
            .root
            .join("vendor/upstream/src/sdk/python/bittensor/settings.py");
        let retained = fixture.root.join("retained.py");
        fs::rename(&imported, &retained)?;
        symlink(&retained, &imported)?;
        ensure!(verify(&fixture.root).is_err());
        ensure!(fixture.import().is_err());
        assert_eq!(fs::read(&retained)?, b"RAO_PER_TAO = 10**9\n");
        fs::remove_file(&imported)?;
        let directory = fixture
            .root
            .join("vendor/upstream/src/sdk/python/bittensor");
        fs::remove_dir(&directory)?;
        symlink(
            fixture.root.join("upstream.git/sdk/python/bittensor"),
            &directory,
        )?;
        ensure!(verify(&fixture.root).is_err());
        ensure!(fixture.import().is_err());
        Ok(())
    }
}
