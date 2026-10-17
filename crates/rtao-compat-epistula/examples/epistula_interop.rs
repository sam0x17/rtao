//! Interop driver for `ci/interop/epistula/run.sh`.
//!
//! * `emit-vectors` prints rtao-signed headers (and canonical Apex bodies)
//!   as JSON for the Python reference verifiers.
//! * `serve` runs `EpistulaLayer` routes for BitMind's Python client and
//!   prints `LISTENING <port>`; it exits when stdin closes.
//! * `client <base-url>` drives BitMind's FastAPI verifier.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{collections::HashSet, error::Error, io::Read, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    routing::{get, post},
};
use rtao_btauth::{BoxFuture, Caller, InMemoryNonceStore, NonceStore};
use rtao_compat_epistula::{
    BodyDigest, Dialect, SignOptions, Verifier,
    apex::canonical_json,
    client::EpistulaClient,
    server::{Admission, CallerPolicy, EpistulaLayer, PolicyDenied, VerifiedEpistula},
    sign_headers,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T = (), E = Box<dyn Error + Send + Sync>> = std::result::Result<T, E>;

const TS: u64 = 1_700_000_000_123;

fn signer(uri: &str) -> KeypairSigner {
    KeypairSigner::from_uri(uri, CryptoType::Sr25519).unwrap()
}

fn ss58(uri: &str) -> String {
    signer(uri).account().to_string()
}

fn pairs(headers: &rtao_compat_epistula::EpistulaHeaders) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| json!([name, value]))
            .collect(),
    )
}

fn ok() -> Value {
    json!({"status": 200})
}

fn bad(detail: &str) -> Value {
    json!({"status": 400, "detail": detail})
}

/// Vector id, dialect, body, receiver, expected verdicts, secret form.
type Case<'a> = (
    &'a str,
    Dialect,
    Vec<u8>,
    Option<&'a str>,
    Value,
    Option<&'a str>,
);

async fn emit_vectors() -> Result {
    let alice = signer("//Alice");
    let bob = ss58("//Bob");
    let json_body = br#"{"key": "value"}"#.to_vec();
    let big_body: Vec<u8> = (0..200_000u32).map(|index| (index % 251) as u8).collect();
    let apex_body = canonical_json(&json!({"b": 1, "a": "x", "text": "é😀"}));
    let apex_empty = canonical_json(&json!({}));
    let not_for_self = "Bad Request, message is not intended for self";
    let cases: Vec<Case<'_>> = vec![
        (
            "rtao-json-bound",
            Dialect::RTAO,
            json_body.clone(),
            Some(&bob),
            json!({"bitmind": ok(), "spec": ok(), "secret": ok()}),
            Some("python"),
        ),
        (
            "bitmind-empty-bound",
            Dialect::BITMIND,
            Vec::new(),
            Some(&bob),
            json!({"bitmind": ok(), "spec": ok(), "secret": ok()}),
            Some("python"),
        ),
        (
            "bitmind-200k-bound",
            Dialect::BITMIND,
            big_body,
            Some(&bob),
            json!({"bitmind": ok(), "spec": ok()}),
            None,
        ),
        (
            "spec-json-unbound",
            Dialect::SPEC,
            json_body.clone(),
            None,
            json!({"bitmind": bad(not_for_self), "spec": bad("Invalid receiver key")}),
            None,
        ),
        (
            "gobt-hello-bound",
            Dialect::GOBT,
            b"Hello World".to_vec(),
            Some(&bob),
            json!({"bitmind": ok(), "spec": ok(), "secret": ok()}),
            Some("gobt"),
        ),
        (
            "apex-unbound",
            Dialect::APEX,
            apex_body.clone(),
            None,
            json!({"apex": ok(), "bitmind": bad("Unknown Epistula version")}),
            None,
        ),
        (
            "apex-bound",
            Dialect::APEX,
            apex_body,
            Some(&bob),
            json!({"apex": bad("Signature Mismatch"), "secret": ok()}),
            Some("python"),
        ),
        (
            "apex-empty",
            Dialect::APEX,
            apex_empty,
            None,
            json!({"apex": ok()}),
            None,
        ),
    ];
    let mut vectors = Vec::new();
    for (id, dialect, body, receiver, expect, secret) in cases {
        let headers = sign_headers(
            &alice,
            &dialect,
            SignOptions {
                digest: BodyDigest::Raw(&body),
                signed_for: receiver,
                now_ms: TS,
                uuid: None,
            },
        )
        .await?;
        vectors.push(json!({
            "id": id,
            "body_hex": hex::encode(&body),
            "headers": pairs(&headers),
            "now_ms": TS,
            "sender": headers.signed_by,
            "receiver": receiver,
            "secret": secret,
            "expect": expect,
        }));
    }
    // The reference verifier accepts an unbound request when Signed-For is "".
    let headers = sign_headers(&alice, &Dialect::SPEC, SignOptions::new(&json_body, TS)).await?;
    let mut with_empty = pairs(&headers);
    with_empty
        .as_array_mut()
        .unwrap()
        .push(json!(["Epistula-Signed-For", ""]));
    vectors.push(json!({
        "id": "spec-json-unbound-empty-for",
        "body_hex": hex::encode(&json_body),
        "headers": with_empty,
        "now_ms": TS,
        "sender": headers.signed_by,
        "expect": {"spec": ok(), "bitmind": bad(not_for_self)},
    }));

    let canonical_sources = [
        r#"{"b": 1, "a": "x"}"#,
        r#"{}"#,
        r#"{"z": [1, 2.5, -0.0, 1e16, 1e-5, 0.1, 5e-324, 1.7976931348623157e308, 1e400, -1e400], "a": {"y": null, "x": true, "w": false}}"#,
        r#"{"big": 123456789012345678901234567890, "neg": -12345678901234567890123, "zero": -0, "f": 123456789.123456789}"#,
        r#"{"s": "\u00e9\ud83d\ude00\u0001\u001f\u007f\"\\/\b\f\n\r\t\u2028\uffff", "\u00e9": 1, "\ud83d\ude00": 2, "\uffff": 3, "Z": 4}"#,
        r#"[{"k": "v"}, [], "", 0]"#,
    ];
    let canonical: Vec<Value> = canonical_sources
        .iter()
        .map(|source| {
            let value: Value = serde_json::from_str(source).unwrap();
            json!({"source": source, "rust_hex": hex::encode(canonical_json(&value))})
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"vectors": vectors, "canonical": canonical}))?
    );
    Ok(())
}

struct Registered(HashSet<AccountId>);

impl CallerPolicy for Registered {
    fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        Box::pin(async move {
            if self.0.contains(&caller.hotkey) {
                Ok(Admission::default())
            } else {
                Err(PolicyDenied("Signer not in metagraph".into()))
            }
        })
    }
}

async fn echo(VerifiedEpistula(caller): VerifiedEpistula, body: Bytes) -> Json<Value> {
    Json(json!({
        "signed_by": caller.signed_by,
        "uuid": caller.uuid,
        "timestamp_ms": caller.timestamp_ms.to_string(),
        "sha256": hex::encode(Sha256::digest(&body)),
    }))
}

async fn health(VerifiedEpistula(caller): VerifiedEpistula) -> Json<Value> {
    Json(json!({"ok": true, "signed_by": caller.signed_by}))
}

fn layer(dialect: Dialect, policy: Arc<dyn CallerPolicy>) -> Result<EpistulaLayer> {
    let bob = signer("//Bob").account();
    let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
    let verifier = Arc::new(Verifier::new(bob, dialect, store)?);
    Ok(EpistulaLayer::new(verifier)
        .with_policy(policy)
        .with_claimed_sender_prefilter(true))
}

async fn serve() -> Result {
    let registered: Arc<dyn CallerPolicy> = Arc::new(Registered(
        ["//Alice", "//Bob", "//Charlie"]
            .iter()
            .map(|uri| signer(uri).account())
            .collect(),
    ));
    let app = Router::new()
        .route("/bitmind/echo", post(echo))
        .route("/bitmind/health", get(health))
        .layer(layer(Dialect::BITMIND, registered.clone())?)
        .merge(
            Router::new()
                .route("/rtao/echo", post(echo))
                .layer(layer(Dialect::RTAO, registered)?),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("LISTENING {}", listener.local_addr()?.port());
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        let _ = closed_tx.send(());
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = closed_rx.await;
        })
        .await?;
    Ok(())
}

async fn client(base: &str) -> Result {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let bob = ss58("//Bob");
    let body = br#"{"prompt": "hello", "n": 1}"#.to_vec();
    let mut failures = 0;
    let mut check = |name: &str, status: u16, detail: Option<&str>, got: (u16, Value)| {
        let got_detail = got
            .1
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let pass = got.0 == status && detail.is_none_or(|detail| got_detail.starts_with(detail));
        failures += usize::from(!pass);
        println!(
            "{} {name}: {} {}",
            if pass { "PASS" } else { "FAIL" },
            got.0,
            got.1
        );
    };
    let send = |client: EpistulaClient,
                method: reqwest::Method,
                path: &str,
                body: Vec<u8>,
                receiver: Option<String>| {
        let url = format!("{base}{path}");
        async move {
            let request = client.http().request(method, url).body(body).build()?;
            let response = client.execute(request, receiver.as_deref()).await?;
            let status = response.status().as_u16();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            Ok::<_, Box<dyn Error + Send + Sync>>((status, value))
        }
    };
    let as_signer = |uri: &str, dialect: Dialect| {
        EpistulaClient::new(http.clone(), Arc::new(signer(uri)), dialect)
    };
    let post = reqwest::Method::POST;

    check(
        "RTAO POST",
        200,
        None,
        send(
            as_signer("//Alice", Dialect::RTAO),
            post.clone(),
            "/echo",
            body.clone(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "BITMIND POST",
        200,
        None,
        send(
            as_signer("//Alice", Dialect::BITMIND),
            post.clone(),
            "/echo",
            body.clone(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "GOBT POST",
        200,
        None,
        send(
            as_signer("//Alice", Dialect::GOBT),
            post.clone(),
            "/echo",
            body.clone(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "RTAO GET empty body",
        200,
        None,
        send(
            as_signer("//Alice", Dialect::RTAO),
            reqwest::Method::GET,
            "/health",
            Vec::new(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "SPEC unbound",
        400,
        Some("Bad Request, message is not intended for self"),
        send(
            as_signer("//Alice", Dialect::SPEC),
            post.clone(),
            "/echo",
            body.clone(),
            None,
        )
        .await?,
    );
    check(
        "unregistered sender",
        401,
        Some("Signer not in metagraph"),
        send(
            as_signer("//Eve", Dialect::RTAO),
            post.clone(),
            "/echo",
            body.clone(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "low-stake sender",
        401,
        Some("Stake below minimum: 100"),
        send(
            as_signer("//Dave", Dialect::RTAO),
            post.clone(),
            "/echo",
            body.clone(),
            Some(bob.clone()),
        )
        .await?,
    );
    check(
        "Apex headers",
        400,
        Some("Unknown Epistula version"),
        send(
            as_signer("//Alice", Dialect::APEX),
            post.clone(),
            "/echo",
            body.clone(),
            None,
        )
        .await?,
    );

    // Hand-built requests: tampered body, stale timestamp, and a replay.
    let alice = signer("//Alice");
    let now = rtao_compat_epistula::now_ms(&rtao_btauth::SystemClock::default())?;
    let raw = |headers: &rtao_compat_epistula::EpistulaHeaders, body: Vec<u8>| {
        let request = http
            .post(format!("{base}/echo"))
            .headers(headers.to_header_map().unwrap())
            .body(body);
        async move {
            let response = request.send().await?;
            let status = response.status().as_u16();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            Ok::<_, Box<dyn Error + Send + Sync>>((status, value))
        }
    };
    let fresh = sign_headers(
        &alice,
        &Dialect::RTAO,
        SignOptions::new(&body, now).signed_for(&bob),
    )
    .await?;
    let mut tampered = body.clone();
    tampered.push(b' ');
    check(
        "tampered body",
        400,
        Some("Signature Mismatch"),
        raw(&fresh, tampered).await?,
    );
    check("first send", 200, None, raw(&fresh, body.clone()).await?);
    check(
        "replay accepted (BitMind keeps no replay store)",
        200,
        None,
        raw(&fresh, body.clone()).await?,
    );
    let stale = sign_headers(
        &alice,
        &Dialect::RTAO,
        SignOptions::new(&body, now - 20_000).signed_for(&bob),
    )
    .await?;
    check(
        "stale",
        400,
        Some("Request is too stale: 20."),
        raw(&stale, body.clone()).await?,
    );
    let future = sign_headers(
        &alice,
        &Dialect::RTAO,
        SignOptions::new(&body, now + 16_000).signed_for(&bob),
    )
    .await?;
    check(
        "future",
        400,
        Some("Request timestamp is in the future: 1"),
        raw(&future, body.clone()).await?,
    );
    println!("client: {failures} failures");
    if failures > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("emit-vectors") => emit_vectors().await,
        Some("serve") => serve().await,
        Some("client") => client(args.get(2).ok_or("client needs a base URL")?).await,
        _ => Err("usage: epistula_interop emit-vectors | serve | client <base-url>".into()),
    }
}
