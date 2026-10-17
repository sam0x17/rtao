//! A validator-side client that authenticates to btlightning miners.
//!
//! One QUIC connection is kept per miner address; each request, and each
//! handshake, uses a fresh bidirectional stream carrying exactly one frame.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use quinn::{Connection, Endpoint, RecvStream};
use rtao_primitives::AccountId;
use rtao_signer::Signer;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::QuicError;
use crate::auth::{fingerprint_b64, sign_handshake, verify_handshake_response};
use crate::data::{from_data, to_data};
use crate::tls::{client_config, peer_fingerprint, transport};
use crate::wire::{
    DEFAULT_MAX_FRAME_PAYLOAD, DataMap, HandshakeResponse, MIN_MAX_FRAME_PAYLOAD, MessageType,
    StreamChunk, StreamEnd, SynapsePacket, SynapseResponse, decode, encode, read_frame, unix_now,
    write_budget, write_frame,
};

/// Reconnection backoff: exponential for a few attempts, then a slow probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// Delay after the first failure.
    pub initial: Duration,
    /// Cap on the exponential delay.
    pub max: Duration,
    /// Failures handled with the exponential delay.
    pub fast_retries: u32,
    /// Delay between later attempts.
    pub slow_probe: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            fast_retries: 5,
            slow_probe: Duration::from_secs(60),
        }
    }
}

impl Backoff {
    /// Delay before the next attempt after `failures` consecutive failures.
    pub fn delay(&self, failures: u32) -> Duration {
        if failures == 0 {
            Duration::ZERO
        } else if failures > self.fast_retries {
            self.slow_probe
        } else {
            let factor = 1u32
                .checked_shl(failures.saturating_sub(1))
                .unwrap_or(u32::MAX);
            self.initial.saturating_mul(factor).min(self.max)
        }
    }
}

/// Client limits and timeouts, defaulting to btlightning's values.
#[derive(Clone, Debug)]
pub struct QuicClientConfig {
    /// Bound on each connect and each handshake (10 s).
    pub connect_timeout: Duration,
    /// QUIC idle timeout (150 s).
    pub idle_timeout: Duration,
    /// QUIC keep-alive interval (30 s), below the idle timeout.
    pub keep_alive: Duration,
    /// Largest accepted frame payload (64 MiB, at least 1 MiB).
    pub max_frame_payload: u32,
    /// Largest total of streamed chunk bytes collected by
    /// [`ChunkStream::collect_all`] (64 MiB, at least the frame maximum).
    pub max_stream_payload: usize,
    /// Optional bound on waiting for each streamed frame.
    pub chunk_timeout: Option<Duration>,
    /// Most miner addresses with an open connection, or a dial or handshake
    /// in progress (1,024). Addresses whose connection failed or closed are
    /// forgotten, oldest backoff first, to make room.
    pub max_connections: usize,
    /// Reconnection backoff.
    pub backoff: Backoff,
}

impl Default for QuicClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(150),
            keep_alive: Duration::from_secs(30),
            max_frame_payload: DEFAULT_MAX_FRAME_PAYLOAD,
            max_stream_payload: DEFAULT_MAX_FRAME_PAYLOAD as usize,
            chunk_timeout: None,
            max_connections: 1024,
            backoff: Backoff::default(),
        }
    }
}

impl QuicClientConfig {
    /// Check the configuration's invariants.
    pub fn validate(&self) -> Result<(), QuicError> {
        let fail = |m: &str| Err(QuicError::Config(m.to_owned()));
        if self.connect_timeout.is_zero() {
            return fail("connect_timeout must be non-zero");
        }
        if self.idle_timeout.is_zero() || self.keep_alive.is_zero() {
            return fail("idle_timeout and keep_alive must be non-zero");
        }
        if self.keep_alive >= self.idle_timeout {
            return fail("keep_alive must be less than idle_timeout");
        }
        if self.max_frame_payload < MIN_MAX_FRAME_PAYLOAD {
            return fail("max_frame_payload must be at least 1 MiB");
        }
        if self.max_stream_payload < self.max_frame_payload as usize {
            return fail("max_stream_payload must be at least max_frame_payload");
        }
        if self.max_connections == 0 {
            return fail("max_connections must be non-zero");
        }
        if self.backoff.initial.is_zero() || self.backoff.max < self.backoff.initial {
            return fail("backoff must be non-zero and max must not be below initial");
        }
        Ok(())
    }
}

/// A miner endpoint and the identity it must prove.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QuicTarget {
    /// The miner hotkey.
    pub hotkey: AccountId,
    /// The miner's QUIC address.
    pub addr: SocketAddr,
    /// Optional certificate fingerprint pin.
    pub fingerprint: Option<[u8; 32]>,
}

impl QuicTarget {
    /// A target with no fingerprint pin.
    pub fn new(hotkey: AccountId, addr: SocketAddr) -> Self {
        Self {
            hotkey,
            addr,
            fingerprint: None,
        }
    }

    /// A target from chain axon fields. Returns `None` for an unset IP or port.
    pub fn from_axon(hotkey: AccountId, ip: u128, ip_type: u8, port: u16) -> Option<Self> {
        if ip == 0 || port == 0 {
            return None;
        }
        let ip = match ip_type {
            4 => IpAddr::V4(Ipv4Addr::from(u32::try_from(ip).ok()?)),
            6 => IpAddr::V6(Ipv6Addr::from(ip)),
            _ => return None,
        };
        Some(Self::new(hotkey, SocketAddr::new(ip, port)))
    }
}

/// A unary reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// Whether the handler succeeded.
    pub success: bool,
    /// Handler output.
    pub data: DataMap,
    /// Server error string.
    pub error: Option<String>,
    /// Client-measured round trip.
    pub latency: Duration,
}

impl Reply {
    /// `Err(Handler)` when the server reported failure.
    pub fn into_result(self) -> Result<Self, QuicError> {
        if self.success {
            Ok(self)
        } else {
            Err(QuicError::Handler(
                self.error.unwrap_or_else(|| "request failed".into()),
            ))
        }
    }

    /// Decode the data map into a typed value.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, QuicError> {
        Ok(from_data(self.data.clone())?)
    }
}

/// Run the validator handshake on an established connection.
///
/// Returns the miner's reply after checking acceptance, identity,
/// certificate fingerprint and signature.
pub async fn authenticate(
    connection: &Connection,
    signer: &dyn Signer,
    expected: &AccountId,
    max_frame_payload: u32,
) -> Result<HandshakeResponse, QuicError> {
    let fp = peer_fingerprint(connection).ok_or_else(|| {
        QuicError::Handshake("peer certificate not available for fingerprinting".into())
    })?;
    let fp_b64 = fingerprint_b64(&fp);
    let request = sign_handshake(signer, &fp_b64, unix_now()).await?;
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| QuicError::Connection(e.to_string()))?;
    write_frame(&mut send, MessageType::HandshakeRequest, &encode(&request)?).await?;
    send.finish()
        .map_err(|e| QuicError::Transport(e.to_string()))?;
    let (kind, payload) = read_frame(&mut recv, max_frame_payload).await?;
    if kind != MessageType::HandshakeResponse {
        return Err(QuicError::Handshake(format!(
            "expected HandshakeResponse, got {kind:?}"
        )));
    }
    let response: HandshakeResponse = decode(&payload)?;
    verify_handshake_response(&response, expected, &request, &fp_b64)?;
    Ok(response)
}

struct Session {
    connection: Connection,
    /// The presented certificate's fingerprint, checked against pins.
    fingerprint: Option<[u8; 32]>,
    authenticated: HashSet<AccountId>,
}

#[derive(Default)]
struct Slot {
    session: Option<Session>,
    failures: u32,
    retry_at: Option<Instant>,
}

impl Slot {
    fn is_live(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.connection.close_reason().is_none())
    }
}

/// A pool of authenticated connections to miners.
pub struct QuicClient {
    signer: Arc<dyn Signer>,
    config: QuicClientConfig,
    endpoint_v4: Endpoint,
    endpoint_v6: StdMutex<Option<Endpoint>>,
    slots: StdMutex<HashMap<SocketAddr, Arc<Mutex<Slot>>>>,
}

impl QuicClient {
    /// A client signing handshakes with `signer`, which must be sr25519 and
    /// support raw signing. Binds an IPv4 UDP socket; an IPv6 socket is bound
    /// on first use.
    ///
    /// Must be called inside a tokio runtime.
    pub fn new(signer: Arc<dyn Signer>, config: QuicClientConfig) -> Result<Self, QuicError> {
        config.validate()?;
        let endpoint_v4 = Self::endpoint(&config, SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?;
        Ok(Self {
            signer,
            config,
            endpoint_v4,
            endpoint_v6: StdMutex::new(None),
            slots: StdMutex::new(HashMap::new()),
        })
    }

    fn endpoint(config: &QuicClientConfig, bind: SocketAddr) -> Result<Endpoint, QuicError> {
        let mut endpoint =
            Endpoint::client(bind).map_err(|e| QuicError::Connection(e.to_string()))?;
        endpoint.set_default_client_config(client_config(
            None,
            transport(config.idle_timeout, config.keep_alive)?,
        )?);
        Ok(endpoint)
    }

    /// The client's configuration.
    pub fn config(&self) -> &QuicClientConfig {
        &self.config
    }

    /// The local address of the IPv4 socket.
    pub fn local_addr(&self) -> Result<SocketAddr, QuicError> {
        self.endpoint_v4
            .local_addr()
            .map_err(|e| QuicError::Connection(e.to_string()))
    }

    fn endpoint_for(&self, addr: &SocketAddr) -> Result<Endpoint, QuicError> {
        if addr.is_ipv4() {
            return Ok(self.endpoint_v4.clone());
        }
        let mut v6 = self
            .endpoint_v6
            .lock()
            .map_err(|_| QuicError::Connection("endpoint lock poisoned".into()))?;
        if let Some(endpoint) = v6.as_ref() {
            return Ok(endpoint.clone());
        }
        let endpoint = Self::endpoint(&self.config, SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))?;
        *v6 = Some(endpoint.clone());
        Ok(endpoint)
    }

    fn slot(&self, addr: SocketAddr) -> Result<Arc<Mutex<Slot>>, QuicError> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| QuicError::Connection("pool lock poisoned".into()))?;
        if let Some(slot) = slots.get(&addr) {
            return Ok(slot.clone());
        }
        if slots.len() >= self.config.max_connections {
            // Forget one idle slot without a live connection: no other task
            // holds it (the map lock stops new clones) and it is unlocked.
            // Slots whose backoff ended first, then the earliest to end.
            let idle = slots
                .iter()
                .filter(|(_, slot)| Arc::strong_count(slot) == 1)
                .filter_map(|(addr, slot)| {
                    let slot = slot.try_lock().ok()?;
                    (!slot.is_live()).then_some((*addr, slot.retry_at))
                })
                .min_by_key(|(_, retry_at)| *retry_at)
                .map(|(addr, _)| addr);
            match idle {
                Some(idle) => {
                    slots.remove(&idle);
                }
                None => {
                    return Err(QuicError::Connection(format!(
                        "connection limit {} reached",
                        self.config.max_connections
                    )));
                }
            }
        }
        let slot = Arc::new(Mutex::new(Slot::default()));
        slots.insert(addr, slot.clone());
        Ok(slot)
    }

    async fn dial(&self, target: &QuicTarget) -> Result<Connection, QuicError> {
        let endpoint = self.endpoint_for(&target.addr)?;
        let server_name = target.addr.ip().to_string();
        let connecting = match target.fingerprint {
            None => endpoint.connect(target.addr, &server_name),
            Some(pin) => endpoint.connect_with(
                client_config(
                    Some(pin),
                    transport(self.config.idle_timeout, self.config.keep_alive)?,
                )?,
                target.addr,
                &server_name,
            ),
        }
        .map_err(|e| QuicError::Connection(e.to_string()))?;
        timeout(self.config.connect_timeout, connecting)
            .await
            .map_err(|_| QuicError::Timeout("connect"))?
            .map_err(|e| QuicError::Connection(e.to_string()))
    }

    /// Connect and authenticate to `target` if needed, returning the
    /// authenticated connection.
    pub async fn connect(&self, target: &QuicTarget) -> Result<Connection, QuicError> {
        let slot = self.slot(target.addr)?;
        let mut slot = slot.lock().await;
        if slot
            .session
            .as_ref()
            .is_some_and(|s| s.connection.close_reason().is_some())
        {
            slot.session = None;
        }
        if let (Some(pin), Some(session)) = (target.fingerprint, &slot.session)
            && session.fingerprint != Some(pin)
        {
            // The pooled connection was made without this pin, or with
            // another one; a redial would reach the same certificate.
            return Err(QuicError::Handshake(format!(
                "pooled connection certificate {} does not match the pinned fingerprint {}",
                session
                    .fingerprint
                    .map(|f| fingerprint_b64(&f))
                    .unwrap_or_default(),
                fingerprint_b64(&pin)
            )));
        }
        if let Some(session) = &slot.session
            && session.authenticated.contains(&target.hotkey)
        {
            return Ok(session.connection.clone());
        }
        if let Some(at) = slot.retry_at {
            let now = Instant::now();
            if at > now {
                return Err(QuicError::Backoff(at - now));
            }
        }
        let result = self.establish(&mut slot, target).await;
        match &result {
            Ok(_) => {
                slot.failures = 0;
                slot.retry_at = None;
            }
            Err(_) => {
                slot.failures = slot.failures.saturating_add(1);
                slot.retry_at = Some(Instant::now() + self.config.backoff.delay(slot.failures));
            }
        }
        result
    }

    async fn establish(
        &self,
        slot: &mut Slot,
        target: &QuicTarget,
    ) -> Result<Connection, QuicError> {
        let connection = match &slot.session {
            Some(session) => session.connection.clone(),
            None => {
                let connection = self.dial(target).await?;
                slot.session = Some(Session {
                    fingerprint: peer_fingerprint(&connection),
                    connection: connection.clone(),
                    authenticated: HashSet::new(),
                });
                connection
            }
        };
        let handshake = timeout(
            self.config.connect_timeout,
            authenticate(
                &connection,
                self.signer.as_ref(),
                &target.hotkey,
                self.config.max_frame_payload,
            ),
        )
        .await
        .map_err(|_| QuicError::Timeout("handshake"))
        .and_then(|r| r);
        match handshake {
            Ok(_) => {
                if let Some(session) = slot.session.as_mut() {
                    session.authenticated.insert(target.hotkey);
                }
                Ok(connection)
            }
            Err(e) => {
                if slot
                    .session
                    .as_ref()
                    .is_some_and(|s| s.authenticated.is_empty())
                {
                    connection.close(0u32.into(), b"no_authenticated_hotkeys");
                    slot.session = None;
                }
                Err(e)
            }
        }
    }

    /// Connect to every target, returning each result.
    pub async fn connect_all(
        &self,
        targets: impl IntoIterator<Item = QuicTarget>,
        concurrency: usize,
    ) -> Vec<(QuicTarget, Result<(), QuicError>)> {
        futures::stream::iter(targets)
            .map(|t| async move {
                let r = self.connect(&t).await.map(|_| ());
                (t, r)
            })
            .buffer_unordered(concurrency.max(1))
            .collect()
            .await
    }

    async fn open(
        &self,
        target: &QuicTarget,
    ) -> Result<(quinn::SendStream, RecvStream), QuicError> {
        let mut retried = false;
        loop {
            let connection = self.connect(target).await?;
            match connection.open_bi().await {
                Ok(streams) => return Ok(streams),
                Err(e) if retried => return Err(QuicError::Connection(e.to_string())),
                Err(_) => retried = true,
            }
        }
    }

    async fn send_packet(
        &self,
        target: &QuicTarget,
        synapse_type: &str,
        data: DataMap,
    ) -> Result<RecvStream, QuicError> {
        let packet = SynapsePacket {
            synapse_type: synapse_type.to_owned(),
            data,
            timestamp: unix_now(),
        };
        let payload = encode(&packet)?;
        let (mut send, recv) = self.open(target).await?;
        timeout(write_budget(payload.len()), async {
            write_frame(&mut send, MessageType::SynapsePacket, &payload).await?;
            send.finish()
                .map_err(|e| QuicError::Transport(e.to_string()))
        })
        .await
        .map_err(|_| QuicError::Timeout("request transfer"))??;
        Ok(recv)
    }

    /// Send a unary request. `success = false` is returned as a [`Reply`];
    /// use [`Reply::into_result`] to turn it into an error.
    pub async fn query(
        &self,
        target: &QuicTarget,
        synapse_type: &str,
        data: DataMap,
        response_timeout: Option<Duration>,
    ) -> Result<Reply, QuicError> {
        let started = Instant::now();
        let mut recv = self.send_packet(target, synapse_type, data).await?;
        let read = read_frame(&mut recv, self.config.max_frame_payload);
        let (kind, payload) = match response_timeout {
            Some(limit) => timeout(limit, read)
                .await
                .map_err(|_| QuicError::Timeout("query"))??,
            None => read.await?,
        };
        match kind {
            MessageType::SynapseResponse => {
                let response: SynapseResponse = decode(&payload)?;
                Ok(Reply {
                    success: response.success,
                    data: response.data,
                    error: response.error,
                    latency: started.elapsed(),
                })
            }
            MessageType::StreamChunk => Err(QuicError::Transport(
                "received StreamChunk on non-streaming query; use query_stream".into(),
            )),
            other => Err(QuicError::Transport(format!(
                "unexpected response type: {other:?}"
            ))),
        }
    }

    /// Send a typed request and decode a successful typed reply.
    pub async fn query_typed<Q: Serialize, R: DeserializeOwned>(
        &self,
        target: &QuicTarget,
        synapse_type: &str,
        request: &Q,
        response_timeout: Option<Duration>,
    ) -> Result<R, QuicError> {
        self.query(target, synapse_type, to_data(request)?, response_timeout)
            .await?
            .into_result()?
            .decode()
    }

    /// Send a streaming request.
    pub async fn query_stream(
        &self,
        target: &QuicTarget,
        synapse_type: &str,
        data: DataMap,
    ) -> Result<ChunkStream, QuicError> {
        let recv = self.send_packet(target, synapse_type, data).await?;
        Ok(ChunkStream {
            recv,
            max_frame_payload: self.config.max_frame_payload,
            max_total: self.config.max_stream_payload,
            chunk_timeout: self.config.chunk_timeout,
            done: false,
        })
    }

    /// Query many targets concurrently, with a per-request timeout.
    pub async fn query_many(
        &self,
        targets: impl IntoIterator<Item = QuicTarget>,
        synapse_type: &str,
        data: &DataMap,
        concurrency: usize,
        per_request: Option<Duration>,
    ) -> Vec<(QuicTarget, Result<Reply, QuicError>)> {
        futures::stream::iter(targets)
            .map(|t| async move {
                let r = self
                    .query(&t, synapse_type, data.clone(), per_request)
                    .await;
                (t, r)
            })
            .buffer_unordered(concurrency.max(1))
            .collect()
            .await
    }

    /// Keep connections only to the given targets.
    ///
    /// Connections to other addresses are closed. At a kept address, miner
    /// hotkeys that are no longer listed there are forgotten, and the
    /// connection is closed when none of its authenticated hotkeys remains.
    pub async fn retain_targets(&self, targets: &[QuicTarget]) {
        let mut keep: HashMap<SocketAddr, HashSet<AccountId>> = HashMap::new();
        for target in targets {
            keep.entry(target.addr).or_default().insert(target.hotkey);
        }
        let (removed, kept): (Vec<_>, Vec<_>) = match self.slots.lock() {
            Ok(mut slots) => {
                let gone: Vec<SocketAddr> = slots
                    .keys()
                    .filter(|a| !keep.contains_key(a))
                    .copied()
                    .collect();
                let removed = gone.iter().filter_map(|a| slots.remove(a)).collect();
                let kept = slots.iter().map(|(a, s)| (*a, s.clone())).collect();
                (removed, kept)
            }
            Err(_) => (Vec::new(), Vec::new()),
        };
        for slot in removed {
            if let Some(session) = slot.lock().await.session.take() {
                session.connection.close(0u32.into(), b"miner_deregistered");
            }
        }
        for (addr, slot) in kept {
            let Some(hotkeys) = keep.get(&addr) else {
                continue;
            };
            let mut slot = slot.lock().await;
            let emptied = slot.session.as_mut().is_some_and(|session| {
                let before = session.authenticated.len();
                session.authenticated.retain(|h| hotkeys.contains(h));
                before > 0 && session.authenticated.is_empty()
            });
            if emptied && let Some(session) = slot.session.take() {
                session.connection.close(0u32.into(), b"miner_deregistered");
            }
        }
    }

    /// Close every connection and the endpoints.
    pub async fn close(&self) {
        self.retain_targets(&[]).await;
        self.endpoint_v4.close(0u32.into(), b"client_shutdown");
        if let Ok(v6) = self.endpoint_v6.lock()
            && let Some(endpoint) = v6.as_ref()
        {
            endpoint.close(0u32.into(), b"client_shutdown");
        }
    }
}

/// The chunks of a streamed reply.
pub struct ChunkStream {
    recv: RecvStream,
    max_frame_payload: u32,
    max_total: usize,
    chunk_timeout: Option<Duration>,
    done: bool,
}

impl ChunkStream {
    /// The next chunk, or `None` after a successful end.
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, QuicError> {
        if self.done {
            return Ok(None);
        }
        let read = read_frame(&mut self.recv, self.max_frame_payload);
        let frame = match self.chunk_timeout {
            Some(limit) => match timeout(limit, read).await {
                Ok(frame) => frame,
                Err(_) => {
                    self.done = true;
                    let _ = self.recv.stop(0u32.into());
                    return Err(QuicError::Stream("chunk read timed out".into()));
                }
            },
            None => read.await,
        };
        let (kind, payload) = frame.inspect_err(|_| self.done = true)?;
        match kind {
            MessageType::StreamChunk => {
                let chunk: StreamChunk = decode(&payload).inspect_err(|_| self.done = true)?;
                Ok(Some(chunk.data))
            }
            MessageType::StreamEnd => {
                self.done = true;
                let end: StreamEnd = decode(&payload)?;
                if end.success {
                    Ok(None)
                } else {
                    Err(QuicError::Stream(end.error.unwrap_or_else(|| {
                        "stream ended with failure status".into()
                    })))
                }
            }
            MessageType::SynapseResponse => {
                self.done = true;
                let response: SynapseResponse = decode(&payload)?;
                Err(QuicError::Stream(format!(
                    "server returned SynapseResponse error on streaming path: {}",
                    response.error.as_deref().unwrap_or("no detail")
                )))
            }
            other => {
                self.done = true;
                Err(QuicError::Stream(format!(
                    "unexpected message type during streaming: {other:?}"
                )))
            }
        }
    }

    /// Collect every chunk, bounded by the configured stream maximum.
    pub async fn collect_all(mut self) -> Result<Vec<Vec<u8>>, QuicError> {
        let mut chunks = Vec::new();
        let mut total = 0usize;
        while let Some(chunk) = self.next_chunk().await? {
            total = total.saturating_add(chunk.len());
            if total > self.max_total {
                let _ = self.recv.stop(0u32.into());
                return Err(QuicError::Stream(format!(
                    "stream payload exceeds maximum {}",
                    self.max_total
                )));
            }
            chunks.push(chunk);
        }
        Ok(chunks)
    }
}
