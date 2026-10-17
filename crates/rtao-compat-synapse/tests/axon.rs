//! The Rust axon answers scripted requests as the captured bittensor 10.5.0
//! axon did (`responses.json`), plus rtao's documented divergences.

#![cfg(all(feature = "axon", feature = "dendrite", feature = "tensor"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "../examples/interop/synapses.rs"]
mod synapses;

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo};
use http::Request;
use http_body_util::BodyExt;
use rtao_compat_synapse::{
    BaseSynapse, CallOptions, Dendrite, EMPTY_BODY_HASH, PreparedRequest, Synapse, V10AxonConfig,
};
use rtao_primitives::{CryptoType, Uid, axon::AxonTarget};
use rtao_signer::KeypairSigner;
use serde_json::Value;
use synapses::*;
use tower::ServiceExt;

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/bittensor-10.5.0"
);
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";

struct Reply {
    status: u16,
    headers: http::HeaderMap,
    body: Value,
}

struct Harness {
    router: axum::Router,
    dendrite: Dendrite,
    target: AxonTarget,
}

impl Harness {
    fn new() -> Self {
        let bob = Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519).unwrap());
        let alice = Arc::new(KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap());
        Self {
            router: build_axon(bob, V10AxonConfig::default()).router(),
            dendrite: Dendrite::new(alice).unwrap(),
            target: AxonTarget {
                uid: Uid(0),
                hotkey: BOB.parse().unwrap(),
                base_url: "http://127.0.0.1:8091".parse().unwrap(),
            },
        }
    }

    async fn prepare<S: Synapse>(&self, body: S) -> PreparedRequest<S> {
        self.dendrite
            .prepare(&self.target, body, &CallOptions::default())
            .await
            .unwrap()
    }

    async fn send<S>(
        &self,
        prepared: &PreparedRequest<S>,
        path: Option<&str>,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Reply {
        self.send_method(http::Method::POST, prepared, path, body, content_type)
            .await
    }

    async fn send_method<S>(
        &self,
        method: http::Method,
        prepared: &PreparedRequest<S>,
        path: Option<&str>,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Reply {
        let path = path.map_or_else(|| prepared.url.path().to_owned(), |path| format!("/{path}"));
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in &prepared.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        let mut request = request
            .body(Body::from(body.unwrap_or(&prepared.body).to_vec()))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        Reply {
            status,
            headers,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }
}

fn set(prepared: &mut PreparedRequest<impl Synapse>, name: &str, value: Option<&str>) {
    prepared.headers.retain(|(header, _)| header != name);
    if let Some(value) = value {
        prepared.headers.push((name.to_owned(), value.to_owned()));
    }
}

fn internal(text: &str) -> bool {
    text.strip_prefix("Internal Server Error #")
        .is_some_and(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

fn header(reply: &Reply, name: &str) -> Option<String> {
    reply
        .headers
        .get(name)
        .map(|value| value.to_str().unwrap().to_owned())
}

#[tokio::test]
async fn matches_captured_python_responses() {
    let fixtures: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/responses.json")).unwrap())
            .unwrap();
    let harness = Harness::new();
    let json = Some("application/json");

    let mut results = Vec::new();
    let p = harness.prepare(BaseSynapse {}).await;
    results.push(("ping", harness.send(&p, None, None, json).await));
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "unknown_route",
        harness.send(&p, Some("Nope"), None, json).await,
    ));
    let p = harness.prepare(Deny {}).await;
    results.push(("blacklisted", harness.send(&p, None, None, json).await));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(&mut p, "bt_header_dendrite_signature", None);
    results.push((
        "missing_signature",
        harness.send(&p, None, None, json).await,
    ));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(
        &mut p,
        "bt_header_dendrite_signature",
        Some(&"00".repeat(64)),
    );
    results.push((
        "signature_without_0x",
        harness.send(&p, None, None, json).await,
    ));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(
        &mut p,
        "bt_header_dendrite_signature",
        Some(&format!("0x{}", "00".repeat(63))),
    );
    results.push((
        "signature_63_bytes",
        harness.send(&p, None, None, json).await,
    ));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(&mut p, "bt_header_dendrite_hotkey", None);
    results.push(("missing_hotkey", harness.send(&p, None, None, json).await));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(&mut p, "bt_header_dendrite_hotkey", Some("0OIl"));
    results.push(("bad_hotkey", harness.send(&p, None, None, json).await));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(&mut p, "bt_header_dendrite_nonce", Some("abc"));
    results.push((
        "header_parse_failure",
        harness.send(&p, None, None, json).await,
    ));
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "invalid_json",
        harness.send(&p, None, Some(b"{"), json).await,
    ));
    let p = harness.prepare(BaseSynapse {}).await;
    results.push(("empty_body", harness.send(&p, None, Some(b""), json).await));
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "text_plain",
        harness.send(&p, None, None, Some("text/plain")).await,
    ));
    let p = harness.prepare(Status404 {}).await;
    results.push(("status_404", harness.send(&p, None, None, json).await));
    let p = harness.prepare(Boom {}).await;
    results.push(("boom", harness.send(&p, None, None, json).await));
    let mut p = harness
        .prepare(Mixed {
            i: 1,
            ..Mixed::default()
        })
        .await;
    set(&mut p, "computed_body_hash", Some(EMPTY_BODY_HASH));
    results.push((
        "hash_header_tampered",
        harness.send(&p, None, None, json).await,
    ));
    // FastAPI's 404/405 come after blacklist and verification, as in Python.
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "extra_segment",
        harness.send(&p, Some("Synapse/extra"), None, json).await,
    ));
    let mut p = harness.prepare(BaseSynapse {}).await;
    set(&mut p, "bt_header_dendrite_signature", None);
    results.push((
        "extra_segment_unsigned",
        harness.send(&p, Some("Synapse/extra"), None, json).await,
    ));
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "put_method",
        harness
            .send_method(http::Method::PUT, &p, None, None, json)
            .await,
    ));
    // Uvicorn percent-decodes the path before routing.
    let p = harness.prepare(BaseSynapse {}).await;
    results.push((
        "percent_encoded",
        harness.send(&p, Some("Syn%61pse"), None, json).await,
    ));

    for (case, reply) in &results {
        let python = &fixtures[*case];
        assert_eq!(
            u64::from(reply.status),
            python["status"].as_u64().unwrap(),
            "{case} status"
        );
        let expected = python["body"].get("message").and_then(Value::as_str);
        let actual = reply.body.get("message").and_then(Value::as_str);
        match (expected, actual) {
            (Some(expected), Some(actual)) if internal(expected) => {
                assert!(internal(actual), "{case}: {actual}")
            }
            (Some(expected), Some(actual))
                if expected.starts_with("Not Verified with error: Signature mismatch with ") =>
            {
                assert!(
                    actual.starts_with("Not Verified with error: Signature mismatch with "),
                    "{case}: {actual}"
                );
            }
            (expected, actual) => assert_eq!(actual, expected, "{case} message"),
        }
        if *case == "invalid_json" {
            assert!(reply.body["detail"].is_array());
        } else {
            assert_eq!(
                reply.body.get("detail"),
                python["body"].get("detail"),
                "{case} detail"
            );
        }
        assert_eq!(
            header(reply, "allow").is_some(),
            python["headers"].get("allow").is_some(),
            "{case} allow header"
        );
    }

    // The success header set equals Python's, plus the verified TCP port.
    let (_, ping) = &results[0];
    let python = &fixtures["ping"];
    let mut names: Vec<String> = ping.headers.keys().map(|name| name.to_string()).collect();
    names.retain(|name| name != "content-length" && name != "bt_header_dendrite_port");
    names.sort();
    let python_names: Vec<String> = python["header_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, python_names);
    for (name, value) in python["headers"].as_object().unwrap() {
        if name == "bt_header_dendrite_hotkey" || name == "bt_header_axon_hotkey" {
            continue;
        }
        assert_eq!(
            header(ping, name).as_deref(),
            value.as_str(),
            "ping header {name}"
        );
    }
    // Error responses carry the header synapse's axon status.
    let (_, blacklisted) = &results[2];
    assert_eq!(
        header(blacklisted, "bt_header_axon_status_code").as_deref(),
        Some("403")
    );
    assert_eq!(
        header(blacklisted, "bt_header_axon_status_message").as_deref(),
        Some("Forbidden. Key is blacklisted: denied by test.")
    );
    assert!(
        header(blacklisted, "bt_header_axon_signature").is_none(),
        "no axon signing oracle"
    );
    let (_, status_404) = &results[12];
    assert_eq!(status_404.body["name"], "Status404");
    assert_eq!(status_404.body["axon"]["status_code"], 404);
}

#[tokio::test]
async fn strict_divergences() {
    let harness = Harness::new();
    let json = Some("application/json");

    // The body cannot choose the response status or the caller identity.
    let mut p = harness.prepare(WhoAmI::default()).await;
    let mut body: Value = serde_json::from_slice(&p.body).unwrap();
    body["dendrite"]["hotkey"] = Value::from(BOB);
    body["axon"]["status_code"] = Value::from(201);
    p.body = serde_json::to_vec(&body).unwrap();
    let reply = harness.send(&p, None, None, json).await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.body["seen"],
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    );

    // Duplicate signed headers are refused (500, as an unparsable header).
    let mut p = harness.prepare(BaseSynapse {}).await;
    let nonce = p
        .headers
        .iter()
        .find(|(n, _)| n == "bt_header_dendrite_nonce")
        .unwrap()
        .1
        .clone();
    p.headers.push(("bt_header_dendrite_nonce".into(), nonce));
    assert_eq!(harness.send(&p, None, None, json).await.status, 500);

    // Oversized bodies answer 413.
    let bob = Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519).unwrap());
    let small = Harness {
        router: build_axon(
            bob,
            V10AxonConfig {
                body_limit: 64,
                ..V10AxonConfig::default()
            },
        )
        .router(),
        ..Harness::new()
    };
    let p = small.prepare(BaseSynapse {}).await;
    let reply = small.send(&p, None, None, json).await;
    assert_eq!(
        (reply.status, reply.body["message"].as_str()),
        (413, Some("Payload Too Large"))
    );

    // Streaming replies.
    let p = harness.prepare(Echo::default()).await;
    let mut request = Request::post("/Echo");
    for (name, value) in &p.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let request = request
        .header("content-type", "application/json")
        .body(Body::from(p.body.clone()))
        .unwrap();
    let response = harness.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"tok\ntok\ntok\n");
}
