//! Declared-layer and package-rooted feature checks for shipped dependency graphs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use rtao_core::dependency_policy::{
    LockedPackage, validate_bls_built, validate_bls_lock, validate_bls_tree,
};
use serde_json::Value as Json;
use toml_edit::{DocumentMut, Item};

#[path = "../../crates/rtao-core/core_provenance.rs"]
mod core_provenance;

const TLS_BANS: &[&str] = &["aws-lc-rs", "aws-lc-sys", "native-tls"];
const NO_HOST_BANS: &[&str] = &[
    "sodiumoxide",
    "libsodium-sys",
    "openssl",
    "openssl-sys",
    "openssl-src",
    "native-tls",
    "fernet",
    "ansible-vault",
    "secp256k1-sys",
    "aws-lc-rs",
    "aws-lc-sys",
    "cmake",
    "hidapi",
    "libloading",
    "rayon",
];
const BYTE_CRATES: &[&str] = &[
    "parity-scale-codec",
    "scale-info",
    "scale-decode",
    "frame-metadata",
    "merkleized-metadata",
    "schnorrkel",
    "ed25519-zebra",
    "bip39",
    "sha2",
    "sha3",
    "blake2",
    "twox-hash",
    "ml-kem",
    "chacha20poly1305",
    "tle",
    "serde_json",
];

#[derive(Clone, Debug)]
struct Dependency {
    name: String,
    alias: String,
    kind: String,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    req: String,
}

#[derive(Clone, Debug)]
struct Package {
    name: String,
    manifest: PathBuf,
    dependencies: Vec<Dependency>,
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Identity {
    name: String,
    version: String,
    // Cargo tree prints Git sources with a short commit and registry sources implicitly.
    source: Option<String>,
}

type Graph = BTreeMap<Identity, BTreeSet<String>>;

struct Profile {
    name: String,
    package: String,
    defaults: bool,
    features: Vec<String>,
    no_host: bool,
    subxt: bool,
    shipped: bool,
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let mut no_host_only = false;
    let mut target = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" => {
                ensure!(
                    args.next().as_deref() == Some("no-host"),
                    "only --profile no-host is supported"
                );
                no_host_only = true;
            }
            "--target" => target = Some(args.next().context("--target needs a target triple")?),
            "--help" | "-h" => {
                println!(
                    "cargo xtask deps-check [--profile no-host] [--target <triple>]\nChecks declared layers, feature gates, lock provenance and separate package-rooted normal/build graphs.\nThe no-host selector checks validator,prometheus and miner,redis separately."
                );
                return Ok(());
            }
            _ => bail!("unknown deps-check option {arg:?}"),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no workspace parent")?;
    let metadata: Json = serde_json::from_slice(&cargo(
        root,
        &[
            "metadata",
            "--locked",
            "--offline",
            "--format-version",
            "1",
            "--no-deps",
            "--all-features",
        ],
    )?)?;
    let packages = parse_metadata(&metadata)?;
    let manifest = read_toml(&root.join("Cargo.toml"))?;
    check_declarations(&packages, &manifest)?;
    check_deny(&read_toml(&root.join("deny.toml"))?)?;
    let lock = read_toml(&root.join("Cargo.lock"))?;
    let locks = read_lock(&lock)?;
    let pins = read_toml(&root.join("upstream.toml"))?;
    if pins["core"].get("canary").is_some() {
        core_provenance::require_disposable(root)
            .map_err(|error| anyhow::Error::msg(error.to_string()))?;
    }
    let bls_source = bls_source(&pins)?;
    check_bls(&lock, &pins)?;
    let aggregate = String::from_utf8(cargo(
        root,
        &[
            "tree",
            "--locked",
            "--offline",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
    )?)?;
    validate_bls_tree(&aggregate, &bls_source).map_err(anyhow::Error::msg)?;
    check_generated(root)?;

    // Each cargo invocation selects exactly one package. A workspace-wide metadata
    // resolve, or tree --workspace/--all-features, would unify owner/test features
    // into the no-host graphs and could neither prove nor disprove these invariants.
    let profiles = profiles(&packages, no_host_only)?;
    for profile in profiles {
        let graph = tree(root, &profile, target.as_deref())?;
        // Fail on ambiguous displayed identities instead of silently merging two
        // registry/path packages with the same name and version. Git annotations
        // retain their source selectors; Cargo omits crates.io source annotations.
        let resolved = resolved_identities(&graph, &locks)?;
        let built: Vec<_> = resolved
            .iter()
            .map(|package| LockedPackage {
                name: &package.name,
                version: &package.version,
                source: package.source.as_deref(),
                checksum: None,
            })
            .collect();
        validate_bls_built(&built, &bls_source).map_err(anyhow::Error::msg)?;
        check_graph(&profile, &graph)?;
        println!(
            "deps-check: {}: {} normal/build packages",
            profile.name,
            graph.len()
        );
    }
    // A core-only tree provides the closure for source comparisons and oracle
    // warnings; facade/tooling dependencies must not enter either comparison.
    let core_graph = tree_with_root(
        root,
        &profile("core no-host", "rtao-core", false, &[], true, false),
        target.as_deref(),
        Some("bittensor-core"),
    )?;
    let staged_graph = tree(
        root,
        &profile(
            "staged core dependencies",
            "rtao-core-ext",
            false,
            &[],
            true,
            false,
        ),
        target.as_deref(),
    )?;
    check_core_ext_copies(&packages, &core_graph, &staged_graph, &locks)?;
    oracle_warnings(root, &core_graph, &locks)?;
    println!("deps-check: declared layers and selected profile invariants passed");
    Ok(())
}

fn read_toml(path: &Path) -> Result<DocumentMut> {
    fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?
        .parse()
        .with_context(|| format!("parse {}", path.display()))
}

fn cargo(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(args)
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .context("run cargo dependency inspection")?;
    ensure!(
        output.status.success(),
        "cargo {} failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn strings(value: &Json) -> Result<Vec<String>> {
    value
        .as_array()
        .context("expected JSON array")?
        .iter()
        .map(|v| Ok(v.as_str().context("expected JSON string")?.to_owned()))
        .collect()
}

fn string<'a>(value: &'a Json, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("metadata missing string {key}"))
}

fn parse_metadata(metadata: &Json) -> Result<Vec<Package>> {
    let members: BTreeSet<_> = strings(&metadata["workspace_members"])?
        .into_iter()
        .collect();
    metadata["packages"]
        .as_array()
        .context("metadata missing packages")?
        .iter()
        .filter(|p| p["id"].as_str().is_some_and(|id| members.contains(id)))
        .map(|p| {
            let dependencies = p["dependencies"]
                .as_array()
                .context("metadata missing dependencies")?
                .iter()
                .map(|d| {
                    Ok(Dependency {
                        name: string(d, "name")?.to_owned(),
                        alias: d["rename"]
                            .as_str()
                            .unwrap_or(string(d, "name")?)
                            .to_owned(),
                        kind: d["kind"].as_str().unwrap_or("normal").to_owned(),
                        optional: d["optional"]
                            .as_bool()
                            .context("metadata missing optional")?,
                        default_features: d["uses_default_features"]
                            .as_bool()
                            .context("metadata missing uses_default_features")?,
                        features: strings(&d["features"])?,
                        req: string(d, "req")?.to_owned(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let features = p["features"]
                .as_object()
                .context("metadata missing features")?
                .iter()
                .map(|(key, value)| Ok((key.clone(), strings(value)?)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            Ok(Package {
                name: string(p, "name")?.to_owned(),
                manifest: string(p, "manifest_path")?.into(),
                dependencies,
                features,
            })
        })
        .collect()
}

fn layer(name: &str) -> Option<u8> {
    Some(match name {
        "rtao-core-ext" => 1,
        "rtao-core" => 2,
        "rtao-primitives" => 3,
        "rtao-catalog" | "rtao-runtime" | "rtao-rpc" | "rtao-signer" => 4,
        "rtao-wallet" | "rtao-client" => 5,
        "rtao-tx" | "rtao-reads" | "rtao-evm" => 6,
        "rtao-intents" | "rtao-metagraph" | "rtao-btauth" => 7,
        "rtao-neuron" => 8,
        "rtao" => 9,
        "rtao-cli" => 10,
        "rtao-testkit" | "xtask" => 11,
        _ => return None,
    })
}

fn check_declarations(packages: &[Package], workspace: &DocumentMut) -> Result<()> {
    let members: BTreeSet<_> = packages.iter().map(|p| p.name.as_str()).collect();
    for package in packages {
        let owner = package.name.as_str();
        let tooling = matches!(owner, "rtao-testkit" | "xtask");
        for dep in &package.dependencies {
            let name = dep.name.as_str();
            if members.contains(name) {
                ensure!(
                    !dep.default_features,
                    "{owner} -> {name} ({}) enables workspace default features",
                    dep.kind
                );
                if dep.kind != "dev" && !tooling {
                    ensure!(
                        name != "rtao-testkit" && name != "xtask",
                        "{owner} ships tooling dependency {name}"
                    );
                    let optional_policy =
                        owner == "rtao-btauth" && name == "rtao-metagraph" && dep.optional;
                    ensure!(
                        layer(name)
                            .zip(layer(owner))
                            .is_some_and(|(to, from)| to < from)
                            || optional_policy,
                        "layer violation: {owner} -> {name}"
                    );
                    ensure!(
                        owner != "rtao-rpc",
                        "rtao-rpc may depend only on third-party crates"
                    );
                }
            }
            if name == "bittensor-core" {
                ensure!(
                    matches!(
                        owner,
                        "rtao-core" | "rtao-core-ext" | "rtao-testkit" | "xtask"
                    ),
                    "{owner} directly depends on bittensor-core"
                );
            }
            if name == "tokio-tungstenite" {
                ensure!(
                    matches!(owner, "rtao-rpc" | "rtao-testkit" | "rtao-signer"),
                    "{owner} directly depends on tokio-tungstenite"
                );
                let expected = workspace["workspace"]["dependencies"][name]
                    .get("version")
                    .and_then(Item::as_str)
                    .or_else(|| workspace["workspace"]["dependencies"][name].as_str())
                    .context("tokio-tungstenite must have a single workspace version")?;
                ensure!(
                    semver::VersionReq::parse(&dep.req)? == semver::VersionReq::parse(expected)?,
                    "{owner}'s tokio-tungstenite requirement differs from workspace version"
                );
                let declaration = read_toml(&package.manifest)?;
                ensure!(
                    inherited_dependency(&declaration, &dep.alias),
                    "{owner} must inherit tokio-tungstenite from workspace.dependencies"
                );
                if owner == "rtao-signer" {
                    check_optional_gate(package, dep, &["extension", "vault"])?;
                }
            }
            if name == "subxt-rpcs" {
                ensure!(
                    matches!(owner, "rtao-rpc" | "rtao-client" | "rtao-testkit"),
                    "{owner} directly depends on subxt-rpcs"
                );
                ensure!(
                    !dep.default_features,
                    "{owner} must disable subxt-rpcs default features"
                );
            }
            if name == "axum" {
                ensure!(
                    !dep.features.iter().any(|f| f == "ws"),
                    "{owner} enables axum/ws (second tungstenite)"
                );
                for values in package.features.values() {
                    ensure!(
                        !values.iter().any(|f| f == &format!("{}/ws", dep.alias)
                            || f == &format!("{}?/ws", dep.alias)),
                        "{owner} enables axum/ws through a feature"
                    );
                }
            }
            if owner == "rtao-core-ext" && dep.kind != "dev" {
                ensure!(
                    [
                        "bittensor-core",
                        "serde",
                        "serde_json",
                        "parity-scale-codec",
                        "hex",
                        "scale-info",
                        "frame-metadata",
                        "schnorrkel",
                        "ed25519-zebra",
                        "subtensor-macros",
                        // Pure env-password compatibility and guarded keyfile
                        // buffers copy the core's portable direct dependencies.
                        "base64",
                        "zeroize",
                    ]
                    .contains(&name),
                    "rtao-core-ext dependency {name} is outside the staged core boundary"
                );
            }
            if name == "rtao-core"
                && dep.kind != "dev"
                && dep.features.iter().any(|f| f == "offload")
            {
                ensure!(
                    matches!(owner, "rtao-client" | "rtao-wallet"),
                    "{owner} unconditionally enables rtao-core/offload"
                );
            }
            if matches!(name, "rtao-client" | "rtao-signer")
                && dep.kind != "dev"
                && owner != "rtao-testkit"
            {
                ensure!(
                    !dep.features.iter().any(|f| f == "test-util"),
                    "{owner} enables shipped {name}/test-util"
                );
            }
            if owner == "rtao" && name == "rtao-neuron" {
                ensure!(
                    !dep.features.iter().any(|f| f == "prometheus"),
                    "facade must not unconditionally enable neuron/prometheus"
                );
            }
        }
        for (feature, values) in &package.features {
            for value in values {
                if let Some((alias, enabled)) = value.split_once('/') {
                    let alias = alias.trim_end_matches('?');
                    if let Some(dep) = package
                        .dependencies
                        .iter()
                        .find(|d| d.alias == alias && d.kind != "dev")
                    {
                        if matches!(dep.name.as_str(), "rtao-signer" | "rtao-client")
                            && enabled == "test-util"
                            && owner != "rtao-testkit"
                        {
                            bail!(
                                "shipped feature {owner}/{feature} enables {}/test-util",
                                dep.name
                            );
                        }
                        if dep.name == "rtao-core" && enabled == "offload" {
                            ensure!(
                                matches!(owner, "rtao-client" | "rtao-wallet")
                                    || (owner == "rtao-signer"
                                        && matches!(feature.as_str(), "ledger" | "vault")),
                                "{owner}/{feature} enables offload outside client, wallet or signer/ledger|vault"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// Includes target-specific sections; a target-specific explicit source must not
// bypass the workspace pin merely because another target inherits it.
fn inherited_dependency(document: &DocumentMut, alias: &str) -> bool {
    fn check(item: &Item, alias: &str, found: &mut bool) -> bool {
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(dependency) = item.get(section).and_then(|s| s.get(alias)) {
                *found = true;
                if dependency.get("workspace").and_then(Item::as_bool) != Some(true) {
                    return false;
                }
            }
        }
        true
    }
    let mut found = false;
    if !check(document.as_item(), alias, &mut found) {
        return false;
    }
    if let Some(targets) = document.get("target").and_then(Item::as_table_like) {
        for (_, target) in targets.iter() {
            if !check(target, alias, &mut found) {
                return false;
            }
        }
    }
    found
}

fn feature_closure(package: &Package, feature: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut pending = vec![feature.to_owned()];
    while let Some(feature) = pending.pop() {
        if result.insert(feature.clone())
            && let Some(values) = package.features.get(&feature)
        {
            pending.extend(values.iter().cloned());
        }
    }
    result
}

fn check_optional_gate(package: &Package, dep: &Dependency, gates: &[&str]) -> Result<()> {
    ensure!(
        dep.optional,
        "{}/{} must be optional and gated by {}",
        package.name,
        dep.name,
        gates.join(" or ")
    );
    for feature in package.features.keys() {
        let closure = feature_closure(package, feature);
        let enabled = closure.iter().any(|f| {
            f == &format!("dep:{}", dep.alias)
                || f == &dep.alias
                || f.starts_with(&format!("{}/", dep.alias))
        });
        if enabled {
            ensure!(
                gates.iter().any(|gate| closure.contains(*gate)),
                "{}/{feature} enables {} without {}",
                package.name,
                dep.name,
                gates.join(" or ")
            );
        }
    }
    Ok(())
}

fn profile(
    name: &str,
    package: &str,
    defaults: bool,
    features: &[&str],
    no_host: bool,
    subxt: bool,
) -> Profile {
    Profile {
        name: name.into(),
        package: package.into(),
        defaults,
        features: features.iter().map(|f| (*f).into()).collect(),
        no_host,
        subxt,
        shipped: true,
    }
}

fn profiles(packages: &[Package], no_host_only: bool) -> Result<Vec<Profile>> {
    let facade = packages
        .iter()
        .find(|p| p.name == "rtao")
        .context("workspace missing rtao facade")?;
    for feature in ["validator", "prometheus", "miner", "redis"] {
        ensure!(
            facade.features.contains_key(feature),
            "facade missing no-host feature {feature}"
        );
    }
    let mut profiles = vec![
        profile(
            "validator,prometheus no-host",
            "rtao",
            false,
            &["validator", "prometheus"],
            true,
            false,
        ),
        profile(
            "miner,redis no-host",
            "rtao",
            false,
            &["miner", "redis"],
            true,
            false,
        ),
        profile("core no-host", "rtao-core", false, &[], true, false),
        profile(
            "primitives no-host",
            "rtao-primitives",
            false,
            &[],
            true,
            false,
        ),
    ];
    if !no_host_only {
        profiles.push(profile("facade default", "rtao", true, &[], false, false));
        profiles.push(profile("CLI", "rtao-cli", true, &[], false, false));
        let cli = packages
            .iter()
            .find(|p| p.name == "rtao-cli")
            .context("workspace missing rtao-cli")?;
        let mut cli_features =
            profile("CLI shipped features", "rtao-cli", false, &[], false, false);
        cli_features.features = cli
            .features
            .keys()
            .filter(|f| f.as_str() != "subxt")
            .cloned()
            .collect();
        profiles.push(cli_features);
        if cli.features.contains_key("subxt") {
            let mut cli_subxt = profile("CLI including subxt", "rtao-cli", false, &[], false, true);
            cli_subxt.features = cli.features.keys().cloned().collect();
            profiles.push(cli_subxt);
        }
        // Check the maximal union of every facade feature except the escape hatch.
        // This catches transitive leaks and combination-only provider conflicts.
        let mut shipped = profile(
            "facade shipped without subxt",
            "rtao",
            false,
            &[],
            false,
            false,
        );
        shipped.features = facade
            .features
            .keys()
            .filter(|f| f.as_str() != "subxt")
            .cloned()
            .collect();
        profiles.push(shipped);
        if facade.features.contains_key("subxt") {
            let mut subxt = profile("facade including subxt", "rtao", false, &[], false, true);
            subxt.features = facade.features.keys().cloned().collect();
            profiles.push(subxt);
        }
        profiles.push(profile(
            "btauth sans features",
            "rtao-btauth",
            false,
            &[],
            false,
            false,
        ));
        profiles.push(profile(
            "staged core sans features",
            "rtao-core-ext",
            false,
            &[],
            false,
            false,
        ));
        for name in ["xtask", "rtao-testkit"] {
            let package = packages
                .iter()
                .find(|p| p.name == name)
                .with_context(|| format!("workspace missing {name}"))?;
            let mut tooling = profile(name, name, false, &[], false, true);
            tooling.features = package.features.keys().cloned().collect();
            tooling.shipped = false;
            profiles.push(tooling);
        }
    }
    Ok(profiles)
}

fn tree(root: &Path, profile: &Profile, target: Option<&str>) -> Result<Graph> {
    tree_with_root(root, profile, target, None)
}

fn tree_with_root(
    root: &Path,
    profile: &Profile,
    target: Option<&str>,
    subroot: Option<&str>,
) -> Result<Graph> {
    let mut args = vec![
        "tree",
        "--locked",
        "--offline",
        "-p",
        profile.package.as_str(),
        "-e",
        "normal,build",
        "--prefix",
        "depth",
        "--format",
        "{p}|{f}",
    ];
    if !profile.defaults {
        args.push("--no-default-features");
    }
    let features = profile.features.join(",");
    if !features.is_empty() {
        args.extend(["--features", features.as_str()]);
    }
    if let Some(target) = target {
        args.extend(["--target", target]);
    }
    let graph = parse_tree_with_root(&String::from_utf8(cargo(root, &args)?)?, true, subroot)?;
    let selected = subroot.unwrap_or(&profile.package);
    ensure!(
        graph.keys().any(|p| p.name == selected),
        "{} graph has no selected package {selected}",
        profile.name
    );
    Ok(graph)
}

#[cfg(test)]
fn parse_tree(output: &str) -> Result<Graph> {
    parse_tree_with_root(output, false, None)
}

fn parse_tree_with_root(output: &str, depths: bool, subroot: Option<&str>) -> Result<Graph> {
    let mut graph = Graph::new();
    let mut edges = BTreeMap::<Identity, BTreeSet<Identity>>::new();
    let mut ancestors = Vec::<Identity>::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let line = line.trim();
        let (depth, line) = if depths {
            let end = line
                .find(|c: char| !c.is_ascii_digit())
                .context("cargo tree depth without package")?;
            let (depth, line) = line.split_at(end);
            (
                depth.parse::<usize>().context("cargo tree invalid depth")?,
                line,
            )
        } else {
            (0, line)
        };
        let (package, features) = line
            .split_once('|')
            .with_context(|| format!("unrecognized cargo tree line {line:?}"))?;
        let mut words = package.split_whitespace();
        let name = words
            .next()
            .context("cargo tree package name missing")?
            .to_owned();
        let version = words
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .context("cargo tree version missing")?
            .to_owned();
        semver::Version::parse(&version).context("cargo tree package version invalid")?;
        let source = package.rsplit(" (").find_map(|annotation| {
            let annotation = annotation.trim_end_matches(')');
            (annotation.contains("://") || annotation.starts_with("git@"))
                .then(|| annotation.to_owned())
        });
        let identity = Identity {
            name,
            version,
            source,
        };
        if depths {
            ensure!(
                depth <= ancestors.len(),
                "cargo tree depth jumped without parent"
            );
            ancestors.truncate(depth);
            if let Some(parent) = ancestors.last() {
                edges
                    .entry(parent.clone())
                    .or_default()
                    .insert(identity.clone());
            }
            ancestors.push(identity.clone());
        }
        let features = features.trim_end_matches(" (*)").trim();
        graph.entry(identity).or_default().extend(
            features
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        );
    }
    ensure!(!graph.is_empty(), "cargo tree returned an empty graph");
    if let Some(root) = subroot {
        let roots: Vec<_> = graph.keys().filter(|p| p.name == root).cloned().collect();
        ensure!(
            roots.len() == 1,
            "expected one {root} source in rooted tree, got {}",
            roots.len()
        );
        let mut reachable = BTreeSet::new();
        let mut pending = roots;
        while let Some(package) = pending.pop() {
            if reachable.insert(package.clone())
                && let Some(children) = edges.get(&package)
            {
                pending.extend(children.iter().cloned());
            }
        }
        graph.retain(|package, _| reachable.contains(package));
    }
    Ok(graph)
}

fn check_graph(profile: &Profile, graph: &Graph) -> Result<()> {
    for (package, features) in graph {
        let name = package.name.as_str();
        ensure!(
            !TLS_BANS.contains(&name),
            "{} contains banned TLS provider {name}",
            profile.name
        );
        if name == "rustls" {
            ensure!(
                features.contains("ring") && !features.contains("aws_lc_rs"),
                "{} rustls {} must use exactly the ring crypto provider",
                profile.name,
                package.version
            );
        }
        if name == "axum" {
            ensure!(!features.contains("ws"), "{} enables axum/ws", profile.name);
        }
        if profile.shipped {
            ensure!(
                !matches!(name, "rtao-testkit" | "xtask"),
                "{} ships tooling {name}",
                profile.name
            );
            if matches!(name, "rtao-signer" | "rtao-client") {
                ensure!(
                    !features.contains("test-util"),
                    "{} ships {name}/test-util",
                    profile.name
                );
            }
            if matches!(name, "subxt" | "jsonrpsee") {
                ensure!(
                    profile.subxt,
                    "{} contains {name} without the facade subxt escape hatch",
                    profile.name
                );
            }
        }
        if profile.no_host {
            ensure!(
                !NO_HOST_BANS.contains(&name),
                "{} contains no-host banned crate {name}",
                profile.name
            );
            if matches!(name, "bittensor-core" | "rtao-core") {
                ensure!(
                    !features.contains("host"),
                    "{} enables {name}/host",
                    profile.name
                );
            }
            if name == "reqwest" {
                ensure!(
                    !features.contains("blocking"),
                    "{} enables reqwest/blocking",
                    profile.name
                );
            }
            if name == "sp-core" {
                ensure!(
                    !features.contains("std"),
                    "{} enables sp-core/std",
                    profile.name
                );
            }
            if name == "rtao-signer" {
                ensure!(
                    !features.contains("extension") && !features.contains("vault"),
                    "{} enables signer extension/vault",
                    profile.name
                );
            }
            ensure!(
                !matches!(name, "subxt" | "jsonrpsee"),
                "{} contains no-host banned escape hatch {name}",
                profile.name
            );
        }
        if profile.package == "rtao-btauth" {
            ensure!(
                !["tokio", "axum", "reqwest", "hyper"].contains(&name),
                "btauth sans features contains I/O crate {name}"
            );
        }
        if profile.package == "rtao-core-ext" {
            ensure!(name != "tokio", "staged core contains tokio");
        }
    }
    for name in ["tokio-tungstenite", "tungstenite"] {
        ensure!(
            graph.keys().filter(|p| p.name == name).count() <= 1,
            "{} contains multiple {name} versions/sources",
            profile.name
        );
    }
    Ok(())
}

fn check_deny(deny: &DocumentMut) -> Result<()> {
    let banned = deny["bans"]["deny"]
        .as_array()
        .context("deny.toml missing bans.deny")?;
    for name in TLS_BANS {
        ensure!(
            banned.iter().any(|v| v.as_str() == Some(name)
                || v.as_inline_table()
                    .and_then(|t| t.get("name"))
                    .and_then(toml_edit::Value::as_str)
                    == Some(name)),
            "deny.toml does not ban {name}"
        );
    }
    Ok(())
}

fn read_lock(lock: &DocumentMut) -> Result<Vec<Identity>> {
    lock["package"]
        .as_array_of_tables()
        .context("Cargo.lock missing packages")?
        .iter()
        .map(|p| {
            Ok(Identity {
                name: p["name"]
                    .as_str()
                    .context("lock package name missing")?
                    .into(),
                version: p["version"]
                    .as_str()
                    .context("lock package version missing")?
                    .into(),
                source: p.get("source").and_then(Item::as_str).map(str::to_owned),
            })
        })
        .collect()
}

fn bls_source(pins: &DocumentMut) -> Result<String> {
    let repo = pins["monorepo"]["url"]
        .as_str()
        .context("upstream.toml missing monorepo.url")?;
    let pin = core_provenance::effective(pins).map_err(anyhow::Error::msg)?;
    let reference = if pin.branch.is_empty() {
        pin.tag
    } else {
        pin.reference
    };
    Ok(format!(
        "git+{repo}?{}={}#{}",
        pin.selector, reference, pin.commit
    ))
}

fn check_bls(lock: &DocumentMut, pins: &DocumentMut) -> Result<()> {
    let packages = lock["package"]
        .as_array_of_tables()
        .context("locked packages absent")?;
    let packages: Vec<_> = packages
        .iter()
        .map(|package| {
            Ok(LockedPackage {
                name: package["name"]
                    .as_str()
                    .context("locked package name absent")?,
                version: package["version"]
                    .as_str()
                    .context("locked package version absent")?,
                source: package.get("source").and_then(Item::as_str),
                checksum: package.get("checksum").and_then(Item::as_str),
            })
        })
        .collect::<Result<_>>()?;
    validate_bls_lock(&packages, &bls_source(pins)?).map_err(anyhow::Error::msg)
}

fn resolved_identities(graph: &Graph, lock: &[Identity]) -> Result<BTreeSet<Identity>> {
    let mut result = BTreeSet::new();
    for package in graph.keys() {
        let matches: Vec<_> = lock
            .iter()
            .filter(|locked| {
                if locked.name != package.name || locked.version != package.version {
                    return false;
                }
                match &package.source {
                    Some(source) => locked.source.as_ref().is_some_and(|s| {
                        ["git+", "registry+", "sparse+"].iter().any(|prefix| {
                            s.strip_prefix(prefix)
                                .is_some_and(|s| s.starts_with(source))
                        })
                    }),
                    None => !locked
                        .source
                        .as_ref()
                        .is_some_and(|s| s.starts_with("git+")),
                }
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "cannot identify {} {} from cargo tree against Cargo.lock ({} matches)",
            package.name,
            package.version,
            matches.len()
        );
        if let Some(package) = matches.first() {
            result.insert((*package).clone());
        }
    }
    Ok(result)
}

fn check_core_ext_copies(
    packages: &[Package],
    core: &Graph,
    staged: &Graph,
    lock: &[Identity],
) -> Result<()> {
    let ext = packages
        .iter()
        .find(|p| p.name == "rtao-core-ext")
        .context("missing rtao-core-ext")?;
    let core = resolved_identities(core, lock)?;
    let staged = resolved_identities(staged, lock)?;
    for dependency in ext.dependencies.iter().filter(|d| d.kind != "dev") {
        // Tooling and unrelated owner features can lock other versions. Check
        // the actual portable staged build, which must reuse one core identity.
        let instances: BTreeSet<_> = staged
            .iter()
            .filter(|p| p.name == dependency.name)
            .collect();
        let requirement = semver::VersionReq::parse(&dependency.req).with_context(|| {
            format!(
                "invalid staged dependency requirement for {}",
                dependency.name
            )
        })?;
        ensure!(
            instances.len() == 1 && instances.iter().all(|p| core.contains(*p)),
            "staged core dependency {} must have one version/source, already present in bittensor-core's graph",
            dependency.name
        );
        for package in instances {
            ensure!(
                requirement.matches(&semver::Version::parse(&package.version)?),
                "staged core dependency {} resolved outside its declared requirement {}",
                dependency.name,
                dependency.req,
            );
        }
    }
    Ok(())
}

fn oracle_warnings(root: &Path, graph: &Graph, lock: &[Identity]) -> Result<()> {
    let path = root.join("vendor/oracle/lock.json");
    if !path.exists() {
        eprintln!(
            "warning: vendor/oracle/lock.json is absent; byte-defining oracle dependency provenance is not recorded yet"
        );
        return Ok(());
    }
    let oracle: Json = serde_json::from_slice(&fs::read(&path)?)?;
    let expected: BTreeSet<_> = oracle["packages"]
        .as_array()
        .context("oracle lock missing packages")?
        .iter()
        .map(|p| {
            Ok(Identity {
                name: string(p, "name")?.into(),
                version: string(p, "version")?.into(),
                source: p["source"].as_str().map(str::to_owned),
            })
        })
        .collect::<Result<_>>()?;
    ensure!(
        !expected.is_empty(),
        "oracle lock records no byte-defining dependencies"
    );
    let actual = resolved_identities(graph, lock)?;
    let names = expected
        .iter()
        .chain(
            actual
                .iter()
                .filter(|p| p.name.starts_with("ark-") || BYTE_CRATES.contains(&p.name.as_str())),
        )
        .map(|p| p.name.as_str())
        .collect::<BTreeSet<_>>();
    for name in names {
        let ours: BTreeSet<_> = actual.iter().filter(|p| p.name == name).collect();
        let theirs: BTreeSet<_> = expected.iter().filter(|p| p.name == name).collect();
        if ours != theirs {
            eprintln!(
                "warning: byte-defining {name} differs from oracle: rtao {ours:?}; oracle {theirs:?}"
            );
        }
    }
    Ok(())
}

fn check_generated(root: &Path) -> Result<()> {
    fn rust_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            ensure!(
                !entry.file_type()?.is_symlink(),
                "generation input/output must not be a symlink: {}",
                path.display()
            );
            if path.is_dir() {
                rust_files(&path, files)?;
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    let catalog = root.join("crates/rtao-catalog/src");
    rust_files(&catalog, &mut files)?;
    let mut generated = Vec::new();
    fn generated_dirs(path: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if entry.file_name() == "generated" {
                    found.push(entry.path());
                } else if entry.file_name() != "target" {
                    generated_dirs(&entry.path(), found)?;
                }
            }
        }
        Ok(())
    }
    generated_dirs(&root.join("crates"), &mut generated)?;
    for directory in &generated {
        rust_files(directory, &mut files)?;
    }
    // M0 creates only a catalog skeleton. Require its exact empty Rust surface;
    // any actual item or generated module activates the M1 regeneration gate.
    let skeleton_only = files.len() == 1
        && files.first() == Some(&catalog.join("lib.rs"))
        && fs::read_to_string(catalog.join("lib.rs"))?
            .lines()
            .all(|line| {
                let line = line.trim();
                line.is_empty() || line.starts_with("//") || line == "#![deny(missing_docs)]"
            });
    if skeleton_only {
        println!("deps-check: catalog is an empty M0 skeleton; no generated Rust artifacts exist");
        return Ok(());
    }
    let before = files
        .iter()
        .map(|path| Ok((path.clone(), fs::read(path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let output = Command::new(std::env::current_exe()?)
        .current_dir(root)
        .args(["regen", "--check"])
        .output()?;
    ensure!(
        output.status.success(),
        "generated artifacts require xtask regen --check:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut after_files = Vec::new();
    rust_files(&catalog, &mut after_files)?;
    let mut after_dirs = Vec::new();
    generated_dirs(&root.join("crates"), &mut after_dirs)?;
    for directory in after_dirs {
        rust_files(&directory, &mut after_files)?;
    }
    let after = after_files
        .iter()
        .map(|path| Ok((path.clone(), fs::read(path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    ensure!(
        before == after,
        "regen --check modified, added or deleted generated Rust files"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str) -> Package {
        Package {
            name: name.into(),
            manifest: PathBuf::new(),
            dependencies: Vec::new(),
            features: BTreeMap::new(),
        }
    }

    fn dep(name: &str) -> Dependency {
        Dependency {
            name: name.into(),
            alias: name.into(),
            kind: "normal".into(),
            optional: false,
            default_features: false,
            features: Vec::new(),
            req: "*".into(),
        }
    }

    #[test]
    fn no_host_build_dependency_and_feature_leaks_fail() -> Result<()> {
        let profile = profile(
            "validator",
            "rtao",
            false,
            &["validator", "prometheus"],
            true,
            false,
        );
        let clean = "rtao v0.1.0|validator,prometheus\nsp-core v37.0.0|full_crypto\nreqwest v0.12.0|rustls-tls\nrustls v0.23.0|ring,std,tls12\n";
        check_graph(&profile, &parse_tree(clean)?)?;
        for leak in [
            "cmake v0.1.0|",
            "openssl-src v300.0.0|",
            "reqwest v0.12.0|blocking",
            "sp-core v37.0.0|std",
            "rtao-signer v0.1.0|extension",
            "rtao-signer v0.1.0|vault",
            "jsonrpsee v0.26.0|",
            "rtao-client v0.1.0|test-util",
            "rustls v0.23.0|aws_lc_rs",
        ] {
            ensure!(
                check_graph(&profile, &parse_tree(&format!("{clean}{leak}\n"))?).is_err(),
                "leak not rejected: {leak}"
            );
        }
        Ok(())
    }

    #[test]
    fn shipped_and_testkit_graphs_are_distinct() -> Result<()> {
        let graph = parse_tree("rtao v0.1.0|default\nrtao-client v0.1.0|test-util\n")?;
        ensure!(check_graph(&profile("default", "rtao", true, &[], false, false), &graph).is_err());
        let mut testkit = profile("testkit", "rtao-testkit", false, &[], false, true);
        testkit.shipped = false;
        check_graph(&testkit, &graph)?;
        let graph = parse_tree("rtao v0.1.0|subxt\nsubxt v0.51.1|\njsonrpsee v0.26.1|\n")?;
        ensure!(check_graph(&profile("default", "rtao", true, &[], false, false), &graph).is_err());
        check_graph(
            &profile("subxt", "rtao", false, &["subxt"], false, true),
            &graph,
        )?;
        Ok(())
    }

    #[test]
    fn btauth_sans_features_is_io_free() -> Result<()> {
        let profile = profile("btauth", "rtao-btauth", false, &[], false, false);
        for io in ["tokio", "axum", "reqwest", "hyper"] {
            ensure!(
                check_graph(
                    &profile,
                    &parse_tree(&format!("rtao-btauth v0.1.0|\n{io} v1.0.0|\n"))?
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn layers_defaults_and_test_seams_are_enforced_from_declarations() -> Result<()> {
        let workspace: DocumentMut = "[workspace]\n".parse()?;
        let mut core = package("rtao-core");
        core.dependencies.push(dep("bittensor-core"));
        let mut primitives = package("rtao-primitives");
        primitives.dependencies.push(dep("rtao-core"));
        check_declarations(&[core.clone(), primitives.clone()], &workspace)?;
        primitives.dependencies.push(dep("bittensor-core"));
        ensure!(check_declarations(&[core.clone(), primitives.clone()], &workspace).is_err());
        primitives.dependencies.pop();
        if let Some(dependency) = primitives.dependencies.first_mut() {
            dependency.default_features = true;
        }
        ensure!(check_declarations(&[core.clone(), primitives], &workspace).is_err());
        core.dependencies.push(dep("rtao"));
        ensure!(check_declarations(&[core, package("rtao")], &workspace).is_err());
        let mut facade = package("rtao");
        let mut seam = dep("rtao-client");
        seam.features.push("test-util".into());
        facade.dependencies.push(seam);
        ensure!(check_declarations(&[facade.clone(), package("rtao-client")], &workspace).is_err());
        if let Some(seam) = facade.dependencies.first_mut() {
            seam.features.clear();
        }
        facade
            .features
            .insert("innocent".into(), vec!["rtao-client?/test-util".into()]);
        ensure!(check_declarations(&[facade.clone(), package("rtao-client")], &workspace).is_err());
        if let Some(seam) = facade.dependencies.first_mut() {
            seam.kind = "dev".into();
        }
        check_declarations(&[facade, package("rtao-client")], &workspace)?;
        Ok(())
    }

    #[test]
    fn signer_offload_is_limited_to_device_and_vault_cpu_work() -> Result<()> {
        let workspace: DocumentMut = "[workspace]\n".parse()?;
        let mut signer = package("rtao-signer");
        signer.dependencies.push(dep("rtao-core"));
        for feature in ["ledger", "vault"] {
            signer.features.clear();
            signer
                .features
                .insert(feature.into(), vec!["rtao-core/offload".into()]);
            check_declarations(std::slice::from_ref(&signer), &workspace)?;
        }
        for feature in ["default", "extension", "test-util"] {
            signer.features.clear();
            signer
                .features
                .insert(feature.into(), vec!["rtao-core/offload".into()]);
            ensure!(check_declarations(std::slice::from_ref(&signer), &workspace).is_err());
        }
        Ok(())
    }

    #[test]
    fn portable_staged_copies_are_allowed_but_unrelated_runtime_edges_fail() -> Result<()> {
        let workspace: DocumentMut = "[workspace]\n".parse()?;
        let mut ext = package("rtao-core-ext");
        for name in ["base64", "zeroize"] {
            ext.dependencies.push(dep(name));
        }
        let mut bench = dep("criterion");
        bench.kind = "dev".into();
        ext.dependencies.push(bench);
        check_declarations(std::slice::from_ref(&ext), &workspace)?;
        for name in [
            "tokio",
            "reqwest",
            "unicode-general-category",
            "blake2",
            "unrelated",
        ] {
            let mut changed = ext.clone();
            changed.dependencies.push(dep(name));
            ensure!(
                check_declarations(&[changed], &workspace).is_err(),
                "accepted unsupported staged runtime dependency {name}"
            );
        }
        if let Some(bench) = ext.dependencies.last_mut() {
            bench.kind = "build".into();
        }
        ensure!(check_declarations(&[ext], &workspace).is_err());
        Ok(())
    }

    #[test]
    fn websocket_feature_gates_follow_transitive_aliases_and_cycles() -> Result<()> {
        let mut signer = package("rtao-signer");
        let mut ws = dep("tokio-tungstenite");
        ws.alias = "ws".into();
        ws.optional = true;
        signer
            .features
            .insert("extension".into(), vec!["dep:ws".into()]);
        signer
            .features
            .insert("vault".into(), vec!["extension".into()]);
        signer
            .features
            .insert("alias".into(), vec!["vault".into(), "cycle".into()]);
        signer.features.insert("cycle".into(), vec!["alias".into()]);
        check_optional_gate(&signer, &ws, &["extension", "vault"])?;
        signer.features.insert("leak".into(), vec!["dep:ws".into()]);
        ensure!(check_optional_gate(&signer, &ws, &["extension", "vault"]).is_err());
        signer.features.remove("leak");
        ws.optional = false;
        ensure!(check_optional_gate(&signer, &ws, &["extension", "vault"]).is_err());
        Ok(())
    }

    #[test]
    fn exact_bls_source_and_recorded_dormant_copy_are_required() -> Result<()> {
        let pins: DocumentMut = "[monorepo]\nurl='https://github.com/RaoFoundation/subtensor.git'\n[core]\ntag='v478'\ncommit='1111111111111111111111111111111111111111'\n".parse()?;
        let source = bls_source(&pins)?;
        let good =
            format!("version=4\n[[package]]\nname='w3f-bls'\nversion='0.1.3'\nsource='{source}'\n");
        check_bls(&good.parse()?, &pins)?;
        let dormant = format!(
            "[[package]]\nname='w3f-bls'\nversion='0.1.9'\nsource='{}'\nchecksum='{}'\n",
            rtao_core::dependency_policy::DORMANT_BLS_SOURCE,
            rtao_core::dependency_policy::DORMANT_BLS_CHECKSUM
        );
        check_bls(&format!("{good}{dormant}").parse()?, &pins)?;
        for source in [
            "registry+https://github.com/rust-lang/crates.io-index",
            "git+https://github.com/RaoFoundation/subtensor.git?tag=v477#1111111111111111111111111111111111111111",
            "git+https://github.com/RaoFoundation/subtensor.git?tag=v478#other",
            "git+https://github.com/other/subtensor.git?tag=v478#1111111111111111111111111111111111111111",
        ] {
            let bad = good.replace(&bls_source(&pins)?, source).parse()?;
            ensure!(check_bls(&bad, &pins).is_err());
        }
        for bad in [
            format!("{good}{dormant}{dormant}"),
            format!(
                "{good}{}",
                dormant.replace("checksum=", "untrusted_checksum=")
            ),
            format!("{good}{}", dormant.replace("0.1.9", "0.1.10")),
            format!("{good}{}", good.replace("version=4\n", "")),
            "version=4\npackage=[]\n".into(),
        ] {
            ensure!(check_bls(&bad.parse()?, &pins).is_err());
        }
        Ok(())
    }

    #[test]
    fn repeated_tree_nodes_merge_features_but_distinct_sources_do_not() -> Result<()> {
        let graph = parse_tree(
            "w3f-bls v0.1.3 (https://github.com/RaoFoundation/subtensor.git?tag=v478#abcdef01)|std\nw3f-bls v0.1.3 (https://github.com/RaoFoundation/subtensor.git?tag=v478#abcdef01)|std (*)\nsha2 v0.10.9|std\nsha2 v0.10.9|default (*)\n",
        )?;
        ensure!(graph.len() == 2);
        let lock = vec![
            Identity {
                name: "w3f-bls".into(),
                version: "0.1.3".into(),
                source: Some(
                    "git+https://github.com/RaoFoundation/subtensor.git?tag=v478#abcdef0123456789"
                        .into(),
                ),
            },
            Identity {
                name: "sha2".into(),
                version: "0.10.9".into(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            },
        ];
        ensure!(resolved_identities(&graph, &lock)?.len() == 2);
        let wrong = vec![Identity {
            name: "w3f-bls".into(),
            version: "0.1.3".into(),
            source: Some(
                "git+https://github.com/RaoFoundation/subtensor.git?tag=v477#abcdef0123456789"
                    .into(),
            ),
        }];
        ensure!(resolved_identities(&graph, &wrong).is_err());
        Ok(())
    }

    #[test]
    fn non_https_sources_remain_distinct_and_implicit_registry_is_unambiguous() -> Result<()> {
        let graph = parse_tree(
            "shared v1.0.0 (ssh://git@example.test/shared?rev=abcdef01#abcdef01)|\nshared v1.0.0 (http://example.test/shared?rev=abcdef02#abcdef02)|\n",
        )?;
        ensure!(graph.len() == 2);
        let lock: Vec<_> = graph
            .keys()
            .map(|p| Identity {
                name: p.name.clone(),
                version: p.version.clone(),
                source: p.source.as_ref().map(|s| format!("git+{s}00000000")),
            })
            .collect();
        ensure!(resolved_identities(&graph, &lock)?.len() == 2);
        let registry = parse_tree("shared v1.0.0|\n")?;
        let lock = vec![
            Identity {
                name: "shared".into(),
                version: "1.0.0".into(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            },
            Identity {
                name: "shared".into(),
                version: "1.0.0".into(),
                source: Some("registry+https://other.example/index".into()),
            },
        ];
        ensure!(resolved_identities(&registry, &lock).is_err());
        Ok(())
    }

    #[test]
    fn core_closure_follows_deduplicated_edges_and_excludes_other_roots() -> Result<()> {
        // sha2's expanded children appear before core and its repeated node is
        // abbreviated inside core. The depth graph must still retain digest,
        // while excluding the seam's separate build-only dependency.
        let tree = "0rtao-core v0.1.0|\n1other v0.1.0|\n2sha2 v0.10.9|std\n3digest v0.10.7|\n1bittensor-core v0.1.0 (https://github.com/RaoFoundation/subtensor.git?tag=v478#abcdef01)|\n2sha2 v0.10.9|std (*)\n2w3f-bls v0.1.3 (https://github.com/RaoFoundation/subtensor.git?tag=v478#abcdef01)|std\n1toml_edit v0.23.0|\n2build-only v0.1.0|\n";
        let graph = parse_tree_with_root(tree, true, Some("bittensor-core"))?;
        let names: BTreeSet<_> = graph.keys().map(|p| p.name.as_str()).collect();
        ensure!(names == BTreeSet::from(["bittensor-core", "sha2", "digest", "w3f-bls"]));
        ensure!(parse_tree_with_root("0rtao-core v0.1.0|\n2sha2 v0.10.9|\n", true, None).is_err());
        Ok(())
    }

    #[test]
    fn staged_dependencies_cannot_introduce_a_second_version_or_source() -> Result<()> {
        let mut ext = package("rtao-core-ext");
        ext.dependencies.push(dep("sha2"));
        let graph = parse_tree("bittensor-core v0.1.0|\nsha2 v0.10.9|std\n")?;
        let mut lock = vec![
            Identity {
                name: "bittensor-core".into(),
                version: "0.1.0".into(),
                source: None,
            },
            Identity {
                name: "sha2".into(),
                version: "0.10.9".into(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            },
        ];
        check_core_ext_copies(std::slice::from_ref(&ext), &graph, &graph, &lock)?;
        lock.push(Identity {
            name: "sha2".into(),
            version: "0.11.0".into(),
            source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
        });
        // An unrelated tooling version in the lock does not become a staged
        // copy. Activating that version in the staged graph fails.
        check_core_ext_copies(std::slice::from_ref(&ext), &graph, &graph, &lock)?;
        let duplicate = parse_tree("bittensor-core v0.1.0|\nsha2 v0.10.9|std\nsha2 v0.11.0|std\n")?;
        ensure!(
            check_core_ext_copies(std::slice::from_ref(&ext), &graph, &duplicate, &lock).is_err()
        );
        let changed = parse_tree("bittensor-core v0.1.0|\nsha2 v0.11.0|std\n")?;
        ensure!(
            check_core_ext_copies(std::slice::from_ref(&ext), &graph, &changed, &lock).is_err()
        );
        lock.pop();
        lock.push(Identity {
            name: "sha2".into(),
            version: "0.10.9".into(),
            source: Some("git+https://example.invalid/sha2#abcdef01".into()),
        });
        check_core_ext_copies(std::slice::from_ref(&ext), &graph, &graph, &lock)?;
        let forked = parse_tree(
            "bittensor-core v0.1.0|\nsha2 v0.10.9 (https://example.invalid/sha2#abcdef01)|std\n",
        )?;
        ensure!(check_core_ext_copies(std::slice::from_ref(&ext), &graph, &forked, &lock).is_err());
        lock.pop();
        if let Some(dependency) = ext.dependencies.first_mut() {
            dependency.req = "=0.10.8".into();
        }
        ensure!(check_core_ext_copies(&[ext], &graph, &graph, &lock).is_err());
        Ok(())
    }

    #[test]
    fn approved_copies_require_exact_core_version_source_and_reachability() -> Result<()> {
        let mut ext = package("rtao-core-ext");
        for (name, req) in [("base64", "0.22"), ("zeroize", "1")] {
            let mut dependency = dep(name);
            dependency.req = req.into();
            ext.dependencies.push(dependency);
        }
        let clean = "base64 v0.22.1|\nzeroize v1.9.1|\n";
        let core = parse_tree(clean)?;
        let mut lock: Vec<_> = core
            .keys()
            .map(|package| Identity {
                name: package.name.clone(),
                version: package.version.clone(),
                source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            })
            .collect();
        // Host and tooling can lock another base64 without copying it into the
        // portable staged graph.
        lock.push(Identity {
            name: "base64".into(),
            version: "0.23.1".into(),
            source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
        });
        check_core_ext_copies(std::slice::from_ref(&ext), &core, &core, &lock)?;
        for changed in [
            clean.replace("base64 v0.22.1", "base64 v0.23.1"),
            clean.replace("base64 v0.22.1|\n", ""),
        ] {
            ensure!(
                check_core_ext_copies(
                    std::slice::from_ref(&ext),
                    &core,
                    &parse_tree(&changed)?,
                    &lock
                )
                .is_err()
            );
        }
        lock.push(Identity {
            name: "base64".into(),
            version: "0.22.1".into(),
            source: Some("git+https://example.invalid/base64?rev=11111111#1111111111111111111111111111111111111111".into()),
        });
        let forked = clean.replace(
            "base64 v0.22.1",
            "base64 v0.22.1 (https://example.invalid/base64?rev=11111111#11111111)",
        );
        ensure!(
            check_core_ext_copies(
                std::slice::from_ref(&ext),
                &core,
                &parse_tree(&forked)?,
                &lock
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn both_no_host_profiles_are_selected_separately() -> Result<()> {
        let mut facade = package("rtao");
        for feature in ["validator", "prometheus", "miner", "redis", "subxt", "full"] {
            facade.features.insert(feature.into(), Vec::new());
        }
        let selected = profiles(&[facade], true)?;
        ensure!(
            selected
                .iter()
                .any(|p| p.package == "rtao" && p.features == ["validator", "prometheus"])
        );
        ensure!(
            selected
                .iter()
                .any(|p| p.package == "rtao" && p.features == ["miner", "redis"])
        );
        ensure!(selected.iter().all(|p| !p.defaults && p.no_host));
        Ok(())
    }
}
