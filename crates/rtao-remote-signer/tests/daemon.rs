//! `rtao-remote-signerd` configuration loading, TLS serving and refusal rules.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{path::PathBuf, sync::Arc, time::Duration};

use common::*;
use rtao_primitives::CryptoType;
use rtao_remote_signer::{
    RemoteSigner, RemoteSignerConfig,
    daemon::{Daemon, DaemonConfig, metadata_bytes},
};
use rtao_signer::Signer;
use url::Url;

fn task_dir(name: &str) -> PathBuf {
    let root = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    // One flat directory per test, so no test removes a shared parent.
    let dir = root.join(format!("rtao-remote-signer-daemon-{name}"));
    cleanup(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

async fn alice_keyfile(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("alice-coldkey");
    let keypair =
        rtao_core::keys::Keypair::from_uri("//Alice", rtao_core::keys::CRYPTO_SR25519).unwrap();
    rtao_wallet::Keyfile::new(&path)
        .set_keypair(
            keypair,
            rtao_wallet::WriteOptions {
                encrypt: false,
                password: None,
                overwrite: rtao_wallet::Overwrite::Yes,
            },
        )
        .await
        .unwrap();
    path
}

fn offline_config(dir: &std::path::Path, keyfile: &std::path::Path, extra: &str) -> String {
    let metadata = dir.join("metadata.hex");
    // The vendored V15 is hex inside its Option<OpaqueMetadata> frame.
    std::fs::write(&metadata, &golden().metadata.v15_hex).unwrap();
    format!(
        r#"
[server]
listen = "127.0.0.1:0"
{extra}

[chain]
metadata = "{metadata}"
spec_version = 461
transaction_version = 1
ss58_format = 42
spec_name = "{SPEC_NAME}"
genesis_hash = "{GENESIS_HEX}"

[[accounts]]
keyfile = "{keyfile}"

[[clients]]
address = "{charlie}"
accounts = ["{alice}"]

[policy]
max_spend_tao = "0.000012345"
allowed_calls = ["Balances.transfer_keep_alive"]
max_era_period = 128
"#,
        metadata = metadata.display(),
        keyfile = keyfile.display(),
        charlie = charlie().account(),
        alice = alice().account(),
    )
}

#[test]
fn metadata_framing_variants_unwrap_to_the_same_blob() {
    let raw = runtime().core().metadata_bytes.clone();
    assert_eq!(metadata_bytes(&raw).unwrap(), raw);
    assert_eq!(
        metadata_bytes(golden().metadata.v15_hex.as_bytes()).unwrap(),
        raw
    );
    let hex = format!("0x{}\n", hex::encode(&raw));
    assert_eq!(metadata_bytes(hex.as_bytes()).unwrap(), raw);
    assert!(metadata_bytes(b"not metadata").is_err());
}

#[tokio::test]
async fn offline_daemon_over_plain_loopback_signs_v1() {
    let dir = task_dir("plain");
    let keyfile = alice_keyfile(&dir).await;
    let config = DaemonConfig::parse(&offline_config(&dir, &keyfile, "")).unwrap();
    let daemon = Daemon::build(config).await.unwrap();
    assert_eq!(daemon.server.server_key(), alice().account());
    let listener = tokio::net::TcpListener::bind(daemon.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(daemon.serve(listener));
    let mut cfg = RemoteSignerConfig::new(
        Url::parse(&format!("http://{address}")).unwrap(),
        alice().account(),
        CryptoType::Sr25519,
        Arc::new(charlie()),
    );
    cfg.timeout = Duration::from_secs(10);
    let (signer, _) = RemoteSigner::connect(cfg).await.unwrap();
    let vector = v1();
    let signature = signer.sign(&vector.signing_request()).await.unwrap();
    assert!(verify(&vector.payload, &signature.bytes, alice().account()));
    // A raw call is outside allowed_calls.
    let remark = with_call(remark(b"x"));
    assert!(signer.sign(&remark.signing_request()).await.is_err());
    cleanup(&dir);
}

#[tokio::test]
async fn tls_daemon_serves_https_with_an_explicit_trust_root() {
    let dir = task_dir("tls");
    let keyfile = alice_keyfile(&dir).await;
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    std::fs::write(&cert, certified.cert.pem()).unwrap();
    std::fs::write(&key, certified.signing_key.serialize_pem()).unwrap();
    let extra = format!(
        "tls_cert = \"{}\"\ntls_key = \"{}\"",
        cert.display(),
        key.display()
    );
    let config = DaemonConfig::parse(&offline_config(&dir, &keyfile, &extra)).unwrap();
    let daemon = Daemon::build(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind(daemon.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(daemon.serve(listener));
    let mut cfg = RemoteSignerConfig::new(
        Url::parse(&format!("https://localhost:{}/", address.port())).unwrap(),
        alice().account(),
        CryptoType::Sr25519,
        Arc::new(charlie()),
    );
    cfg.timeout = Duration::from_secs(10);
    cfg.trust_roots = rtao_remote_signer::client::load_pem_roots(&cert).unwrap();
    let (signer, _) = RemoteSigner::connect(cfg).await.unwrap();
    let vector = v1();
    let signature = signer.sign(&vector.signing_request()).await.unwrap();
    assert!(verify(&vector.payload, &signature.bytes, alice().account()));
    cleanup(&dir);
}

#[tokio::test]
async fn unsafe_daemon_configurations_are_refused() {
    let dir = task_dir("refusals");
    let keyfile = alice_keyfile(&dir).await;
    let base = offline_config(&dir, &keyfile, "");
    let build = |text: String| async move {
        match DaemonConfig::parse(&text) {
            Ok(config) => Daemon::build(config).await.map(drop).map_err(|e| e.0),
            Err(error) => Err(error.0),
        }
    };
    let public = base.replace("listen = \"127.0.0.1:0\"", "listen = \"0.0.0.0:0\"");
    assert!(build(public).await.unwrap_err().contains("loopback"));
    let unknown = base.replace("[policy]", "[policy]\nunknown_key = true");
    assert!(build(unknown).await.is_err());
    let both = base.replace("[chain]", "[chain]\nendpoint = \"ws://127.0.0.1:9944\"");
    assert!(build(both).await.unwrap_err().contains("exactly one"));
    let fee = base.replace("[policy]", "[policy]\nmax_fee_tao = \"1\"");
    assert!(build(fee).await.unwrap_err().contains("max_fee"));
    let raw = base.replace(
        "allowed_calls = [\"Balances.transfer_keep_alive\"]",
        "allow_raw_calls = true",
    );
    assert!(
        build(raw)
            .await
            .unwrap_err()
            .contains("allow_raw_calls cannot")
    );
    // Allowlisting the raw call does not let it bypass max_spend_tao.
    let raw_listed = base.replace(
        "allowed_calls = [\"Balances.transfer_keep_alive\"]",
        "allow_raw_calls = true\nallowed_calls = [\"Balances.transfer_keep_alive\", \"Utility.batch_all\"]",
    );
    assert!(
        build(raw_listed)
            .await
            .unwrap_err()
            .contains("allow_raw_calls cannot")
    );
    let netuids = base.replace("[policy]", "[policy]\nallowed_netuids = [1, 2]");
    assert!(
        build(netuids)
            .await
            .unwrap_err()
            .contains("allowed_netuids")
    );
    let self_client = base.replace(
        &format!("address = \"{}\"", charlie().account()),
        &format!("address = \"{}\"", alice().account()),
    );
    assert!(build(self_client).await.is_err());
    let no_key = base.replace(
        &format!("keyfile = \"{}\"", keyfile.display()),
        "secret_uri_env = \"RTAO_REMOTE_SIGNER_TEST_UNSET_VARIABLE\"",
    );
    assert!(build(no_key).await.unwrap_err().contains("is not set"));
    assert!(build(base).await.is_ok());
    cleanup(&dir);
}

async fn plain_daemon(
    name: &str,
    edit: impl FnOnce(&mut Daemon),
) -> (std::net::SocketAddr, PathBuf) {
    let dir = task_dir(name);
    let keyfile = alice_keyfile(&dir).await;
    let config = DaemonConfig::parse(&offline_config(&dir, &keyfile, "")).unwrap();
    let mut daemon = Daemon::build(config).await.unwrap();
    edit(&mut daemon);
    let listener = tokio::net::TcpListener::bind(daemon.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(daemon.serve(listener));
    (address, dir)
}

/// Read until EOF or `limit`; returns what was read and whether EOF came first.
async fn read_until_closed(stream: &mut tokio::net::TcpStream, limit: Duration) -> (Vec<u8>, bool) {
    use tokio::io::AsyncReadExt;
    let mut received = Vec::new();
    let mut buffer = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return (received, true),
            Ok(Ok(read)) => received.extend_from_slice(&buffer[..read]),
            Err(_) => return (received, false),
        }
    }
}

/// Review finding 2: a peer that never finishes its headers is disconnected.
#[tokio::test]
async fn stalled_request_headers_are_timed_out() {
    use tokio::io::AsyncWriteExt;
    let (address, dir) = plain_daemon("header-timeout", |daemon| {
        daemon.limits.header_read = Duration::from_millis(200);
    })
    .await;
    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    stalled
        .write_all(b"POST /sign HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let (_, closed) = read_until_closed(&mut stalled, Duration::from_secs(5)).await;
    assert!(closed, "a stalled header read must be closed");
    // An idle connection that never sends a request is closed the same way.
    let mut idle = tokio::net::TcpStream::connect(address).await.unwrap();
    let (received, closed) = read_until_closed(&mut idle, Duration::from_secs(5)).await;
    assert!(closed && received.is_empty());
    cleanup(&dir);
}

/// Review finding 2: a slowly sent body cannot hold a request open.
#[tokio::test]
async fn slow_request_bodies_hit_the_request_deadline() {
    use tokio::io::AsyncWriteExt;
    let (address, dir) = plain_daemon("body-deadline", |daemon| {
        daemon.limits.request = Duration::from_millis(300);
    })
    .await;
    let body = serde_json::to_vec(&v1().body()).unwrap();
    let headers = rtao_btauth::sign(
        &charlie(),
        &http::Method::POST,
        "/sign",
        &body,
        Some(&alice().account()),
        &rtao_btauth::SystemClock::default(),
    )
    .await
    .unwrap();
    let mut head = format!(
        "POST /sign HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in &headers {
        head.push_str(&format!("{name}: {}\r\n", value.to_str().unwrap()));
    }
    head.push_str("\r\n");
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&body[..body.len() / 2]).await.unwrap();
    let (received, _) = read_until_closed(&mut stream, Duration::from_secs(5)).await;
    let text = String::from_utf8_lossy(&received);
    assert!(text.starts_with("HTTP/1.1 408"), "{text}");
    cleanup(&dir);
}

/// Review finding 2: connections beyond the cap wait for a free slot.
#[tokio::test]
async fn connections_beyond_the_cap_wait_for_a_slot() {
    use tokio::io::AsyncWriteExt;
    let (address, dir) = plain_daemon("connection-cap", |daemon| {
        daemon.limits.max_connections = 1;
    })
    .await;
    let holder = tokio::net::TcpStream::connect(address).await.unwrap();
    // Let the daemon accept the holder before the second peer arrives.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut waiting = tokio::net::TcpStream::connect(address).await.unwrap();
    waiting
        .write_all(b"GET /accounts HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let (received, closed) = read_until_closed(&mut waiting, Duration::from_millis(500)).await;
    assert!(received.is_empty() && !closed, "served beyond the cap");
    drop(holder);
    let (received, _) = read_until_closed(&mut waiting, Duration::from_secs(5)).await;
    // Unauthenticated, so btauth answers 401 once the slot is free.
    let text = String::from_utf8_lossy(&received);
    assert!(text.starts_with("HTTP/1.1 401"), "{text}");
    cleanup(&dir);
}

#[tokio::test]
async fn max_connections_is_validated() {
    let dir = task_dir("max-connections");
    let keyfile = alice_keyfile(&dir).await;
    let zero = offline_config(&dir, &keyfile, "max_connections = 0");
    let error = Daemon::build(DaemonConfig::parse(&zero).unwrap())
        .await
        .unwrap_err();
    assert!(error.0.contains("max_connections"), "{error}");
    let some = offline_config(&dir, &keyfile, "max_connections = 3");
    let daemon = Daemon::build(DaemonConfig::parse(&some).unwrap())
        .await
        .unwrap();
    assert_eq!(daemon.limits.max_connections, 3);
    cleanup(&dir);
}

/// Review finding 10: a certificate outside the configured roots is refused,
/// and configured roots replace (rather than extend) the platform roots.
#[tokio::test]
async fn tls_with_an_untrusted_certificate_is_refused() {
    let dir = task_dir("tls-untrusted");
    let keyfile = alice_keyfile(&dir).await;
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let other = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let other_cert = dir.join("other.pem");
    std::fs::write(&cert, certified.cert.pem()).unwrap();
    std::fs::write(&key, certified.signing_key.serialize_pem()).unwrap();
    std::fs::write(&other_cert, other.cert.pem()).unwrap();
    let extra = format!(
        "tls_cert = \"{}\"\ntls_key = \"{}\"",
        cert.display(),
        key.display()
    );
    let config = DaemonConfig::parse(&offline_config(&dir, &keyfile, &extra)).unwrap();
    let daemon = Daemon::build(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind(daemon.listen).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(daemon.serve(listener));
    let connect = |roots: Vec<rtao_remote_signer::client::CertificateDer<'static>>| {
        let mut cfg = RemoteSignerConfig::new(
            Url::parse(&format!("https://localhost:{}/", address.port())).unwrap(),
            alice().account(),
            CryptoType::Sr25519,
            Arc::new(charlie()),
        );
        cfg.timeout = Duration::from_secs(10);
        cfg.trust_roots = roots;
        RemoteSigner::connect(cfg)
    };
    // Platform roots only, and a different private CA: both refused.
    let error = connect(Vec::new()).await.unwrap_err();
    assert!(
        matches!(error, rtao_remote_signer::RemoteSignerError::Transport(_)),
        "{error:?}"
    );
    let wrong = rtao_remote_signer::client::load_pem_roots(&other_cert).unwrap();
    let error = connect(wrong).await.unwrap_err();
    assert!(
        matches!(error, rtao_remote_signer::RemoteSignerError::Transport(_)),
        "{error:?}"
    );
    let right = rtao_remote_signer::client::load_pem_roots(&cert).unwrap();
    assert!(connect(right).await.is_ok());
    cleanup(&dir);
}
