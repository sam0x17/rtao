//! A miner-side endpoint that serves btlightning validators.
//!
//! Admission at each incoming QUIC connection runs in btlightning's order:
//! source policy (silently ignored), address validation (QUIC Retry), the
//! authenticated-validator limit, then rtao's pre-authentication caps. A
//! connection that has not completed a handshake within the handshake
//! timeout is closed.
//!
//! Per-source limits (the handshake rate limit and the pre-authentication
//! cap) key IPv6 sources by their /64 prefix; see [`source_key`].

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quinn::{Connection, Endpoint, Incoming, RecvStream, SendStream};
use rtao_btauth::{InMemoryNonceStore, NonceStore, NonceStoreConfig};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::Signer;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::QuicError;
use crate::auth::{
    HandshakeReject, HandshakeVerifier, fingerprint_b64, sign_handshake_response, signer_ss58,
};
use crate::data::{from_data, to_data};
use crate::tls::{ServerIdentity, server_config, transport};
use crate::wire::{
    DEFAULT_MAX_FRAME_PAYLOAD, DEFAULT_MAX_SIGNATURE_AGE_SECS, DataMap, HandshakeRequest,
    HandshakeResponse, MAX_MAX_SIGNATURE_AGE_SECS, MIN_MAX_FRAME_PAYLOAD, MessageType, StreamChunk,
    StreamEnd, SynapsePacket, SynapseResponse, decode, encode, encode_frame, errors, read_header,
    read_payload, unix_now, write_budget, write_frame,
};

/// A sendable boxed future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Extra time a stream chunk already being written may take past the stream
/// deadline before the stream is reset instead.
const FRAME_WRITE_GRACE: Duration = Duration::from_secs(5);
const RATE_WINDOW_SECS: u64 = 60;
/// Smallest configurable pre-authentication frame payload.
const MIN_PREAUTH_FRAME_PAYLOAD: u32 = 1024;

/// Server limits and timeouts, defaulting to btlightning's values.
#[derive(Clone, Debug)]
pub struct QuicServerConfig {
    /// Maximum handshake signature age in seconds (300, at most 3600).
    pub max_signature_age_secs: u64,
    /// QUIC idle timeout (150 s).
    pub idle_timeout: Duration,
    /// QUIC keep-alive interval (30 s).
    pub keep_alive: Duration,
    /// Most authenticated validators at once (128).
    pub max_connections: usize,
    /// Most retained handshake nonces (100,000). Full stores reject.
    pub max_nonce_entries: usize,
    /// Most retained nonces of one validator (4,096).
    pub max_nonces_per_validator: usize,
    /// Bound on handshake processing and on reaching authentication (10 s).
    pub handshake_timeout: Duration,
    /// Handshakes accepted per source per minute (30), keyed by
    /// [`source_key`].
    pub max_handshake_attempts_per_minute: u32,
    /// Most sources tracked by the rate limiter (10,000).
    pub max_tracked_rate_ips: usize,
    /// Concurrent bidirectional streams per connection (128).
    pub max_concurrent_bidi_streams: u32,
    /// Bound on a unary handler or a whole streamed reply (30 s).
    pub handler_timeout: Duration,
    /// Largest accepted frame payload (64 MiB, at least 1 MiB).
    pub max_frame_payload: u32,
    /// Chunks buffered between a streaming handler and the writer (32).
    pub streaming_channel_buffer: usize,
    /// Send QUIC Retry to unvalidated addresses (true).
    pub require_address_validation: bool,
    /// Most unauthenticated connections at once (256). rtao addition.
    ///
    /// When it is reached, a new connection is admitted by closing the
    /// oldest unauthenticated connection of the source holding the most, so
    /// a few sources cannot keep validators out by filling every slot.
    pub max_preauth_connections: usize,
    /// Most unauthenticated connections per source (16), keyed by
    /// [`source_key`]. Further connections from that source are refused.
    /// rtao addition.
    pub max_preauth_per_ip: usize,
    /// Largest frame payload read before the connection authenticates
    /// (64 KiB, at least 1 KiB). A larger first frame on an unauthenticated
    /// connection is answered without reading its payload: a `SynapsePacket`
    /// with `"authentication failed"`, a `HandshakeRequest` with a rejection.
    /// rtao addition.
    pub max_preauth_frame_payload: u32,
}

impl Default for QuicServerConfig {
    fn default() -> Self {
        Self {
            max_signature_age_secs: DEFAULT_MAX_SIGNATURE_AGE_SECS,
            idle_timeout: Duration::from_secs(150),
            keep_alive: Duration::from_secs(30),
            max_connections: 128,
            max_nonce_entries: 100_000,
            max_nonces_per_validator: 4_096,
            handshake_timeout: Duration::from_secs(10),
            max_handshake_attempts_per_minute: 30,
            max_tracked_rate_ips: 10_000,
            max_concurrent_bidi_streams: 128,
            handler_timeout: Duration::from_secs(30),
            max_frame_payload: DEFAULT_MAX_FRAME_PAYLOAD,
            streaming_channel_buffer: 32,
            require_address_validation: true,
            max_preauth_connections: 256,
            max_preauth_per_ip: 16,
            max_preauth_frame_payload: 64 * 1024,
        }
    }
}

impl QuicServerConfig {
    /// Check the configuration's invariants.
    pub fn validate(&self) -> Result<(), QuicError> {
        let fail = |m: &str| Err(QuicError::Config(m.to_owned()));
        if self.max_signature_age_secs == 0
            || self.max_signature_age_secs > MAX_MAX_SIGNATURE_AGE_SECS
        {
            return fail("max_signature_age_secs must be in 1..=3600");
        }
        if self.idle_timeout.is_zero() || self.keep_alive.is_zero() {
            return fail("idle_timeout and keep_alive must be non-zero");
        }
        if self.keep_alive >= self.idle_timeout {
            return fail("keep_alive must be less than idle_timeout");
        }
        if self.handshake_timeout.is_zero() || self.handshake_timeout >= self.idle_timeout {
            return fail("handshake_timeout must be non-zero and less than idle_timeout");
        }
        if self.handler_timeout.is_zero() || self.handler_timeout >= self.idle_timeout {
            return fail("handler_timeout must be non-zero and less than idle_timeout");
        }
        if self.max_frame_payload < MIN_MAX_FRAME_PAYLOAD {
            return fail("max_frame_payload must be at least 1 MiB");
        }
        if self.max_preauth_frame_payload < MIN_PREAUTH_FRAME_PAYLOAD
            || self.max_preauth_frame_payload > self.max_frame_payload
        {
            return fail("max_preauth_frame_payload must be in 1 KiB..=max_frame_payload");
        }
        for (value, name) in [
            (self.max_connections, "max_connections"),
            (self.max_nonce_entries, "max_nonce_entries"),
            (self.max_nonces_per_validator, "max_nonces_per_validator"),
            (self.max_tracked_rate_ips, "max_tracked_rate_ips"),
            (self.streaming_channel_buffer, "streaming_channel_buffer"),
            (self.max_preauth_connections, "max_preauth_connections"),
            (self.max_preauth_per_ip, "max_preauth_per_ip"),
        ] {
            if value == 0 {
                return Err(QuicError::Config(format!("{name} must be non-zero")));
            }
        }
        if self.max_handshake_attempts_per_minute == 0 || self.max_concurrent_bidi_streams == 0 {
            return fail("rate and stream limits must be non-zero");
        }
        Ok(())
    }
}

/// The authenticated peer of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caller {
    /// The validator hotkey proven by the connection's handshake.
    pub hotkey: AccountId,
    /// The connection's current remote address.
    pub remote: SocketAddr,
}

/// A handler failure. Its text is logged, never sent to the peer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HandlerError(pub String);

impl HandlerError {
    /// A handler error from any displayable value.
    pub fn new(message: impl std::fmt::Display) -> Self {
        Self(message.to_string())
    }
}

/// A unary request handler.
pub trait SynapseHandler: Send + Sync + 'static {
    /// Handle one request.
    fn handle(
        &self,
        caller: Caller,
        synapse_type: String,
        data: DataMap,
    ) -> BoxFuture<Result<DataMap, HandlerError>>;
}

/// The sending half given to a streaming handler. Each item becomes one
/// `StreamChunk` frame.
pub type ChunkSender = mpsc::Sender<Vec<u8>>;

/// A streaming request handler.
pub trait StreamingHandler: Send + Sync + 'static {
    /// Handle one request, sending chunks until done.
    fn handle(
        &self,
        caller: Caller,
        synapse_type: String,
        data: DataMap,
        chunks: ChunkSender,
    ) -> BoxFuture<Result<(), HandlerError>>;
}

struct FnUnary<F>(F);

impl<F, Fut> SynapseHandler for FnUnary<F>
where
    F: Fn(Caller, DataMap) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<DataMap, HandlerError>> + Send + 'static,
{
    fn handle(
        &self,
        caller: Caller,
        _: String,
        data: DataMap,
    ) -> BoxFuture<Result<DataMap, HandlerError>> {
        Box::pin((self.0)(caller, data))
    }
}

struct FnStreaming<F>(F);

impl<F, Fut> StreamingHandler for FnStreaming<F>
where
    F: Fn(Caller, DataMap, ChunkSender) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
{
    fn handle(
        &self,
        caller: Caller,
        _: String,
        data: DataMap,
        chunks: ChunkSender,
    ) -> BoxFuture<Result<(), HandlerError>> {
        Box::pin((self.0)(caller, data, chunks))
    }
}

/// Handlers by synapse type. Streaming handlers take precedence.
#[derive(Clone, Default)]
pub struct QuicRouter {
    unary: HashMap<String, Arc<dyn SynapseHandler>>,
    streaming: HashMap<String, Arc<dyn StreamingHandler>>,
}

impl QuicRouter {
    /// An empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a unary handler.
    pub fn unary(mut self, synapse_type: &str, handler: impl SynapseHandler) -> Self {
        self.unary
            .insert(synapse_type.to_owned(), Arc::new(handler));
        self
    }

    /// Register a unary handler closure.
    pub fn unary_fn<F, Fut>(self, synapse_type: &str, handler: F) -> Self
    where
        F: Fn(Caller, DataMap) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<DataMap, HandlerError>> + Send + 'static,
    {
        self.unary(synapse_type, FnUnary(handler))
    }

    /// Register a typed unary handler; requests and replies use
    /// [`crate::data`]'s encoding.
    pub fn typed<Q, R, F, Fut>(self, synapse_type: &str, handler: F) -> Self
    where
        Q: DeserializeOwned + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn(Caller, Q) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, HandlerError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        self.unary_fn(synapse_type, move |caller, data| {
            let handler = handler.clone();
            async move {
                let request: Q = from_data(data).map_err(HandlerError::new)?;
                let reply = handler(caller, request).await?;
                to_data(&reply).map_err(HandlerError::new)
            }
        })
    }

    /// Register a streaming handler.
    pub fn streaming(mut self, synapse_type: &str, handler: impl StreamingHandler) -> Self {
        self.streaming
            .insert(synapse_type.to_owned(), Arc::new(handler));
        self
    }

    /// Register a streaming handler closure.
    pub fn streaming_fn<F, Fut>(self, synapse_type: &str, handler: F) -> Self
    where
        F: Fn(Caller, DataMap, ChunkSender) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
    {
        self.streaming(synapse_type, FnStreaming(handler))
    }
}

/// Which validators may complete a handshake (btlightning's permit gate).
pub trait HotkeyPolicy: Send + Sync + 'static {
    /// Whether `hotkey` may authenticate. Called after its signature
    /// verifies and before its nonce is stored.
    fn admit(&self, hotkey: &AccountId) -> bool;
}

impl<F: Fn(&AccountId) -> bool + Send + Sync + 'static> HotkeyPolicy for F {
    fn admit(&self, hotkey: &AccountId) -> bool {
        self(hotkey)
    }
}

/// Which source IPs may open a connection (btlightning's allowlist).
/// Refused sources are ignored without a reply.
pub trait SourcePolicy: Send + Sync + 'static {
    /// Whether `ip` may connect.
    fn allows(&self, ip: IpAddr) -> bool;
}

impl<F: Fn(IpAddr) -> bool + Send + Sync + 'static> SourcePolicy for F {
    fn allows(&self, ip: IpAddr) -> bool {
        self(ip)
    }
}

/// Configures and binds a [`QuicServer`].
pub struct QuicServerBuilder {
    signer: Arc<dyn Signer>,
    router: QuicRouter,
    config: QuicServerConfig,
    policy: Option<Arc<dyn HotkeyPolicy>>,
    source_policy: Option<Arc<dyn SourcePolicy>>,
    nonce_store: Option<Arc<dyn NonceStore>>,
    identity: Option<ServerIdentity>,
}

impl QuicServerBuilder {
    /// Replace the configuration.
    pub fn config(mut self, config: QuicServerConfig) -> Self {
        self.config = config;
        self
    }

    /// Admit only validators accepted by `policy`.
    pub fn policy(mut self, policy: impl HotkeyPolicy) -> Self {
        self.policy = Some(Arc::new(policy));
        self
    }

    /// Accept connections only from sources allowed by `policy`.
    pub fn source_policy(mut self, policy: impl SourcePolicy) -> Self {
        self.source_policy = Some(Arc::new(policy));
        self
    }

    /// Use a shared nonce store. Its retention must cover the signature age
    /// plus 60 seconds.
    pub fn nonce_store(mut self, store: Arc<dyn NonceStore>) -> Self {
        self.nonce_store = Some(store);
        self
    }

    /// Serve a persisted certificate instead of generating one. Peers cannot
    /// tell the difference; the fingerprint stays stable across restarts.
    pub fn identity(mut self, identity: ServerIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Bind the UDP socket. Must be called inside a tokio runtime.
    pub fn bind(self, addr: SocketAddr) -> Result<QuicServer, QuicError> {
        let config = self.config;
        config.validate()?;
        let crypto = self.signer.crypto_type();
        if crypto != CryptoType::Sr25519 {
            return Err(QuicError::Config(format!(
                "the miner signer must be sr25519, not {crypto:?}"
            )));
        }
        if self.policy.is_none() {
            warn!(
                "QUIC server has no hotkey policy: any sr25519 key can authenticate and \
                 consume validator and nonce-store capacity"
            );
        }
        let store = match self.nonce_store {
            Some(store) => store,
            None => Arc::new(
                InMemoryNonceStore::with_config(NonceStoreConfig {
                    retention: HandshakeVerifier::recommended_retention(
                        config.max_signature_age_secs,
                    ),
                    max_per_hotkey: config.max_nonces_per_validator,
                    max_entries: config.max_nonce_entries,
                })
                .map_err(|e| QuicError::Config(e.to_string()))?,
            ),
        };
        let verifier = HandshakeVerifier::new(config.max_signature_age_secs, store)
            .map_err(QuicError::Config)?;
        let identity = match self.identity {
            Some(identity) => identity,
            None => ServerIdentity::generate()?,
        };
        let quinn_config = server_config(
            &identity,
            transport(config.idle_timeout, config.keep_alive)?,
            config.max_concurrent_bidi_streams,
        )?;
        let endpoint = Endpoint::server(quinn_config, addr)
            .map_err(|e| QuicError::Connection(e.to_string()))?;
        let ctx = Arc::new(Ctx {
            miner_ss58: signer_ss58(self.signer.as_ref()),
            signer: self.signer,
            fingerprint: identity.fingerprint,
            fp_b64: fingerprint_b64(&identity.fingerprint),
            router: self.router,
            verifier,
            policy: self.policy,
            source_policy: self.source_policy,
            rate: Mutex::new(HashMap::new()),
            state: Mutex::new(State::default()),
            counters: Counters::default(),
            config,
        });
        Ok(QuicServer { endpoint, ctx })
    }
}

/// The IP address per-source limits count against: an IPv4 address itself,
/// or the /64 prefix of an IPv6 address. IPv4-mapped IPv6 addresses count
/// as IPv4.
///
/// A single IPv6 host usually controls a whole /64, so keying by full
/// address would let it multiply its handshake and pre-authentication
/// budgets. btlightning keys by full address.
pub fn source_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        },
    }
}

/// An unauthenticated connection counted against the pre-authentication caps.
struct Preauth {
    source: IpAddr,
    /// Cancelled on eviction, ending a QUIC handshake still in progress.
    evict: CancellationToken,
    /// Set once the QUIC handshake completes.
    connection: Option<Connection>,
}

impl Preauth {
    fn close(self) {
        self.evict.cancel();
        if let Some(connection) = self.connection {
            connection.close(0u32.into(), b"preauth_evicted");
        }
    }
}

#[derive(Default)]
struct State {
    validators: HashMap<AccountId, (usize, Connection)>,
    /// Keyed by admission order, so the first entry is the oldest.
    preauth: BTreeMap<u64, Preauth>,
    preauth_by_source: HashMap<IpAddr, usize>,
    next_preauth: u64,
}

impl State {
    fn release_preauth(&mut self, seq: u64) -> Option<Preauth> {
        let entry = self.preauth.remove(&seq)?;
        if let Some(n) = self.preauth_by_source.get_mut(&entry.source) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.preauth_by_source.remove(&entry.source);
            }
        }
        Some(entry)
    }

    /// Remove the oldest pre-authentication entry of the source that holds
    /// the most, returning it for closing.
    fn evict_preauth(&mut self) -> Option<Preauth> {
        let (&source, _) = self.preauth_by_source.iter().max_by_key(|(_, n)| **n)?;
        let seq = self
            .preauth
            .iter()
            .find(|(_, e)| e.source == source)
            .map(|(seq, _)| *seq)?;
        self.release_preauth(seq)
    }

    fn is_authenticated_addr(&self, remote: SocketAddr) -> bool {
        self.validators
            .values()
            .any(|(_, c)| c.remote_address() == remote)
    }
}

#[derive(Default)]
struct Counters {
    nonce_store_rejections: AtomicU64,
    preauth_evictions: AtomicU64,
    preauth_refusals: AtomicU64,
}

struct Ctx {
    config: QuicServerConfig,
    signer: Arc<dyn Signer>,
    miner_ss58: String,
    fingerprint: [u8; 32],
    fp_b64: String,
    router: QuicRouter,
    verifier: HandshakeVerifier,
    policy: Option<Arc<dyn HotkeyPolicy>>,
    source_policy: Option<Arc<dyn SourcePolicy>>,
    rate: Mutex<HashMap<IpAddr, Vec<u64>>>,
    state: Mutex<State>,
    counters: Counters,
}

struct ConnState {
    id: usize,
    /// This connection's key in [`State::preauth`].
    preauth: u64,
    identity: Mutex<Option<AccountId>>,
    authenticated: tokio::sync::Notify,
}

impl ConnState {
    fn identity(&self) -> Option<AccountId> {
        *self.identity.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Ctx {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn release_preauth(&self, seq: u64) {
        let _ = self.state().release_preauth(seq);
    }

    /// btlightning's limiter: attempts at or after `now - 60` count; only
    /// admitted attempts are recorded. Sources are keyed by [`source_key`].
    fn check_rate(&self, ip: IpAddr, now: u64) -> bool {
        let ip = source_key(ip);
        let mut rates = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        if !rates.contains_key(&ip) && rates.len() >= self.config.max_tracked_rate_ips {
            let oldest = rates
                .iter()
                .min_by_key(|(_, a)| a.iter().copied().max().unwrap_or(0))
                .map(|(ip, _)| *ip);
            if let Some(oldest) = oldest {
                rates.remove(&oldest);
            }
        }
        let cutoff = now.saturating_sub(RATE_WINDOW_SECS);
        let attempts = rates.entry(ip).or_default();
        attempts.retain(|ts| *ts >= cutoff);
        if attempts.len() >= self.config.max_handshake_attempts_per_minute as usize {
            return false;
        }
        attempts.push(now);
        true
    }

    fn sweep_rates(&self, now: u64) {
        let cutoff = now.saturating_sub(RATE_WINDOW_SECS);
        let mut rates = self.rate.lock().unwrap_or_else(|e| e.into_inner());
        rates.retain(|_, attempts| {
            attempts.retain(|ts| *ts >= cutoff);
            !attempts.is_empty()
        });
    }
}

/// Live counters of a server. They stay readable after
/// [`QuicServer::serve`] takes the server.
#[derive(Clone)]
pub struct ServerStats {
    ctx: Arc<Ctx>,
}

impl std::fmt::Debug for ServerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerStats")
            .field("authenticated_validators", &self.authenticated_validators())
            .field("preauth_connections", &self.preauth_connections())
            .field("nonce_store_rejections", &self.nonce_store_rejections())
            .field("preauth_evictions", &self.preauth_evictions())
            .field("preauth_refusals", &self.preauth_refusals())
            .finish()
    }
}

impl ServerStats {
    /// Validators with an authenticated connection.
    pub fn authenticated_validators(&self) -> usize {
        self.ctx.state().validators.len()
    }

    /// Connections that have not completed a handshake.
    pub fn preauth_connections(&self) -> usize {
        self.ctx.state().preauth.len()
    }

    /// Verified handshakes rejected because the nonce store was full or
    /// unavailable. A rising count means the store is under pressure; a
    /// hotkey policy stops unknown keys from consuming it.
    pub fn nonce_store_rejections(&self) -> u64 {
        self.ctx
            .counters
            .nonce_store_rejections
            .load(Ordering::Relaxed)
    }

    /// Unauthenticated connections closed to admit a newer one.
    pub fn preauth_evictions(&self) -> u64 {
        self.ctx.counters.preauth_evictions.load(Ordering::Relaxed)
    }

    /// Connections refused by the pre-authentication caps.
    pub fn preauth_refusals(&self) -> u64 {
        self.ctx.counters.preauth_refusals.load(Ordering::Relaxed)
    }
}

/// A bound miner endpoint.
pub struct QuicServer {
    endpoint: Endpoint,
    ctx: Arc<Ctx>,
}

impl QuicServer {
    /// Start configuring a server that signs with `signer` (sr25519, raw
    /// signing) and dispatches with `router`.
    pub fn builder(signer: Arc<dyn Signer>, router: QuicRouter) -> QuicServerBuilder {
        QuicServerBuilder {
            signer,
            router,
            config: QuicServerConfig::default(),
            policy: None,
            source_policy: None,
            nonce_store: None,
            identity: None,
        }
    }

    /// Bind with the default policy and a generated certificate.
    pub fn bind(
        addr: SocketAddr,
        signer: Arc<dyn Signer>,
        router: QuicRouter,
        config: QuicServerConfig,
    ) -> Result<Self, QuicError> {
        Self::builder(signer, router).config(config).bind(addr)
    }

    /// The bound UDP address.
    pub fn local_addr(&self) -> Result<SocketAddr, QuicError> {
        self.endpoint
            .local_addr()
            .map_err(|e| QuicError::Connection(e.to_string()))
    }

    /// The certificate fingerprint validators sign.
    pub fn fingerprint(&self) -> [u8; 32] {
        self.ctx.fingerprint
    }

    /// The SS58 address the server answers as.
    pub fn miner_hotkey(&self) -> &str {
        &self.ctx.miner_ss58
    }

    /// The number of authenticated validators.
    pub fn authenticated_validators(&self) -> usize {
        self.ctx.state().validators.len()
    }

    /// A handle to the server's live counters.
    pub fn stats(&self) -> ServerStats {
        ServerStats {
            ctx: self.ctx.clone(),
        }
    }

    /// Accept connections until `shutdown` is cancelled.
    pub async fn serve(self, shutdown: CancellationToken) -> Result<(), QuicError> {
        let mut sweep = tokio::time::interval(Duration::from_secs(RATE_WINDOW_SECS));
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = sweep.tick() => self.ctx.sweep_rates(unix_now()),
                incoming = self.endpoint.accept() => match incoming {
                    Some(incoming) => self.admit(incoming, &shutdown),
                    None => break,
                },
            }
        }
        self.endpoint.close(0u32.into(), b"server_shutdown");
        let _ = timeout(Duration::from_secs(2), self.endpoint.wait_idle()).await;
        Ok(())
    }

    fn admit(&self, incoming: Incoming, shutdown: &CancellationToken) {
        let ctx = &self.ctx;
        let remote = incoming.remote_address();
        if let Some(policy) = &ctx.source_policy
            && !policy.allows(remote.ip())
        {
            incoming.ignore();
            return;
        }
        if ctx.config.require_address_validation
            && !incoming.remote_address_validated()
            && incoming.may_retry()
        {
            if let Err(e) = incoming.retry() {
                e.into_incoming().ignore();
            }
            return;
        }
        let source = source_key(remote.ip());
        let (seq, evict, evicted) = {
            let mut state = ctx.state();
            if state.validators.len() >= ctx.config.max_connections
                && !state.is_authenticated_addr(remote)
            {
                drop(state);
                warn!(%remote, "validator limit reached; refusing connection");
                incoming.refuse();
                return;
            }
            let per_source = state.preauth_by_source.get(&source).copied().unwrap_or(0);
            let mut evicted = None;
            if per_source < ctx.config.max_preauth_per_ip
                && state.preauth.len() >= ctx.config.max_preauth_connections
            {
                evicted = state.evict_preauth();
            }
            if per_source >= ctx.config.max_preauth_per_ip
                || state.preauth.len() >= ctx.config.max_preauth_connections
            {
                drop(state);
                ctx.counters
                    .preauth_refusals
                    .fetch_add(1, Ordering::Relaxed);
                warn!(%remote, "pre-authentication limit reached; refusing connection");
                incoming.refuse();
                return;
            }
            let seq = state.next_preauth;
            state.next_preauth += 1;
            let evict = CancellationToken::new();
            state.preauth.insert(
                seq,
                Preauth {
                    source,
                    evict: evict.clone(),
                    connection: None,
                },
            );
            *state.preauth_by_source.entry(source).or_insert(0) += 1;
            (seq, evict, evicted)
        };
        if let Some(evicted) = evicted {
            ctx.counters
                .preauth_evictions
                .fetch_add(1, Ordering::Relaxed);
            debug!(source = %evicted.source, "evicting oldest pre-authentication connection");
            evicted.close();
        }
        let ctx = ctx.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let connecting = match incoming.accept() {
                Ok(connecting) => connecting,
                Err(e) => {
                    debug!(%remote, error = %e, "incoming connection failed");
                    ctx.release_preauth(seq);
                    return;
                }
            };
            // The QUIC handshake counts against the same deadline as the
            // validator handshake; dropping `connecting` abandons it.
            let connection = tokio::select! {
                _ = evict.cancelled() => return,
                r = timeout(ctx.config.handshake_timeout, connecting) => match r {
                    Ok(r) => r.map_err(|e| e.to_string()),
                    Err(_) => Err("QUIC handshake timed out".to_owned()),
                },
            };
            match connection {
                Ok(connection) => {
                    let still_preauth = match ctx.state().preauth.get_mut(&seq) {
                        Some(entry) => {
                            entry.connection = Some(connection.clone());
                            true
                        }
                        None => false,
                    };
                    if !still_preauth {
                        connection.close(0u32.into(), b"preauth_evicted");
                        return;
                    }
                    let conn = Arc::new(ConnState {
                        id: connection.stable_id(),
                        preauth: seq,
                        identity: Mutex::new(None),
                        authenticated: tokio::sync::Notify::new(),
                    });
                    handle_connection(connection, conn, ctx, shutdown).await;
                }
                Err(e) => {
                    debug!(%remote, error = %e, "incoming connection failed");
                    ctx.release_preauth(seq);
                }
            }
        });
    }
}

async fn handle_connection(
    connection: Connection,
    conn: Arc<ConnState>,
    ctx: Arc<Ctx>,
    shutdown: CancellationToken,
) {
    let deadline = {
        let connection = connection.clone();
        let conn = conn.clone();
        let limit = ctx.config.handshake_timeout;
        tokio::spawn(async move {
            tokio::select! {
                _ = conn.authenticated.notified() => {}
                _ = connection.closed() => {}
                _ = tokio::time::sleep(limit) => {
                    if conn.identity().is_none() {
                        connection.close(0u32.into(), b"handshake_timeout");
                    }
                }
            }
        })
    };
    loop {
        let accepted = tokio::select! {
            _ = shutdown.cancelled() => break,
            accepted = connection.accept_bi() => accepted,
        };
        match accepted {
            Ok((send, recv)) => {
                let connection = connection.clone();
                let conn = conn.clone();
                let ctx = ctx.clone();
                tokio::spawn(async move { handle_stream(send, recv, connection, conn, ctx).await });
            }
            Err(e) => {
                debug!(remote = %connection.remote_address(), error = %e, "connection ended");
                break;
            }
        }
    }
    deadline.abort();
    // Close before cleaning up: a handshake that registers afterwards sees
    // the close and refuses to register.
    connection.close(0u32.into(), b"done");
    let mut state = ctx.state();
    let _ = state.release_preauth(conn.preauth);
    // Every entry this connection registered, not only its last identity.
    state.validators.retain(|_, (id, _)| *id != conn.id);
}

/// Write one reply frame and finish the stream. A peer that withholds flow
/// control gets the stream reset after the write budget for its size.
async fn send_frame(send: &mut SendStream, kind: MessageType, bytes: &[u8]) {
    match timeout(write_budget(bytes.len()), write_frame(send, kind, bytes)).await {
        Ok(Ok(())) => {
            let _ = send.finish();
        }
        Ok(Err(e)) => debug!(error = %e, "reply write failed"),
        Err(_) => {
            debug!(len = bytes.len(), "reply write timed out; resetting stream");
            let _ = send.reset(0u32.into());
        }
    }
}

async fn reply(send: &mut SendStream, kind: MessageType, value: &impl Serialize) {
    match encode(value) {
        Ok(bytes) => send_frame(send, kind, &bytes).await,
        Err(_) => {
            let _ = send.finish();
        }
    }
}

async fn handle_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    connection: Connection,
    conn: Arc<ConnState>,
    ctx: Arc<Ctx>,
) {
    let header = match read_header(&mut recv, ctx.config.max_frame_payload).await {
        Ok(header) => header,
        Err(e) => {
            debug!(error = %e, "unreadable first frame");
            return;
        }
    };
    if header.len > ctx.config.max_preauth_frame_payload && conn.identity().is_none() {
        // Answer as btlightning would after reading the payload, without
        // buffering a large frame for a peer that has not authenticated.
        let _ = recv.stop(0u32.into());
        match header.kind {
            MessageType::SynapsePacket => {
                let response = SynapseResponse::error(errors::AUTH_FAILED, unix_now());
                reply(&mut send, MessageType::SynapseResponse, &response).await;
            }
            MessageType::HandshakeRequest => {
                let _ = ctx.check_rate(connection.remote_address().ip(), unix_now());
                let response = HandshakeResponse::rejected_preauth(unix_now());
                reply(&mut send, MessageType::HandshakeResponse, &response).await;
            }
            _ => {
                let _ = send.finish();
            }
        }
        return;
    }
    let (kind, payload) = match read_payload(&mut recv, header.len).await {
        Ok(payload) => (header.kind, payload),
        Err(e) => {
            debug!(error = %e, "unreadable first frame");
            return;
        }
    };
    match kind {
        MessageType::SynapsePacket => {
            let packet: SynapsePacket = match decode(&payload) {
                Ok(packet) => packet,
                Err(_) => {
                    let response = SynapseResponse::error(errors::INVALID_REQUEST, unix_now());
                    return reply(&mut send, MessageType::SynapseResponse, &response).await;
                }
            };
            drop(payload);
            let caller = conn.identity().map(|hotkey| Caller {
                hotkey,
                remote: connection.remote_address(),
            });
            if let Some(handler) = ctx.router.streaming.get(&packet.synapse_type).cloned() {
                serve_streaming(send, packet, caller, handler, &ctx).await;
            } else {
                let response = match timeout(
                    ctx.config.handler_timeout,
                    serve_unary(packet, caller, &ctx),
                )
                .await
                {
                    Ok(response) => response,
                    Err(_) => SynapseResponse::error(errors::TIMED_OUT, unix_now()),
                };
                match encode(&response) {
                    Ok(bytes) => {
                        drop(response);
                        send_frame(&mut send, MessageType::SynapseResponse, &bytes).await;
                    }
                    Err(_) => {
                        let fallback = SynapseResponse::error(errors::SERIALIZATION, unix_now());
                        reply(&mut send, MessageType::SynapseResponse, &fallback).await;
                    }
                }
            }
        }
        MessageType::HandshakeRequest => {
            let response = handshake(&payload, &connection, &conn, &ctx).await;
            reply(&mut send, MessageType::HandshakeResponse, &response).await;
        }
        other => {
            debug!(?other, "unexpected first frame type");
            let _ = send.finish();
        }
    }
}

async fn handshake(
    payload: &[u8],
    connection: &Connection,
    conn: &ConnState,
    ctx: &Ctx,
) -> HandshakeResponse {
    let remote = connection.remote_address();
    if !ctx.check_rate(remote.ip(), unix_now()) {
        warn!(ip = %remote.ip(), "handshake rate limit exceeded");
        return HandshakeResponse::rejected_preauth(unix_now());
    }
    let request: HandshakeRequest = match decode(payload) {
        Ok(request) => request,
        Err(_) => return HandshakeResponse::rejected_preauth(unix_now()),
    };
    match timeout(
        ctx.config.handshake_timeout,
        verify_and_register(&request, connection, conn, ctx),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(reason)) => {
            warn!(validator = %request.validator_hotkey, %reason, "handshake rejected");
            HandshakeResponse::rejected(&ctx.miner_ss58, unix_now())
        }
        Err(_) => {
            warn!(validator = %request.validator_hotkey, "handshake timed out");
            HandshakeResponse::rejected(&ctx.miner_ss58, unix_now())
        }
    }
}

async fn verify_and_register(
    request: &HandshakeRequest,
    connection: &Connection,
    conn: &ConnState,
    ctx: &Ctx,
) -> Result<HandshakeResponse, String> {
    let current = conn.identity();
    let account = ctx
        .verifier
        .verify(request, &ctx.fp_b64, unix_now(), |hotkey| {
            current.is_none_or(|c| c == *hotkey)
                && ctx.policy.as_ref().is_none_or(|p| p.admit(hotkey))
        })
        .await
        .map_err(|e| {
            if matches!(e, HandshakeReject::Store(_)) {
                ctx.counters
                    .nonce_store_rejections
                    .fetch_add(1, Ordering::Relaxed);
            }
            e.to_string()
        })?;
    let now = unix_now();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let connection_id = format!("conn_{}_{}", request.validator_hotkey, millis);
    let response = sign_handshake_response(
        ctx.signer.as_ref(),
        request,
        &ctx.fp_b64,
        now,
        connection_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    let replaced = {
        // Lock order: state, then identity. The identity is claimed in the
        // same critical section that registers it, so concurrent handshakes
        // on one connection cannot bind two validators to it.
        let mut state = ctx.state();
        let mut identity = conn.identity.lock().unwrap_or_else(|e| e.into_inner());
        if identity.is_some_and(|c| c != account) {
            return Err("connection is already authenticated as another validator".into());
        }
        if connection.close_reason().is_some() {
            return Err("connection closed during the handshake".into());
        }
        let known = state.validators.contains_key(&account);
        if !known && state.validators.len() >= ctx.config.max_connections {
            return Err("validator limit reached".into());
        }
        let previous = state
            .validators
            .insert(account, (conn.id, connection.clone()));
        *identity = Some(account);
        let _ = state.release_preauth(conn.preauth);
        previous.filter(|(id, _)| *id != conn.id)
    };
    if let Some((_, previous)) = replaced {
        previous.close(0u32.into(), b"replaced");
    }
    conn.authenticated.notify_one();
    Ok(response)
}

/// Aborts a spawned handler when dropped, so timeouts stop the work.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve_unary(packet: SynapsePacket, caller: Option<Caller>, ctx: &Ctx) -> SynapseResponse {
    let Some(caller) = caller else {
        return SynapseResponse::error(errors::AUTH_FAILED, unix_now());
    };
    let Some(handler) = ctx.router.unary.get(&packet.synapse_type).cloned() else {
        return SynapseResponse::error(errors::UNRECOGNIZED, unix_now());
    };
    let synapse_type = packet.synapse_type;
    let data = packet.data;
    let mut task = AbortOnDrop(tokio::spawn(async move {
        handler.handle(caller, synapse_type, data).await
    }));
    match (&mut task.0).await {
        Ok(Ok(data)) => SynapseResponse {
            success: true,
            data,
            timestamp: unix_now(),
            error: None,
        },
        Ok(Err(e)) => {
            debug!(error = %e, "handler failed");
            SynapseResponse::error(errors::PROCESSING_FAILED, unix_now())
        }
        Err(e) => {
            warn!(error = %e, "handler panicked or was cancelled");
            SynapseResponse::error(errors::PROCESSING_FAILED, unix_now())
        }
    }
}

async fn serve_streaming(
    mut send: SendStream,
    packet: SynapsePacket,
    caller: Option<Caller>,
    handler: Arc<dyn StreamingHandler>,
    ctx: &Ctx,
) {
    let deadline = Instant::now() + ctx.config.handler_timeout;
    let Some(caller) = caller else {
        return reply(
            &mut send,
            MessageType::StreamEnd,
            &StreamEnd::error(errors::AUTH_FAILED),
        )
        .await;
    };
    let (tx, mut rx) = mpsc::channel(ctx.config.streaming_channel_buffer);
    let synapse_type = packet.synapse_type;
    let data = packet.data;
    let mut task = AbortOnDrop(tokio::spawn(async move {
        handler.handle(caller, synapse_type, data, tx).await
    }));
    let end = loop {
        let chunk = match timeout_at(deadline, rx.recv()).await {
            Err(_) => break StreamEnd::error(errors::TIMED_OUT),
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                break match timeout_at(deadline, &mut task.0).await {
                    Err(_) => StreamEnd::error(errors::TIMED_OUT),
                    Ok(Ok(Ok(()))) => StreamEnd {
                        success: true,
                        error: None,
                    },
                    Ok(Ok(Err(e))) => {
                        debug!(error = %e, "streaming handler failed");
                        StreamEnd::error(errors::STREAM_FAILED)
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "streaming handler panicked or was cancelled");
                        StreamEnd::error(errors::STREAM_FAILED)
                    }
                };
            }
        };
        let frame = match encode(&StreamChunk { data: chunk })
            .and_then(|bytes| encode_frame(MessageType::StreamChunk, &bytes))
        {
            Ok(frame) => frame,
            Err(_) => break StreamEnd::error(errors::STREAM_WRITE_FAILED),
        };
        // A started frame is written to completion (within a grace period)
        // so a StreamEnd is never appended to a partial chunk.
        match timeout_at(deadline + FRAME_WRITE_GRACE, send.write_all(&frame)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break StreamEnd::error(errors::STREAM_WRITE_FAILED),
            Err(_) => {
                let _ = send.reset(0u32.into());
                return;
            }
        }
        if Instant::now() >= deadline {
            break StreamEnd::error(errors::TIMED_OUT);
        }
    };
    drop(rx);
    drop(task);
    reply(&mut send, MessageType::StreamEnd, &end).await;
}
