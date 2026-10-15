//! Materialized btauth/1 requests and bounded concurrent axon queries.
//!
//! Each attempt signs the exact encoded path/query and body. Redirects and
//! reqwest's automatic retries are disabled so an existing signature is never
//! reused on a different request. HTTP responses may still stream.

use std::{fmt, sync::Arc, time::Duration};

use futures::{
    StreamExt,
    stream::{self, BoxStream},
};
use http::{
    HeaderMap, HeaderValue, Method, StatusCode,
    header::{CONTENT_TYPE, HeaderName},
};
pub use rtao_primitives::axon::{AxonTarget, Url, axon_base_url};
use rtao_primitives::{AccountId, Uid};
use rtao_signer::Signer;
use serde::{Serialize, de::DeserializeOwned};
use tokio::time::{Instant, timeout, timeout_at};

use crate::{AuthError, Clock, SystemClock, sign};

/// Timing, concurrency and response limits for one axon query call.
#[derive(Debug, Clone)]
pub struct FanOut {
    /// Maximum attempts in flight; further targets queue within the deadline.
    pub concurrency: usize,
    /// TCP/TLS connection timeout for each attempt.
    pub connect_timeout: Duration,
    /// Maximum duration of one attempt, including signing and response body.
    pub timeout: Duration,
    /// Call-wide deadline measured when the stream is created, including queueing.
    ///
    /// The default is 13 seconds (the default attempt timeout plus one second).
    /// Set `None` to remove the call deadline; modifying `timeout` does not
    /// automatically modify an explicitly stored deadline.
    pub deadline: Option<Duration>,
    /// Additional attempts for transient network errors or HTTP 408/429/5xx.
    /// Each attempt obtains a fresh nonce and signature.
    pub retries: u8,
    /// Maximum response-body bytes retained per attempt before JSON decoding.
    ///
    /// Defaults to 1 MiB. This must be positive; larger axon payloads require
    /// an explicit budget. Concurrent responses and decoded JSON use additional
    /// memory. The lower-level request API still returns a streaming response.
    pub max_response_bytes: usize,
}
impl Default for FanOut {
    fn default() -> Self {
        Self {
            concurrency: 256,
            connect_timeout: Duration::from_secs(3),
            timeout: Duration::from_secs(12),
            deadline: Some(Duration::from_secs(13)),
            retries: 0,
            max_response_bytes: 1024 * 1024,
        }
    }
}

/// A request could not be built, signed or transmitted.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// Authentication or signing failed before transmission.
    Auth(AuthError),
    /// HTTP connection, timeout or body transport failed.
    Http(reqwest::Error),
    /// Request JSON serialization failed.
    Json(serde_json::Error),
    /// Signing or transmission exceeded the request timeout.
    Timeout,
    /// URL cannot describe a credential-free HTTP endpoint.
    InvalidUrl(String),
    /// Method uses a request-target form unsupported by btauth path signing.
    InvalidMethod(String),
    /// An application header is invalid or overrides an authentication header.
    InvalidHeader(String),
    /// Timing, TLS roots or concurrency configuration is invalid.
    Configuration(String),
}
impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(error) => error.fmt(f),
            Self::Http(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
            Self::Timeout => f.write_str("signed HTTP request timed out"),
            Self::InvalidUrl(message)
            | Self::InvalidMethod(message)
            | Self::InvalidHeader(message)
            | Self::Configuration(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Auth(error) => Some(error),
            Self::Http(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// The result for one UID in a fan-out call.
#[derive(Debug)]
#[non_exhaustive]
pub enum FanOutError {
    /// The call deadline elapsed, including time spent waiting for a slot.
    Deadline,
    /// One attempt exceeded its timeout, including signing and body decoding.
    Timeout,
    /// Building, signing or transmitting the request failed.
    Request(ClientError),
    /// The axon returned an unsuccessful HTTP status; the code is retained.
    Status(StatusCode),
    /// The successful response was not valid JSON for the requested type.
    Decode(serde_json::Error),
    /// Declared or acquired response-body bytes exceeded the configured budget.
    /// This is a permanent response failure and is never retried.
    ResponseTooLarge {
        /// Maximum permitted response-body bytes for this attempt.
        limit: usize,
    },
}
impl fmt::Display for FanOutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Deadline => f.write_str("axon query call deadline elapsed"),
            Self::Timeout => f.write_str("axon query attempt timed out"),
            Self::Request(error) => error.fmt(f),
            Self::Status(status) => write!(f, "axon returned HTTP {status}"),
            Self::Decode(error) => error.fmt(f),
            Self::ResponseTooLarge { limit } => {
                write!(f, "axon response exceeds the {limit}-byte body limit")
            }
        }
    }
}
impl std::error::Error for FanOutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request(error) => Some(error),
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}
impl FanOutError {
    fn retryable(&self) -> bool {
        match self {
            Self::Timeout | Self::Request(ClientError::Timeout) => true,
            Self::Request(ClientError::Http(error)) => {
                error.is_connect() || error.is_timeout() || error.is_body()
            }
            Self::Status(status) => {
                status.is_server_error() || matches!(status.as_u16(), 408 | 429)
            }
            _ => false,
        }
    }
}

/// HTTP connection pool and signing identity shared by axon queries.
#[derive(Clone)]
pub struct SignedClient {
    http: reqwest::Client,
    signer: Arc<dyn Signer>,
    clock: Arc<dyn Clock>,
    tls: rustls::ClientConfig,
}
impl fmt::Debug for SignedClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedClient")
            .field("sender", &self.signer.account())
            .finish_non_exhaustive()
    }
}
impl SignedClient {
    /// Build with native trust roots, an explicit ring TLS provider and a shared
    /// monotonic system nonce clock. No process TLS default is consulted.
    pub fn new(signer: Arc<dyn Signer>) -> Result<Self, ClientError> {
        Self::with_clock(signer, Arc::new(SystemClock::default()))
    }
    /// Build with an injected clock and native roots using explicit ring TLS.
    pub fn with_clock(signer: Arc<dyn Signer>, clock: Arc<dyn Clock>) -> Result<Self, ClientError> {
        Self::with_extra_roots(signer, clock, Vec::new())
    }
    /// Append trust roots, retaining native roots and the explicit ring provider.
    pub fn with_extra_roots(
        signer: Arc<dyn Signer>,
        clock: Arc<dyn Clock>,
        extra_roots: Vec<rustls_pki_types::CertificateDer<'static>>,
    ) -> Result<Self, ClientError> {
        let mut roots = rustls::RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        let (added, _) = roots.add_parsable_certificates(native.certs);
        if added == 0 && extra_roots.is_empty() {
            return Err(ClientError::Configuration(
                "no usable native TLS roots; configure an explicit trust root".into(),
            ));
        }
        for root in extra_roots {
            roots.add(root).map_err(|error| {
                ClientError::Configuration(format!("invalid TLS trust root: {error}"))
            })?;
        }
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|error| ClientError::Configuration(error.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Self::with_tls_config(signer, clock, tls)
    }
    /// Use an owned TLS configuration created with an explicit provider.
    ///
    /// The client sets ALPN to `h2` and `http/1.1`; it passes this owned value to
    /// reqwest's preconfigured backend, never an `Arc<ClientConfig>`.
    pub fn with_tls_config(
        signer: Arc<dyn Signer>,
        clock: Arc<dyn Clock>,
        mut tls: rustls::ClientConfig,
    ) -> Result<Self, ClientError> {
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let http = http_client(tls.clone(), Duration::from_secs(3))?;
        Ok(Self {
            http,
            signer,
            clock,
            tls,
        })
    }
    /// Begin a receiver-bound request with an empty materialized body.
    pub fn request(
        &self,
        method: Method,
        url: Url,
        receiver: &AccountId,
    ) -> SignedRequestBuilder<'_> {
        SignedRequestBuilder {
            client: self,
            method,
            url,
            receiver: *receiver,
            body: Ok(Vec::new()),
            headers: HeaderMap::new(),
            timeout: Duration::from_secs(12),
        }
    }
    /// Yield each UID's response as it completes, with bounded concurrency and
    /// one call deadline that also resolves queued targets.
    pub fn fan_out_stream<Q, R>(
        &self,
        targets: impl IntoIterator<Item = AxonTarget>,
        make: impl Fn(&AxonTarget) -> (Method, String, Q) + Send + 'static,
        cfg: FanOut,
    ) -> BoxStream<'static, (Uid, Result<R, FanOutError>)>
    where
        Q: Serialize,
        R: DeserializeOwned + Send + 'static,
    {
        let started = Instant::now();
        let targets = targets.into_iter().collect::<Vec<_>>();
        let deadline = match cfg.deadline {
            Some(duration) => match started.checked_add(duration) {
                Some(at) => Some(at),
                None => {
                    return configuration_stream(
                        targets,
                        "call deadline exceeds monotonic clock range",
                    );
                }
            },
            None => None,
        };
        if cfg.concurrency == 0
            || cfg.connect_timeout.is_zero()
            || cfg.timeout.is_zero()
            || cfg.timeout.as_secs() > 31_536_000
            || cfg.connect_timeout.as_secs() > 31_536_000
        {
            return configuration_stream(
                targets,
                "concurrency and timeouts must be positive and timeouts at most one year",
            );
        }
        if cfg.max_response_bytes == 0 {
            return configuration_stream(targets, "response-byte budget must be positive");
        }
        let http = match http_client(self.tls.clone(), cfg.connect_timeout) {
            Ok(http) => http,
            Err(error) => return configuration_stream(targets, &error.to_string()),
        };
        let client = self.clone();
        let concurrency = cfg.concurrency;
        stream::iter(targets)
            .map(move |target| {
                let client = client.clone();
                let http = http.clone();
                let cfg = cfg.clone();
                // Materialize before entering the attempt future, and preserve these
                // same bytes across retries without requiring Q: Clone or Send.
                let request = if deadline.is_some_and(|at| Instant::now() >= at) {
                    None
                } else {
                    let (method, path, body) = make(&target);
                    Some((method, path, serde_json::to_vec(&body)))
                };
                async move {
                    let uid = target.uid;
                    let Some((method, path, body)) = request else {
                        return (uid, Err(FanOutError::Deadline));
                    };
                    if deadline.is_some_and(|at| Instant::now() >= at) {
                        return (uid, Err(FanOutError::Deadline));
                    }
                    let query = async {
                        let body =
                            body.map_err(|error| FanOutError::Request(ClientError::Json(error)))?;
                        let url =
                            target_url(&target.base_url, &path).map_err(FanOutError::Request)?;
                        let mut headers = HeaderMap::new();
                        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                        for attempt in 0..=cfg.retries {
                            if deadline.is_some_and(|at| Instant::now() >= at) {
                                return Err(FanOutError::Deadline);
                            }
                            let attempt_started = Instant::now();
                            let budget = deadline.map_or(cfg.timeout, |at| {
                                cfg.timeout
                                    .min(at.saturating_duration_since(attempt_started))
                            });
                            let response = timeout(cfg.timeout, async {
                                let response = client
                                    .send(
                                        &http,
                                        RequestParts {
                                            method: &method,
                                            url: &url,
                                            receiver: &target.hotkey,
                                            body: &body,
                                            headers: headers.clone(),
                                            deadline,
                                        },
                                        budget,
                                    )
                                    .await
                                    .map_err(FanOutError::Request)?;
                                if !response.status().is_success() {
                                    return Err(FanOutError::Status(response.status()));
                                }
                                let bytes =
                                    response_bytes(response, cfg.max_response_bytes).await?;
                                rtao_primitives::json::from_slice::<R>(&bytes)
                                    .map_err(FanOutError::Decode)
                            })
                            .await
                            .unwrap_or(Err(FanOutError::Timeout));
                            let response = finish_attempt(
                                response,
                                attempt_started.elapsed(),
                                cfg.timeout,
                                deadline.is_some_and(|at| Instant::now() >= at),
                            );
                            if response.is_ok()
                                || attempt == cfg.retries
                                || !response.as_ref().err().is_some_and(FanOutError::retryable)
                            {
                                return response;
                            }
                        }
                        Err(FanOutError::Timeout)
                    };
                    let result = match deadline {
                        Some(at) => timeout_at(at, query)
                            .await
                            .unwrap_or(Err(FanOutError::Deadline)),
                        None => query.await,
                    };
                    (uid, result)
                }
            })
            .buffer_unordered(concurrency)
            .boxed()
    }
    /// Collect the completion-ordered response stream, retaining every UID and
    /// its individual result. Dropping the stream cancels pending attempts.
    pub async fn fan_out<Q, R>(
        &self,
        targets: impl IntoIterator<Item = AxonTarget>,
        make: impl Fn(&AxonTarget) -> (Method, String, Q) + Send + 'static,
        cfg: FanOut,
    ) -> Vec<(Uid, Result<R, FanOutError>)>
    where
        Q: Serialize,
        R: DeserializeOwned + Send + 'static,
    {
        self.fan_out_stream(targets, make, cfg).collect().await
    }
    async fn send(
        &self,
        http: &reqwest::Client,
        parts: RequestParts<'_>,
        limit: Duration,
    ) -> Result<reqwest::Response, ClientError> {
        let started = Instant::now();
        let RequestParts {
            method,
            url,
            receiver,
            body,
            mut headers,
            deadline,
        } = parts;
        validate_url(url)?;
        if method == Method::CONNECT {
            return Err(ClientError::InvalidMethod(
                "CONNECT uses authority-form and cannot use btauth path/query signing".into(),
            ));
        }
        let mut target = url.path().to_owned();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        let authentication = sign(
            self.signer.as_ref(),
            method,
            &target,
            body,
            Some(receiver),
            self.clock.as_ref(),
        )
        .await
        .map_err(ClientError::Auth)?;
        let remaining = limit.saturating_sub(started.elapsed());
        let remaining = deadline.map_or(remaining, |at| {
            remaining.min(at.saturating_duration_since(Instant::now()))
        });
        if remaining.is_zero() {
            return Err(ClientError::Timeout);
        }
        headers.extend(authentication);
        http.request(method.clone(), url.clone())
            .headers(headers)
            .body(body.to_vec())
            .timeout(remaining)
            .send()
            .await
            .map_err(ClientError::Http)
    }
}

fn finish_attempt<R>(
    response: Result<R, FanOutError>,
    elapsed: Duration,
    timeout: Duration,
    deadline_elapsed: bool,
) -> Result<R, FanOutError> {
    // Tokio polls the inner future before its timer. Preserve a completed size
    // rejection even if that poll crossed a timing boundary: replacing it with
    // Timeout would turn a permanent resource failure into a retryable one.
    if matches!(&response, Err(FanOutError::ResponseTooLarge { .. })) {
        return response;
    }
    // Deserialize is synchronous and may return Ready after a timer's deadline
    // in the same future poll. Such successful responses must still be refused.
    if deadline_elapsed {
        Err(FanOutError::Deadline)
    } else if elapsed >= timeout {
        Err(FanOutError::Timeout)
    } else {
        response
    }
}

async fn response_bytes(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, FanOutError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(FanOutError::ResponseTooLarge { limit });
    }
    // Do not reserve from the peer's declared length. Check every acquired
    // chunk before copying it, including bodies without a Content-Length.
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| FanOutError::Request(ClientError::Http(error)))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(FanOutError::ResponseTooLarge { limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct RequestParts<'a> {
    method: &'a Method,
    url: &'a Url,
    receiver: &'a AccountId,
    body: &'a [u8],
    headers: HeaderMap,
    deadline: Option<Instant>,
}

fn http_client(
    tls: rustls::ClientConfig,
    connect_timeout: Duration,
) -> Result<reqwest::Client, ClientError> {
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(ClientError::Http)
}
fn validate_url(url: &Url) -> Result<(), ClientError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ClientError::InvalidUrl(
            "axon URL must be http/https without credentials or a fragment".into(),
        ));
    }
    Ok(())
}
fn target_url(base: &Url, path: &str) -> Result<Url, ClientError> {
    validate_url(base)?;
    // Only origin-form paths are accepted: a target's receiver cannot be bound
    // to a request silently redirected to another authority by make().
    if !path.starts_with('/') || path.starts_with("//") || path.contains('#') {
        return Err(ClientError::InvalidUrl(
            "axon query path must start with one slash and have no fragment".into(),
        ));
    }
    let mut origin = base.clone();
    origin.set_path("/");
    origin.set_query(None);
    let joined = origin
        .join(path)
        .map_err(|_| ClientError::InvalidUrl("invalid axon query path".into()))?;
    validate_url(&joined)?;
    if joined.origin() != base.origin() {
        return Err(ClientError::InvalidUrl(
            "axon query path changed endpoint authority".into(),
        ));
    }
    Ok(joined)
}
fn configuration_stream<R: Send + 'static>(
    targets: Vec<AxonTarget>,
    message: &str,
) -> BoxStream<'static, (Uid, Result<R, FanOutError>)> {
    let message = message.to_owned();
    stream::iter(targets.into_iter().map(move |target| {
        (
            target.uid,
            Err(FanOutError::Request(ClientError::Configuration(
                message.clone(),
            ))),
        )
    }))
    .boxed()
}

/// Materialized body, headers and timeout for one receiver-bound request.
pub struct SignedRequestBuilder<'a> {
    client: &'a SignedClient,
    method: Method,
    url: Url,
    receiver: AccountId,
    body: Result<Vec<u8>, ClientError>,
    headers: HeaderMap,
    timeout: Duration,
}
impl SignedRequestBuilder<'_> {
    /// Serialize JSON exactly once, before signing. The exact resulting bytes
    /// are transmitted with `Content-Type: application/json`.
    pub fn json<Q: Serialize + ?Sized>(mut self, body: &Q) -> Self {
        if self.body.is_ok() {
            self.body = serde_json::to_vec(body).map_err(ClientError::Json);
        }
        self.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        self
    }
    /// Supply the exact bytes to hash and transmit. Streaming request bodies
    /// require materialization by the caller before this operation.
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        if self.body.is_ok() {
            self.body = Ok(body.into());
        }
        self
    }
    /// Add an application header. Authentication headers are set only by sign().
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let parsed = HeaderName::from_bytes(name.as_ref().as_bytes())
            .ok()
            .zip(HeaderValue::from_str(value.as_ref()).ok());
        match parsed {
            Some((name, value))
                if !name.as_str().starts_with("x-bittensor-")
                    && !matches!(
                        name.as_str(),
                        "host" | "content-length" | "transfer-encoding" | "connection"
                    ) =>
            {
                self.headers.insert(name, value);
            }
            _ => {
                if self.body.is_ok() {
                    self.body = Err(ClientError::InvalidHeader(
                        "invalid or reserved authentication header".into(),
                    ));
                }
            }
        }
        self
    }
    /// Bound signing, transmission and response-body consumption for this request.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Obtain a fresh signature and transmit the materialized request. HTTP
    /// error statuses remain inspectable on the returned response.
    pub async fn send(self) -> Result<reqwest::Response, ClientError> {
        if self.timeout.is_zero() || self.timeout.as_secs() > 31_536_000 {
            return Err(ClientError::Configuration(
                "request timeout must be positive and at most one year".into(),
            ));
        }
        let body = self.body?;
        timeout(
            self.timeout,
            self.client.send(
                &self.client.http,
                RequestParts {
                    method: &self.method,
                    url: &self.url,
                    receiver: &self.receiver,
                    body: &body,
                    headers: self.headers,
                    deadline: None,
                },
                self.timeout,
            ),
        )
        .await
        .unwrap_or(Err(ClientError::Timeout))
    }
}

#[cfg(test)]
mod attempt_budget_tests {
    use super::*;

    #[test]
    fn completed_oversize_rejection_remains_permanent_across_timing_boundaries() {
        let timeout = Duration::from_secs(1);
        // Inject elapsed time at the production post-poll boundary, avoiding
        // scheduler races or sleeps while covering Tokio's inner-Ready case.
        for elapsed in [timeout - Duration::from_nanos(1), timeout, timeout * 2] {
            for deadline_elapsed in [false, true] {
                let response = finish_attempt::<()>(
                    Err(FanOutError::ResponseTooLarge { limit: 11 }),
                    elapsed,
                    timeout,
                    deadline_elapsed,
                );
                assert!(matches!(
                    response,
                    Err(FanOutError::ResponseTooLarge { limit: 11 })
                ));
                assert!(!response.as_ref().err().is_some_and(FanOutError::retryable));
            }
        }
    }

    #[test]
    fn successful_inner_ready_still_obeys_attempt_and_call_timing_boundaries() {
        let timeout = Duration::from_secs(1);
        assert!(matches!(
            finish_attempt(Ok(()), timeout, timeout, false),
            Err(FanOutError::Timeout)
        ));
        assert!(matches!(
            finish_attempt(Ok(()), Duration::ZERO, timeout, true),
            Err(FanOutError::Deadline)
        ));
        assert!(finish_attempt(Ok(()), Duration::ZERO, timeout, false).is_ok());
    }
}
