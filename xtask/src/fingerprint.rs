//! Git-object-only core input fingerprinting (architecture §1.2).

use anyhow::{Context, Result, bail, ensure};
use semver::{Version, VersionReq};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use toml_edit::{DocumentMut, Item};

const CORE: &str = "sdk/bittensor-core";

fn input_bytes(mut document: Value) -> Result<Vec<u8>> {
    // The v1 fingerprint uses recursively sorted JSON object keys. Workspace
    // feature unification must not change source identity by enabling
    // serde_json/preserve_order in an unrelated consumer.
    document.sort_all_objects();
    Ok(serde_json::to_vec(&document)?)
}

/// Hash the exact source/manifests and normalized inherited inputs at a revision.
pub(crate) fn fingerprint(repo: &Path, revision: &str) -> Result<String> {
    let mut inputs = Inputs::new(repo, revision)?;
    inputs.resolve(false)?;
    let mut packages = BTreeMap::new();
    for path in inputs.paths.keys() {
        packages.insert(
            path,
            json!({
                "src_tree": inputs.git.object(&format!("{path}/src"), "tree", true)?,
                "manifest_blob": inputs.git.object(&format!("{path}/Cargo.toml"), "blob", true)?,
                "build_blob": inputs.git.object(&format!("{path}/build.rs"), "blob", false)?,
            }),
        );
    }
    let graph = inputs.graph()?;
    let names: BTreeSet<_> = graph
        .iter()
        .map(|i| inputs.lock[*i].name.as_str())
        .collect();
    let patches: BTreeMap<_, _> = inputs
        .patches()?
        .into_iter()
        .filter(|(_, dep)| names.contains(dep.package.as_str()))
        .map(|(key, dep)| (key, dep.canonical))
        .collect();
    let document = json!({
        "format": "rtao/core-inputs/v1",
        "packages": packages,
        "workspace_dependencies": inputs.inherited_dependencies,
        "workspace_package": inputs.inherited_package,
        "patch_crates_io": patches,
    });
    let bytes = input_bytes(document)?;
    Ok(format!("b3:{}", blake3::hash(&bytes).to_hex()))
}

/// The all-features core graph with identities from upstream's lockfile.
///
/// Cargo resolves normal/build dependencies from source-free Git manifest
/// projections. Path-package dev dependencies and node/runtime feature union
/// are excluded. Lock versions identify oracle inputs and are never hash inputs.
pub(crate) fn resolved_graph(repo: &Path, revision: &str) -> Result<Vec<LockedPackage>> {
    let mut inputs = Inputs::new(repo, revision)?;
    inputs.resolve(false)?;
    Ok(inputs
        .graph()?
        .into_iter()
        .map(|i| inputs.lock[i].clone())
        .collect())
}

/// Populate external source caches during bootstrap; ordinary pin checks are offline.
pub(crate) fn prepare_source_cache(repo: &Path, revision: &str) -> Result<()> {
    Inputs::new(repo, revision)?.resolve(true)
}

/// A source-qualified package identity from the upstream lockfile.
#[derive(Clone, Debug)]
pub(crate) struct LockedPackage {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) source: Option<String>,
    dependencies: Vec<String>,
}

struct Git<'a> {
    repo: &'a Path,
    commit: String,
}

impl<'a> Git<'a> {
    fn new(repo: &'a Path, revision: &str) -> Result<Self> {
        ensure!(
            !revision.is_empty()
                && !revision.starts_with('-')
                && !revision.contains("..")
                && revision
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b)),
            "revision must be a commit ID or a plain ref name"
        );
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", "--end-of-options"])
            .arg(format!("{revision}^{{commit}}"))
            .output()
            .context("run git revision lookup")?;
        ensure!(
            output.status.success(),
            "unknown commit revision {revision}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let commit = String::from_utf8(output.stdout)?.trim().to_owned();
        ensure!(
            matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "git did not return a commit ID"
        );
        Ok(Self { repo, commit })
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.repo)
            .args(args)
            .output()
            .context("run git object lookup")?;
        ensure!(
            output.status.success(),
            "git object lookup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output.stdout)
    }

    fn object(&self, path: &str, kind: &str, required: bool) -> Result<Option<String>> {
        let output = self.run(&["ls-tree", "-z", &self.commit, "--", path])?;
        let entries: Vec<_> = output
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .collect();
        if entries.is_empty() {
            ensure!(!required, "missing {path} at {}", self.commit);
            return Ok(None);
        }
        ensure!(entries.len() == 1, "ambiguous tree path {path}");
        let line = std::str::from_utf8(entries[0])?;
        let (header, found_path) = line.split_once('\t').context("invalid git ls-tree entry")?;
        let fields: Vec<_> = header.split_whitespace().collect();
        ensure!(
            fields.len() == 3 && found_path == path && fields[1] == kind && fields[0] != "120000",
            "{path} must be a regular {kind}, not a symlink or submodule"
        );
        Ok(Some(fields[2].to_owned()))
    }

    fn document(&self, path: &str) -> Result<DocumentMut> {
        let oid = self
            .object(path, "blob", true)?
            .context("required manifest absent")?;
        let bytes = self.run(&["cat-file", "blob", &oid])?;
        std::str::from_utf8(&bytes)?
            .parse::<DocumentMut>()
            .with_context(|| format!("parse {path} at {}", self.commit))
    }
}

#[derive(Clone)]
struct Dependency {
    package: String,
    version: Option<VersionReq>,
    path: Option<String>,
    git: Option<String>,
    registry: Option<String>,
    canonical: Value,
}

struct Package {
    name: String,
    version: String,
    dependencies: Vec<Dependency>,
}

struct Inputs<'a> {
    git: Git<'a>,
    root: DocumentMut,
    lock: Vec<LockedPackage>,
    paths: BTreeMap<String, Package>,
    inherited_dependencies: BTreeMap<String, Value>,
    inherited_package: BTreeMap<String, Value>,
    active_graph: Option<BTreeSet<usize>>,
}

impl<'a> Inputs<'a> {
    fn new(repo: &'a Path, revision: &str) -> Result<Self> {
        let git = Git::new(repo, revision)?;
        let root = git.document("Cargo.toml")?;
        let document = git.document("Cargo.lock")?;
        let packages = document
            .get("package")
            .and_then(Item::as_array_of_tables)
            .context("Cargo.lock has no package array")?;
        let mut lock = Vec::new();
        let mut identities = BTreeSet::new();
        for package in packages {
            let name = package
                .get("name")
                .and_then(Item::as_str)
                .context("lock package name missing")?
                .to_owned();
            let version = package
                .get("version")
                .and_then(Item::as_str)
                .context("lock package version missing")?
                .to_owned();
            Version::parse(&version).context("invalid lock package version")?;
            let source = package
                .get("source")
                .map(|v| {
                    v.as_str()
                        .context("lock source must be a string")
                        .map(str::to_owned)
                })
                .transpose()?;
            ensure!(
                identities.insert((name.clone(), version.clone(), source.clone())),
                "duplicate locked package {name} {version}"
            );
            let dependencies = match package.get("dependencies") {
                None => Vec::new(),
                Some(item) => item
                    .as_array()
                    .context("lock dependencies must be an array")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .context("lock dependency must be a string")
                            .map(str::to_owned)
                    })
                    .collect::<Result<_>>()?,
            };
            lock.push(LockedPackage {
                name,
                version,
                source,
                dependencies,
            });
        }
        Ok(Self {
            git,
            root,
            lock,
            paths: BTreeMap::new(),
            inherited_dependencies: BTreeMap::new(),
            inherited_package: BTreeMap::new(),
            active_graph: None,
        })
    }

    fn resolve(&mut self, allow_fetch: bool) -> Result<()> {
        self.visit(CORE)?;
        // Full-workspace lock edges include features enabled only by the node.
        // Resolve a source-free core projection to determine active patch names.
        let possible = self.graph()?;
        let possible_names: BTreeSet<_> = possible
            .iter()
            .map(|i| self.lock[*i].name.clone())
            .collect();
        let patches = self.patches()?;
        let mut selected: BTreeSet<String> = patches
            .iter()
            .filter(|(_, p)| p.path.is_some() && possible_names.contains(&p.package))
            .map(|(k, _)| k.clone())
            .collect();
        let mut projection = Projection::new(self.git.repo)?;
        loop {
            let graph = projection.resolve(self, &selected, allow_fetch)?;
            let names: BTreeSet<_> = graph.iter().map(|i| self.lock[*i].name.clone()).collect();
            let next: BTreeSet<_> = patches
                .iter()
                .filter(|(_, p)| names.contains(&p.package))
                .map(|(k, _)| k.clone())
                .collect();
            if selected == next {
                let additions: Vec<_> = patches
                    .iter()
                    .filter(|(k, _)| selected.contains(*k))
                    .filter_map(|(_, p)| p.path.clone())
                    .filter(|p| !self.paths.contains_key(p))
                    .collect();
                for path in additions {
                    self.visit(&path)?;
                }
                self.active_graph = Some(graph);
                break;
            }
            ensure!(
                selected.is_subset(&next) || next.is_subset(&selected),
                "patch resolution is unstable"
            );
            selected = next;
        }
        projection.remove()?;
        Ok(())
    }

    fn patches(&self) -> Result<BTreeMap<String, Dependency>> {
        let Some(table) = self.root.get("patch").and_then(|i| i.get("crates-io")) else {
            return Ok(BTreeMap::new());
        };
        table
            .as_table_like()
            .context("patch.crates-io must be a table")?
            .iter()
            .map(|(name, item)| Ok((name.to_owned(), dependency(name, item, "")?)))
            .collect()
    }

    fn visit(&mut self, path: &str) -> Result<()> {
        if self.paths.contains_key(path) {
            return Ok(());
        }
        let document = self.git.document(&format!("{path}/Cargo.toml"))?;
        ensure!(
            document.get("workspace").is_none(),
            "nested workspace at {path} makes root inheritance ambiguous"
        );
        let mut ancestor = path;
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            let manifest = format!("{parent}/Cargo.toml");
            if self.git.object(&manifest, "blob", false)?.is_some() {
                ensure!(
                    self.git.document(&manifest)?.get("workspace").is_none(),
                    "nested workspace at {parent} makes root inheritance ambiguous"
                );
            }
            ancestor = parent;
        }
        let package = document
            .get("package")
            .and_then(Item::as_table_like)
            .context("path manifest must have a package table")?;
        if let Some(workspace) = package.get("workspace") {
            let location = workspace
                .as_str()
                .context("package.workspace must be a path")?;
            ensure!(
                normalize_path(path, location)?.is_empty(),
                "only root workspace inheritance is supported"
            );
        }
        for (key, item) in package.iter() {
            if item.get("workspace").and_then(Item::as_bool) == Some(true) {
                let root_item = self
                    .root
                    .get("workspace")
                    .and_then(|i| i.get("package"))
                    .and_then(|i| i.get(key))
                    .with_context(|| format!("unresolved workspace.package.{key}"))?;
                self.inherited_package
                    .insert(key.to_owned(), canonical(root_item)?);
            }
        }
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .context("package.name must be explicit")?
            .to_owned();
        let version_item = package.get("version").context("package.version missing")?;
        let version = if version_item.get("workspace").and_then(Item::as_bool) == Some(true) {
            self.root
                .get("workspace")
                .and_then(|i| i.get("package"))
                .and_then(|i| i.get("version"))
                .and_then(Item::as_str)
        } else {
            version_item.as_str()
        }
        .context("package.version must resolve to a string")?
        .to_owned();
        Version::parse(&version)?;
        let mut tables = Vec::new();
        collect_dependency_tables(document.as_item(), &mut tables)?;
        let mut dependencies = Vec::new();
        for table in tables {
            for (name, item) in table
                .as_table_like()
                .context("dependencies must be a table")?
                .iter()
            {
                let (item, base) = if item.get("workspace").and_then(Item::as_bool) == Some(true) {
                    for (key, _) in item
                        .as_table_like()
                        .context("inherited dependency must be a table")?
                        .iter()
                    {
                        ensure!(
                            ["workspace", "features", "optional", "default-features"]
                                .contains(&key),
                            "inherited dependency {name} overrides unsupported key {key}"
                        );
                    }
                    let inherited = self
                        .root
                        .get("workspace")
                        .and_then(|i| i.get("dependencies"))
                        .and_then(|i| i.get(name))
                        .with_context(|| format!("unresolved workspace dependency {name}"))?;
                    self.inherited_dependencies
                        .insert(name.to_owned(), canonical(inherited)?);
                    (inherited, "")
                } else {
                    (item, path)
                };
                dependencies.push(
                    dependency(name, item, base)
                        .with_context(|| format!("resolve {name} in {path}"))?,
                );
            }
        }
        let children: Vec<_> = dependencies.iter().filter_map(|d| d.path.clone()).collect();
        // Inserting before recursion handles cycles without dropping either input.
        self.paths.insert(
            path.to_owned(),
            Package {
                name,
                version,
                dependencies,
            },
        );
        for child in children {
            self.visit(&child)?;
        }
        Ok(())
    }

    fn path_index(&self, package: &Package) -> Result<usize> {
        let found: Vec<_> = self
            .lock
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.name == package.name && p.version == package.version && p.source.is_none()
            })
            .map(|(i, _)| i)
            .collect();
        ensure!(
            found.len() == 1,
            "path package {} {} is missing or ambiguous in Cargo.lock",
            package.name,
            package.version
        );
        Ok(found[0])
    }

    fn lock_reference(&self, reference: &str) -> Result<usize> {
        let (identity, source) = match reference.split_once(" (") {
            Some((id, source)) => (
                id,
                Some(
                    source
                        .strip_suffix(')')
                        .context("invalid locked source suffix")?,
                ),
            ),
            None => (reference, None),
        };
        let fields: Vec<_> = identity.split_whitespace().collect();
        ensure!(
            matches!(fields.len(), 1 | 2),
            "invalid lock dependency reference {reference}"
        );
        let found: Vec<_> = self
            .lock
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.name == fields[0]
                    && (fields.len() == 1 || p.version == fields[1])
                    && source.is_none_or(|s| {
                        p.source
                            .as_deref()
                            .is_some_and(|p| p.split('#').next() == Some(s))
                    })
            })
            .map(|(i, _)| i)
            .collect();
        ensure!(
            found.len() == 1,
            "unresolved or ambiguous lock dependency {reference}"
        );
        Ok(found[0])
    }

    fn graph(&self) -> Result<BTreeSet<usize>> {
        if let Some(graph) = &self.active_graph {
            return Ok(graph.clone());
        }
        let path_indices: BTreeMap<_, _> = self
            .paths
            .values()
            .map(|p| Ok((self.path_index(p)?, p)))
            .collect::<Result<_>>()?;
        let core = self.paths.get(CORE).context("core package missing")?;
        let mut pending = vec![self.path_index(core)?];
        let mut seen = BTreeSet::new();
        let patches = self.patches()?;
        while let Some(index) = pending.pop() {
            if !seen.insert(index) {
                continue;
            }
            let locked = &self.lock[index];
            if let Some(package) = path_indices.get(&index) {
                for dep in &package.dependencies {
                    let candidates: Vec<_> = locked
                        .dependencies
                        .iter()
                        .map(|r| self.lock_reference(r))
                        .collect::<Result<Vec<_>>>()?
                        .into_iter()
                        .filter(|i| {
                            self.lock[*i].name == dep.package
                                && dep.version.as_ref().is_none_or(|v| {
                                    Version::parse(&self.lock[*i].version)
                                        .is_ok_and(|v2| v.matches(&v2))
                                })
                        })
                        .collect();
                    ensure!(
                        candidates.len() == 1,
                        "dependency {} of {} is missing or ambiguous in Cargo.lock",
                        dep.package,
                        package.name
                    );
                    let target = &self.lock[candidates[0]];
                    if let Some(path) = &dep.path {
                        let local = self
                            .paths
                            .get(path)
                            .with_context(|| format!("unresolved path dependency {path}"))?;
                        ensure!(
                            local.name == dep.package
                                && target.source.is_none()
                                && target.version == local.version,
                            "path dependency {} resolves to wrong package",
                            dep.package
                        );
                    } else if let Some(git) = &dep.git {
                        ensure!(
                            target
                                .source
                                .as_deref()
                                .is_some_and(|s| git_source_matches(s, git)),
                            "git dependency {} resolves to wrong source",
                            dep.package
                        );
                    } else if !patches.values().any(|p| p.package == dep.package) {
                        ensure!(
                            dep.registry.is_none(),
                            "custom registry dependency {} cannot be resolved without registry configuration",
                            dep.package
                        );
                        ensure!(
                            target
                                .source
                                .as_deref()
                                .is_some_and(|s| s.starts_with("registry+")
                                    && (s.ends_with("crates.io-index")
                                        || s.ends_with("index.crates.io/"))),
                            "registry dependency {} resolves to wrong source",
                            dep.package
                        );
                    }
                    pending.push(candidates[0]);
                }
            } else {
                for reference in &locked.dependencies {
                    pending.push(self.lock_reference(reference)?);
                }
            }
        }
        Ok(seen)
    }
}

fn git_source_matches(source: &str, url: &str) -> bool {
    source
        .strip_prefix("git+")
        .and_then(|s| s.split(['?', '#']).next())
        .is_some_and(|s| s.trim_end_matches(".git") == url.trim_end_matches(".git"))
}

static NEXT_PROJECTION: AtomicU64 = AtomicU64::new(0);

struct Projection {
    path: PathBuf,
    root: PathBuf,
    removed: bool,
}

impl Projection {
    fn new(repo: &Path) -> Result<Self> {
        let root = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("target/fingerprint"));
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let path = root.join(format!(
            "fingerprint-resolve-{}-{}",
            std::process::id(),
            NEXT_PROJECTION.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self {
            path,
            root,
            removed: false,
        })
    }

    fn resolve(
        &mut self,
        inputs: &Inputs<'_>,
        selected: &BTreeSet<String>,
        allow_fetch: bool,
    ) -> Result<BTreeSet<usize>> {
        let patches = inputs.patches()?;
        let mut paths = BTreeSet::new();
        self.materialize(inputs, CORE, &mut paths)?;
        for key in selected {
            if let Some(path) = &patches[key].path {
                self.materialize(inputs, path, &mut paths)?;
            }
        }
        let mut root = DocumentMut::new();
        root["workspace"] = Item::Table(toml_edit::Table::new());
        root["workspace"]["resolver"] = toml_edit::value("2");
        root["workspace"]["members"] = toml_edit::value(
            paths
                .iter()
                .map(String::as_str)
                .collect::<toml_edit::Array>(),
        );
        for key in ["package", "dependencies", "lints"] {
            if let Some(value) = inputs.root.get("workspace").and_then(|w| w.get(key)) {
                root["workspace"][key] = value.clone();
            }
        }
        root["patch"] = Item::Table(toml_edit::Table::new());
        root["patch"]["crates-io"] = Item::Table(toml_edit::Table::new());
        for key in selected {
            root["patch"]["crates-io"][key] = inputs.root["patch"]["crates-io"][key].clone();
        }
        fs::write(self.path.join("Cargo.toml"), root.to_string())?;
        fs::write(
            self.path.join("Cargo.lock"),
            inputs.git.document("Cargo.lock")?.to_string(),
        )?;
        let core = inputs.git.document(&format!("{CORE}/Cargo.toml"))?;
        let mut features = BTreeSet::new();
        if let Some(table) = core.get("features") {
            for (key, _) in table
                .as_table_like()
                .context("core features must be a table")?
                .iter()
            {
                features.insert(key.to_owned());
            }
        }
        let mut dependency_tables = Vec::new();
        collect_dependency_tables(core.as_item(), &mut dependency_tables)?;
        let explicit_dep_features: BTreeSet<_> = core
            .get("features")
            .and_then(Item::as_table_like)
            .into_iter()
            .flat_map(|t| t.iter())
            .filter_map(|(_, i)| i.as_array())
            .flat_map(|a| a.iter())
            .filter_map(|v| v.as_str())
            .filter_map(|v| v.strip_prefix("dep:"))
            .collect();
        for table in dependency_tables {
            for (alias, item) in table
                .as_table_like()
                .context("dependency table malformed")?
                .iter()
            {
                if item.get("optional").and_then(Item::as_bool) == Some(true)
                    && !explicit_dep_features.contains(alias)
                {
                    features.insert(alias.to_owned());
                }
            }
        }
        let name = inputs.paths.get(CORE).context("core absent")?.name.as_str();
        let feature_arg = features
            .iter()
            .map(|f| format!("{name}/{f}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut command = Command::new("cargo");
        command
            .current_dir(&self.path)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-default-features",
                "--manifest-path",
            ])
            .arg(self.path.join("Cargo.toml"));
        if !allow_fetch {
            command.arg("--offline");
        }
        if !feature_arg.is_empty() {
            command.args(["--features", &feature_arg]);
        }
        let output = command
            .output()
            .context("resolve core-only Cargo metadata")?;
        ensure!(
            output.status.success(),
            "offline core resolution failed; prepare Cargo's source cache during pin bootstrap: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let metadata: Value = serde_json::from_slice(&output.stdout)?;
        let packages = metadata["packages"]
            .as_array()
            .context("Cargo metadata packages absent")?;
        let by_id: BTreeMap<_, _> = packages
            .iter()
            .map(|p| Ok((p["id"].as_str().context("Cargo package ID absent")?, p)))
            .collect::<Result<_>>()?;
        let manifest = self.path.join(CORE).join("Cargo.toml");
        let roots: Vec<_> = packages
            .iter()
            .filter(|p| p["manifest_path"].as_str() == manifest.to_str())
            .collect();
        ensure!(
            roots.len() == 1,
            "Cargo metadata core is absent or ambiguous"
        );
        let nodes = metadata["resolve"]["nodes"]
            .as_array()
            .context("Cargo resolve nodes absent")?;
        let nodes: BTreeMap<_, _> = nodes
            .iter()
            .map(|n| Ok((n["id"].as_str().context("Cargo node ID absent")?, n)))
            .collect::<Result<_>>()?;
        let mut pending = vec![roots[0]["id"].as_str().context("core ID absent")?];
        let mut seen = BTreeSet::new();
        let mut graph = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let package = by_id.get(id).context("resolved Cargo package absent")?;
            let name = package["name"]
                .as_str()
                .context("Cargo package name absent")?;
            let version = package["version"]
                .as_str()
                .context("Cargo package version absent")?;
            let source = package["source"].as_str();
            let matches: Vec<_> = inputs
                .lock
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.name == name && p.version == version && p.source.as_deref() == source
                })
                .map(|(i, _)| i)
                .collect();
            ensure!(
                matches.len() == 1,
                "resolved package {name} {version} {source:?} is absent or ambiguous in upstream Cargo.lock"
            );
            graph.insert(matches[0]);
            let node = nodes.get(id).context("resolved Cargo node absent")?;
            for dependency in node["deps"].as_array().context("Cargo node deps absent")? {
                pending.push(
                    dependency["pkg"]
                        .as_str()
                        .context("Cargo dependency ID absent")?,
                );
            }
        }
        Ok(graph)
    }

    fn materialize(
        &self,
        inputs: &Inputs<'_>,
        path: &str,
        paths: &mut BTreeSet<String>,
    ) -> Result<()> {
        ensure!(
            !path.is_empty(),
            "a core path dependency cannot be the repository root"
        );
        if !paths.insert(path.to_owned()) {
            return Ok(());
        }
        let mut manifest = inputs.git.document(&format!("{path}/Cargo.toml"))?;
        strip_dev_tables(manifest.as_item_mut())?;
        manifest.as_table_mut().remove("workspace");
        for key in ["test", "bench", "example", "bin"] {
            manifest.as_table_mut().remove(key);
        }
        for key in [
            "autobins",
            "autoexamples",
            "autotests",
            "autobenches",
            "build",
        ] {
            manifest["package"][key] = toml_edit::value(false);
        }
        manifest["package"]
            .as_table_like_mut()
            .context("package table absent")?
            .remove("workspace");
        if manifest.get("lib").is_none() {
            manifest["lib"] = Item::Table(toml_edit::Table::new());
        }
        manifest["lib"]["path"] = toml_edit::value("src/lib.rs");
        let mut tables = Vec::new();
        collect_dependency_tables(manifest.as_item(), &mut tables)?;
        let mut children = Vec::new();
        for table in tables {
            for (alias, item) in table
                .as_table_like()
                .context("dependency table absent")?
                .iter()
            {
                let (item, base) = if item.get("workspace").and_then(Item::as_bool) == Some(true) {
                    (
                        inputs
                            .root
                            .get("workspace")
                            .and_then(|w| w.get("dependencies"))
                            .and_then(|d| d.get(alias))
                            .with_context(|| format!("unresolved workspace dependency {alias}"))?,
                        "",
                    )
                } else {
                    (item, path)
                };
                if let Some(path) = dependency(alias, item, base)?.path {
                    children.push(path);
                }
            }
        }
        let destination = self.path.join(path);
        fs::create_dir_all(destination.join("src"))?;
        fs::write(destination.join("Cargo.toml"), manifest.to_string())?;
        fs::write(
            destination.join("src/lib.rs"),
            "// Source-free Cargo resolution only.\n",
        )?;
        for child in children {
            self.materialize(inputs, &child, paths)?;
        }
        Ok(())
    }

    fn remove(&mut self) -> Result<()> {
        if !self.removed {
            let actual = self.path.canonicalize()?;
            ensure!(
                actual.starts_with(&self.root) && actual != self.root,
                "projection cleanup escaped task root"
            );
            // Cargo metadata compiles nothing and installs no service/venv/config.
            // Its sole process has been reaped before cleanup of these owned files.
            fs::remove_dir_all(&actual)?;
            self.removed = true;
        }
        Ok(())
    }
}

impl Drop for Projection {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

fn strip_dev_tables(item: &mut Item) -> Result<()> {
    let table = item
        .as_table_like_mut()
        .context("manifest section must be a table")?;
    table.remove("dev-dependencies");
    if let Some(targets) = table.get_mut("target") {
        for (_, target) in targets
            .as_table_like_mut()
            .context("target must be a table")?
            .iter_mut()
        {
            strip_dev_tables(target)?;
        }
    }
    Ok(())
}

fn collect_dependency_tables<'a>(item: &'a Item, out: &mut Vec<&'a Item>) -> Result<()> {
    for key in ["dependencies", "build-dependencies"] {
        if let Some(table) = item.get(key) {
            out.push(table);
        }
    }
    if let Some(targets) = item.get("target") {
        for (_, target) in targets
            .as_table_like()
            .context("target must be a table")?
            .iter()
        {
            collect_dependency_tables(target, out)?;
        }
    }
    Ok(())
}

fn dependency(alias: &str, item: &Item, base: &str) -> Result<Dependency> {
    let version = item
        .as_str()
        .or_else(|| item.get("version").and_then(Item::as_str));
    ensure!(
        item.as_str().is_some() || item.as_table_like().is_some(),
        "dependency {alias} must be a version string or table"
    );
    let package = item
        .get("package")
        .map(|i| i.as_str().context("dependency package must be a string"))
        .transpose()?
        .unwrap_or(alias)
        .to_owned();
    let path = item
        .get("path")
        .map(|i| {
            normalize_path(
                base,
                i.as_str().context("dependency path must be a string")?,
            )
        })
        .transpose()?;
    let git = item
        .get("git")
        .map(|i| {
            i.as_str()
                .context("dependency git must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    let registry = item
        .get("registry")
        .map(|i| {
            i.as_str()
                .context("dependency registry must be a string")
                .map(str::to_owned)
        })
        .transpose()?;
    ensure!(
        path.is_some() || git.is_some() || version.is_some(),
        "dependency {alias} has no resolvable source"
    );
    ensure!(
        !(path.is_some() && git.is_some()),
        "dependency {alias} has conflicting path and git sources"
    );
    Ok(Dependency {
        package,
        version: version.map(VersionReq::parse).transpose()?,
        path,
        git,
        registry,
        canonical: canonical(item)?,
    })
}

fn normalize_path(base: &str, value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && !value.starts_with('/') && !value.contains(['\\', ':', '\0']),
        "dependency path must stay inside the git repository"
    );
    let mut components: Vec<_> = base.split('/').filter(|c| !c.is_empty()).collect();
    for part in value.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                ensure!(
                    components.pop().is_some(),
                    "dependency path escapes git repository"
                );
            }
            part => components.push(part),
        }
    }
    Ok(components.join("/"))
}

fn canonical(item: &Item) -> Result<Value> {
    if let Some(table) = item.as_table_like() {
        let sorted: BTreeMap<_, _> = table
            .iter()
            .map(|(k, v)| Ok((k, canonical(v)?)))
            .collect::<Result<_>>()?;
        return Ok(Value::Object(
            sorted
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect::<Map<_, _>>(),
        ));
    }
    if let Some(tables) = item.as_array_of_tables() {
        return tables
            .iter()
            .map(|t| canonical(&Item::Table(t.clone())))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }
    let value = item.as_value().context("missing TOML value")?;
    match value {
        toml_edit::Value::String(v) => Ok(json!(v.value())),
        toml_edit::Value::Integer(v) => Ok(json!(v.value())),
        toml_edit::Value::Float(v) => serde_json::Number::from_f64(*v.value())
            .map(Value::Number)
            .context("nonfinite TOML value cannot be canonicalized"),
        toml_edit::Value::Boolean(v) => Ok(json!(v.value())),
        toml_edit::Value::Datetime(v) => Ok(json!({"$datetime": v.value().to_string()})),
        toml_edit::Value::Array(v) => v
            .iter()
            .map(|v| canonical(&Item::Value(v.clone())))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        toml_edit::Value::InlineTable(_) => bail!("unexpected inline-table representation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_fingerprint_keeps_legacy_object_order_and_feature_array_order() -> Result<()> {
        let document: Value = serde_json::from_str(
            r#"{"workspace_dependencies":{"wire":{"version":"1","features":["z","a"]}},"packages":{"sdk/bittensor-core":{"src_tree":"tree","manifest_blob":"blob","build_blob":null}},"format":"rtao/core-inputs/v1"}"#,
        )?;
        let legacy = br#"{"format":"rtao/core-inputs/v1","packages":{"sdk/bittensor-core":{"build_blob":null,"manifest_blob":"blob","src_tree":"tree"}},"workspace_dependencies":{"wire":{"features":["z","a"],"version":"1"}}}"#;
        assert_eq!(input_bytes(document)?.as_slice(), legacy);
        Ok(())
    }

    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join("target/fingerprint-tests")
                });
            fs::create_dir_all(&root)?;
            let root = root.canonicalize()?;
            let path = root.join(format!(
                "fingerprint-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            let fixture = Self { path, root };
            fixture.git(&["init", "--quiet"])?;
            fixture.git(&["config", "user.name", "Fingerprint test"])?;
            fixture.git(&["config", "user.email", "fingerprint@example.invalid"])?;
            fixture.write(
                "Cargo.toml",
                r#"[workspace]
members = ["sdk/bittensor-core", "support/helper"]
resolver = "2"
[workspace.package]
edition = "2024"
license = "MIT"
[workspace.dependencies]
helper = { path = "support/helper", default-features = false }
unused = "1"
[patch.crates-io]
inner = { path = "vendor/inner" }
wire = { path = "vendor/wire" }
zstd-safe = { git = "https://example.invalid/zstd", rev = "aaaa" }
"#,
            )?;
            fixture.write(
                "sdk/bittensor-core/Cargo.toml",
                r#"[package]
name = "bittensor-core"
version = "0.1.0"
edition = "2021"
[dependencies]
helper = { workspace = true }
wire-alias = { package = "wire", version = "1" }
[dev-dependencies]
dev-only = { path = "../../dev-only" }
"#,
            )?;
            fixture.write(
                "support/helper/Cargo.toml",
                r#"[package]
name = "helper"
version = "0.1.0"
edition.workspace = true
[dependencies]
wire = "1"
"#,
            )?;
            fixture.write(
                "vendor/inner/Cargo.toml",
                "[package]\nname = \"inner\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            )?;
            fixture.write("vendor/wire/Cargo.toml", "[package]\nname='wire'\nversion='1.0.0'\nedition='2021'\n[dependencies]\ninner='1'\n")?;
            fixture.write("vendor/wire/src/lib.rs", "pub fn wire() {}\n")?;
            fixture.write("sdk/bittensor-core/src/lib.rs", "pub fn core() {}\n")?;
            fixture.write("support/helper/src/lib.rs", "pub fn helper() {}\n")?;
            fixture.write("vendor/inner/src/lib.rs", "pub fn inner() {}\n")?;
            fixture.write(
                "Cargo.lock",
                r#"version = 4
[[package]]
name = "bittensor-core"
version = "0.1.0"
dependencies = ["helper", "wire"]
[[package]]
name = "helper"
version = "0.1.0"
dependencies = ["wire"]
[[package]]
name = "wire"
version = "1.0.0"
dependencies = ["inner"]
[[package]]
name = "inner"
version = "1.0.0"
[[package]]
name = "unrelated"
version = "1.0.0"
"#,
            )?;
            fixture.commit()?;
            Ok(fixture)
        }

        fn git(&self, args: &[&str]) -> Result<String> {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.path)
                .args(args)
                .output()?;
            ensure!(
                output.status.success(),
                "fixture git failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Ok(String::from_utf8(output.stdout)?)
        }

        fn write(&self, path: &str, contents: &str) -> Result<()> {
            let path = self.path.join(path);
            fs::create_dir_all(path.parent().context("fixture path has no parent")?)?;
            fs::write(path, contents)?;
            Ok(())
        }

        fn replace(&self, path: &str, from: &str, to: &str) -> Result<()> {
            let text = fs::read_to_string(self.path.join(path))?;
            ensure!(text.contains(from), "fixture replacement missing {from}");
            self.write(path, &text.replace(from, to))
        }

        fn commit(&self) -> Result<String> {
            self.git(&["add", "."])?;
            self.git(&[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ])?;
            Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_owned())
        }

        fn hash(&self) -> Result<String> {
            fingerprint(&self.path, "HEAD")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this fixture's known generated files exist here. All spawned git
            // processes have been synchronously reaped; no launchd/config/venv
            // entries are installed and no application depends on these sources.
            if self
                .path
                .canonicalize()
                .is_ok_and(|p| p.starts_with(&self.root) && p != self.root)
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn immutable_git_inputs_and_excluded_test_trees() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        fixture.write("sdk/bittensor-core/src/lib.rs", "uncommitted source\n")?;
        assert_eq!(fixture.hash()?, initial);
        fixture.write("sdk/bittensor-core/src/lib.rs", "pub fn core() {}\n")?;
        for path in ["tests/test.rs", "benches/speed.rs", "fixtures/vector.json"] {
            fixture.write(&format!("sdk/bittensor-core/{path}"), "excluded\n")?;
        }
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        fixture.write("support/helper/src/lib.rs", "pub fn changed() {}\n")?;
        let changed_commit = fixture.commit()?;
        assert_ne!(fixture.hash()?, initial);
        fixture.git(&["checkout", "--quiet", "HEAD~1"])?;
        assert_ne!(fingerprint(&fixture.path, &changed_commit)?, initial);
        assert_eq!(fixture.hash()?, initial);
        Ok(())
    }

    #[test]
    fn inherited_root_inputs_are_normalized_and_scoped() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        fixture.replace("Cargo.toml", "helper = { path = \"support/helper\", default-features = false }", "[workspace.dependencies.helper]\ndefault-features=false # same value\npath='support/helper'")?;
        // Restore table context so the following unused key remains a dependency.
        fixture.replace(
            "Cargo.toml",
            "unused = \"1\"",
            "[workspace.dependencies.unused]\nversion='99'",
        )?;
        fixture.replace("Cargo.toml", "rev = \"aaaa\"", "rev = \"bbbb\"")?;
        fixture.replace(
            "Cargo.toml",
            "license = \"MIT\"",
            "license = \"Apache-2.0\"",
        )?;
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        fixture.replace("Cargo.toml", "edition = \"2024\"", "edition = \"2021\"")?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, initial);
        fixture.replace("Cargo.toml", "edition = \"2021\"", "edition = \"2024\"")?;
        fixture.replace(
            "Cargo.toml",
            "default-features=false",
            "default-features=true",
        )?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, initial);
        Ok(())
    }

    #[test]
    fn transitive_patch_sources_and_build_scripts_are_inputs() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        let graph = resolved_graph(&fixture.path, "HEAD")?;
        assert_eq!(
            graph
                .iter()
                .map(|p| p.name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["bittensor-core", "helper", "inner", "wire"])
        );
        fixture.replace(
            "Cargo.toml",
            "inner = { path = \"vendor/inner\" }",
            "inner={path='vendor/inner'} # formatting",
        )?;
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        fixture.write("vendor/inner/src/lib.rs", "pub fn patched() {}\n")?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, initial);
        let before_build = fixture.hash()?;
        fixture.write("support/helper/build.rs", "fn main() {}\n")?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, before_build);
        let before_manifest = fixture.hash()?;
        fixture.replace(
            "support/helper/Cargo.toml",
            "edition.workspace = true",
            "edition.workspace = true # manifest blob intentionally counted",
        )?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, before_manifest);
        Ok(())
    }

    #[test]
    fn new_optional_target_build_path_dependencies_join_the_closure() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        let helper = fs::read_to_string(fixture.path.join("support/helper/Cargo.toml"))?;
        fixture.write("support/helper/Cargo.toml", &format!("{helper}\n[target.'cfg(unix)'.build-dependencies]\nextra={{path='../extra', optional=true}}\n"))?;
        fixture.write(
            "support/extra/Cargo.toml",
            "[package]\nname='extra'\nversion='0.1.0'\nedition='2021'\n",
        )?;
        fixture.write("support/extra/src/lib.rs", "pub fn extra() {}\n")?;
        fixture.replace(
            "Cargo.lock",
            "dependencies = [\"wire\"]",
            "dependencies = [\"wire\", \"extra\"]",
        )?;
        let lock = fs::read_to_string(fixture.path.join("Cargo.lock"))?;
        fixture.write(
            "Cargo.lock",
            &format!("{lock}\n[[package]]\nname='extra'\nversion='0.1.0'\n"),
        )?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, initial);
        let before_source = fixture.hash()?;
        fixture.write("support/extra/src/lib.rs", "pub fn another() {}\n")?;
        fixture.commit()?;
        assert_ne!(fixture.hash()?, before_source);
        Ok(())
    }

    #[test]
    fn rejects_unresolved_inheritance_escape_and_revision_injection() -> Result<()> {
        let fixture = Fixture::new()?;
        for revision in [
            "--help",
            "HEAD:Cargo.toml",
            "HEAD^{tree}",
            "v1;touch /tmp/x",
            "../HEAD",
            "$(id)",
        ] {
            assert!(fingerprint(&fixture.path, revision).is_err());
        }
        fixture.replace(
            "Cargo.toml",
            "helper = { path = \"support/helper\", default-features = false }",
            "helper = { path = \"../outside\" }",
        )?;
        fixture.commit()?;
        assert!(fixture.hash().is_err());
        fixture.replace(
            "Cargo.toml",
            "helper = { path = \"../outside\" }",
            "missing = { path = \"support/helper\" }",
        )?;
        fixture.commit()?;
        assert!(fixture.hash().is_err());
        fixture.replace(
            "Cargo.toml",
            "missing = { path = \"support/helper\" }",
            "helper = { path = \"support/helper\" }",
        )?;
        fixture.write(
            "sdk/Cargo.toml",
            "[workspace]\n[workspace.dependencies]\nhelper={path='../unrelated'}\n",
        )?;
        fixture.commit()?;
        assert!(fixture.hash().is_err());
        Ok(())
    }

    #[test]
    fn lock_resolution_is_fail_closed_and_lock_versions_are_not_hash_inputs() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        fixture.replace(
            "Cargo.lock",
            "name = \"unrelated\"\nversion = \"1.0.0\"",
            "name = \"unrelated\"\nversion = \"1.1.0\"",
        )?;
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        let lock = fs::read_to_string(fixture.path.join("Cargo.lock"))?;
        fixture.write("Cargo.lock", &format!("{lock}\n[[package]]\nname='wire'\nversion='1.2.0'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n"))?;
        fixture.commit()?;
        assert!(fixture.hash().is_err());
        fixture.write(
            "Cargo.lock",
            &lock.replace(
                "dependencies = [\"helper\", \"wire\"]",
                "dependencies = [\"helper\", \"absent\"]",
            ),
        )?;
        fixture.commit()?;
        assert!(fixture.hash().is_err());
        Ok(())
    }

    #[test]
    fn workspace_unified_lock_edges_do_not_activate_unrelated_patches() -> Result<()> {
        let fixture = Fixture::new()?;
        let initial = fixture.hash()?;
        fixture.replace(
            "Cargo.lock",
            "name = \"inner\"\nversion = \"1.0.0\"",
            "name = \"inner\"\nversion = \"1.0.0\"\ndependencies = [\"phantom\"]",
        )?;
        let lock = fs::read_to_string(fixture.path.join("Cargo.lock"))?;
        fixture.write("Cargo.lock", &format!("{lock}\n[[package]]\nname='phantom'\nversion='1.0.0'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n"))?;
        fixture.replace(
            "Cargo.toml",
            "[patch.crates-io]",
            "[patch.crates-io]\nphantom = {git='https://example.invalid/phantom', rev='aaaa'}",
        )?;
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        assert!(
            !resolved_graph(&fixture.path, "HEAD")?
                .iter()
                .any(|p| p.name == "phantom")
        );
        fixture.replace("Cargo.toml", "rev='aaaa'", "rev='bbbb'")?;
        fixture.commit()?;
        assert_eq!(fixture.hash()?, initial);
        Ok(())
    }
}
