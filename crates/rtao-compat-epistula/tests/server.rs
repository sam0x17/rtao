//! Exercise `EpistulaLayer`, `DualAuthLayer` and their extractors.

#![cfg(feature = "server")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    error::Error,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    routing::post,
};
use futures::Stream;
use http::{Method, Request, StatusCode};
use rtao_btauth::{
    BoxFuture, Caller, Clock, ClockError, InMemoryNonceStore, NonceStore, StoreError, VerifyConfig,
    server::{BtAuthLayer, LimiterConfig},
};
use rtao_compat_epistula::{
    BodyDigest, Dialect, ReplayPolicy, SignOptions, Verifier, header,
    server::{
        Admission, CallerPolicy, CallerPriority, DualAuthLayer, EpistulaLayer, PerHotkeyLimiter,
        PolicyDenied, StatusMap, VerifiedCaller, VerifiedEpistula,
    },
    sign_headers,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use serde_json::{Value, json};
use tower::ServiceExt;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const NOW_MS: u64 = 1_752_076_800_000;

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0 * 1_000_000)
    }
}

fn signer(uri: &str) -> KeypairSigner {
    KeypairSigner::from_uri(uri, CryptoType::Sr25519).unwrap()
}

fn bob() -> AccountId {
    signer("//Bob").account()
}

fn layer(dialect: Dialect, store: Arc<dyn NonceStore>) -> EpistulaLayer {
    let verifier = Arc::new(Verifier::new(bob(), dialect, store).unwrap());
    EpistulaLayer::new(verifier).with_clock(Arc::new(FixedClock(NOW_MS)))
}

async fn handler(
    VerifiedEpistula(caller): VerifiedEpistula,
    btauth: Option<axum::Extension<Caller>>,
    axum::Extension(CallerPriority(priority)): axum::Extension<CallerPriority>,
    body: Bytes,
) -> axum::Json<Value> {
    axum::Json(json!({
        "signed_by": caller.signed_by,
        "uuid": caller.uuid,
        "btauth_nonce": btauth.map(|axum::Extension(btauth)| btauth.nonce_ns.to_string()),
        "priority": priority.0.to_string(),
        "len": body.len(),
    }))
}

fn router(layer: EpistulaLayer) -> Router {
    Router::new().route("/echo", post(handler)).layer(layer)
}

async fn request(
    uri: &str,
    dialect: &Dialect,
    body: &[u8],
    signed_for: Option<&str>,
    now_ms: u64,
) -> Request<Body> {
    let headers = sign_headers(
        &signer(uri),
        dialect,
        SignOptions {
            digest: BodyDigest::Raw(body),
            signed_for,
            now_ms,
            uuid: None,
        },
    )
    .await
    .unwrap();
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/echo")
        .body(Body::from(body.to_vec()))
        .unwrap();
    headers.apply(request.headers_mut()).unwrap();
    request
}

async fn call(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn detail(value: &Value) -> &str {
    value["detail"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn accepts_verified_requests_and_rejects_replays() -> TestResult {
    let bob = bob().to_string();
    let router = router(layer(Dialect::BITMIND, Arc::new(InMemoryNonceStore::new())));
    let req = request("//Alice", &Dialect::BITMIND, b"payload", Some(&bob), NOW_MS).await;
    let replay = clone_request(&req, b"payload");
    let (status, body) = call(&router, req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["signed_by"], signer("//Alice").account().to_string());
    assert_eq!(body["len"], 7);
    // No btauth caller unless the layer opts in (Epistula binds no target).
    assert_eq!(body["btauth_nonce"], Value::Null);
    let (status, body) = call(&router, replay).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(detail(&body), "Request has already been processed");

    let router = self::router(
        layer(Dialect::BITMIND, Arc::new(InMemoryNonceStore::new())).with_btauth_caller(true),
    );
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["btauth_nonce"],
        (u128::from(NOW_MS) * 1_000_000).to_string()
    );
    Ok(())
}

fn clone_request(request: &Request<Body>, body: &[u8]) -> Request<Body> {
    let mut copy = Request::builder()
        .method(request.method())
        .uri(request.uri())
        .body(Body::from(body.to_vec()))
        .unwrap();
    *copy.headers_mut() = request.headers().clone();
    copy
}

#[tokio::test]
async fn status_maps_and_detail_strings() -> TestResult {
    let bob = bob().to_string();
    let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
    let bitmind = router(layer(Dialect::BITMIND, store.clone()));
    let btauth_codes = router(layer(Dialect::BITMIND, store).with_status_map(StatusMap::BTAUTH));

    let req = request("//Alice", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await;
    let tampered = clone_request(&req, b"y");
    let (status, body) = call(&bitmind, tampered).await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::BAD_REQUEST, "Signature Mismatch")
    );

    let req = request(
        "//Alice",
        &Dialect::BITMIND,
        b"x",
        Some(&bob),
        NOW_MS - 20_000,
    )
    .await;
    let (status, body) = call(&btauth_codes, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        detail(&body),
        "Request is too stale: 20.0s old (limit: 15.0s)"
    );

    let req = request("//Alice", &Dialect::SPEC, b"x", None, NOW_MS).await;
    let (status, body) = call(&bitmind, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        detail(&body),
        "Bad Request, message is not intended for self"
    );

    let mut req = request("//Alice", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await;
    req.headers_mut().remove(header::VERSION);
    let (status, body) = call(&bitmind, req).await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::BAD_REQUEST, "Unknown Epistula version")
    );
    Ok(())
}

struct Members(Vec<AccountId>, AtomicUsize);
impl CallerPolicy for Members {
    fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.0.contains(&caller.hotkey) {
                Ok(Admission {
                    priority: rtao_primitives::StakeWeight(7),
                })
            } else {
                Err(PolicyDenied("Signer not in metagraph".into()))
            }
        })
    }
}

/// A body that never completes, proving a rejection did not read it.
struct Pending(Arc<AtomicUsize>);
impl Stream for Pending {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Poll::Pending
    }
}

#[tokio::test]
async fn policy_runs_on_claimed_and_verified_sender() -> TestResult {
    let bob = bob().to_string();
    let members = Arc::new(Members(
        vec![signer("//Alice").account()],
        AtomicUsize::new(0),
    ));
    let policy: Arc<dyn CallerPolicy> = members.clone();
    let router = router(
        layer(Dialect::BITMIND, Arc::new(InMemoryNonceStore::new()))
            .with_policy(policy.clone())
            .with_claimed_sender_prefilter(true),
    );
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["priority"], "7");
    // The prefilter's admission is reused for the identical verified caller.
    assert_eq!(members.1.load(Ordering::SeqCst), 1);

    // A non-member is refused before the body is polled (BitMind ordering).
    let polls = Arc::new(AtomicUsize::new(0));
    let signed = request("//Eve", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await;
    let (parts, _) = signed.into_parts();
    let pending = Request::from_parts(parts, Body::from_stream(Pending(polls.clone())));
    let (status, body) = call(&router, pending).await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::UNAUTHORIZED, "Signer not in metagraph")
    );
    assert_eq!(polls.load(Ordering::SeqCst), 0);

    // Malformed headers are refused before the body too.
    let signed = request(
        "//Alice",
        &Dialect::BITMIND,
        b"x",
        Some("someone-else"),
        NOW_MS,
    )
    .await;
    let (parts, _) = signed.into_parts();
    let pending = Request::from_parts(parts, Body::from_stream(Pending(polls.clone())));
    let (status, body) = call(&router, pending).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        detail(&body),
        "Bad Request, message is not intended for self"
    );
    assert_eq!(polls.load(Ordering::SeqCst), 0);

    // Without the prefilter, a non-member is still refused after verification.
    let router = self::router(
        layer(Dialect::BITMIND, Arc::new(InMemoryNonceStore::new())).with_policy(policy),
    );
    let (status, body) = call(
        &router,
        request("//Eve", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::UNAUTHORIZED, "Signer not in metagraph")
    );
    let before = members.1.load(Ordering::SeqCst);
    let (status, _) = call(
        &router,
        request("//Alice", &Dialect::BITMIND, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members.1.load(Ordering::SeqCst), before + 1);
    Ok(())
}

#[tokio::test]
async fn stale_and_future_heads_are_refused_before_the_body() -> TestResult {
    let bob = bob().to_string();
    let router = router(layer(Dialect::RTAO, Arc::new(InMemoryNonceStore::new())));
    for (signed_at, expected) in [
        (
            NOW_MS - 15_001,
            "Request is too stale: 15.0s old (limit: 15.0s)",
        ),
        (
            NOW_MS + 2_001,
            "Request timestamp is in the future: 2.0s ahead (limit: 2.0s)",
        ),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let signed = request("//Alice", &Dialect::RTAO, b"x", Some(&bob), signed_at).await;
        let (parts, _) = signed.into_parts();
        let pending = Request::from_parts(parts, Body::from_stream(Pending(polls.clone())));
        let (status, body) = call(&router, pending).await;
        assert_eq!((status, detail(&body)), (StatusCode::BAD_REQUEST, expected));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

/// A clock the test (or a request body) can move.
#[derive(Clone)]
struct SharedClock(Arc<AtomicU64>);
impl Clock for SharedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0.load(Ordering::SeqCst) * 1_000_000)
    }
}

/// A body that moves the clock to `to` while it is being uploaded.
struct SlowUpload {
    clock: Arc<AtomicU64>,
    to: u64,
    data: Option<Bytes>,
}
impl Stream for SlowUpload {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.clock.store(self.to, Ordering::SeqCst);
        Poll::Ready(self.data.take().map(Ok))
    }
}

#[tokio::test]
async fn freshness_is_measured_at_the_head_and_the_body_has_a_deadline() -> TestResult {
    let bob = bob().to_string();
    let time = Arc::new(AtomicU64::new(NOW_MS));
    let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
    let verifier = Arc::new(Verifier::new(self::bob(), Dialect::BITMIND, store)?);
    // 60 s retention - 30 s BitMind window - 1 ms.
    let base = EpistulaLayer::new(verifier).with_clock(Arc::new(SharedClock(time.clone())));
    assert_eq!(base.body_deadline(), Duration::from_millis(29_999));
    assert!(
        base.clone()
            .with_body_deadline(Duration::from_secs(30))
            .is_err()
    );
    assert!(
        base.clone()
            .with_body_deadline(Duration::from_secs(10))
            .is_ok()
    );
    let router = router(base);
    let upload = |elapsed: u64| {
        let bob = bob.clone();
        let time = time.clone();
        async move {
            time.store(NOW_MS, Ordering::SeqCst);
            let signed = request("//Alice", &Dialect::BITMIND, b"big", Some(&bob), NOW_MS).await;
            let (parts, _) = signed.into_parts();
            Request::from_parts(
                parts,
                Body::from_stream(SlowUpload {
                    clock: time,
                    to: NOW_MS + elapsed,
                    data: Some(Bytes::from_static(b"big")),
                }),
            )
        }
    };
    // BitMind accepts a 20 s upload of a request that was fresh at its head.
    let (status, body) = call(&router, upload(20_000).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&router, upload(29_999).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&router, upload(30_000).await).await;
    assert_eq!(
        (status, detail(&body)),
        (
            StatusCode::REQUEST_TIMEOUT,
            "Request processing exceeded its deadline"
        )
    );

    // The deadline also cuts a body that never finishes.
    let router = self::router(
        layer(Dialect::RTAO, Arc::new(InMemoryNonceStore::new()))
            .with_body_deadline(Duration::from_millis(50))?,
    );
    let signed = request("//Alice", &Dialect::RTAO, b"x", Some(&bob), NOW_MS).await;
    let (parts, _) = signed.into_parts();
    let polls = Arc::new(AtomicUsize::new(0));
    let pending = Request::from_parts(parts, Body::from_stream(Pending(polls)));
    let (status, _) = call(&router, pending).await;
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
    Ok(())
}

#[tokio::test]
async fn body_limit_and_limiter() -> TestResult {
    let bob = bob().to_string();
    let router = router(
        layer(Dialect::RTAO, Arc::new(InMemoryNonceStore::new()))
            .with_body_limit(4)
            .with_limiter(Arc::new(PerHotkeyLimiter::new(LimiterConfig {
                burst: 1,
                ..LimiterConfig::default()
            })?)),
    );
    let (status, _) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"12345", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"1234", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"1", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::TOO_MANY_REQUESTS, "caller rate limit exceeded")
    );

    // A body above axum's 2 MB default but within the layer's limit reaches
    // the handler's `Bytes` extractor instead of failing with 413.
    let large = vec![b'a'; 3 * 1024 * 1024];
    let router = self::router(layer(Dialect::RTAO, Arc::new(InMemoryNonceStore::new())));
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::RTAO, &large, Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["len"], large.len());
    Ok(())
}

struct FailingStore;
impl NonceStore for FailingStore {
    fn retention(&self) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }
    fn check_and_store<'a>(
        &'a self,
        _: &'a AccountId,
        _: &'a num_bigint::BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async { Err(StoreError::Full) })
    }
}

#[tokio::test]
async fn store_failures_fail_closed_and_disabled_replay_skips_store() -> TestResult {
    let bob = bob().to_string();
    let router = router(layer(Dialect::RTAO, Arc::new(FailingStore)));
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::SERVICE_UNAVAILABLE, "nonce store is full")
    );

    let store: Arc<dyn NonceStore> = Arc::new(FailingStore);
    let verifier = Verifier::with_store(self::bob(), Dialect::SPEC, store, ReplayPolicy::Disabled)?;
    let router = self::router(
        EpistulaLayer::new(Arc::new(verifier)).with_clock(Arc::new(FixedClock(NOW_MS))),
    );
    let req = request("//Alice", &Dialect::SPEC, b"x", Some(&bob), NOW_MS).await;
    let again = clone_request(&req, b"x");
    assert_eq!(call(&router, req).await.0, StatusCode::OK);
    assert_eq!(call(&router, again).await.0, StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn dual_layer_routes_by_protocol() -> TestResult {
    let bob_account = bob();
    let bob = bob_account.to_string();
    let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
    let btauth = BtAuthLayer::new(Arc::new(rtao_btauth::Verifier::new(
        bob_account,
        VerifyConfig::default(),
        store.clone(),
    )?))
    .with_clock(Arc::new(FixedClock(NOW_MS)));
    let dual_router = |epistula: EpistulaLayer| {
        Router::new()
            .route(
                "/echo",
                post(
                    |VerifiedCaller(caller): VerifiedCaller,
                     epistula: Option<axum::Extension<rtao_compat_epistula::EpistulaCaller>>,
                     body: Bytes| async move {
                        axum::Json(json!({
                            "hotkey": caller.hotkey.to_string(),
                            "epistula": epistula.is_some(),
                            "len": body.len(),
                        }))
                    },
                ),
            )
            .layer(DualAuthLayer::new(epistula, btauth.clone()))
    };
    // By default a btauth-only handler does not accept Epistula requests.
    let strict = dual_router(layer(Dialect::RTAO, store.clone()));
    let (status, body) = call(
        &strict,
        request("//Alice", &Dialect::RTAO, b"abc", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::UNAUTHORIZED, "missing authenticated caller")
    );
    // Opting in admits them, and the handler can still tell them apart.
    let router = dual_router(layer(Dialect::RTAO, store).with_btauth_caller(true));
    let (status, body) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"abc", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["len"], 3);
    assert_eq!(body["epistula"], true);

    let alice = signer("//Alice");
    let headers = rtao_btauth::sign(
        &alice,
        &Method::POST,
        "/echo",
        b"abcd",
        Some(&bob_account),
        &FixedClock(NOW_MS),
    )
    .await?;
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/echo")
        .body(Body::from(&b"abcd"[..]))?;
    *req.headers_mut() = headers;
    let (status, body) = call(&router, req).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["hotkey"], alice.account().to_string());
    assert_eq!(body["epistula"], false);

    // Neither protocol: btauth answers.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/echo")
        .body(Body::empty())?;
    let (status, _) = call(&router, req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[cfg(feature = "secret-filter")]
#[tokio::test]
async fn secret_filter_uses_the_claimed_interval_and_exact_signatures() -> TestResult {
    use rtao_compat_epistula::{EpistulaError, SecretPolicy, secret::SecretSigFilter};

    let bob = bob().to_string();
    let fresh = || SecretSigFilter::new(self::bob(), SecretPolicy::PYTHON);
    let signed = |now: u64| {
        let bob = bob.clone();
        async move { request("//Alice", &Dialect::RTAO, b"x", Some(&bob), now).await }
    };
    // Same interval passes.
    let current = signed(1_700_000_005_000).await;
    assert!(fresh().check(current.headers(), 1_700_000_005_500).is_ok());
    // A sender 2 ms ahead across an interval boundary passes.
    let ahead = signed(1_700_000_010_001).await;
    assert!(fresh().check(ahead.headers(), 1_700_000_009_999).is_ok());
    // A request more than one interval old, still within 15 s, passes.
    let old = signed(1_700_000_010_000).await;
    assert!(fresh().check(old.headers(), 1_700_000_020_001).is_ok());
    assert!(fresh().check(old.headers(), 1_700_000_025_000).is_ok());
    // Outside the freshness window it fails, as the verifier would.
    assert_eq!(
        fresh().check(old.headers(), 1_700_000_025_001).err(),
        Some(EpistulaError::Stale(
            "Request is too stale: 15.0s old (limit: 15.0s)".into()
        ))
    );
    let rtao = SecretSigFilter::for_dialect(self::bob(), &Dialect::RTAO);
    assert!(matches!(
        rtao.check(ahead.headers(), 1_700_000_007_999),
        Err(EpistulaError::Future(_))
    ));
    assert!(rtao.check(ahead.headers(), 1_700_000_008_001).is_ok());
    // A secret signature for another interval fails.
    let mut moved = signed(1_700_000_005_000).await;
    moved.headers_mut().insert(
        header::TIMESTAMP,
        http::HeaderValue::from_static("1700000015000"),
    );
    assert_eq!(
        fresh().check(moved.headers(), 1_700_000_015_000).err(),
        Some(EpistulaError::SecretSignatureMismatch)
    );

    // The cache holds exact signatures, never a bare sender name.
    let filter = fresh();
    assert!(filter.check(current.headers(), 1_700_000_005_000).is_ok());
    assert!(filter.check(current.headers(), 1_700_000_005_000).is_ok());
    let mut missing = signed(1_700_000_005_000).await;
    missing.headers_mut().remove(header::SECRET_SIGNATURE[1]);
    assert_eq!(
        filter.check(missing.headers(), 1_700_000_005_000).err(),
        Some(EpistulaError::SecretSignatureMismatch)
    );
    let mut wrong = signed(1_700_000_005_000).await;
    let other = wrong.headers()[header::SECRET_SIGNATURE[0]].clone();
    wrong
        .headers_mut()
        .insert(header::SECRET_SIGNATURE[1], other);
    assert_eq!(
        filter.check(wrong.headers(), 1_700_000_005_000).err(),
        Some(EpistulaError::SecretSignatureMismatch)
    );
    // A cached signature replayed with a timestamp in another interval fails.
    let mut shifted = current;
    shifted.headers_mut().insert(
        header::TIMESTAMP,
        http::HeaderValue::from_static("1700000012000"),
    );
    assert_eq!(
        filter.check(shifted.headers(), 1_700_000_012_000).err(),
        Some(EpistulaError::SecretSignatureMismatch)
    );

    // gobt: interval index and sender subject.
    let gobt = SecretSigFilter::for_dialect(self::bob(), &Dialect::GOBT);
    let req = request(
        "//Alice",
        &Dialect::GOBT,
        b"x",
        Some(&bob),
        1_700_000_010_001,
    )
    .await;
    assert!(gobt.check(req.headers(), 1_700_000_009_999).is_ok());

    // The server layer applies it before the body.
    let router = router(
        layer(Dialect::RTAO, Arc::new(InMemoryNonceStore::new()))
            .with_clock(Arc::new(FixedClock(NOW_MS)))
            .with_secret_filter(Arc::new(SecretSigFilter::for_dialect(
                self::bob(),
                &Dialect::RTAO,
            ))),
    );
    let mut req = request("//Alice", &Dialect::RTAO, b"x", Some(&bob), NOW_MS).await;
    req.headers_mut().remove(header::SECRET_SIGNATURE[1]);
    let (status, body) = call(&router, req).await;
    assert_eq!(
        (status, detail(&body)),
        (StatusCode::BAD_REQUEST, "Secret Signature Mismatch")
    );
    let (status, _) = call(
        &router,
        request("//Alice", &Dialect::RTAO, b"x", Some(&bob), NOW_MS).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}
