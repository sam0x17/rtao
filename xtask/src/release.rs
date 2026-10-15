//! Fail-closed verification of the complete release asset set.
//!
//! This command never publishes, uploads, signs, or changes host settings.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REPOSITORY: &str = "sam0x17/rtao";
const WORKFLOW: &str = "sam0x17/rtao/.github/workflows/release.yml";
const TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
];
const MAC_CHECKS: [&str; 10] = [
    "build-arm",
    "build-intel",
    "facade-arm",
    "facade-intel",
    "test-cli-arm",
    "test-cli-intel",
    "test-suite-arm",
    "test-suite-intel",
    "smoke-arm",
    "smoke-intel",
];
const MAX_RECORD: u64 = 1024 * 1024;
const ROOT_CERT_LICENSE: &str = "webpki-root-certs-LICENSE";
const ROOT_CERT_LICENSE_SHA256: &str =
    "e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8";
const ROOT_CERT_LICENSE_BYTES: &[u8] =
    include_bytes!("../../docs/licenses/webpki-root-certs-LICENSE");
const UNICODE_LICENSE: &str = "Unicode-DFS-2016.txt";
const UNICODE_LICENSE_SHA256: &str =
    "68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844";
const UNICODE_LICENSE_BYTES: &[u8] = include_bytes!("../../docs/licenses/Unicode-DFS-2016.txt");

pub(crate) struct License {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) sha256: Option<&'static str>,
    pub(crate) bytes: &'static [u8],
}

pub(crate) const LICENSES: [License; 6] = [
    License {
        name: "LICENSE-MIT",
        source: "LICENSE-MIT",
        sha256: Some("ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7"),
        bytes: include_bytes!("../../LICENSE-MIT"),
    },
    License {
        name: "LICENSE-APACHE",
        source: "LICENSE-APACHE",
        sha256: Some("cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"),
        bytes: include_bytes!("../../LICENSE-APACHE"),
    },
    License {
        name: "NOTICE",
        source: "NOTICE",
        // Attribution follows the release source/tag, including imported pins.
        sha256: None,
        bytes: include_bytes!("../../NOTICE"),
    },
    License {
        name: "LICENSE-PSF",
        source: "crates/rtao-primitives/LICENSE-PSF",
        sha256: Some("3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf"),
        bytes: include_bytes!("../../crates/rtao-primitives/LICENSE-PSF"),
    },
    License {
        name: ROOT_CERT_LICENSE,
        source: "docs/licenses/webpki-root-certs-LICENSE",
        sha256: Some(ROOT_CERT_LICENSE_SHA256),
        bytes: ROOT_CERT_LICENSE_BYTES,
    },
    License {
        name: UNICODE_LICENSE,
        source: "docs/licenses/Unicode-DFS-2016.txt",
        sha256: Some(UNICODE_LICENSE_SHA256),
        bytes: UNICODE_LICENSE_BYTES,
    },
];

pub(crate) fn validate_license_bytes(license: &License, bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty()
            && bytes.len() as u64 <= MAX_RECORD
            && bytes == license.bytes
            && license
                .sha256
                .is_none_or(|expected| format!("{:x}", Sha256::digest(bytes)) == expected),
        "{} notice differs from the exact source text/hash",
        license.name
    );
    Ok(())
}

fn release_licenses(root: &Path, dir: &Path) -> Result<()> {
    for license in &LICENSES {
        let source = root.join(license.source);
        for (directory, name) in [
            (
                source.parent().context("notice source parent")?,
                source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("notice source name")?,
            ),
            (dir, license.name),
        ] {
            let path = regular_file(directory, name)?;
            ensure!(
                fs::metadata(&path)?.len() == license.bytes.len() as u64
                    && fs::metadata(&path)?.len() <= MAX_RECORD,
                "{} license length differs from its exact approved text",
                license.name
            );
            validate_license_bytes(license, &fs::read(path)?)?;
        }
    }
    Ok(())
}

fn output(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let result = Command::new(program)
        .current_dir(root)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    ensure!(
        result.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn source(root: &Path, tag: &str) -> Result<(String, String)> {
    crate::release_notes::tag_version(tag)?;
    let commit = output(
        root,
        "git",
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )?;
    ensure!(
        output(root, "git", &["rev-parse", "HEAD"])? == commit,
        "checkout does not match the release tag"
    );
    ensure!(
        output(
            root,
            "git",
            &["status", "--porcelain", "--untracked-files=normal"]
        )?
        .is_empty(),
        "release source checkout is dirty"
    );
    let tree = output(root, "git", &["rev-parse", "HEAD^{tree}"])?;
    Ok((commit, tree))
}

fn regular_file(dir: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
            && name != "."
            && name != "..",
        "invalid asset name"
    );
    let path = dir.join(name);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("required release asset {} is missing", path.display()))?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "release assets must be nonempty regular files: {name}"
    );
    Ok(path)
}

fn hash(dir: &Path, name: &str) -> Result<String> {
    use std::io::Read;
    let mut input = fs::File::open(regular_file(dir, name)?)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn record(dir: &Path, name: &str) -> Result<Value> {
    let path = regular_file(dir, name)?;
    ensure!(
        fs::metadata(&path)?.len() <= MAX_RECORD,
        "release record exceeds size limit"
    );
    serde_json::from_slice(&fs::read(path)?).with_context(|| format!("parse {name}"))
}

fn exact(value: &Value, key: &str, wanted: &str) -> Result<()> {
    ensure!(
        value.get(key).and_then(Value::as_str) == Some(wanted),
        "release record {key} does not match {wanted}"
    );
    Ok(())
}

fn source_record(value: &Value, tag: &str, commit: &str, tree: &str) -> Result<()> {
    exact(value, "schema", "rtao-build-v1")?;
    exact(value, "tag", tag)?;
    exact(value, "commit", commit)?;
    exact(value, "tree", tree)?;
    let toolchain = value
        .get("rustc")
        .and_then(Value::as_str)
        .context("missing Rust toolchain record")?;
    ensure!(
        toolchain.starts_with("rustc ") && toolchain.contains("commit-hash:"),
        "Rust toolchain record must contain rustc --version --verbose"
    );
    ensure!(
        value
            .get("cargo")
            .and_then(Value::as_str)
            .is_some_and(|version| version.starts_with("cargo ")),
        "missing Cargo toolchain record"
    );
    Ok(())
}

fn pinned_toolchain(root: &Path, value: &Value) -> Result<()> {
    let config: toml_edit::DocumentMut =
        fs::read_to_string(root.join("rust-toolchain.toml"))?.parse()?;
    let channel = config
        .get("toolchain")
        .and_then(|item| item.get("channel"))
        .and_then(toml_edit::Item::as_str)
        .context("release source has no pinned Rust channel")?;
    let version = semver::Version::parse(channel)
        .context("release Rust channel must be an exact stable version")?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty() && version.to_string() == channel,
        "release Rust channel must be an exact stable version"
    );
    ensure!(
        value["rustc"]
            .as_str()
            .is_some_and(|rustc| rustc.starts_with(&format!("rustc {channel} (")))
            && value["cargo"]
                .as_str()
                .is_some_and(|cargo| cargo.starts_with(&format!("cargo {channel} ("))),
        "recorded Rust/Cargo versions do not match the source toolchain pin"
    );
    Ok(())
}

fn archive(tag: &str, target: &str) -> String {
    format!("rtao-{tag}-{target}.tar.gz")
}

fn archive_shape(dir: &Path, name: &str) -> Result<()> {
    let path = regular_file(dir, name)?;
    let listing = output(
        dir,
        "tar",
        &["-tzf", path.to_str().context("asset path is not UTF-8")?],
    )?;
    ensure!(
        listing.lines().collect::<Vec<_>>()
            == std::iter::once("rtao")
                .chain(LICENSES.iter().map(|license| license.name))
                .collect::<Vec<_>>(),
        "CLI archive must contain exactly the rtao executable and all six notices"
    );
    let details = output(
        dir,
        "tar",
        &["-tvzf", path.to_str().context("asset path is not UTF-8")?],
    )?;
    ensure!(
        details.starts_with('-')
            && details
                .split_whitespace()
                .next()
                .is_some_and(|mode| mode.contains('x')),
        "CLI archive must contain a regular executable"
    );
    let entries: Vec<_> = details.lines().collect();
    ensure!(
        entries.len() == LICENSES.len() + 1
            && entries[1..].iter().all(|entry| entry.starts_with('-')),
        "CLI license members must be regular files"
    );
    for license in &LICENSES {
        archived_license(dir, &path, license, license.name)?;
    }
    Ok(())
}

fn archived_license(dir: &Path, path: &Path, license: &License, member: &str) -> Result<()> {
    use std::io::Read;
    ensure!(
        license.bytes.len() as u64 <= MAX_RECORD,
        "compiled notice exceeds size limit"
    );
    let mut extracted = Command::new("tar")
        .current_dir(dir)
        .args([
            "-xOzf",
            path.to_str().context("asset path is not UTF-8")?,
            member,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    extracted
        .stdout
        .take()
        .context("archived license stdout")?
        .take(license.bytes.len() as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > license.bytes.len() {
        let _ = extracted.kill();
        let _ = extracted.wait();
        bail!("archived {} license exceeds its pinned size", license.name);
    }
    ensure!(
        extracted.wait()?.success(),
        "cannot inspect archived {} license",
        license.name
    );
    validate_license_bytes(license, &bytes)?;
    Ok(())
}

fn docs_archive(dir: &Path, name: &str) -> Result<()> {
    let path = regular_file(dir, name)?;
    let listing = output(
        dir,
        "tar",
        &[
            "-tzf",
            path.to_str().context("docs asset path is not UTF-8")?,
        ],
    )?;
    let mut has_index = false;
    let mut has_facade = false;
    let mut notices = BTreeMap::new();
    for entry in listing.lines() {
        let member = entry;
        ensure!(
            !entry.starts_with('/') && !entry.contains('\\') && !entry.contains('\0'),
            "unsafe docs archive path"
        );
        let entry = entry
            .strip_prefix("./")
            .unwrap_or(entry)
            .trim_end_matches('/');
        if entry.is_empty() {
            continue;
        }
        ensure!(
            entry
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
            "docs archive path escapes its directory"
        );
        has_index |= entry == "index.html";
        has_facade |= entry == "rtao/index.html" || entry == "api/rtao/index.html";
        if LICENSES.iter().any(|license| entry == license.name) {
            ensure!(
                notices.insert(entry, member).is_none(),
                "duplicate docs notice {entry}"
            );
        }
    }
    ensure!(
        has_index && has_facade,
        "docs archive lacks an index or facade rustdoc"
    );
    let details = output(
        dir,
        "tar",
        &[
            "-tvzf",
            path.to_str().context("docs asset path is not UTF-8")?,
        ],
    )?;
    ensure!(
        details
            .lines()
            .all(|line| line.starts_with('-') || line.starts_with('d')),
        "docs archive contains a link or special file"
    );
    for license in &LICENSES {
        let member = notices
            .get(license.name)
            .context("docs archive lacks a required notice")?;
        archived_license(dir, &path, license, member)?;
    }
    Ok(())
}

fn asset_record(value: &Value, dir: &Path, name: &str, target: &str, mode: &str) -> Result<()> {
    exact(value, "name", name)?;
    exact(value, "target", target)?;
    exact(value, "mode", mode)?;
    exact(value, "sha256", &hash(dir, name)?)?;
    archive_shape(dir, name)
}

fn verify_attestation(
    root: &Path,
    dir: &Path,
    asset: &str,
    bundle: &str,
    tag: &str,
    commit: &str,
) -> Result<()> {
    let asset = regular_file(dir, asset)?;
    let bundle = regular_file(dir, bundle)?;
    let status = Command::new("gh")
        .current_dir(root)
        .args(["attestation", "verify"])
        .arg(asset)
        .args([
            "--repo",
            REPOSITORY,
            "--signer-workflow",
            WORKFLOW,
            "--source-ref",
            &format!("refs/tags/{tag}"),
            "--source-digest",
            commit,
            "--signer-digest",
            commit,
            "--deny-self-hosted-runners",
            "--bundle",
        ])
        .arg(bundle)
        .status()
        .context("gh with attestation verification support is required")?;
    ensure!(status.success(), "GitHub provenance verification failed");
    Ok(())
}

fn verify_mac_signature(root: &Path, dir: &Path) -> Result<()> {
    let allowed = root.join("ci/release/allowed-signers");
    ensure!(
        allowed.is_file() && !fs::symlink_metadata(&allowed)?.file_type().is_symlink(),
        "local release trust anchor ci/release/allowed-signers is absent; local-build provenance is unresolved"
    );
    let keys = fs::read_to_string(&allowed)?;
    let active: Vec<_> = keys
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .collect();
    ensure!(
        !active.is_empty()
            && active.iter().all(|line| line
                .split_whitespace()
                .take(2)
                .eq(["rtao-mac", "ssh-ed25519"])),
        "local release trust anchor must contain only rtao-mac Ed25519 signers"
    );
    let manifest = regular_file(dir, "macos-build.json")?;
    let signature = regular_file(dir, "macos-build.json.sig")?;
    let mut child = Command::new("ssh-keygen")
        .current_dir(root)
        .args(["-Y", "verify", "-I", "rtao-mac", "-n", "rtao-release", "-f"])
        .arg(allowed)
        .arg("-s")
        .arg(signature)
        .stdin(Stdio::piped())
        .spawn()
        .context("OpenSSH signature verification is required")?;
    child
        .stdin
        .take()
        .context("signature verifier stdin")?
        .write_all(&fs::read(manifest)?)?;
    ensure!(
        child.wait()?.success(),
        "local Mac build signature is not trusted"
    );
    Ok(())
}

fn validate_mac(value: &Value, dir: &Path, tag: &str, commit: &str, tree: &str) -> Result<()> {
    source_record(value, tag, commit, tree)?;
    exact(value, "builder", "local-mac-studio")?;
    exact(value, "host", "aarch64-apple-darwin")?;
    ensure!(
        value
            .get("macos")
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("ProductVersion:")),
        "missing macOS version record"
    );
    ensure!(
        value
            .get("sdk")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "missing Apple SDK version"
    );
    let assets = value
        .get("artifacts")
        .and_then(Value::as_array)
        .context("missing Mac artifacts")?;
    ensure!(
        assets.len() == 2,
        "Mac record must contain exactly both targets"
    );
    for (target, mode) in [(TARGETS[2], "native"), (TARGETS[3], "cross-rosetta")] {
        let matches: Vec<_> = assets
            .iter()
            .filter(|asset| asset.get("target").and_then(Value::as_str) == Some(target))
            .collect();
        ensure!(matches.len() == 1, "Mac target is missing or duplicated");
        asset_record(matches[0], dir, &archive(tag, target), target, mode)?;
    }
    let checks = value
        .get("checks")
        .and_then(Value::as_array)
        .context("missing Mac checks")?;
    ensure!(
        checks.len() == MAC_CHECKS.len(),
        "Mac record has an incomplete check set"
    );
    for name in MAC_CHECKS {
        let matches: Vec<_> = checks
            .iter()
            .filter(|check| check.get("name").and_then(Value::as_str) == Some(name))
            .collect();
        ensure!(
            matches.len() == 1,
            "Mac check {name} is missing or duplicated"
        );
        let check = matches[0];
        ensure!(
            check.get("exit_code").and_then(Value::as_u64) == Some(0),
            "Mac check {name} did not pass"
        );
        let log = format!("macos-{name}.log");
        exact(check, "log", &log)?;
        exact(check, "sha256", &hash(dir, &log)?)?;
    }
    Ok(())
}

fn expected(tag: &str) -> BTreeSet<String> {
    let mut files: BTreeSet<_> = TARGETS.iter().map(|target| archive(tag, target)).collect();
    for target in &TARGETS[..2] {
        files.insert(format!("linux-{target}-build.json"));
        files.insert(format!("linux-{target}.intoto.jsonl"));
    }
    for name in [
        "macos-build.json",
        "macos-build.json.sig",
        "docs-build.json",
        "docs.intoto.jsonl",
        "COMPAT.md",
        "compat.json",
        "notes.md",
        "llms.txt",
    ] {
        files.insert(name.to_owned());
    }
    files.extend(LICENSES.iter().map(|license| license.name.to_owned()));
    for name in MAC_CHECKS {
        files.insert(format!("macos-{name}.log"));
    }
    files.insert(format!("rustdoc-{tag}.tar.gz"));
    files.insert(format!("book-{tag}.tar.gz"));
    files
}

fn docs_files(tag: &str) -> BTreeSet<String> {
    let mut files: BTreeSet<_> = [
        format!("rustdoc-{tag}.tar.gz"),
        format!("book-{tag}.tar.gz"),
        "COMPAT.md".into(),
        "compat.json".into(),
        "llms.txt".into(),
        "notes.md".into(),
    ]
    .into_iter()
    .collect();
    files.extend(LICENSES.iter().map(|license| license.name.to_owned()));
    files
}

fn checksum_text(dir: &Path, files: &BTreeSet<String>) -> Result<String> {
    files
        .iter()
        .map(|name| Ok(format!("{}  {name}\n", hash(dir, name)?)))
        .collect()
}

fn check_inventory(dir: &Path, files: &BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("non-UTF8 release asset"))?;
        ensure!(
            name == "SHA256SUMS" || files.contains(&name),
            "unexpected release asset {name}"
        );
        regular_file(dir, &name)?;
    }
    for name in files {
        regular_file(dir, name)?;
    }
    Ok(())
}

fn verify(root: &Path, dir: &Path, tag: &str, write_checksums: bool) -> Result<()> {
    let (commit, tree) = source(root, tag)?;
    let files = expected(tag);
    check_inventory(dir, &files)?;
    release_licenses(root, dir)?;
    verify_mac_signature(root, dir)?;
    let mac = record(dir, "macos-build.json")?;
    pinned_toolchain(root, &mac)?;
    validate_mac(&mac, dir, tag, &commit, &tree)?;
    for (target, mode) in [(TARGETS[0], "native"), (TARGETS[1], "cross-qemu")] {
        let build_name = format!("linux-{target}-build.json");
        let build = record(dir, &build_name)?;
        source_record(&build, tag, &commit, &tree)?;
        pinned_toolchain(root, &build)?;
        exact(&build, "builder", "github-ubuntu-latest")?;
        exact(
            &build,
            "container",
            "ubuntu@sha256:08ea48a03a3e78ebc7cd526e6a275053223aadd88bfc09cc49b06d5281525fde",
        )?;
        exact(&build, "glibc_ceiling", "2.35")?;
        ensure!(
            build.get("checks_passed").and_then(Value::as_bool) == Some(true),
            "Linux checks did not pass"
        );
        let asset = archive(tag, target);
        asset_record(&build["artifact"], dir, &asset, target, mode)?;
        let bundle = format!("linux-{target}.intoto.jsonl");
        for name in [&asset, &build_name] {
            verify_attestation(root, dir, name, &bundle, tag, &commit)?;
        }
    }
    let docs = record(dir, "docs-build.json")?;
    source_record(&docs, tag, &commit, &tree)?;
    pinned_toolchain(root, &docs)?;
    exact(&docs, "builder", "github-ubuntu-latest")?;
    let docs_files = docs_files(tag);
    let doc_assets = docs["artifacts"]
        .as_object()
        .context("docs artifact hashes missing")?;
    ensure!(
        doc_assets.keys().cloned().collect::<BTreeSet<_>>() == docs_files,
        "docs record has an incomplete artifact set"
    );
    for name in &docs_files {
        ensure!(
            doc_assets[name].as_str() == Some(hash(dir, name)?.as_str()),
            "docs hash mismatch for {name}"
        );
        verify_attestation(root, dir, name, "docs.intoto.jsonl", tag, &commit)?;
        if name.ends_with(".tar.gz") {
            docs_archive(dir, name)?;
        }
    }
    verify_attestation(
        root,
        dir,
        "docs-build.json",
        "docs.intoto.jsonl",
        tag,
        &commit,
    )?;
    ensure!(
        fs::read(dir.join("compat.json"))? == fs::read(root.join("compat.json"))?
            && fs::read(dir.join("COMPAT.md"))? == fs::read(root.join("COMPAT.md"))?,
        "release compatibility files differ from the source tag"
    );
    ensure!(
        fs::read_to_string(dir.join("notes.md"))? == crate::release_notes::render(root, tag)?,
        "release notes differ from source pins"
    );
    let checksums = checksum_text(dir, &files)?;
    if write_checksums {
        fs::write(dir.join("SHA256SUMS"), checksums)?;
    } else {
        ensure!(
            fs::read_to_string(regular_file(dir, "SHA256SUMS")?)? == checksums,
            "SHA256SUMS is missing, incomplete, or stale"
        );
    }
    println!(
        "Verified all four targets, exact source tag, checksums, GitHub provenance and signed local Mac provenance for {tag}."
    );
    Ok(())
}

fn local_record(root: &Path, dir: &Path, tag: &str) -> Result<()> {
    let (commit, tree) = source(root, tag)?;
    ensure!(
        std::env::consts::OS == "macos" && std::env::consts::ARCH == "aarch64",
        "local Mac record must be produced on native Apple ARM macOS"
    );
    let mut checks = Vec::new();
    for name in MAC_CHECKS {
        let log = format!("macos-{name}.log");
        // The builder records this marker only after the command returns zero.
        ensure!(
            fs::read_to_string(regular_file(dir, &log)?)?.ends_with("\nRTAO_CHECK_EXIT=0\n"),
            "Mac check {name} has no success marker"
        );
        checks.push(json!({"name": name, "exit_code": 0, "log": log, "sha256": hash(dir, &log)?}));
    }
    let artifacts = [(TARGETS[2], "native"), (TARGETS[3], "cross-rosetta")]
        .into_iter()
        .map(|(target, mode)| {
            let name = archive(tag, target);
            archive_shape(dir, &name)?;
            Ok(json!({"name": name, "target": target, "mode": mode, "sha256": hash(dir, &name)?}))
        })
        .collect::<Result<Vec<_>>>()?;
    let build = json!({"schema": "rtao-build-v1", "tag": tag, "commit": commit, "tree": tree, "builder": "local-mac-studio", "host": "aarch64-apple-darwin", "rustc": output(root, "rustc", &["--version", "--verbose"] )?, "cargo": output(root, "cargo", &["--version"] )?, "macos": output(root, "sw_vers", &[])?, "sdk": output(root, "xcrun", &["--sdk", "macosx", "--show-sdk-version"] )?, "checks": checks, "artifacts": artifacts});
    let path = dir.join("macos-build.json");
    ensure!(
        !path.exists(),
        "local build record already exists; use a fresh task directory"
    );
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&build)?))?;
    println!(
        "Wrote an unsigned local build record. A trusted detached Ed25519 signature is required before release verification."
    );
    Ok(())
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace root")?;
    let mut args = args.into_iter();
    let command = args
        .next()
        .context("release needs verify or local-record")?;
    let mut values = BTreeMap::new();
    let mut write_checksums = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tag" | "--dir" if !values.contains_key(&arg) => {
                let value = args
                    .next()
                    .with_context(|| format!("{arg} needs a value"))?;
                values.insert(arg, value);
            }
            "--write-checksums" if command == "verify" && !write_checksums => {
                write_checksums = true
            }
            _ => bail!(
                "usage: cargo xtask release verify|local-record --tag rtao-vX.Y.Z --dir <assets> [--write-checksums]"
            ),
        }
    }
    let tag = values.get("--tag").context("--tag is required")?;
    let dir = PathBuf::from(values.get("--dir").context("--dir is required")?);
    ensure!(
        dir.is_dir() && !fs::symlink_metadata(&dir)?.file_type().is_symlink(),
        "release asset directory must be a regular directory"
    );
    let dir = dir.canonicalize()?;
    match command.as_str() {
        "verify" => verify(root, &dir, tag, write_checksums),
        "local-record" => local_record(root, &dir, tag),
        _ => bail!("unknown release command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_members(path: &str) -> Vec<&str> {
        let mut members = vec!["-czf", path, "rtao"];
        members.extend(LICENSES.iter().map(|license| license.name));
        members
    }

    fn docs_members(path: &str) -> Vec<&str> {
        let mut members = vec!["-czf", path, "index.html", "rtao"];
        members.extend(LICENSES.iter().map(|license| license.name));
        members
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = root.join(format!(
                "release-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // Only this exact newly-created fixture directory is removed. All
            // child processes have returned; it has no host/service bindings.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn local_mac_provenance_fails_without_an_approved_key() -> Result<()> {
        let scratch = Scratch::new()?;
        let error = verify_mac_signature(&scratch.0, &scratch.0)
            .err()
            .context("unapproved signer was accepted")?;
        assert!(error.to_string().contains("provenance is unresolved"));
        Ok(())
    }

    #[test]
    fn rejects_an_unpinned_toolchain() -> Result<()> {
        let scratch = Scratch::new()?;
        fs::write(
            scratch.0.join("rust-toolchain.toml"),
            "[toolchain]\nchannel='1.99.0'\n",
        )?;
        let build = json!({"rustc":"rustc 1.99.0 (hash date)\ncommit-hash: abc", "cargo":"cargo 1.99.0 (hash date)"});
        pinned_toolchain(&scratch.0, &build)?;
        let mut wrong = build.clone();
        wrong["rustc"] = json!("rustc 1.89.0 (hash date)\ncommit-hash: abc");
        assert!(pinned_toolchain(&scratch.0, &wrong).is_err());
        wrong = build;
        wrong["cargo"] = json!("cargo 1.89.0 (hash date)");
        assert!(pinned_toolchain(&scratch.0, &wrong).is_err());
        fs::write(
            scratch.0.join("rust-toolchain.toml"),
            "[toolchain]\nchannel='stable'\n",
        )?;
        assert!(pinned_toolchain(&scratch.0, &wrong).is_err());
        Ok(())
    }

    #[test]
    fn authenticates_local_record_and_rejects_tampering() -> Result<()> {
        let scratch = Scratch::new()?;
        let private = scratch.0.join("ephemeral-fixture-key");
        let private_text = private.to_str().context("fixture path")?;
        output(
            &scratch.0,
            "ssh-keygen",
            &["-q", "-t", "ed25519", "-N", "", "-f", private_text],
        )?;
        let public = fs::read_to_string(private.with_extension("pub"))?;
        let trusted = scratch.0.join("ci/release");
        fs::create_dir_all(&trusted)?;
        fs::write(
            trusted.join("allowed-signers"),
            format!("rtao-mac {public}"),
        )?;
        let record = scratch.0.join("macos-build.json");
        fs::write(&record, b"signed fixture record\n")?;
        output(
            &scratch.0,
            "ssh-keygen",
            &[
                "-Y",
                "sign",
                "-f",
                private_text,
                "-n",
                "rtao-release",
                record.to_str().context("fixture record path")?,
            ],
        )?;
        verify_mac_signature(&scratch.0, &scratch.0)?;
        fs::write(record, b"changed fixture record\n")?;
        assert!(verify_mac_signature(&scratch.0, &scratch.0).is_err());
        Ok(())
    }

    #[test]
    fn rejects_asset_tampering_and_archive_traversal() -> Result<()> {
        let scratch = Scratch::new()?;
        for license in &LICENSES {
            fs::write(scratch.0.join(license.name), license.bytes)?;
        }
        let binary = scratch.0.join("rtao");
        fs::write(&binary, b"test executable")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        }
        let name = archive("rtao-v0.1.0", TARGETS[2]);
        let path = scratch.0.join(&name);
        let path = path.to_str().context("fixture path")?;
        output(&scratch.0, "tar", &cli_members(path))?;
        let asset = json!({"name":name,"target":TARGETS[2],"mode":"native","sha256":hash(&scratch.0,&name)?});
        asset_record(&asset, &scratch.0, &name, TARGETS[2], "native")?;
        fs::write(&binary, b"tampered executable")?;
        output(&scratch.0, "tar", &cli_members(path))?;
        assert!(asset_record(&asset, &scratch.0, &name, TARGETS[2], "native").is_err());
        let parent_name = binary.to_str().context("fixture path")?;
        output(&scratch.0, "tar", &["-Pczf", path, parent_name])?;
        assert!(archive_shape(&scratch.0, &name).is_err());
        Ok(())
    }

    #[test]
    fn docs_archive_requires_facade_and_rejects_links() -> Result<()> {
        let scratch = Scratch::new()?;
        fs::create_dir(scratch.0.join("rtao"))?;
        fs::write(scratch.0.join("index.html"), "root index")?;
        fs::write(scratch.0.join("rtao/index.html"), "facade index")?;
        for license in &LICENSES {
            fs::write(scratch.0.join(license.name), license.bytes)?;
        }
        let name = "rustdoc-rtao-v0.1.0.tar.gz";
        let archive = scratch.0.join(name);
        let archive = archive.to_str().context("fixture path")?;
        let members = docs_members(archive);
        output(&scratch.0, "tar", &members)?;
        docs_archive(&scratch.0, name)?;
        for license in &LICENSES {
            let missing: Vec<_> = members
                .iter()
                .copied()
                .filter(|member| *member != license.name)
                .collect();
            output(&scratch.0, "tar", &missing)?;
            assert!(docs_archive(&scratch.0, name).is_err());
            let notice = scratch.0.join(license.name);
            let mut altered = license.bytes.to_vec();
            altered[0] ^= 1;
            fs::write(&notice, altered)?;
            output(&scratch.0, "tar", &members)?;
            assert!(docs_archive(&scratch.0, name).is_err());
            fs::write(&notice, vec![b'x'; license.bytes.len() + 1])?;
            output(&scratch.0, "tar", &members)?;
            assert!(docs_archive(&scratch.0, name).is_err());
            #[cfg(unix)]
            {
                fs::remove_file(&notice)?;
                let retained = scratch.0.join(format!("retained-{}", license.name));
                fs::write(&retained, license.bytes)?;
                std::os::unix::fs::symlink(&retained, &notice)?;
                output(&scratch.0, "tar", &members)?;
                assert!(docs_archive(&scratch.0, name).is_err());
                assert_eq!(fs::read(&retained)?, license.bytes);
                fs::remove_file(&notice)?;
            }
            fs::write(&notice, license.bytes)?;
        }
        output(&scratch.0, "tar", &["-czf", archive, "rtao"])?;
        assert!(docs_archive(&scratch.0, name).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("index.html", scratch.0.join("linked.html"))?;
            let mut linked = docs_members(archive);
            linked.push("linked.html");
            output(&scratch.0, "tar", &linked)?;
            assert!(docs_archive(&scratch.0, name).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_extra_assets_and_missing_targets() -> Result<()> {
        let scratch = Scratch::new()?;
        let files = ["required.tar.gz".to_owned()].into_iter().collect();
        assert!(check_inventory(&scratch.0, &files).is_err());
        fs::write(scratch.0.join("required.tar.gz"), "required")?;
        check_inventory(&scratch.0, &files)?;
        fs::write(scratch.0.join("extra.tar.gz"), "extra")?;
        assert!(check_inventory(&scratch.0, &files).is_err());
        Ok(())
    }

    #[test]
    fn release_requires_both_builders_and_all_four_targets() {
        let files = expected("rtao-v0.1.0");
        assert!(
            TARGETS
                .iter()
                .all(|target| files.contains(&archive("rtao-v0.1.0", target)))
        );
        assert!(files.contains("macos-build.json.sig"));
        assert!(files.contains("docs.intoto.jsonl"));
        let docs = docs_files("rtao-v0.1.0");
        for license in &LICENSES {
            assert!(files.contains(license.name));
            assert!(docs.contains(license.name));
        }
        assert!(docs.is_subset(&files));
        assert_eq!(
            files
                .iter()
                .filter(|file| file.ends_with(".intoto.jsonl"))
                .count(),
            3
        );
        assert!(!files.contains("SHA256SUMS"));
    }

    #[test]
    fn local_provenance_rejects_host_or_source_substitution() {
        let build = json!({"schema":"rtao-build-v1", "tag":"rtao-v0.1.0", "commit":"correct", "tree":"tree", "rustc":"rustc 1.99.0\ncommit-hash: abc", "cargo":"cargo 1.99.0"});
        assert!(source_record(&build, "rtao-v0.1.0", "correct", "tree").is_ok());
        assert!(source_record(&build, "rtao-v0.1.0", "different", "tree").is_err());
        assert!(source_record(&build, "rtao-v0.2.0", "correct", "tree").is_err());
        assert!(source_record(&build, "rtao-v0.1.0", "correct", "different").is_err());
    }

    #[test]
    fn license_sources_and_assets_are_regular_exact_bounded_and_in_checksums() -> Result<()> {
        let scratch = Scratch::new()?;
        let source = scratch.0.join("source");
        let assets = scratch.0.join("assets");
        fs::create_dir(&assets)?;
        for license in &LICENSES {
            let source_path = source.join(license.source);
            fs::create_dir_all(source_path.parent().context("fixture source parent")?)?;
            fs::write(source_path, license.bytes)?;
            fs::write(assets.join(license.name), license.bytes)?;
        }
        release_licenses(&source, &assets)?;
        let files = LICENSES
            .iter()
            .map(|license| license.name.to_owned())
            .collect();
        let checksums = checksum_text(&assets, &files)?;
        for license in &LICENSES {
            assert!(checksums.contains(&format!(
                "{:x}  {}\n",
                Sha256::digest(license.bytes),
                license.name
            )));
        }
        for license in &LICENSES {
            for path in [source.join(license.source), assets.join(license.name)] {
                fs::remove_file(&path)?;
                assert!(release_licenses(&source, &assets).is_err());
                let mut altered = license.bytes.to_vec();
                altered[0] ^= 1;
                fs::write(&path, altered)?;
                assert!(release_licenses(&source, &assets).is_err());
                fs::write(&path, vec![b'x'; license.bytes.len() + 1])?;
                assert!(release_licenses(&source, &assets).is_err());
                fs::write(&path, license.bytes)?;
                release_licenses(&source, &assets)?;
                #[cfg(unix)]
                {
                    fs::remove_file(&path)?;
                    let retained = path.with_file_name(format!("retained-{}", license.name));
                    fs::write(&retained, license.bytes)?;
                    std::os::unix::fs::symlink(&retained, &path)?;
                    assert!(release_licenses(&source, &assets).is_err());
                    assert_eq!(fs::read(&retained)?, license.bytes);
                    fs::remove_file(&path)?;
                    fs::write(&path, license.bytes)?;
                }
            }
        }
        release_licenses(&source, &assets)?;
        let notice = LICENSES
            .iter()
            .find(|license| license.name == "NOTICE")
            .context("NOTICE missing")?;
        let mut changed = notice.bytes.to_vec();
        changed[0] ^= 1;
        fs::write(source.join(notice.source), &changed)?;
        fs::write(assets.join(notice.name), &changed)?;
        assert!(release_licenses(&source, &assets).is_err());
        fs::write(source.join(notice.source), notice.bytes)?;
        fs::write(assets.join(notice.name), notice.bytes)?;
        Ok(())
    }

    #[test]
    fn archive_requires_all_six_notices_and_rejects_modified_oversized_and_linked_notices()
    -> Result<()> {
        let scratch = Scratch::new()?;
        let name = archive("rtao-v0.1.0", TARGETS[0]);
        let path = scratch.0.join(&name);
        let path = path.to_str().context("fixture archive path")?;
        fs::write(scratch.0.join("rtao"), b"synthetic binary")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(scratch.0.join("rtao"), fs::Permissions::from_mode(0o755))?;
        }
        output(&scratch.0, "tar", &["-czf", path, "rtao"])?;
        assert!(archive_shape(&scratch.0, &name).is_err());
        for license in &LICENSES {
            fs::write(scratch.0.join(license.name), license.bytes)?;
        }
        let members = cli_members(path);
        output(&scratch.0, "tar", &members)?;
        archive_shape(&scratch.0, &name)?;
        for license in &LICENSES {
            let missing: Vec<_> = members
                .iter()
                .copied()
                .filter(|member| *member != license.name)
                .collect();
            output(&scratch.0, "tar", &missing)?;
            assert!(archive_shape(&scratch.0, &name).is_err());
            let notice = scratch.0.join(license.name);
            let mut altered = license.bytes.to_vec();
            altered[0] ^= 1;
            fs::write(&notice, altered)?;
            output(&scratch.0, "tar", &members)?;
            assert!(archive_shape(&scratch.0, &name).is_err());
            fs::write(&notice, vec![b'x'; license.bytes.len() + 1])?;
            output(&scratch.0, "tar", &members)?;
            assert!(archive_shape(&scratch.0, &name).is_err());
            #[cfg(unix)]
            {
                fs::remove_file(&notice)?;
                let retained = scratch.0.join(format!("retained-{}", license.name));
                fs::write(&retained, license.bytes)?;
                std::os::unix::fs::symlink(&retained, &notice)?;
                output(&scratch.0, "tar", &members)?;
                assert!(archive_shape(&scratch.0, &name).is_err());
                assert_eq!(fs::read(&retained)?, license.bytes);
                fs::remove_file(&notice)?;
            }
            fs::write(&notice, license.bytes)?;
        }
        output(&scratch.0, "tar", &members)?;
        archive_shape(&scratch.0, &name)?;
        Ok(())
    }
}
