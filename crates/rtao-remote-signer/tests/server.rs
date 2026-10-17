//! S1-S14: the reference server against golden vectors, offline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, body::Body};
use common::*;
use http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use num_bigint::BigInt;
use rtao_btauth::{
    BoxFuture, Clock, InMemoryNonceStore, NonceStore, NonceStoreConfig, StoreError, VerifyConfig,
    WrappedPolicy, server::LimiterConfig,
};
use rtao_primitives::{AccountId, Balance, Tao};
use rtao_remote_signer::{
    server::{
        AuthorizedClient, PinnedRuntimeSource, RemoteSignerServer, ServedAccount, ServerConfig,
    },
    wire::{SignRequest, SignResponse, WirePurpose},
};
use rtao_signer::{KeypairSigner, Signer};
use serde_json::{Value as Json, json};
use tower::ServiceExt;

const SERVER_NOW: u64 = NOW + 1_000_000_000;

fn source() -> Arc<PinnedRuntimeSource> {
    Arc::new(PinnedRuntimeSource::new(runtime(), SPEC_NAME))
}

struct Harness {
    router: Router,
    backend: CountingSigner,
    clock: Arc<StepClock>,
}

fn config(backend: &CountingSigner) -> ServerConfig {
    let mut cfg = ServerConfig::new(genesis());
    cfg.accounts = vec![ServedAccount::new(Arc::new(backend.clone()))];
    cfg.clients = vec![AuthorizedClient {
        client: charlie().account(),
        accounts: BTreeSet::from([alice().account()]),
    }];
    cfg
}

fn harness_with(edit: impl FnOnce(&mut ServerConfig), store: Arc<dyn NonceStore>) -> Harness {
    let backend = CountingSigner::new(alice());
    let mut cfg = config(&backend);
    edit(&mut cfg);
    let server = RemoteSignerServer::new(cfg, source(), store)
        .unwrap()
        .with_clock(Arc::new(FixedClock(SERVER_NOW)));
    Harness {
        router: server.router(),
        backend,
        clock: StepClock::at(NOW),
    }
}

fn harness(edit: impl FnOnce(&mut ServerConfig)) -> Harness {
    harness_with(edit, Arc::new(InMemoryNonceStore::new()))
}

struct Reply {
    status: StatusCode,
    body: Json,
}

impl Reply {
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }
    fn detail(&self) -> &str {
        self.body["detail"].as_str().unwrap_or("")
    }
}

async fn send(
    router: &Router,
    method: Method,
    target: &str,
    headers: HeaderMap,
    body: Vec<u8>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(target);
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let request = request
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or(Json::String(String::from_utf8_lossy(&bytes).into_owned()));
    Reply { status, body }
}

async fn headers_for(
    signer: &KeypairSigner,
    method: &Method,
    target: &str,
    body: &[u8],
    receiver: Option<AccountId>,
    clock: &dyn Clock,
) -> HeaderMap {
    rtao_btauth::sign(signer, method, target, body, receiver.as_ref(), clock)
        .await
        .unwrap()
}

impl Harness {
    async fn post(&self, body: Vec<u8>) -> Reply {
        let headers = headers_for(
            &charlie(),
            &Method::POST,
            "/sign",
            &body,
            Some(alice().account()),
            self.clock.as_ref(),
        )
        .await;
        send(&self.router, Method::POST, "/sign", headers, body).await
    }
    async fn sign(&self, request: &SignRequest) -> Reply {
        self.post(serde_json::to_vec(request).unwrap()).await
    }
}

fn assert_signed(reply: &Reply, payload: &[u8]) {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let response: SignResponse = serde_json::from_value(reply.body.clone()).unwrap();
    assert_eq!(response.account.0, alice().account());
    let signature = response
        .signature(rtao_primitives::CryptoType::Sr25519)
        .unwrap();
    assert!(verify(payload, &signature.bytes, alice().account()));
}

fn assert_refused(reply: &Reply, status: StatusCode, code: &str) {
    assert_eq!(reply.status, status, "{}", reply.body);
    assert_eq!(reply.code(), code, "{}", reply.body);
}

#[tokio::test]
async fn s1_v1_is_signed_over_the_golden_payload() {
    let h = harness(|_| {});
    let vector = v1();
    let reply = h.sign(&vector.body()).await;
    assert_signed(&reply, &vector.payload);
    assert_eq!(h.backend.count(), 1);
}

#[tokio::test]
async fn s2_a_verbatim_replay_is_refused_by_the_nonce_store() {
    let h = harness(|_| {});
    let body = v1().body_bytes();
    let headers = headers_for(
        &charlie(),
        &Method::POST,
        "/sign",
        &body,
        Some(alice().account()),
        h.clock.as_ref(),
    )
    .await;
    let first = send(
        &h.router,
        Method::POST,
        "/sign",
        headers.clone(),
        body.clone(),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = send(&h.router, Method::POST, "/sign", headers, body).await;
    assert_eq!(second.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        second.body,
        json!({"detail": "this nonce was already accepted from this hotkey"})
    );
    assert_eq!(h.backend.count(), 1);
}

#[tokio::test]
async fn s3_tampering_in_transit_breaks_the_btauth_signature() {
    let h = harness(|_| {});
    let body = v1().body_bytes();
    let headers = headers_for(
        &charlie(),
        &Method::POST,
        "/sign",
        &body,
        Some(alice().account()),
        h.clock.as_ref(),
    )
    .await;
    let mut flipped = body.clone();
    let index = flipped.len() / 2;
    flipped[index] ^= 0x01;
    let reply = send(&h.router, Method::POST, "/sign", headers.clone(), flipped).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.detail(),
        "signature does not verify against the claimed hotkey"
    );
    let reply = send(
        &h.router,
        Method::POST,
        "/sign?x=1",
        headers.clone(),
        body.clone(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.detail(),
        "signature does not verify against the claimed hotkey"
    );
    let reply = send(&h.router, Method::GET, "/sign", headers, body).await;
    assert_ne!(reply.status, StatusCode::OK);
    assert_eq!(h.backend.count(), 0);
}

#[tokio::test]
async fn s4_receiver_and_freshness_are_enforced() {
    let h = harness(|_| {});
    let body = v1().body_bytes();
    let check = |receiver: Option<AccountId>, clock: Arc<dyn Clock>, detail: &'static str| {
        let h = &h;
        let body = body.clone();
        async move {
            let headers = headers_for(
                &charlie(),
                &Method::POST,
                "/sign",
                &body,
                receiver,
                clock.as_ref(),
            )
            .await;
            let reply = send(&h.router, Method::POST, "/sign", headers, body).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{detail}");
            assert_eq!(reply.detail(), detail);
        }
    };
    check(
        Some(bob().account()),
        StepClock::at(NOW),
        "request was signed for a different receiving hotkey",
    )
    .await;
    check(
        Some(alice().account()),
        StepClock::at(SERVER_NOW - 11_000_000_000),
        "nonce is older than the 10s freshness window",
    )
    .await;
    check(
        Some(alice().account()),
        StepClock::at(SERVER_NOW + 3_000_000_000),
        "nonce is more than 2s in the future",
    )
    .await;
    check(
        None,
        StepClock::at(NOW),
        "missing X-Bittensor-Receiver: this server requires receiver-bound requests",
    )
    .await;
    assert_eq!(h.backend.count(), 0);
}

struct RecordingStore {
    inner: InMemoryNonceStore,
    calls: AtomicUsize,
}
impl NonceStore for RecordingStore {
    fn retention(&self) -> Duration {
        self.inner.retention()
    }
    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce_ns: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.check_and_store(hotkey, nonce_ns)
    }
}

#[tokio::test]
async fn s5_unknown_clients_are_refused_before_the_replay_store() {
    let store = Arc::new(RecordingStore {
        inner: InMemoryNonceStore::new(),
        calls: AtomicUsize::new(0),
    });
    let h = harness_with(|_| {}, store.clone());
    let body = v1().body_bytes();
    let headers = headers_for(
        &dave(),
        &Method::POST,
        "/sign",
        &body,
        Some(alice().account()),
        h.clock.as_ref(),
    )
    .await;
    let reply = send(&h.router, Method::POST, "/sign", headers, body.clone()).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.detail(), "client key is not authorized");
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    // An authorized client does reach the store.
    let reply = h.post(body).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.backend.count(), 1);
}

fn transfer_12346() -> Vec<u8> {
    let mut call = v1().call;
    let length = call.len();
    call[length - 2..].copy_from_slice(&[0xe9, 0xc0]);
    call
}

type Edit = Box<dyn Fn(&mut SignRequest)>;

#[tokio::test]
async fn s6_internally_inconsistent_authenticated_bodies_are_tampered() {
    let h = harness(|_| {});
    let base = v1().body();
    let cases: Vec<(&str, Edit)> = vec![
        ("payload byte", Box::new(|body| body.payload.0[40] ^= 1)),
        (
            "nonce in included_in_extrinsic",
            Box::new(|body| body.included_in_extrinsic.0[2] = 0x10),
        ),
        (
            "genesis byte in included_in_signed_data",
            Box::new(|body| body.included_in_signed_data.0[8] ^= 1),
        ),
        (
            "payload_json.method",
            Box::new(|body| {
                body.payload_json.insert(
                    "method".into(),
                    json!(format!("0x{}", hex::encode(transfer_12346()))),
                );
            }),
        ),
        (
            "payload_json.nonce",
            Box::new(|body| {
                body.payload_json
                    .insert("nonce".into(), json!("0x00000004"));
            }),
        ),
        (
            "payload_json.tip",
            Box::new(|body| {
                body.payload_json
                    .insert("tip".into(), json!("0x00000000000000000000000000000001"));
            }),
        ),
        (
            "payload_json.era",
            Box::new(|body| {
                body.payload_json.insert("era".into(), json!("0x3500"));
            }),
        ),
        (
            "call 12346 with payload 12345",
            Box::new(|body| {
                let call = transfer_12346();
                body.payload_json
                    .insert("method".into(), json!(format!("0x{}", hex::encode(&call))));
                body.call = call.into();
            }),
        ),
        (
            "extra payload_json key",
            Box::new(|body| {
                body.payload_json.insert("assetIdx".into(), json!("0x00"));
            }),
        ),
    ];
    for (name, edit) in cases {
        let mut body = base.clone();
        edit(&mut body);
        let reply = h.sign(&body).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}: {}",
            reply.body
        );
        assert_eq!(reply.code(), "tampered_request", "{name}");
    }
    assert_eq!(h.backend.count(), 0);
}

#[tokio::test]
async fn s7_runtime_and_metadata_mismatches_are_conflicts() {
    let h = harness(|_| {});
    let mut body = v1().body();
    body.chain.spec_version = 462;
    body.payload_json
        .insert("specVersion".into(), json!("0x000001ce"));
    assert_refused(
        &h.sign(&body).await,
        StatusCode::CONFLICT,
        "runtime_mismatch",
    );
    let mut body = v1().body();
    body.chain.metadata_blake2_256.0[0] ^= 1;
    let reply = h.sign(&body).await;
    assert_refused(&reply, StatusCode::CONFLICT, "runtime_mismatch");
    assert_eq!(
        reply.detail(),
        "client metadata differs from the server's trusted runtime"
    );
    let mut body = v1().body();
    body.chain.genesis_hash.0[0] ^= 1;
    assert_refused(
        &h.sign(&body).await,
        StatusCode::CONFLICT,
        "runtime_mismatch",
    );
    assert_eq!(h.backend.count(), 0);
}

#[tokio::test]
async fn s8_non_canonical_calls_are_undecodable() {
    let h = harness(|_| {});
    let mut call = v1().call;
    call.push(0);
    let reply = h.sign(&with_call(call).body()).await;
    assert_refused(&reply, StatusCode::BAD_REQUEST, "undecodable_call");
    let reply = h.sign(&with_call(vec![0xff, 0xff]).body()).await;
    assert_refused(&reply, StatusCode::BAD_REQUEST, "undecodable_call");
}

fn with_era(era: [u8; 2]) -> Vector {
    let mut vector = v1();
    vector.extra[..2].copy_from_slice(&era);
    vector
        .payload_json
        .insert("era".into(), json!(format!("0x{}", hex::encode(era))));
    vector.payload = [vector.call.as_slice(), &vector.extra, &vector.additional].concat();
    vector
}

#[tokio::test]
async fn s9_era_policy() {
    let h = harness(|_| {});
    let reply = h.sign(&v2().body()).await;
    assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "era_rejected");
    let allowed = harness(|cfg| cfg.policy.allow_immortal = true);
    assert_signed(&allowed.sign(&v2().body()).await, &v2().payload);
    // Period 256 (low nibble 7), phase 2.
    let long = with_era([0x27, 0x00]);
    let reply = h.sign(&long.body()).await;
    assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "era_rejected");
    let relaxed = harness(|cfg| cfg.policy.max_era_period = 256);
    assert_signed(&relaxed.sign(&long.body()).await, &long.payload);
    assert_eq!(h.backend.count(), 0);
}

#[tokio::test]
async fn s10_policy_limits_calls_and_destinations() {
    let capped =
        harness(|cfg| cfg.policy.limits.max_spend = Some(Balance::<Tao>::from_rao(12_344)));
    let reply = capped.sign(&v1().body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    let first = reply.body["violations"][0].as_str().unwrap();
    assert!(first.starts_with("spend "), "{first}");
    assert!(first.contains("exceeds max_spend_tao"), "{first}");
    assert_eq!(capped.backend.count(), 0);

    let exact = harness(|cfg| cfg.policy.limits.max_spend = Some(Balance::<Tao>::from_rao(12_345)));
    assert_signed(&exact.sign(&v1().body()).await, &v1().payload);

    let default = harness(|_| {});
    let remark = with_call(remark(b"hello"));
    let reply = default.sign(&remark.body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    assert_eq!(
        reply.body["violations"],
        json!(["raw call submission is disabled by policy (set allow_raw_calls=True)"])
    );

    let allowlisted = harness(|cfg| {
        cfg.policy.limits.allow_raw_calls = true;
        cfg.policy.allowed_calls = Some(BTreeSet::from([(
            "Balances".to_owned(),
            "transfer_keep_alive".to_owned(),
        )]));
    });
    let reply = allowlisted.sign(&remark.body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    assert_eq!(
        reply.body["violations"],
        json!(["call System.remark is not in allowed_calls"])
    );
    assert_signed(&allowlisted.sign(&v1().body()).await, &v1().payload);

    let destinations = harness(|cfg| {
        cfg.policy.allowed_destinations = Some(BTreeSet::from([charlie().account()]));
    });
    let reply = destinations.sign(&v1().body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    let to_bob = harness(|cfg| {
        cfg.policy.allowed_destinations = Some(BTreeSet::from([bob().account()]));
    });
    assert_signed(&to_bob.sign(&v1().body()).await, &v1().payload);
    let reply = to_bob.sign(&remark.body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
}

#[tokio::test]
async fn s11_shield_purposes_are_disabled_by_default() {
    let h = harness(|_| {});
    let mut body = v1().body();
    body.purpose = WirePurpose::ShieldInner;
    assert_refused(
        &h.sign(&body).await,
        StatusCode::FORBIDDEN,
        "purpose_not_allowed",
    );
    let shielded = harness(|cfg| cfg.policy.allow_shielded = true);
    assert_signed(&shielded.sign(&body).await, &v1().payload);
}

#[tokio::test]
async fn s12_body_limit_and_rate_limit() {
    let h = harness(|_| {});
    let reply = h.post(vec![b' '; 1024 * 1024 + 1]).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    let limited = harness(|cfg| {
        cfg.limiter = Some(LimiterConfig {
            burst: 1,
            refill_interval: Duration::from_secs(1),
            ..LimiterConfig::default()
        });
    });
    assert_eq!(limited.sign(&v1().body()).await.status, StatusCode::OK);
    assert_eq!(
        limited.sign(&v1().body()).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn s13_unsafe_configurations_are_refused() {
    let backend = CountingSigner::new(alice());
    let store = || -> Arc<dyn NonceStore> { Arc::new(InMemoryNonceStore::new()) };
    let build = |cfg: ServerConfig| RemoteSignerServer::new(cfg, source(), store()).map(drop);

    let mut cfg = config(&backend);
    cfg.clients[0].client = alice().account();
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    cfg.accounts.push(ServedAccount::new(Arc::new(bob())));
    assert!(build(cfg.clone()).is_err());
    cfg.server_key = Some(dave().account());
    assert!(build(cfg).is_ok());

    let mut cfg = config(&backend);
    cfg.verify.require_receiver = false;
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    cfg.verify = VerifyConfig::default();
    assert_eq!(cfg.verify.wrapped, WrappedPolicy::AcceptLikePython);
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    let short = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(12),
        ..NonceStoreConfig::default()
    })
    .unwrap();
    cfg.verify.wrapped = WrappedPolicy::Strict;
    assert!(RemoteSignerServer::new(cfg, source(), Arc::new(short)).is_err());

    let mut cfg = config(&backend);
    cfg.policy.limits.max_fee = Some(Balance::<Tao>::from_rao(1));
    assert!(build(cfg).is_err());

    // Raw calls cannot be measured against spend or fee caps, whatever
    // allowed_calls says, and no call is modeled with a subnet.
    let mut cfg = config(&backend);
    cfg.policy.limits.allow_raw_calls = true;
    assert!(build(cfg.clone()).is_ok());
    cfg.policy.limits.max_spend = Some(Balance::<Tao>::from_rao(1));
    assert!(build(cfg.clone()).is_err());
    cfg.policy.allowed_calls = Some(BTreeSet::new());
    assert!(build(cfg).is_err());
    let mut cfg = config(&backend);
    cfg.policy.limits.allowed_netuids = Some(BTreeSet::from([rtao_primitives::NetUid(1)]));
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    cfg.policy.max_era_period = 100;
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    cfg.clients[0].accounts.insert(bob().account());
    assert!(build(cfg).is_err());

    let mut cfg = config(&backend);
    cfg.accounts.clear();
    assert!(build(cfg).is_err());

    assert!(build(config(&backend)).is_ok());
}

#[tokio::test]
async fn s14_wrong_crypto_unserved_account_and_bad_version_are_refused() {
    let h = harness(|_| {});
    let mut body = v1().body();
    body.crypto = rtao_remote_signer::wire::WireCrypto::Ed25519;
    assert_refused(
        &h.sign(&body).await,
        StatusCode::FORBIDDEN,
        "account_not_authorized",
    );
    let mut body = v1().body();
    body.account = bob().account().into();
    assert_refused(
        &h.sign(&body).await,
        StatusCode::FORBIDDEN,
        "account_not_authorized",
    );
    let mut body = v1().body();
    body.version = "rtao-remote-signer/2".into();
    assert_refused(
        &h.sign(&body).await,
        StatusCode::BAD_REQUEST,
        "malformed_request",
    );
    let reply = h.post(b"{not json".to_vec()).await;
    assert_refused(&reply, StatusCode::BAD_REQUEST, "malformed_request");
    let mut value = serde_json::to_value(v1().body()).unwrap();
    value["unexpected"] = json!(1);
    let reply = h.post(serde_json::to_vec(&value).unwrap()).await;
    assert_refused(&reply, StatusCode::BAD_REQUEST, "malformed_request");
    assert_eq!(h.backend.count(), 0);
}

#[tokio::test]
async fn accounts_lists_only_granted_accounts() {
    let h = harness(|cfg| {
        cfg.server_key = Some(dave().account());
        cfg.accounts.push(ServedAccount::new(Arc::new(bob())));
        cfg.clients.push(AuthorizedClient {
            client: KeypairSigner::from_uri("//Eve", rtao_primitives::CryptoType::Sr25519)
                .unwrap()
                .account(),
            accounts: BTreeSet::from([bob().account()]),
        });
    });
    let headers = headers_for(
        &charlie(),
        &Method::GET,
        "/accounts",
        b"",
        Some(dave().account()),
        h.clock.as_ref(),
    )
    .await;
    let reply = send(&h.router, Method::GET, "/accounts", headers, Vec::new()).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(
        reply.body,
        json!({
            "version": "rtao-remote-signer/1",
            "server_key": dave().account().to_ss58(42),
            "accounts": [{"account": alice().account().to_ss58(42), "crypto": "sr25519"}],
            "genesis_hash": GENESIS_HEX,
            "purposes": ["extrinsic"],
        })
    );
}

fn pair(pallet: &str, call: &str) -> (String, String) {
    (pallet.to_owned(), call.to_owned())
}

/// Review finding 1: limits must not be bypassed by allowlisted raw calls.
#[test]
fn raw_calls_cannot_bypass_configured_spend_fee_or_subnet_limits() {
    let backend = CountingSigner::new(alice());
    let build = |cfg: ServerConfig| {
        RemoteSignerServer::new(cfg, source(), Arc::new(InMemoryNonceStore::new()))
            .map(drop)
            .map_err(|error| error.0)
    };
    // The reviewed bypass: a batch of large transfers under a 1 TAO cap.
    let mut cfg = config(&backend);
    cfg.policy.limits.allow_raw_calls = true;
    cfg.policy.limits.max_spend = Some(Balance::<Tao>::parse("1").unwrap());
    cfg.policy.allowed_calls = Some(BTreeSet::from([
        pair("Balances", "transfer_keep_alive"),
        pair("Utility", "batch_all"),
    ]));
    assert!(build(cfg).unwrap_err().contains("allow_raw_calls"));
    // add_stake under a subnet allowlist.
    let mut cfg = config(&backend);
    cfg.policy.limits.allow_raw_calls = true;
    cfg.policy.limits.allowed_netuids = Some(BTreeSet::from([rtao_primitives::NetUid(1)]));
    cfg.policy.allowed_calls = Some(BTreeSet::from([pair("SubtensorModule", "add_stake")]));
    assert!(build(cfg).unwrap_err().contains("allowed_netuids"));
    // max_fee is refused with raw calls before the online-source check.
    let mut cfg = config(&backend);
    cfg.policy.limits.allow_raw_calls = true;
    cfg.policy.limits.max_fee = Some(Balance::<Tao>::from_rao(1));
    assert!(build(cfg).unwrap_err().contains("allow_raw_calls"));

    // A caller evaluating the policy directly also fails closed.
    let mut policy = rtao_remote_signer::server::RemotePolicy::default();
    policy.limits.allow_raw_calls = true;
    policy.limits.max_spend = Some(Balance::<Tao>::parse("1").unwrap());
    let raw = rtao_remote_signer::server::classify_call(&runtime(), &remark(b"x")).unwrap();
    assert_eq!(raw.label(), "System.remark");
    let violations = policy.evaluate(&raw, None);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("cannot be measured"),
        "{violations:?}"
    );
    policy.limits.max_spend = None;
    assert!(policy.evaluate(&raw, None).is_empty());
}

/// Review finding 7: refusals never reflect client-chosen strings.
#[tokio::test]
async fn refusal_details_do_not_echo_request_content() {
    let h = harness(|_| {});
    let marker = "EVIL\u{1b}[31m";
    let mut body = v1().body();
    body.payload_json.insert(marker.into(), json!("0x00"));
    let reply = h.sign(&body).await;
    assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "tampered_request");
    let mut value = serde_json::to_value(v1().body()).unwrap();
    value[marker] = json!(1);
    let unknown_field = h.post(serde_json::to_vec(&value).unwrap()).await;
    assert_refused(&unknown_field, StatusCode::BAD_REQUEST, "malformed_request");
    let mut value = serde_json::to_value(v1().body()).unwrap();
    value["purpose"] = json!(marker);
    let unknown_variant = h.post(serde_json::to_vec(&value).unwrap()).await;
    assert_refused(
        &unknown_variant,
        StatusCode::BAD_REQUEST,
        "malformed_request",
    );
    let mut call = v1().call;
    call.extend_from_slice(b"EVIL");
    let undecodable = h.sign(&with_call(call).body()).await;
    assert_refused(&undecodable, StatusCode::BAD_REQUEST, "undecodable_call");
    for reply in [&reply, &unknown_field, &unknown_variant, &undecodable] {
        let text = reply.body.to_string();
        assert!(!text.contains("EVIL"), "{text}");
        assert!(!text.contains("\u{1b}"), "{text}");
    }
    assert_eq!(h.backend.count(), 0);
}

mod online {
    //! Review findings 4 and 5: every online check, against a scripted source.
    use super::*;
    use rtao_remote_signer::server::{RuntimeSource, SourceError};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use std::sync::Mutex;

    pub(super) struct Scripted {
        pub(super) pinned: PinnedRuntimeSource,
        pub(super) number: Result<Option<u64>, SourceError>,
        pub(super) best: u64,
        pub(super) canonical: Option<[u8; 32]>,
        pub(super) fee: Option<Balance<Tao>>,
        pub(super) calls: Mutex<Vec<&'static str>>,
    }

    impl Scripted {
        pub(super) fn anchored(number: u64, best: u64) -> Self {
            Self {
                pinned: PinnedRuntimeSource::new(runtime(), SPEC_NAME),
                number: Ok(Some(number)),
                best,
                canonical: Some(hex32(ERA_BLOCK_HEX)),
                fee: None,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn record(&self, call: &'static str) {
            self.calls.lock().unwrap().push(call);
        }
        pub(super) fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl RuntimeSource for Scripted {
        fn current(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
            self.record("current");
            self.pinned.current()
        }
        fn refresh(&self) -> BoxFuture<'_, Result<RuntimeHandle, SourceError>> {
            self.record("refresh");
            self.pinned.current()
        }
        fn spec_name(&self) -> BoxFuture<'_, Result<String, SourceError>> {
            self.pinned.spec_name()
        }
        fn is_online(&self) -> bool {
            true
        }
        fn genesis(&self) -> Option<[u8; 32]> {
            Some(genesis())
        }
        fn block_number(&self, hash: [u8; 32]) -> BoxFuture<'_, Result<Option<u64>, SourceError>> {
            self.record("block_number");
            assert_eq!(hash, hex32(ERA_BLOCK_HEX));
            let number = self.number.clone();
            Box::pin(async move { number })
        }
        fn best_number(&self) -> BoxFuture<'_, Result<u64, SourceError>> {
            self.record("best_number");
            let best = self.best;
            Box::pin(async move { Ok(best) })
        }
        fn canonical_hash(
            &self,
            _number: u64,
        ) -> BoxFuture<'_, Result<Option<[u8; 32]>, SourceError>> {
            self.record("canonical_hash");
            let canonical = self.canonical;
            Box::pin(async move { Ok(canonical) })
        }
        fn estimate_fee(
            &self,
            _call: CallBytes,
            _account: AccountId,
            _crypto: rtao_primitives::CryptoType,
        ) -> BoxFuture<'_, Result<Option<Balance<Tao>>, SourceError>> {
            self.record("estimate_fee");
            let fee = self.fee;
            Box::pin(async move { Ok(fee) })
        }
    }

    pub(super) fn serve(source: Arc<Scripted>, edit: impl FnOnce(&mut ServerConfig)) -> Harness {
        let backend = CountingSigner::new(alice());
        let mut cfg = config(&backend);
        edit(&mut cfg);
        let server = RemoteSignerServer::new(cfg, source, Arc::new(InMemoryNonceStore::new()))
            .unwrap()
            .with_clock(Arc::new(FixedClock(SERVER_NOW)));
        Harness {
            router: server.router(),
            backend,
            clock: StepClock::at(NOW),
        }
    }
}

/// V1 is mortal {period 64, phase 2}: an anchor at 66 is live while best < 130.
#[tokio::test]
async fn online_era_anchor_checks_phase_window_and_canonicality_cheapest_first() {
    use online::Scripted;
    let run = |source: Scripted| async move {
        let source = Arc::new(source);
        let h = online::serve(source.clone(), |_| {});
        let reply = h.sign(&v1().body()).await;
        (reply, source.calls(), h.backend.count())
    };
    let (reply, calls, signed) = run(Scripted::anchored(66, 66)).await;
    assert_signed(&reply, &v1().payload);
    assert_eq!(
        calls,
        ["current", "block_number", "best_number", "canonical_hash"]
    );
    assert_eq!(signed, 1);
    assert_signed(&run(Scripted::anchored(66, 129)).await.0, &v1().payload);

    // (a) unknown anchor; nothing else is looked up.
    let mut unknown = Scripted::anchored(66, 66);
    unknown.number = Ok(None);
    let (reply, calls, signed) = run(unknown).await;
    assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "era_rejected");
    assert_eq!(calls, ["current", "block_number"]);
    assert_eq!(signed, 0);
    // (b) wrong phase, (c) expired, (d) anchored above the best block: all
    // refused before the canonical-hash lookup.
    for (number, best) in [(67, 67), (66, 130), (66, 65), (2, 66)] {
        let (reply, calls, signed) = run(Scripted::anchored(number, best)).await;
        assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "era_rejected");
        assert_eq!(
            reply.detail(),
            "era block does not match the era phase or the era has expired",
            "{number}/{best}"
        );
        assert_eq!(calls, ["current", "block_number", "best_number"]);
        assert_eq!(signed, 0);
    }
    // Not canonical: another hash, or none, at that height.
    for canonical in [Some([7; 32]), None] {
        let mut fork = Scripted::anchored(66, 66);
        fork.canonical = canonical;
        let (reply, _, signed) = run(fork).await;
        assert_refused(&reply, StatusCode::UNPROCESSABLE_ENTITY, "era_rejected");
        assert_eq!(reply.detail(), "era block is not on the canonical chain");
        assert_eq!(signed, 0);
    }
    // (e) a source failure is the server's: 503, without its internal text.
    let mut failing = Scripted::anchored(66, 66);
    failing.number = Err(SourceErrorText::secret());
    let (reply, _, signed) = run(failing).await;
    assert_refused(&reply, StatusCode::SERVICE_UNAVAILABLE, "chain_unavailable");
    assert!(
        !reply.detail().contains("node-internal"),
        "{}",
        reply.detail()
    );
    assert_eq!(signed, 0);
    // Offline sources never consult anchors (V1 is signed with any anchor).
    assert_signed(&harness(|_| {}).sign(&v1().body()).await, &v1().payload);
}

struct SourceErrorText;
impl SourceErrorText {
    fn secret() -> rtao_remote_signer::server::SourceError {
        rtao_remote_signer::server::SourceError("node-internal detail".into())
    }
}

/// (f) A version mismatch refreshes an online runtime exactly once.
#[tokio::test]
async fn online_version_mismatch_refreshes_exactly_once() {
    let source = Arc::new(online::Scripted::anchored(66, 66));
    let h = online::serve(source.clone(), |_| {});
    let mut body = v1().body();
    body.chain.spec_version = 462;
    body.payload_json
        .insert("specVersion".into(), json!("0x000001ce"));
    let reply = h.sign(&body).await;
    assert_refused(&reply, StatusCode::CONFLICT, "runtime_mismatch");
    assert_eq!(source.calls(), ["current", "refresh"]);
    // A matching request does not refresh.
    let h = online::serve(Arc::new(online::Scripted::anchored(66, 66)), |_| {});
    assert_signed(&h.sign(&v1().body()).await, &v1().payload);
}

/// (g) max_fee compares the online estimate, and a missing estimate refuses.
#[tokio::test]
async fn online_max_fee_uses_the_estimate() {
    let cap = Balance::<Tao>::from_rao(124_414);
    let with_fee = |fee: Option<u64>| {
        let mut source = online::Scripted::anchored(66, 66);
        source.fee = fee.map(Balance::<Tao>::from_rao);
        let source = Arc::new(source);
        (
            online::serve(source.clone(), |cfg| cfg.policy.limits.max_fee = Some(cap)),
            source,
        )
    };
    let (h, source) = with_fee(Some(124_414));
    assert_signed(&h.sign(&v1().body()).await, &v1().payload);
    assert!(source.calls().contains(&"estimate_fee"));
    let (h, _) = with_fee(Some(124_415));
    let reply = h.sign(&v1().body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    assert!(
        reply.body["violations"][0]
            .as_str()
            .unwrap()
            .contains("exceeds max_fee_tao"),
        "{}",
        reply.body
    );
    let (h, _) = with_fee(None);
    let reply = h.sign(&v1().body()).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "policy_denied");
    assert!(
        reply.body["violations"][0]
            .as_str()
            .unwrap()
            .contains("could not be estimated"),
        "{}",
        reply.body
    );
    assert_eq!(h.backend.count(), 0);
}

/// An ed25519 served account signs with MultiSignature variant 0 and self-checks.
#[tokio::test]
async fn an_ed25519_served_account_signs_variant_zero() {
    let ed = KeypairSigner::from_uri("//Alice", rtao_primitives::CryptoType::Ed25519).unwrap();
    let account = ed.account();
    let backend = CountingSigner::new(ed);
    let mut cfg = ServerConfig::new(genesis());
    cfg.accounts = vec![ServedAccount::new(Arc::new(backend.clone()))];
    cfg.clients = vec![AuthorizedClient {
        client: charlie().account(),
        accounts: BTreeSet::from([account]),
    }];
    let server = RemoteSignerServer::new(cfg, source(), Arc::new(InMemoryNonceStore::new()))
        .unwrap()
        .with_clock(Arc::new(FixedClock(SERVER_NOW)));
    let h = Harness {
        router: server.router(),
        backend,
        clock: StepClock::at(NOW),
    };
    let mut vector = v1();
    vector
        .payload_json
        .insert("address".into(), json!(account.to_ss58(42)));
    let body = SignRequest::from_signing_request(
        &vector.signing_request(),
        account,
        rtao_primitives::CryptoType::Ed25519,
    );
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let headers = headers_for(
        &charlie(),
        &Method::POST,
        "/sign",
        &body_bytes,
        Some(account),
        h.clock.as_ref(),
    )
    .await;
    let reply = send(&h.router, Method::POST, "/sign", headers, body_bytes).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let response: SignResponse = serde_json::from_value(reply.body.clone()).unwrap();
    let signature = response
        .signature(rtao_primitives::CryptoType::Ed25519)
        .unwrap();
    assert_eq!(response.signature.0[0], 0);
    assert!(
        rtao_core::keys::verify_strict(&vector.payload, &signature.bytes, &account.to_ss58(42), 0)
            .unwrap()
    );
    // Asking for sr25519 on an ed25519 account is refused.
    let mut wrong = body.clone();
    wrong.crypto = rtao_remote_signer::wire::WireCrypto::Sr25519;
    let bytes = serde_json::to_vec(&wrong).unwrap();
    let headers = headers_for(
        &charlie(),
        &Method::POST,
        "/sign",
        &bytes,
        Some(account),
        h.clock.as_ref(),
    )
    .await;
    let reply = send(&h.router, Method::POST, "/sign", headers, bytes).await;
    assert_refused(&reply, StatusCode::FORBIDDEN, "account_not_authorized");
    assert_eq!(h.backend.count(), 1);
}
