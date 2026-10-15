// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/upgrade_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
#![allow(clippy::expect_used)]

use super::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::Cursor,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// A hand-encoded proxy/sudo/setCode proposal, including the 50B/0 weight.
fn proposal(code: &[u8], encoded_length: &[u8]) -> Vec<u8> {
    let mut bytes = vec![16, 0, 0];
    bytes.extend_from_slice(&[7; 32]);
    bytes.extend_from_slice(&[0, 12, 1, 0, 2]);
    bytes.extend_from_slice(encoded_length);
    bytes.extend_from_slice(code);
    bytes.extend_from_slice(&[7, 0, 0x74, 0x3b, 0xa4, 0x0b, 0]);
    bytes
}

#[test]
fn release_references_accept_the_pinned_forms_and_preserve_asset_tags() {
    let expected = ReleaseRef {
        repo: "RaoFoundation/subtensor".into(),
        tag: "proposal-v432".into(),
    };
    for source in [
        "  RaoFoundation/subtensor@proposal-v432\n",
        "https://github.com/RaoFoundation/subtensor/releases/tag/proposal-v432",
        "http://github.com/RaoFoundation/subtensor/releases/download/proposal-v432/proxy_proxy_blob.hex?download=1#file",
    ] {
        assert_eq!(
            ReleaseRef::parse(source).expect("pinned release form"),
            expected
        );
    }
    assert_eq!(
        expected.asset_url(CALL_DATA_ASSET),
        "https://github.com/RaoFoundation/subtensor/releases/download/proposal-v432/proxy_proxy_blob.hex"
    );
    let slash_tag =
        ReleaseRef::parse("https://github.com/RaoFoundation/subtensor/releases/tag/release%2F432")
            .expect("escaped tag");
    assert_eq!(slash_tag.tag, "release%2F432");
    assert!(slash_tag.asset_url(WASM_ASSET).contains("/release%2F432/"));
    for source in [
        "RaoFoundation/subtensor@",
        "RaoFoundation/subtensor@foo/bar",
        "RaoFoundation/subtensor/other@v1",
        "https://github.com.evil.invalid/O/R/releases/tag/v1",
        "https://github.com@evil.invalid/O/R/releases/tag/v1",
        "https://user:secret@github.com/O/R/releases/tag/v1",
        "https://github.com:8443/O/R/releases/tag/v1",
        "https://github.com/O/R/tree/v1",
        "https://github.com/O/R/releases/tag/",
        "https://gitlab.com/O/R/releases/tag/v1",
    ] {
        assert!(ReleaseRef::parse(source).is_err(), "refuse {source}");
    }
}

#[test]
fn github_credentials_are_api_only_sensitive_and_never_redirect_to_another_origin() {
    use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
    let api = Url::parse("https://api.github.com/repos/O/R/releases").expect("API URL");
    let headers = request_headers(&api, true, Some("fixture-private-token"))
        .expect("authenticated API headers");
    assert_eq!(headers[AUTHORIZATION], "Bearer fixture-private-token");
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(!format!("{headers:?}").contains("fixture-private-token"));
    assert_eq!(headers[ACCEPT], "application/vnd.github+json");
    assert_eq!(headers[USER_AGENT], "rtao-upgrade");
    for url in [
        "https://github.com/O/R/releases/download/v1/asset",
        "https://api.github.com.evil.invalid/releases",
        "http://api.github.com/repos/O/R/releases",
        "https://api.github.com:8443/repos/O/R/releases",
        "https://user@api.github.com/repos/O/R/releases",
    ] {
        let url = Url::parse(url).expect("foreign origin");
        assert!(
            !request_headers(&url, true, Some("fixture-private-token"))
                .expect("foreign request")
                .contains_key(AUTHORIZATION)
        );
        assert!(!redirect_allowed(Some(&api), &url));
    }
    assert!(
        !request_headers(&api, false, Some("fixture-private-token"))
            .expect("ordinary asset request")
            .contains_key(AUTHORIZATION)
    );
    let invalid = request_headers(&api, true, Some("fixture-private-token\r\ninvalid"))
        .expect_err("header injection refused");
    assert!(!invalid.message.contains("fixture-private-token"));
    assert!(redirect_allowed(Some(&api), &api));
    let github = Url::parse("https://github.com/O/R/releases/download/v1/a").expect("GitHub");
    let cdn = Url::parse("https://release-assets.githubusercontent.com/a").expect("asset CDN");
    assert!(redirect_allowed(Some(&github), &cdn));
    assert!(!redirect_allowed(
        Some(&github),
        &Url::parse("file:///etc/passwd").expect("file URL")
    ));
}

#[test]
fn hex_and_hashes_use_raw_call_bytes_and_known_standard_digests() {
    assert_eq!(
        parse_hex_blob(b" \n0x00aBff\t").expect("ASCII hex"),
        [0, 0xab, 0xff]
    );
    for source in [
        &b""[..],
        &b"0x"[..],
        &b"0X00"[..],
        &b"abc"[..],
        &b"ab cd"[..],
        &b"0xzz"[..],
        &b"\xff00"[..],
    ] {
        assert!(parse_hex_blob(source).is_err());
    }
    assert_eq!(
        call_hash_hex(b"").expect("BLAKE2b-256 empty vector"),
        "0x0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn proposal_parser_extracts_only_the_account_and_borrowed_runtime() {
    let code = b"\0asm\x01\0\0\0";
    let bytes = proposal(code, &[0x20]);
    let parsed = parse_proposal_blob(&bytes).expect("hand-encoded CI proposal");
    assert_eq!(parsed.real, AccountId([7; 32]));
    assert_eq!(parsed.code, code);
    assert_eq!(parsed.code.as_ptr(), bytes[41..].as_ptr());
    assert_eq!(PROPOSAL_REF_TIME, 50_000_000_000);
    assert_eq!(PROPOSAL_PROOF_SIZE, 0);
    assert_eq!(FINALIZE_REF_TIME, 80_000_000_000);
    assert_eq!(FINALIZE_PROOF_SIZE, 50_000);
    let empty = proposal(&[], &[0]);
    assert!(
        parse_proposal_blob(&empty)
            .expect("empty Vec is structurally valid")
            .code
            .is_empty()
    );
}

#[test]
fn every_truncation_wrong_wrapper_weight_and_trailing_call_is_refused() {
    let bytes = proposal(b"\0asm\x01\0\0\0", &[0x20]);
    for length in 0..bytes.len() {
        assert!(
            parse_proposal_blob(&bytes[..length]).is_err(),
            "prefix {length}"
        );
    }
    for offset in [
        0,
        1,
        2,
        35,
        36,
        37,
        38,
        39,
        bytes.len() - 2,
        bytes.len() - 1,
    ] {
        let mut changed = bytes.clone();
        changed[offset] ^= 1;
        assert!(
            parse_proposal_blob(&changed).is_err(),
            "modified wrapper or weight {offset}"
        );
    }
    for tail in [&b"\0"[..], &b"\x1a\0\0"[..]] {
        let mut changed = bytes.clone();
        changed.extend_from_slice(tail);
        assert!(parse_proposal_blob(&changed).is_err(), "trailing call/data");
    }
    // Correct runtime bytes inside a different outer call cannot pass.
    let mut batch = vec![26, 0, 4];
    batch.extend_from_slice(&bytes);
    assert!(parse_proposal_blob(&batch).is_err());
    assert!(parse_proposal_blob(&proposal(b"\0asm\x01\0\0\0", &[0x24])).is_err());
    assert!(parse_proposal_blob(&proposal(b"\0asm\x01\0\0\0", &[0x1c])).is_err());
    assert!(parse_proposal_blob(&proposal(b"\0asm\x01\0\0\0", &[0x21, 0])).is_err());
    assert!(
        parse_proposal_blob(&proposal(
            &[],
            &[0x13, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ))
        .is_err()
    );
}

#[test]
fn scale_compacts_use_checked_canonical_boundaries_in_all_four_modes() {
    let vectors = [
        (0, vec![0]),
        (63, vec![0xfc]),
        (64, vec![1, 1]),
        (16_383, vec![0xfd, 0xff]),
        (16_384, vec![2, 0, 1, 0]),
        ((1 << 30) - 1, vec![0xfe, 0xff, 0xff, 0xff]),
        (1 << 30, vec![3, 0, 0, 0, 0x40]),
        (50_000_000_000, vec![7, 0, 0x74, 0x3b, 0xa4, 0x0b]),
        (
            u64::MAX,
            vec![0x13, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ),
    ];
    for (value, expected) in vectors {
        assert_eq!(compact_encode(value), expected);
        assert_eq!(
            compact_decode(&expected).expect("canonical compact"),
            (value, expected.len())
        );
        for length in 0..expected.len() {
            assert!(compact_decode(&expected[..length]).is_err());
        }
    }
    for noncanonical in [
        vec![1, 0],
        vec![0xfd, 0],
        vec![2, 1, 0, 0],
        vec![3, 0xff, 0xff, 0xff, 0x3f],
        vec![7, 0, 0, 0, 0x40, 0],
        vec![0x17, 0, 0, 0, 0, 0, 0, 0, 0, 1],
    ] {
        assert!(compact_decode(&noncanonical).is_err(), "{noncanonical:?}");
    }
}

#[derive(Default)]
struct FakeFetch {
    responses: BTreeMap<String, Vec<u8>>,
    calls: Mutex<Vec<(String, bool)>>,
}

impl FakeFetch {
    fn asset(&mut self, release: &ReleaseRef, name: &str, value: &[u8]) {
        self.responses
            .insert(release.asset_url(name), value.to_vec());
    }

    fn calls(&self) -> Vec<(String, bool)> {
        self.calls.lock().expect("fetch calls").clone()
    }
}

impl UpgradeFetch for FakeFetch {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("record fetch")
                .push((url.into(), api));
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| CliError::failure("fixture missing asset"))
        })
    }
}

#[tokio::test]
async fn release_bundle_fetches_fixed_assets_and_retains_release_trust_source() {
    let release = ReleaseRef::parse("O/R@v1").expect("release");
    let bytes = proposal(b"wasm", &[16]);
    let mut fetch = FakeFetch::default();
    fetch.asset(&release, CALL_DATA_ASSET, hex::encode(&bytes).as_bytes());
    fetch.asset(
        &release,
        MANIFEST_ASSET,
        br#"{"tag":"v1","call_hash":"0x1234"}"#,
    );
    fetch.asset(&release, DIGEST_ASSET, br#"{"sha256":"abcd"}"#);
    fetch.asset(&release, WASM_ASSET, b"wasm");
    let bundle = fetch_proposal_bundle(
        &BundleInputs {
            release_url: Some("O/R@v1".into()),
            ..Default::default()
        },
        &fetch,
    )
    .await
    .expect("complete release");
    assert_eq!(bundle.blob, bytes);
    assert_eq!(
        bundle.call_hash().expect("call hash"),
        call_hash_hex(&bytes).expect("same raw bytes")
    );
    assert_eq!(
        bundle.manifest,
        Some(json!({"tag":"v1","call_hash":"0x1234"}))
    );
    assert_eq!(bundle.digest, Some(json!({"sha256":"abcd"})));
    assert_eq!(bundle.wasm.as_deref(), Some(&b"wasm"[..]));
    assert_eq!(bundle.wasm_source, Some("release"));
    assert!(bundle.notes.is_empty());
    assert_eq!(
        fetch.calls(),
        [CALL_DATA_ASSET, MANIFEST_ASSET, DIGEST_ASSET, WASM_ASSET]
            .map(|asset| (release.asset_url(asset), false))
    );
}

#[tokio::test]
async fn optional_missing_or_malformed_assets_are_notes_but_call_data_is_required() {
    let release = ReleaseRef::parse("O/R@v1").expect("release");
    let inputs = BundleInputs {
        release_url: Some("O/R@v1".into()),
        ..Default::default()
    };
    let mut fetch = FakeFetch::default();
    assert!(fetch_proposal_bundle(&inputs, &fetch).await.is_err());
    fetch.asset(&release, CALL_DATA_ASSET, b"00");
    fetch.asset(&release, MANIFEST_ASSET, br#"["wrong shape"]"#);
    fetch.asset(&release, DIGEST_ASSET, b"not-json");
    let bundle = fetch_proposal_bundle(&inputs, &fetch)
        .await
        .expect("optional assets are advisory");
    assert_eq!(bundle.blob, [0]);
    assert!(bundle.manifest.is_none());
    assert!(bundle.digest.is_none());
    assert!(bundle.wasm.is_none());
    assert!(bundle.wasm_source.is_none());
    assert_eq!(
        bundle.notes,
        [
            "release has no upgrade-manifest.json asset (older release train)",
            "release has no subtensor-digest.json asset",
            "release has no subtensor.wasm asset",
        ]
    );
    fetch.asset(&release, CALL_DATA_ASSET, b"0xinvalid");
    assert!(fetch_proposal_bundle(&inputs, &fetch).await.is_err());
}

#[tokio::test]
async fn a_single_source_is_required_before_any_fetch_and_raw_url_needs_no_release_assets() {
    let fetch = FakeFetch::default();
    for inputs in [
        BundleInputs::default(),
        BundleInputs {
            hex_url: Some(String::new()),
            ..Default::default()
        },
        BundleInputs {
            release_url: Some("O/R@v1".into()),
            hex_url: Some("https://example.com/a".into()),
            ..Default::default()
        },
        BundleInputs {
            hex_url: Some("https://example.com/a".into()),
            hex_file: Some("unused".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(
            fetch_proposal_bundle(&inputs, &fetch)
                .await
                .expect_err("source cardinality")
                .code,
            2
        );
    }
    assert!(fetch.calls().is_empty());
    let mut fetch = FakeFetch::default();
    fetch
        .responses
        .insert("https://example.com/a".into(), b" 0xabcd\n".to_vec());
    let bundle = fetch_proposal_bundle(
        &BundleInputs {
            hex_url: Some("https://example.com/a".into()),
            ..Default::default()
        },
        &fetch,
    )
    .await
    .expect("raw hex source");
    assert_eq!(bundle.blob, [0xab, 0xcd]);
    assert!(bundle.notes.is_empty());
    assert_eq!(fetch.calls(), [("https://example.com/a".into(), false)]);
}

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/upgrade-artifact-tests")
            });
        let root = root.join("tmp").join(format!(
            "upgrade-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("task-scoped artifact fixture");
        Self(root)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.0.join(name);
        fs::write(&path, bytes).expect("fixture file");
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Only test-owned files live here; every offloaded read is awaited.
        fs::remove_dir_all(&self.0).expect("remove task-scoped artifact fixture");
    }
}

#[tokio::test]
async fn local_wasm_is_the_trust_anchor_and_suppresses_release_wasm_download() {
    let fixture = Fixture::new();
    let wasm_path = fixture.write("independent.wasm", b"independently built runtime");
    let hex_path = fixture.write("proposal.hex", b"0x000102");
    let fetch = FakeFetch::default();
    let inputs = BundleInputs {
        hex_file: Some(hex_path),
        wasm_path: Some(wasm_path.clone()),
        ..Default::default()
    };
    let bundle = fetch_proposal_bundle(&inputs, &fetch)
        .await
        .expect("local bundle");
    assert_eq!(bundle.blob, [0, 1, 2]);
    assert_eq!(
        bundle.wasm.as_deref(),
        Some(&b"independently built runtime"[..])
    );
    assert_eq!(bundle.wasm_source, Some("local"));
    assert!(fetch.calls().is_empty());
    let release = ReleaseRef::parse("O/R@v1").expect("release");
    let mut fetch = FakeFetch::default();
    fetch.asset(&release, CALL_DATA_ASSET, b"00");
    fetch.asset(
        &release,
        WASM_ASSET,
        b"release runtime should never be acquired",
    );
    let inputs = BundleInputs {
        release_url: Some("O/R@v1".into()),
        wasm_path: Some(wasm_path),
        ..Default::default()
    };
    let bundle = fetch_proposal_bundle(&inputs, &fetch)
        .await
        .expect("release with local anchor");
    assert_eq!(bundle.wasm_source, Some("local"));
    assert_eq!(
        bundle.wasm.as_deref(),
        Some(&b"independently built runtime"[..])
    );
    assert!(
        !fetch
            .calls()
            .iter()
            .any(|(url, _)| url == &release.asset_url(WASM_ASSET))
    );
    let missing = BundleInputs {
        hex_file: Some(fixture.0.join("absent").to_string_lossy().into_owned()),
        ..Default::default()
    };
    assert!(fetch_proposal_bundle(&missing, &fetch).await.is_err());
}

#[test]
fn regular_files_sparse_oversize_growth_directories_and_symlinks_obey_caps() {
    let fixture = Fixture::new();
    let path = fixture.write("small", b"abcd");
    assert_eq!(
        read_bounded_file(Path::new(&path), 4).expect("inclusive cap"),
        b"abcd"
    );
    assert_eq!(
        read_bounded_file(Path::new(&path), 3)
            .expect_err("metadata cap")
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(
        read_bounded_file(&fixture.0, 4)
            .expect_err("directory")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        read_capped(Cursor::new(b"abcde"), 4)
            .expect_err("growth capped after metadata")
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    let oversized = fixture.0.join("sparse");
    fs::File::create(&oversized)
        .expect("sparse fixture")
        .set_len(MAX_FETCH_BYTES as u64 + 1)
        .expect("sparse length");
    assert_eq!(
        read_bounded_file(&oversized, MAX_FETCH_BYTES as u64)
            .expect_err("64MiB metadata cap")
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    #[cfg(unix)]
    {
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(path, &link).expect("regular symlink");
        assert_eq!(
            read_bounded_file(&link, 4).expect("regular symlink allowed"),
            b"abcd"
        );
    }
}

#[cfg(unix)]
#[test]
fn sockets_and_fifos_are_rejected_before_a_blocking_read() {
    use std::{
        os::unix::{fs::OpenOptionsExt, net::UnixListener},
        process::Command,
        time::Instant,
    };
    let fixture = Fixture::new();
    let socket = fixture.0.join("socket");
    let listener = UnixListener::bind(&socket).expect("fixture socket");
    assert_eq!(
        read_bounded_file(&socket, 4).expect_err("socket").kind(),
        io::ErrorKind::InvalidInput
    );
    drop(listener);
    let fifo = fixture.0.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("FIFO fixture")
            .success()
    );
    // Rescue an accidentally blocking open so regression failure still joins
    // its worker and cleans the one task-scoped fixture directory.
    let rescue_path = fifo.clone();
    let rescue = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        drop(
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(rescue_path)
                .expect("FIFO rescue"),
        );
    });
    let start = Instant::now();
    let result = read_bounded_file(&fifo, 4);
    let elapsed = start.elapsed();
    rescue.join().expect("FIFO rescue joined before cleanup");
    assert_eq!(
        result.expect_err("FIFO").kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(
        elapsed < Duration::from_millis(750),
        "FIFO open blocked for {elapsed:?}"
    );
}

#[test]
fn streaming_cap_refuses_growth_before_appending_and_keeps_previous_bytes() {
    let mut bytes = Vec::new();
    append_capped(&mut bytes, b"ab", 4).expect("first chunk");
    append_capped(&mut bytes, b"cd", 4).expect("inclusive cap");
    assert!(append_capped(&mut bytes, b"e", 4).is_err());
    assert_eq!(bytes, b"abcd");
}
