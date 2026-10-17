// Derived from opentensor/bittensor bittensor/core/axon.py @ v10.5.0 (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/core/stream.py @ v10.5.0 (MIT); modified by rtao.
//! An axum v10 axon.
//!
//! The pipeline follows Python's middleware order: route lookup, header
//! parsing, blacklist, verification, priority, body integrity, forward and
//! response post-processing. Status codes and `{"message": …}` texts match
//! Python's for every reachable row of its error table.

use std::{
    convert::Infallible,
    fmt,
    future::Future,
    marker::PhantomData,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request, State},
    response::Response,
    routing::any,
};
use bytes::Bytes;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header::CONTENT_TYPE};
use http_body_util::{BodyExt, LengthLimitError, Limited};
use rtao_btauth::{Clock, SystemClock};
use rtao_primitives::{AccountId, pyfmt::float_repr, pyfmt::string_repr};
use rtao_signer::Signer;
use serde_json::Value;

use crate::{
    BaseSynapse, BoundedNonceTable, Envelope, RequestHead, Synapse, UnboundedNonceTable,
    V10_VERSION_AS_INT, V10NonceTable, V10VerifyPolicy, VerifiedHead, VerifyRejection,
    default_verify, is_valid_synapse_name, json::to_starlette_json, parse_head, uuid1_string,
    uuid4_string,
};

/// Whether the axon signs its own `bt_header_axon_signature`.
///
/// Python signs `"{axon_nonce}.{dendrite_hotkey}.{axon_hotkey}.{axon_uuid}"`,
/// with attacker-chosen header text, before any check. No dendrite verifies
/// that signature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AxonSignature {
    /// Never sign (default).
    #[default]
    Omit,
    /// Sign only requests that passed verification.
    AfterVerify,
}

/// Axon limits and verification policy.
#[derive(Clone, Debug)]
pub struct V10AxonConfig {
    /// Largest accepted request body (default 16 MiB; Python has none).
    pub body_limit: usize,
    /// Optional bound on a forward handler (Python: none) → 408.
    pub handler_timeout: Option<Duration>,
    /// Axon-side signature behaviour.
    pub axon_signature: AxonSignature,
    /// Nonce freshness rules.
    pub policy: V10VerifyPolicy,
    /// Bounded nonce-table capacity.
    pub nonce_capacity: usize,
    /// Live nonce-table keys one hotkey may hold.
    pub nonce_keys_per_hotkey: usize,
    /// Use Python's unbounded, never-evicted nonce table when `policy` cannot
    /// support a bounded one (for example [`V10VerifyPolicy::python`]). Any
    /// keypair can then grow the table without limit, so this is for
    /// differential tests only. Without it, [`Axon::new`] refuses such a
    /// policy.
    pub allow_unbounded_nonce_table: bool,
}

impl Default for V10AxonConfig {
    fn default() -> Self {
        Self {
            body_limit: 16 * 1024 * 1024,
            handler_timeout: None,
            axon_signature: AxonSignature::Omit,
            policy: V10VerifyPolicy::default(),
            nonce_capacity: BoundedNonceTable::DEFAULT_CAPACITY,
            nonce_keys_per_hotkey: BoundedNonceTable::DEFAULT_KEYS_PER_HOTKEY,
            allow_unbounded_nonce_table: false,
        }
    }
}

/// The axon configuration cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxonConfigError(pub String);

impl fmt::Display for AxonConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for AxonConfigError {}

fn nonce_table(
    config: &V10AxonConfig,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn V10NonceTable>, AxonConfigError> {
    match BoundedNonceTable::new(config.nonce_capacity, &config.policy, clock) {
        Ok(table) => Ok(Arc::new(
            table.with_keys_per_hotkey(config.nonce_keys_per_hotkey),
        )),
        Err(_) if config.allow_unbounded_nonce_table => {
            Ok(Arc::new(UnboundedNonceTable::default()))
        }
        Err(reason) => Err(AxonConfigError(format!(
            "{reason}; set allow_unbounded_nonce_table to use an unbounded table"
        ))),
    }
}

/// A priority hook failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PriorityError {
    /// Python's `TimeoutError` arm: 408 `Response timeout after: {timeout}s`.
    Timeout,
    /// Any other failure: 500.
    Failed(String),
}

/// Per-synapse hooks. Python passes hooks a header-only synapse; rtao passes
/// the parsed headers.
///
/// Blacklisting runs before verification, on an unverified hotkey claim: use
/// it only for deny decisions. Anything keyed by the caller (rate limits,
/// budgets) belongs in [`Hooks::verify`] or the forward handler, which see
/// the verified identity.
pub trait Hooks<S: Synapse>: Send + Sync + 'static {
    /// `Err(reason)` answers 403 `Forbidden. Key is blacklisted: {reason}.`.
    fn blacklist<'a>(&'a self, _head: &'a RequestHead) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    /// Extra checks after Python's default verification, which always runs.
    /// `Err(message)` answers 401 `Not Verified with error: {message}`.
    fn verify<'a>(&'a self, _head: &'a VerifiedHead) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    /// Python computes a priority but never schedules by it; the value is
    /// informational here too.
    fn priority<'a>(
        &'a self,
        _head: &'a VerifiedHead,
    ) -> BoxFuture<'a, Result<f64, PriorityError>> {
        Box::pin(async { Ok(0.0) })
    }
}

/// Hooks that admit everyone verification admits.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultHooks;
impl<S: Synapse> Hooks<S> for DefaultHooks {}

/// A forward handler's reply.
pub enum Reply<S> {
    /// A JSON synapse response. `axon.status_code` (default 200) becomes the
    /// HTTP status.
    Synapse(Envelope<S>),
    /// A streaming response: always HTTP 200 with `text/event-stream`, the
    /// envelope's headers and the raw chunks. Do not emit a terminator chunk.
    Stream {
        /// Envelope whose headers accompany the stream.
        head: Envelope<S>,
        /// Raw body chunks.
        body: BoxStream<'static, Bytes>,
    },
}

impl<S: fmt::Debug> fmt::Debug for Reply<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Synapse(envelope) => formatter.debug_tuple("Synapse").field(envelope).finish(),
            Self::Stream { head, .. } => formatter
                .debug_struct("Stream")
                .field("head", head)
                .finish_non_exhaustive(),
        }
    }
}

/// Python's `SynapseException(synapse=…)` analogue.
///
/// `status` defaults to 500 and `message` to `Internal Server Error #<uuid4>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SynapseError {
    /// HTTP status.
    pub status: Option<u16>,
    /// `{"message": …}` text.
    pub message: Option<String>,
}

impl SynapseError {
    /// An error with an explicit status and message.
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            message: Some(message.into()),
        }
    }
    /// Python's generic 500 for a non-synapse exception.
    pub fn internal() -> Self {
        Self::default()
    }
}

impl fmt::Display for SynapseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {}",
            self.status.unwrap_or(500),
            self.message.as_deref().unwrap_or("Internal Server Error")
        )
    }
}
impl std::error::Error for SynapseError {}

struct Inner {
    signer: Arc<dyn Signer>,
    me: AccountId,
    uuid: String,
    routes: Vec<(String, Arc<dyn Route>)>,
    table: Arc<dyn V10NonceTable>,
    clock: Arc<dyn Clock>,
    config: V10AxonConfig,
}

/// A v10 axon. Build with [`Axon::new`] and [`Axon::attach`], then mount
/// [`Axon::router`] or call [`Axon::serve`].
///
/// The router must be served with `into_make_service_with_connect_info::<SocketAddr>()`
/// so the dendrite's TCP address is known; otherwise `0.0.0.0:0` is reported.
/// Do not wrap it in btauth/1's `BtAuthLayer`: v10 requests carry no btauth/1
/// headers.
pub struct Axon {
    inner: Inner,
}

impl fmt::Debug for Axon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Axon")
            .field("hotkey", &self.inner.me)
            .field("uuid", &self.inner.uuid)
            .field("routes", &self.route_names())
            .finish_non_exhaustive()
    }
}

impl Axon {
    /// An axon for `signer`'s hotkey with the `/Synapse` ping attached.
    ///
    /// Fails when `config.policy` cannot support a bounded nonce table and
    /// `config.allow_unbounded_nonce_table` is not set.
    pub fn new(signer: Arc<dyn Signer>, config: V10AxonConfig) -> Result<Self, AxonConfigError> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::default());
        let table = nonce_table(&config, clock.clone())?;
        let axon = Self {
            inner: Inner {
                me: signer.account(),
                signer,
                uuid: uuid1_string(),
                routes: Vec::new(),
                table,
                clock,
                config,
            },
        };
        Ok(
            axon.attach::<BaseSynapse, _, _, _>(DefaultHooks, |_, envelope| async move {
                Ok(Reply::Synapse(envelope))
            }),
        )
    }

    /// Replace the verification clock, with a fresh nonce table built from
    /// the configuration that reads the same clock.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Result<Self, AxonConfigError> {
        self.inner.table = nonce_table(&self.inner.config, clock.clone())?;
        self.inner.clock = clock;
        Ok(self)
    }

    /// Replace the verification clock and nonce table (for tests).
    ///
    /// A [`BoundedNonceTable`] expires entries by its own clock, so build
    /// `table` with the same `clock`; [`Self::with_clock`] does that.
    pub fn with_clock_and_table(
        mut self,
        clock: Arc<dyn Clock>,
        table: Arc<dyn V10NonceTable>,
    ) -> Self {
        self.inner.clock = clock;
        self.inner.table = table;
        self
    }

    /// This axon's hotkey (the `MY_HOTKEY_SS58` of the signed message).
    pub fn hotkey(&self) -> AccountId {
        self.inner.me
    }

    /// This axon instance's uuid.
    pub fn uuid(&self) -> &str {
        &self.inner.uuid
    }

    /// Attached synapse names, in attach order.
    pub fn route_names(&self) -> Vec<String> {
        self.inner
            .routes
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Attach `forward` at `/{S::NAME}` (GET and POST). Re-attaching a name
    /// replaces it in place, as Python's dict assignment does. A `S::NAME`
    /// that is not an ASCII Python identifier fails to compile.
    ///
    /// `forward` receives the verified headers and the validated body. Before
    /// the call the body's `dendrite` is replaced by the verified header
    /// values (with the TCP peer address) and its `axon` by the header values
    /// with no status, so neither can be spoofed through the JSON body.
    pub fn attach<S, H, F, Fut>(mut self, hooks: H, forward: F) -> Self
    where
        S: Synapse,
        H: Hooks<S>,
        F: Fn(VerifiedHead, Envelope<S>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<Reply<S>, SynapseError>> + Send + 'static,
    {
        const {
            assert!(
                is_valid_synapse_name(S::NAME),
                "Synapse::NAME must be an ASCII Python identifier"
            );
        }
        let route: Arc<dyn Route> = Arc::new(RouteImpl {
            hooks,
            forward,
            marker: PhantomData::<fn() -> S>,
        });
        if let Some(slot) = self
            .inner
            .routes
            .iter_mut()
            .find(|(name, _)| name == S::NAME)
        {
            slot.1 = route;
        } else {
            self.inner.routes.push((S::NAME.to_owned(), route));
        }
        self
    }

    /// Routes `/{NAME}` for every attached synapse, plus a fallback that
    /// answers Python's 404 for unknown names.
    pub fn router(self) -> Router {
        let state = Arc::new(self.inner);
        Self::routes_for(&state)
            .fallback(dispatch)
            .with_state(state)
    }

    /// Exact-path routes only, for merging into another router.
    pub fn routes(self) -> Router {
        let state = Arc::new(self.inner);
        Self::routes_for(&state).with_state(state)
    }

    fn routes_for(state: &Arc<Inner>) -> Router<Arc<Inner>> {
        let mut router = Router::new();
        for (name, _) in &state.routes {
            router = router.route(&format!("/{name}"), any(dispatch));
        }
        router
    }

    /// Serve on `listener` until `shutdown` resolves, recording peer addresses.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
    }
}

trait Route: Send + Sync {
    fn handle(
        self: Arc<Self>,
        axon: Arc<Inner>,
        head: RequestHead,
        request: Request,
        start: Instant,
        unroutable: Option<Unroutable>,
    ) -> BoxFuture<'static, Response>;
}

/// FastAPI's answer for a known synapse name with extra path segments or
/// another method, given after the middleware's checks, as Python does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unroutable {
    NotFound,
    MethodNotAllowed,
}

/// Uvicorn's `unquote(raw_path)`: percent-decode as UTF-8, replacing invalid
/// sequences, and leave malformed escapes as they are.
fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(index + offset)
                .and_then(|byte| (*byte as char).to_digit(16))
        };
        match (bytes[index], hex(1), hex(2)) {
            (b'%', Some(high), Some(low)) => {
                out.push(u8::try_from(high * 16 + low).unwrap_or(b'?'));
                index += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct RouteImpl<S, H, F> {
    hooks: H,
    forward: F,
    marker: PhantomData<fn() -> S>,
}

async fn dispatch(State(axon): State<Arc<Inner>>, request: Request) -> Response {
    let start = Instant::now();
    let path = percent_decode(request.uri().path());
    let mut segments = path.split('/');
    segments.next();
    let name = segments.next().unwrap_or("").to_owned();
    let extra = segments.next().is_some();
    let Some(route) = axon
        .routes
        .iter()
        .find(|(route, _)| *route == name)
        .map(|(_, route)| route.clone())
    else {
        let names: Vec<String> = axon
            .routes
            .iter()
            .map(|(name, _)| string_repr(name))
            .collect();
        let message = format!(
            "Synapse name '{name}' not found. Available synapses [{}]",
            names.join(", ")
        );
        return error_response(404, Some(message), None, start);
    };
    let unroutable = if extra {
        Some(Unroutable::NotFound)
    } else if request.method() != Method::GET && request.method() != Method::POST {
        Some(Unroutable::MethodNotAllowed)
    } else {
        None
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(SocketAddr::from(([0, 0, 0, 0], 0)), |info| info.0);
    let head = match parse_head(request.headers(), &name, peer) {
        Ok(head) => head,
        Err(_) => return error_response(500, None, None, start),
    };
    route.handle(axon, head, request, start, unroutable).await
}

impl<S, H, F, Fut> Route for RouteImpl<S, H, F>
where
    S: Synapse,
    H: Hooks<S>,
    F: Fn(VerifiedHead, Envelope<S>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Reply<S>, SynapseError>> + Send + 'static,
{
    fn handle(
        self: Arc<Self>,
        axon: Arc<Inner>,
        mut head: RequestHead,
        request: Request,
        start: Instant,
        unroutable: Option<Unroutable>,
    ) -> BoxFuture<'static, Response> {
        Box::pin(async move {
            // Python preprocess: axon fields, then the TCP peer.
            let axon_nonce = axon.clock.now_ns().unwrap_or(0);
            head.axon.version = Some(V10_VERSION_AS_INT);
            head.axon.uuid = Some(axon.uuid.clone());
            head.axon.nonce = i64::try_from(axon_nonce).ok();
            head.axon.status_code = Some(100);
            head.axon.signature = None;
            head.dendrite.ip = Some(head.peer.ip().to_string());
            head.dendrite.port = Some(i64::from(head.peer.port()));

            if let Err(reason) = self.hooks.blacklist(&head).await {
                let message = format!("Forbidden. Key is blacklisted: {reason}.");
                return error_response(403, Some(message), Some(&head), start);
            }
            let mut verified = match default_verify(
                head,
                &axon.me,
                axon.table.as_ref(),
                &axon.config.policy,
                axon.clock.as_ref(),
            ) {
                Ok(verified) => verified,
                Err((head, VerifyRejection::NotVerified(message))) => {
                    let message = format!("Not Verified with error: {message}");
                    return error_response(401, Some(message), Some(&head), start);
                }
                Err((head, VerifyRejection::Unavailable(message))) => {
                    return error_response(503, Some(message), Some(&head), start);
                }
            };
            if axon.config.axon_signature == AxonSignature::AfterVerify {
                // Sign only verified text: the caller's verified hotkey and
                // this axon's own hotkey, never the unsigned
                // `bt_header_axon_hotkey` claim.
                let me = axon.me.to_string();
                let caller = verified.caller();
                let head = verified.head();
                if head
                    .axon
                    .hotkey
                    .as_deref()
                    .is_none_or(|claimed| claimed == me)
                {
                    let message = format!(
                        "{}.{caller}.{me}.{}",
                        head.axon
                            .nonce
                            .map_or_else(|| "None".to_owned(), |nonce| nonce.to_string()),
                        axon.uuid
                    );
                    if let Ok(signature) = axon.signer.sign_raw(message.as_bytes()).await {
                        verified.head_mut().axon.signature =
                            Some(format!("0x{}", hex::encode(signature)));
                    }
                }
            }
            if let Err(message) = self.hooks.verify(&verified).await {
                let message = format!("Not Verified with error: {message}");
                return error_response(401, Some(message), Some(verified.head()), start);
            }
            match self.hooks.priority(&verified).await {
                Ok(_) => {}
                Err(PriorityError::Timeout) => {
                    let timeout = verified
                        .head()
                        .timeout
                        .map_or_else(|| "None".to_owned(), float_repr);
                    let message = format!("Response timeout after: {timeout}s");
                    return error_response(408, Some(message), Some(verified.head()), start);
                }
                Err(PriorityError::Failed(_)) => {
                    return error_response(500, None, Some(verified.head()), start);
                }
            }
            match unroutable {
                Some(Unroutable::NotFound) => {
                    detail_response(StatusCode::NOT_FOUND, Value::from("Not Found"))
                }
                Some(Unroutable::MethodNotAllowed) => {
                    let mut response = detail_response(
                        StatusCode::METHOD_NOT_ALLOWED,
                        Value::from("Method Not Allowed"),
                    );
                    response
                        .headers_mut()
                        .insert(http::header::ALLOW, HeaderValue::from_static("GET, POST"));
                    response
                }
                None => self.run(axon, verified, request, start).await,
            }
        })
    }
}

impl<S, H, F, Fut> RouteImpl<S, H, F>
where
    S: Synapse,
    H: Hooks<S>,
    F: Fn(VerifiedHead, Envelope<S>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Reply<S>, SynapseError>> + Send + 'static,
{
    async fn run(
        &self,
        axon: Arc<Inner>,
        verified: VerifiedHead,
        request: Request,
        start: Instant,
    ) -> Response {
        let head = verified.head().clone();
        let json_type = json_content_type(request.headers());
        let declared = request
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let limit = axon.config.body_limit;
        if declared.is_some_and(|length| length > limit as u64) {
            return too_large(&head, start);
        }
        let body = match Limited::new(request.into_body(), limit).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) if error.downcast_ref::<LengthLimitError>().is_some() => {
                return too_large(&head, start);
            }
            Err(_) => return error_response(500, None, Some(&head), start),
        };
        // FastAPI skips JSON decoding of an empty body; the integrity
        // dependency's `json.loads("")` then fails with 500. FastAPI 0.143
        // passes a body without a JSON content type (or with none) to the
        // model as bytes, whose validator raises: also 500.
        if body.is_empty() || !json_type {
            return error_response(500, None, Some(&head), start);
        }
        // Large bodies are decoded and hashed off the async workers.
        let decoded = if body.len() > INLINE_DECODE_BYTES {
            match tokio::task::spawn_blocking(move || decode::<S>(&body)).await {
                Ok(decoded) => decoded,
                Err(_) => return error_response(500, None, Some(&head), start),
            }
        } else {
            decode::<S>(&body)
        };
        let mut envelope = match decoded {
            Decoded::Invalid(error) => return json_invalid(&error),
            Decoded::Unusable => return error_response(500, None, Some(&head), start),
            Decoded::Valid(envelope, hash)
                if hash == head.computed_body_hash.as_deref().unwrap_or("") =>
            {
                *envelope
            }
            Decoded::Valid(..) => return error_response(500, None, Some(&head), start),
        };
        envelope.dendrite = Some(head.dendrite.clone());
        // The dendrite's own view of this axon (ip, port, hotkey), without
        // the axon-side status, version, nonce, uuid or signature: Python's
        // success headers echo exactly these body fields.
        let mut axon_info = head.axon.clone();
        axon_info.status_code = None;
        axon_info.status_message = None;
        axon_info.process_time = None;
        axon_info.version = None;
        axon_info.nonce = None;
        axon_info.uuid = None;
        axon_info.signature = None;
        envelope.axon = Some(axon_info);

        let forward = (self.forward)(verified, envelope);
        let outcome = match axon.config.handler_timeout {
            Some(limit) => match tokio::time::timeout(limit, forward).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    let timeout = head.timeout.map_or_else(|| "None".to_owned(), float_repr);
                    let message = format!("Response timeout after: {timeout}s");
                    return error_response(408, Some(message), Some(&head), start);
                }
            },
            None => forward.await,
        };
        match outcome {
            Ok(Reply::Synapse(envelope)) => synapse_response(envelope, &head, start),
            Ok(Reply::Stream {
                head: envelope,
                body,
            }) => stream_response(envelope, body, &head, start),
            Err(error) => error_response(
                error.status.unwrap_or(500),
                error.message,
                Some(&head),
                start,
            ),
        }
    }
}

/// Bodies up to this size are decoded on the async worker.
const INLINE_DECODE_BYTES: usize = 64 * 1024;

enum Decoded<S> {
    /// Not JSON, even with Python's `NaN`/`Infinity` extensions: FastAPI's 422.
    Invalid(serde_json::Error),
    /// JSON that does not validate or hash as this synapse: 500.
    Unusable,
    /// The envelope and its body hash.
    Valid(Box<Envelope<S>>, String),
}

fn decode<S: Synapse>(body: &[u8]) -> Decoded<S> {
    if let Err(error) = crate::pyjson::validate(body) {
        return Decoded::Invalid(error);
    }
    let Ok(envelope) = Envelope::<S>::from_json_slice(body) else {
        return Decoded::Unusable;
    };
    match envelope.body_hash() {
        Ok(hash) => Decoded::Valid(Box::new(envelope), hash),
        Err(_) => Decoded::Unusable,
    }
}

fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(CONTENT_TYPE) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let essence = text
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence == "application/json"
        || (essence.starts_with("application/") && essence.ends_with("+json"))
}

/// Python's `synapse_to_response` on the forward's envelope.
fn finish_axon<S>(envelope: &mut Envelope<S>, start: Instant) -> u16 {
    let axon = envelope.axon.get_or_insert_with(Default::default);
    let status = *axon.status_code.get_or_insert(200);
    if status == 200 && axon.status_message.as_deref().is_none_or(str::is_empty) {
        axon.status_message = Some("Success".into());
    }
    axon.process_time = Some(start.elapsed().as_secs_f64());
    u16::try_from(status).unwrap_or(0)
}

fn synapse_response<S: Synapse>(
    mut envelope: Envelope<S>,
    head: &RequestHead,
    start: Instant,
) -> Response {
    let status = finish_axon(&mut envelope, start);
    let Ok(status) = StatusCode::from_u16(status) else {
        return error_response(500, None, Some(head), start);
    };
    let Ok(body) = to_starlette_json(&envelope) else {
        return error_response(500, None, Some(head), start);
    };
    envelope.total_size = i64::try_from(body.len()).ok();
    let headers = match envelope.to_headers() {
        Ok(headers) => headers,
        Err(_) => return error_response(500, None, Some(head), start),
    };
    build(status, headers, "application/json", Body::from(body))
}

fn stream_response<S: Synapse>(
    mut envelope: Envelope<S>,
    body: BoxStream<'static, Bytes>,
    head: &RequestHead,
    start: Instant,
) -> Response {
    finish_axon(&mut envelope, start);
    let headers = match envelope.to_headers() {
        Ok(headers) => headers,
        Err(_) => return error_response(500, None, Some(head), start),
    };
    let body = Body::from_stream(body.map(Ok::<Bytes, Infallible>));
    build(StatusCode::OK, headers, "text/event-stream", body)
}

fn build(
    status: StatusCode,
    headers: HeaderMap,
    content_type: &'static str,
    body: Body,
) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let target = response.headers_mut();
    target.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    for (name, value) in headers {
        if let Some(name) = name {
            target.append(name, value);
        }
    }
    response
}

fn internal_message() -> String {
    format!("Internal Server Error #{}", uuid4_string())
}

/// Python's `create_error_response`: `{"message": …}` with the header
/// synapse's headers, or a fresh `Synapse()`'s when headers did not parse.
fn error_response(
    status: u16,
    message: Option<String>,
    head: Option<&RequestHead>,
    start: Instant,
) -> Response {
    let message = Some(message.unwrap_or_else(internal_message));
    let elapsed = start.elapsed().as_secs_f64();
    let pairs = match head {
        Some(head) => {
            let mut head = head.clone();
            head.axon.status_code = Some(i64::from(status));
            head.axon.status_message.clone_from(&message);
            head.axon.process_time = Some(elapsed);
            Ok(head.unchecked_header_pairs())
        }
        None => {
            let mut envelope = Envelope::new(BaseSynapse {});
            if let Some(axon) = envelope.axon.as_mut() {
                axon.status_code = Some(i64::from(status));
                axon.status_message.clone_from(&message);
                axon.process_time = Some(elapsed);
            }
            envelope.unchecked_header_pairs()
        }
    };
    // Starlette encodes header values as Latin-1, so reflected request text
    // (decoded as Latin-1) goes back byte for byte. A value it cannot encode
    // makes Python's error handler itself fail: Starlette then answers a
    // plain-text 500.
    let Some(headers) = pairs.ok().and_then(|pairs| latin1_headers(&pairs)) else {
        return plain_internal_error();
    };
    let body = to_starlette_json(&serde_json::json!({ "message": message })).unwrap_or_default();
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    build(status, headers, "application/json", Body::from(body))
}

fn latin1_headers(pairs: &[(String, String)]) -> Option<HeaderMap> {
    let mut headers = HeaderMap::with_capacity(pairs.len());
    for (name, value) in pairs {
        let name = http::HeaderName::from_bytes(name.as_bytes()).ok()?;
        let bytes = value
            .chars()
            .map(|character| u8::try_from(u32::from(character)).ok())
            .collect::<Option<Vec<u8>>>()?;
        headers.append(name, HeaderValue::from_bytes(&bytes).ok()?);
    }
    Some(headers)
}

/// Starlette's `ServerErrorMiddleware` response.
fn plain_internal_error() -> Response {
    let mut response = Response::new(Body::from("Internal Server Error"));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn too_large(head: &RequestHead, start: Instant) -> Response {
    error_response(413, Some("Payload Too Large".into()), Some(head), start)
}

fn detail_response(status: StatusCode, detail: Value) -> Response {
    let body = to_starlette_json(&serde_json::json!({ "detail": detail })).unwrap_or_default();
    build(
        status,
        HeaderMap::new(),
        "application/json",
        Body::from(body),
    )
}

/// FastAPI's 422 for an undecodable JSON body (not byte-exact).
fn json_invalid(error: &serde_json::Error) -> Response {
    detail_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        serde_json::json!([{
            "type": "json_invalid",
            "loc": ["body", error.column()],
            "msg": "JSON decode error",
            "input": {},
            "ctx": { "error": error.to_string() },
        }]),
    )
}
