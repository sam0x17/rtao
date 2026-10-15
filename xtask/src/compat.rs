//! Generate the release compatibility matrix from pins and recorded validation.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use blake2::{Blake2b, digest::consts::U32};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, TableLike};

const EVIDENCE: &str = "docs/compat/validation.json";
const METADATA_INDEX: &str = "vendor/metadata/finney/index.toml";
const MAX_REPORT_BYTES: usize = 1024 * 1024;
const HASH_KIND: &str = "blake2b-256-raw";
const BUILD_WEIGHT: &str = "docs/compat/build-weight";
const BUILD_COMMAND: &str = "cargo tree --locked --target <target> -p <package> --no-default-features --features <features> --edges normal,build --prefix none --format {p}";
const BUILD_METHOD: &str = "distinct package name/version/source identities; repeated-node suffix removed; local root normalized to $WORKSPACE; dev edges excluded";
const BUILD_FEATURE_COMMAND: &str = "cargo tree --locked --target <target> -p <package> --no-default-features --features <features> --edges features --invert bittensor-core";
const BUILD_PROFILES: &[(&str, &str, &[&str])] = &[
    ("core-no-host", "rtao-core", &[]),
    ("validator", "rtao", &["validator"]),
    ("miner", "rtao", &["miner"]),
    ("owner", "rtao", &["owner"]),
];

#[derive(Debug, PartialEq, Eq)]
struct Options {
    check: bool,
    milestone: Option<String>,
    markdown: String,
    json: String,
    measure_build_weight: bool,
    build_target: Option<String>,
}

fn options(args: Vec<String>) -> Result<Options> {
    let mut options = Options {
        check: false,
        milestone: None,
        markdown: "COMPAT.md".into(),
        json: "compat.json".into(),
        measure_build_weight: false,
        build_target: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => options.check = true,
            "--measure-build-weight" => options.measure_build_weight = true,
            "--build-target" => {
                options.build_target = Some(
                    args.next()
                        .context("--build-target requires a Rust target triple")?,
                )
            }
            "--write" => options.markdown = args.next().context("--write requires a path")?,
            "--json" => options.json = args.next().context("--json requires a path")?,
            "--milestone" => {
                let milestone = args.next().context("--milestone requires M0 through M10")?;
                ensure!(
                    (0..=10).any(|index| milestone == format!("M{index}")),
                    "--milestone requires M0 through M10"
                );
                options.milestone = Some(milestone);
            }
            _ => bail!("unknown compat option {arg:?}"),
        }
    }
    relative_path(&options.markdown)?;
    relative_path(&options.json)?;
    ensure!(
        options.markdown != options.json,
        "compat outputs must differ"
    );
    ensure!(
        !options.check || !options.measure_build_weight,
        "--check cannot rewrite build-weight evidence"
    );
    ensure!(
        options.build_target.is_none() || options.measure_build_weight,
        "--build-target requires --measure-build-weight"
    );
    if let Some(target) = &options.build_target {
        validate_target(target)?;
    }
    Ok(options)
}

fn validate_target(target: &str) -> Result<()> {
    ensure!(
        target.contains('-')
            && target
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "invalid build target triple"
    );
    Ok(())
}

fn capture(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    ensure!(
        output.status.success(),
        "{program} {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        output.stdout.len() <= 16 * 1024 * 1024,
        "build-weight command output exceeded limit"
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn build_inputs(root: &Path) -> Result<Json> {
    let mut paths = vec![
        "Cargo.toml".to_owned(),
        "Cargo.lock".into(),
        "upstream.toml".into(),
    ];
    for entry in fs::read_dir(root.join("crates"))? {
        let entry = entry?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "crate manifest path contains a symlink"
        );
        if entry.file_type()?.is_dir() && entry.path().join("Cargo.toml").is_file() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("crate name is not UTF-8"))?;
            paths.push(format!("crates/{name}/Cargo.toml"));
        }
    }
    paths.push("xtask/Cargo.toml".into());
    paths.sort();
    let mut hashes = BTreeMap::new();
    for path in paths {
        hashes.insert(path.clone(), sha256(&read(root, &path, 16 * 1024 * 1024)?));
    }
    Ok(json!(hashes))
}

fn package_identities(tree: &str, root: &Path) -> Result<BTreeSet<String>> {
    let root = root.to_str().context("workspace path is not UTF-8")?;
    let mut packages = BTreeSet::new();
    for line in tree.lines() {
        let line = line.trim();
        ensure!(!line.is_empty(), "empty cargo tree package line");
        let line = line.strip_suffix(" (*)").unwrap_or(line);
        // Local paths do not identify a different graph in another checkout.
        let identity = line.replace(root, "$WORKSPACE");
        let mut fields = identity.split_whitespace();
        ensure!(
            fields.next().is_some()
                && fields.next().is_some_and(|version| version.starts_with('v')
                    && semver::Version::parse(&version[1..]).is_ok()),
            "invalid cargo tree package identity: {identity}"
        );
        packages.insert(identity);
    }
    ensure!(
        !packages.is_empty(),
        "cargo tree measured no package identities"
    );
    Ok(packages)
}

fn build_machine(root: &Path) -> Result<String> {
    if cfg!(target_os = "macos") {
        return capture(root, "sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    let cpu = fs::read_to_string("/proc/cpuinfo")?;
    cpu.lines()
        .find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, value)| value.trim().to_owned())
        })
        .context("build-weight CPU identity unavailable")
}

fn measure_build_weight(root: &Path, target: Option<&str>) -> Result<()> {
    let compiler = capture(root, "rustc", &["-Vv"])?;
    let host = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .context("compiler host missing")?;
    let target = target.unwrap_or(host);
    validate_target(target)?;
    let before = build_inputs(root)?;
    let mut profiles = Vec::new();
    for (profile, package, features) in BUILD_PROFILES {
        let mut arguments = vec![
            "tree",
            "--locked",
            "--target",
            target,
            "-p",
            package,
            "--no-default-features",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ];
        let joined = features.join(",");
        if !joined.is_empty() {
            arguments.extend(["--features", &joined]);
        }
        let packages = package_identities(&capture(root, "cargo", &arguments)?, root)?;
        let mut feature_arguments = vec![
            "tree",
            "--locked",
            "--target",
            target,
            "-p",
            package,
            "--no-default-features",
            "--edges",
            "features",
            "--invert",
            "bittensor-core",
        ];
        if !joined.is_empty() {
            feature_arguments.extend(["--features", &joined]);
        }
        let feature_graph = capture(root, "cargo", &feature_arguments)?;
        let host_enabled = feature_graph.contains("bittensor-core feature \"host\"");
        if matches!(*profile, "core-no-host" | "validator" | "miner") {
            ensure!(!host_enabled, "{profile} activated bittensor-core/host");
        }
        profiles.push(json!({"profile": profile, "package": package, "features": features,
            "default_features": false, "normal_build_packages": packages.len(), "package_identities": packages,
            "core_host_enabled": host_enabled, "feature_graph_sha256": sha256(feature_graph.as_bytes())}));
    }
    ensure!(
        before == build_inputs(root)?,
        "dependency inputs changed during build-weight measurement"
    );
    let manifest = toml(root, "Cargo.toml")?;
    let report = canonical(
        &json!({"schema": 1, "rtao_version": manifest["workspace"]["package"]["version"].as_str().context("version missing")?,
        "target": target, "host": host, "compiler": compiler, "cargo": capture(root,"cargo", &["-V"] )?,
        "machine": build_machine(root)?, "measured_unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "source": {"head_commit": capture(root,"git", &["rev-parse", "HEAD"] )?, "dirty": !capture(root,"git", &["status", "--porcelain", "--untracked-files=normal"] )?.is_empty(),
            "dependency_inputs_sha256": sha256(&serde_json::to_vec(&canonical(&before))?), "dependency_inputs": before},
        "method": {"command": BUILD_COMMAND, "count": BUILD_METHOD, "host_feature_command": BUILD_FEATURE_COMMAND},
        "profiles": profiles}),
    );
    fs::create_dir_all(root.join(BUILD_WEIGHT))?;
    let relative = format!("{BUILD_WEIGHT}/{target}.json");
    let path = checked_path(root, &relative, true)?;
    crate::pin::atomic_write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&report)?).as_bytes(),
    )?;
    println!("Measured target/profile build weight at {relative}");
    Ok(())
}

fn validate_build_weight(report: &Json, version: &str, input: &Json) -> Result<()> {
    ensure!(
        report["method"]
            == json!({"command": BUILD_COMMAND, "count": BUILD_METHOD, "host_feature_command": BUILD_FEATURE_COMMAND}),
        "build-weight measurement method differs"
    );
    ensure!(
        report["schema"] == 1 && report["rtao_version"] == version,
        "unsupported or stale build-weight release record"
    );
    validate_target(json_text(report, "target")?)?;
    validate_target(json_text(report, "host")?)?;
    ensure!(
        report["source"]["dependency_inputs"] == *input
            && json_text(&report["source"], "dependency_inputs_sha256")?
                == sha256(&serde_json::to_vec(&canonical(input))?),
        "build-weight dependency inputs changed; remeasure for this release"
    );
    ensure!(
        canonical_hash(json_text(&report["source"], "head_commit")?, "", 40)
            && report["source"]["dirty"].is_boolean(),
        "invalid build-weight source provenance"
    );
    ensure!(
        report["measured_unix_seconds"]
            .as_u64()
            .is_some_and(|v| v > 0),
        "missing build-weight measurement time"
    );
    for field in ["compiler", "cargo", "machine"] {
        ensure!(
            !json_text(report, field)?.is_empty(),
            "empty build-weight context"
        );
    }
    let host = json_text(report, "host")?;
    ensure!(
        json_text(report, "compiler")?
            .lines()
            .any(|line| line == format!("host: {host}")),
        "build-weight compiler host differs"
    );
    let rows = members(report, "profiles")?;
    ensure!(
        rows.len() == BUILD_PROFILES.len(),
        "build-weight profile coverage incomplete"
    );
    let mut seen = BTreeSet::new();
    for row in rows {
        let profile = json_text(row, "profile")?;
        ensure!(seen.insert(profile), "duplicate measured profile");
        let (_, package, features) = BUILD_PROFILES
            .iter()
            .find(|(name, _, _)| *name == profile)
            .context("unknown measured profile")?;
        ensure!(
            row["package"] == *package
                && row["features"] == json!(features)
                && row["default_features"] == false,
            "measured profile selection differs"
        );
        let packages = members(row, "package_identities")?;
        let unique = packages
            .iter()
            .map(|item| item.as_str().context("package identity must be a string"))
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            !unique.is_empty()
                && unique.len() == packages.len()
                && row["normal_build_packages"].as_u64() == Some(u64::try_from(unique.len())?),
            "measured package count differs from identity inventory"
        );
        ensure!(
            row["core_host_enabled"].is_boolean()
                && canonical_hash(json_text(row, "feature_graph_sha256")?, "", 64),
            "missing measured feature provenance"
        );
        if matches!(profile, "core-no-host" | "validator" | "miner") {
            ensure!(
                row["core_host_enabled"] == false,
                "no-host profile activates host"
            );
        }
    }
    Ok(())
}

fn build_weight(root: &Path, version: &str) -> Result<Vec<Json>> {
    let directory = root.join(BUILD_WEIGHT);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    ensure!(
        fs::symlink_metadata(&directory)?.is_dir()
            && !fs::symlink_metadata(&directory)?.file_type().is_symlink(),
        "build-weight directory must be an ordinary directory"
    );
    let input = build_inputs(root)?;
    let mut reports = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let filename = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("build-weight filename is not UTF-8"))?;
        ensure!(filename.ends_with(".json"), "unexpected build-weight file");
        let path = format!("{BUILD_WEIGHT}/{filename}");
        let bytes = read(root, &path, MAX_REPORT_BYTES)?;
        let record: Json = serde_json::from_slice(&bytes)?;
        validate_build_weight(&record, version, &input)?;
        ensure!(
            filename == format!("{}.json", json_text(&record, "target")?),
            "build-weight filename/target differ"
        );
        reports.push(json!({"path":path,"sha256":sha256(&bytes),"record":record}));
    }
    reports.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(reports)
}

fn relative_path(path: &str) -> Result<&Path> {
    ensure!(
        path.split(['/', '\\'])
            .all(|part| !part.is_empty() && part != "." && part != ".."),
        "compat paths must not contain empty or dot components"
    );
    let path = Path::new(path);
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "compat paths must be relative paths without dot components"
    );
    Ok(path)
}

fn checked_path(root: &Path, relative: &str, allow_missing_file: bool) -> Result<PathBuf> {
    let relative = relative_path(relative)?;
    let mut path = root.to_owned();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "compat path contains a symlink"
                );
                if components.peek().is_some() {
                    ensure!(metadata.is_dir(), "compat parent is not a directory");
                } else {
                    ensure!(
                        metadata.is_file(),
                        "compat input or output is not a regular file"
                    );
                }
            }
            Err(error)
                if allow_missing_file
                    && components.peek().is_none()
                    && error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
        }
    }
    Ok(path)
}

fn read(root: &Path, path: &str, maximum: usize) -> Result<Vec<u8>> {
    let path = checked_path(root, path, false)?;
    ensure!(
        fs::metadata(&path)?.len() <= maximum as u64,
        "compat input exceeds its size limit"
    );
    fs::read(&path).with_context(|| format!("read {}", path.display()))
}

fn toml(root: &Path, path: &str) -> Result<DocumentMut> {
    String::from_utf8(read(root, path, 16 * 1024 * 1024)?)?
        .parse()
        .with_context(|| format!("parse {path}"))
}

fn text<'a>(table: &'a dyn TableLike, key: &str) -> Result<&'a str> {
    table
        .get(key)
        .and_then(Item::as_str)
        .with_context(|| format!("missing string {key}"))
}

fn canonical_hash(value: &str, prefix: &str, length: usize) -> bool {
    value.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == length
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn source(pins: &DocumentMut, name: &str) -> Result<Json> {
    let table = pins
        .get(name)
        .and_then(Item::as_table_like)
        .with_context(|| format!("missing {name} pin"))?;
    let tag = text(table, "tag")?;
    ensure!(
        tag.strip_prefix('v')
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())),
        "invalid {name} tag"
    );
    let commit = text(table, "commit")?;
    ensure!(canonical_hash(commit, "", 40), "invalid {name} commit");
    Ok(json!({"tag": tag, "commit": commit}))
}

fn core_input(pins: &DocumentMut, dependency: &Item) -> Result<Json> {
    let released = source(pins, "core")?;
    let repository = pins
        .get("monorepo")
        .and_then(|section| section.get("url"))
        .and_then(Item::as_str)
        .context("monorepo source missing")?;
    ensure!(
        dependency.get("git").and_then(Item::as_str) == Some(repository),
        "core repository differs from workspace dependency"
    );
    if let Some(commit) = crate::core_canary::commit(pins)? {
        ensure!(
            dependency.get("rev").and_then(Item::as_str) == Some(commit)
                && ["tag", "branch", "path"]
                    .iter()
                    .all(|selector| dependency.get(selector).is_none()),
            "core canary revision differs from workspace dependency"
        );
        return Ok(json!({
            "tag": null,
            "branch": "main",
            "commit": commit,
            "release_pin": released,
        }));
    }
    ensure!(
        dependency.get("tag").and_then(Item::as_str) == released["tag"].as_str()
            && ["rev", "branch", "path"]
                .iter()
                .all(|selector| dependency.get(selector).is_none()),
        "core tag differs from workspace dependency"
    );
    Ok(released)
}

// Sorting recursively keeps output identical when serde_json's preserve_order
// feature is enabled elsewhere in a workspace invocation.
fn canonical(value: &Json) -> Json {
    match value {
        Json::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_unstable();
            Json::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&object[key])))
                    .collect(),
            )
        }
        Json::Array(values) => Json::Array(values.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn metadata(root: &Path, pins: &DocumentMut) -> Result<Vec<Json>> {
    crate::metadata_pin::verify(root, pins)?;
    let index = toml(root, METADATA_INDEX)?;
    let network = index
        .get("network")
        .and_then(Item::as_str)
        .context("metadata network missing")?;
    let rows = index
        .get("runtimes")
        .and_then(Item::as_array_of_tables)
        .context("metadata runtimes missing")?;
    let mut output = Vec::new();
    for row in rows {
        let spec = row
            .get("spec")
            .and_then(Item::as_integer)
            .context("metadata spec missing")?;
        let mut record = json!({
            "network": network,
            "spec_version": spec,
            "genesis": text(row, "genesis")?,
            "code_hash": text(row, "code_hash")?,
            "tag": text(row, "tag")?,
            "commit": text(row, "commit")?,
            "finalized_head": text(row, "finalized_head")?,
            "endpoint": text(row, "endpoint")?,
        });
        for version in [15, 16] {
            let key = format!("v{version}");
            let blob = row
                .get(&key)
                .and_then(Item::as_table_like)
                .context("metadata blob missing")?;
            let path = format!("vendor/metadata/{network}/{}", text(blob, "path")?);
            let bytes = read(root, &path, 16 * 1024 * 1024)?;
            record[&key] = json!({
                "path": path,
                "bytes": bytes.len(),
                "sha256": sha256(&bytes),
                "metadata_hash": {
                    "kind": HASH_KIND,
                    "value": format!("0x{:x}", Blake2b::<U32>::digest(&bytes)),
                },
            });
        }
        output.push(record);
    }
    output.sort_by_key(|row| row["spec_version"].as_u64());
    Ok(output)
}

fn inputs(root: &Path) -> Result<Json> {
    let manifest = toml(root, "Cargo.toml")?;
    let version = manifest["workspace"]["package"]["version"]
        .as_str()
        .context("workspace version missing")?;
    semver::Version::parse(version).context("invalid workspace version")?;
    let pins = toml(root, "upstream.toml")?;
    let dependency = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.get("bittensor-core"))
        .context("core workspace dependency missing")?;
    let core = core_input(&pins, dependency)?;
    let mut parity = source(&pins, "parity")?;
    let source_version = pins["parity"]["python_package_version"]
        .as_str()
        .context("Python source version missing")?;
    let project_path = "vendor/upstream/src/sdk/python/pyproject.toml";
    let project_bytes = read(root, project_path, MAX_REPORT_BYTES)?;
    let artifact = pins["artifacts"]["sdk/python/pyproject.toml"]
        .as_table_like()
        .context("Python project artifact pin missing")?;
    ensure!(
        text(artifact, "to")? == project_path,
        "Python project artifact path differs"
    );
    ensure!(
        text(artifact, "sha256")? == sha256(&project_bytes),
        "Python project artifact checksum differs"
    );
    let project: DocumentMut = String::from_utf8(project_bytes)?.parse()?;
    ensure!(
        project["project"]["version"].as_str() == Some(source_version),
        "Python source version differs from pinned project"
    );
    parity["python_source_version"] = json!(source_version);
    // A source version alone proves no PyPI release from this exact commit.
    parity["pypi_release"] = Json::Null;
    let mut localnet =
        json!({"image": pins["localnet"]["image"].as_str().context("localnet image missing")?});
    for name in ["pinned", "mainnet", "testnet", "devnet", "main"] {
        let image = pins["localnet"][name]
            .as_table_like()
            .with_context(|| format!("localnet {name} missing"))?;
        let digest = text(image, "digest")?;
        ensure!(
            canonical_hash(digest, "sha256:", 64),
            "invalid localnet {name} digest"
        );
        localnet[name] = json!({"tag": text(image, "tag")?, "digest": digest});
    }
    Ok(canonical(&json!({
        "rtao_version": version,
        "core": core,
        "parity": parity,
        "metadata_pins": metadata(root, &pins)?,
        "localnet": localnet,
        "btauth": {"protocol": rtao_btauth::PROTOCOL, "version": rtao_btauth::VERSION},
        "interop": {
            "package": "bittensor",
            "tag": parity["tag"],
            "commit": parity["commit"],
            "source_version": source_version,
        },
    })))
}

fn json_text<'a>(value: &'a Json, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .with_context(|| format!("missing evidence string {key}"))
}

fn members<'a>(value: &'a Json, key: &str) -> Result<&'a Vec<Json>> {
    value
        .get(key)
        .and_then(Json::as_array)
        .with_context(|| format!("missing evidence array {key}"))
}

fn spec(value: &Json) -> Result<u32> {
    let value = value
        .get("spec_version")
        .and_then(Json::as_u64)
        .context("evidence spec must be u32")?;
    u32::try_from(value).context("evidence spec must be u32")
}

fn runtime_identity(pin: &Json, version: &str) -> Json {
    json!({
        "network": pin["network"],
        "spec_version": pin["spec_version"],
        "genesis": pin["genesis"],
        "metadata_hash": pin[version]["metadata_hash"],
    })
}

fn candidates(inputs: &Json) -> Result<Vec<Json>> {
    Ok(members(inputs, "metadata_pins")?
        .iter()
        .flat_map(|pin| [runtime_identity(pin, "v15"), runtime_identity(pin, "v16")])
        .collect())
}

fn image_digests(inputs: &Json) -> Result<BTreeSet<String>> {
    let images = inputs["localnet"]
        .as_object()
        .context("localnet inputs missing")?;
    images
        .values()
        .filter(|value| value.is_object())
        .map(|value| Ok(json_text(value, "digest")?.to_owned()))
        .collect()
}

struct Validation {
    range: Json,
    runtimes: Vec<Json>,
    reports: Vec<Json>,
}

impl Validation {
    fn absent() -> Self {
        Self {
            range: Json::Null,
            runtimes: Vec::new(),
            reports: Vec::new(),
        }
    }
}

fn validate(
    inputs: &Json,
    evidence: &Json,
    mut read_report: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Validation> {
    ensure!(
        evidence["schema"] == 1,
        "unsupported compatibility evidence schema"
    );
    let input_hash = sha256(&serde_json::to_vec(&canonical(inputs))?);
    ensure!(
        json_text(evidence, "inputs_sha256")? == input_hash,
        "compatibility evidence has stale inputs"
    );
    let range = &evidence["validated_spec_range"];
    let minimum = range["min"]
        .as_u64()
        .context("validation minimum spec missing")?;
    let maximum = range["max"]
        .as_u64()
        .context("validation maximum spec missing")?;
    ensure!(
        minimum <= maximum && maximum <= u64::from(u32::MAX) && maximum - minimum < 4096,
        "invalid validation spec range"
    );
    let runtimes = members(evidence, "runtimes")?.clone();
    ensure!(
        !runtimes.is_empty(),
        "validated range has no runtime identities"
    );
    let available = candidates(inputs)?;
    let mut seen = BTreeSet::new();
    let mut specs = BTreeSet::new();
    for runtime in &runtimes {
        ensure!(
            available.contains(runtime),
            "evidence runtime is absent from immutable metadata pins"
        );
        let runtime_spec = u64::from(spec(runtime)?);
        ensure!(
            (minimum..=maximum).contains(&runtime_spec),
            "evidence runtime is outside validated range"
        );
        ensure!(
            seen.insert(serde_json::to_string(&canonical(runtime))?),
            "duplicate evidence runtime"
        );
        specs.insert(runtime_spec);
    }
    ensure!(
        specs == (minimum..=maximum).collect(),
        "validated range has a missing spec identity"
    );
    let images = image_digests(inputs)?;
    let mut coverage = BTreeSet::new();
    let mut drift = BTreeSet::new();
    let references = members(evidence, "reports")?;
    let mut report_paths = BTreeSet::new();
    for reference in references {
        let path = json_text(reference, "path")?;
        relative_path(path)?;
        ensure!(
            path.starts_with("docs/compat/reports/") && path.ends_with(".json"),
            "compat report must be under docs/compat/reports"
        );
        ensure!(
            report_paths.insert(path),
            "duplicate compatibility report path"
        );
        let checksum = json_text(reference, "sha256")?;
        ensure!(
            canonical_hash(checksum, "", 64),
            "invalid compatibility report checksum"
        );
        let bytes = read_report(path)?;
        ensure!(
            bytes.len() <= MAX_REPORT_BYTES && sha256(&bytes) == checksum,
            "compatibility report checksum differs"
        );
        let report: Json = serde_json::from_slice(&bytes).context("parse compatibility report")?;
        ensure!(
            report["schema"] == 1,
            "unsupported compatibility report schema"
        );
        ensure!(
            report["status"] == "passed",
            "compatibility report did not pass"
        );
        ensure!(
            json_text(&report, "inputs_sha256")? == input_hash,
            "compatibility report has stale inputs"
        );
        let run = &report["run"];
        let url = json_text(run, "url")?;
        let id = url
            .strip_prefix("https://github.com/sam0x17/rtao/actions/runs/")
            .context("compatibility report requires an rtao GitHub Actions run URL")?;
        ensure!(
            !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()),
            "invalid compatibility run URL"
        );
        ensure!(
            canonical_hash(json_text(run, "head_commit")?, "", 40),
            "invalid compatibility run commit"
        );
        match json_text(&report, "kind")? {
            kind @ ("e2e" | "interop") => {
                let tested = members(&report, "runtimes")?;
                ensure!(
                    !tested.is_empty(),
                    "compatibility report tested no runtimes"
                );
                let digests = members(&report, "image_digests")?;
                ensure!(!digests.is_empty(), "compatibility report tested no images");
                for runtime in tested {
                    ensure!(
                        runtimes.contains(runtime),
                        "report runtime is outside validation scope"
                    );
                    for digest in digests {
                        let digest = digest
                            .as_str()
                            .context("report image digest must be a string")?;
                        ensure!(
                            images.contains(digest),
                            "report image digest differs from pinned inputs"
                        );
                        coverage.insert((
                            kind.to_owned(),
                            serde_json::to_string(&canonical(runtime))?,
                            digest.to_owned(),
                        ));
                    }
                }
            }
            "drift" => {
                let from = &report["from"];
                let to = &report["to"];
                ensure!(
                    runtimes.contains(from) && runtimes.contains(to),
                    "drift endpoint is outside validation scope"
                );
                ensure!(
                    from["network"] == to["network"] && from["genesis"] == to["genesis"],
                    "drift endpoints must belong to the same chain"
                );
                let class = json_text(&report, "classification")?;
                ensure!(
                    matches!(class, "surface-unchanged" | "additive" | "behavioural"),
                    "breaking or unknown drift cannot validate a spec range"
                );
                ensure!(
                    from != to || class == "surface-unchanged",
                    "a runtime self comparison must have unchanged surface"
                );
                drift.insert((
                    serde_json::to_string(&canonical(from))?,
                    serde_json::to_string(&canonical(to))?,
                ));
            }
            kind => bail!("unknown compatibility report kind {kind:?}"),
        }
    }
    for runtime in &runtimes {
        let key = serde_json::to_string(&canonical(runtime))?;
        for kind in ["e2e", "interop"] {
            for image in &images {
                ensure!(
                    coverage.contains(&(kind.into(), key.clone(), image.clone())),
                    "validated runtime lacks passing {kind} evidence on {image}"
                );
            }
        }
        let runtime_spec = u64::from(spec(runtime)?);
        if runtime_spec > minimum {
            let mut linked = false;
            for previous in &runtimes {
                if previous["network"] == runtime["network"]
                    && previous["genesis"] == runtime["genesis"]
                    && previous["spec_version"].as_u64() == Some(runtime_spec - 1)
                {
                    linked |= drift
                        .contains(&(serde_json::to_string(&canonical(previous))?, key.clone()));
                }
            }
            ensure!(
                linked,
                "validated runtime lacks an incoming drift transition"
            );
        }
        let next_spec = runtime_spec.saturating_add(1);
        let successors: Vec<_> = if minimum == maximum {
            vec![runtime]
        } else if next_spec <= maximum {
            runtimes
                .iter()
                .filter(|next| {
                    next["network"] == runtime["network"]
                        && next["genesis"] == runtime["genesis"]
                        && next["spec_version"].as_u64() == Some(next_spec)
                })
                .collect()
        } else {
            continue;
        };
        ensure!(
            !successors.is_empty(),
            "drift validation has a missing chain/spec transition"
        );
        let mut linked = false;
        for next in successors {
            linked |= drift.contains(&(key.clone(), serde_json::to_string(&canonical(next))?));
        }
        ensure!(linked, "validated range lacks a passing drift report");
    }
    Ok(Validation {
        range: range.clone(),
        runtimes,
        reports: references.clone(),
    })
}

fn document(root: &Path, milestone: Option<&str>) -> Result<Json> {
    let inputs = inputs(root)?;
    let evidence_path = checked_path(root, EVIDENCE, true)?;
    let validation = if evidence_path.exists() {
        validate(
            &inputs,
            &serde_json::from_slice(&read(root, EVIDENCE, MAX_REPORT_BYTES)?)?,
            |path| read(root, path, MAX_REPORT_BYTES),
        )?
    } else {
        Validation::absent()
    };
    let mut document = assemble_document(inputs, milestone, validation)?;
    let version = json_text(&document, "rtao_version")?;
    document["build_weight"] = json!(build_weight(root, version)?);
    Ok(canonical(&document))
}

fn assemble_document(
    mut inputs: Json,
    milestone: Option<&str>,
    validation: Validation,
) -> Result<Json> {
    let input_hash = sha256(&serde_json::to_vec(&canonical(&inputs))?);
    inputs["schema"] = json!(1);
    inputs["milestone"] = json!(milestone);
    inputs["validated_spec_range"] = validation.range;
    inputs["validated_runtimes"] = json!(validation.runtimes);
    inputs["validation"] = json!({"inputs_sha256": input_hash, "reports": validation.reports});
    Ok(canonical(&inputs))
}

fn markdown(document: &Json) -> Result<String> {
    let version = json_text(document, "rtao_version")?;
    let core = &document["core"];
    let core_label = if core["tag"].is_null() {
        format!("branch {}", json_text(core, "branch")?)
    } else {
        json_text(core, "tag")?.to_owned()
    };
    let parity = &document["parity"];
    let range = &document["validated_spec_range"];
    let range = if range.is_null() {
        "None recorded".to_owned()
    } else {
        format!("{}-{}", range["min"], range["max"])
    };
    let mut output = format!(
        "<!-- Generated by cargo xtask compat. Do not edit. -->\n# Compatibility\n\n\
         | rtao | core tag (commit) | parity tag (Python source version) | validated spec range | btauth | interop target |\n\
         |---|---|---|---|---|---|\n\
         | {version} | {} (`{}`) | {} ({}) | {range} | {} | `bittensor` @ {} (`{}`) |\n\n",
        core_label,
        json_text(core, "commit")?,
        json_text(parity, "tag")?,
        json_text(parity, "python_source_version")?,
        document["btauth"]["protocol"]
            .as_str()
            .context("btauth protocol missing")?,
        json_text(parity, "tag")?,
        json_text(parity, "commit")?,
    );
    if core["tag"].is_null() {
        output.push_str(&format!(
            "Disposable canary source: monorepo branch `{}` at `{}`. Retained release pin: `{}` at `{}`.\n\n",
            json_text(core, "branch")?,
            json_text(core, "commit")?,
            json_text(&core["release_pin"], "tag")?,
            json_text(&core["release_pin"], "commit")?,
        ));
    }
    if let Some(milestone) = document["milestone"].as_str() {
        output.push_str(&format!("Release milestone: {milestone}.\n\n"));
    }
    output.push_str("No PyPI release has been verified from the pinned Python commit. The Python version above is the source project version.\n\n");
    if document["validated_spec_range"].is_null() {
        output.push_str("No runtime range has passed the required drift, e2e and interop validation. Clients warn for every connected runtime. Metadata pins describe fetched inputs; per-call surface checks still determine which operations are available.\n\n");
    } else {
        output.push_str("The validated identities and immutable report checksums are listed in [compat.json](compat.json). Validation requires passing drift, e2e and interop reports for each runtime and every listed localnet digest. Clients warn outside those identities; per-call surface checks determine which operations are available.\n\n");
    }
    output.push_str("## Metadata inputs\n\n| network | spec | finalized block | version | raw bytes | SHA-256 checksum | runtime metadata hash |\n|---|---|---|---|---|---|---|\n");
    for pin in members(document, "metadata_pins")? {
        for version in ["v15", "v16"] {
            let blob = &pin[version];
            output.push_str(&format!(
                "| {} | {} | `{}` | {version} | [{}]({}) | `{}` | `{}` |\n",
                json_text(pin, "network")?,
                pin["spec_version"],
                json_text(pin, "finalized_head")?,
                blob["bytes"],
                json_text(blob, "path")?,
                json_text(blob, "sha256")?,
                json_text(&blob["metadata_hash"], "value")?,
            ));
        }
    }
    output.push_str("\nRuntime metadata hashes use BLAKE2b with a 256-bit output over the complete raw metadata blob, including its SCALE prefix and version byte. SHA-256 verifies the stored file. These hashes are independent of structural surface hashes and the runtime's transaction metadata commitment. [compat.json](compat.json) records the hash kind and each input's chain, code hash and source provenance.\n\n## Localnet images\n\n");
    output.push_str(&format!(
        "Image repository: `{}`.\n\n| role | tag | digest |\n|---|---|---|\n",
        json_text(&document["localnet"], "image")?
    ));
    for role in ["pinned", "mainnet", "testnet", "devnet", "main"] {
        let image = &document["localnet"][role];
        output.push_str(&format!(
            "| {role} | `{}` | `{}` |\n",
            json_text(image, "tag")?,
            json_text(image, "digest")?
        ));
    }
    output.push_str("\n## Measured build weight\n\n");
    let weight = document.get("build_weight").and_then(Json::as_array);
    if weight.is_none_or(Vec::is_empty) {
        output.push_str("No target/profile dependency counts are recorded for this release. Measure with `cargo xtask compat --measure-build-weight`; select a target with `--build-target <triple>`.\n\n");
    } else {
        output.push_str("Distinct package identities reached through normal and build edges, excluding development dependencies. Each row selects one profile with default features disabled. Counts describe the recorded target and dependency inputs, rather than every platform or elapsed build time.\n\n| target | profile | packages | core host | report |\n|---|---|---:|---|---|\n");
        for report in weight.into_iter().flatten() {
            let record = &report["record"];
            for row in members(record, "profiles")? {
                output.push_str(&format!(
                    "| `{}` | `{}` | {} | {} | [measurement]({}) |\n",
                    json_text(record, "target")?,
                    json_text(row, "profile")?,
                    row["normal_build_packages"],
                    row["core_host_enabled"],
                    json_text(report, "path")?
                ));
            }
            output.push_str(&format!("\nMeasured on `{}` ({}) with `{}`, source `{}`{}; source-manifest and lock hashes, selected features, package inventories and measurement method are retained in the report.\n\n", json_text(record,"host")?,json_text(record,"machine")?,json_text(record,"compiler")?.lines().next().context("compiler missing")?,json_text(&record["source"],"head_commit")?, if record["source"]["dirty"]==true { " with dependency changes in the working tree" } else { "" }));
        }
    }
    output.push_str("Regenerate with `cargo xtask compat`; verify offline with `cargo xtask compat --check`. Validation report requirements are documented in [docs/compat/README.md](docs/compat/README.md).\n");
    Ok(output)
}

fn verify_output(path: &Path, expected: &[u8]) -> Result<()> {
    ensure!(
        fs::read(path)? == expected,
        "{} is stale; run cargo xtask compat",
        path.display()
    );
    Ok(())
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let options = options(args)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no workspace root")?;
    if options.measure_build_weight {
        measure_build_weight(root, options.build_target.as_deref())?;
    }
    let json_path = checked_path(root, &options.json, true)?;
    let markdown_path = checked_path(root, &options.markdown, true)?;
    let previous: Option<Json> = if json_path.exists() {
        Some(serde_json::from_slice(&fs::read(&json_path)?)?)
    } else {
        None
    };
    let milestone = options.milestone.as_deref().or_else(|| {
        previous
            .as_ref()
            .and_then(|value| value["milestone"].as_str())
    });
    if let Some(milestone) = milestone {
        ensure!(
            (0..=10).any(|index| milestone == format!("M{index}")),
            "invalid recorded milestone"
        );
    }
    let document = document(root, milestone)?;
    if milestone.is_some_and(|name| name != "M0") {
        ensure!(
            !members(&document, "build_weight")?.is_empty(),
            "release milestone requires measured target/profile build-weight records"
        );
    }
    let mut json_bytes = serde_json::to_vec_pretty(&document)?;
    json_bytes.push(b'\n');
    let markdown = markdown(&document)?;
    if options.check {
        verify_output(&json_path, &json_bytes)?;
        verify_output(&markdown_path, markdown.as_bytes())?;
        println!("Verified compatibility outputs and validation evidence offline");
    } else {
        crate::pin::atomic_write(&json_path, &json_bytes)?;
        crate::pin::atomic_write(&markdown_path, markdown.as_bytes())?;
        println!("Generated {} and {}", options.json, options.markdown);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn build_weight_deduplicates_packages_without_merging_versions_or_sources() -> Result<()> {
        let root = Path::new("/workspace");
        let packages = package_identities(
            "rtao v0.1.0 (/workspace/crates/rtao)\nserde v1.0.1\nserde v1.0.1 (*)\nserde v1.0.2\nserde v1.0.1 (https://example.invalid/repo#abcdef)",
            root,
        )?;
        assert_eq!(packages.len(), 4);
        assert!(packages.contains("rtao v0.1.0 ($WORKSPACE/crates/rtao)"));
        assert!(package_identities("unparseable", root).is_err());
        Ok(())
    }

    #[test]
    fn build_weight_rejects_stale_inputs_counts_features_and_host_activation() -> Result<()> {
        let input = json!({"Cargo.lock":"a".repeat(64)});
        let report = json!({"schema":1,"rtao_version":"0.1.0","target":"aarch64-apple-darwin","host":"aarch64-apple-darwin",
            "compiler":"rustc fixture\nhost: aarch64-apple-darwin", "cargo":"cargo fixture","machine":"fixture CPU","measured_unix_seconds":1,
            "method":{"command":BUILD_COMMAND,"count":BUILD_METHOD,"host_feature_command":BUILD_FEATURE_COMMAND},
            "source":{"head_commit":"a".repeat(40),"dirty":false,"dependency_inputs":input,
                "dependency_inputs_sha256":sha256(&serde_json::to_vec(&canonical(&input))?)},
            "profiles":BUILD_PROFILES.iter().map(|(profile,package,features)|json!({
                "profile":profile,"package":package,"features":features,"default_features":false,
                "package_identities":["fixture v1.0.0"],"normal_build_packages":1,"core_host_enabled":false,"feature_graph_sha256":"a".repeat(64),
            })).collect::<Vec<_>>()});
        validate_build_weight(&report, "0.1.0", &input)?;
        assert!(validate_build_weight(&report, "0.2.0", &input).is_err());
        assert!(
            validate_build_weight(&report, "0.1.0", &json!({"Cargo.lock":"b".repeat(64)})).is_err()
        );
        for (key, value) in [
            ("normal_build_packages", json!(2)),
            ("core_host_enabled", json!(true)),
            ("features", json!(["owner"])),
            ("default_features", json!(true)),
        ] {
            let mut altered = report.clone();
            altered["profiles"][0][key] = value;
            assert!(validate_build_weight(&altered, "0.1.0", &input).is_err());
        }
        let mut altered = report.clone();
        altered["profiles"][0]["package_identities"] = json!(["fixture v1.0.0", "fixture v1.0.0"]);
        assert!(validate_build_weight(&altered, "0.1.0", &input).is_err());
        Ok(())
    }

    #[test]
    fn canary_records_actual_revision_and_keeps_release_provenance_separate() -> Result<()> {
        let released = "11".repeat(20);
        let candidate = "22".repeat(20);
        let mut pins: DocumentMut = format!(
            "[monorepo]\nurl='https://github.com/RaoFoundation/subtensor.git'\n[core]\ntag='v478'\ncommit='{released}'\n"
        )
        .parse()?;
        let mut manifest: DocumentMut =
            "dependency={git='https://github.com/RaoFoundation/subtensor.git',tag='v478'}"
                .parse()?;
        let release = core_input(&pins, &manifest["dependency"])?;
        pins["core"]["canary"] = toml_edit::table();
        pins["core"]["canary"]["branch"] = toml_edit::value("main");
        pins["core"]["canary"]["commit"] = toml_edit::value(&candidate);
        let dependency = manifest["dependency"]
            .as_table_like_mut()
            .context("fixture dependency missing")?;
        dependency.remove("tag");
        dependency.insert("rev", toml_edit::value(&candidate));
        let canary = core_input(&pins, &manifest["dependency"])?;
        assert!(canary["tag"].is_null());
        assert_eq!(canary["branch"], "main");
        assert_eq!(canary["commit"], candidate);
        assert_eq!(canary["release_pin"], release);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let mut input = inputs(root)?;
        input["core"] = canary;
        let document = assemble_document(input, None, Validation::absent())?;
        let rendered = markdown(&document)?;
        assert!(rendered.contains("branch main") && rendered.contains("Retained release pin"));
        assert!(document["validated_spec_range"].is_null());
        manifest["dependency"]["tag"] = toml_edit::value("v478");
        assert!(core_input(&pins, &manifest["dependency"]).is_err());
        manifest["dependency"]
            .as_table_like_mut()
            .context("fixture dependency missing")?
            .remove("tag");
        manifest["dependency"]["rev"] = toml_edit::value(released);
        assert!(core_input(&pins, &manifest["dependency"]).is_err());
        Ok(())
    }

    fn fixture_inputs(specs: &[u32]) -> Json {
        let pins: Vec<_> = specs.iter().map(|spec| json!({
            "network": "finney", "spec_version": spec,
            "genesis": format!("0x{}", "1".repeat(64)),
            "v15": {"metadata_hash": {"kind": HASH_KIND, "value": format!("0x{spec:064x}")}},
            "v16": {"metadata_hash": {"kind": HASH_KIND, "value": format!("0x{:064x}", u64::from(*spec) + 1000)}},
        })).collect();
        json!({
            "rtao_version": "0.1.0",
            "metadata_pins": pins,
            "localnet": {"image": "ghcr.io/raofoundation/subtensor-localnet", "pinned": {"tag": "v478", "digest": format!("sha256:{}", "a".repeat(64))}},
        })
    }

    fn passing_reports(inputs: &Json) -> Result<Vec<Json>> {
        let runtimes: Vec<_> = members(inputs, "metadata_pins")?
            .iter()
            .map(|pin| runtime_identity(pin, "v15"))
            .collect();
        let images: Vec<_> = image_digests(inputs)?.into_iter().collect();
        let mut reports = vec![
            json!({"kind": "e2e", "runtimes": runtimes, "image_digests": images}),
            json!({"kind": "interop", "runtimes": runtimes, "image_digests": images}),
        ];
        if runtimes.len() == 1 {
            reports.push(json!({"kind": "drift", "from": runtimes[0], "to": runtimes[0], "classification": "surface-unchanged"}));
        } else {
            for pair in runtimes.windows(2) {
                reports.push(json!({"kind": "drift", "from": pair[0], "to": pair[1], "classification": "additive"}));
            }
        }
        Ok(reports)
    }

    fn evidence(inputs: &Json, reports: Vec<Json>) -> Result<(Json, BTreeMap<String, Vec<u8>>)> {
        let input_hash = sha256(&serde_json::to_vec(&canonical(inputs))?);
        let runtimes: Vec<_> = members(inputs, "metadata_pins")?
            .iter()
            .map(|pin| runtime_identity(pin, "v15"))
            .collect();
        let mut references = Vec::new();
        let mut files = BTreeMap::new();
        for (index, mut report) in reports.into_iter().enumerate() {
            report["schema"] = json!(1);
            report["inputs_sha256"] = json!(input_hash);
            if report.get("status").is_none() {
                report["status"] = json!("passed");
            }
            report["run"] = json!({"url": "https://github.com/sam0x17/rtao/actions/runs/123", "head_commit": "a".repeat(40)});
            let bytes = serde_json::to_vec(&report)?;
            let path = format!("docs/compat/reports/{index}.json");
            references.push(json!({"path": path, "sha256": sha256(&bytes)}));
            files.insert(path, bytes);
        }
        Ok((
            json!({
                "schema": 1,
                "inputs_sha256": input_hash,
                "validated_spec_range": {"min": runtimes.first().context("no fixture runtimes")?["spec_version"], "max": runtimes.last().context("no fixture runtimes")?["spec_version"]},
                "runtimes": runtimes, "reports": references,
            }),
            files,
        ))
    }

    fn verify_fixture(
        inputs: &Json,
        ledger: &Json,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Validation> {
        validate(inputs, ledger, |path| {
            files.get(path).cloned().context("missing report")
        })
    }

    #[test]
    fn compat_records_inputs_without_claiming_validation() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let document = assemble_document(inputs(root)?, None, Validation::absent())?;
        ensure!(document["validated_spec_range"].is_null());
        ensure!(members(&document, "validated_runtimes")?.is_empty());
        ensure!(document["parity"]["pypi_release"].is_null());
        let pins = members(&document, "metadata_pins")?;
        ensure!(!pins.is_empty());
        for pin in pins {
            for version in ["v15", "v16"] {
                let blob = &pin[version];
                ensure!(blob["metadata_hash"]["kind"] == HASH_KIND);
                ensure!(
                    json_text(&blob["metadata_hash"], "value")?
                        != format!("0x{}", json_text(blob, "sha256")?)
                );
            }
        }
        ensure!(markdown(&document)?.contains("Clients warn for every connected runtime"));
        Ok(())
    }

    #[test]
    fn compat_validation_requires_all_three_passed_report_kinds() -> Result<()> {
        let inputs = fixture_inputs(&[478]);
        let reports = passing_reports(&inputs)?;
        let (ledger, files) = evidence(&inputs, reports.clone())?;
        ensure!(verify_fixture(&inputs, &ledger, &files)?.range == json!({"min": 478, "max": 478}));
        for missing in ["e2e", "interop", "drift"] {
            let subset = reports
                .iter()
                .filter(|report| report["kind"] != missing)
                .cloned()
                .collect();
            let (ledger, files) = evidence(&inputs, subset)?;
            ensure!(
                verify_fixture(&inputs, &ledger, &files).is_err(),
                "missing {missing} unexpectedly validated"
            );
        }
        Ok(())
    }

    #[test]
    fn compat_rejects_stale_inputs_and_mutated_report_bytes() -> Result<()> {
        let inputs = fixture_inputs(&[478]);
        let (ledger, mut files) = evidence(&inputs, passing_reports(&inputs)?)?;
        let mut changed = inputs.clone();
        changed["rtao_version"] = json!("0.2.0");
        ensure!(verify_fixture(&changed, &ledger, &files).is_err());
        files
            .get_mut("docs/compat/reports/0.json")
            .context("fixture report missing")?
            .push(b' ');
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        Ok(())
    }

    #[test]
    fn compat_rejects_failed_reports_and_breaking_or_unknown_drift() -> Result<()> {
        let inputs = fixture_inputs(&[478]);
        for status in ["failed", "skipped", "pending"] {
            let mut reports = passing_reports(&inputs)?;
            reports[0]["status"] = json!(status);
            let (ledger, files) = evidence(&inputs, reports)?;
            ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        }
        for class in ["breaking", "unknown"] {
            let mut reports = passing_reports(&inputs)?;
            reports[2]["classification"] = json!(class);
            let (ledger, files) = evidence(&inputs, reports)?;
            ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        }
        Ok(())
    }

    #[test]
    fn compat_requires_coverage_on_every_listed_image_digest() -> Result<()> {
        let mut inputs = fixture_inputs(&[478]);
        inputs["localnet"]["devnet"] =
            json!({"tag": "devnet", "digest": format!("sha256:{}", "b".repeat(64))});
        let mut reports = passing_reports(&inputs)?;
        reports[1]["image_digests"]
            .as_array_mut()
            .context("fixture digests missing")?
            .pop();
        let (ledger, files) = evidence(&inputs, reports)?;
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        let mut reports = passing_reports(&inputs)?;
        reports[0]["image_digests"][0] = json!(format!("sha256:{}", "f".repeat(64)));
        let (ledger, files) = evidence(&inputs, reports)?;
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        Ok(())
    }

    #[test]
    fn compat_range_requires_contiguous_specs_and_linked_drift() -> Result<()> {
        let inputs = fixture_inputs(&[478, 479]);
        let reports = passing_reports(&inputs)?;
        let (ledger, files) = evidence(&inputs, reports.clone())?;
        ensure!(verify_fixture(&inputs, &ledger, &files)?.runtimes.len() == 2);
        let mut reversed = reports;
        let from = reversed[2]["from"].clone();
        reversed[2]["from"] = reversed[2]["to"].clone();
        reversed[2]["to"] = from;
        let (ledger, files) = evidence(&inputs, reversed)?;
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        let inputs = fixture_inputs(&[478, 480]);
        let (ledger, files) = evidence(&inputs, passing_reports(&inputs)?)?;
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        Ok(())
    }

    #[test]
    fn compat_rejects_metadata_hash_algorithm_and_identity_changes() -> Result<()> {
        let inputs = fixture_inputs(&[478]);
        let (mut ledger, files) = evidence(&inputs, passing_reports(&inputs)?)?;
        ledger["runtimes"][0]["metadata_hash"]["kind"] = json!("sha256");
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        ledger["runtimes"][0]["metadata_hash"]["kind"] = json!(HASH_KIND);
        ledger["runtimes"][0]["genesis"] = json!(format!("0x{}", "2".repeat(64)));
        ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        Ok(())
    }

    #[test]
    fn compat_options_reject_traversal_and_unknown_milestones() -> Result<()> {
        for path in [
            "../compat.json",
            "/tmp/compat.json",
            "docs/../compat.json",
            "docs/./compat.json",
            "docs//compat.json",
            "",
        ] {
            ensure!(options(vec!["--json".into(), path.into()]).is_err());
        }
        for milestone in ["M11", "M01", "m1", "1"] {
            ensure!(options(vec!["--milestone".into(), milestone.into()]).is_err());
        }
        ensure!(options(vec!["--check".into(), "--milestone".into(), "M1".into()])?.check);
        Ok(())
    }

    #[test]
    fn compat_report_reference_cannot_escape_the_recorded_report_directory() -> Result<()> {
        let inputs = fixture_inputs(&[478]);
        let (mut ledger, files) = evidence(&inputs, passing_reports(&inputs)?)?;
        for path in [
            "docs/compat/reports/../private.json",
            "docs/execution/report.json",
            "/tmp/report.json",
        ] {
            ledger["reports"][0]["path"] = json!(path);
            ensure!(verify_fixture(&inputs, &ledger, &files).is_err());
        }
        Ok(())
    }

    #[test]
    fn compat_canonical_json_is_independent_of_object_insertion_order() -> Result<()> {
        let first: Json = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":3}"#)?;
        let second: Json = serde_json::from_str(r#"{"a":3,"z":{"a":1,"b":2}}"#)?;
        ensure!(
            serde_json::to_vec(&canonical(&first))? == serde_json::to_vec(&canonical(&second))?
        );
        Ok(())
    }
}
