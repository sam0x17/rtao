//! The crates.io republish of the core graph (D-183).
//!
//! crates.io refuses Git dependencies, so `bittensor-core` and the five
//! packages it takes from Git (`subtensor-hashed`, `subtensor-macros`, the
//! monorepo's vendored `fips204` and `w3f-bls` forks, and `tle`) are
//! republished as `rtao-upstream-*` packages. Each package under `upstream/`
//! is generated from one reviewed revision: every file except `Cargo.toml`,
//! `NOTICE` and `MODIFICATIONS` is that revision's file byte for byte, and the
//! manifest is derived from the original one. `upstream.toml` `[publish]`
//! records each package's version, source and, once published, the SHA256 of
//! its `.crate`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::pin::{cached_repo, git, read_document};

const UPSTREAM: &str = "https://github.com/RaoFoundation/subtensor.git";
const FORK: &str = "https://github.com/sam0x17/subtensor.git";
const TIMELOCK: &str = "https://github.com/ideal-lab5/timelock";
const POLKADOT_SDK: &str = "https://github.com/RaoFoundation/polkadot-sdk.git";
const RTAO: &str = "https://github.com/sam0x17/rtao";
/// Git packages only xtask reaches: its linting crate and that crate's own
/// dependencies (never published, D-183).
const XTASK_ONLY: [&str; 3] = ["subtensor-linting", "procedural-fork", "sp-crypto-hashing"];
/// Directory of the generated packages, relative to the workspace root.
pub(crate) const DIRECTORY: &str = "upstream";
const GENERATED: [&str; 3] = ["Cargo.toml", "NOTICE", "MODIFICATIONS"];
/// Crate subdirectories that only upstream's own tests, benchmarks and
/// examples use; the republished library never reads them.
const OMITTED: [&str; 4] = ["tests", "benches", "examples", "fixtures"];

/// Where a republished package's sources come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Origin {
    /// The monorepo at `[core].commit`.
    Monorepo,
    /// The reviewed core: `[core.patch].rev` on the fork, else `[core].commit`.
    Core,
    /// ideal-lab5/timelock at the revision the monorepo's workspace pins.
    Timelock,
}

/// One republished package.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fork {
    /// Short name: `upstream/<key>`, `rtao-upstream-<key>`, `[publish.<key>]`.
    pub(crate) key: &'static str,
    /// The upstream package name, kept as the dependency name and library name.
    pub(crate) original: &'static str,
    /// The package directory inside its source repository.
    path: &'static str,
    origin: Origin,
    /// Who maintains the original, for the description and NOTICE.
    project: &'static str,
}

impl Fork {
    /// The crates.io package name.
    pub(crate) fn package(&self) -> String {
        format!("rtao-upstream-{}", self.key)
    }

    /// The upstream library name, which the republished library keeps.
    fn library(&self) -> String {
        self.original.replace('-', "_")
    }
}

/// Every republished package, in publication order: each depends only on
/// packages before it.
pub(crate) const FORKS: [Fork; 6] = [
    Fork {
        key: "macros",
        original: "subtensor-macros",
        path: "support/macros",
        origin: Origin::Monorepo,
        project: "RaoFoundation/subtensor",
    },
    Fork {
        key: "fips204",
        original: "fips204",
        path: "vendor/fips204",
        origin: Origin::Monorepo,
        project: "RaoFoundation/subtensor (vendored from integritychain/fips204)",
    },
    Fork {
        key: "w3f-bls",
        original: "w3f-bls",
        path: "vendor/w3f-bls",
        origin: Origin::Monorepo,
        project: "RaoFoundation/subtensor (vendored from w3f/bls)",
    },
    Fork {
        key: "hashed",
        original: "subtensor-hashed",
        path: "primitives/hashed",
        origin: Origin::Monorepo,
        project: "RaoFoundation/subtensor",
    },
    Fork {
        key: "tle",
        original: "tle",
        path: "tle",
        origin: Origin::Timelock,
        project: "ideal-lab5/timelock",
    },
    Fork {
        key: "core",
        original: "bittensor-core",
        path: "sdk/bittensor-core",
        origin: Origin::Core,
        project: "RaoFoundation/subtensor",
    },
];

/// The fork with this key.
pub(crate) fn fork(key: &str) -> Result<&'static Fork> {
    FORKS
        .iter()
        .find(|fork| fork.key == key)
        .with_context(|| format!("no republished package {key}"))
}

/// The key of the republished package of an original package name.
pub(crate) fn fork_key(original: &str) -> &'static str {
    FORKS
        .iter()
        .find(|fork| fork.original == original)
        .map_or("", |fork| fork.key)
}

/// The packages rtao's own crates depend on, as `(dependency name, key)`.
pub(crate) const WORKSPACE_DEPENDENCIES: [(&str, &str); 3] = [
    ("bittensor-core", "core"),
    ("w3f-bls", "w3f-bls"),
    ("tle", "tle"),
];

/// A resolved source revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) repo: String,
    pub(crate) rev: String,
}

fn field<'a>(item: &'a Item, key: &str) -> Result<&'a str> {
    item.get(key)
        .and_then(Item::as_str)
        .with_context(|| format!("upstream.toml is missing {key}"))
}

fn full_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The core tag's release number, the minor version of every package.
fn release(pins: &DocumentMut) -> Result<u64> {
    let tag = field(&pins["core"], "tag")?;
    tag.strip_prefix('v')
        .and_then(|number| number.parse().ok())
        .with_context(|| format!("core tag {tag} is not vN"))
}

/// The recorded `[publish.<key>]` entry.
fn entry<'a>(pins: &'a DocumentMut, fork: &Fork) -> Result<&'a Item> {
    pins.get("publish")
        .and_then(|publish| publish.get(fork.key))
        .with_context(|| format!("upstream.toml has no [publish.{}]", fork.key))
}

/// The recorded version of a package.
pub(crate) fn version<'a>(pins: &'a DocumentMut, fork: &Fork) -> Result<&'a str> {
    let version = field(entry(pins, fork)?, "version")?;
    let semver = semver::Version::parse(version)
        .with_context(|| format!("[publish.{}] version {version} is not semver", fork.key))?;
    ensure!(
        semver.major == 0 && semver.minor == release(pins)? && semver.pre.is_empty(),
        "[publish.{}] version {version} must be 0.{}.<revision>",
        fork.key,
        release(pins)?
    );
    Ok(version)
}

/// The recorded SHA256 of the published `.crate`, if any.
pub(crate) fn checksum<'a>(pins: &'a DocumentMut, fork: &Fork) -> Result<Option<&'a str>> {
    let Some(checksum) = entry(pins, fork)?.get("checksum") else {
        return Ok(None);
    };
    let checksum = checksum
        .as_str()
        .context("[publish] checksum must be a string")?;
    ensure!(
        checksum.len() == 64
            && checksum
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "[publish.{}] checksum must be a lowercase SHA256",
        fork.key
    );
    Ok(Some(checksum))
}

/// The reviewed polkadot-sdk revision and the crates.io release of each of
/// its crates that stands in for it.
fn polkadot_sdk(pins: &DocumentMut) -> Result<(&str, BTreeMap<String, String>)> {
    let publish = pins
        .get("publish")
        .context("upstream.toml has no [publish]")?;
    let rev = field(publish, "polkadot_sdk_rev")?;
    ensure!(
        full_commit(rev),
        "[publish] polkadot_sdk_rev must be a full commit"
    );
    let crates = publish
        .get("polkadot_sdk")
        .and_then(Item::as_table_like)
        .context("upstream.toml has no [publish] polkadot_sdk")?
        .iter()
        .map(|(name, version)| {
            Ok((
                name.to_owned(),
                version
                    .as_str()
                    .context("[publish] polkadot_sdk versions must be strings")?
                    .to_owned(),
            ))
        })
        .collect::<Result<_>>()?;
    Ok((rev, crates))
}

/// The repository and revision a package is generated from, derived from the
/// core pin, independently of the `[publish]` record.
pub(crate) fn derived_source(repo: &Path, pins: &DocumentMut, fork: &Fork) -> Result<Source> {
    let core = &pins["core"];
    let commit = field(core, "commit")?;
    ensure!(full_commit(commit), "core.commit must be a full commit");
    Ok(match fork.origin {
        Origin::Monorepo => Source {
            repo: UPSTREAM.into(),
            rev: commit.into(),
        },
        Origin::Core => match core.get("patch") {
            Some(patch) => Source {
                repo: field(patch, "repo")?.into(),
                rev: field(patch, "rev")?.into(),
            },
            None => Source {
                repo: UPSTREAM.into(),
                rev: commit.into(),
            },
        },
        Origin::Timelock => {
            let workspace = manifest_at(repo, commit, "Cargo.toml")?;
            let tle = &workspace["workspace"]["dependencies"]["tle"];
            ensure!(
                tle.get("git").and_then(Item::as_str) == Some(TIMELOCK),
                "the monorepo's tle no longer comes from {TIMELOCK}"
            );
            let rev = field(tle, "rev")?;
            ensure!(
                full_commit(rev),
                "the monorepo's tle must pin a full commit"
            );
            Source {
                repo: TIMELOCK.into(),
                rev: rev.into(),
            }
        }
    })
}

/// The recorded source of a package, which must equal the derived one.
pub(crate) fn recorded_source(pins: &DocumentMut, fork: &Fork) -> Result<Source> {
    let entry = entry(pins, fork)?;
    ensure!(
        field(entry, "package")? == fork.package(),
        "[publish.{}] package must be {}",
        fork.key,
        fork.package()
    );
    ensure!(
        field(entry, "path")? == fork.path,
        "[publish.{}] path must be {}",
        fork.key,
        fork.path
    );
    let source = Source {
        repo: field(entry, "repo")?.into(),
        rev: field(entry, "rev")?.into(),
    };
    ensure!(
        [UPSTREAM, FORK, TIMELOCK].contains(&source.repo.as_str()) && full_commit(&source.rev),
        "[publish.{}] must name a known repository and a full commit",
        fork.key
    );
    Ok(source)
}

/// The Git object store holding a repository's reviewed commits: the shared
/// partial clone of the monorepo (which also receives the fork's commits),
/// or a sibling partial clone of ideal-lab5/timelock.
fn store(root: &Path, repo: &str) -> Result<PathBuf> {
    let monorepo = cached_repo(root)?;
    if repo != TIMELOCK {
        return Ok(monorepo);
    }
    let path = monorepo
        .parent()
        .context("upstream cache has no parent")?
        .join("timelock.git");
    if !path.exists() {
        let output = Command::new("git")
            .args([
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                "--quiet",
                TIMELOCK,
            ])
            .arg(&path)
            .output()?;
        ensure!(
            output.status.success(),
            "partial clone of {TIMELOCK} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let origin = String::from_utf8(git(&path, &["remote", "get-url", "origin"])?)?;
    ensure!(
        origin.trim() == TIMELOCK,
        "timelock cache has an unexpected origin"
    );
    Ok(path)
}

/// Make sure a commit is present, fetching it from its repository if not.
fn ensure_commit(store: &Path, source: &Source) -> Result<()> {
    let object = format!("{}^{{commit}}", source.rev);
    if git(store, &["cat-file", "-e", &object]).is_err() {
        git(
            store,
            &[
                "fetch",
                "--quiet",
                "--filter=blob:none",
                &source.repo,
                &source.rev,
            ],
        )?;
    }
    git(store, &["cat-file", "-e", &object])
        .with_context(|| format!("{} is missing from {}", source.rev, source.repo))?;
    Ok(())
}

fn show(store: &Path, rev: &str, path: &str) -> Result<Vec<u8>> {
    git(store, &["show", &format!("{rev}:{path}")])
}

fn manifest_at(store: &Path, rev: &str, path: &str) -> Result<DocumentMut> {
    String::from_utf8(show(store, rev, path)?)
        .with_context(|| format!("{path} at {rev} is not UTF-8"))?
        .parse()
        .with_context(|| format!("parse {path} at {rev}"))
}

/// Regular files below `path` at `rev`, relative to `path`.
fn files(store: &Path, rev: &str, path: &str) -> Result<Vec<String>> {
    let listing = String::from_utf8(git(store, &["ls-tree", "-r", "-z", rev, "--", path])?)?;
    let mut files = Vec::new();
    for record in listing.split('\0').filter(|record| !record.is_empty()) {
        let (meta, name) = record.split_once('\t').context("malformed git ls-tree")?;
        let mode = meta.split_whitespace().next().unwrap_or_default();
        let relative = name
            .strip_prefix(path)
            .and_then(|name| name.strip_prefix('/'))
            .with_context(|| format!("{name} is outside {path}"))?;
        ensure!(
            mode == "100644" || mode == "100755",
            "{name} at {rev} is not a regular file (mode {mode})"
        );
        files.push(relative.to_owned());
    }
    Ok(files)
}

/// Whether a source file belongs in the published package.
fn included(relative: &str) -> bool {
    match relative.split_once('/') {
        Some((directory, _)) => !OMITTED.contains(&directory),
        None => relative != "Cargo.toml" && relative != "Cargo.lock",
    }
}

/// One generated package: its files and the upstream file each one copies.
pub(crate) struct Package {
    pub(crate) fork: Fork,
    pub(crate) source: Source,
    /// Package-relative path to content.
    pub(crate) files: BTreeMap<String, Vec<u8>>,
    /// Package-relative path to the repository path it copies at `source.rev`
    /// (all files except the generated ones).
    pub(crate) copied: BTreeMap<String, String>,
    /// The original manifest, for the Ed25519 erasure check of the core.
    pub(crate) original_manifest: String,
}

/// Generate every package from the cache.
pub(crate) fn generate(root: &Path, pins: &DocumentMut) -> Result<Vec<Package>> {
    let monorepo = cached_repo(root)?;
    let commit = field(&pins["core"], "commit")?;
    ensure_commit(
        &monorepo,
        &Source {
            repo: UPSTREAM.into(),
            rev: commit.into(),
        },
    )?;
    let workspace = manifest_at(&monorepo, commit, "Cargo.toml")?;
    let (sdk_rev, sdk) = polkadot_sdk(pins)?;
    let mut packages = Vec::new();
    for fork in FORKS {
        let source = derived_source(&monorepo, pins, &fork)?;
        let recorded = recorded_source(pins, &fork)?;
        ensure!(
            recorded == source,
            "[publish.{}] records {} {} but the core pin gives {} {}; run cargo xtask forks refresh",
            fork.key,
            recorded.repo,
            recorded.rev,
            source.repo,
            source.rev
        );
        let store = store(root, &source.repo)?;
        ensure_commit(&store, &source)?;
        let original_text = String::from_utf8(show(
            &store,
            &source.rev,
            &format!("{}/Cargo.toml", fork.path),
        )?)
        .context("original manifest is not UTF-8")?;
        let original: DocumentMut = original_text.parse()?;
        ensure!(
            original["package"]["name"].as_str() == Some(fork.original),
            "{}/Cargo.toml at {} is not {}",
            fork.path,
            source.rev,
            fork.original
        );
        let context = Context_ {
            pins,
            fork: &fork,
            source: &source,
            workspace: (fork.origin != Origin::Timelock).then_some(&workspace),
            sdk_rev,
            sdk: &sdk,
        };
        let mut files = BTreeMap::new();
        let mut copied = BTreeMap::new();
        let mut licensed = false;
        let mut omitted = BTreeSet::new();
        for relative in files_of(&store, &source.rev, fork.path)? {
            if !included(&relative) {
                if let Some((directory, _)) = relative.split_once('/') {
                    omitted.insert(directory.to_owned());
                }
                continue;
            }
            ensure!(
                !GENERATED.contains(&relative.as_str()),
                "{}/{relative} collides with a generated file",
                fork.path
            );
            licensed |= relative.starts_with("LICENSE");
            let path = format!("{}/{relative}", fork.path);
            files.insert(relative.clone(), show(&store, &source.rev, &path)?);
            copied.insert(relative, path);
        }
        if !licensed {
            // The repository's licence covers a package that has no copy.
            files.insert("LICENSE".into(), show(&store, &source.rev, "LICENSE")?);
            copied.insert("LICENSE".into(), "LICENSE".into());
        }
        // Files the reviewed core patch changed from the monorepo commit it is
        // based on, which MODIFICATIONS lists (Apache-2.0 4(b)).
        let mut patched = Vec::new();
        if fork.origin == Origin::Core && source.rev != commit {
            for (relative, path) in &copied {
                let object = format!("{commit}:{path}");
                let unchanged = git(&store, &["cat-file", "-e", &object]).is_ok()
                    && Some(&show(&store, commit, path)?) == files.get(relative);
                if !unchanged {
                    patched.push(relative.clone());
                }
            }
        }
        let (manifest, notes) =
            context.manifest(&original, &original_text, files.contains_key("README.md"))?;
        files.insert("Cargo.toml".into(), manifest.into_bytes());
        let declared = context.license(&original["package"])?;
        files.insert(
            "NOTICE".into(),
            context.notice(!licensed, &declared)?.into_bytes(),
        );
        files.insert(
            "MODIFICATIONS".into(),
            context
                .modifications(&notes, &patched, &omitted, commit, !licensed)?
                .into_bytes(),
        );
        packages.push(Package {
            fork,
            source,
            files,
            copied,
            original_manifest: original_text,
        });
    }
    Ok(packages)
}

fn files_of(store: &Path, rev: &str, path: &str) -> Result<Vec<String>> {
    files(store, rev, path)
}

#[allow(non_camel_case_types)]
struct Context_<'a> {
    pins: &'a DocumentMut,
    fork: &'a Fork,
    source: &'a Source,
    /// The monorepo workspace manifest, for workspace inheritance.
    workspace: Option<&'a DocumentMut>,
    sdk_rev: &'a str,
    sdk: &'a BTreeMap<String, String>,
}

/// One translated dependency.
enum Target {
    /// Another republished package.
    Fork(&'static Fork),
    /// A crates.io package with this version requirement.
    Registry(String),
}

impl Context_<'_> {
    /// Where the source comes from, for the description and NOTICE.
    fn origin(&self) -> Result<String> {
        let tag = field(&self.pins["core"], "tag")?;
        Ok(match self.fork.origin {
            Origin::Monorepo => match self.fork.project.split_once(" (") {
                Some((project, vendored)) => format!("{project} {tag} ({vendored}"),
                None => format!("{} {tag}", self.fork.project),
            },
            Origin::Core if self.source.repo == FORK => format!(
                "{} {tag} with rtao's reviewed core patch (the Ed25519 erasure repair and the injectable Ledger transport)",
                self.fork.project
            ),
            Origin::Core => format!("{} {tag}", self.fork.project),
            Origin::Timelock => format!(
                "{} at the revision RaoFoundation/subtensor {tag} pins",
                self.fork.project
            ),
        })
    }

    fn description(&self) -> Result<String> {
        Ok(format!(
            "Unofficial republish of {} from {}, for rtao; not affiliated with or endorsed by its authors.",
            self.fork.original,
            self.origin()?
        ))
    }

    fn homepage(&self) -> &'static str {
        match self.fork.origin {
            Origin::Timelock => "https://github.com/ideal-lab5/timelock",
            _ => "https://github.com/RaoFoundation/subtensor",
        }
    }

    /// A workspace-inherited package field.
    fn inherited<'b>(&'b self, package: &'b Item, key: &str) -> Result<Option<&'b Item>> {
        let Some(item) = package.get(key) else {
            return Ok(None);
        };
        if item.get("workspace").and_then(Item::as_bool) == Some(true) {
            let workspace = self
                .workspace
                .context("workspace-inherited field outside the monorepo")?;
            return Ok(Some(
                workspace["workspace"]["package"]
                    .get(key)
                    .with_context(|| format!("workspace.package.{key} missing"))?,
            ));
        }
        Ok(Some(item))
    }

    /// The license expression: the package's, else the repository's.
    fn license(&self, package: &Item) -> Result<String> {
        Ok(match self.inherited(package, "license")? {
            Some(license) => license
                .as_str()
                .context("license must be a string")?
                .replace("MIT/Apache-2.0", "MIT OR Apache-2.0"),
            None => "Apache-2.0".into(),
        })
    }

    fn manifest(
        &self,
        original: &DocumentMut,
        original_text: &str,
        readme: bool,
    ) -> Result<(String, Vec<String>)> {
        let fork = self.fork;
        let package = &original["package"];
        let mut notes = vec![format!(
            "[package]: renamed from {} to {}, version {}, with a new description, repository, homepage and documentation; `publish = false` removed",
            fork.original,
            fork.package(),
            version(self.pins, fork)?
        )];
        let mut out = DocumentMut::new();
        let mut table = Table::new();
        table.insert("name", value(fork.package()));
        table.insert("version", value(version(self.pins, fork)?));
        let edition = self
            .inherited(package, "edition")?
            .and_then(Item::as_str)
            .unwrap_or("2015");
        if package
            .get("edition")
            .and_then(|item| item.get("workspace"))
            .is_some()
        {
            notes.push(format!(
                "[package]: edition {edition} written out from the monorepo workspace"
            ));
        }
        table.insert("edition", value(edition));
        if let Some(rust_version) = self.inherited(package, "rust-version")? {
            table.insert("rust-version", rust_version.clone());
        }
        if let Some(authors) = self.inherited(package, "authors")? {
            table.insert("authors", authors.clone());
        }
        table.insert("description", value(self.description()?));
        let license = self.license(package)?;
        if package.get("license").is_none() {
            notes.push(format!(
                "[package]: license {license}, the repository's licence, since the package declares none"
            ));
        } else if Some(license.as_str()) != package.get("license").and_then(Item::as_str) {
            notes.push(format!(
                "[package]: license written as the SPDX expression {license}"
            ));
        }
        table.insert("license", value(license));
        table.insert("repository", value(RTAO));
        table.insert("homepage", value(self.homepage()));
        table.insert(
            "documentation",
            value(format!("https://docs.rs/{}", fork.package())),
        );
        if readme {
            table.insert("readme", value("README.md"));
        }
        for key in ["autobins", "autoexamples", "autotests", "autobenches"] {
            table.insert(key, value(false));
        }
        out.insert("package", Item::Table(table));

        let mut lib = Table::new();
        lib.insert("name", value(fork.library()));
        lib.insert("path", value("src/lib.rs"));
        if let Some(original_lib) = original.get("lib") {
            if let Some(name) = original_lib.get("name").and_then(Item::as_str) {
                ensure!(
                    name == fork.library(),
                    "{} names its library {name}",
                    fork.original
                );
            }
            for key in ["proc-macro", "crate-type"] {
                if let Some(item) = original_lib.get(key) {
                    lib.insert(key, item.clone());
                }
            }
            if let Some(path) = original_lib.get("path").and_then(Item::as_str) {
                ensure!(
                    path == "src/lib.rs",
                    "{} library path {path}",
                    fork.original
                );
            }
        }
        lib.insert("test", value(false));
        lib.insert("doctest", value(false));
        lib.insert("bench", value(false));
        out.insert("lib", Item::Table(lib));
        notes.push(
            "[lib]: library name kept as upstream's; library tests, benchmarks and doctests disabled, since upstream's test fixtures and dev-dependencies are not republished".into(),
        );
        for (kind, label) in [
            ("bin", "binaries"),
            ("test", "integration tests"),
            ("bench", "benchmarks"),
            ("example", "examples"),
        ] {
            if original.get(kind).is_some() {
                notes.push(format!("[[{kind}]]: {label} removed"));
            }
        }

        let dev = original
            .get("dev-dependencies")
            .and_then(Item::as_table_like)
            .map(|table| {
                table
                    .iter()
                    .map(|(name, _)| name.to_owned())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let mut normal = BTreeSet::new();
        for kind in ["dependencies", "build-dependencies"] {
            if let Some(table) = original.get(kind).and_then(Item::as_table_like) {
                normal.extend(table.iter().map(|(name, _)| name.to_owned()));
                out.insert(kind, Item::Table(self.dependencies(table, &mut notes)?));
            }
        }
        if let Some(targets) = original.get("target").and_then(Item::as_table_like) {
            let mut translated = Table::new();
            translated.set_implicit(true);
            for (cfg, target) in targets.iter() {
                let mut entry = Table::new();
                entry.set_implicit(true);
                for kind in ["dependencies", "build-dependencies"] {
                    if let Some(table) = target.get(kind).and_then(Item::as_table_like) {
                        normal.extend(table.iter().map(|(name, _)| name.to_owned()));
                        entry.insert(kind, Item::Table(self.dependencies(table, &mut notes)?));
                    }
                }
                ensure!(
                    target.get("dev-dependencies").is_none(),
                    "target dev-dependencies are not supported"
                );
                if !entry.is_empty() {
                    translated.insert(cfg, Item::Table(entry));
                }
            }
            out.insert("target", Item::Table(translated));
        }
        if !dev.is_empty() {
            notes.push(format!(
                "[dev-dependencies]: removed ({})",
                dev.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let dev_only: BTreeSet<_> = dev.difference(&normal).cloned().collect();

        if let Some(features) = original.get("features").and_then(Item::as_table_like) {
            let mut table = Table::new();
            for (name, members) in features.iter() {
                let members = members.as_array().context("feature must be an array")?;
                let mut kept = Array::new();
                for member in members {
                    let member = member.as_str().context("feature member must be a string")?;
                    let dependency = member
                        .strip_prefix("dep:")
                        .unwrap_or(member)
                        .split(['/', '?'])
                        .next()
                        .unwrap_or_default();
                    if member.contains('/') && dev_only.contains(dependency) {
                        notes.push(format!(
                            "[features]: {name} no longer enables {member}, a feature of a removed dev-dependency"
                        ));
                        continue;
                    }
                    kept.push(member);
                }
                table.insert(name, value(kept));
            }
            out.insert("features", Item::Table(table));
        }
        // rtao's gates deny warnings; upstream's code keeps its own (D-183).
        let mut rust = Table::new();
        for lint in ["deprecated", "unused", "unexpected_cfgs"] {
            rust.insert(lint, value("allow"));
        }
        let mut clippy = Table::new();
        let mut all = InlineTable::new();
        all.insert("level", "allow".into());
        all.insert("priority", (-1).into());
        clippy.insert("all", Item::Value(Value::InlineTable(all.clone())));
        let mut rustdoc = Table::new();
        rustdoc.insert("all", Item::Value(Value::InlineTable(all.clone())));
        let mut lints = Table::new();
        lints.set_implicit(true);
        lints.insert("rust", Item::Table(rust));
        lints.insert("clippy", Item::Table(clippy));
        lints.insert("rustdoc", Item::Table(rustdoc));
        out.insert("lints", Item::Table(lints));
        notes.push(if original.get("lints").is_some() {
            "[lints]: upstream's workspace lints replaced by allowing upstream's existing warnings (deprecated, unused, unexpected_cfgs, Clippy's default set, rustdoc's lints), so rtao's warning-denying gates check rtao's own code".into()
        } else {
            "[lints]: added, allowing upstream's existing warnings (deprecated, unused, unexpected_cfgs, Clippy's default set, rustdoc's lints), so rtao's warning-denying gates check rtao's own code".into()
        });
        if original.get("workspace").is_some() {
            notes.push("[workspace]: removed".into());
        }
        for key in original.iter().map(|(key, _)| key) {
            ensure!(
                [
                    "package",
                    "lib",
                    "bin",
                    "test",
                    "bench",
                    "example",
                    "dependencies",
                    "build-dependencies",
                    "dev-dependencies",
                    "target",
                    "features",
                    "lints",
                    "workspace",
                ]
                .contains(&key),
                "{} manifest has an unsupported table [{key}]",
                fork.original
            );
        }
        for key in package
            .as_table_like()
            .context("[package] missing")?
            .iter()
            .map(|(key, _)| key)
        {
            ensure!(
                [
                    "name",
                    "version",
                    "edition",
                    "rust-version",
                    "authors",
                    "description",
                    "license",
                    "repository",
                    "homepage",
                    "documentation",
                    "publish",
                    "metadata",
                ]
                .contains(&key),
                "{} [package] has an unsupported key {key}",
                fork.original
            );
        }
        if package.get("metadata").is_some() {
            notes.push("[package.metadata]: removed".into());
        }
        // A leading comment can carry upstream's own notice (the vendored
        // w3f-bls says what it changed from w3f/bls), so it is kept.
        let leading: Vec<&str> = original_text
            .lines()
            .take_while(|line| line.starts_with('#'))
            .collect();
        let comments = original_text
            .lines()
            .filter(|line| line.trim_start().starts_with('#') || line.contains(" #"))
            .count();
        if !leading.is_empty() {
            notes.push(
                "comments: the original manifest's leading comment is kept, under the generated header".into(),
            );
        }
        if comments > leading.len() {
            notes.push(if leading.is_empty() {
                "comments: the original manifest's comments are not carried over".into()
            } else {
                "comments: the original manifest's other comments are not carried over".into()
            });
        }
        let mut header = format!(
            "# Generated by `cargo xtask forks` from {}/Cargo.toml at\n# {} {}.\n# Do not edit: see MODIFICATIONS for every difference from that file.\n",
            fork.path, self.source.repo, self.source.rev
        );
        if !leading.is_empty() {
            header.push_str("#\n# The original manifest begins with this comment:\n#\n");
            for line in &leading {
                header.push_str(line);
                header.push('\n');
            }
        }
        header.push('\n');
        notes.dedup();
        Ok((format!("{header}{out}"), notes))
    }

    fn dependencies(
        &self,
        table: &dyn toml_edit::TableLike,
        notes: &mut Vec<String>,
    ) -> Result<Table> {
        let mut out = Table::new();
        for (name, dependency) in table.iter() {
            out.insert(
                name,
                Item::Value(Value::InlineTable(
                    self.dependency(name, dependency, notes)?,
                )),
            );
        }
        Ok(out)
    }

    /// Resolve workspace inheritance and translate the source of one dependency.
    fn dependency(&self, name: &str, local: &Item, notes: &mut Vec<String>) -> Result<InlineTable> {
        let local = normalize(local)?;
        let inherited = local.get("workspace").and_then(Value::as_bool) == Some(true);
        let (resolved, base) = if inherited {
            let workspace = self
                .workspace
                .context("workspace-inherited dependency outside the monorepo")?;
            let entry = workspace["workspace"]["dependencies"]
                .get(name)
                .with_context(|| format!("workspace.dependencies.{name} missing"))?;
            let mut resolved = normalize(entry)?;
            // Cargo's merge: features add up, `optional` is the member's and
            // `default-features = false` in the workspace holds unless the
            // member asks for them.
            let mut features: Vec<String> = string_array(resolved.get("features"))?;
            for feature in string_array(local.get("features"))? {
                if !features.contains(&feature) {
                    features.push(feature);
                }
            }
            if !features.is_empty() {
                resolved.insert("features", Value::Array(features.iter().collect()));
            }
            for key in ["optional", "default-features"] {
                if let Some(item) = local.get(key) {
                    resolved.insert(key, item.clone());
                }
            }
            for key in local.iter().map(|(key, _)| key) {
                ensure!(
                    ["workspace", "features", "optional", "default-features"].contains(&key),
                    "unsupported key {key} on inherited dependency {name}"
                );
            }
            (resolved, "")
        } else {
            (local, self.fork.path)
        };
        let package = resolved
            .get("package")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_owned();
        let target = if let Some(path) = resolved.get("path").and_then(Value::as_str) {
            let path = join(base, path)?;
            let fork = FORKS
                .iter()
                .find(|fork| fork.origin != Origin::Timelock && fork.path == path)
                .with_context(|| {
                    format!("{name} is a path dependency on {path}, which is not republished")
                })?;
            Target::Fork(fork)
        } else if let Some(repo) = resolved.get("git").and_then(Value::as_str) {
            if repo == POLKADOT_SDK {
                ensure!(
                    resolved.get("rev").and_then(Value::as_str) == Some(self.sdk_rev),
                    "{name} uses polkadot-sdk at another revision than the reviewed {}; review the crates.io releases for the new revision and update [publish] polkadot_sdk",
                    self.sdk_rev
                );
                let version = self.sdk.get(&package).with_context(|| {
                    format!("{package} has no reviewed crates.io release in [publish] polkadot_sdk")
                })?;
                notes.push(format!(
                    "[dependencies]: {package} from crates.io {version} instead of {POLKADOT_SDK} {}",
                    self.sdk_rev
                ));
                Target::Registry(version.clone())
            } else if repo == TIMELOCK {
                Target::Fork(fork("tle")?)
            } else {
                bail!("{name} comes from an unreviewed Git repository {repo}")
            }
        } else if let Some(fork) = self.patched(&package)? {
            Target::Fork(fork)
        } else {
            Target::Registry(
                resolved
                    .get("version")
                    .and_then(Value::as_str)
                    .with_context(|| format!("{name} has no version"))?
                    .to_owned(),
            )
        };
        let mut out = InlineTable::new();
        match target {
            Target::Fork(fork) => {
                ensure!(
                    package == fork.original,
                    "{name} renames {package}, expected {}",
                    fork.original
                );
                out.insert("package", fork.package().as_str().into());
                out.insert(
                    "version",
                    format!("={}", version(self.pins, fork)?).as_str().into(),
                );
                out.insert("path", format!("../{}", fork.key).as_str().into());
                notes.push(format!(
                    "[dependencies]: {name} is {} (the republished {}), by exact version; the dependency name is unchanged",
                    fork.package(),
                    fork.original
                ));
            }
            Target::Registry(version) => {
                if package != name {
                    out.insert("package", package.as_str().into());
                }
                out.insert("version", version.as_str().into());
            }
        }
        for key in ["default-features", "optional"] {
            if let Some(item) = resolved.get(key) {
                out.insert(
                    key,
                    item.as_bool()
                        .with_context(|| format!("{name}.{key} must be a boolean"))?
                        .into(),
                );
            }
        }
        let features = string_array(resolved.get("features"))?;
        if !features.is_empty() {
            out.insert("features", Value::Array(features.iter().collect()));
        }
        out.fmt();
        for key in resolved.iter().map(|(key, _)| key) {
            ensure!(
                [
                    "version",
                    "path",
                    "git",
                    "rev",
                    "tag",
                    "branch",
                    "package",
                    "default-features",
                    "features",
                    "optional",
                    "workspace",
                ]
                .contains(&key),
                "unsupported key {key} on dependency {name}"
            );
        }
        if inherited {
            notes.push(
                "[dependencies]: dependencies inherited from the monorepo workspace written out in full".into(),
            );
        }
        Ok(out)
    }

    /// A crates.io dependency the monorepo replaces with a vendored package
    /// through its root `[patch.crates-io]` (w3f-bls for tle).
    fn patched(&self, package: &str) -> Result<Option<&'static Fork>> {
        let workspace = match self.workspace {
            Some(workspace) => workspace,
            None => return self.monorepo_patch(package),
        };
        let Some(entry) = workspace
            .get("patch")
            .and_then(|patch| patch.get("crates-io"))
            .and_then(|patch| patch.get(package))
        else {
            return Ok(None);
        };
        let path = entry
            .get("path")
            .and_then(Item::as_str)
            .with_context(|| format!("monorepo patch of {package} is not a path"))?;
        FORKS
            .iter()
            .find(|fork| fork.path == path)
            .map(Some)
            .with_context(|| {
                format!("monorepo patches {package} with {path}, which is not republished")
            })
    }

    /// tle is built inside the monorepo's workspace, whose `[patch.crates-io]`
    /// replaces its crates.io w3f-bls with the vendored fork.
    fn monorepo_patch(&self, package: &str) -> Result<Option<&'static Fork>> {
        if package == "w3f-bls" {
            return fork("w3f-bls").map(Some);
        }
        Ok(None)
    }

    fn notice(&self, repository_licence: bool, declared: &str) -> Result<String> {
        let fork = self.fork;
        let licence = if !repository_licence {
            "The licence files are the package's own.".to_owned()
        } else if declared.contains("Apache-2.0") {
            "LICENSE is the source repository's root LICENSE, which covers the package.".to_owned()
        } else {
            format!(
                "The package declares the {declared} licence; LICENSE is the source repository's root LICENSE, included as it is published."
            )
        };
        Ok(format!(
            "{package}\n\n\
             This package republishes {original} from {project} so that rtao\n\
             ({RTAO}) can be published on crates.io, which does not accept Git\n\
             dependencies. It is not affiliated with or endorsed by the authors of\n\
             {original}. Report problems with this package to rtao.\n\n\
             Source: {repo}\n\
             Revision: {rev}\n\
             Path: {path}\n\n\
             Every file except Cargo.toml, NOTICE and MODIFICATIONS is the file at\n\
             that path and revision, byte for byte. {licence}\n\
             MODIFICATIONS lists how Cargo.toml differs from the original manifest.\n",
            package = fork.package(),
            original = fork.original,
            project = self.origin()?,
            repo = self.source.repo,
            rev = self.source.rev,
            path = fork.path,
        ))
    }

    fn modifications(
        &self,
        notes: &[String],
        patched: &[String],
        omitted: &BTreeSet<String>,
        base: &str,
        licence_added: bool,
    ) -> Result<String> {
        let mut unique = Vec::new();
        for note in notes {
            if !unique.contains(note) {
                unique.push(note.clone());
            }
        }
        let mut text = format!(
            "Modified files\n\n\
             As required by section 4(b) of the Apache License 2.0: Cargo.toml is a\n\
             modified version of {path}/Cargo.toml at {repo} {rev},\n\
             changed by rtao (cargo xtask forks) as follows.\n\n",
            path = self.fork.path,
            repo = self.source.repo,
            rev = self.source.rev,
        );
        for note in unique {
            text.push_str("- ");
            text.push_str(&note);
            text.push('\n');
        }
        if !omitted.is_empty() {
            text.push_str(&format!(
                "\nThe package leaves out these directories of {path}, which only upstream's\n\
                 own tests, benchmarks and examples use: {list}.\n",
                path = self.fork.path,
                list = omitted.iter().cloned().collect::<Vec<_>>().join(", "),
            ));
        }
        if !patched.is_empty() {
            let branch = self.pins["core"]
                .get("patch")
                .and_then(|patch| patch.get("branch"))
                .and_then(Item::as_str)
                .context("upstream.toml [core.patch] has no branch")?;
            text.push_str(&format!(
                "\nThat revision is rtao's reviewed core patch (branch {branch}) on\n\
                 {UPSTREAM} {base}.\n\
                 It modifies these files of {path} at that commit:\n\n",
                path = self.fork.path,
            ));
            for file in patched {
                text.push_str("- ");
                text.push_str(file);
                text.push('\n');
            }
        }
        text.push_str(if licence_added {
            "\nNo other file is modified. LICENSE (the source repository's root LICENSE),\nNOTICE and this file are added by rtao.\n"
        } else {
            "\nNo other file is modified. NOTICE and this file are added by rtao.\n"
        });
        Ok(text)
    }
}

fn normalize(item: &Item) -> Result<InlineTable> {
    Ok(match item {
        Item::Value(Value::String(version)) => {
            let mut table = InlineTable::new();
            table.insert("version", version.value().as_str().into());
            table
        }
        Item::Value(Value::InlineTable(table)) => table.clone(),
        Item::Table(table) => table.clone().into_inline_table(),
        _ => bail!("unsupported dependency form"),
    })
}

fn string_array(item: Option<&Value>) -> Result<Vec<String>> {
    let Some(item) = item else {
        return Ok(Vec::new());
    };
    item.as_array()
        .context("features must be an array")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("feature must be a string")
        })
        .collect()
}

/// `base/path`, normalised, within the repository.
fn join(base: &str, path: &str) -> Result<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts
                    .pop()
                    .context("path dependency leaves the repository")?;
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

/// The files currently under `upstream/<key>`, relative to it.
fn existing(root: &Path, fork: &Fork) -> Result<BTreeMap<String, Vec<u8>>> {
    let directory = root.join(DIRECTORY).join(fork.key);
    let mut found = BTreeMap::new();
    if !directory.exists() {
        return Ok(found);
    }
    let mut pending = vec![directory.clone()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(
                    kind.is_file(),
                    "{} is not a regular file",
                    entry.path().display()
                );
                let relative = entry
                    .path()
                    .strip_prefix(&directory)?
                    .to_string_lossy()
                    .replace('\\', "/");
                found.insert(relative, fs::read(entry.path())?);
            }
        }
    }
    Ok(found)
}

/// Compare the committed packages with freshly generated ones.
pub(crate) fn check(root: &Path, pins: &DocumentMut) -> Result<()> {
    verify_workspace(root, pins)?;
    let packages = generate(root, pins)?;
    let mut keys = BTreeSet::new();
    for package in &packages {
        keys.insert(package.fork.key.to_owned());
        let actual = existing(root, &package.fork)?;
        ensure!(
            actual == package.files,
            "{}/{} differs from the package generated from {} {}; run cargo xtask forks write (differing: {})",
            DIRECTORY,
            package.fork.key,
            package.source.repo,
            package.source.rev,
            differing(&actual, &package.files).join(", ")
        );
        if package.fork.key == "core" {
            verify_core_sources(package)?;
        }
    }
    ensure!(
        fs::read_to_string(root.join(DIRECTORY).join("rustfmt.toml"))
            .ok()
            .as_deref()
            == Some(RUSTFMT),
        "{DIRECTORY}/rustfmt.toml differs; run cargo xtask forks write"
    );
    keys.insert("rustfmt.toml".into());
    let present: BTreeSet<_> = fs::read_dir(root.join(DIRECTORY))?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    ensure!(
        present == keys,
        "{DIRECTORY}/ must hold exactly the republished packages {keys:?}; found {present:?}"
    );
    Ok(())
}

fn differing(left: &BTreeMap<String, Vec<u8>>, right: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    left.keys()
        .chain(right.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| left.get(*path) != right.get(*path))
        .map(|path| path.to_owned())
        .collect()
}

/// The Ed25519 erasure check (D-139) on the republished core's sources.
fn verify_core_sources(package: &Package) -> Result<()> {
    let sources = package
        .files
        .iter()
        .filter(|(path, _)| path.starts_with("src/") && path.ends_with(".rs"))
        .map(|(path, bytes)| {
            Ok((
                format!("{}/{path}", package.fork.path),
                String::from_utf8(bytes.clone()).with_context(|| format!("{path} is not UTF-8"))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    crate::core_patch::verify_security_source_text(&package.original_manifest, &sources)
}

/// `upstream/rustfmt.toml`: `cargo fmt --all` leaves the republished
/// sources as upstream formatted them.
const RUSTFMT: &str = "# Generated by `cargo xtask forks`: the republished packages keep upstream's\n# formatting byte for byte (D-183).\ndisable_all_formatting = true\n";

/// Replace `upstream/` with freshly generated packages.
pub(crate) fn write(root: &Path, pins: &DocumentMut) -> Result<()> {
    let packages = generate(root, pins)?;
    let directory = root.join(DIRECTORY);
    if directory.exists() {
        fs::remove_dir_all(&directory)?;
    }
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("rustfmt.toml"), RUSTFMT)?;
    for package in packages {
        for (relative, bytes) in &package.files {
            let path = directory.join(package.fork.key).join(relative);
            fs::create_dir_all(path.parent().context("file has no parent")?)?;
            fs::write(path, bytes)?;
        }
    }
    Ok(())
}

/// Record each package's source for the current core pin, keeping a version
/// and checksum that still describe the same source and resetting them when
/// the core moved.
pub(crate) fn refresh(root: &Path, pins: &mut DocumentMut) -> Result<bool> {
    let repo = cached_repo(root)?;
    let mut changed = false;
    if pins.get("publish").is_none() {
        let mut table = Table::new();
        table.set_implicit(false);
        pins["publish"] = Item::Table(table);
    }
    for fork in FORKS {
        let source = derived_source(&repo, pins, &fork)?;
        let previous = recorded_source(pins, &fork).ok();
        let previous_version = entry(pins, &fork)
            .ok()
            .and_then(|entry| entry.get("version"))
            .and_then(Item::as_str)
            .map(str::to_owned);
        let version = next_version(
            previous.as_ref().zip(previous_version.as_deref()),
            &source,
            release(pins)?,
        );
        if previous.as_ref() == Some(&source)
            && previous_version.as_deref() == Some(version.as_str())
        {
            continue;
        }
        let mut table = Table::new();
        table.insert("package", value(fork.package()));
        table.insert("version", value(version));
        table.insert("repo", value(&source.repo));
        table.insert("rev", value(&source.rev));
        table.insert("path", value(fork.path));
        pins["publish"][fork.key] = Item::Table(table);
        changed = true;
    }
    Ok(changed)
}

/// The version a package gets for its source under the core release: the
/// recorded version while the source and the release are unchanged, the next
/// revision for a new source of the same release, and `0.<N>.0` otherwise. A
/// package whose source survives a core bump (tle, whose revision the
/// monorepo rarely moves) still moves to the new release's minor, which
/// [`version`] requires of every package.
fn next_version(previous: Option<(&Source, &str)>, source: &Source, release: u64) -> String {
    match previous.and_then(|(previous, version)| {
        semver::Version::parse(version)
            .ok()
            .filter(|version| version.major == 0 && version.minor == release)
            .map(|version| (previous, version))
    }) {
        Some((previous, version)) if previous == source => version.to_string(),
        Some((_, mut version)) => {
            // Same core release, new reviewed source: the next revision.
            version.patch += 1;
            version.pre = semver::Prerelease::EMPTY;
            version.build = semver::BuildMetadata::EMPTY;
            version.to_string()
        }
        None => format!("0.{release}.0"),
    }
}

/// Replace the republished packages with their original Git sources at the
/// core tag, for disposable checks against upstream itself (the unpatched core
/// check and the core canary). The monorepo's BLS fork then joins tle's
/// crates.io request through the `[patch.crates-io]` a Git build needs.
pub(crate) fn as_git(manifest: &mut DocumentMut, pins: &DocumentMut) -> Result<()> {
    let tag = field(&pins["core"], "tag")?;
    let tle = recorded_source(pins, fork("tle")?)?;
    let members = manifest["workspace"]["members"]
        .as_array_mut()
        .context("workspace members missing")?;
    members.retain(|member| member.as_str() != Some("upstream/*"));
    if manifest["workspace"].get("exclude").is_none() {
        manifest["workspace"]["exclude"] = value(Array::new());
    }
    manifest["workspace"]["exclude"]
        .as_array_mut()
        .context("workspace exclude must be an array")?
        .push(DIRECTORY);
    let git = |repo: &str, selector: &str, reference: &str, features: &[&str]| {
        let mut table = InlineTable::new();
        table.insert("git", repo.into());
        table.insert(selector, reference.into());
        table.insert("default-features", false.into());
        if !features.is_empty() {
            table.insert("features", Value::Array(features.iter().copied().collect()));
        }
        Item::Value(Value::InlineTable(table))
    };
    let dependencies = &mut manifest["workspace"]["dependencies"];
    dependencies["bittensor-core"] = git(UPSTREAM, "tag", tag, &[]);
    dependencies["w3f-bls"] = git(UPSTREAM, "tag", tag, &["std"]);
    dependencies["subtensor-macros"] = git(UPSTREAM, "tag", tag, &[]);
    dependencies["tle"] = git(TIMELOCK, "rev", &tle.rev, &[]);
    let mut bls = InlineTable::new();
    bls.insert("git", UPSTREAM.into());
    bls.insert("tag", tag.into());
    manifest["patch"]["crates-io"]["w3f-bls"] = Item::Value(Value::InlineTable(bls));
    if let Some(patch) = manifest["patch"].as_table_mut() {
        patch.set_implicit(true);
    }
    Ok(())
}

/// Point the root manifest's dependencies at the recorded versions.
pub(crate) fn write_workspace(root: &Path, pins: &DocumentMut) -> Result<()> {
    let path = root.join("Cargo.toml");
    let mut manifest = read_document(&path)?;
    for (name, key) in WORKSPACE_DEPENDENCIES {
        let requirement = format!("={}", version(pins, fork(key)?)?);
        let dependency = manifest["workspace"]["dependencies"][name]
            .as_table_like_mut()
            .with_context(|| format!("workspace dependency {name} missing"))?;
        dependency.insert("version", value(requirement));
    }
    crate::pin::atomic_write(&path, manifest.to_string().as_bytes())?;
    verify_workspace(root, pins)
}

/// The root manifest's dependencies on the republished packages.
pub(crate) fn verify_workspace(root: &Path, pins: &DocumentMut) -> Result<()> {
    let manifest = read_document(&root.join("Cargo.toml"))?;
    let members = manifest["workspace"]["members"]
        .as_array()
        .context("workspace members missing")?;
    ensure!(
        members
            .iter()
            .any(|member| member.as_str() == Some("upstream/*")),
        "the workspace must include upstream/*, so CI builds and lints what is published"
    );
    for (name, key) in WORKSPACE_DEPENDENCIES {
        let fork = fork(key)?;
        let dependency = &manifest["workspace"]["dependencies"][name];
        let expected = format!("={}", version(pins, fork)?);
        ensure!(
            dependency.get("package").and_then(Item::as_str) == Some(fork.package().as_str())
                && dependency.get("path").and_then(Item::as_str)
                    == Some(format!("{DIRECTORY}/{key}").as_str())
                && dependency.get("version").and_then(Item::as_str) == Some(expected.as_str())
                && dependency.get("default-features").and_then(Item::as_bool) == Some(false)
                && dependency.get("git").is_none(),
            "workspace dependency {name} must be {{ package = {:?}, path = \"{DIRECTORY}/{key}\", version = {expected:?}, default-features = false }}",
            fork.package()
        );
    }
    if let Some(patch) = manifest.get("patch").and_then(Item::as_table_like)
        && let Some((source, _)) = patch.iter().next()
    {
        bail!("the root manifest patches {source}; the published graph must build without [patch]");
    }
    Ok(())
}

/// Download the published `.crate` of every package and compare it with the
/// generated one (see [`verify_archive`]); its SHA256 must equal the
/// recorded checksum.
pub(crate) fn verify_published(root: &Path, pins: &DocumentMut, scratch: &Path) -> Result<()> {
    let packages = generate(root, pins)?;
    fs::create_dir_all(scratch)?;
    for package in &packages {
        let fork = &package.fork;
        let name = fork.package();
        let version = version(pins, fork)?;
        let recorded = checksum(pins, fork)?.with_context(|| {
            format!(
                "[publish.{}] has no checksum; record it after publishing",
                fork.key
            )
        })?;
        let directory = scratch.join(format!("{name}-{version}.download"));
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        let archive = directory.join("package.crate");
        let url = format!("https://static.crates.io/crates/{name}/{name}-{version}.crate");
        let output = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--max-time",
                "120",
                "--output",
            ])
            .arg(&archive)
            .arg(&url)
            .output()?;
        ensure!(
            output.status.success(),
            "download {url}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual = verify_archive(root, package, version, &archive, &directory)?;
        ensure!(
            actual == recorded,
            "{name} {version}: published SHA256 {actual} differs from the recorded {recorded}"
        );
        fs::remove_dir_all(&directory)?;
        println!("{name} {version}: published archive verified ({actual})");
    }
    Ok(())
}

/// Before publishing: compare the `.crate` files `cargo package` wrote under
/// `target/package` with the generated packages, and print each SHA256.
pub(crate) fn verify_packaged(root: &Path, pins: &DocumentMut, scratch: &Path) -> Result<()> {
    let packages = generate(root, pins)?;
    fs::create_dir_all(scratch)?;
    for package in &packages {
        let name = package.fork.package();
        let version = version(pins, &package.fork)?;
        let archive = root
            .join("target/package")
            .join(format!("{name}-{version}.crate"));
        ensure!(
            archive.is_file(),
            "{} is missing; run cargo package first",
            archive.display()
        );
        let directory = scratch.join(format!("{name}-{version}.packaged"));
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        let actual = verify_archive(root, package, version, &archive, &directory)?;
        fs::remove_dir_all(&directory)?;
        println!("{name} {version}: packaged archive verified, sha256 {actual}");
    }
    Ok(())
}

/// Extract one `.crate` and compare it with the generated package: every
/// copied file equals the upstream file at the revision, `Cargo.toml.orig`
/// equals the generated manifest, nothing else is added but Cargo's own
/// `Cargo.toml`, `Cargo.lock` and `.cargo_vcs_info.json`, and the core passes
/// the Ed25519 erasure check. Returns the archive's SHA256.
fn verify_archive(
    root: &Path,
    package: &Package,
    version: &str,
    archive: &Path,
    directory: &Path,
) -> Result<String> {
    let name = package.fork.package();
    let bytes = fs::read(archive)?;
    let actual = hex::encode(Sha256::digest(&bytes));
    let extracted = directory.join("extracted");
    fs::create_dir_all(&extracted)?;
    let output = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(&extracted)
        .arg("--no-same-owner")
        .arg("--no-same-permissions")
        .output()?;
    ensure!(
        output.status.success(),
        "extract {name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prefix = format!("{name}-{version}");
    ensure!(
        fs::read_dir(&extracted)?.count() == 1,
        "{name}: the archive holds more than {prefix}/"
    );
    let mut published = existing_files(&extracted.join(&prefix))?;
    // Cargo writes these itself.
    published.remove(".cargo_vcs_info.json");
    published.remove("Cargo.lock");
    published
        .remove("Cargo.toml")
        .with_context(|| format!("{name}: no normalised Cargo.toml"))?;
    let original = published
        .remove("Cargo.toml.orig")
        .with_context(|| format!("{name}: no Cargo.toml.orig"))?;
    ensure!(
        Some(&original) == package.files.get("Cargo.toml"),
        "{name}: the archived manifest differs from the generated one"
    );
    let expected: BTreeMap<_, _> = package
        .files
        .iter()
        .filter(|(path, _)| path.as_str() != "Cargo.toml")
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();
    ensure!(
        published == expected,
        "{name}: archived files differ from {} {} (differing: {})",
        package.source.repo,
        package.source.rev,
        differing(&published, &expected).join(", ")
    );
    // Read each copied file from the revision again, independently of the
    // generator's file map.
    let store = store(root, &package.source.repo)?;
    for (relative, upstream) in &package.copied {
        ensure!(
            published.get(relative) == Some(&show(&store, &package.source.rev, upstream)?),
            "{name}: {relative} differs from {upstream} at {}",
            package.source.rev
        );
    }
    if package.fork.key == "core" {
        verify_core_sources(&Package {
            fork: package.fork,
            source: package.source.clone(),
            files: published,
            copied: BTreeMap::new(),
            original_manifest: package.original_manifest.clone(),
        })?;
    }
    Ok(actual)
}

fn existing_files(directory: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(
                    kind.is_file(),
                    "{} is not a regular file",
                    entry.path().display()
                );
                found.insert(
                    entry
                        .path()
                        .strip_prefix(directory)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read(entry.path())?,
                );
            }
        }
    }
    Ok(found)
}

/// Lock entries for the republished packages: each exactly once, at its
/// recorded version, from the workspace path or crates.io with the recorded
/// checksum; and no package of the original names from Git.
pub(crate) fn verify_lock(lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lock packages missing")?;
    for fork in FORKS {
        let name = fork.package();
        let matching: Vec<_> = packages
            .iter()
            .filter(|package| package.get("name").and_then(Item::as_str) == Some(name.as_str()))
            .collect();
        ensure!(
            matching.len() == 1,
            "Cargo.lock must contain exactly one {name}; found {}",
            matching.len()
        );
        let package = matching[0];
        ensure!(
            package.get("version").and_then(Item::as_str) == Some(version(pins, &fork)?),
            "Cargo.lock {name} version differs from [publish.{}]",
            fork.key
        );
        match package.get("source").and_then(Item::as_str) {
            None => {}
            Some("registry+https://github.com/rust-lang/crates.io-index") => {
                let recorded = checksum(pins, &fork)?.with_context(|| {
                    format!("{name} resolves from crates.io without a recorded checksum")
                })?;
                ensure!(
                    package.get("checksum").and_then(Item::as_str) == Some(recorded),
                    "Cargo.lock {name} checksum differs from [publish.{}]",
                    fork.key
                );
            }
            Some(source) => bail!("Cargo.lock {name} comes from an unexpected source {source}"),
        }
    }
    for package in packages {
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .unwrap_or_default();
        let source = package
            .get("source")
            .and_then(Item::as_str)
            .unwrap_or_default();
        ensure!(
            !source.contains(FORK)
                && !source.contains(TIMELOCK)
                && (!source.contains(POLKADOT_SDK) || XTASK_ONLY.contains(&name)),
            "Cargo.lock contains {name} from {source}; the published graph has no Git dependency"
        );
        ensure!(
            !(FORKS.iter().any(|fork| fork.original == name) && source.starts_with("git+")),
            "Cargo.lock contains {name} from Git; rtao depends on its republished package"
        );
    }
    Ok(())
}

pub(crate) fn run(arguments: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root missing")?;
    let pins_path = root.join("upstream.toml");
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check"] | ["--check"] => {
            check(root, &read_document(&pins_path)?)?;
            println!("{DIRECTORY}/ equals the packages generated from the recorded revisions");
            Ok(())
        }
        ["write"] => {
            write(root, &read_document(&pins_path)?)?;
            println!("{DIRECTORY}/ regenerated");
            Ok(())
        }
        ["refresh"] => {
            let mut pins = read_document(&pins_path)?;
            if refresh(root, &mut pins)? {
                crate::pin::write_pins(root, &pins)?;
            }
            write(root, &pins)?;
            println!("[publish] and {DIRECTORY}/ follow the core pin");
            Ok(())
        }
        ["verify-published"] => {
            let scratch = PathBuf::from(
                std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?,
            )
            .join("forks-published");
            verify_published(root, &read_document(&pins_path)?, &scratch)?;
            if scratch.exists() {
                fs::remove_dir_all(&scratch)?;
            }
            Ok(())
        }
        ["verify-packaged"] => {
            let scratch = PathBuf::from(
                std::env::var_os("RTAO_TEMP_ROOT").context("RTAO_TEMP_ROOT is required")?,
            )
            .join("forks-packaged");
            verify_packaged(root, &read_document(&pins_path)?, &scratch)?;
            if scratch.exists() {
                fs::remove_dir_all(&scratch)?;
            }
            Ok(())
        }
        ["order"] => {
            for fork in FORKS {
                println!("{}", fork.package());
            }
            Ok(())
        }
        _ => bail!(
            "usage: cargo xtask forks check | write | refresh | verify-packaged | verify-published | order"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_join_within_the_repository() -> Result<()> {
        assert_eq!(
            join("sdk/bittensor-core", "../../primitives/hashed")?,
            "primitives/hashed"
        );
        assert_eq!(join("", "vendor/w3f-bls")?, "vendor/w3f-bls");
        assert!(join("sdk", "../../x").is_err());
        Ok(())
    }

    #[test]
    fn package_files_omit_tests_and_manifests() {
        assert!(included("src/lib.rs"));
        assert!(included("README.md"));
        assert!(included("LICENSE-MIT"));
        assert!(!included("Cargo.toml"));
        assert!(!included("tests/fixtures/x.json"));
        assert!(!included("benches/tlock.rs"));
        assert!(!included("fixtures/ledger_proof_vector.json"));
    }

    #[test]
    fn publication_order_puts_dependencies_first() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let mut seen = BTreeSet::new();
        for fork in FORKS {
            let manifest = read_document(&root.join(DIRECTORY).join(fork.key).join("Cargo.toml"))?;
            for kind in ["dependencies", "build-dependencies"] {
                for (_, dependency) in manifest
                    .get(kind)
                    .and_then(Item::as_table_like)
                    .map(|table| table.iter().collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    if let Some(package) = dependency.get("package").and_then(Item::as_str)
                        && package.starts_with("rtao-upstream-")
                    {
                        ensure!(
                            seen.contains(package),
                            "{} depends on {package} before it",
                            fork.key
                        );
                    }
                }
            }
            seen.insert(fork.package());
        }
        Ok(())
    }

    #[test]
    fn versions_follow_the_source_and_the_core_release() {
        let source = |rev: &str| Source {
            repo: TIMELOCK.into(),
            rev: rev.into(),
        };
        let (old, new) = (source(&"1".repeat(40)), source(&"2".repeat(40)));
        // Unchanged source and release: the recorded version stays.
        assert_eq!(next_version(Some((&old, "0.490.0")), &old, 490), "0.490.0");
        assert_eq!(next_version(Some((&old, "0.490.2")), &old, 490), "0.490.2");
        // A new adaptation revision of the same release: the next revision.
        assert_eq!(next_version(Some((&old, "0.490.0")), &new, 490), "0.490.1");
        // A core bump moves every package to the new release, including one
        // whose source did not change (tle at the same timelock revision).
        assert_eq!(next_version(Some((&old, "0.490.3")), &old, 491), "0.491.0");
        assert_eq!(next_version(Some((&old, "0.490.3")), &new, 491), "0.491.0");
        assert_eq!(next_version(None, &new, 491), "0.491.0");
        assert_eq!(
            next_version(Some((&old, "not semver")), &old, 491),
            "0.491.0"
        );
    }

    #[test]
    fn committed_packages_match_the_record() -> Result<()> {
        // Offline: the manifests carry the recorded versions and the root
        // manifest depends on them exactly. `forks check` compares the
        // sources with the revisions, which needs the upstream cache.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let pins = read_document(&root.join("upstream.toml"))?;
        verify_workspace(root, &pins)?;
        for fork in FORKS {
            let manifest = read_document(&root.join(DIRECTORY).join(fork.key).join("Cargo.toml"))?;
            ensure!(manifest["package"]["name"].as_str() == Some(fork.package().as_str()));
            ensure!(manifest["package"]["version"].as_str() == Some(version(&pins, &fork)?));
            let source = recorded_source(&pins, &fork)?;
            let notice = fs::read_to_string(root.join(DIRECTORY).join(fork.key).join("NOTICE"))?;
            ensure!(notice.contains(&source.rev) && notice.contains(&source.repo));
        }
        verify_lock(&read_document(&root.join("Cargo.lock"))?, &pins)
    }
}
