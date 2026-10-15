//! Verified upstream pins and finalized-release provenance.

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use blake2::{
    Blake2b,
    digest::{Digest, consts::U32},
};
use rtao_core::dependency_policy::{LockedPackage, validate_bls_lock, validate_bls_tree};
use serde_json::{Value as Json, json};
use sha2::Sha256;
use toml_edit::{DocumentMut, Item, Table, Value, value};

const UPSTREAM: &str = "https://github.com/RaoFoundation/subtensor.git";
const REPOSITORY: &str = "RaoFoundation/subtensor";
const IMAGE: &str = "ghcr.io/raofoundation/subtensor-localnet";
const FINNEY: &str = "https://entrypoint-finney.opentensor.ai:443";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    check: bool,
    networks: bool,
    parity: Option<String>,
    github_output: Option<PathBuf>,
    core_branch: Option<String>,
    no_commit: bool,
    force: bool,
    plan: bool,
    dry_run: bool,
    prepare_candidate: bool,
    resolve_only: bool,
}

fn options(args: Vec<String>) -> Result<Options> {
    let mut result = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => result.check = true,
            "--networks" => {
                ensure!(!result.networks, "duplicate --networks");
                result.networks = true;
            }
            "--parity" => {
                ensure!(result.parity.is_none(), "duplicate --parity");
                result.parity = Some(args.next().context("--parity needs a tag or auto")?);
            }
            "--github-output" => {
                result.github_output =
                    Some(args.next().context("--github-output needs a path")?.into())
            }
            "--core-branch" => {
                ensure!(result.core_branch.is_none(), "duplicate --core-branch");
                result.core_branch = Some(args.next().context("--core-branch needs main")?);
            }
            "--no-commit" => {
                ensure!(!result.no_commit, "duplicate --no-commit");
                result.no_commit = true;
            }
            "--force" if !result.force => result.force = true,
            "--plan" if !result.plan => result.plan = true,
            "--dry-run" if !result.dry_run => result.dry_run = true,
            "--prepare-candidate" if !result.prepare_candidate => result.prepare_candidate = true,
            "--resolve-only" if !result.resolve_only => result.resolve_only = true,
            _ => bail!("unknown pin argument {arg:?}"),
        }
    }
    ensure!(
        result.check
            || result.parity.is_some()
            || result.networks
            || result.core_branch.is_some()
            || result.force,
        "pin needs --check, --networks, --parity <vN|auto>, or --core-branch main --no-commit"
    );
    ensure!(
        !result.networks || (result.parity.is_none() && result.github_output.is_none()),
        "--networks cannot be combined with parity or GitHub output"
    );
    ensure!(
        result.github_output.is_none() || result.parity.is_some(),
        "--github-output requires --parity"
    );
    ensure!(
        result.core_branch.is_none()
            || (result.no_commit
                && !result.check
                && !result.networks
                && result.parity.is_none()
                && result.github_output.is_none()),
        "--core-branch requires --no-commit and cannot be combined with other pin modes"
    );
    ensure!(
        result.core_branch.is_some() || !result.no_commit,
        "--no-commit requires --core-branch"
    );
    ensure!(
        !(result.force || result.plan || result.dry_run || result.prepare_candidate)
            || (!result.check
                && !result.networks
                && result.core_branch.is_none()
                && (result.parity.is_some() || result.force)),
        "pipeline flags require --parity or --force and exclude checking/network/canary modes"
    );
    ensure!(
        !result.prepare_candidate || (!result.force && !result.plan && !result.dry_run),
        "--prepare-candidate cannot be combined with replay, plan, or dry-run"
    );
    ensure!(
        !result.resolve_only || (result.check && result.parity.is_some()),
        "--resolve-only requires --parity <vN|auto> --check"
    );
    Ok(result)
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let opts = options(args)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no workspace parent")?;
    let pin_path = root.join("upstream.toml");
    let current = if pin_path.exists() {
        Some(read_document(&pin_path)?)
    } else {
        None
    };
    if let Some(pins) = &current {
        verify_schema(pins)?;
        ensure!(
            !(opts.force || opts.plan || opts.dry_run || opts.prepare_candidate)
                || pins["core"].get("canary").is_none(),
            "release pipeline requires production release pins"
        );
    }
    if !opts.check && (opts.parity.is_some() || opts.force) && !opts.prepare_candidate {
        let current = current
            .as_ref()
            .context("bootstrap release pins before running the candidate pipeline")?;
        let pinned = pin_string(current, "parity", "tag")?;
        let requested = opts.parity.as_deref().unwrap_or(pinned);
        ensure!(
            !opts.force || requested == pinned,
            "--force must replay the committed parity tag {pinned}"
        );
        return crate::pin_pipeline::run(
            root,
            requested,
            opts.force,
            opts.plan,
            opts.dry_run,
            opts.github_output.as_deref(),
        );
    }
    if opts.prepare_candidate {
        crate::pin_pipeline::require_candidate(root)?;
    }
    if let Some(branch) = &opts.core_branch {
        crate::core_canary::run(
            root,
            current
                .as_ref()
                .context("core canary needs existing release pins")?,
            branch,
        )?;
        // The disposable canary follows the current :main image, while release
        // and parity image provenance stays pinned to the production release.
        let mut canary = read_document(&pin_path)?;
        canary["localnet"]["main"]["digest"] = value(image_digest("main")?);
        atomic_write(&pin_path, canary.to_string().as_bytes())?;
        return Ok(());
    }
    if current
        .as_ref()
        .is_some_and(|pins| pins["core"].get("canary").is_some())
    {
        ensure!(
            opts.check && !opts.networks && opts.parity.is_none(),
            "a disposable canary accepts only pin --check; use a release checkout for production pin modes"
        );
        let pins = current.as_ref().context("canary pins missing")?;
        let repo = cached_repo(root)?;
        verify_pin_identity(&repo, pins)?;
        crate::core_canary::verify(root, pins)?;
        crate::metadata_pin::verify(root, pins)?;
        crate::network_pin::verify(root, pins)?;
        println!(
            "Disposable core canary commit, fingerprint, manifest and lock verified; retained production release/parity provenance verified."
        );
        return Ok(());
    }
    if opts.networks {
        return crate::network_pin::run(
            root,
            current
                .as_ref()
                .context("generate the initial upstream pins first")?,
            opts.check,
            rpc_at,
        );
    }
    let repo = cached_repo(root)?;
    if let Some(parity) = &opts.parity {
        let proof = release_proof(&repo, parity)?;
        let mut next = current.clone().unwrap_or_default();
        if current.is_none() {
            for section in ["monorepo", "core", "parity", "localnet", "networks"] {
                next[section] = Item::Table(Table::new());
            }
            next["networks"]["finney"] = Item::Table(Table::new());
            next["networks"]["finney"]["metadata"] = Item::Table(Table::new());
        }
        // A requested parity check may fetch immutable source dependencies;
        // ordinary --check performs no network resolution beyond cached Git.
        crate::fingerprint::prepare_source_cache(&repo, &proof.commit)?;
        let fingerprint = crate::fingerprint::fingerprint(&repo, &proof.commit)?;
        let core_moved = current
            .as_ref()
            .map(|d| d["core"]["fingerprint"].as_str() != Some(&fingerprint))
            .unwrap_or(true);
        if opts.resolve_only {
            if let Some(output) = opts.github_output {
                let newer = current
                    .as_ref()
                    .and_then(|pins| pins["parity"]["spec_version"].as_integer())
                    .is_none_or(|previous| i64::from(proof.spec) > previous);
                writeln!(
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(output)?,
                    "tag={}\ndigest={}\nnew={newer}\ncore_bump={core_moved}",
                    proof.tag,
                    proof.digest
                )?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"tag":proof.tag,"commit":proof.commit,"digest":proof.digest,"core_bump":core_moved,"pins_modified":false})
                )?
            );
            return Ok(());
        }
        if current.is_some() && opts.check {
            // The polling form proposes a release and refreshes digests. It never
            // silently bumps the already committed core or parity pins.
            ensure!(
                next["localnet"]["image"].as_str() == Some(IMAGE),
                "unexpected localnet image repository"
            );
        } else {
            next["monorepo"]["url"] = value(UPSTREAM);
            if core_moved {
                ensure!(
                    next.get("core")
                        .and_then(|core| core.get("patch"))
                        .is_none(),
                    "core-patch-conflict: core patch requires core-patch rebase before moving the core pin"
                );
                next["core"]["tag"] = value(&proof.tag);
                next["core"]["commit"] = value(&proof.commit);
                next["core"]["toolchain"] = value(toolchain(&repo, &proof.commit)?);
                next["core"]["fingerprint"] = value(fingerprint);
            }
            next["parity"]["tag"] = value(&proof.tag);
            next["parity"]["commit"] = value(&proof.commit);
            next["parity"]["spec_version"] = value(i64::from(proof.spec));
            next["parity"]["python_package_version"] = value(python_version(&repo, &proof.commit)?);
            next["localnet"]["image"] = value(IMAGE);
            next["networks"]["finney"]["genesis"] = value(&proof.genesis);
            next["networks"]["finney"]["metadata"]["spec"] = value(i64::from(proof.spec));
            next["networks"]["finney"]["metadata"]["code_hash"] = value(&proof.code_hash);
            next["networks"]["finney"]["metadata"]["finalized_head"] = value(&proof.finalized_head);
            if current
                .as_ref()
                .is_none_or(|old| old["parity"]["tag"].as_str() != Some(proof.tag.as_str()))
            {
                seed_artifacts(&repo, &proof.commit, &mut next)?;
            }
        }
        for (lane, tag) in [
            ("pinned", proof.tag.as_str()),
            ("mainnet", "mainnet"),
            ("testnet", "testnet"),
            ("devnet", "devnet"),
            ("main", "main"),
        ] {
            let digest = if lane == "pinned" {
                proof.digest.clone()
            } else {
                image_digest(tag)?
            };
            next["localnet"][lane]["tag"] = value(tag);
            next["localnet"][lane]["digest"] = value(digest);
        }
        // For a newer candidate, leave the committed pinned image intact. The
        // candidate digest is returned to the pin-bump workflow through outputs.
        if opts.check
            && let Some(old) = &current
            && old["parity"]["tag"].as_str() != Some(proof.tag.as_str())
        {
            next["localnet"]["pinned"] = old["localnet"]["pinned"].clone();
        }
        if opts.prepare_candidate && core_moved {
            rewrite_core_references(
                root,
                current
                    .as_ref()
                    .context("candidate requires previous core pin")?,
                &next,
            )?;
        }
        let metadata = if current.is_none() || !opts.check {
            let endpoint = std::env::var("RTAO_FINNEY_HTTP").unwrap_or_else(|_| FINNEY.to_owned());
            let prepared = crate::metadata_pin::prepare(root, &next, &endpoint, rpc)?;
            prepared.update_pins(&mut next);
            Some(prepared)
        } else {
            crate::metadata_pin::verify(root, &next)?;
            None
        };
        verify_pin_identity(&repo, &next)?;
        verify_manifest(root, &next)?;
        if current.is_none() || !opts.check {
            record_oracle_lock(root, &repo, &proof.commit)?;
        }
        if let Some(metadata) = metadata {
            metadata.publish(root)?;
        }
        crate::metadata_pin::verify(root, &next)?;
        let serialized = next.to_string();
        let generated =
            serialized.trim_start_matches("# Generated by cargo xtask pin; do not edit by hand.\n");
        atomic_write(
            &pin_path,
            format!("# Generated by cargo xtask pin; do not edit by hand.\n{generated}").as_bytes(),
        )?;
        if opts.prepare_candidate {
            crate::pin_pipeline::prune_imports(
                root,
                current
                    .as_ref()
                    .context("previous artifact inventory missing")?,
            )?;
        }
        if let Some(output) = opts.github_output {
            let newer = current
                .as_ref()
                .and_then(|d| d["parity"]["spec_version"].as_integer())
                .map(|n| i64::from(proof.spec) > n)
                .unwrap_or(true);
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(output)?;
            writeln!(
                file,
                "tag={}\ndigest={}\nnew={newer}\ncore_bump={core_moved}",
                proof.tag, proof.digest
            )?;
        }
        println!(
            "Verified {} ({}) on finalized finney {} with code {}; pinned image {}@{}",
            proof.tag, proof.commit, proof.finalized_head, proof.code_hash, IMAGE, proof.digest
        );
    } else {
        let current = current
            .context("upstream.toml is missing; bootstrap with pin --parity auto --check")?;
        verify_pin_identity(&repo, &current)?;
        verify_manifest(root, &current)?;
        crate::metadata_pin::verify(root, &current)?;
        crate::network_pin::verify(root, &current)?;
        verify_lock(root, &current)?;
        println!(
            "Committed core and parity pins, manifests, fingerprint, metadata and Cargo.lock verified"
        );
    }
    Ok(())
}

pub(crate) fn read_document(path: &Path) -> Result<DocumentMut> {
    fs::read_to_string(path)?
        .parse()
        .with_context(|| format!("parse {}", path.display()))
}

pub(super) fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn git_string(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(repo, args)?)?.trim().to_owned())
}

pub(crate) fn cached_repo(root: &Path) -> Result<PathBuf> {
    let path = std::env::var_os("RTAO_UPSTREAM_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .map(|p| p.join("pin/upstream.git"))
                .unwrap_or_else(|| root.join("target/upstream.git"))
        });
    if !path.exists() {
        fs::create_dir_all(path.parent().context("cache has no parent")?)?;
        let output = Command::new("git")
            .args(["clone", "--filter=blob:none", "--no-checkout", UPSTREAM])
            .arg(&path)
            .output()?;
        ensure!(
            output.status.success(),
            "partial clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    ensure!(
        git_string(&path, &["remote", "get-url", "origin"])? == UPSTREAM,
        "upstream cache has an unexpected origin"
    );
    Ok(path)
}

fn resolve_tag(repo: &Path, tag: &str) -> Result<String> {
    release_spec(tag)?;
    let reference = format!("refs/tags/{tag}^{{commit}}");
    if git_string(repo, &["rev-parse", "--verify", &reference]).is_err() {
        let refspec = format!("refs/tags/{tag}:refs/tags/{tag}");
        git(repo, &["fetch", "--filter=blob:none", "origin", &refspec])?;
    }
    let commit = git_string(repo, &["rev-parse", "--verify", &reference])?;
    ensure!(hex_string(&commit, 40), "invalid Git commit for {tag}");
    Ok(commit)
}

fn release_spec(tag: &str) -> Result<u32> {
    let suffix = tag
        .strip_prefix('v')
        .context("release tags must have form vN")?;
    ensure!(
        !suffix.is_empty()
            && suffix.bytes().all(|b| b.is_ascii_digit())
            && !suffix.starts_with('0'),
        "release tags must have canonical form vN"
    );
    suffix
        .parse()
        .context("release spec version is out of range")
}

fn hex_string(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn prefixed_hash(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|s| hex_string(s, 64))
}

fn http(
    url: &str,
    headers: &[String],
    body: Option<&str>,
    include_headers: bool,
) -> Result<Vec<u8>> {
    ensure!(
        url.starts_with("https://"),
        "only HTTPS upstream requests are allowed"
    );
    let mut command = Command::new("curl");
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        "60",
        "--retry",
        "2",
    ]);
    if include_headers {
        command.arg("--include");
    }
    for header in headers {
        command.args(["--header", header]);
    }
    if let Some(body) = body {
        command.args(["--data", body]);
    }
    let output = command.arg(url).output()?;
    ensure!(
        output.status.success(),
        "HTTPS request to {url} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn github(path: &str) -> Result<Json> {
    let output = Command::new("gh")
        .args(["api", "--method", "GET", path])
        .output()
        .context("GitHub reads require gh (or install the GitHub CLI)")?;
    ensure!(
        output.status.success(),
        "GitHub read {path} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).context("invalid GitHub JSON")
}

fn json_string<'a>(object: &'a Json, key: &str) -> Result<&'a str> {
    object[key]
        .as_str()
        .with_context(|| format!("missing JSON string {key}"))
}

fn rpc(method: &str, params: Json) -> Result<Json> {
    let endpoint = std::env::var("RTAO_FINNEY_HTTP").unwrap_or_else(|_| FINNEY.to_owned());
    rpc_at(&endpoint, method, params)
}

fn rpc_at(endpoint: &str, method: &str, params: Json) -> Result<Json> {
    let response: Json = serde_json::from_slice(&http(
        endpoint,
        &["Content-Type: application/json".to_owned()],
        Some(&json!({"id": 1, "jsonrpc": "2.0", "method": method, "params": params}).to_string()),
        false,
    )?)?;
    ensure!(
        response.get("error").is_none(),
        "network {method}: {}",
        response["error"]
    );
    response
        .get("result")
        .filter(|r| !r.is_null())
        .cloned()
        .with_context(|| format!("network {method} returned no result"))
}

struct ReleaseProof {
    tag: String,
    commit: String,
    spec: u32,
    digest: String,
    genesis: String,
    finalized_head: String,
    code_hash: String,
}

fn validate_release(
    release: &Json,
    tag: &str,
    commit: &str,
    manifest: &Json,
    digest: &Json,
    wasm: &[u8],
) -> Result<String> {
    ensure!(
        release["draft"].as_bool() == Some(false) && release["prerelease"].as_bool() == Some(false),
        "{tag} is not a final release"
    );
    ensure!(
        release["tag_name"].as_str() == Some(tag),
        "release tag differs from requested tag"
    );
    ensure!(
        manifest["tag"].as_str() == Some(tag) && manifest["commit"].as_str() == Some(commit),
        "upgrade manifest does not identify the immutable tag commit"
    );
    ensure!(
        manifest["repo"].as_str() == Some(REPOSITORY)
            && manifest["network"].as_str() == Some("finney"),
        "upgrade manifest has unexpected repository or network"
    );
    ensure!(
        manifest["spec_version"].as_u64() == Some(u64::from(release_spec(tag)?)),
        "upgrade manifest spec differs from release"
    );
    ensure!(
        digest["commit"].as_str() == Some(commit),
        "srtool digest commit differs from release tag"
    );
    let sha256 = format!("0x{:x}", Sha256::digest(wasm));
    ensure!(
        manifest["wasm_sha256"].as_str() == Some(&sha256)
            && digest["sha256"].as_str() == Some(&sha256),
        "release wasm sha256 differs from manifest or srtool digest"
    );
    let hash = format!("0x{:x}", Blake2b::<U32>::digest(wasm));
    ensure!(
        digest["runtimes"]["compressed"]["blake2_256"].as_str() == Some(&hash),
        "release wasm BLAKE2b-256 differs from srtool digest"
    );
    ensure!(
        digest["runtimes"]["compressed"]["subwasm"]["core_version"]["specVersion"].as_u64()
            == Some(u64::from(release_spec(tag)?)),
        "release wasm spec differs from requested release"
    );
    Ok(hash)
}

fn release_proof(repo: &Path, requested: &str) -> Result<ReleaseProof> {
    let release = github(&format!(
        "repos/{REPOSITORY}/releases/{}",
        if requested == "auto" {
            "latest".to_owned()
        } else {
            format!("tags/{requested}")
        }
    ))?;
    let tag = json_string(&release, "tag_name")?.to_owned();
    ensure!(
        requested == "auto" || requested == tag,
        "GitHub returned a different release"
    );
    let spec = release_spec(&tag)?;
    let commit = resolve_tag(repo, &tag)?;
    let tag_ref = github(&format!("repos/{REPOSITORY}/git/ref/tags/{tag}"))?;
    ensure!(
        tag_ref["object"]["type"].as_str() == Some("commit")
            && tag_ref["object"]["sha"].as_str() == Some(&commit),
        "release tag does not match authoritative immutable commit"
    );
    let assets = release["assets"]
        .as_array()
        .context("release has no assets")?;
    let asset = |name: &str| -> Result<Vec<u8>> {
        let candidates: Vec<_> = assets
            .iter()
            .filter(|a| a["name"].as_str() == Some(name))
            .collect();
        ensure!(
            candidates.len() == 1,
            "release needs exactly one {name} asset"
        );
        let url = json_string(candidates[0], "browser_download_url")?;
        ensure!(
            url == format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"),
            "unexpected release asset URL"
        );
        http(url, &[], None, false)
    };
    let manifest: Json = serde_json::from_slice(&asset("upgrade-manifest.json")?)?;
    let srtool: Json = serde_json::from_slice(&asset("subtensor-digest.json")?)?;
    let expected_hash = validate_release(
        &release,
        &tag,
        &commit,
        &manifest,
        &srtool,
        &asset("subtensor.wasm")?,
    )?;
    // Keep every runtime read at the same finalized block, as upstream's
    // mainnet-finalized-runtime.sh does; a best-head version is insufficient.
    let finalized_head = rpc("chain_getFinalizedHead", json!([]))?
        .as_str()
        .context("invalid finalized head")?
        .to_owned();
    ensure!(
        prefixed_hash(&finalized_head, "0x"),
        "invalid finalized block hash"
    );
    let runtime = rpc("state_getRuntimeVersion", json!([finalized_head]))?;
    ensure!(
        runtime["specVersion"].as_u64() == Some(u64::from(spec)),
        "finney finalized spec {} differs from requested {spec}",
        runtime["specVersion"]
    );
    let code_hash = rpc(
        "state_getStorageHash",
        json!(["0x3a636f6465", finalized_head]),
    )?
    .as_str()
    .context("invalid runtime code hash")?
    .to_owned();
    ensure!(
        code_hash == expected_hash,
        "finney finalized :code hash {code_hash} differs from release wasm {expected_hash}"
    );
    let genesis = rpc("chain_getBlockHash", json!([0]))?
        .as_str()
        .context("invalid genesis")?
        .to_owned();
    ensure!(
        genesis == "0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03",
        "endpoint is not finney"
    );
    let digest = image_digest(&tag)?;
    Ok(ReleaseProof {
        tag,
        commit,
        spec,
        digest,
        genesis,
        finalized_head,
        code_hash,
    })
}

fn image_digest(tag: &str) -> Result<String> {
    ensure!(
        !tag.is_empty()
            && tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "invalid image tag"
    );
    let token: Json = serde_json::from_slice(&http(
        "https://ghcr.io/token?scope=repository%3Araofoundation%2Fsubtensor-localnet%3Apull",
        &[],
        None,
        false,
    )?)?;
    let token = token["token"]
        .as_str()
        .or_else(|| token["access_token"].as_str())
        .context("GHCR supplied no anonymous pull token")?;
    let raw = http(&format!("https://ghcr.io/v2/raofoundation/subtensor-localnet/manifests/{tag}"), &[format!("Authorization: Bearer {token}"), "Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json".to_owned()], None, true)?;
    parse_registry_response(&raw)
}

fn parse_registry_response(raw: &[u8]) -> Result<String> {
    // curl may emit an HTTP proxy's CONNECT headers before origin headers.
    // Skip only successive HTTP header blocks, preserving exact manifest bytes.
    let mut remaining = raw;
    let mut digest = None;
    while remaining.starts_with(b"HTTP/") {
        let end = remaining
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .context("GHCR response lacks a header terminator")?;
        let headers = std::str::from_utf8(&remaining[..end])?;
        digest = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case("docker-content-digest"))
            .map(|(_, value)| value.trim().to_owned());
        remaining = &remaining[end + 4..];
    }
    let digest = digest.context("GHCR response lacks Docker-Content-Digest")?;
    ensure!(prefixed_hash(&digest, "sha256:"), "invalid GHCR digest");
    ensure!(
        digest == format!("sha256:{:x}", Sha256::digest(remaining)),
        "GHCR manifest bytes do not match the returned digest"
    );
    let manifest: Json = serde_json::from_slice(remaining)?;
    ensure!(
        manifest.get("schemaVersion").and_then(Json::as_u64) == Some(2),
        "unsupported registry manifest schema"
    );
    Ok(digest)
}

pub(crate) fn toolchain(repo: &Path, commit: &str) -> Result<String> {
    let doc: DocumentMut = String::from_utf8(git(
        repo,
        &["show", &format!("{commit}:rust-toolchain.toml")],
    )?)?
    .parse()?;
    Ok(doc["toolchain"]["channel"]
        .as_str()
        .context("upstream toolchain has no channel")?
        .to_owned())
}

fn python_version(repo: &Path, commit: &str) -> Result<String> {
    let doc: DocumentMut = String::from_utf8(git(
        repo,
        &["show", &format!("{commit}:sdk/python/pyproject.toml")],
    )?)?
    .parse()?;
    Ok(doc["project"]["version"]
        .as_str()
        .context("upstream Python package has no source version")?
        .to_owned())
}

fn seed_artifacts(repo: &Path, commit: &str, pins: &mut DocumentMut) -> Result<()> {
    let mut paths = BTreeMap::from([
        (
            "website/apps/bittensor-website/public/catalog/intents.json".to_owned(),
            "vendor/upstream/catalog/intents.json".to_owned(),
        ),
        (
            "website/apps/bittensor-website/public/catalog/reads.json".to_owned(),
            "vendor/upstream/catalog/reads.json".to_owned(),
        ),
        (
            "website/apps/bittensor-website/public/catalog/errors.json".to_owned(),
            "vendor/upstream/catalog/errors.json".to_owned(),
        ),
        (
            "sdk/python/tests/fixtures/golden.json".to_owned(),
            "vendor/upstream/golden.json".to_owned(),
        ),
        (
            "sdk/python/tests/fixtures/shape_corpus/corpus.json".to_owned(),
            "vendor/upstream/corpus.json".to_owned(),
        ),
        (
            "sdk/bittensor-core/fixtures/ledger_proof_vector.json".to_owned(),
            "vendor/upstream/ledger_proof_vector.json".to_owned(),
        ),
        (
            "sdk/bittensor-core/tests/e2e-manifest.json".to_owned(),
            "vendor/upstream/e2e-manifest.json".to_owned(),
        ),
        (
            "docs/guides/signed-requests.mdx".to_owned(),
            "vendor/upstream/signed-requests.mdx".to_owned(),
        ),
    ]);
    let static_files = git_string(
        repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            commit,
            "--",
            "sdk/python/bittensor/vault/static",
            "sdk/python/bittensor/extension/static",
            "sdk/python/bittensor/evm/abi",
        ],
    )?;
    for path in static_files.lines() {
        let suffix = path
            .strip_prefix("sdk/python/bittensor/")
            .context("unexpected static source path")?;
        paths.insert(path.to_owned(), format!("vendor/upstream/{suffix}"));
    }
    let mut artifacts = Table::new();
    for (source, destination) in paths {
        let bytes = git(repo, &["show", &format!("{commit}:{source}")])?;
        let mut row = toml_edit::InlineTable::new();
        row.insert("to", Value::from(destination));
        row.insert(
            "sha256",
            Value::from(format!("{:x}", Sha256::digest(&bytes))),
        );
        artifacts[&source] = Item::Value(Value::InlineTable(row));
    }
    // Extraction/source-input rows are subsequently added by import-upstream.
    pins["artifacts"] = Item::Table(artifacts);
    Ok(())
}

fn record_oracle_lock(root: &Path, repo: &Path, commit: &str) -> Result<()> {
    let packages: Vec<_> = crate::fingerprint::resolved_graph(repo, commit)?
        .into_iter()
        .filter(|p| {
            p.name.starts_with("ark-")
                || [
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
                ]
                .contains(&p.name.as_str())
        })
        .map(|p| json!({"name": p.name, "version": p.version, "source": p.source}))
        .collect();
    let path = root.join("vendor/oracle/lock.json");
    fs::create_dir_all(path.parent().context("oracle lock has no parent")?)?;
    let mut bytes =
        serde_json::to_vec_pretty(&json!({"upstream_commit": commit, "packages": packages}))?;
    bytes.push(b'\n');
    // A generated, retained oracle provenance artifact; no temporary checkout.
    fs::write(path, bytes)?;
    Ok(())
}

fn rewrite_core_references(root: &Path, previous: &DocumentMut, next: &DocumentMut) -> Result<()> {
    let old = pin_string(previous, "core", "tag")?;
    let tag = pin_string(next, "core", "tag")?;
    for path in crate::pin_pipeline::source_files(root)? {
        let relative = path.to_string_lossy();
        if !(relative == "Cargo.toml"
            || relative.ends_with("Cargo.toml")
            || relative.ends_with(".md")
            || relative.ends_with(".rs"))
        {
            continue;
        }
        if relative.starts_with("vendor/")
            || relative.starts_with("docs/plan/")
            || relative == "PLAN.md"
        {
            continue;
        }
        let absolute = root.join(&path);
        let text = fs::read_to_string(&absolute)?;
        let rewritten = text.replace(
            &format!("git = \"{UPSTREAM}\", tag = \"{old}\""),
            &format!("git = \"{UPSTREAM}\", tag = \"{tag}\""),
        );
        if rewritten != text {
            fs::write(absolute, rewritten)?;
        }
    }
    Ok(())
}

fn pin_string<'a>(pins: &'a DocumentMut, section: &str, key: &str) -> Result<&'a str> {
    item_at(pins, &[section, key])?
        .as_str()
        .with_context(|| format!("upstream.toml is missing [{section}].{key}"))
}

fn item_at<'a>(doc: &'a DocumentMut, path: &[&str]) -> Result<&'a Item> {
    let mut item = doc.as_item();
    for key in path {
        item = item
            .get(key)
            .with_context(|| format!("missing TOML field {}", path.join(".")))?;
    }
    Ok(item)
}

fn verify_schema(pins: &DocumentMut) -> Result<()> {
    for table in [
        "monorepo",
        "core",
        "parity",
        "localnet",
        "networks",
        "artifacts",
    ] {
        ensure!(
            item_at(pins, &[table])?.is_table(),
            "{table} must be a TOML table"
        );
    }
    for (section, fields) in [
        ("monorepo", &["url"][..]),
        ("core", &["tag", "commit", "toolchain", "fingerprint"][..]),
        ("parity", &["tag", "commit", "python_package_version"][..]),
        ("localnet", &["image"][..]),
    ] {
        for key in fields {
            pin_string(pins, section, key)?;
        }
    }
    ensure!(
        item_at(pins, &["parity", "spec_version"])?
            .as_integer()
            .is_some(),
        "parity.spec_version must be an integer"
    );
    ensure!(
        item_at(pins, &["networks", "finney", "genesis"])?
            .as_str()
            .is_some(),
        "finney genesis must be a string"
    );
    for lane in ["pinned", "mainnet", "testnet", "devnet", "main"] {
        for key in ["tag", "digest"] {
            ensure!(
                item_at(pins, &["localnet", lane, key])?.as_str().is_some(),
                "localnet.{lane}.{key} must be a string"
            );
        }
    }
    Ok(())
}

fn verify_pin_identity(repo: &Path, pins: &DocumentMut) -> Result<()> {
    verify_schema(pins)?;
    ensure!(
        pin_string(pins, "monorepo", "url")? == UPSTREAM,
        "unsupported monorepo source"
    );
    for section in ["core", "parity"] {
        let tag = pin_string(pins, section, "tag")?;
        ensure!(
            resolve_tag(repo, tag)? == pin_string(pins, section, "commit")?,
            "[{section}] commit differs from immutable tag"
        );
    }
    let core = pin_string(pins, "core", "commit")?;
    ensure!(
        crate::fingerprint::fingerprint(repo, core)? == pin_string(pins, "core", "fingerprint")?,
        "core-input fingerprint changed"
    );
    ensure!(
        toolchain(repo, core)? == pin_string(pins, "core", "toolchain")?,
        "core toolchain pin differs from source"
    );
    let parity = pin_string(pins, "parity", "commit")?;
    ensure!(
        crate::fingerprint::fingerprint(repo, parity)? == pin_string(pins, "core", "fingerprint")?,
        "parity core inputs differ from core pin; move core.tag"
    );
    ensure!(
        python_version(repo, parity)? == pin_string(pins, "parity", "python_package_version")?,
        "Python source version pin differs from source"
    );
    ensure!(
        pins["parity"]["spec_version"].as_integer()
            == Some(i64::from(release_spec(pin_string(pins, "parity", "tag")?)?)),
        "parity spec differs from tag"
    );
    ensure!(
        pins["localnet"]["image"].as_str() == Some(IMAGE),
        "unexpected localnet source"
    );
    for lane in ["pinned", "mainnet", "testnet", "devnet", "main"] {
        let expected_tag = if lane == "pinned" {
            pin_string(pins, "parity", "tag")?
        } else {
            lane
        };
        ensure!(
            pins["localnet"][lane]["tag"].as_str() == Some(expected_tag),
            "localnet {lane} tag differs from pin"
        );
        ensure!(
            pins["localnet"][lane]["digest"]
                .as_str()
                .is_some_and(|d| prefixed_hash(d, "sha256:")),
            "localnet {lane} lacks a valid digest"
        );
    }
    ensure!(
        pins["networks"]["finney"]["genesis"].as_str()
            == Some("0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03"),
        "incorrect finney genesis pin"
    );
    ensure!(
        pins["artifacts"].as_table().is_some_and(|t| !t.is_empty()),
        "upstream artifact pins are missing"
    );
    for (source, artifact) in pins["artifacts"]
        .as_table()
        .context("artifacts must be a table")?
    {
        ensure!(
            artifact
                .get("sha256")
                .and_then(Item::as_str)
                .is_some_and(|s| hex_string(s, 64)),
            "artifact {source} has an invalid SHA256"
        );
        let destination = artifact
            .get("to")
            .and_then(Item::as_str)
            .context("artifact has no destination")?;
        ensure!(
            destination.starts_with("vendor/")
                && !Path::new(destination)
                    .components()
                    .any(|p| matches!(p, std::path::Component::ParentDir)),
            "artifact {source} escapes vendor/"
        );
    }
    Ok(())
}

fn verify_git_dependency(item: &Item, tag: &str, name: &str) -> Result<()> {
    ensure!(
        item.get("git").and_then(Item::as_str) == Some(UPSTREAM),
        "{name} must use the upstream Git source"
    );
    ensure!(
        item.get("tag").and_then(Item::as_str) == Some(tag),
        "{name} must use core.tag {tag}"
    );
    ensure!(
        item.get("rev").is_none() && item.get("branch").is_none() && item.get("path").is_none(),
        "{name} carries an alternate source selector"
    );
    Ok(())
}

fn verify_manifest(root: &Path, pins: &DocumentMut) -> Result<()> {
    let manifest = read_document(&root.join("Cargo.toml"))?;
    let tag = pin_string(pins, "core", "tag")?;
    for name in ["bittensor-core", "subtensor-macros", "subtensor-linting"] {
        verify_git_dependency(
            item_at(&manifest, &["workspace", "dependencies", name])?,
            tag,
            name,
        )?;
    }
    ensure!(
        item_at(
            &manifest,
            &[
                "workspace",
                "dependencies",
                "bittensor-core",
                "default-features"
            ]
        )?
        .as_bool()
            == Some(false),
        "bittensor-core must disable default features"
    );
    verify_git_dependency(
        item_at(&manifest, &["patch", "crates-io", "w3f-bls"])?,
        tag,
        "w3f-bls patch",
    )?;
    for (name, dependency) in item_at(&manifest, &["workspace", "dependencies"])?
        .as_table_like()
        .context("workspace dependencies missing")?
        .iter()
    {
        if dependency.get("git").and_then(Item::as_str) == Some(UPSTREAM) {
            verify_git_dependency(dependency, tag, name)?;
        }
    }
    // Direct upstream dependencies belong only in the workspace root.
    for entry in fs::read_dir(root.join("crates"))? {
        let path = entry?.path().join("Cargo.toml");
        if path.is_file() {
            ensure_no_direct_upstream(&read_document(&path)?, &path)?;
        }
    }
    ensure_no_direct_upstream(
        &read_document(&root.join("xtask/Cargo.toml"))?,
        &root.join("xtask/Cargo.toml"),
    )?;
    ensure!(
        pins["core"].get("patch").is_none(),
        "core.patch verification must be implemented before using patched core pins"
    );
    Ok(())
}

fn ensure_no_direct_upstream(doc: &DocumentMut, path: &Path) -> Result<()> {
    fn visit(item: &Item, path: &Path) -> Result<()> {
        if let Some(table) = item.as_table_like() {
            ensure!(
                table.get("git").and_then(Item::as_str) != Some(UPSTREAM),
                "{} declares a direct monorepo dependency; inherit workspace.dependencies",
                path.display()
            );
            for (_, child) in table.iter() {
                visit(child, path)?;
            }
        }
        Ok(())
    }
    visit(doc.as_item(), path)
}

fn verify_lock(root: &Path, pins: &DocumentMut) -> Result<()> {
    let lock = read_document(&root.join("Cargo.lock"))
        .context("committed Cargo.lock is required for pin --check")?;
    let expected = verify_lock_document(&lock, pins)?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
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
        ])
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .context("inspect built BLS provenance for pin --check")?;
    ensure!(
        output.status.success(),
        "cargo tree failed while inspecting built BLS provenance:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).context("cargo tree output is not UTF-8")?;
    validate_bls_tree(&tree, &expected).map_err(anyhow::Error::msg)
}

fn verify_lock_document(lock: &DocumentMut, pins: &DocumentMut) -> Result<String> {
    let packages = lock
        .get("package")
        .and_then(Item::as_array_of_tables)
        .context("Cargo.lock has no packages")?;
    let tag = pin_string(pins, "core", "tag")?;
    let commit = pin_string(pins, "core", "commit")?;
    let expected = format!("git+{UPSTREAM}?tag={tag}#{commit}");
    for name in ["bittensor-core", "subtensor-macros"] {
        let matching: Vec<_> = packages
            .iter()
            .filter(|p| p.get("name").and_then(Item::as_str) == Some(name))
            .collect();
        ensure!(
            matching.len() == 1,
            "Cargo.lock must contain exactly one {name}; found {}",
            matching.len()
        );
        ensure!(
            matching[0].get("source").and_then(Item::as_str) == Some(&expected),
            "Cargo.lock {name} source differs from core pin {expected}"
        );
    }
    let identities: Vec<_> = packages
        .iter()
        .map(|package| {
            Ok(LockedPackage {
                name: package
                    .get("name")
                    .and_then(Item::as_str)
                    .context("lock package name missing")?,
                version: package
                    .get("version")
                    .and_then(Item::as_str)
                    .context("lock package version missing")?,
                source: package.get("source").and_then(Item::as_str),
                checksum: package.get("checksum").and_then(Item::as_str),
            })
        })
        .collect::<Result<_>>()?;
    validate_bls_lock(&identities, &expected).map_err(anyhow::Error::msg)?;
    for package in packages {
        if let Some(source) = package.get("source").and_then(Item::as_str)
            && source.starts_with(&format!("git+{UPSTREAM}"))
        {
            ensure!(
                source == expected,
                "Cargo.lock contains a monorepo package at an inconsistent pin: {source}"
            );
        }
    }
    Ok(expected)
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary_root = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("target"));
    let temporary_root = temporary_root.join("pin/write");
    fs::create_dir_all(&temporary_root)?;
    let temporary = temporary_root.join(format!("upstream.{}.toml", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_canary_mode_requires_explicit_no_commit_and_excludes_release_modes() -> Result<()> {
        let canary = options(vec![
            "--core-branch".into(),
            "main".into(),
            "--no-commit".into(),
        ])?;
        assert_eq!(canary.core_branch.as_deref(), Some("main"));
        assert!(canary.no_commit);
        for arguments in [
            vec!["--core-branch", "main"],
            vec!["--no-commit"],
            vec!["--core-branch", "main", "--no-commit", "--check"],
            vec!["--core-branch", "main", "--no-commit", "--parity", "auto"],
            vec!["--core-branch", "main", "--no-commit", "--networks"],
        ] {
            assert!(options(arguments.into_iter().map(str::to_owned).collect()).is_err());
        }
        Ok(())
    }

    #[test]
    fn check_is_explicit_and_has_no_implicit_parity() -> Result<()> {
        ensure!(
            options(vec!["--check".into()])?
                == Options {
                    check: true,
                    ..Options::default()
                }
        );
        ensure!(options(vec!["--parity".into()]).is_err());
        ensure!(
            options(vec![
                "--check".into(),
                "--github-output".into(),
                "out".into()
            ])
            .is_err()
        );
        ensure!(options(vec!["--check".into(), "--unknown".into()]).is_err());
        Ok(())
    }

    #[test]
    fn canonical_release_tags_only() -> Result<()> {
        ensure!(release_spec("v478")? == 478);
        for tag in [
            "478",
            "v0478",
            "v478-rc1",
            "main",
            "v",
            "v0",
            "v999999999999",
        ] {
            ensure!(release_spec(tag).is_err());
        }
        Ok(())
    }

    #[test]
    fn replay_and_poll_flags_require_their_explicit_modes() -> Result<()> {
        assert!(options(vec!["--force".into(), "--plan".into()])?.force);
        assert!(
            options(vec![
                "--parity".into(),
                "v478".into(),
                "--force".into(),
                "--dry-run".into()
            ])?
            .dry_run
        );
        assert!(
            options(vec![
                "--parity".into(),
                "auto".into(),
                "--check".into(),
                "--resolve-only".into()
            ])?
            .resolve_only
        );
        for args in [
            vec!["--force", "--check"],
            vec!["--force", "--networks"],
            vec!["--force", "--force"],
            vec!["--resolve-only", "--parity", "auto"],
            vec!["--prepare-candidate", "--parity", "v478", "--dry-run"],
            vec!["--plan", "--core-branch", "main", "--no-commit"],
        ] {
            assert!(options(args.into_iter().map(str::to_owned).collect()).is_err());
        }
        Ok(())
    }

    #[test]
    fn malformed_pin_schema_returns_errors() -> Result<()> {
        for input in [
            "",
            "[core]\ntag='v478'",
            "monorepo={url='wrong'}\n[core]\n",
            "[monorepo]\nurl=42\n",
        ] {
            let pins: DocumentMut = input.parse()?;
            ensure!(verify_schema(&pins).is_err());
        }
        Ok(())
    }

    fn release_fixture() -> (Json, Json, Json, Vec<u8>) {
        let wasm = b"release runtime fixture".to_vec();
        let sha = format!("0x{:x}", Sha256::digest(&wasm));
        let code = format!("0x{:x}", Blake2b::<U32>::digest(&wasm));
        (
            json!({"draft": false, "prerelease": false, "tag_name": "v478"}),
            json!({"tag": "v478", "commit": "commit", "repo": REPOSITORY, "network": "finney", "spec_version": 478, "wasm_sha256": sha}),
            json!({"commit": "commit", "sha256": sha, "runtimes": {"compressed": {"blake2_256": code, "subwasm": {"core_version": {"specVersion": 478}}}}}),
            wasm,
        )
    }

    #[test]
    fn release_provenance_fails_closed_on_each_identity() -> Result<()> {
        let (release, manifest, digest, wasm) = release_fixture();
        validate_release(&release, "v478", "commit", &manifest, &digest, &wasm)?;
        for key in ["draft", "prerelease"] {
            let mut bad = release.clone();
            bad[key] = json!(true);
            ensure!(validate_release(&bad, "v478", "commit", &manifest, &digest, &wasm).is_err());
        }
        for (key, bad_value) in [
            ("commit", json!("other")),
            ("tag", json!("v477")),
            ("repo", json!("attacker/subtensor")),
            ("network", json!("testnet")),
            ("spec_version", json!(477)),
            ("wasm_sha256", json!("0xdead")),
        ] {
            let mut bad = manifest.clone();
            bad[key] = bad_value;
            ensure!(validate_release(&release, "v478", "commit", &bad, &digest, &wasm).is_err());
        }
        ensure!(
            validate_release(
                &release,
                "v478",
                "commit",
                &manifest,
                &digest,
                b"different wasm"
            )
            .is_err()
        );
        let mut bad = digest.clone();
        bad["runtimes"]["compressed"]["blake2_256"] = json!("0xdead");
        ensure!(validate_release(&release, "v478", "commit", &manifest, &bad, &wasm).is_err());
        Ok(())
    }

    #[test]
    fn registry_digest_is_hash_of_exact_manifest() -> Result<()> {
        let body = br#"{"schemaVersion":2,"manifests":[]}"#;
        let digest = format!("sha256:{:x}", Sha256::digest(body));
        let mut response = format!("HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\ndocker-content-digest: {digest}\r\n\r\n").into_bytes();
        response.extend(body);
        ensure!(parse_registry_response(&response)? == digest);
        response.push(b' ');
        ensure!(parse_registry_response(&response).is_err());
        ensure!(parse_registry_response(b"HTTP/2 200\r\n\r\n{}").is_err());
        Ok(())
    }

    #[test]
    fn manifest_dependency_rejects_alternate_sources() -> Result<()> {
        let good: DocumentMut =
            format!("dep = {{ git = {UPSTREAM:?}, tag = \"v478\" }}").parse()?;
        verify_git_dependency(&good["dep"], "v478", "fixture")?;
        for selector in ["rev", "branch", "path"] {
            let bad: DocumentMut =
                format!("dep = {{ git = {UPSTREAM:?}, tag = \"v478\", {selector} = \"other\" }}")
                    .parse()?;
            ensure!(verify_git_dependency(&bad["dep"], "v478", "fixture").is_err());
        }
        ensure!(verify_git_dependency(&good["dep"], "v477", "fixture").is_err());
        Ok(())
    }

    fn lock_fixture(dormant: bool) -> Result<(DocumentMut, DocumentMut)> {
        let pins: DocumentMut =
            "[core]\ntag = 'v478'\ncommit = '1111111111111111111111111111111111111111'".parse()?;
        let source = format!("git+{UPSTREAM}?tag=v478#1111111111111111111111111111111111111111");
        let mut lock = String::from("version = 4\n");
        for (name, version) in [
            ("bittensor-core", "0.1.0"),
            ("subtensor-macros", "0.1.0"),
            ("w3f-bls", "0.1.3"),
            ("subtensor-linting", "0.1.0"),
        ] {
            lock.push_str(&format!(
                "\n[[package]]\nname = '{name}'\nversion = '{version}'\nsource = '{source}'\n"
            ));
        }
        if dormant {
            lock.push_str(&format!(
                "\n[[package]]\nname = 'w3f-bls'\nversion = '{}'\nsource = '{}'\nchecksum = '{}'\n",
                rtao_core::dependency_policy::DORMANT_BLS_VERSION,
                rtao_core::dependency_policy::DORMANT_BLS_SOURCE,
                rtao_core::dependency_policy::DORMANT_BLS_CHECKSUM
            ));
        }
        Ok((lock.parse()?, pins))
    }

    #[test]
    fn lock_policy_accepts_only_recorded_dormant_bls_identity() -> Result<()> {
        let (primary, pins) = lock_fixture(false)?;
        verify_lock_document(&primary, &pins)?;
        let (recorded, _) = lock_fixture(true)?;
        verify_lock_document(&recorded, &pins)?;
        for (key, invalid) in [
            ("version", "0.1.10"),
            ("source", "registry+https://other.invalid/index"),
            ("checksum", "changed"),
        ] {
            let mut changed = recorded.clone();
            let entries = changed["package"]
                .as_array_of_tables_mut()
                .context("fixture packages missing")?;
            entries
                .get_mut(4)
                .context("fixture dormant package missing")?[key] = value(invalid);
            assert!(
                verify_lock_document(&changed, &pins).is_err(),
                "accepted changed dormant {key}"
            );
        }
        let mut missing_checksum = recorded.clone();
        missing_checksum["package"]
            .as_array_of_tables_mut()
            .context("fixture packages missing")?
            .get_mut(4)
            .context("fixture dormant package missing")?
            .remove("checksum");
        assert!(verify_lock_document(&missing_checksum, &pins).is_err());
        Ok(())
    }

    #[test]
    fn lock_policy_preserves_single_core_macros_and_all_monorepo_pins() -> Result<()> {
        let (recorded, pins) = lock_fixture(true)?;
        for index in [0, 1] {
            let mut duplicate = recorded.clone();
            let entries = duplicate["package"]
                .as_array_of_tables_mut()
                .context("fixture packages missing")?;
            let copy = entries
                .get(index)
                .context("fixture package missing")?
                .clone();
            entries.push(copy);
            assert!(verify_lock_document(&duplicate, &pins).is_err());

            let mut missing = recorded.clone();
            missing["package"]
                .as_array_of_tables_mut()
                .context("fixture packages missing")?
                .remove(index);
            assert!(verify_lock_document(&missing, &pins).is_err());
        }
        for index in [0, 1, 2, 3] {
            let mut changed = recorded.clone();
            changed["package"]
                .as_array_of_tables_mut()
                .context("fixture packages missing")?
                .get_mut(index)
                .context("fixture package missing")?["source"] = value(format!(
                "git+{UPSTREAM}?tag=v479#2222222222222222222222222222222222222222"
            ));
            assert!(verify_lock_document(&changed, &pins).is_err());
        }
        Ok(())
    }

    #[test]
    fn lock_and_actual_build_policy_reject_dormant_activation() -> Result<()> {
        let (recorded, pins) = lock_fixture(true)?;
        let expected = verify_lock_document(&recorded, &pins)?;
        let primary = format!("w3f-bls v0.1.3 ({UPSTREAM}?tag=v478#11111111)\n");
        validate_bls_tree(&primary, &expected).map_err(anyhow::Error::msg)?;
        assert!(validate_bls_tree(&format!("{primary}w3f-bls v0.1.9\n"), &expected).is_err());
        assert!(validate_bls_tree("w3f-bls v0.1.9\n", &expected).is_err());
        assert!(validate_bls_tree("", &expected).is_err());
        Ok(())
    }
}
