//! C1-C7: the real `RemoteSigner` against loopback servers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Router,
    routing::{get, post},
};
use common::*;
use rtao_btauth::InMemoryNonceStore;
use rtao_primitives::{Balance, CryptoType, Tao};
use rtao_remote_signer::{
    RemoteSigner, RemoteSignerConfig, RemoteSignerError,
    server::{
        AuthorizedClient, PinnedRuntimeSource, RemoteSignerServer, ServedAccount, ServerConfig,
    },
    wire::{AccountEntry, AccountsResponse, Hex32, PROTOCOL, SignResponse, Ss58, WireCrypto},
};
use rtao_signer::{MultiSignature, Signer, SignerError};
use url::Url;

async fn spawn(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    address
}

async fn reference(edit: impl FnOnce(&mut ServerConfig)) -> SocketAddr {
    let mut cfg = ServerConfig::new(genesis());
    cfg.accounts = vec![ServedAccount::new(Arc::new(alice()))];
    cfg.clients = vec![AuthorizedClient {
        client: charlie().account(),
        accounts: BTreeSet::from([alice().account()]),
    }];
    edit(&mut cfg);
    let server = RemoteSignerServer::new(
        cfg,
        Arc::new(PinnedRuntimeSource::new(runtime(), SPEC_NAME)),
        Arc::new(InMemoryNonceStore::new()),
    )
    .unwrap();
    spawn(server.router()).await
}

fn config(address: SocketAddr) -> RemoteSignerConfig {
    let mut cfg = RemoteSignerConfig::new(
        Url::parse(&format!("http://{address}/")).unwrap(),
        alice().account(),
        CryptoType::Sr25519,
        Arc::new(charlie()),
    );
    cfg.timeout = Duration::from_secs(10);
    cfg
}

#[tokio::test]
async fn c1_connect_then_sign_returns_a_verifying_signature() {
    let address = reference(|_| {}).await;
    let (signer, accounts) = RemoteSigner::connect(config(address)).await.unwrap();
    assert_eq!(accounts.genesis_hash, Hex32(genesis()));
    assert_eq!(
        accounts.accounts,
        vec![AccountEntry {
            account: Ss58(alice().account()),
            crypto: WireCrypto::Sr25519
        }]
    );
    let vector = v1();
    let signature = signer.sign(&vector.signing_request()).await.unwrap();
    assert_eq!(signature.crypto, CryptoType::Sr25519);
    assert!(verify(&vector.payload, &signature.bytes, alice().account()));
    // Each call re-signs a fresh btauth nonce, so a retry is accepted.
    let again = signer.sign(&vector.signing_request()).await.unwrap();
    assert!(verify(&vector.payload, &again.bytes, alice().account()));
    // A base URL with a path prefix joins sign/accounts below it.
    let prefixed = Router::new().nest("/signer", {
        let mut cfg = ServerConfig::new(genesis());
        cfg.accounts = vec![ServedAccount::new(Arc::new(alice()))];
        cfg.clients = vec![AuthorizedClient {
            client: charlie().account(),
            accounts: BTreeSet::from([alice().account()]),
        }];
        RemoteSignerServer::new(
            cfg,
            Arc::new(PinnedRuntimeSource::new(runtime(), SPEC_NAME)),
            Arc::new(InMemoryNonceStore::new()),
        )
        .unwrap()
        .router()
    });
    let address = spawn(prefixed).await;
    let mut cfg = config(address);
    cfg.url = Url::parse(&format!("http://{address}/signer")).unwrap();
    let (signer, _) = RemoteSigner::connect(cfg).await.unwrap();
    assert_eq!(signer.sign_url().path(), "/signer/sign");
    // Nested routers see the stripped path, but btauth signs the original URI.
    let signature = signer.sign(&vector.signing_request()).await.unwrap();
    assert!(verify(&vector.payload, &signature.bytes, alice().account()));
}

fn fake_accounts() -> AccountsResponse {
    AccountsResponse {
        version: PROTOCOL.into(),
        server_key: Ss58(alice().account()),
        accounts: vec![AccountEntry {
            account: Ss58(alice().account()),
            crypto: WireCrypto::Sr25519,
        }],
        genesis_hash: Hex32(genesis()),
        purposes: vec![],
    }
}

async fn fake(response: SignResponse) -> SocketAddr {
    spawn(
        Router::new()
            .route("/sign", post(move || async move { axum::Json(response) }))
            .route("/accounts", get(|| async { axum::Json(fake_accounts()) })),
    )
    .await
}

#[tokio::test]
async fn c2_signatures_over_other_bytes_or_for_other_accounts_are_refused() {
    let other = alice().sign_raw(b"some other payload").await.unwrap();
    let forged = MultiSignature {
        crypto: CryptoType::Sr25519,
        bytes: other,
    };
    let address = fake(SignResponse {
        version: PROTOCOL.into(),
        account: Ss58(alice().account()),
        signature: forged.to_bytes().to_vec().into(),
    })
    .await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let vector = v1();
    let error = signer.sign(&vector.signing_request()).await.unwrap_err();
    assert!(
        matches!(error, SignerError::InvalidSignature(_)),
        "{error:?}"
    );

    let genuine = alice().sign_raw(&vector.payload).await.unwrap();
    let address = fake(SignResponse {
        version: PROTOCOL.into(),
        account: Ss58(bob().account()),
        signature: MultiSignature {
            crypto: CryptoType::Sr25519,
            bytes: genuine,
        }
        .to_bytes()
        .to_vec()
        .into(),
    })
    .await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let error = signer.sign(&vector.signing_request()).await.unwrap_err();
    assert!(
        matches!(error, SignerError::InvalidSignature(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn c3_raw_signing_is_unsupported() {
    let signer = RemoteSigner::new(config("127.0.0.1:9".parse().unwrap())).unwrap();
    assert_eq!(
        signer.sign_raw(b"message").await.unwrap_err(),
        SignerError::RawSigningUnsupported
    );
    assert_eq!(signer.caps(), rtao_signer::SignerCaps::RAW_PAYLOAD);
}

#[test]
fn c4_plain_http_is_loopback_only() {
    let with_url = |url: &str| {
        let mut cfg = config("127.0.0.1:1".parse().unwrap());
        cfg.url = Url::parse(url).unwrap();
        RemoteSigner::new(cfg)
    };
    assert!(matches!(
        with_url("http://192.0.2.1/"),
        Err(RemoteSignerError::Config(_))
    ));
    assert!(matches!(
        with_url("http://signer.example/"),
        Err(RemoteSignerError::Config(_))
    ));
    assert!(with_url("http://127.0.0.1:8730/").is_ok());
    assert!(with_url("http://[::1]:8730/").is_ok());
    assert!(with_url("http://127.0.0.1:8730/prefix?x=1").is_err());
    assert!(with_url("http://user:pw@127.0.0.1:8730/").is_err());
    assert!(with_url("ftp://127.0.0.1/").is_err());
}

#[tokio::test]
async fn c5_oversized_responses_are_refused() {
    let address =
        spawn(Router::new().route("/sign", post(|| async { vec![b'a'; 70 * 1024] }))).await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let error = signer
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Protocol(message) if message.contains("exceeds")),
        "{error:?}"
    );
}

#[test]
fn c6_the_client_key_must_differ_from_the_account() {
    let mut cfg = config("127.0.0.1:1".parse().unwrap());
    cfg.client_key = Arc::new(alice());
    let error = RemoteSigner::new(cfg).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the remote signer client key must differ from the signing account"
    );
}

#[tokio::test]
async fn c7_policy_denials_surface_the_first_violation_with_help() {
    let address = reference(|cfg| {
        cfg.policy.limits.max_spend = Some(Balance::<Tao>::from_rao(12_344));
    })
    .await;
    let (signer, _) = RemoteSigner::connect(config(address)).await.unwrap();
    let error = signer.sign(&v1().signing_request()).await.unwrap_err();
    let SignerError::WithHelp { message, help } = &error else {
        panic!("{error:?}");
    };
    assert!(message.contains("HTTP 403"), "{message}");
    assert!(message.contains("policy_denied"), "{message}");
    assert!(message.contains("exceeds max_spend_tao"), "{message}");
    assert!(help.contains("policy"), "{help}");
}

#[tokio::test]
async fn connect_refuses_unlisted_accounts_and_discovery_needs_the_receiver() {
    let address = reference(|_| {}).await;
    let mut cfg = config(address);
    cfg.account = bob().account();
    cfg.receiver = Some(alice().account());
    let error = RemoteSigner::connect(cfg).await.unwrap_err();
    assert!(matches!(error, RemoteSignerError::Config(_)), "{error:?}");
    // Discovery binds btauth to an explicit receiver; a wrong one is a 401.
    let url = Url::parse(&format!("http://{address}/")).unwrap();
    let listed = RemoteSigner::discover(
        url.clone(),
        Arc::new(charlie()),
        alice().account(),
        Duration::from_secs(10),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(listed.accounts.len(), 1);
    let error = RemoteSigner::discover(
        url,
        Arc::new(charlie()),
        bob().account(),
        Duration::from_secs(10),
        Vec::new(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Http { status: 401, detail, .. } if detail == "request was signed for a different receiving hotkey"),
        "{error:?}"
    );
}

/// A one-shot raw HTTP/1.1 server that answers every request with `response`.
async fn raw_server(response: Vec<u8>) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let response = response.clone();
            tokio::spawn(async move {
                // Read the head and a Content-Length body, then answer.
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let Ok(read) = stream.read(&mut buffer).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    address
}

#[tokio::test]
async fn c5_streamed_responses_without_a_length_are_capped() {
    let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for _ in 0..20 {
        response.extend_from_slice(b"1000\r\n");
        response.extend_from_slice(&[b'a'; 0x1000]);
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"0\r\n\r\n");
    let address = raw_server(response).await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let error = signer
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Protocol(message) if message.contains("exceeds")),
        "{error:?}"
    );
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let target = reference(|_| {}).await;
    let address = spawn(Router::new().route(
        "/sign",
        post(move || async move {
            (
                http::StatusCode::TEMPORARY_REDIRECT,
                [(http::header::LOCATION, format!("http://{target}/sign"))],
            )
        }),
    ))
    .await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let error = signer
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RemoteSignerError::Http { status: 307, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn server_diagnostics_are_sanitized_and_truncated() {
    let hostile = format!("\u{1b}]0;owned\u{7}\u{1b}[2J{}", "x".repeat(70_000 / 2));
    let body = serde_json::json!({
        "detail": hostile,
        "code": "policy_denied\u{1b}[31m",
        "violations": vec!["\u{1b}[1mviolation"; 100],
    })
    .to_string();
    let address = spawn(Router::new().route(
        "/sign",
        post(move || async move {
            (
                http::StatusCode::FORBIDDEN,
                [(http::header::CONTENT_TYPE, "application/json")],
                body,
            )
        }),
    ))
    .await;
    let signer = RemoteSigner::new(config(address)).unwrap();
    let error = signer
        .sign_request(&v1().signing_request())
        .await
        .unwrap_err();
    let RemoteSignerError::Http {
        status: 403,
        detail,
        code,
        violations,
    } = &error
    else {
        panic!("{error:?}");
    };
    assert!(!detail.chars().any(char::is_control), "{detail:?}");
    assert!(detail.chars().count() <= 513, "{}", detail.len());
    assert!(
        code.as_deref()
            .is_some_and(|code| !code.chars().any(char::is_control))
    );
    assert_eq!(violations.len(), 16);
    assert!(violations.iter().all(|v| !v.chars().any(char::is_control)));
    let shown = SignerError::from(error).to_string();
    assert!(!shown.chars().any(char::is_control), "{shown:?}");
}

#[test]
fn localhost_is_accepted_by_name_without_resolution() {
    let mut cfg = config("127.0.0.1:1".parse().unwrap());
    cfg.url = Url::parse("http://localhost:8730/").unwrap();
    assert!(RemoteSigner::new(cfg.clone()).is_ok());
    cfg.url = Url::parse("http://LOCALHOST:8730/").unwrap();
    assert!(RemoteSigner::new(cfg.clone()).is_ok());
    cfg.url = Url::parse("http://localhost.example:8730/").unwrap();
    assert!(matches!(
        RemoteSigner::new(cfg),
        Err(RemoteSignerError::Config(_))
    ));
}
