//! Maintain the bounded core patch and authenticate every carried source change.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value as Json;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::pin::{atomic_write, cached_repo, git, read_document};

const UPSTREAM: &str = "https://github.com/RaoFoundation/subtensor.git";
const FORK: &str = "https://github.com/sam0x17/subtensor.git";
const LEDGER_PR: &str = "https://github.com/RaoFoundation/subtensor/pull/3242";
const SECURITY_ISSUE: &str = "https://github.com/RaoFoundation/subtensor/issues/3251";
const LEDGER_PATHS: [&str; 3] = [
    "sdk/bittensor-core/src/signers/mod.rs",
    "sdk/bittensor-core/src/signers/ledger.rs",
    "sdk/bittensor-core/src/signers/hid.rs",
];
// Every path is modify-only: a carried diff may change an existing regular
// source and never add, delete, rename or re-mode one. `keys/hashed.rs` is
// new in v488, absent at the PR-U17 base, so only a reviewed adaptation
// (D-139) can change it; it holds the hashed-account Ed25519 keys.
const PATHS: [&str; 7] = [
    "sdk/bittensor-core/src/signers/mod.rs",
    "sdk/bittensor-core/src/signers/ledger.rs",
    "sdk/bittensor-core/src/signers/hid.rs",
    "sdk/bittensor-core/Cargo.toml",
    "sdk/bittensor-core/src/keys/mod.rs",
    "sdk/bittensor-core/src/keys/encrypted_json.rs",
    "sdk/bittensor-core/src/keys/hashed.rs",
];
const HASHED: &str = "sdk/bittensor-core/src/keys/hashed.rs";
// The core's own path packages that always exist; others (v488's
// `subtensor-hashed` and `fips204`) are found from the core tag's manifests.
const SHARED_BASE: [&str; 2] = ["subtensor-macros", "w3f-bls"];
const MAX_ADAPTATIONS: usize = 4;
const PROBES: [&str; 2] = [
    "bittensor_core::signers::ApduTransport",
    "bittensor_core::signers::ledger::LedgerDevice::with_transport",
];

fn hash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn tag(value: &str) -> bool {
    value.strip_prefix('v').is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn tag_number(value: &str) -> Result<u64> {
    ensure!(tag(value), "invalid release tag {value}");
    value[1..]
        .parse()
        .with_context(|| format!("release tag {value} out of range"))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A reviewed adaptation of the combined patch to a later core (D-139): the
/// diff `base..rev` (fork commits, every changed path in `PATHS`, digest
/// `diff_sha256`), applied after the combined patch to a core at `tag` or a
/// later one until an adaptation for a later tag replaces it. A recorded
/// adaptation for a tag after the core's is staged for the next core bump.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Adaptation {
    tag: String,
    base: String,
    rev: String,
    diff_sha256: String,
}

fn adaptations(patch: &Item) -> Result<Vec<Adaptation>> {
    let Some(item) = patch.get("adaptation") else {
        return Ok(Vec::new());
    };
    let tables = item
        .as_array_of_tables()
        .context("core.patch.adaptation must be an array of tables")?;
    ensure!(
        !tables.is_empty() && tables.len() <= MAX_ADAPTATIONS,
        "core patch needs 1..={MAX_ADAPTATIONS} adaptations when the key exists"
    );
    let mut result = Vec::new();
    let mut tags = BTreeSet::new();
    for table in tables.iter() {
        let keys: BTreeSet<_> = table.iter().map(|(key, _)| key).collect();
        ensure!(
            keys == BTreeSet::from(["base", "diff_sha256", "rev", "tag"]),
            "core patch adaptation needs exactly tag, base, rev and diff_sha256"
        );
        let text = |key: &str| -> Result<String> {
            table
                .get(key)
                .and_then(Item::as_str)
                .map(str::to_owned)
                .with_context(|| format!("core patch adaptation {key} must be a string"))
        };
        let adaptation = Adaptation {
            tag: text("tag")?,
            base: text("base")?,
            rev: text("rev")?,
            diff_sha256: text("diff_sha256")?,
        };
        ensure!(
            tags.insert(tag_number(&adaptation.tag)?),
            "duplicate core patch adaptation for {}",
            adaptation.tag
        );
        ensure!(
            hash(&adaptation.base) && hash(&adaptation.rev) && adaptation.base != adaptation.rev,
            "core patch adaptation commits must be two full immutable Git hashes"
        );
        ensure!(
            digest(&adaptation.diff_sha256),
            "invalid core patch adaptation diff_sha256"
        );
        result.push(adaptation);
    }
    result.sort_by_key(|adaptation| tag_number(&adaptation.tag).unwrap_or(u64::MAX));
    Ok(result)
}

/// The adaptation a core at `core_tag` carries: the one recorded for the
/// latest tag not after it.
fn applicable<'a>(adaptations: &'a [Adaptation], core_tag: &str) -> Result<Option<&'a Adaptation>> {
    let number = tag_number(core_tag)?;
    let mut selected = None;
    for adaptation in adaptations {
        if tag_number(&adaptation.tag)? <= number {
            selected = Some(adaptation);
        }
    }
    Ok(selected)
}

fn adaptation_table(adaptations: &[Adaptation]) -> Item {
    let mut tables = toml_edit::ArrayOfTables::new();
    for adaptation in adaptations {
        let mut table = Table::new();
        for (key, contents) in [
            ("tag", &adaptation.tag),
            ("base", &adaptation.base),
            ("rev", &adaptation.rev),
            ("diff_sha256", &adaptation.diff_sha256),
        ] {
            table.insert(key, value(contents.as_str()));
        }
        tables.push(table);
    }
    Item::ArrayOfTables(tables)
}

fn shared(patch: &Item) -> Result<BTreeSet<String>> {
    let names = strings(patch, "shared")?;
    for name in SHARED_BASE {
        ensure!(
            names.contains(name),
            "core patch shared packages lost {name}"
        );
    }
    for name in &names {
        ensure!(
            name != "bittensor-core"
                && !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "invalid core patch shared package {name}"
        );
    }
    Ok(names)
}

fn field<'a>(item: &'a Item, name: &str) -> Result<&'a str> {
    item.get(name)
        .and_then(Item::as_str)
        .with_context(|| format!("core patch needs {name}"))
}

fn strings(item: &Item, name: &str) -> Result<BTreeSet<String>> {
    let array = item
        .get(name)
        .and_then(Item::as_array)
        .with_context(|| format!("core patch needs {name} array"))?;
    let result: BTreeSet<_> = array
        .iter()
        .map(|value| value.as_str().map(str::to_owned).context("expected string"))
        .collect::<Result<_>>()?;
    ensure!(result.len() == array.len(), "duplicate core patch {name}");
    Ok(result)
}

fn patch(pins: &DocumentMut) -> Result<Option<&Item>> {
    let Some(patch) = pins.get("core").and_then(|core| core.get("patch")) else {
        return Ok(None);
    };
    validate_patch(patch)?;
    let core = pins.get("core").context("core pin missing")?;
    let core_tag = field(core, "tag")?;
    ensure!(
        field(patch, "branch")? == format!("rtao-core-patch/{core_tag}"),
        "core patch branch differs from core.tag"
    );
    ensure!(
        field(patch, "base")? == field(core, "commit")?,
        "core patch base differs from core.commit"
    );
    Ok(Some(patch))
}

fn validate_patch(patch: &Item) -> Result<()> {
    ensure!(patch.is_table(), "core.patch must be a table");
    ensure!(field(patch, "repo")? == FORK, "unexpected core patch fork");
    ensure!(
        field(patch, "upstream_pr")? == LEDGER_PR,
        "unexpected core patch PR"
    );
    ensure!(
        field(patch, "security_issue")? == SECURITY_ISSUE,
        "unexpected core patch security issue"
    );
    let core_tag = field(patch, "branch")?
        .strip_prefix("rtao-core-patch/")
        .context("unexpected core patch branch prefix")?;
    ensure!(tag(core_tag), "invalid core patch release tag");
    ensure!(
        field(patch, "branch")? == format!("rtao-core-patch/{core_tag}"),
        "core patch branch differs from core.tag"
    );
    ensure!(
        hash(field(patch, "rev")?) && hash(field(patch, "base")?),
        "core patch commits must be full immutable Git hashes"
    );
    ensure!(
        strings(patch, "paths")? == PATHS.into_iter().map(str::to_owned).collect(),
        "core patch path allowlist changed"
    );
    ensure!(
        strings(patch, "probe")? == PROBES.into_iter().map(str::to_owned).collect(),
        "core patch obsolescence probes changed"
    );
    ensure!(
        hash(field(patch, "upstream_pr_rev")?),
        "invalid core patch upstream_pr_rev"
    );
    for key in ["review_base", "review_rev", "review_diff_sha256"] {
        let value = field(patch, key)?;
        ensure!(
            if key == "review_diff_sha256" {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            } else {
                hash(value)
            },
            "invalid core patch {key}"
        );
    }
    shared(patch)?;
    adaptations(patch)?;
    Ok(())
}

/// Lock source expected for the core, which is the fork only during a patch.
pub(crate) fn expected_core_source(pins: &DocumentMut) -> Result<String> {
    if let Some(patch) = patch(pins)? {
        let rev = field(patch, "rev")?;
        Ok(format!("git+{FORK}?rev={rev}#{rev}"))
    } else {
        let core = pins.get("core").context("core pin missing")?;
        Ok(format!(
            "git+{UPSTREAM}?tag={}#{}",
            field(core, "tag")?,
            field(core, "commit")?
        ))
    }
}

/// No package other than the single pinned core may resolve from the fork.
pub(crate) fn verify_lock(lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let expected = expected_core_source(pins)?;
    let patched = patch(pins)?.is_some();
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?;
    let mut cores = 0;
    for package in packages {
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .context("package name missing")?;
        let source = package.get("source").and_then(Item::as_str).unwrap_or("");
        if name == "bittensor-core" {
            ensure!(
                source == expected,
                "core lock source differs from core patch/pin"
            );
            cores += 1;
        }
        if source.contains("github.com/sam0x17/subtensor") {
            ensure!(
                patched && name == "bittensor-core" && source == expected,
                "unexpected fork package/source in lock: {name} {source}"
            );
        }
    }
    ensure!(cores == 1, "lock must contain exactly one bittensor-core");
    Ok(())
}

fn dependency(repo: &str, selector: &str, reference: &str) -> Item {
    let mut dependency = InlineTable::new();
    dependency.insert("git", Value::from(repo));
    dependency.insert(selector, Value::from(reference));
    Item::Value(Value::InlineTable(dependency))
}

fn remove_tables(manifest: &mut DocumentMut) -> Result<()> {
    let tables = manifest["patch"]
        .as_table_like_mut()
        .context("patch table missing")?;
    tables.remove(UPSTREAM);
    tables.remove(FORK);
    Ok(())
}

fn patch_tables(
    manifest: &mut DocumentMut,
    core_tag: &str,
    rev: &str,
    shared: &BTreeSet<String>,
) -> Result<()> {
    remove_tables(manifest)?;
    let mut core = Table::new();
    core.insert("bittensor-core", dependency(FORK, "rev", rev));
    manifest["patch"][UPSTREAM] = Item::Table(core);
    // The patched core's own path packages would otherwise come from the
    // fork's checkout: keep every one at the core tag, so the core is the
    // only package from the fork (D-139; v488 adds subtensor-hashed and
    // its fips204).
    let mut other = Table::new();
    for name in shared {
        other.insert(name, dependency(UPSTREAM, "tag", core_tag));
    }
    manifest["patch"][FORK] = Item::Table(other);
    Ok(())
}

fn verify_tables(manifest: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let actual = manifest["patch"]
        .as_table_like()
        .context("patch table missing")?;
    if let Some(patch) = patch(pins)? {
        let mut expected = manifest.clone();
        patch_tables(
            &mut expected,
            field(&pins["core"], "tag")?,
            field(patch, "rev")?,
            &shared(patch)?,
        )?;
        for repo in [UPSTREAM, FORK] {
            let left = actual.get(repo).context("core patch Cargo table missing")?;
            let right = &expected["patch"][repo];
            let left = left
                .as_table_like()
                .context("core patch entry must be a table")?;
            let right = right.as_table_like().context("expected core patch table")?;
            ensure!(
                left.len() == right.len(),
                "unexpected core patch dependency count: {repo}"
            );
            for (name, expected) in right.iter() {
                let actual = left
                    .get(name)
                    .and_then(Item::as_table_like)
                    .context("core patch dependency missing")?;
                let expected = expected
                    .as_table_like()
                    .context("expected core patch dependency")?;
                ensure!(
                    actual.len() == expected.len(),
                    "unexpected core patch dependency fields: {name}"
                );
                for (key, expected) in expected.iter() {
                    ensure!(
                        actual.get(key).and_then(Item::as_str) == expected.as_str(),
                        "core patch dependency differs: {name}.{key}"
                    );
                }
            }
        }
    } else {
        ensure!(
            actual.get(UPSTREAM).is_none() && actual.get(FORK).is_none(),
            "core patch Cargo tables exist without core.patch"
        );
    }
    Ok(())
}

fn verify_clippy(root: &Path) -> Result<()> {
    let config = read_document(&root.join("clippy.toml"))?;
    for (list, path) in [
        ("disallowed-types", PROBES[0]),
        ("disallowed-methods", PROBES[1]),
    ] {
        let entries = config[list]
            .as_array()
            .with_context(|| format!("missing Clippy {list}"))?;
        let matches: Vec<_> = entries
            .iter()
            .filter_map(Value::as_inline_table)
            .filter(|entry| entry.get("path").and_then(Value::as_str) == Some(path))
            .collect();
        ensure!(
            matches.len() == 1
                && matches[0].get("allow-invalid").and_then(Value::as_bool) == Some(true),
            "Clippy must ban {path} with allow-invalid = true"
        );
    }
    Ok(())
}

fn git_text(repo: &Path, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(repo, arguments)?)?.trim().to_owned())
}

fn ancestor(repo: &Path, base: &str, rev: &str) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", base, rev])
        .output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "cannot inspect patch ancestry: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn verify_diff_paths(repo: &Path, base: &str, rev: &str, paths: &[&str]) -> Result<()> {
    ensure!(
        ancestor(repo, base, rev)?,
        "core patch does not descend from core.commit"
    );
    let names = git(
        repo,
        &["diff", "--no-renames", "--name-status", "-z", base, rev],
    )?;
    let records: Vec<_> = names
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .collect();
    ensure!(
        !records.is_empty() && records.len() % 2 == 0,
        "empty or malformed core patch diff"
    );
    for pair in records.as_chunks::<2>().0 {
        ensure!(
            pair.first() == Some(&b"M".as_slice()),
            "core patch must only modify existing regular sources"
        );
        let path = std::str::from_utf8(pair.get(1).context("diff path missing")?)?;
        ensure!(
            paths.contains(&path),
            "core patch changed forbidden path {path}"
        );
        for reference in [base, rev] {
            let mode = git_text(repo, &["ls-tree", reference, "--", path])?;
            ensure!(
                mode.starts_with("100644 blob "),
                "core patch path is not a regular source: {path}"
            );
        }
    }
    Ok(())
}

fn verify_diff(repo: &Path, base: &str, rev: &str) -> Result<()> {
    verify_diff_paths(repo, base, rev, &PATHS)
}

fn diff_digest_paths(repo: &Path, base: &str, rev: &str, paths: &[&str]) -> Result<String> {
    use sha2::Digest;
    // Each path is a separate argument. Keep the exact diff bytes as provenance
    // while the path/regular-file allowlist is verified independently.
    let mut arguments = vec![
        "diff",
        "--binary",
        "--full-index",
        "--no-ext-diff",
        "--no-renames",
        base,
        rev,
        "--",
    ];
    arguments.extend(paths);
    Ok(hex::encode(sha2::Sha256::digest(git(repo, &arguments)?)))
}

fn diff_digest(repo: &Path, base: &str, rev: &str) -> Result<String> {
    diff_digest_paths(repo, base, rev, &PATHS)
}

/// An adaptation's own provenance: a linear series of 1..=20 fork commits
/// whose net diff changes only existing regular sources in `PATHS` and has
/// the recorded digest. Returns the commits, oldest first.
fn verify_adaptation(repo: &Path, adaptation: &Adaptation) -> Result<Vec<String>> {
    verify_diff(repo, &adaptation.base, &adaptation.rev)
        .with_context(|| format!("core patch adaptation for {}", adaptation.tag))?;
    ensure!(
        diff_digest(repo, &adaptation.base, &adaptation.rev)? == adaptation.diff_sha256,
        "recorded core patch adaptation diff for {} changed",
        adaptation.tag
    );
    let commits = git_text(
        repo,
        &[
            "rev-list",
            "--reverse",
            "--ancestry-path",
            &format!("{}..{}", adaptation.base, adaptation.rev),
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    ensure!(
        !commits.is_empty() && commits.len() <= 20 && commits.iter().all(|commit| hash(commit)),
        "core patch adaptation for {} needs 1..=20 linear commits",
        adaptation.tag
    );
    Ok(commits)
}

fn verify_review(repo: &Path, entry: &Item) -> Result<()> {
    let review_base = field(entry, "review_base")?;
    let review_rev = field(entry, "review_rev")?;
    verify_diff(repo, review_base, review_rev)?;
    ensure!(
        diff_digest(repo, review_base, review_rev)? == field(entry, "review_diff_sha256")?,
        "recorded core patch diff changed"
    );
    let recorded = adaptations(entry)?;
    for adaptation in &recorded {
        verify_adaptation(repo, adaptation)?;
    }
    let applied = if recorded.is_empty() {
        None
    } else {
        let core_tag = field(entry, "branch")?
            .strip_prefix("rtao-core-patch/")
            .context("unexpected core patch branch prefix")?;
        applicable(&recorded, core_tag)?
    };
    let base = field(entry, "base")?;
    let expected = if base == review_base && applied.is_none() {
        git_text(repo, &["rev-parse", &format!("{review_rev}^{{tree}}")])?
    } else {
        let mut diffs = vec![(review_base, review_rev)];
        if let Some(adaptation) = applied {
            diffs.push((adaptation.base.as_str(), adaptation.rev.as_str()));
        }
        applied_review_tree(repo, base, &diffs)?
    };
    let actual = git_text(
        repo,
        &["rev-parse", &format!("{}^{{tree}}", field(entry, "rev")?)],
    )?;
    ensure!(
        expected == actual,
        "carried core tree differs from applying the recorded core patch; review the conflict resolution"
    );
    Ok(())
}

// The Ed25519 erasure repair is checked structurally, so comments, aliases or
// a second field cannot satisfy or evade it. Every Rust file of the patched
// core is parsed; no type position may name `ed25519::Pair` (it does not wipe
// its expanded key), the stored variant and derivation owner must hold
// `Zeroizing<SigningKey>`, and the meaningful regressions must remain tests.
fn verify_security_source_text(manifest: &str, sources: &[(String, String)]) -> Result<()> {
    use syn::visit::Visit;
    ensure!(
        manifest.contains("ed25519-zebra = { version = \"4.0\", default-features = false }"),
        "core patch lost its bounded ed25519-zebra dependency"
    );
    fn tokens(value: &impl quote::ToTokens) -> String {
        value.to_token_stream().to_string().replace(' ', "")
    }
    struct PairTypes(Vec<String>);
    impl<'ast> Visit<'ast> for PairTypes {
        fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
            let path = tokens(&node.path);
            if path == "Pair" || path.ends_with("ed25519::Pair") {
                self.0.push(path);
            }
            syn::visit::visit_type_path(self, node);
        }
        fn visit_use_tree(&mut self, node: &'ast syn::UseTree) {
            let tree = tokens(node);
            if tree.contains("ed25519::Pair")
                || tree.contains("ed25519::{") && tree.contains("Pair")
            {
                self.0.push(format!("use {tree}"));
            }
            syn::visit::visit_use_tree(self, node);
        }
    }
    let mut keys = None;
    let mut encrypted = None;
    let mut hashed = None;
    for (path, text) in sources {
        let file = syn::parse_file(text)
            .with_context(|| format!("core patch source does not parse: {path}"))?;
        let mut found = PairTypes(Vec::new());
        found.visit_file(&file);
        ensure!(
            found.0.is_empty(),
            "core patch names a non-wiping Ed25519 pair type in {path}: {:?}",
            found.0
        );
        if path.ends_with("src/keys/mod.rs") {
            keys = Some((file, text));
        } else if path.ends_with("src/keys/encrypted_json.rs") {
            encrypted = Some(text);
        } else if path == HASHED {
            hashed = Some((file, text));
        }
    }
    let (keys, keys_text) = keys.context("core patch lost keys/mod.rs")?;
    let encrypted = encrypted.context("core patch lost keys/encrypted_json.rs")?;
    let mut stored = None;
    let mut owner = None;
    let mut tests = std::collections::BTreeSet::new();
    struct Items<'a> {
        stored: &'a mut Option<String>,
        owner: &'a mut Option<String>,
        tests: &'a mut std::collections::BTreeSet<String>,
    }
    impl<'ast> Visit<'ast> for Items<'_> {
        fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
            if node.ident == "KeypairInner" {
                for variant in &node.variants {
                    if variant.ident == "Ed25519" {
                        *self.stored = Some(tokens(&variant.fields));
                    }
                }
            }
            syn::visit::visit_item_enum(self, node);
        }
        fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
            if node.ident == "WipingEd25519Pair" {
                *self.owner = Some(tokens(&node.fields));
            }
            syn::visit::visit_item_struct(self, node);
        }
        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            if node.attrs.iter().any(|attr| attr.path().is_ident("test")) {
                self.tests.insert(node.sig.ident.to_string());
            }
            syn::visit::visit_item_fn(self, node);
        }
    }
    Items {
        stored: &mut stored,
        owner: &mut owner,
        tests: &mut tests,
    }
    .visit_file(&keys);
    ensure!(
        stored.as_deref() == Some("(Zeroizing<SigningKey>)"),
        "core patch stores Ed25519 signing state without the wiping owner: {stored:?}"
    );
    ensure!(
        owner.as_deref() == Some("{secret:Zeroizing<SigningKey>,}"),
        "core patch derivation owner is not exactly Zeroizing<SigningKey>: {owner:?}"
    );
    for test in [
        "ed25519_signing_object_has_a_wiping_owner_and_keeps_chain_bytes",
        "zeroize_clears_every_signing_key_secret",
        "hard_derivation_keeps_substrate_vectors",
    ] {
        ensure!(
            tests.contains(test),
            "core patch lost Ed25519 erasure regression test {test}"
        );
    }
    for required in [
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        "88dc3417d5058ec4b4503e0c12ea1a0a89be200fe98922423d4334014fa6b0ee",
    ] {
        ensure!(
            keys_text.contains(required),
            "core patch lost an Ed25519 compatibility vector: {required}"
        );
    }
    ensure!(
        !keys_text.contains(".using_encoded(sp_core::hashing::blake2_256)"),
        "core patch derives Ed25519 junctions through an unwiped encoding buffer"
    );
    for required in [
        "WipingEd25519Pair::from_seed_slice(seed)",
        "KeypairInner::Ed25519(pair.into_secret())",
        "Zeroizing::new([0u8; SEC_LENGTH])",
    ] {
        ensure!(
            encrypted.contains(required),
            "encrypted JSON import bypasses a wiping Ed25519 owner: {required}"
        );
    }
    if let Some((file, text)) = hashed {
        verify_hashed_source(&file, text)?;
    }
    Ok(())
}

// v488's hashed accounts derive one Ed25519 signing key per generation
// (`keys/hashed.rs`). Beyond the type-position rule above, which already
// refuses `ed25519::Pair` there, the derivation must return the wiping owner
// and the regression test pinning the v488 bytes must remain (D-139).
fn verify_hashed_source(file: &syn::File, text: &str) -> Result<()> {
    use syn::visit::Visit;
    fn tokens(value: &impl quote::ToTokens) -> String {
        value.to_token_stream().to_string().replace(' ', "")
    }
    struct Items {
        derive: Vec<String>,
        tests: BTreeSet<String>,
    }
    impl<'ast> Visit<'ast> for Items {
        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            if node.sig.ident == "derive_ed" || node.sig.ident == "ed_at" {
                self.derive.push(tokens(&node.sig.output));
            }
            syn::visit::visit_impl_item_fn(self, node);
        }
        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            if node.sig.ident == "derive_ed" || node.sig.ident == "ed_at" {
                self.derive.push(tokens(&node.sig.output));
            }
            if node.attrs.iter().any(|attr| attr.path().is_ident("test")) {
                self.tests.insert(node.sig.ident.to_string());
            }
            syn::visit::visit_item_fn(self, node);
        }
    }
    let mut items = Items {
        derive: Vec::new(),
        tests: BTreeSet::new(),
    };
    items.visit_file(file);
    ensure!(
        items.derive.len() == 2
            && items
                .derive
                .iter()
                .all(|output| output == "->Result<Zeroizing<SigningKey>,CoreError>"),
        "hashed-account Ed25519 derivation does not return the wiping owner: {:?}",
        items.derive
    );
    ensure!(
        items
            .tests
            .contains("hashed_ed25519_signing_key_has_a_wiping_owner_and_keeps_vectors"),
        "core patch lost the hashed Ed25519 erasure regression test"
    );
    for required in [
        "8b9c4ae043290453eb92a40d48f5074178031bc6623e60544ae4a2f5ab6542a1",
        "87ba9efaab5ed270bf9fa5e8a4101283cd6ba1541f9166177778f1ca47aa88c7",
        "5EPuttWmgUjN8Wq2qKELKso6WrfRpBpgZpuNRzAwyAgQTXzo",
    ] {
        ensure!(
            text.contains(required),
            "core patch lost a hashed Ed25519 compatibility vector: {required}"
        );
    }
    Ok(())
}

fn verify_security_source(repo: &Path, rev: &str) -> Result<()> {
    let source = |path: &str| -> Result<String> {
        String::from_utf8(git(repo, &["show", &format!("{rev}:{path}")])?)
            .with_context(|| format!("non-UTF-8 core patch source: {path}"))
    };
    let listing = String::from_utf8(git(
        repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            rev,
            "--",
            "sdk/bittensor-core/src",
        ],
    )?)
    .context("non-UTF-8 core patch file listing")?;
    let mut sources = Vec::new();
    for path in listing.lines().filter(|path| path.ends_with(".rs")) {
        sources.push((path.to_owned(), source(path)?));
    }
    verify_security_source_text(&source("sdk/bittensor-core/Cargo.toml")?, &sources)
}

/// The exact tree of `base` with each reviewed diff applied in order (the
/// combined patch, then the core's adaptation when it has one).
fn applied_review_tree(repo: &Path, base: &str, diffs: &[(&str, &str)]) -> Result<String> {
    use std::io::Write;
    use std::process::Stdio;
    let temporary = hosted_root()?.join("core-patch");
    fs::create_dir_all(&temporary)?;
    let index = temporary.join("review-verify.index");
    let lock = temporary.join("review-verify.index.lock");
    ensure!(
        !index.exists() && !lock.exists(),
        "review verification index already exists; use a fresh hosted task root"
    );
    // An independent index computes the exact tree that a three-way application
    // produces, including whitespace and string/byte literal contents. It never
    // touches the checkout index or relies on whitespace-insensitive patch IDs.
    let result = (|| -> Result<String> {
        let command = || {
            let mut command = Command::new("git");
            command
                .arg("-C")
                .arg(repo)
                .args(["-c", "core.splitIndex=false"])
                .env("GIT_INDEX_FILE", &index);
            command
        };
        let output = command().args(["read-tree", base]).output()?;
        ensure!(
            output.status.success(),
            "cannot initialize isolated review index"
        );
        for (from, to) in diffs {
            let diff = git(
                repo,
                &[
                    "diff",
                    "--binary",
                    "--full-index",
                    "--no-ext-diff",
                    "--no-renames",
                    from,
                    to,
                ],
            )?;
            ensure!(diff.len() <= 1_048_576, "core patch diff exceeds 1 MiB");
            let mut child = command()
                .args(["apply", "--cached", "--3way", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child
                .stdin
                .take()
                .context("review apply stdin missing")?
                .write_all(&diff)?;
            let output = child.wait_with_output()?;
            ensure!(
                output.status.success(),
                "core-patch-conflict: recorded core patch {from}..{to} does not apply to core base: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let output = command().arg("write-tree").output()?;
        ensure!(
            output.status.success(),
            "cannot derive exact reviewed core tree"
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    })();
    for path in [&index, &lock] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    result
}

fn hosted_root() -> Result<PathBuf> {
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Linux"),
        "core-patch builds and rebases run only in bounded hosted Linux CI (PLAN Q30)"
    );
    let root =
        PathBuf::from(std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?);
    ensure!(root.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
    fs::create_dir_all(&root)?;
    root.canonicalize().context("canonical hosted task root")
}

fn missing_symbols(output: &[u8], directory: &Path) -> Result<bool> {
    let source = directory.join("src/main.rs");
    let manifest = directory.join("Cargo.toml");
    let mut errors = 0;
    for line in std::str::from_utf8(output)?.lines() {
        let Ok(record) = serde_json::from_str::<Json>(line) else {
            continue;
        };
        if record["reason"] != "compiler-message" || record["message"]["level"] != "error" {
            continue;
        }
        let own_package = record["package_id"]
            .as_str()
            .and_then(|identity| identity.split_once('#'))
            .is_some_and(|(url, name)| {
                let url = url
                    .strip_prefix("path+")
                    .and_then(|url| url::Url::parse(url).ok());
                name == "rtao-core-patch-probe@0.0.0"
                    && url.and_then(|url| url.to_file_path().ok()).as_deref() == Some(directory)
            });
        let own_target = record["manifest_path"].as_str().map(Path::new)
            == Some(manifest.as_path())
            && record["target"]["name"] == "rtao-core-patch-probe"
            && record["target"]["kind"] == serde_json::json!(["bin"])
            && record["target"]["src_path"].as_str().map(Path::new) == Some(source.as_path());
        let own_span = record["message"]["spans"].as_array().is_some_and(|spans| {
            spans.iter().any(|span| {
                span["is_primary"] == true
                    && span["file_name"].as_str().is_some_and(|name| {
                        let name = Path::new(name);
                        name == source || (!name.is_absolute() && directory.join(name) == source)
                    })
            })
        });
        let missing = record["message"]["code"]["code"]
            .as_str()
            .is_some_and(|code| {
                ["E0405", "E0412", "E0425", "E0432", "E0433", "E0599"].contains(&code)
            });
        if !(own_package && own_target && own_span && missing) {
            return Ok(false);
        }
        errors += 1;
    }
    Ok(errors > 0)
}

fn probe(root: &Path, pins: &DocumentMut, patched: bool) -> Result<Output> {
    let temporary = hosted_root()?;
    let directory = temporary.join(if patched {
        "core-patch/probe/patched"
    } else {
        "core-patch/probe/unpatched"
    });
    ensure!(
        !directory.exists(),
        "core patch probe already exists; use a fresh hosted task root"
    );
    fs::create_dir_all(directory.join("src"))?;
    let core = &pins["core"];
    let core_tag = field(core, "tag")?;
    let mut manifest: DocumentMut = format!(
        "[workspace]\n[package]\nname = \"rtao-core-patch-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nbittensor-core = {{ git = {UPSTREAM:?}, tag = {core_tag:?}, default-features = false, features = [\"ledger\"] }}\n[patch.crates-io]\nw3f-bls = {{ git = {UPSTREAM:?}, tag = {core_tag:?} }}\n"
    ).parse()?;
    if patched {
        let entry = patch(pins)?.context("patch missing")?;
        patch_tables(
            &mut manifest,
            core_tag,
            field(entry, "rev")?,
            &shared(entry)?,
        )?;
    }
    fs::write(directory.join("Cargo.toml"), manifest.to_string())?;
    // Reuse only the small lock inventory, never compiled output. This keeps
    // isolated probes on reviewed dependency versions while Cargo adjusts the
    // single core source for the unpatched comparison.
    fs::copy(root.join("Cargo.lock"), directory.join("Cargo.lock"))?;
    // The generic function names both exact probe items, so the constructor's
    // inferred transport type cannot produce an unrelated inference error.
    fs::write(
        directory.join("src/main.rs"),
        "fn injected<T: bittensor_core::signers::ApduTransport + 'static>(transport: T) -> bittensor_core::signers::ledger::LedgerDevice { bittensor_core::signers::ledger::LedgerDevice::with_transport(transport) }\nfn main() {}\n",
    )?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(&directory)
        .args(["check", "--message-format=json"])
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_TARGET_DIR", temporary.join("core-patch/build"))
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .context("compile exact core patch API probe")?;
    let missing = missing_symbols(&output.stdout, &directory.canonicalize()?)?;
    if patched {
        ensure!(
            output.status.success() && !missing,
            "patched core API probe failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    } else if !output.status.success() {
        ensure!(
            missing,
            "unpatched probe failed for a reason other than missing PR-U17 symbols:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // The workflow owns the one task root; remove only our known source dirs.
    fs::remove_dir_all(&directory)?;
    Ok(output)
}

/// Source and identity checks, without executing hosted API build probes.
pub(crate) fn verify_source(root: &Path, pins: &DocumentMut) -> Result<()> {
    verify_source_with(root, pins, true)
}

/// As `verify_source`; `lock` false skips only the lockfile check, for a
/// candidate whose moved core is re-resolved and fully checked afterwards.
pub(crate) fn verify_source_with(root: &Path, pins: &DocumentMut, lock: bool) -> Result<()> {
    verify_tables(&read_document(&root.join("Cargo.toml"))?, pins)?;
    verify_clippy(root)?;
    if lock {
        verify_lock(&read_document(&root.join("Cargo.lock"))?, pins)?;
    }
    if let Some(patch) = patch(pins)? {
        let repo = cached_repo(root)?;
        let rev = field(patch, "rev")?;
        git(&repo, &["cat-file", "-e", &format!("{rev}^{{commit}}")])
            .context("patch source cache missing; prepare it explicitly in hosted CI")?;
        verify_diff(&repo, field(patch, "base")?, rev)?;
        verify_review(&repo, patch)?;
        ensure!(
            shared(patch)? == crate::fingerprint::core_path_packages(&repo, field(patch, "base")?)?,
            "core patch shared packages differ from the core's path packages at core.commit"
        );
        let review_base = field(patch, "review_base")?;
        let upstream_pr_rev = field(patch, "upstream_pr_rev")?;
        verify_diff_paths(&repo, review_base, upstream_pr_rev, &LEDGER_PATHS)?;
        ensure!(
            ancestor(&repo, upstream_pr_rev, field(patch, "review_rev")?)?,
            "recorded Ledger PR revision is not part of the combined core patch"
        );
        verify_security_source(&repo, rev)?;
    }
    Ok(())
}

/// Full verification retains both mandatory hosted build probes.
pub(crate) fn verify(root: &Path, pins: &DocumentMut) -> Result<()> {
    verify_source(root, pins)?;
    if patch(pins)?.is_some() {
        hosted_root()?;
        probe(root, pins, true)?;
        probe(root, pins, false)?;
    }
    Ok(())
}

/// Populate only immutable patch commits in the existing hosted source cache.
pub(crate) fn prepare_cache(root: &Path, pins: &DocumentMut) -> Result<()> {
    let Some(entry) = patch(pins)? else {
        return Ok(());
    };
    hosted_root()?;
    let repo = cached_repo(root)?;
    let recorded = adaptations(entry)?;
    for revision in [
        field(entry, "rev")?,
        field(entry, "review_rev")?,
        field(entry, "upstream_pr_rev")?,
    ]
    .into_iter()
    .chain(recorded.iter().map(|adaptation| adaptation.rev.as_str()))
    {
        git(&repo, &["fetch", "--filter=blob:none", FORK, revision])?;
    }
    Ok(())
}

fn gh(arguments: &[&str]) -> Result<Json> {
    let output = Command::new("gh").args(arguments).output()?;
    ensure!(
        output.status.success(),
        "GitHub PR query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.stdout.len() <= 1_048_576,
        "GitHub PR response exceeds 1 MiB"
    );
    serde_json::from_slice(&output.stdout).context("GitHub response is not JSON")
}

fn pr_commits(repo: &Path, pr: &Json) -> Result<(String, String, Vec<String>)> {
    let records = gh(&["api", "repos/RaoFoundation/subtensor/pulls/3242/commits"])?;
    let records = records.as_array().context("PR commits missing")?;
    ensure!(
        !records.is_empty() && records.len() <= 20,
        "PR-U17 needs 1..=20 commits"
    );
    let mut previous = None;
    let mut base = None;
    let mut commits = Vec::new();
    for record in records {
        let sha = record["sha"].as_str().context("PR commit SHA missing")?;
        ensure!(hash(sha), "invalid PR commit SHA");
        let parents = record["parents"]
            .as_array()
            .context("PR commit parents missing")?;
        ensure!(parents.len() == 1, "PR-U17 must be a linear commit series");
        let parent = parents
            .first()
            .and_then(|parent| parent["sha"].as_str())
            .context("PR parent missing")?;
        ensure!(hash(parent), "invalid PR parent");
        if let Some(previous) = previous {
            ensure!(parent == previous, "PR-U17 commit series is not contiguous");
        } else {
            base = Some(parent.to_owned());
        }
        previous = Some(sha);
        commits.push(sha.to_owned());
    }
    let head = commits.last().context("PR head missing")?.clone();
    ensure!(
        Some(head.as_str()) == pr["head"]["sha"].as_str(),
        "PR changed while querying commits; retry the same operation"
    );
    git(repo, &["fetch", "--filter=blob:none", FORK, &head])?;
    let base = base.context("PR base missing")?;
    verify_diff_paths(repo, &base, &head, &LEDGER_PATHS)?;
    Ok((base, head, commits))
}

fn git_with_input(repo: &Path, arguments: &[&str], bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    command
        .stdin
        .take()
        .context("Git stdin missing")?
        .write_all(bytes)?;
    let output = command.wait_with_output()?;
    ensure!(
        output.status.success(),
        "core-patch-conflict: retained {}: {}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Rebase a prepared release candidate while preserving its previous patch identity.
/// The pin pipeline calls this after updating candidate core identity, before
/// publishing pins or rebuilding the candidate lock.
pub(crate) fn rebase_candidate(
    root: &Path,
    previous: &DocumentMut,
    next: &mut DocumentMut,
) -> Result<()> {
    let target = field(&next["core"], "tag")?.to_owned();
    ensure!(tag(&target), "core-patch rebase needs a release tag vN");
    let temporary = hosted_root()?;
    let repo = cached_repo(root)?;
    let previous_patch = patch(previous)?;
    let base = git_text(
        &repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{target}^{{commit}}"),
        ],
    )?;
    ensure!(
        base == field(&next["core"], "commit")?,
        "core tag identity changed"
    );
    let pr = gh(&["api", "repos/RaoFoundation/subtensor/pulls/3242"])?;
    ensure!(
        pr["head"]["repo"]["full_name"] == "sam0x17/subtensor",
        "PR-U17 changed fork"
    );
    ensure!(
        pr["state"] == "open" || pr["merged"] == true,
        "PR-U17 was closed without merging; user decision required (PLAN rule 6)"
    );
    let issue = gh(&["api", "repos/RaoFoundation/subtensor/issues/3251"])?;
    ensure!(
        issue["html_url"] == SECURITY_ISSUE
            && issue["number"] == 3251
            && issue.get("pull_request").is_none(),
        "core patch security issue identity changed"
    );
    let (ledger_base, ledger_rev, _) = pr_commits(&repo, &pr)?;
    let previous = previous_patch.context(
        "combined core patch provenance is required before rebasing; review the Ledger and security deltas together",
    )?;
    ensure!(
        field(previous, "upstream_pr_rev")? == ledger_rev,
        "PR-U17 changed after the combined core patch review; review the new PR revision first"
    );
    ensure!(
        field(previous, "review_base")? == ledger_base,
        "combined core patch review base differs from PR-U17"
    );
    let review_base = field(previous, "review_base")?.to_owned();
    let review_rev = field(previous, "review_rev")?.to_owned();
    git(&repo, &["fetch", "--filter=blob:none", FORK, &review_rev])?;
    let recorded = adaptations(previous)?;
    for adaptation in &recorded {
        git(
            &repo,
            &["fetch", "--filter=blob:none", FORK, &adaptation.rev],
        )?;
    }
    verify_review(&repo, previous)?;
    // The new core carries the adaptation recorded for it (or the latest
    // earlier one); adaptations staged for later cores stay recorded.
    let adaptation = applicable(&recorded, &target)?.cloned();
    let retained: Vec<Adaptation> = recorded
        .iter()
        .filter(|candidate| match &adaptation {
            Some(selected) => tag_number(&candidate.tag).ok() >= tag_number(&selected.tag).ok(),
            None => true,
        })
        .cloned()
        .collect();
    let adaptation_commits = match &adaptation {
        Some(adaptation) => verify_adaptation(&repo, adaptation)?,
        None => Vec::new(),
    };
    let shared_packages = crate::fingerprint::core_path_packages(&repo, &base)?;
    ensure!(
        ancestor(&repo, &ledger_rev, &review_rev)?,
        "PR-U17 is not an ancestor of the combined core patch review"
    );
    let review_diff = field(previous, "review_diff_sha256")?.to_owned();
    let commits = git_text(
        &repo,
        &[
            "rev-list",
            "--reverse",
            "--ancestry-path",
            &format!("{review_base}..{review_rev}"),
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    ensure!(
        !commits.is_empty() && commits.len() <= 20 && commits.iter().all(|commit| hash(commit)),
        "combined core patch needs 1..=20 linear commits"
    );
    let branch = format!("rtao-core-patch/{target}");
    let remote = git_text(
        &repo,
        &[
            "ls-remote",
            "--heads",
            FORK,
            &format!("refs/heads/{branch}"),
        ],
    )?;
    let remote = if remote.is_empty() {
        None
    } else {
        let rows = remote.lines().collect::<Vec<_>>();
        ensure!(rows.len() == 1, "ambiguous patch branch identity");
        let (sha, reference) = rows
            .first()
            .context("remote branch missing")?
            .split_once('\t')
            .context("malformed patch ref")?;
        ensure!(
            hash(sha) && reference == format!("refs/heads/{branch}"),
            "unexpected patch branch identity"
        );
        Some(sha.to_owned())
    };
    let previous_same =
        previous_patch.filter(|entry| field(entry, "branch").is_ok_and(|name| name == branch));
    let entry_for = |rev: &str| -> Result<Item> {
        let mut entry = Table::new();
        for (key, contents) in [
            ("repo", FORK),
            ("branch", &branch),
            ("rev", rev),
            ("base", &base),
            ("upstream_pr", LEDGER_PR),
            ("upstream_pr_rev", &ledger_rev),
            ("security_issue", SECURITY_ISSUE),
            ("review_base", &review_base),
            ("review_rev", &review_rev),
            ("review_diff_sha256", &review_diff),
        ] {
            entry.insert(key, value(contents));
        }
        for (key, values) in [("paths", &PATHS[..]), ("probe", &PROBES[..])] {
            let mut array = Array::new();
            for item in values {
                array.push(*item);
            }
            entry.insert(key, value(array));
        }
        let mut array = Array::new();
        for name in &shared_packages {
            array.push(name.as_str());
        }
        entry.insert("shared", value(array));
        if !retained.is_empty() {
            entry.insert("adaptation", adaptation_table(&retained));
        }
        Ok(Item::Table(entry))
    };
    if let Some(remote) = &remote {
        git(&repo, &["fetch", "--filter=blob:none", FORK, remote])?;
        verify_diff(&repo, &base, remote)?;
        if let Some(previous) = previous_same {
            ensure!(
                ancestor(&repo, field(previous, "rev")?, remote)?,
                "remote patch branch rewrote pinned history"
            );
        }
        let entry = entry_for(remote)?;
        if verify_review(&repo, &entry).is_ok() {
            verify_security_source(&repo, remote).with_context(|| {
                format!("core-patch-security: the published patch branch {branch} fails the Ed25519 erasure check")
            })?;
            // This includes the initial v478 branch, which can point directly
            // at the already reviewed combined commit. Hosted CI only reads the fork.
            let mut prepared = next.clone();
            prepared["core"]["patch"] = entry;
            let mut manifest = read_document(&root.join("Cargo.toml"))?;
            patch_tables(&mut manifest, &target, remote, &shared_packages)?;
            verify_tables(&manifest, &prepared)?;
            atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
            *next = prepared;
            println!(
                "Adopted existing append-only core patch branch {branch} at {remote}; validate the hosted candidate before source adoption."
            );
            return Ok(());
        }
        let previous = previous_same.context(
            "existing patch branch differs from the reviewed core patch; inspect it before adopting",
        )?;
        ensure!(
            field(previous, "rev")? == remote,
            "unrecorded patch branch differs from the reviewed core patch; inspect it before adopting"
        );
        verify_review(&repo, previous)?;
    } else {
        ensure!(
            previous_same.is_none(),
            "recorded patch branch disappeared; do not recreate or delete pinned history"
        );
        if base == review_base && adaptation.is_none() {
            // No new Git object is needed: the authorized publisher can create
            // this ref at the existing reviewed commit using their own access.
            let entry = entry_for(&review_rev)?;
            write_candidate(&temporary, &repo, &base, &base, &review_rev, &entry)?;
            bail!(
                "core-patch-publish-required: create refs/heads/{branch} at existing reviewed commit {review_rev}, then rerun rebase; hosted CI does not push the fork"
            );
        }
    }
    let start = remote.as_deref().unwrap_or(&base);
    let directory = temporary.join("core-patch/rebase");
    ensure!(
        !directory.exists(),
        "core-patch rebase directory already exists; retained conflict must be resolved first"
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            directory.to_str().context("non-UTF-8 task root")?,
            start,
        ],
    )?;
    if remote.is_some() {
        // Revert the previous net delta, then append the current review series.
        // Publication uses an ordinary fast-forward; no history is overwritten.
        let reverse = git(
            &directory,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                start,
                &base,
            ],
        )?;
        git_with_input(&directory, &["apply", "--index", "--3way", "-"], &reverse)?;
        git(
            &directory,
            &[
                "-c",
                "user.name=rtao core patch",
                "-c",
                "user.email=core-patch@users.noreply.github.com",
                "commit",
                "-m",
                "Replace the previous core patch delta with its reviewed revision",
            ],
        )?;
    }
    // The combined review series, then the new core's reviewed adaptation.
    for commit in commits.iter().chain(&adaptation_commits) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&directory)
            .args([
                "-c",
                "user.name=rtao core patch",
                "-c",
                "user.email=core-patch@users.noreply.github.com",
                "cherry-pick",
                "--keep-redundant-commits",
                commit,
            ])
            .output()?;
        ensure!(
            output.status.success(),
            "core-patch-conflict: retained {}: {}",
            directory.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let rev = git_text(&directory, &["rev-parse", "HEAD"])?;
    verify_diff(&directory, &base, &rev)?;
    let entry = entry_for(&rev)?;
    verify_review(&directory, &entry)?;
    // A carried tree that pin --check would refuse is never offered for
    // publication: the new core's own sources must pass the erasure check.
    verify_security_source(&directory, &rev).with_context(|| {
        format!(
            "core-patch-security: the reviewed patch carried onto {target} fails the Ed25519 erasure check; the patch must be extended and reviewed for the new core before publication (retained {})",
            directory.display()
        )
    })?;
    write_candidate(&temporary, &directory, &base, start, &rev, &entry)?;
    git(
        &repo,
        &[
            "worktree",
            "remove",
            directory.to_str().context("non-UTF-8 task root")?,
        ],
    )?;
    bail!(
        "core-patch-publish-required: bounded candidate files are in {}/core-patch/candidate; publish with an ordinary fast-forward using existing authorized access, then rerun rebase",
        temporary.display()
    );
}

fn write_candidate(
    temporary: &Path,
    repo: &Path,
    base: &str,
    publication_parent: &str,
    rev: &str,
    entry: &Item,
) -> Result<()> {
    use sha2::Digest;
    let output = temporary.join("core-patch/candidate");
    fs::create_dir_all(&output)?;
    let patch = git(
        repo,
        &[
            "format-patch",
            "--stdout",
            "--binary",
            &format!("{publication_parent}..{rev}"),
        ],
    )?;
    ensure!(
        patch.len() <= 1_048_576,
        "core patch candidate exceeds 1 MiB; inspect the source before publication"
    );
    let mut pins = DocumentMut::new();
    pins["core"] = Item::Table(Table::new());
    pins["core"]["patch"] = entry.clone();
    fs::write(output.join("core.patch"), &patch)?;
    fs::write(output.join("identity.toml"), pins.to_string())?;
    let report = serde_json::json!({
        "schema": 1, "status":"publication-required", "repo":FORK,
        "branch":field(entry,"branch")?, "base":base, "publication_parent":publication_parent,
        "candidate_rev":rev, "candidate_rev_is_advisory":true,
        "upstream_pr":field(entry,"upstream_pr")?,
        "upstream_pr_rev":field(entry,"upstream_pr_rev")?,
        "security_issue":field(entry,"security_issue")?,
        "review_base":field(entry,"review_base")?, "review_rev":field(entry,"review_rev")?,
        "review_diff_sha256":field(entry,"review_diff_sha256")?,
        "patch_sha256":hex::encode(sha2::Sha256::digest(&patch)), "patch_bytes":patch.len(),
        "tests_executed":false, "emulator_evidence":false
    });
    fs::write(
        output.join("candidate.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "Retained bounded core patch candidate at {}",
        output.display()
    );
    Ok(())
}

fn rebase(root: &Path, pins: &DocumentMut, target: &str) -> Result<()> {
    if field(&pins["core"], "tag")? != target {
        // A new core release needs its complete pin/metadata/fingerprint
        // candidate. The pin pipeline calls rebase_candidate with both old and
        // new identities, then runs all candidate gates before adoption.
        return crate::pin::run(vec!["--parity".into(), target.into(), "--core-bump".into()]);
    }
    let mut next = pins.clone();
    rebase_candidate(root, pins, &mut next)?;
    atomic_write(&root.join("upstream.toml"), next.to_string().as_bytes())?;
    Ok(())
}

pub(crate) fn run(arguments: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    match arguments.as_slice() {
        [command, target] if command == "rebase" => {
            rebase(root, &read_document(&root.join("upstream.toml"))?, target)
        }
        [command] if command == "strip" => {
            hosted_root()?;
            ensure!(
                std::env::var("RTAO_CORE_UNPATCHED_DISPOSABLE").as_deref() == Ok("1"),
                "core-patch strip requires RTAO_CORE_UNPATCHED_DISPOSABLE=1 in a disposable hosted checkout"
            );
            let mut manifest = read_document(&root.join("Cargo.toml"))?;
            remove_tables(&mut manifest)?;
            atomic_write(&root.join("Cargo.toml"), manifest.to_string().as_bytes())?;
            let mut pins = read_document(&root.join("upstream.toml"))?;
            pins["core"]
                .as_table_like_mut()
                .context("core table missing")?
                .remove("patch");
            atomic_write(&root.join("upstream.toml"), pins.to_string().as_bytes())?;
            Ok(())
        }
        [command, rev, rest @ ..] if command == "verify-security" && rest.len() <= 1 => {
            // Read-only and local: runs the Ed25519 erasure check on a
            // carried tree before it is published (D-139). Without a
            // repository it fetches the commit from the fork into the
            // upstream cache; nothing is built.
            ensure!(hash(rev), "verify-security needs a full commit hash");
            let repo = match rest.first() {
                Some(path) => PathBuf::from(path),
                None => {
                    let repo = cached_repo(root)?;
                    git(&repo, &["fetch", "--filter=blob:none", FORK, rev])?;
                    repo
                }
            };
            verify_security_source(&repo, rev)?;
            println!("{rev}: the core sources pass the Ed25519 erasure check");
            Ok(())
        }
        _ => bail!(
            "usage: cargo xtask core-patch rebase <vN> | strip | verify-security <commit> [<repository>]"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_lock_bind_only_the_core_to_the_fork() -> Result<()> {
        let base = "1".repeat(40);
        let rev = "2".repeat(40);
        let mut pins: DocumentMut = format!("[core]\ntag='v478'\ncommit='{base}'\n").parse()?;
        let mut entry = Table::new();
        for (key, contents) in [
            ("repo", FORK),
            ("branch", "rtao-core-patch/v478"),
            ("rev", &rev),
            ("base", &base),
            ("upstream_pr", LEDGER_PR),
            ("upstream_pr_rev", &rev),
            ("security_issue", SECURITY_ISSUE),
            ("review_base", &base),
            ("review_rev", &rev),
            ("review_diff_sha256", &"a".repeat(64)),
        ] {
            entry.insert(key, value(contents));
        }
        for (key, values) in [
            ("paths", &PATHS[..]),
            ("probe", &PROBES[..]),
            ("shared", &SHARED_BASE[..]),
        ] {
            let mut array = Array::new();
            for path in values {
                array.push(*path);
            }
            entry.insert(key, value(array));
        }
        pins["core"]["patch"] = Item::Table(entry);
        let mut manifest: DocumentMut =
            "[patch.crates-io]\nw3f-bls={git='upstream',tag='v478'}\n".parse()?;
        let shared_names: BTreeSet<String> = SHARED_BASE.iter().map(|n| n.to_string()).collect();
        patch_tables(&mut manifest, "v478", &rev, &shared_names)?;
        verify_tables(&manifest, &pins)?;
        // A core with more path packages (v488's subtensor-hashed and
        // fips204) keeps every one of them at the core tag.
        let mut wider = pins.clone();
        let mut names = Array::new();
        for name in ["fips204", "subtensor-hashed", "subtensor-macros", "w3f-bls"] {
            names.push(name);
        }
        wider["core"]["patch"]["shared"] = value(names);
        assert!(verify_tables(&manifest, &wider).is_err());
        let mut wide_manifest = manifest.clone();
        patch_tables(
            &mut wide_manifest,
            "v478",
            &rev,
            &shared(&wider["core"]["patch"])?,
        )?;
        verify_tables(&wide_manifest, &wider)?;
        assert_eq!(
            wide_manifest["patch"][FORK]["subtensor-hashed"]["tag"].as_str(),
            Some("v478")
        );
        let mut lost = pins.clone();
        let mut names = Array::new();
        names.push("w3f-bls");
        lost["core"]["patch"]["shared"] = value(names);
        assert!(shared(&lost["core"]["patch"]).is_err());
        let source = expected_core_source(&pins)?;
        let lock: DocumentMut =
            format!("[[package]]\nname='bittensor-core'\nsource='{source}'\n").parse()?;
        verify_lock(&lock, &pins)?;
        let wrong: DocumentMut = format!(
            "{}\n[[package]]\nname='subtensor-macros'\nsource='{source}'\n",
            lock
        )
        .parse()?;
        assert!(verify_lock(&wrong, &pins).is_err());
        manifest["patch"][FORK]["w3f-bls"]["tag"] = value("v477");
        assert!(verify_tables(&manifest, &pins).is_err());
        pins["core"]["patch"]["base"] = value("3".repeat(40));
        assert!(expected_core_source(&pins).is_err());
        Ok(())
    }

    #[test]
    fn only_missing_symbols_in_the_exact_probe_count_as_absence() -> Result<()> {
        use serde_json::json;
        let directory = Path::new("/task/probe");
        let record = json!({
            "reason": "compiler-message",
            "package_id": "path+file:///task/probe#rtao-core-patch-probe@0.0.0",
            "manifest_path": "/task/probe/Cargo.toml",
            "target": {"name":"rtao-core-patch-probe","kind":["bin"],"src_path":"/task/probe/src/main.rs"},
            "message": {"level":"error","code":{"code":"E0405"},"spans":[{"file_name":"src/main.rs","is_primary":true}]}
        });
        assert!(missing_symbols(record.to_string().as_bytes(), directory)?);
        for (pointer, value) in [
            ("/message/code/code", json!("E0277")),
            (
                "/package_id",
                json!("path+file:///task/dependency#dependency@0.0.0"),
            ),
            ("/manifest_path", json!("/task/dependency/Cargo.toml")),
            ("/target/src_path", json!("/task/dependency/src/main.rs")),
            ("/target/kind", json!(["custom-build"])),
            (
                "/message/spans/0/file_name",
                json!("/cargo/dep/src/main.rs"),
            ),
            ("/message/spans/0/is_primary", json!(false)),
        ] {
            let mut changed = record.clone();
            *changed
                .pointer_mut(pointer)
                .context("fixture pointer missing")? = value;
            assert!(!missing_symbols(changed.to_string().as_bytes(), directory)?);
            let combined = format!("{record}\n{changed}");
            assert!(!missing_symbols(combined.as_bytes(), directory)?);
        }
        assert!(!missing_symbols(b"download failed", directory)?);
        Ok(())
    }

    #[test]
    fn ed25519_source_guard_requires_wiping_ownership_and_chain_vectors() -> Result<()> {
        let manifest = "ed25519-zebra = { version = \"4.0\", default-features = false }";
        let keys = r#"
use ed25519_zebra::{SigningKey, VerificationKey};
use sp_core::ed25519;
struct WipingEd25519Pair { secret: Zeroizing<SigningKey>, }
enum KeypairInner { Ed25519(Zeroizing<SigningKey>), PublicOnly }
fn verify() -> bool { ed25519::Pair::verify(&s, m, &p) }
const V: [&str; 4] = [
    "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    "88dc3417d5058ec4b4503e0c12ea1a0a89be200fe98922423d4334014fa6b0ee",
];
#[test] fn ed25519_signing_object_has_a_wiping_owner_and_keeps_chain_bytes() {}
#[test] fn zeroize_clears_every_signing_key_secret() {}
#[test] fn hard_derivation_keeps_substrate_vectors() {}
"#;
        let encrypted = "fn f() { WipingEd25519Pair::from_seed_slice(seed); KeypairInner::Ed25519(pair.into_secret()); Zeroizing::new([0u8; SEC_LENGTH]); }";
        let sources = |keys: &str, other: &str| {
            vec![
                (
                    "sdk/bittensor-core/src/keys/mod.rs".to_owned(),
                    keys.to_owned(),
                ),
                (
                    "sdk/bittensor-core/src/keys/encrypted_json.rs".to_owned(),
                    encrypted.to_owned(),
                ),
                (
                    "sdk/bittensor-core/src/other.rs".to_owned(),
                    other.to_owned(),
                ),
            ]
        };
        verify_security_source_text(manifest, &sources(keys, ""))?;
        for mutant in [
            keys.replace("Ed25519(Zeroizing<SigningKey>)", "Ed25519(ed25519::Pair)"),
            keys.replace(
                "Ed25519(Zeroizing<SigningKey>)",
                "Ed25519(sp_core::ed25519::Pair)",
            ),
            keys.replace(
                "Ed25519(Zeroizing<SigningKey>)",
                "Ed25519(Zeroizing<SigningKey>, ed25519::Pair)",
            ),
            keys.replace("secret: Zeroizing<SigningKey>,", "secret: SigningKey,"),
            keys.replace("fn zeroize_clears_every_signing_key_secret", "fn renamed"),
            keys.replace("#[test] fn hard_derivation", "fn hard_derivation"),
            keys.replace("88dc3417", "00dc3417"),
            format!("{keys}\ntype Alias = ed25519::Pair;"),
            format!("{keys}\nuse sp_core::ed25519::Pair as Hidden;"),
            format!("{keys}\nfn d() {{ (a, b).using_encoded(sp_core::hashing::blake2_256); }}"),
        ] {
            assert!(verify_security_source_text(manifest, &sources(&mutant, "")).is_err());
        }
        assert!(
            verify_security_source_text(
                manifest,
                &sources(keys, "struct Elsewhere { pair: sp_core::ed25519::Pair }")
            )
            .is_err()
        );
        let mut missing = sources(keys, "");
        missing[1].1 = "fn f() {}".to_owned();
        assert!(verify_security_source_text(manifest, &missing).is_err());

        // v488's hashed accounts (D-139): the derivation returns the wiping
        // owner and the regression test with its v488 vectors remains.
        let hashed = r#"
use ed25519_zebra::{SigningKey, VerificationKey};
impl HashedKeypair {
    fn derive_ed(master: &[u8; 32], generation: u64) -> Result<Zeroizing<SigningKey>, CoreError> { todo!() }
    fn ed_at(&self, generation: u64) -> Result<Zeroizing<SigningKey>, CoreError> { todo!() }
}
fn check() -> bool { ed25519::Pair::verify(&s, m, &p) }
mod tests {
    #[test]
    fn hashed_ed25519_signing_key_has_a_wiping_owner_and_keeps_vectors() {
        let v = [
            "8b9c4ae043290453eb92a40d48f5074178031bc6623e60544ae4a2f5ab6542a1",
            "87ba9efaab5ed270bf9fa5e8a4101283cd6ba1541f9166177778f1ca47aa88c7",
            "5EPuttWmgUjN8Wq2qKELKso6WrfRpBpgZpuNRzAwyAgQTXzo",
        ];
    }
}
"#;
        let with_hashed = |hashed: &str| {
            let mut all = sources(keys, "");
            all.push((HASHED.to_owned(), hashed.to_owned()));
            all
        };
        verify_security_source_text(manifest, &with_hashed(hashed))?;
        for mutant in [
            // v488 as released: derives the non-wiping sp_core pair.
            hashed.replace(
                "Result<Zeroizing<SigningKey>, CoreError> { todo!() }\n    fn ed_at",
                "Result<ed25519::Pair, CoreError> { todo!() }\n    fn ed_at",
            ),
            hashed.replace(
                "fn ed_at(&self, generation: u64) -> Result<Zeroizing<SigningKey>, CoreError>",
                "fn ed_at(&self, generation: u64) -> Result<SigningKey, CoreError>",
            ),
            hashed.replace("fn ed_at", "fn renamed_ed_at"),
            hashed.replace("    #[test]\n", ""),
            hashed.replace("8b9c4ae0", "0b9c4ae0"),
            format!("{hashed}\nfn leak() -> sp_core::ed25519::Pair {{ todo!() }}"),
        ] {
            assert!(
                verify_security_source_text(manifest, &with_hashed(&mutant)).is_err(),
                "{mutant}"
            );
        }
        Ok(())
    }

    #[test]
    fn adaptations_are_exact_and_the_latest_recorded_for_a_core_applies() -> Result<()> {
        let one = |tag: &str, base: char, rev: char| {
            format!(
                "[[adaptation]]\ntag = '{tag}'\nbase = '{}'\nrev = '{}'\ndiff_sha256 = '{}'\n",
                base.to_string().repeat(40),
                rev.to_string().repeat(40),
                "a".repeat(64)
            )
        };
        let parse = |text: &str| -> Result<Vec<Adaptation>> {
            let document: DocumentMut = text.parse()?;
            adaptations(document.as_item())
        };
        assert!(parse("")?.is_empty());
        let recorded = parse(&format!(
            "{}{}",
            one("v500", '3', '4'),
            one("v488", '1', '2')
        ))?;
        assert_eq!(
            recorded.iter().map(|a| a.tag.as_str()).collect::<Vec<_>>(),
            ["v488", "v500"]
        );
        assert_eq!(applicable(&recorded, "v478")?, None);
        assert_eq!(
            applicable(&recorded, "v488")?.map(|a| a.tag.as_str()),
            Some("v488")
        );
        assert_eq!(
            applicable(&recorded, "v499")?.map(|a| a.tag.as_str()),
            Some("v488")
        );
        assert_eq!(
            applicable(&recorded, "v501")?.map(|a| a.tag.as_str()),
            Some("v500")
        );
        for bad in [
            format!("{}{}", one("v488", '1', '2'), one("v488", '3', '4')),
            one("488", '1', '2'),
            one("v488", '1', '1'),
            one("v488", 'g', '2'),
            one("v488", '1', '2').replace(&"a".repeat(64), &"A".repeat(64)),
            format!("{}extra = 'x'\n", one("v488", '1', '2')),
            one("v488", '1', '2').replace("tag = 'v488'\n", ""),
            "adaptation = []\n".to_owned(),
            (0..5).map(|n| one(&format!("v48{n}"), '1', '2')).collect(),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
        let table = adaptation_table(&recorded);
        let mut document = DocumentMut::new();
        document["adaptation"] = table;
        assert_eq!(adaptations(document.as_item())?, recorded);
        Ok(())
    }

    #[test]
    fn hosted_git_fixture_proves_exact_trees_path_modes_and_append_only_history() -> Result<()> {
        let root = hosted_root()?.join("core-patch/source-fixture");
        ensure!(!root.exists(), "hosted source fixture already exists");
        fs::create_dir_all(&root)?;
        let result = (|| -> Result<()> {
            git(&root, &["init", "--quiet"])?;
            git(&root, &["config", "user.name", "rtao source fixture"])?;
            git(
                &root,
                &[
                    "config",
                    "user.email",
                    "source-fixture@users.noreply.github.com",
                ],
            )?;
            let source = root.join(PATHS[1]);
            fs::create_dir_all(source.parent().context("fixture source parent")?)?;
            fs::write(&source, "pub fn open() {}\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "base"])?;
            let base = git_text(&root, &["rev-parse", "HEAD"])?;
            fs::write(
                &source,
                "pub fn open() {}\npub const LABEL: &str = \"APDU bytes\";\n",
            )?;
            git(&root, &["commit", "--quiet", "-am", "reviewed patch"])?;
            let review = git_text(&root, &["rev-parse", "HEAD"])?;
            let digest = diff_digest(&root, &base, &review)?;
            let entry = |base: &str, revision: &str| {
                let mut entry = Table::new();
                for (key, value) in [
                    ("base", base),
                    ("rev", revision),
                    ("review_base", base),
                    ("review_rev", review.as_str()),
                    ("review_diff_sha256", digest.as_str()),
                ] {
                    entry.insert(key, super::value(value));
                }
                Item::Table(entry)
            };
            verify_diff(&root, &base, &review)?;
            verify_review(&root, &entry(&base, &review))?;
            // patch-id ignores this doubled space inside a string literal;
            // exact tree equality must reject the changed runtime value.
            fs::write(
                &source,
                "pub fn open() {}\npub const LABEL: &str = \"APDU  bytes\";\n",
            )?;
            git(&root, &["commit", "--quiet", "-am", "altered literal"])?;
            let altered = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_review(&root, &entry(&base, &altered)).is_err());
            assert!(ancestor(&root, &review, &altered)?);
            git(&root, &["checkout", "--quiet", "--detach", &base])?;
            fs::write(root.join("release.txt"), "unrelated new core release\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "new release base"])?;
            let next_base = git_text(&root, &["rev-parse", "HEAD"])?;
            git(&root, &["cherry-pick", &review])?;
            let rebased = git_text(&root, &["rev-parse", "HEAD"])?;
            let mut rebased_entry = entry(&base, &rebased);
            rebased_entry["base"] = value(&next_base);
            verify_diff(&root, &next_base, &rebased)?;
            verify_review(&root, &rebased_entry)?;
            assert!(!ancestor(&root, &review, &rebased)?);
            assert!(ancestor(&root, &next_base, &rebased)?);
            fs::write(root.join("forbidden.txt"), "unexpected patch content\n")?;
            git(&root, &["add", "."])?;
            git(&root, &["commit", "--quiet", "-m", "forbidden path"])?;
            let forbidden = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_diff(&root, &next_base, &forbidden).is_err());
            git(&root, &["checkout", "--quiet", "--detach", &rebased])?;
            git(&root, &["update-index", "--chmod=+x", PATHS[1]])?;
            git(
                &root,
                &["commit", "--quiet", "-m", "forbidden executable mode"],
            )?;
            let executable = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_diff(&root, &next_base, &executable).is_err());

            // D-139: a later core adds a source absent at the review base.
            // The review series cannot add or touch it; a reviewed adaptation
            // after the carried review may modify it.
            // --force: the index-only mode change above left the work tree
            // file with its old mode.
            git(
                &root,
                &["checkout", "--quiet", "--force", "--detach", &next_base],
            )?;
            let hashed = root.join(HASHED);
            fs::create_dir_all(hashed.parent().context("fixture hashed parent")?)?;
            fs::write(&hashed, "pub fn derive() -> Pair {}\n")?;
            git(&root, &["add", "."])?;
            git(
                &root,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "core release with a new key source",
                ],
            )?;
            let newer_base = git_text(&root, &["rev-parse", "HEAD"])?;
            assert!(verify_diff(&root, &next_base, &newer_base).is_err());
            git(&root, &["cherry-pick", &review])?;
            let carried = git_text(&root, &["rev-parse", "HEAD"])?;
            fs::write(&hashed, "pub fn derive() -> Zeroizing<SigningKey> {}\n")?;
            git(&root, &["commit", "--quiet", "-am", "reviewed adaptation"])?;
            let adapted = git_text(&root, &["rev-parse", "HEAD"])?;
            let adaptation = Adaptation {
                tag: "v488".into(),
                base: carried.clone(),
                rev: adapted.clone(),
                diff_sha256: diff_digest(&root, &carried, &adapted)?,
            };
            assert_eq!(
                verify_adaptation(&root, &adaptation)?,
                std::slice::from_ref(&adapted)
            );
            let adapted_entry = |tag: &str, rev: &str, adaptation: Option<&Adaptation>| {
                let mut entry = entry(&base, rev);
                entry["base"] = value(&newer_base);
                entry["branch"] = value(format!("rtao-core-patch/{tag}"));
                if let Some(adaptation) = adaptation {
                    entry["adaptation"] = adaptation_table(std::slice::from_ref(adaptation));
                }
                entry
            };
            verify_diff(&root, &newer_base, &adapted)?;
            verify_review(&root, &adapted_entry("v488", &adapted, Some(&adaptation)))?;
            // Without the adaptation, or on a core before its tag, the
            // adapted tree is not the reviewed one; the carried review is.
            assert!(verify_review(&root, &adapted_entry("v488", &adapted, None)).is_err());
            assert!(
                verify_review(&root, &adapted_entry("v487", &adapted, Some(&adaptation))).is_err()
            );
            verify_review(&root, &adapted_entry("v487", &carried, Some(&adaptation)))?;
            // A recorded digest binds the adaptation's exact bytes.
            let mut wrong = adaptation.clone();
            wrong.diff_sha256 = "0".repeat(64);
            assert!(verify_review(&root, &adapted_entry("v488", &adapted, Some(&wrong))).is_err());
            // An adaptation may not reach outside the allowlist or add files.
            fs::write(
                root.join("forbidden.txt"),
                "unexpected adaptation content\n",
            )?;
            git(&root, &["add", "."])?;
            git(
                &root,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "adaptation outside the allowlist",
                ],
            )?;
            let outside = Adaptation {
                rev: git_text(&root, &["rev-parse", "HEAD"])?,
                diff_sha256: diff_digest(
                    &root,
                    &carried,
                    &git_text(&root, &["rev-parse", "HEAD"])?,
                )?,
                ..adaptation.clone()
            };
            assert!(verify_adaptation(&root, &outside).is_err());
            Ok(())
        })();
        // Every child above is synchronous and has exited. Only this exact
        // fixture directory was created, and it contains no external links.
        fs::remove_dir_all(&root)?;
        result
    }
}
