//! Exercise the production Tower authentication path and its axum extractors.

#![cfg(feature = "server")]

use std::{
    error::Error,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::DefaultBodyLimit,
    routing::post,
};
use futures::Stream;
#[cfg(feature = "legacy-v10")]
use http::HeaderMap;
use http::{Method, Request, StatusCode, header};
use num_bigint::BigInt;
#[cfg(feature = "legacy-v10")]
use rtao_btauth::AuthError;
use rtao_btauth::{
    BoxFuture, Caller, Clock, ClockError, InMemoryNonceStore, NonceStore, NonceStoreConfig,
    StoreError, Verifier, VerifyConfig,
    server::{
        Admission, BtAuthLayer, CallerPolicy, CallerPriority, Json, LimiterConfig,
        PerHotkeyLimiter, PolicyDenied, VerifiedCaller,
    },
    sign,
};
use rtao_primitives::{AccountId, CryptoType, StakeWeight};
use rtao_signer::{KeypairSigner, Signer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower::{Layer, ServiceExt};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const NOW: u64 = 1_752_076_800_000_000_000;

struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0)
    }
}

struct Fixture {
    alice: KeypairSigner,
    bob: AccountId,
    verifier: Arc<Verifier<dyn NonceStore>>,
}

impl Fixture {
    fn new(store: Arc<dyn NonceStore>) -> TestResult<Self> {
        let alice = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
        let bob = KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?.account();
        let verifier = Arc::new(Verifier::new(bob, VerifyConfig::default(), store)?);
        Ok(Self {
            alice,
            bob,
            verifier,
        })
    }

    fn layer(&self) -> BtAuthLayer {
        BtAuthLayer::new(self.verifier.clone()).with_clock(Arc::new(FixedClock(NOW)))
    }

    async fn request(&self, body: Vec<u8>, target: &str, nonce: u64) -> TestResult<Request<Body>> {
        let headers = sign(
            &self.alice,
            &Method::POST,
            target,
            &body,
            Some(&self.bob),
            &FixedClock(nonce),
        )
        .await?;
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(target)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))?;
        request.headers_mut().extend(headers);
        Ok(request)
    }
}

async fn response_json(response: axum::response::Response) -> TestResult<Value> {
    Ok(serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 5 * 1024 * 1024).await?,
    )?)
}

struct Policy {
    allow: AtomicBool,
    visits: AtomicUsize,
}

impl Policy {
    fn new(allow: bool) -> Self {
        Self {
            allow: AtomicBool::new(allow),
            visits: AtomicUsize::new(0),
        }
    }
}

impl CallerPolicy for Policy {
    fn admit<'a>(&'a self, _: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        self.visits.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if self.allow.load(Ordering::SeqCst) {
                Ok(Admission {
                    priority: StakeWeight(123),
                })
            } else {
                Err(PolicyDenied("caller denied".into()))
            }
        })
    }
}

#[tokio::test]
async fn authentication_policy_commit_limit_order_protects_capacity_and_tokens() -> TestResult {
    let store = Arc::new(InMemoryNonceStore::new());
    let fixture = Fixture::new(store.clone())?;
    let policy = Arc::new(Policy::new(false));
    let limiter = Arc::new(PerHotkeyLimiter::new(LimiterConfig {
        burst: 1,
        refill_interval: Duration::from_secs(3600),
        max_hotkeys: 1,
    })?);
    let layer = fixture
        .layer()
        .with_policy(policy.clone())
        .with_limiter(limiter.clone());
    let inner = tower::service_fn(|req: Request<Body>| async move {
        assert!(req.extensions().get::<Caller>().is_some());
        assert_eq!(
            req.extensions().get::<CallerPriority>(),
            Some(&CallerPriority(StakeWeight(123)))
        );
        Ok::<_, std::convert::Infallible>(axum::response::IntoResponse::into_response(
            StatusCode::OK,
        ))
    });
    let service = layer.layer(inner);

    let mut malformed = fixture.request(b"{}".to_vec(), "/", NOW).await?;
    malformed
        .headers_mut()
        .remove(rtao_btauth::HEADER_SIGNATURE);
    assert_eq!(
        service.clone().oneshot(malformed).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(policy.visits.load(Ordering::SeqCst), 0);

    let denied = service
        .clone()
        .oneshot(fixture.request(b"{}".to_vec(), "/", NOW).await?)
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json(denied).await?,
        json!({"detail":"caller denied"})
    );
    assert!(
        store
            .check_and_store(&fixture.alice.account(), &BigInt::from(NOW))
            .await?
    );

    // Use a fresh nonce after the independent assertion above committed NOW.
    policy.allow.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .clone()
            .oneshot(fixture.request(b"{}".to_vec(), "/", NOW + 1).await?)
            .await?
            .status(),
        StatusCode::OK
    );
    let replay = service
        .clone()
        .oneshot(fixture.request(b"{}".to_vec(), "/", NOW + 1).await?)
        .await?;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response_json(replay).await?,
        json!({"detail":"this nonce was already accepted from this hotkey"})
    );
    let limited = service
        .clone()
        .oneshot(fixture.request(b"{}".to_vec(), "/", NOW + 2).await?)
        .await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        limited
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("3600")
    );
    assert!(
        !store
            .check_and_store(&fixture.alice.account(), &BigInt::from(NOW + 2))
            .await?
    );
    assert_eq!(
        service
            .clone()
            .oneshot(fixture.request(b"{}".to_vec(), "/", NOW + 2).await?)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // Policy is deliberately before replay, including denied replays (D-032).
    policy.allow.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .oneshot(fixture.request(b"{}".to_vec(), "/", NOW + 1).await?)
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    Ok(())
}

#[tokio::test]
async fn replay_does_not_spend_limiter_tokens() -> TestResult {
    let fixture = Fixture::new(Arc::new(InMemoryNonceStore::new()))?;
    let layer = fixture
        .layer()
        .with_limiter(Arc::new(PerHotkeyLimiter::new(LimiterConfig {
            burst: 2,
            refill_interval: Duration::from_secs(3600),
            max_hotkeys: 1,
        })?));
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(layer);
    for (nonce, expected) in [
        (NOW, StatusCode::OK),
        (NOW, StatusCode::UNAUTHORIZED),
        (NOW + 1, StatusCode::OK),
        (NOW + 2, StatusCode::TOO_MANY_REQUESTS),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(fixture.request(b"{}".to_vec(), "/", nonce).await?)
                .await?
                .status(),
            expected
        );
    }
    Ok(())
}

#[tokio::test]
async fn store_bound_is_503_and_never_reopens_accepted_nonce() -> TestResult {
    let store = InMemoryNonceStore::with_config(NonceStoreConfig {
        max_entries: 1,
        max_per_hotkey: 1,
        ..NonceStoreConfig::default()
    })?;
    let fixture = Fixture::new(Arc::new(store))?;
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(fixture.layer());
    for (nonce, expected) in [
        (NOW, StatusCode::OK),
        (NOW + 1, StatusCode::SERVICE_UNAVAILABLE),
        (NOW, StatusCode::UNAUTHORIZED),
    ] {
        let response = router
            .clone()
            .oneshot(fixture.request(b"{}".to_vec(), "/", nonce).await?)
            .await?;
        assert_eq!(response.status(), expected);
        if expected == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(
                response_json(response).await?,
                json!({"detail":"nonce store is full"})
            );
        }
    }
    Ok(())
}

struct UnavailableStore;
impl NonceStore for UnavailableStore {
    fn retention(&self) -> Duration {
        Duration::from_secs(60)
    }
    fn check_and_store<'a>(
        &'a self,
        _: &'a AccountId,
        _: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable(
                "backend credentials would be here".into(),
            ))
        })
    }
}

#[tokio::test]
async fn unavailable_store_fails_closed_without_exposing_backend_detail() -> TestResult {
    let fixture = Fixture::new(Arc::new(UnavailableStore))?;
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(fixture.layer());
    let response = router
        .oneshot(fixture.request(b"{}".to_vec(), "/", NOW).await?)
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(response).await?,
        json!({"detail":"nonce store is unavailable"})
    );
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct Prompt {
    prompt: String,
}

#[tokio::test]
async fn three_mib_signed_body_reaches_json_handler_with_matching_router_limit() -> TestResult {
    let fixture = Fixture::new(Arc::new(InMemoryNonceStore::new()))?;
    let layer = fixture.layer();
    let router = Router::new()
        .route(
            "/generate",
            post(
                |VerifiedCaller(caller): VerifiedCaller, Json(prompt): Json<Prompt>| async move {
                    Json(json!({"bytes":prompt.prompt.len(), "nonce":caller.nonce_ns.to_string()}))
                },
            ),
        )
        .layer(DefaultBodyLimit::max(layer.body_limit()))
        .layer(layer);
    let body = serde_json::to_vec(&Prompt {
        prompt: "x".repeat(3 * 1024 * 1024),
    })?;
    let response = router
        .oneshot(fixture.request(body, "/generate", NOW).await?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await?,
        json!({"bytes":3 * 1024 * 1024,"nonce":NOW.to_string()})
    );
    Ok(())
}

struct CountedStream {
    chunks: Vec<Bytes>,
    polls: Arc<AtomicUsize>,
}
impl Stream for CountedStream {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(self.chunks.pop().map(Ok))
    }
}

#[tokio::test]
async fn body_limit_is_applied_before_buffering_or_policy_and_stops_stream() -> TestResult {
    let fixture = Fixture::new(Arc::new(InMemoryNonceStore::new()))?;
    let policy = Arc::new(Policy::new(true));
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(
            fixture
                .layer()
                .with_body_limit(4)
                .with_policy(policy.clone()),
        );
    let polls = Arc::new(AtomicUsize::new(0));
    let stream = || {
        Body::from_stream(CountedStream {
            chunks: vec![Bytes::from_static(b"tail"), Bytes::from_static(b"12345")],
            polls: polls.clone(),
        })
    };
    let declared = Request::builder()
        .method(Method::POST)
        .uri("/")
        .header(header::CONTENT_LENGTH, "5")
        .body(stream())?;
    assert_eq!(
        router.clone().oneshot(declared).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let unknown_length = Request::builder()
        .method(Method::POST)
        .uri("/")
        .body(stream())?;
    assert_eq!(
        router.oneshot(unknown_length).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    assert_eq!(policy.visits.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn raw_body_and_percent_encoded_target_survive_without_normalization() -> TestResult {
    let fixture = Fixture::new(Arc::new(InMemoryNonceStore::new()))?;
    let layer = fixture.layer();
    let service = layer.layer(tower::service_fn(|req: Request<Body>| async move {
        let target = req
            .uri()
            .path_and_query()
            .map_or("", |target| target.as_str())
            .to_owned();
        let body = axum::body::to_bytes(req.into_body(), 1024).await;
        let result = match body {
            Ok(body) => Json(json!({"target":target,"body":String::from_utf8_lossy(&body)})),
            Err(_) => Json(Value::Null),
        };
        Ok::<_, std::convert::Infallible>(axum::response::IntoResponse::into_response(result))
    }));
    for (index, target) in ["/some%2Fpath?q=a%20b&x=%2f", "/some%2Fpath?"]
        .iter()
        .enumerate()
    {
        let body = b"{ \"b\":2, \"a\": 1 }\n";
        let response = service
            .clone()
            .oneshot(
                fixture
                    .request(body.to_vec(), target, NOW + u64::try_from(index)?)
                    .await?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await?,
            json!({"target":target,"body":std::str::from_utf8(body)?})
        );
    }
    Ok(())
}

#[tokio::test]
async fn nested_router_authenticates_original_wire_target() -> TestResult {
    let fixture = Fixture::new(Arc::new(InMemoryNonceStore::new()))?;
    let inner = Router::new()
        .route("/generate", post(|| async { StatusCode::OK }))
        .layer(fixture.layer());
    let router = Router::new().nest("/api", inner);
    for (index, target) in ["/api/generate?q=a%20b&x=%2f", "/api/generate?"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            router
                .clone()
                .oneshot(
                    fixture
                        .request(b"{}".to_vec(), target, NOW + u64::try_from(index)?)
                        .await?
                )
                .await?
                .status(),
            StatusCode::OK
        );
    }
    let mut stripped = fixture
        .request(b"{}".to_vec(), "/generate?q=a%20b&x=%2f", NOW + 2)
        .await?;
    *stripped.uri_mut() = "/api/generate?q=a%20b&x=%2f".parse()?;
    assert_eq!(
        router.oneshot(stripped).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

struct MutableClock(AtomicU64);
impl Clock for MutableClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct DelayedPolicy {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl CallerPolicy for DelayedPolicy {
    fn admit<'a>(&'a self, _: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.resume.notified().await;
            Ok(Admission::default())
        })
    }
}

#[tokio::test]
async fn policy_wait_cannot_admit_request_after_freshness_window() -> TestResult {
    let store = Arc::new(InMemoryNonceStore::new());
    let fixture = Fixture::new(store.clone())?;
    let clock = Arc::new(MutableClock(AtomicU64::new(NOW)));
    let policy = Arc::new(DelayedPolicy {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(
            fixture
                .layer()
                .with_clock(clock.clone())
                .with_policy(policy.clone()),
        );
    let request = fixture.request(b"{}".to_vec(), "/", NOW).await?;
    let pending = tokio::spawn(router.oneshot(request));
    policy.entered.notified().await;
    clock.0.store(NOW + 61_000_000_000, Ordering::SeqCst);
    policy.resume.notify_one();
    let response = pending.await??;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response_json(response).await?,
        json!({"detail":"nonce is older than the 10s freshness window"})
    );
    assert!(
        store
            .check_and_store(&fixture.alice.account(), &BigInt::from(NOW))
            .await?
    );
    Ok(())
}

struct SlowStore {
    clock: Arc<MutableClock>,
    committed: AtomicBool,
}

impl NonceStore for SlowStore {
    fn retention(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn check_and_store<'a>(
        &'a self,
        _: &'a AccountId,
        _: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            self.clock.0.store(NOW + 61_000_000_000, Ordering::SeqCst);
            Ok(!self.committed.swap(true, Ordering::SeqCst))
        })
    }
}

#[tokio::test]
async fn successful_async_commit_cannot_admit_an_expired_request() -> TestResult {
    let clock = Arc::new(MutableClock(AtomicU64::new(NOW)));
    let store = Arc::new(SlowStore {
        clock: clock.clone(),
        committed: AtomicBool::new(false),
    });
    let fixture = Fixture::new(store.clone())?;
    let router = Router::new()
        .route("/", post(|| async { StatusCode::OK }))
        .layer(fixture.layer().with_clock(clock));
    let response = router
        .oneshot(fixture.request(b"{}".to_vec(), "/", NOW).await?)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(store.committed.load(Ordering::SeqCst));
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FloatOrText {
    Float(f64),
    Text(String),
}

#[derive(Debug, Deserialize)]
struct Flattened {
    #[serde(flatten)]
    values: std::collections::BTreeMap<String, f64>,
}

#[test]
fn value_first_json_accepts_untagged_and_flattened_float_tokens() -> TestResult {
    assert!(axum::Json::<FloatOrText>::from_bytes(b"1e-05").is_err());
    match Json::<FloatOrText>::from_bytes(b"1e-05") {
        Ok(Json(FloatOrText::Float(value))) => assert_eq!(value, 0.00001),
        value => return Err(format!("unexpected numeric result: {value:?}").into()),
    }
    match Json::<FloatOrText>::from_bytes(b"\"text\"") {
        Ok(Json(FloatOrText::Text(value))) => assert_eq!(value, "text"),
        value => return Err(format!("unexpected text result: {value:?}").into()),
    }
    assert!(axum::Json::<Flattened>::from_bytes(br#"{"x":1e-05}"#).is_err());
    assert_eq!(
        Json::<Flattened>::from_bytes(br#"{"x":1e-05}"#)?.values["x"],
        0.00001
    );
    assert_eq!(
        Json::<u128>::from_bytes(b"340282366920938463463374607431768211455")?.0,
        u128::MAX
    );
    Ok(())
}

#[tokio::test]
async fn json_rejection_statuses_and_text_match_axum() -> TestResult {
    let router = Router::new().route("/", post(|Json(_): Json<Prompt>| async { StatusCode::OK }));
    for (body, content_type, status, prefix) in [
        (
            "{}",
            None,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Expected request with `Content-Type: application/json`",
        ),
        (
            "{",
            Some("application/json"),
            StatusCode::BAD_REQUEST,
            "Failed to parse the request body as JSON",
        ),
        (
            "{}",
            Some("application/json"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "Failed to deserialize the JSON body into the target type",
        ),
    ] {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .body(Body::from(body))?;
        if let Some(content_type) = content_type {
            request
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type.parse()?);
        }
        let response = router.clone().oneshot(request).await?;
        assert_eq!(response.status(), status);
        assert!(
            std::str::from_utf8(&axum::body::to_bytes(response.into_body(), 4096).await?)?
                .starts_with(prefix)
        );
    }
    for content_type in [
        "application/json; charset=utf-8",
        "application/vnd.test+json",
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(r#"{"prompt":"hello"}"#))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    Ok(())
}

#[tokio::test]
async fn verified_caller_extractor_requires_authenticated_extension() -> TestResult {
    let router = Router::new().route(
        "/",
        post(|VerifiedCaller(_): VerifiedCaller| async { StatusCode::OK }),
    );
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[cfg(feature = "legacy-v10")]
#[tokio::test]
async fn legacy_v10_preserves_documented_signature_recipe() -> TestResult {
    let signer = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let receiver = KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?.account();
    let sender = signer.account().to_string();
    let body_hash = "sha3-schema-hash";
    let message = format!("nonce-text.{sender}.{receiver}.uuid.{body_hash}");
    let signature = signer.sign_raw(message.as_bytes()).await?;
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("bt_header_dendrite_hotkey", sender),
        ("bt_header_dendrite_nonce", "nonce-text".into()),
        ("bt_header_axon_hotkey", receiver.to_string()),
        ("bt_header_dendrite_uuid", "uuid".into()),
        (
            "bt_header_dendrite_signature",
            format!("0x{}", hex::encode(signature)),
        ),
    ] {
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes())?,
            value.parse()?,
        );
    }
    assert_eq!(
        rtao_btauth::legacy::verify_v10(&headers, body_hash)?,
        signer.account()
    );
    assert!(matches!(
        rtao_btauth::legacy::verify_v10(&headers, "different"),
        Err(AuthError::BadSignature(_))
    ));
    headers.remove("bt_header_dendrite_uuid");
    assert!(matches!(
        rtao_btauth::legacy::verify_v10(&headers, body_hash),
        Err(AuthError::MalformedAuth(_))
    ));
    Ok(())
}
