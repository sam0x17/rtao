//! Tower authentication and admission for buffered signed HTTP requests.
//!
//! Hashing requires the complete raw request body. Set the same limit with
//! axum's `DefaultBodyLimit` on the inner router; responses may still stream.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::{Body, Bytes},
    extract::{FromRequest, FromRequestParts, Request, rejection::BytesRejection},
    response::{IntoResponse, Response},
};
use http::{StatusCode, header, request::Parts};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use rtao_primitives::{AccountId, StakeWeight};
use serde::{Serialize, de::DeserializeOwned};
use tower::{Layer, Service};

use crate::{AuthError, BoxFuture, Caller, Clock, ClockError, NonceStore, StoreError, Verifier};

/// Default maximum signed raw body size, in bytes.
pub const DEFAULT_BODY_LIMIT: usize = 4 * 1024 * 1024;

/// Policy-approved request priority, expressed as stake weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// Priority for optional application scheduling.
    pub priority: StakeWeight,
}

/// A verified caller failed the application's admission policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyDenied(pub String);

impl fmt::Display for PolicyDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for PolicyDenied {}

/// Admission evaluated after cryptography and before replay commitment.
pub trait CallerPolicy: Send + Sync {
    /// Allow a verified identity or return the detail sent with HTTP 403.
    fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>>;
}

/// Admission based on one current metagraph snapshot.
#[cfg(feature = "metagraph-policy")]
#[derive(Clone, Debug)]
pub struct MetagraphPolicy {
    /// Lock-free access to the last-good metagraph.
    pub metagraph: rtao_metagraph::MetagraphHandle,
    /// Require membership in this subnet.
    pub require_registered: bool,
    /// Require a current validator permit.
    pub require_validator_permit: bool,
    /// Minimum alpha-equivalent stake weight, when configured.
    pub min_stake: Option<StakeWeight>,
}

#[cfg(feature = "metagraph-policy")]
impl MetagraphPolicy {
    /// Require registered validators; no additional stake floor by default.
    pub fn validators(metagraph: rtao_metagraph::MetagraphHandle) -> Self {
        Self {
            metagraph,
            require_registered: true,
            require_validator_permit: true,
            min_stake: None,
        }
    }
}

#[cfg(feature = "metagraph-policy")]
impl CallerPolicy for MetagraphPolicy {
    fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        Box::pin(async move {
            metagraph_admission(
                &self.metagraph.load(),
                caller,
                self.require_registered,
                self.require_validator_permit,
                self.min_stake,
            )
        })
    }
}

#[cfg(feature = "metagraph-policy")]
fn metagraph_admission(
    graph: &rtao_metagraph::Metagraph,
    caller: &Caller,
    require_registered: bool,
    require_validator_permit: bool,
    min_stake: Option<StakeWeight>,
) -> Result<Admission, PolicyDenied> {
    let Some(neuron) = graph.by_hotkey(&caller.hotkey) else {
        if require_registered || require_validator_permit || min_stake.is_some() {
            return Err(PolicyDenied(
                "caller is not registered on this subnet".into(),
            ));
        }
        return Ok(Admission::default());
    };
    if require_validator_permit && !neuron.validator_permit() {
        return Err(PolicyDenied(
            "caller does not have a validator permit".into(),
        ));
    }
    let stake = neuron
        .total_stake()
        .map_err(|_| PolicyDenied("caller stake is unavailable".into()))?;
    if min_stake.is_some_and(|minimum| stake < minimum) {
        return Err(PolicyDenied(
            "caller stake is below the required minimum".into(),
        ));
    }
    Ok(Admission { priority: stake })
}

/// Bounded token bucket configuration; only admitted identities reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimiterConfig {
    /// Maximum immediate requests per hotkey.
    pub burst: u32,
    /// Time needed to replenish one request token.
    pub refill_interval: Duration,
    /// Maximum hotkeys whose outstanding token debt is remembered.
    pub max_hotkeys: usize,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            burst: 64,
            refill_interval: Duration::from_secs(1),
            max_hotkeys: 4096,
        }
    }
}

/// Invalid zero-valued limiter capacity or refill interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimiterConfigError;
impl fmt::Display for LimiterConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("limiter burst, refill interval and hotkey capacity must be positive")
    }
}
impl std::error::Error for LimiterConfigError {}

/// A token bucket rejected a request, or cannot retain its state safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitError {
    /// The hotkey has exhausted its tokens.
    RateLimited {
        /// Time until one token becomes available.
        retry_after: Duration,
    },
    /// All bounded slots have outstanding debt; no live state is evicted.
    Full,
    /// A poisoned state lock prevents a safe decision.
    Unavailable,
}

#[derive(Debug)]
struct Bucket {
    tokens: u32,
    at: Instant,
}

/// Per-hotkey token buckets with bounded storage and monotonic refill.
#[derive(Debug)]
pub struct PerHotkeyLimiter {
    config: LimiterConfig,
    buckets: Mutex<HashMap<AccountId, Bucket>>,
}

impl Default for PerHotkeyLimiter {
    fn default() -> Self {
        Self {
            config: LimiterConfig::default(),
            buckets: Mutex::new(HashMap::new()),
        }
    }
}

impl PerHotkeyLimiter {
    /// Construct a limiter with positive quotas and bounded hotkey storage.
    pub fn new(config: LimiterConfig) -> Result<Self, LimiterConfigError> {
        if config.burst == 0 || config.refill_interval.is_zero() || config.max_hotkeys == 0 {
            return Err(LimiterConfigError);
        }
        Ok(Self {
            config,
            buckets: Mutex::new(HashMap::new()),
        })
    }

    /// Consume one token for a replay-committed identity.
    pub fn check(&self, hotkey: &AccountId) -> Result<(), LimitError> {
        self.check_at(hotkey, Instant::now())
    }

    fn refill(&self, bucket: &mut Bucket, now: Instant) {
        let elapsed = now.saturating_duration_since(bucket.at);
        let replenished = elapsed.as_nanos() / self.config.refill_interval.as_nanos();
        let deficit = self.config.burst.saturating_sub(bucket.tokens);
        if replenished >= u128::from(deficit) {
            bucket.tokens = self.config.burst;
            bucket.at = now;
        } else if let Ok(replenished) = u32::try_from(replenished) {
            bucket.tokens += replenished;
            if let Some(advance) = self.config.refill_interval.checked_mul(replenished)
                && let Some(at) = bucket.at.checked_add(advance)
            {
                bucket.at = at;
            }
        }
    }

    fn check_at(&self, hotkey: &AccountId, now: Instant) -> Result<(), LimitError> {
        let mut buckets = self.buckets.lock().map_err(|_| LimitError::Unavailable)?;
        if !buckets.contains_key(hotkey) && buckets.len() >= self.config.max_hotkeys {
            // A fully replenished bucket is equivalent to absent state.
            // Outstanding debt is never evicted to make space for another key.
            buckets.retain(|_, bucket| {
                self.refill(bucket, now);
                bucket.tokens != self.config.burst
            });
            if buckets.len() >= self.config.max_hotkeys {
                return Err(LimitError::Full);
            }
        }
        let bucket = buckets.entry(*hotkey).or_insert(Bucket {
            tokens: self.config.burst,
            at: now,
        });
        self.refill(bucket, now);
        if bucket.tokens == 0 {
            return Err(LimitError::RateLimited {
                retry_after: self
                    .config
                    .refill_interval
                    .saturating_sub(now.saturating_duration_since(bucket.at)),
            });
        }
        bucket.tokens -= 1;
        Ok(())
    }
}

/// The verified caller supplied by an outer [`BtAuthLayer`].
#[derive(Clone, Debug)]
pub struct VerifiedCaller(pub Caller);

impl<S: Send + Sync> FromRequestParts<S> for VerifiedCaller {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Caller>()
            .cloned()
            .map(Self)
            .ok_or_else(|| detail(StatusCode::UNAUTHORIZED, "missing authenticated caller"))
    }
}

/// Policy priority inserted alongside the caller after a successful check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallerPriority(pub StakeWeight);

/// Authentication middleware configuration shared by all service clones.
#[derive(Clone)]
pub struct BtAuthLayer {
    verifier: Arc<Verifier<dyn NonceStore>>,
    policy: Option<Arc<dyn CallerPolicy>>,
    limiter: Option<Arc<PerHotkeyLimiter>>,
    body_limit: usize,
    clock: Arc<dyn Clock>,
}

#[derive(Default)]
struct VerificationClock {
    anchor: Mutex<Option<(u64, Instant)>>,
}

impl VerificationClock {
    fn sample(&self, wall_ns: u64, monotonic: Instant) -> Result<u64, ClockError> {
        let mut anchor = self.anchor.lock().map_err(|_| ClockError::NonceOverflow)?;
        let monotonic = anchor
            .as_ref()
            .map_or(monotonic, |(_, at)| monotonic.max(*at));
        let projected = match *anchor {
            Some((unix_ns, at)) => {
                let elapsed = u64::try_from(monotonic.saturating_duration_since(at).as_nanos())
                    .map_err(|_| ClockError::NonceOverflow)?;
                unix_ns
                    .checked_add(elapsed)
                    .ok_or(ClockError::NonceOverflow)?
            }
            None => wall_ns,
        };
        let verified_ns = wall_ns.max(projected);
        *anchor = Some((verified_ns, monotonic));
        Ok(verified_ns)
    }
}

impl Clock for VerificationClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ClockError::BeforeUnixEpoch)?;
        let wall_ns = u64::try_from(elapsed.as_nanos()).map_err(|_| ClockError::NonceOverflow)?;
        self.sample(wall_ns, Instant::now())
    }
}

impl BtAuthLayer {
    /// Authenticate with the verifier's replay store and a 4 MiB body cap.
    pub fn new(verifier: Arc<Verifier<dyn NonceStore>>) -> Self {
        Self {
            verifier,
            policy: None,
            limiter: None,
            body_limit: DEFAULT_BODY_LIMIT,
            clock: Arc::new(VerificationClock::default()),
        }
    }

    /// Evaluate this caller policy before storing any authenticated nonce.
    pub fn with_policy(mut self, policy: Arc<dyn CallerPolicy>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Apply bounded per-hotkey limiting after replay commitment.
    pub fn with_limiter(mut self, limiter: Arc<PerHotkeyLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// Set the raw-body cap; configure the inner router to match this value.
    pub fn with_body_limit(mut self, body_limit: usize) -> Self {
        self.body_limit = body_limit;
        self
    }

    /// Supply the verification clock, usually for deterministic tests.
    ///
    /// Its time must advance at least as fast as nonce-store expiry. The
    /// default uses a monotonic floor to resist backwards wall-clock changes.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Raw signed-body cap, also used for the router's `DefaultBodyLimit`.
    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    async fn authenticate(&self, req: Request) -> Result<Request, Box<Response>> {
        let (mut parts, body) = req.into_parts();
        // A declared oversize body can be refused without polling its stream.
        if parts
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.parse::<u64>().ok())
            .is_some_and(|length| u128::from(length) > self.body_limit as u128)
        {
            return Err(Box::new(detail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds configured limit",
            )));
        }
        let body = Limited::new(body, self.body_limit)
            .collect()
            .await
            .map_err(|error| {
                if error.is::<LengthLimitError>() {
                    detail(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "request body exceeds configured limit",
                    )
                } else {
                    detail(StatusCode::BAD_REQUEST, "failed to read request body")
                }
            })?
            .to_bytes();
        let uri = parts
            .extensions
            .get::<axum::extract::OriginalUri>()
            .map_or(&parts.uri, |original| &original.0);
        let target = uri.path_and_query().map_or("/", |value| value.as_str());
        let now = self.clock.now_ns().map_err(clock_error)?;
        let authenticated = self
            .verifier
            .authenticate(&parts.headers, &parts.method, target, &body, now)
            .map_err(auth_error)?;
        let admission = match &self.policy {
            Some(policy) => policy
                .admit(authenticated.caller())
                .await
                .map_err(|error| detail(StatusCode::FORBIDDEN, &error.to_string()))?,
            None => Admission::default(),
        };
        // A policy may await external work. A request that expires during that
        // wait must not regain admission after an earlier replay entry expires.
        let now = self.clock.now_ns().map_err(clock_error)?;
        self.verifier
            .check_freshness(&authenticated.caller().nonce_ns, now)
            .map_err(auth_error)?;
        let caller = authenticated
            .commit(self.verifier.store.as_ref())
            .await
            .map_err(auth_error)?;
        // A shared replay backend may also await I/O. Its successful result
        // cannot authorize an HTTP handler after the request has expired.
        self.verifier
            .check_freshness(&caller.nonce_ns, self.clock.now_ns().map_err(clock_error)?)
            .map_err(auth_error)?;
        if let Some(limiter) = &self.limiter {
            limiter.check(&caller.hotkey).map_err(|error| match error {
                LimitError::RateLimited { retry_after } => {
                    let mut response =
                        detail(StatusCode::TOO_MANY_REQUESTS, "caller rate limit exceeded");
                    let seconds = retry_after
                        .as_secs()
                        .saturating_add(u64::from(retry_after.subsec_nanos() != 0))
                        .max(1);
                    if let Ok(value) = http::HeaderValue::from_str(&seconds.to_string()) {
                        response.headers_mut().insert(header::RETRY_AFTER, value);
                    }
                    response
                }
                LimitError::Full => {
                    detail(StatusCode::SERVICE_UNAVAILABLE, "caller limiter is full")
                }
                LimitError::Unavailable => detail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "caller limiter is unavailable",
                ),
            })?;
        }
        parts.extensions.insert(caller);
        parts.extensions.insert(CallerPriority(admission.priority));
        Ok(Request::from_parts(parts, Body::from(body)))
    }
}

impl<S> Layer<S> for BtAuthLayer {
    type Service = BtAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BtAuthService {
            inner,
            config: self.clone(),
        }
    }
}

/// A service that validates the raw request before invoking its handler.
#[derive(Clone)]
pub struct BtAuthService<S> {
    inner: S,
    config: BtAuthLayer,
}

impl<S> Service<Request> for BtAuthService<S>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send,
    S::Error: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        // Call the instance whose readiness was polled, retaining a clone for
        // the next request. Cloned services need not share readiness permits.
        let replacement = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, replacement);
        let config = self.config.clone();
        Box::pin(async move {
            match config.authenticate(req).await {
                Ok(req) => inner.call(req).await,
                Err(response) => Ok(*response),
            }
        })
    }
}

fn auth_error(error: AuthError) -> Response {
    match error {
        AuthError::Store(StoreError::Full) => {
            detail(StatusCode::SERVICE_UNAVAILABLE, "nonce store is full")
        }
        AuthError::Store(StoreError::Unavailable(_)) => detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "nonce store is unavailable",
        ),
        error => detail(StatusCode::UNAUTHORIZED, &error.to_string()),
    }
}

fn clock_error(_: ClockError) -> Response {
    detail(
        StatusCode::SERVICE_UNAVAILABLE,
        "verification clock is unavailable",
    )
}

fn detail(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "detail": message }))).into_response()
}

/// An axum JSON extractor using the workspace's Value-first deserializer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Json<T>(pub T);

/// JSON rejection with axum's status codes and response text prefixes.
#[derive(Debug)]
pub enum JsonRejection {
    /// Missing or unsupported JSON media type (415).
    MissingContentType,
    /// Invalid JSON syntax (400).
    Syntax(serde_json::Error),
    /// Valid JSON that does not fit the target type (422).
    Data(serde_json::Error),
    /// The body extractor failed, including its configured size limit.
    Bytes(BytesRejection),
}

impl fmt::Display for JsonRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingContentType => {
                f.write_str("Expected request with `Content-Type: application/json`")
            }
            Self::Syntax(error) => write!(f, "Failed to parse the request body as JSON: {error}"),
            Self::Data(error) => write!(
                f,
                "Failed to deserialize the JSON body into the target type: {error}"
            ),
            Self::Bytes(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for JsonRejection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syntax(error) | Self::Data(error) => Some(error),
            Self::Bytes(error) => Some(error),
            Self::MissingContentType => None,
        }
    }
}

impl JsonRejection {
    /// HTTP status using axum's JSON rejection classification.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::MissingContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Syntax(_) => StatusCode::BAD_REQUEST,
            Self::Data(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Bytes(error) => error.status(),
        }
    }

    /// Response text using axum's JSON rejection prefixes.
    pub fn body_text(&self) -> String {
        match self {
            Self::Bytes(error) => error.body_text(),
            error => error.to_string(),
        }
    }
}

impl IntoResponse for JsonRejection {
    fn into_response(self) -> Response {
        match self {
            Self::MissingContentType => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Expected request with `Content-Type: application/json`",
            )
                .into_response(),
            Self::Syntax(error) => (
                StatusCode::BAD_REQUEST,
                format!("Failed to parse the request body as JSON: {error}"),
            )
                .into_response(),
            Self::Data(error) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Failed to deserialize the JSON body into the target type: {error}"),
            )
                .into_response(),
            Self::Bytes(error) => error.into_response(),
        }
    }
}

impl<T: DeserializeOwned> Json<T> {
    /// Parse raw JSON through an intermediate value before typed decoding.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, JsonRejection> {
        rtao_primitives::json::from_slice(bytes)
            .map(Self)
            .map_err(|error| match error.classify() {
                serde_json::error::Category::Data => JsonRejection::Data(error),
                _ => JsonRejection::Syntax(error),
            })
    }
}

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Json<T> {
    type Rejection = JsonRejection;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<mime::Mime>().ok())
            .is_some_and(|mime| {
                mime.type_() == mime::APPLICATION
                    && (mime.subtype() == mime::JSON || mime.suffix() == Some(mime::JSON))
            });
        if !is_json {
            return Err(JsonRejection::MissingContentType);
        }
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(JsonRejection::Bytes)?;
        Self::from_bytes(&bytes)
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl<T> From<T> for Json<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> std::ops::Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_time_advances_monotonically_across_wall_clock_rollback()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = VerificationClock::default();
        let initial = Instant::now();
        let unix_ns = 1_752_076_800_000_000_000;
        assert_eq!(clock.sample(unix_ns, initial)?, unix_ns);
        assert_eq!(
            clock.sample(unix_ns - 120_000_000_000, initial + Duration::from_secs(60))?,
            unix_ns + 60_000_000_000
        );
        assert_eq!(
            clock.sample(unix_ns, initial + Duration::from_secs(120))?,
            unix_ns + 120_000_000_000
        );
        assert_eq!(
            clock.sample(
                unix_ns + 240_000_000_000,
                initial + Duration::from_secs(121)
            )?,
            unix_ns + 240_000_000_000
        );
        assert_eq!(
            clock.sample(unix_ns, initial + Duration::from_secs(122))?,
            unix_ns + 241_000_000_000
        );
        assert_eq!(
            clock.sample(unix_ns, initial + Duration::from_secs(121))?,
            unix_ns + 241_000_000_000
        );
        assert_eq!(
            clock.sample(unix_ns, initial + Duration::from_secs(123))?,
            unix_ns + 242_000_000_000
        );
        Ok(())
    }

    #[test]
    fn limiter_never_evicts_debt_and_reclaims_only_full_buckets()
    -> Result<(), Box<dyn std::error::Error>> {
        let limiter = PerHotkeyLimiter::new(LimiterConfig {
            burst: 2,
            refill_interval: Duration::from_secs(1),
            max_hotkeys: 1,
        })?;
        let now = Instant::now();
        assert_eq!(limiter.check_at(&AccountId([1; 32]), now), Ok(()));
        assert_eq!(
            limiter.check_at(&AccountId([2; 32]), now),
            Err(LimitError::Full)
        );
        assert_eq!(limiter.check_at(&AccountId([1; 32]), now), Ok(()));
        assert_eq!(
            limiter.check_at(&AccountId([1; 32]), now + Duration::from_millis(500)),
            Err(LimitError::RateLimited {
                retry_after: Duration::from_millis(500)
            })
        );
        assert_eq!(
            limiter.check_at(&AccountId([2; 32]), now + Duration::from_secs(1)),
            Err(LimitError::Full)
        );
        assert_eq!(
            limiter.check_at(&AccountId([2; 32]), now + Duration::from_secs(2)),
            Ok(())
        );
        assert_eq!(
            limiter.check_at(&AccountId([1; 32]), now + Duration::from_secs(2)),
            Err(LimitError::Full)
        );
        Ok(())
    }

    #[test]
    fn limiter_refills_at_boundary_without_renewing_debt_on_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let limiter = PerHotkeyLimiter::new(LimiterConfig {
            burst: 1,
            refill_interval: Duration::from_secs(1),
            max_hotkeys: 2,
        })?;
        let now = Instant::now();
        let key = AccountId([1; 32]);
        assert_eq!(limiter.check_at(&key, now), Ok(()));
        assert_eq!(
            limiter.check_at(&key, now + Duration::from_millis(999)),
            Err(LimitError::RateLimited {
                retry_after: Duration::from_millis(1)
            })
        );
        assert_eq!(limiter.check_at(&key, now + Duration::from_secs(1)), Ok(()));
        assert_eq!(limiter.check_at(&AccountId([2; 32]), now), Ok(()));
        Ok(())
    }

    #[test]
    fn limiter_rejects_zero_configuration() {
        for config in [
            LimiterConfig {
                burst: 0,
                ..LimiterConfig::default()
            },
            LimiterConfig {
                max_hotkeys: 0,
                ..LimiterConfig::default()
            },
            LimiterConfig {
                refill_interval: Duration::ZERO,
                ..LimiterConfig::default()
            },
        ] {
            assert!(PerHotkeyLimiter::new(config).is_err());
        }
    }

    #[cfg(feature = "metagraph-policy")]
    #[test]
    fn metagraph_policy_checks_membership_permit_stake_and_exact_floor()
    -> Result<(), Box<dyn std::error::Error>> {
        use rtao_core::codec::Value;
        use rtao_metagraph::{Metagraph, SnapshotInfo};
        use rtao_primitives::{CryptoType, NetUid};
        let graph = Metagraph::from_dynamic(
            NetUid(1),
            Value::record(vec![
                ("block".into(), Value::Int(1)),
                ("tempo".into(), Value::Int(10)),
                ("last_step".into(), Value::Int(0)),
                ("blocks_since_last_step".into(), Value::Int(1)),
                ("network_registered_at".into(), Value::Int(0)),
                ("owner_hotkey".into(), Value::Bytes(vec![10; 32])),
                ("owner_coldkey".into(), Value::Bytes(vec![11; 32])),
                (
                    "hotkeys".into(),
                    Value::List(vec![
                        Value::Bytes(vec![1; 32]),
                        Value::Bytes(vec![2; 32]),
                        Value::Bytes(vec![3; 32]),
                    ]),
                ),
                (
                    "validator_permit".into(),
                    Value::List(vec![
                        Value::Bool(true),
                        Value::Bool(false),
                        Value::Bool(true),
                    ]),
                ),
                (
                    "total_stake".into(),
                    Value::List(vec![Value::Int(1000), Value::Int(2000), Value::Int(-1)]),
                ),
            ]),
            SnapshotInfo::default(),
        )?;
        let caller = |key| Caller {
            hotkey: AccountId([key; 32]),
            nonce_ns: 1.into(),
            crypto: CryptoType::Sr25519,
        };
        assert_eq!(
            metagraph_admission(&graph, &caller(1), true, true, Some(StakeWeight(1000)))?,
            Admission {
                priority: StakeWeight(1000)
            }
        );
        assert_eq!(
            metagraph_admission(&graph, &caller(1), true, true, Some(StakeWeight(1001))),
            Err(PolicyDenied(
                "caller stake is below the required minimum".into()
            ))
        );
        assert_eq!(
            metagraph_admission(&graph, &caller(2), true, true, None),
            Err(PolicyDenied(
                "caller does not have a validator permit".into()
            ))
        );
        assert_eq!(
            metagraph_admission(&graph, &caller(2), true, false, None)?,
            Admission {
                priority: StakeWeight(2000)
            }
        );
        assert_eq!(
            metagraph_admission(&graph, &caller(3), true, true, None),
            Err(PolicyDenied("caller stake is unavailable".into()))
        );
        assert_eq!(
            metagraph_admission(&graph, &caller(4), false, false, None)?,
            Admission::default()
        );
        for (registered, permit, floor) in [
            (true, false, None),
            (false, true, None),
            (false, false, Some(StakeWeight(0))),
        ] {
            assert_eq!(
                metagraph_admission(&graph, &caller(4), registered, permit, floor),
                Err(PolicyDenied(
                    "caller is not registered on this subnet".into()
                ))
            );
        }
        Ok(())
    }

    #[test]
    fn concurrent_limiter_calls_cannot_overspend_tokens_or_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        let limiter = Arc::new(PerHotkeyLimiter::new(LimiterConfig {
            burst: 8,
            refill_interval: Duration::from_secs(3600),
            max_hotkeys: 1,
        })?);
        let now = Instant::now();
        let tasks: Vec<_> = (0..64)
            .map(|_| {
                let limiter = limiter.clone();
                std::thread::spawn(move || limiter.check_at(&AccountId([1; 32]), now))
            })
            .collect();
        let mut accepted = 0;
        for task in tasks {
            accepted += usize::from(task.join().map_err(|_| "limiter thread failed")?.is_ok());
        }
        assert_eq!(accepted, 8);
        assert_eq!(
            limiter.check_at(&AccountId([2; 32]), now),
            Err(LimitError::Full)
        );
        Ok(())
    }
}
