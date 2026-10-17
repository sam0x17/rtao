//! Axon options and hooks, and Python dendrite inputs the Rust dendrite never
//! sends (non-finite floats, Latin-1 header text).

#![cfg(all(feature = "axon", feature = "dendrite", feature = "tensor"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "../examples/interop/synapses.rs"]
mod synapses;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{body::Body, extract::ConnectInfo};
use futures::future::BoxFuture;
use http::{HeaderValue, Request};
use http_body_util::BodyExt;
use num_bigint::BigInt;
use rtao_compat_synapse::{
    Axon, AxonSignature, BaseSynapse, CallOptions, DefaultHooks, Dendrite, Envelope, Hooks,
    PreparedRequest, PriorityError, Reply, RequestHead, Synapse, V10AxonConfig, V10VerifyPolicy,
    VerifiedHead, v10_message,
};
use rtao_primitives::{CryptoType, Uid, axon::AxonTarget};
use rtao_signer::{KeypairSigner, Signer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use synapses::*;
use tower::ServiceExt;

const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
const CHARLIE: &str = "5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y";

struct Raw {
    status: u16,
    headers: http::HeaderMap,
    bytes: Vec<u8>,
}

impl Raw {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }
    fn message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or("<none>")
            .to_owned()
    }
    fn header(&self, name: &str) -> Option<Vec<u8>> {
        self.headers
            .get(name)
            .map(|value| value.as_bytes().to_vec())
    }
}

fn bob() -> Arc<KeypairSigner> {
    Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519).unwrap())
}

fn alice() -> Arc<KeypairSigner> {
    Arc::new(KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap())
}

fn target() -> AxonTarget {
    AxonTarget {
        uid: Uid(0),
        hotkey: BOB.parse().unwrap(),
        base_url: "http://127.0.0.1:8091".parse().unwrap(),
    }
}

async fn prepare<S: Synapse>(dendrite: &Dendrite, body: S) -> PreparedRequest<S> {
    dendrite
        .prepare(&target(), body, &CallOptions::default())
        .await
        .unwrap()
}

/// Send with raw header bytes; `overrides` replace prepared headers.
async fn send<S>(
    router: &axum::Router,
    prepared: &PreparedRequest<S>,
    overrides: &[(&str, &[u8])],
    body: Option<Vec<u8>>,
) -> Raw {
    let mut request = Request::post(prepared.url.path());
    for (name, value) in &prepared.headers {
        if overrides.iter().all(|(other, _)| other != name) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    for (name, value) in overrides {
        request = request.header(*name, HeaderValue::from_bytes(value).unwrap());
    }
    let mut request = request
        .header("content-type", "application/json")
        .body(Body::from(body.unwrap_or_else(|| prepared.body.clone())))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Raw {
        status,
        headers,
        bytes,
    }
}

fn header_text(prepared: &PreparedRequest<impl Synapse>, name: &str) -> String {
    prepared
        .headers
        .iter()
        .find(|(header, _)| header == name)
        .unwrap()
        .1
        .clone()
}

fn is_internal(message: &str) -> bool {
    message
        .strip_prefix("Internal Server Error #")
        .is_some_and(|id| id.len() == 36)
}

#[tokio::test]
async fn after_verify_signs_only_verified_text() {
    // As in Python, the axon signature appears on error responses, which
    // carry the header synapse; `Boom` fails after verification.
    let config = V10AxonConfig {
        axon_signature: AxonSignature::AfterVerify,
        ..V10AxonConfig::default()
    };
    let router = build_axon(bob(), config).router();
    let dendrite = Dendrite::new(alice()).unwrap();

    let prepared = prepare(&dendrite, Boom {}).await;
    let reply = send(&router, &prepared, &[], None).await;
    assert_eq!(reply.status, 500);
    let text = |name: &str| String::from_utf8(reply.header(name).unwrap()).unwrap();
    let message = format!(
        "{}.{ALICE}.{BOB}.{}",
        text("bt_header_axon_nonce"),
        text("bt_header_axon_uuid")
    );
    let signature: [u8; 64] =
        hex::decode(text("bt_header_axon_signature").trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap();
    assert!(
        rtao_core::keys::verify(
            message.as_bytes(),
            &signature,
            BOB,
            u8::from(CryptoType::Sr25519)
        )
        .unwrap()
    );

    // An unsigned axon-hotkey claim that is not this axon's is never signed.
    let prepared = prepare(&dendrite, Boom {}).await;
    let reply = send(
        &router,
        &prepared,
        &[("bt_header_axon_hotkey", CHARLIE.as_bytes())],
        None,
    )
    .await;
    assert_eq!(reply.status, 500);
    assert!(reply.header("bt_header_axon_signature").is_none());

    // Rejected requests are never signed.
    let prepared = prepare(&dendrite, Boom {}).await;
    let reply = send(
        &router,
        &prepared,
        &[("bt_header_dendrite_signature", b"0x00")],
        None,
    )
    .await;
    assert_eq!(reply.status, 401);
    assert!(reply.header("bt_header_axon_signature").is_none());

    // The default never signs.
    let router = build_axon(bob(), V10AxonConfig::default()).router();
    let prepared = prepare(&dendrite, Boom {}).await;
    let reply = send(&router, &prepared, &[], None).await;
    assert_eq!(reply.status, 500);
    assert!(reply.header("bt_header_axon_signature").is_none());
}

#[tokio::test]
async fn handler_timeout_answers_408() {
    let config = V10AxonConfig {
        handler_timeout: Some(Duration::from_millis(50)),
        ..V10AxonConfig::default()
    };
    let router = build_axon(bob(), config).router();
    let dendrite = Dendrite::new(alice()).unwrap();
    let prepared = prepare(&dendrite, Slow { delay: 5.0 }).await;
    let reply = send(&router, &prepared, &[], None).await;
    assert_eq!(
        (reply.status, reply.message().as_str()),
        (408, "Response timeout after: 12.0s")
    );
    assert_eq!(reply.header("bt_header_axon_status_code").unwrap(), b"408");
}

macro_rules! empty_synapse {
    ($($name:ident),*) => {$(
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        struct $name {}
        impl Synapse for $name {
            const NAME: &'static str = stringify!($name);
        }
    )*};
}
empty_synapse!(VerifyDenied, PriorityTimeout, PriorityFailed, UnicodeDenied);

#[derive(Clone, Copy)]
enum Mode {
    Verify,
    Timeout,
    Failed,
    Unicode,
}

struct TestHooks(Mode);

impl<S: Synapse> Hooks<S> for TestHooks {
    fn blacklist<'a>(&'a self, _head: &'a RequestHead) -> BoxFuture<'a, Result<(), String>> {
        let mode = self.0;
        Box::pin(async move {
            match mode {
                Mode::Unicode => Err("bad \u{2713}".into()),
                _ => Ok(()),
            }
        })
    }
    fn verify<'a>(&'a self, head: &'a VerifiedHead) -> BoxFuture<'a, Result<(), String>> {
        let mode = self.0;
        Box::pin(async move {
            assert_eq!(head.caller().to_string(), ALICE);
            match mode {
                Mode::Verify => Err("caller not registered".into()),
                _ => Ok(()),
            }
        })
    }
    fn priority<'a>(
        &'a self,
        _head: &'a VerifiedHead,
    ) -> BoxFuture<'a, Result<f64, PriorityError>> {
        let mode = self.0;
        Box::pin(async move {
            match mode {
                Mode::Timeout => Err(PriorityError::Timeout),
                Mode::Failed => Err(PriorityError::Failed("queue closed".into())),
                _ => Ok(1.0),
            }
        })
    }
}

async fn echo<S: Synapse>(
    _: VerifiedHead,
    envelope: Envelope<S>,
) -> Result<Reply<S>, rtao_compat_synapse::SynapseError> {
    Ok(Reply::Synapse(envelope))
}

#[tokio::test]
async fn hook_failures_follow_python() {
    let router = Axon::new(bob(), V10AxonConfig::default())
        .unwrap()
        .attach::<VerifyDenied, _, _, _>(TestHooks(Mode::Verify), echo)
        .attach::<PriorityTimeout, _, _, _>(TestHooks(Mode::Timeout), echo)
        .attach::<PriorityFailed, _, _, _>(TestHooks(Mode::Failed), echo)
        .attach::<UnicodeDenied, _, _, _>(TestHooks(Mode::Unicode), echo)
        .router();
    let dendrite = Dendrite::new(alice()).unwrap();

    let reply = send(
        &router,
        &prepare(&dendrite, VerifyDenied {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(
        (reply.status, reply.message().as_str()),
        (401, "Not Verified with error: caller not registered")
    );
    let reply = send(
        &router,
        &prepare(&dendrite, PriorityTimeout {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(
        (reply.status, reply.message().as_str()),
        (408, "Response timeout after: 12.0s")
    );
    let reply = send(
        &router,
        &prepare(&dendrite, PriorityFailed {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(reply.status, 500);
    assert!(is_internal(&reply.message()), "{}", reply.message());

    // Starlette cannot Latin-1 encode the reflected reason, so Python's
    // error handler fails and ServerErrorMiddleware answers plain text.
    let reply = send(
        &router,
        &prepare(&dendrite, UnicodeDenied {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(reply.status, 500);
    assert_eq!(reply.bytes, b"Internal Server Error");
    assert_eq!(
        reply.header("content-type").unwrap(),
        b"text/plain; charset=utf-8"
    );
    assert!(reply.header("bt_header_axon_status_code").is_none());
}

#[tokio::test]
async fn latin1_header_text_is_reflected_byte_for_byte() {
    let router = Axon::new(bob(), V10AxonConfig::default()).unwrap().router();
    let dendrite = Dendrite::new(alice()).unwrap();
    let prepared = prepare(&dendrite, BaseSynapse {}).await;
    let reply = send(
        &router,
        &prepared,
        &[("bt_header_dendrite_uuid", b"u\xe9")],
        None,
    )
    .await;
    assert_eq!(reply.status, 401);
    let message = reply.message();
    assert!(
        message.starts_with("Not Verified with error: Signature mismatch with ")
            && message.contains(".u\u{e9}."),
        "{message}"
    );
    let header = reply.header("bt_header_axon_status_message").unwrap();
    assert!(header.windows(3).any(|window| window == b".u\xe9"));
    assert_eq!(reply.header("bt_header_dendrite_uuid").unwrap(), b"u\xe9");
}

/// Re-sign `prepared` for `body`, a Python-dendrite JSON text whose hashed
/// values are those of `envelope`.
async fn python_body<S: Synapse>(
    prepared: &PreparedRequest<S>,
    envelope: &Envelope<S>,
) -> (String, String) {
    let hash = envelope.body_hash().unwrap();
    let nonce: BigInt = header_text(prepared, "bt_header_dendrite_nonce")
        .parse()
        .unwrap();
    let uuid = header_text(prepared, "bt_header_dendrite_uuid");
    let message = v10_message(&nonce, ALICE, BOB, Some(&uuid), Some(&hash));
    let signature = alice().sign_raw(message.as_bytes()).await.unwrap();
    (hash, format!("0x{}", hex::encode(signature)))
}

#[tokio::test]
async fn python_non_finite_request_floats_are_accepted() {
    let router = build_axon(bob(), V10AxonConfig::default()).router();
    let dendrite = Dendrite::new(alice()).unwrap();

    for count in [2, 20_000] {
        // 20,000 values exceed the inline decode size and run off-worker.
        let values: Vec<f64> = (0..count)
            .map(|index| {
                if index % 2 == 0 {
                    f64::INFINITY
                } else {
                    f64::NEG_INFINITY
                }
            })
            .collect();
        let prepared = prepare(&dendrite, NanReq::default()).await;
        let python = NanReq {
            v: f64::NAN,
            vs: values.clone(),
            seen: None,
        };
        let tokens: Vec<&str> = values
            .iter()
            .map(|value| {
                if *value > 0.0 {
                    "Infinity"
                } else {
                    "-Infinity"
                }
            })
            .collect();
        let text = String::from_utf8(prepared.body.clone()).unwrap();
        let text = text.replace(
            r#""v": 0.0, "vs": []"#,
            &format!(r#""v": NaN, "vs": [{}]"#, tokens.join(", ")),
        );
        assert!(text.contains("NaN"));
        let (hash, signature) = python_body(&prepared, &Envelope::new(python)).await;
        let reply = send(
            &router,
            &prepared,
            &[
                ("computed_body_hash", hash.as_bytes()),
                ("bt_header_dendrite_signature", signature.as_bytes()),
            ],
            Some(text.into_bytes()),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.bytes)
        );
        let reprs: Vec<&str> = values
            .iter()
            .map(|value| if *value > 0.0 { "inf" } else { "-inf" })
            .collect();
        assert_eq!(
            reply.json()["seen"],
            format!("(nan, [{}])", reprs.join(", "))
        );
    }

    // Still not JSON for Python either: FastAPI's 422.
    let prepared = prepare(&dendrite, NanReq::default()).await;
    let reply = send(&router, &prepared, &[], Some(br#"{"v": NaN,"#.to_vec())).await;
    assert_eq!(reply.status, 422);
    assert!(reply.json()["detail"].is_array());
}

#[tokio::test]
async fn nonce_table_policy_is_explicit() {
    let python = V10AxonConfig {
        policy: V10VerifyPolicy::python(),
        ..V10AxonConfig::default()
    };
    let error = Axon::new(bob(), python.clone()).unwrap_err();
    assert!(
        error.to_string().contains("allow_unbounded_nonce_table"),
        "{error}"
    );
    let unbounded = V10AxonConfig {
        allow_unbounded_nonce_table: true,
        ..python
    };
    let router = Axon::new(bob(), unbounded).unwrap().router();
    let dendrite = Dendrite::new(alice()).unwrap();
    let reply = send(
        &router,
        &prepare(&dendrite, BaseSynapse {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(reply.status, 200);

    // with_clock rebuilds the table on the new clock.
    struct Late;
    impl rtao_btauth::Clock for Late {
        fn now_ns(&self) -> Result<u64, rtao_btauth::ClockError> {
            Ok(4_000_000_000_000_000_000)
        }
    }
    let router = Axon::new(bob(), V10AxonConfig::default())
        .unwrap()
        .with_clock(Arc::new(Late))
        .unwrap()
        .attach::<BaseSynapse, _, _, _>(DefaultHooks, echo)
        .router();
    let reply = send(
        &router,
        &prepare(&dendrite, BaseSynapse {}).await,
        &[],
        None,
    )
    .await;
    assert_eq!(reply.status, 401);
    assert!(
        reply
            .message()
            .starts_with("Not Verified with error: Nonce is too old"),
        "{}",
        reply.message()
    );
}

#[tokio::test]
async fn percent_decoded_unknown_names_follow_uvicorn() {
    let router = Axon::new(bob(), V10AxonConfig::default()).unwrap().router();
    let dendrite = Dendrite::new(alice()).unwrap();
    let prepared = prepare(&dendrite, BaseSynapse {}).await;
    let mut request = Request::post("/Syn%2Fapse%ZZ");
    for (name, value) in &prepared.headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(prepared.body.clone())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["message"],
        "Synapse name 'Syn' not found. Available synapses ['Synapse']"
    );
}
