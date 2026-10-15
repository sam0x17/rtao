// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/upgrade_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded proposal acquisition and exact, offline deployment-call parsing.

use crate::error::CliError;
use futures::future::BoxFuture;
use rtao::primitives::AccountId;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    path::Path,
    time::Duration,
};
use url::Url;
use zeroize::Zeroizing;

pub(super) const PROPOSAL_REF_TIME: u64 = 50_000_000_000;
pub(super) const PROPOSAL_PROOF_SIZE: u64 = 0;
pub(super) const FINALIZE_REF_TIME: u64 = 80_000_000_000;
pub(super) const FINALIZE_PROOF_SIZE: u64 = 50_000;
const MAX_FETCH_BYTES: usize = 64 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const MANIFEST_ASSET: &str = "upgrade-manifest.json";
const CALL_DATA_ASSET: &str = "proxy_proxy_blob.hex";
const WASM_ASSET: &str = "subtensor.wasm";
const DIGEST_ASSET: &str = "subtensor-digest.json";

/// One pinned GitHub release, independent of its optional enrichment assets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReleaseRef {
    pub repo: String,
    pub tag: String,
}

impl ReleaseRef {
    pub fn parse(value: &str) -> Result<Self, CliError> {
        let value = crate::prompt::python_strip(value);
        if let Some((repo, tag)) = value.split_once('@')
            && valid_repo(repo)
            && valid_component(tag)
        {
            return Ok(Self {
                repo: repo.to_owned(),
                tag: tag.to_owned(),
            });
        }
        let url = Url::parse(value).map_err(|_| invalid_release())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(invalid_release());
        }
        let segments = url
            .path_segments()
            .ok_or_else(invalid_release)?
            .collect::<Vec<_>>();
        let [owner, name, "releases", kind, tag, ..] = segments.as_slice() else {
            return Err(invalid_release());
        };
        if !valid_component(owner)
            || !valid_component(name)
            || !matches!(*kind, "tag" | "download")
            || tag.is_empty()
        {
            return Err(invalid_release());
        }
        Ok(Self {
            repo: format!("{owner}/{name}"),
            tag: (*tag).to_owned(),
        })
    }

    pub fn asset_url(&self, name: &str) -> String {
        format!(
            "https://github.com/{}/releases/download/{}/{name}",
            self.repo, self.tag
        )
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn valid_repo(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, repo)| valid_component(owner) && valid_component(repo))
}

fn invalid_release() -> CliError {
    CliError::input(
        "expected a GitHub release URL (https://github.com/OWNER/REPO/releases/tag/TAG) or OWNER/REPO@TAG",
    )
}

/// HTTP acquisition is injectable; fixtures need neither live releases nor TLS.
pub(super) trait UpgradeFetch: Send + Sync {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>>;
}

/// Explicit TLS provider, an overall request deadline and a streaming byte cap.
pub(super) struct GithubFetch {
    client: reqwest::Client,
}

impl GithubFetch {
    pub fn new() -> Result<Self, CliError> {
        let tls = rtao_rpc::tls_config(&rtao_rpc::SessionConfig::default())
            .map_err(|_| CliError::failure("cannot initialize upgrade asset TLS"))?;
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured((*tls).clone())
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("upgrade asset redirect limit exceeded");
                }
                let previous = attempt.previous().first();
                // The API is the only origin that can receive our bearer
                // token. Refuse its cross-origin and plaintext redirects,
                // rather than relying on a downstream header-stripping rule.
                if !redirect_allowed(previous, attempt.url()) {
                    return attempt.stop();
                }
                attempt.follow()
            }))
            .build()
            .map_err(|_| CliError::failure("cannot initialize upgrade asset HTTP client"))?;
        Ok(Self { client })
    }
}

fn api_origin(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("api.github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn redirect_allowed(initial: Option<&Url>, next: &Url) -> bool {
    matches!(next.scheme(), "http" | "https")
        && next.username().is_empty()
        && next.password().is_none()
        && (!initial.is_some_and(api_origin) || api_origin(next))
}

fn request_headers(
    url: &Url,
    api: bool,
    token: Option<&str>,
) -> Result<reqwest::header::HeaderMap, CliError> {
    use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("rtao-upgrade"));
    if api {
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
    }
    if let Some(token) = token.filter(|_| api && api_origin(url)) {
        let bearer = Zeroizing::new(format!("Bearer {token}"));
        let mut value = HeaderValue::from_str(bearer.as_str())
            .map_err(|_| CliError::failure("invalid GitHub API authentication token"))?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

impl UpgradeFetch for GithubFetch {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>> {
        Box::pin(async move {
            let url = Url::parse(url)
                .map_err(|_| CliError::input("upgrade asset URL is not valid HTTP(S)"))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(CliError::input("upgrade asset URL is not valid HTTP(S)"));
            }
            let token = if api && api_origin(&url) {
                std::env::var("GH_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        std::env::var("GITHUB_TOKEN")
                            .ok()
                            .filter(|value| !value.is_empty())
                    })
                    .map(Zeroizing::new)
            } else {
                None
            };
            let headers = request_headers(&url, api, token.as_deref().map(String::as_str))?;
            drop(token);
            let request = self.client.get(url).headers(headers);
            let mut response = request
                .send()
                .await
                // reqwest errors can include URLs and headers. Keep those
                // outside the CLI diagnostic, including on redirect failure.
                .map_err(|_| CliError::failure("upgrade asset request failed"))?;
            if !response.status().is_success() {
                return Err(CliError::failure(format!(
                    "upgrade asset request failed: HTTP {}",
                    response.status().as_u16()
                )));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_FETCH_BYTES as u64)
            {
                return Err(fetch_cap());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| CliError::failure("upgrade asset body read failed"))?
            {
                append_capped(&mut bytes, &chunk, MAX_FETCH_BYTES)?;
            }
            Ok(bytes)
        })
    }
}

fn append_capped(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), CliError> {
    if chunk.len() > limit.saturating_sub(bytes.len()) {
        return Err(fetch_cap());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn fetch_cap() -> CliError {
    CliError::failure("upgrade asset exceeds the 64 MiB fetch cap")
}

async fn fetch_bytes(fetch: &dyn UpgradeFetch, url: &str, api: bool) -> Result<Vec<u8>, CliError> {
    let bytes = fetch.bytes(url, api).await?;
    if bytes.len() > MAX_FETCH_BYTES {
        return Err(fetch_cap());
    }
    Ok(bytes)
}

async fn fetch_object(fetch: &dyn UpgradeFetch, url: &str, api: bool) -> Result<Value, CliError> {
    let bytes = fetch_bytes(fetch, url, api).await?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| CliError::failure("upgrade asset is not valid JSON"))?;
    if !matches!(value, Value::Object(_)) {
        return Err(CliError::failure("upgrade asset must be a JSON object"));
    }
    Ok(value)
}

/// Exactly one call-data source, with an optional independently built runtime.
#[derive(Clone, Debug, Default)]
pub(super) struct BundleInputs {
    pub release_url: Option<String>,
    pub hex_url: Option<String>,
    pub hex_file: Option<String>,
    pub wasm_path: Option<String>,
}

/// Distinguishes the local runtime trust anchor from advisory release assets.
#[derive(Debug)]
pub(super) struct ProposalBundle {
    pub blob: Vec<u8>,
    pub manifest: Option<Value>,
    pub wasm: Option<Vec<u8>>,
    pub digest: Option<Value>,
    pub wasm_source: Option<&'static str>,
    pub notes: Vec<String>,
}

impl ProposalBundle {
    pub fn call_hash(&self) -> Result<String, CliError> {
        call_hash_hex(&self.blob)
    }
}

pub(super) async fn fetch_proposal_bundle(
    inputs: &BundleInputs,
    fetch: &dyn UpgradeFetch,
) -> Result<ProposalBundle, CliError> {
    let release_url = nonempty(&inputs.release_url);
    let hex_url = nonempty(&inputs.hex_url);
    let hex_file = nonempty(&inputs.hex_file);
    if [release_url, hex_url, hex_file]
        .into_iter()
        .flatten()
        .count()
        != 1
    {
        return Err(CliError::input(
            "pass exactly one of --url, --hex-url, or --hex-file",
        ));
    }
    let mut bundle = ProposalBundle {
        blob: Vec::new(),
        manifest: None,
        wasm: None,
        digest: None,
        wasm_source: None,
        notes: Vec::new(),
    };
    let mut release_ref = None;
    if let Some(release_url) = release_url {
        let release = ReleaseRef::parse(release_url)?;
        bundle.blob =
            parse_hex_blob(&fetch_bytes(fetch, &release.asset_url(CALL_DATA_ASSET), false).await?)?;
        match fetch_object(fetch, &release.asset_url(MANIFEST_ASSET), false).await {
            Ok(manifest) => bundle.manifest = Some(manifest),
            Err(_) => bundle
                .notes
                .push("release has no upgrade-manifest.json asset (older release train)".into()),
        }
        match fetch_object(fetch, &release.asset_url(DIGEST_ASSET), false).await {
            Ok(digest) => bundle.digest = Some(digest),
            Err(_) => bundle
                .notes
                .push("release has no subtensor-digest.json asset".into()),
        }
        release_ref = Some(release);
    } else if let Some(hex_url) = hex_url {
        bundle.blob = parse_hex_blob(&fetch_bytes(fetch, hex_url, false).await?)?;
    } else if let Some(hex_file) = hex_file {
        bundle.blob = parse_hex_blob(&read_asset(hex_file).await?)?;
    }
    if let Some(wasm_path) = nonempty(&inputs.wasm_path) {
        bundle.wasm = Some(read_asset(wasm_path).await?);
        bundle.wasm_source = Some("local");
    } else if let Some(release) = &release_ref {
        match fetch_bytes(fetch, &release.asset_url(WASM_ASSET), false).await {
            Ok(wasm) => {
                bundle.wasm = Some(wasm);
                bundle.wasm_source = Some("release");
            }
            Err(_) => bundle
                .notes
                .push("release has no subtensor.wasm asset".into()),
        }
    }
    Ok(bundle)
}

fn nonempty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

async fn read_asset(path: &str) -> Result<Vec<u8>, CliError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || read_bounded_file(Path::new(&path), MAX_FETCH_BYTES as u64))
        .await
        .map_err(|_| CliError::failure("upgrade asset file reader failed"))?
        .map_err(|error| CliError::failure(format!("cannot read upgrade asset file: {error}")))
}

fn read_bounded_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !fs::metadata(path)?.is_file() {
        return Err(nonregular_file());
    }
    let file = options.open(path).map_err(|error| {
        if fs::metadata(path).is_ok_and(|metadata| !metadata.is_file()) {
            nonregular_file()
        } else {
            error
        }
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(nonregular_file());
    }
    if metadata.len() > limit {
        return Err(oversized_file());
    }
    read_capped(file, limit)
}

fn nonregular_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected a regular upgrade asset file",
    )
}

fn oversized_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::FileTooLarge,
        "upgrade asset file exceeds 64 MiB cap",
    )
}

fn read_capped(reader: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(oversized_file());
    }
    Ok(bytes)
}

pub(super) fn parse_hex_blob(raw: &[u8]) -> Result<Vec<u8>, CliError> {
    if raw.len() > MAX_FETCH_BYTES {
        return Err(fetch_cap());
    }
    // The upstream bytes path decodes ASCII before applying Python strip.
    if !raw.is_ascii() {
        return Err(CliError::input("call data is not a valid ASCII hex string"));
    }
    let text = std::str::from_utf8(raw)
        .map_err(|_| CliError::input("call data is not a valid ASCII hex string"))?;
    let text = crate::prompt::python_strip(text);
    let text = text.strip_prefix("0x").unwrap_or(text);
    if text.is_empty() {
        return Err(CliError::input("call data is not a valid hex string"));
    }
    hex::decode(text).map_err(|_| CliError::input("call data is not a valid hex string"))
}

pub(super) fn call_hash_hex(blob: &[u8]) -> Result<String, CliError> {
    let bytes = rtao::core::codec::storage::hash_param("Blake2_256", blob)
        .map_err(|_| CliError::failure("cannot hash upgrade call data"))?;
    if bytes.len() != 32 {
        return Err(CliError::failure("blake2_256 did not return 32 bytes"));
    }
    Ok(format!("0x{}", hex::encode(bytes)))
}

/// Unprefixed lowercase SHA-256, as recorded in subtensor-digest.json.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Only these two proposal fields vary; every wrapper byte and weight is pinned.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct ParsedBlob<'a> {
    pub real: AccountId,
    pub code: &'a [u8],
}

pub(super) fn parse_proposal_blob(blob: &[u8]) -> Result<ParsedBlob<'_>, CliError> {
    if blob.len() > MAX_FETCH_BYTES {
        return Err(fetch_cap());
    }
    let rest = blob.strip_prefix(&[16, 0, 0]).ok_or_else(|| {
        CliError::failure("call data is not proxy.proxy(MultiAddress::Id(...), ...)")
    })?;
    let real: [u8; 32] = rest
        .get(..32)
        .ok_or_else(|| CliError::failure("call data is truncated inside the proxied account"))?
        .try_into()
        .map_err(|_| CliError::failure("invalid proxied account length"))?;
    let rest = rest[32..].strip_prefix(&[0, 12, 1, 0, 2]).ok_or_else(|| {
        CliError::failure(
            "call data after the proxied account is not sudoUncheckedWeight(setCode(...), ...)",
        )
    })?;
    let (length, consumed) = compact_decode(rest)?;
    let length = usize::try_from(length)
        .map_err(|_| CliError::failure("embedded runtime length overflows this platform"))?;
    let code_end = consumed
        .checked_add(length)
        .ok_or_else(|| CliError::failure("embedded runtime length overflows this platform"))?;
    let code = rest
        .get(consumed..code_end)
        .ok_or_else(|| CliError::failure("call data is truncated inside the runtime bytes"))?;
    let mut weight = compact_encode(PROPOSAL_REF_TIME);
    weight.extend_from_slice(&compact_encode(PROPOSAL_PROOF_SIZE));
    if rest.get(code_end..) != Some(weight.as_slice()) {
        return Err(CliError::failure(
            "call data does not end with exactly the pinned sudoUncheckedWeight weight {ref_time: 50000000000, proof_size: 0} (or carries trailing bytes)",
        ));
    }
    Ok(ParsedBlob {
        real: AccountId(real),
        code,
    })
}

fn compact_encode(value: u64) -> Vec<u8> {
    if value < 1 << 6 {
        return vec![(value as u8) << 2];
    }
    if value < 1 << 14 {
        return (((value as u16) << 2) | 1).to_le_bytes().to_vec();
    }
    if value < 1 << 30 {
        return (((value as u32) << 2) | 2).to_le_bytes().to_vec();
    }
    let length = ((u64::BITS - value.leading_zeros()) as usize).div_ceil(8);
    let mut encoded = vec![(((length - 4) as u8) << 2) | 3];
    encoded.extend_from_slice(&value.to_le_bytes()[..length]);
    encoded
}

fn compact_decode(bytes: &[u8]) -> Result<(u64, usize), CliError> {
    let first = *bytes
        .first()
        .ok_or_else(|| CliError::failure("empty compact integer in upgrade call"))?;
    let mode = first & 3;
    let length = match mode {
        0 => return Ok((u64::from(first >> 2), 1)),
        1 => 2,
        2 => 4,
        _ => usize::from(first >> 2) + 4,
    };
    if mode == 3 && length > 8 {
        return Err(CliError::failure(
            "compact integer in upgrade call exceeds u64",
        ));
    }
    let consumed = length + usize::from(mode == 3);
    let data = bytes
        .get(usize::from(mode == 3)..consumed)
        .ok_or_else(|| CliError::failure("truncated compact integer in upgrade call"))?;
    let mut integer = [0_u8; 8];
    integer[..data.len()].copy_from_slice(data);
    let value = u64::from_le_bytes(integer) >> if mode == 3 { 0 } else { 2 };
    // Canonicality excludes overlong small modes, leading zero big-mode
    // payloads and every alternative representation of the same code length.
    if compact_encode(value).as_slice() != &bytes[..consumed] {
        return Err(CliError::failure(
            "noncanonical compact integer in upgrade call",
        ));
    }
    Ok((value, consumed))
}

#[cfg(test)]
mod tests;
