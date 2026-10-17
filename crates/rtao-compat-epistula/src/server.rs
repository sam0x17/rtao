//! Tower authentication for Epistula requests, mirroring `rtao_btauth::server`.
//!
//! The pipeline is: capture the time when the request head arrives, run the
//! header-only and freshness checks before reading the body, buffer the body
//! under a size limit and a deadline, verify the signature, evaluate the
//! caller policy, commit the replay identity and apply the rate limiter.
//!
//! Freshness is measured at the request head, as BitMind does, so a large
//! upload is not rejected as stale. Replay safety instead comes from the
//! body deadline: the body read, the policy and the replay commit must all
//! finish within the
//! [`body_deadline`](crate::server::EpistulaLayer::body_deadline) of the
//! head, and the replay store must retain entries for longer than
//! `max_age_ms + max_future_ms + body_deadline`.
//!
//! # What an Epistula signature binds
//!
//! An Epistula signature covers the body digest, uuid, timestamp and
//! receiver. Unlike btauth, it does **not** cover the HTTP method, path or
//! query. Over plain HTTP, anyone on the network path can take a signed
//! request that never reached this server and send its headers and body to a
//! different method, path or query; the unused uuid does not stop that.
//! Mount Epistula only on routes whose meaning is fully in the body, and do
//! not treat an Epistula caller as a btauth caller. For that reason the
//! layer inserts a btauth [`Caller`](rtao_btauth::Caller), which
//! [`VerifiedCaller`](crate::server::VerifiedCaller) reads, only when
//! [`with_btauth_caller`](crate::server::EpistulaLayer::with_btauth_caller)
//! opts in. Handlers can always tell the protocols apart: Epistula requests
//! carry an [`EpistulaCaller`] extension, read by
//! [`VerifiedEpistula`](crate::server::VerifiedEpistula), and btauth
//! requests never do.

use std::{
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, FromRequestParts, Request},
    response::{IntoResponse, Response},
};
use http::{StatusCode, header as http_header, request::Parts};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use rtao_btauth::{
    BoxFuture, Caller, Clock, NonceStore, StoreError, SystemClock,
    server::{BtAuthLayer, BtAuthService, LimitError},
};
use rtao_primitives::CryptoType;
use tower::{Layer, Service, ServiceExt};

#[cfg(feature = "metagraph-policy")]
#[cfg_attr(docsrs, doc(cfg(feature = "metagraph-policy")))]
pub use rtao_btauth::server::MetagraphPolicy;
pub use rtao_btauth::server::{
    Admission, CallerPolicy, CallerPriority, DEFAULT_BODY_LIMIT, LimiterConfig, PerHotkeyLimiter,
    PolicyDenied, VerifiedCaller,
};

use crate::{
    BodyDigest, ConfigError, EpistulaCaller, EpistulaError, ReplayPolicy, Verifier, header, now_ms,
    verify::btauth_caller,
};

/// Default deadline for the body read, policy and replay commit, measured
/// from the request head.
pub const DEFAULT_BODY_DEADLINE: Duration = Duration::from_secs(30);

/// HTTP status codes for each outcome class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusMap {
    /// Malformed, stale or unverifiable requests.
    pub auth: StatusCode,
    /// Caller policy denial.
    pub policy: StatusCode,
    /// A replayed uuid or signature.
    pub replay: StatusCode,
    /// The replay store is full or unavailable, or the clock failed.
    pub store: StatusCode,
    /// The per-hotkey rate limit.
    pub limiter: StatusCode,
}

impl StatusMap {
    /// BitMind: 400 for authentication failures and 401 for policy denial.
    pub const BITMIND: Self = Self {
        auth: StatusCode::BAD_REQUEST,
        policy: StatusCode::UNAUTHORIZED,
        replay: StatusCode::BAD_REQUEST,
        store: StatusCode::SERVICE_UNAVAILABLE,
        limiter: StatusCode::TOO_MANY_REQUESTS,
    };

    /// btauth: 401 for authentication failures and 403 for policy denial.
    pub const BTAUTH: Self = Self {
        auth: StatusCode::UNAUTHORIZED,
        policy: StatusCode::FORBIDDEN,
        replay: StatusCode::UNAUTHORIZED,
        store: StatusCode::SERVICE_UNAVAILABLE,
        limiter: StatusCode::TOO_MANY_REQUESTS,
    };
}

impl Default for StatusMap {
    fn default() -> Self {
        Self::BITMIND
    }
}

/// The verified caller supplied by an outer [`EpistulaLayer`].
///
/// Its presence marks a request as Epistula-authenticated. The layer inserts
/// the equivalent btauth [`Caller`] only when
/// [`EpistulaLayer::with_btauth_caller`] opts in.
#[derive(Clone, Debug)]
pub struct VerifiedEpistula(pub EpistulaCaller);

impl<S: Send + Sync> FromRequestParts<S> for VerifiedEpistula {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<EpistulaCaller>()
            .cloned()
            .map(Self)
            .ok_or_else(|| detail(StatusCode::UNAUTHORIZED, "missing authenticated caller"))
    }
}

/// Epistula authentication middleware shared by all service clones.
#[derive(Clone)]
pub struct EpistulaLayer {
    verifier: Arc<Verifier<dyn NonceStore>>,
    policy: Option<Arc<dyn CallerPolicy>>,
    limiter: Option<Arc<PerHotkeyLimiter>>,
    body_limit: usize,
    clock: Arc<dyn Clock>,
    status: StatusMap,
    prefilter: bool,
    btauth_caller: bool,
    deadline: Duration,
    #[cfg(feature = "secret-filter")]
    secret: Option<Arc<crate::secret::SecretSigFilter>>,
}

impl EpistulaLayer {
    /// Authenticate with BitMind status codes and a 4 MiB body cap.
    ///
    /// The body deadline is [`DEFAULT_BODY_DEADLINE`], shortened when the
    /// replay store's retention leaves less slack (see
    /// [`Self::with_body_deadline`]). The deadline uses a Tokio timer, so
    /// the service must run on a runtime with the time driver enabled, as
    /// `axum::serve` does.
    pub fn new(verifier: Arc<Verifier<dyn NonceStore>>) -> Self {
        let deadline = verifier
            .commit_slack()
            .map_or(DEFAULT_BODY_DEADLINE, |slack| {
                slack.min(DEFAULT_BODY_DEADLINE)
            });
        Self {
            verifier,
            policy: None,
            limiter: None,
            body_limit: DEFAULT_BODY_LIMIT,
            clock: Arc::new(SystemClock::default()),
            status: StatusMap::BITMIND,
            prefilter: false,
            btauth_caller: false,
            deadline,
            #[cfg(feature = "secret-filter")]
            secret: None,
        }
    }

    /// Bound the time from the request head to the replay commit.
    ///
    /// Requests whose body read, policy and commit take longer get 408. With
    /// replay enforcement, fails unless the store retains entries strictly
    /// longer than `max_age_ms + max_future_ms + deadline` (in whole
    /// milliseconds), which keeps head-time freshness replay-safe.
    pub fn with_body_deadline(mut self, deadline: Duration) -> Result<Self, ConfigError> {
        if let Some(slack) = self.verifier.commit_slack()
            && deadline.as_millis() > slack.as_millis()
        {
            return Err(ConfigError::new(format!(
                "a {}ms body deadline needs a replay store retaining entries for more than \
                 max_age_ms + max_future_ms + {}ms; at most {}ms fits this store",
                deadline.as_millis(),
                deadline.as_millis(),
                slack.as_millis()
            )));
        }
        self.deadline = deadline;
        Ok(self)
    }

    /// The deadline from the request head to the replay commit.
    pub fn body_deadline(&self) -> Duration {
        self.deadline
    }

    /// Also insert the equivalent btauth [`Caller`], read by [`VerifiedCaller`].
    ///
    /// Off by default. Turn it on only when every route behind this layer
    /// accepts the weaker Epistula binding (see the module documentation):
    /// a btauth handler otherwise assumes the method and target were signed.
    pub fn with_btauth_caller(mut self, enabled: bool) -> Self {
        self.btauth_caller = enabled;
        self
    }

    /// Evaluate this caller policy after verification and before replay commitment.
    pub fn with_policy(mut self, policy: Arc<dyn CallerPolicy>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Evaluate the policy on the *claimed* sender before reading the body.
    ///
    /// BitMind rejects non-members with 401 before reading the body. This
    /// restores that cheap rejection. The policy still runs once per request:
    /// its admission is used only after the signature verifies, and only
    /// because the verified caller (sender and timestamp) is then identical
    /// to the claimed one. Off by default, because the policy then sees
    /// unauthenticated input: enable it only for a policy without per-caller
    /// side effects, such as a metagraph membership check, never for a quota
    /// that an attacker could spend by naming someone else's hotkey.
    pub fn with_claimed_sender_prefilter(mut self, enabled: bool) -> Self {
        self.prefilter = enabled;
        self
    }

    /// Apply bounded per-hotkey limiting after replay commitment.
    pub fn with_limiter(mut self, limiter: Arc<PerHotkeyLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// Set the raw-body cap.
    ///
    /// The layer also sets axum's `DefaultBodyLimit` to this value for the
    /// inner service, so a body extractor accepts every body the layer
    /// authenticated. A `DefaultBodyLimit` applied inside this layer wins.
    pub fn with_body_limit(mut self, body_limit: usize) -> Self {
        self.body_limit = body_limit;
        self
    }

    /// Supply the verification clock, usually for deterministic tests.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Select the status codes for each outcome class.
    pub fn with_status_map(mut self, status: StatusMap) -> Self {
        self.status = status;
        self
    }

    /// Check `Epistula-Secret-Signature-1` before reading the body.
    #[cfg(feature = "secret-filter")]
    #[cfg_attr(docsrs, doc(cfg(feature = "secret-filter")))]
    pub fn with_secret_filter(mut self, filter: Arc<crate::secret::SecretSigFilter>) -> Self {
        self.secret = Some(filter);
        self
    }

    /// Raw signed-body cap, also used for the router's `DefaultBodyLimit`.
    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    fn error(&self, error: EpistulaError) -> Response {
        match error {
            EpistulaError::Replayed => detail(self.status.replay, &error.to_string()),
            EpistulaError::Store(StoreError::Full) => {
                detail(self.status.store, "nonce store is full")
            }
            EpistulaError::Store(StoreError::Unavailable(_)) => {
                detail(self.status.store, "nonce store is unavailable")
            }
            EpistulaError::Clock(_) => {
                detail(self.status.store, "verification clock is unavailable")
            }
            EpistulaError::DeadlineExceeded => {
                detail(StatusCode::REQUEST_TIMEOUT, &error.to_string())
            }
            error => detail(self.status.auth, &error.to_string()),
        }
    }

    fn now(&self) -> Result<u64, Box<Response>> {
        now_ms(self.clock.as_ref()).map_err(|error| Box::new(self.error(error)))
    }

    /// The time left before the deadline that started at `head`.
    fn remaining(&self, head: u64) -> Result<Duration, Box<Response>> {
        let elapsed = Duration::from_millis(self.now()?.saturating_sub(head));
        self.deadline
            .checked_sub(elapsed)
            .ok_or_else(|| Box::new(self.error(EpistulaError::DeadlineExceeded)))
    }

    async fn authenticate(&self, req: Request) -> Result<Request, Box<Response>> {
        let head = self.now()?;
        let (mut parts, body) = req.into_parts();
        let (claimed, timestamp) = self
            .verifier
            .claimed(&parts.headers, head)
            .map_err(|error| self.error(error))?;
        #[cfg(feature = "secret-filter")]
        if let Some(filter) = &self.secret {
            filter
                .check(&parts.headers, head)
                .map_err(|error| self.error(error))?;
        }
        let mut prefiltered = None;
        if self.prefilter
            && let Some(policy) = &self.policy
        {
            let claimed = Caller {
                hotkey: claimed,
                nonce_ns: timestamp * 1_000_000,
                crypto: CryptoType::Sr25519,
            };
            let admission = policy
                .admit(&claimed)
                .await
                .map_err(|denied| detail(self.status.policy, &denied.to_string()))?;
            prefiltered = Some((claimed, admission));
        }
        if parts
            .headers
            .get(http_header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.parse::<u64>().ok())
            .is_some_and(|length| u128::from(length) > self.body_limit as u128)
        {
            return Err(Box::new(detail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body exceeds configured limit",
            )));
        }
        let remaining = self.remaining(head)?;
        let body = tokio::time::timeout(remaining, Limited::new(body, self.body_limit).collect())
            .await
            .map_err(|_| self.error(EpistulaError::DeadlineExceeded))?
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
        // Freshness stays measured at the head; the deadline bounds the rest.
        let authenticated = self
            .verifier
            .authenticate(&parts.headers, BodyDigest::Raw(&body), head)
            .map_err(|error| self.error(error))?;
        let verified = authenticated.btauth_caller();
        let admission = match (&self.policy, prefiltered) {
            (Some(_), Some((claimed, admission))) if claimed == verified => admission,
            (Some(policy), _) => policy
                .admit(&verified)
                .await
                .map_err(|denied| detail(self.status.policy, &denied.to_string()))?,
            (None, _) => Admission::default(),
        };
        // A policy may await external work; the commit must still be in time.
        self.remaining(head)?;
        let caller = match (self.verifier.store(), self.verifier.replay()) {
            (Some(store), ReplayPolicy::Enforce) => authenticated
                .commit(store.as_ref(), self.clock.as_ref())
                .await
                .map_err(|error| self.error(error))?,
            _ => authenticated.into_caller_without_replay(),
        };
        self.remaining(head)?;
        if let Some(limiter) = &self.limiter {
            limiter
                .check(&caller.hotkey)
                .map_err(|error| self.limit_error(error))?;
        }
        if self.btauth_caller {
            parts.extensions.insert(btauth_caller(&caller));
        }
        parts.extensions.insert(caller);
        parts.extensions.insert(CallerPriority(admission.priority));
        Ok(Request::from_parts(parts, Body::from(body)))
    }

    fn limit_error(&self, error: LimitError) -> Response {
        match error {
            LimitError::RateLimited { retry_after } => {
                let mut response = detail(self.status.limiter, "caller rate limit exceeded");
                let seconds = retry_after
                    .as_secs()
                    .saturating_add(u64::from(retry_after.subsec_nanos() != 0))
                    .max(1);
                if let Ok(value) = http::HeaderValue::from_str(&seconds.to_string()) {
                    response
                        .headers_mut()
                        .insert(http_header::RETRY_AFTER, value);
                }
                response
            }
            LimitError::Full => detail(self.status.store, "caller limiter is full"),
            LimitError::Unavailable => detail(self.status.store, "caller limiter is unavailable"),
        }
    }
}

impl<S> Layer<S> for EpistulaLayer {
    type Service = EpistulaService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        EpistulaService {
            inner,
            config: self.clone(),
        }
    }
}

/// A service that authenticates Epistula requests before invoking its handler.
#[derive(Clone)]
pub struct EpistulaService<S> {
    inner: S,
    config: EpistulaLayer,
}

impl<S> Service<Request> for EpistulaService<S>
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
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        let config = self.config.clone();
        Box::pin(async move {
            match config.authenticate(req).await {
                // The ready inner service, with the router's body extractors
                // raised to the limit this layer already enforced.
                Ok(req) => {
                    DefaultBodyLimit::max(config.body_limit)
                        .layer(inner)
                        .call(req)
                        .await
                }
                Err(response) => Ok(*response),
            }
        })
    }
}

/// Route requests carrying `Epistula-Signed-By` to Epistula, all others to btauth.
///
/// Lets one router accept both protocols during a migration. Epistula
/// handlers use [`VerifiedEpistula`] and btauth handlers [`VerifiedCaller`].
/// [`VerifiedCaller`] accepts Epistula requests too only when the Epistula
/// layer opts in with [`EpistulaLayer::with_btauth_caller`]; Epistula does
/// not sign the method, path or query (see the module documentation).
#[derive(Clone)]
pub struct DualAuthLayer {
    epistula: EpistulaLayer,
    btauth: BtAuthLayer,
}

impl DualAuthLayer {
    /// Combine an Epistula and a btauth layer.
    pub fn new(epistula: EpistulaLayer, btauth: BtAuthLayer) -> Self {
        Self { epistula, btauth }
    }
}

impl<S> Layer<S> for DualAuthLayer {
    type Service = DualAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DualAuthService {
            inner,
            config: self.clone(),
        }
    }
}

/// The service produced by [`DualAuthLayer`].
#[derive(Clone)]
pub struct DualAuthService<S> {
    inner: S,
    config: DualAuthLayer,
}

impl<S> Service<Request> for DualAuthService<S>
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
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        if req.headers().contains_key(header::SIGNED_BY) {
            let service: EpistulaService<S> = self.config.epistula.layer(inner);
            Box::pin(service.oneshot(req))
        } else {
            let service: BtAuthService<S> = self.config.btauth.layer(inner);
            Box::pin(service.oneshot(req))
        }
    }
}

fn detail(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "detail": message }))).into_response()
}
