// Derived from RaoFoundation/subtensor sdk/python/tests/harness/fake_node.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use rtao_primitives::H256;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{Notify, mpsc, watch},
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        Message,
        handshake::server::{ErrorResponse, Request, Response},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

use super::{FakeState, Notification, Reply, RpcRequest};

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An injected failure on the next accepted connection.
#[derive(Debug, Clone)]
pub enum Fault {
    /// Reject the WebSocket handshake with an actual HTTP response.
    HandshakeRefuse {
        /// HTTP status code.
        status: u16,
        /// Response headers, including gateway rate limit metadata.
        headers: Vec<(String, String)>,
        /// Response body.
        body: String,
    },
    /// Abruptly drop the connection after this many received application frames.
    DropAfterFrames(usize),
    /// Send a WebSocket close immediately after completing its handshake.
    Close {
        /// RFC 6455 close code, such as 1008.
        code: u16,
        /// Close reason.
        reason: String,
    },
    /// Return an error for the first request matching a method.
    JsonRpcError {
        /// Method to intercept.
        method: String,
        /// JSON-RPC error code.
        code: i64,
        /// Error message.
        message: String,
        /// Optional structured data.
        data: Option<Value>,
    },
    /// Record requests but omit replies for this period after the handshake.
    Silence(Duration),
    /// Return the node's pruned-state error for requests at this hash.
    StateDiscarded {
        /// Block whose state has been pruned.
        for_hash: H256,
    },
    /// Delay each request frame's response by this duration.
    SlowResponse(Duration),
}

/// Ordered connection failures. Each connection consumes one entry.
#[derive(Debug, Clone, Default)]
pub struct FaultPlan {
    faults: VecDeque<Fault>,
}

impl FaultPlan {
    /// Construct an ordered plan from connection faults.
    pub fn new(faults: impl IntoIterator<Item = Fault>) -> Self {
        Self {
            faults: faults.into_iter().collect(),
        }
    }

    /// Append a fault for a future connection.
    pub fn push(&mut self, fault: Fault) {
        self.faults.push_back(fault);
    }
}

/// One real TCP connection attempt and its WebSocket lifecycle.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    /// Stable, zero-based connection number.
    pub id: usize,
    /// The WebSocket handshake completed successfully.
    pub accepted: bool,
    /// The socket's task has ended, including abrupt drops and refused handshakes.
    pub closed: bool,
    /// Number of incoming application frames, including batch frames.
    pub frames: usize,
    /// Request headers observed during the HTTP handshake.
    pub headers: Vec<(String, String)>,
    /// Error encountered by the server, if any.
    pub error: Option<String>,
}

/// Fake server setup, JSON, certificate, or socket failure.
#[derive(Debug)]
pub struct FakeError(String);

impl fmt::Display for FakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl Error for FakeError {}

impl From<std::io::Error> for FakeError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug)]
enum Command {
    Frame(Value),
    Drop,
    Close(u16, String),
}

struct Observation {
    records: Vec<ConnectionRecord>,
    frames: Vec<Value>,
    controls: BTreeMap<usize, mpsc::UnboundedSender<Command>>,
    refuse: usize,
    silent: bool,
}

struct NodeInner {
    state: Arc<Mutex<FakeState>>,
    observation: Mutex<Observation>,
    faults: Mutex<FaultPlan>,
    changed: Notify,
    handshakes: watch::Sender<bool>,
}

/// A real JSON-RPC WebSocket server listening on an ephemeral loopback port.
pub struct FakeNode {
    inner: Arc<NodeInner>,
    url: String,
    root: Option<CertificateDer<'static>>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl FakeNode {
    /// Start a plaintext WebSocket server over a new default chain state.
    pub async fn start() -> Result<Self, FakeError> {
        let state = FakeState::try_default().map_err(|error| FakeError(error.to_string()))?;
        Self::with_state(Arc::new(Mutex::new(state))).await
    }

    /// Start a plaintext WebSocket server over an existing shared state.
    pub async fn with_state(state: Arc<Mutex<FakeState>>) -> Result<Self, FakeError> {
        Self::bind(state, FaultPlan::default(), false).await
    }

    /// Start a server with deterministic per-connection faults.
    pub async fn with_faults(
        state: Arc<Mutex<FakeState>>,
        faults: FaultPlan,
    ) -> Result<Self, FakeError> {
        Self::bind(state, faults, false).await
    }

    /// Start a TLS WebSocket server using an explicit ring provider.
    ///
    /// Pass [`Self::extra_root`] to the client's extra trust roots. This never
    /// consults or installs the process-default crypto provider.
    pub async fn start_tls() -> Result<Self, FakeError> {
        let state = FakeState::try_default().map_err(|error| FakeError(error.to_string()))?;
        Self::bind(Arc::new(Mutex::new(state)), FaultPlan::default(), true).await
    }

    /// Start a TLS server over shared state with connection faults.
    pub async fn with_tls(
        state: Arc<Mutex<FakeState>>,
        faults: FaultPlan,
    ) -> Result<Self, FakeError> {
        Self::bind(state, faults, true).await
    }

    async fn bind(
        state: Arc<Mutex<FakeState>>,
        faults: FaultPlan,
        tls: bool,
    ) -> Result<Self, FakeError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (acceptor, root) = if tls {
            let certified =
                rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
                    .map_err(|error| FakeError(error.to_string()))?;
            let certificate = certified.cert.der().clone();
            let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                certified.signing_key.serialize_der(),
            ));
            let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|error| FakeError(error.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private)
            .map_err(|error| FakeError(error.to_string()))?;
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            (Some(TlsAcceptor::from(Arc::new(config))), Some(certificate))
        } else {
            (None, None)
        };
        let (handshakes, _) = watch::channel(false);
        let inner = Arc::new(NodeInner {
            state,
            observation: Mutex::new(Observation {
                records: Vec::new(),
                frames: Vec::new(),
                controls: BTreeMap::new(),
                refuse: 0,
                silent: false,
            }),
            faults: Mutex::new(faults),
            changed: Notify::new(),
            handshakes,
        });
        let (shutdown, mut stopping) = watch::channel(false);
        let server = inner.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopping.changed() => break,
                    result = listener.accept() => {
                        let Ok((socket, _)) = result else { break };
                        let (id, refused) = {
                            let mut observation = lock(&server.observation);
                            let id = observation.records.len();
                            let refused = observation.refuse > 0;
                            if refused { observation.refuse -= 1; }
                            observation.records.push(ConnectionRecord {
                                id, accepted: false, closed: false, frames: 0, headers: Vec::new(), error: None,
                            });
                            (id, refused)
                        };
                        server.changed.notify_waiters();
                        let fault = lock(&server.faults).faults.pop_front();
                        let server = server.clone();
                        let acceptor = acceptor.clone();
                        connections.spawn(async move {
                            let _guard = ConnectionGuard { inner: server.clone(), id };
                            if refused { return; }
                            let mut blocked = server.handshakes.subscribe();
                            while *blocked.borrow_and_update() {
                                if blocked.changed().await.is_err() { return; }
                            }
                            if let Some(acceptor) = acceptor {
                                match acceptor.accept(socket).await {
                                    Ok(socket) => serve(socket, server, id, fault).await,
                                    Err(error) => set_error(&server, id, error.to_string()),
                                }
                            } else {
                                serve(socket, server, id, fault).await;
                            }
                        });
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self {
            inner,
            url: format!("{}://{address}", if tls { "wss" } else { "ws" }),
            root,
            shutdown,
            task,
        })
    }

    /// Endpoint URL for the running node.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Self-signed TLS certificate to add to a client's trust roots.
    pub fn extra_root(&self) -> Option<CertificateDer<'static>> {
        self.root.clone()
    }

    /// The same chain state used by a [`super::FakeChain`].
    pub fn state(&self) -> Arc<Mutex<FakeState>> {
        self.inner.state.clone()
    }

    /// Change handlers or encoded chain data atomically.
    pub fn mutate_state<R>(&self, update: impl FnOnce(&mut FakeState) -> R) -> R {
        update(&mut lock(&self.inner.state))
    }

    /// Every request in receive order, with batch entries recorded individually.
    pub fn requests(&self) -> Vec<RpcRequest> {
        lock(&self.inner.state).requests.clone()
    }

    /// Every incoming application frame, preserving batch grouping.
    pub fn frames(&self) -> Vec<Value> {
        lock(&self.inner.observation).frames.clone()
    }

    /// TCP attempts, successful handshakes, received frame counts and closed sockets.
    pub fn connections(&self) -> Vec<ConnectionRecord> {
        lock(&self.inner.observation).records.clone()
    }

    /// Number of observed TCP connection attempts.
    pub fn connect_attempts(&self) -> usize {
        lock(&self.inner.observation).records.len()
    }

    /// Refuse the next `count` TCP connections before their WebSocket handshake.
    pub fn refuse_next(&self, count: usize) {
        lock(&self.inner.observation).refuse += count;
    }

    /// Record incoming requests without answering while enabled.
    pub fn silence(&self, silent: bool) {
        lock(&self.inner.observation).silent = silent;
    }

    /// Hold new connections before their handshake until explicitly released.
    pub fn hold_handshakes(&self, hold: bool) {
        self.inner.handshakes.send_replace(hold);
    }

    /// Add a fault consumed by the next connection.
    pub fn push_fault(&self, fault: Fault) {
        lock(&self.inner.faults).push(fault);
    }

    /// Abruptly close the most recently accepted active WebSocket.
    pub fn drop_connection(&self) -> Result<(), FakeError> {
        self.current_command(Command::Drop)
    }

    /// Send a close frame on the current WebSocket.
    pub fn close_connection(&self, code: u16, reason: impl Into<String>) -> Result<(), FakeError> {
        self.current_command(Command::Close(code, reason.into()))
    }

    /// Push any JSON frame, including reordered responses or batch responses.
    pub fn push_frame(&self, frame: Value) -> Result<(), FakeError> {
        self.current_command(Command::Frame(frame))
    }

    /// Send a subscription update on the current WebSocket.
    pub fn push_subscription_update(
        &self,
        subscription_id: impl Into<String>,
        result: Value,
    ) -> Result<(), FakeError> {
        self.push_frame(json!({"jsonrpc":"2.0", "method":"subscription", "params":{
            "subscription":subscription_id.into(), "result":result,
        }}))
    }

    fn current_command(&self, command: Command) -> Result<(), FakeError> {
        let observation = lock(&self.inner.observation);
        let Some((_, sender)) = observation.controls.last_key_value() else {
            return Err(FakeError("the node has no active WebSocket".into()));
        };
        sender
            .send(command)
            .map_err(|error| FakeError(error.to_string()))
    }

    /// Wait until at least `count` request entries have been recorded.
    pub async fn wait_for_requests(&self, count: usize) {
        loop {
            let changed = self.inner.changed.notified();
            if self.requests().len() >= count {
                return;
            }
            changed.await;
        }
    }

    /// Wait until at least `count` TCP attempts have been observed.
    pub async fn wait_for_connections(&self, count: usize) {
        loop {
            let changed = self.inner.changed.notified();
            if self.connect_attempts() >= count {
                return;
            }
            changed.await;
        }
    }

    /// Wait until at least `count` sockets, including refused attempts, have closed.
    pub async fn wait_for_closed(&self, count: usize) {
        loop {
            let changed = self.inner.changed.notified();
            if self
                .connections()
                .iter()
                .filter(|record| record.closed)
                .count()
                >= count
            {
                return;
            }
            changed.await;
        }
    }

    /// Stop the listener and all its active socket tasks, then wait for cleanup.
    pub async fn shutdown(&mut self) {
        self.shutdown.send_replace(true);
        let _ = (&mut self.task).await;
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

struct ConnectionGuard {
    inner: Arc<NodeInner>,
    id: usize,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut observation = lock(&self.inner.observation);
        observation.records[self.id].closed = true;
        observation.controls.remove(&self.id);
        self.inner.changed.notify_waiters();
    }
}

fn set_error(inner: &NodeInner, id: usize, error: String) {
    lock(&inner.observation).records[id].error = Some(error);
}

async fn serve<S>(mut socket: S, inner: Arc<NodeInner>, id: usize, mut fault: Option<Fault>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Some(Fault::HandshakeRefuse {
        status,
        headers,
        body,
    }) = &fault
    {
        if let Err(error) = refuse_handshake(&mut socket, &inner, id, *status, headers, body).await
        {
            set_error(&inner, id, error.to_string());
        }
        return;
    }
    let observed = inner.clone();
    #[allow(clippy::result_large_err)]
    // tungstenite's Callback requires the complete HTTP response.
    let callback =
        move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            lock(&observed.observation).records[id].headers = request
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_owned(),
                        value.to_str().unwrap_or("<non-text>").to_owned(),
                    )
                })
                .collect();
            Ok(response)
        };
    let config = WebSocketConfig::default()
        .max_message_size(Some(256 * 1024 * 1024))
        .max_frame_size(Some(256 * 1024 * 1024));
    let mut socket = match accept_hdr_async_with_config(socket, callback, Some(config)).await {
        Ok(socket) => socket,
        Err(error) => {
            set_error(&inner, id, error.to_string());
            return;
        }
    };
    let (sender, mut commands) = mpsc::unbounded_channel();
    {
        let mut observation = lock(&inner.observation);
        observation.records[id].accepted = true;
        observation.controls.insert(id, sender);
    }
    inner.changed.notify_waiters();
    if let Some(Fault::Close { code, reason }) = &fault {
        let _ = socket
            .close(Some(CloseFrame {
                code: CloseCode::from(*code),
                reason: reason.clone().into(),
            }))
            .await;
        return;
    }
    let started = tokio::time::Instant::now();
    let mut outgoing: VecDeque<(tokio::time::Instant, Value)> = VecDeque::new();
    loop {
        let next_response = outgoing.front().map(|(ready, _)| *ready);
        tokio::select! {
            _ = async {
                match next_response {
                    Some(ready) => tokio::time::sleep_until(ready).await,
                    None => futures::future::pending().await,
                }
            } => {
                if let Some((_, frame)) = outgoing.pop_front()
                    && let Err(error) = socket.send(Message::Text(frame.to_string().into())).await
                {
                    set_error(&inner, id, error.to_string());
                    return;
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                let result = match command {
                    Command::Drop => return,
                    Command::Frame(value) => socket.send(Message::Text(value.to_string().into())).await,
                    Command::Close(code, reason) => {
                        let _ = socket.close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() })).await;
                        return;
                    }
                };
                if let Err(error) = result { set_error(&inner, id, error.to_string()); return; }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else { return };
                let message = match incoming {
                    Ok(message) => message,
                    Err(error) => { set_error(&inner, id, error.to_string()); return; }
                };
                let text = match message {
                    Message::Text(text) => text.to_string(),
                    Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                        Ok(text) => text,
                        Err(error) => { set_error(&inner, id, error.to_string()); return; }
                    },
                    Message::Close(_) => { let _ = socket.flush().await; return; }
                    Message::Ping(_) => { let _ = socket.flush().await; continue; }
                    _ => continue,
                };
                let frame: Value = match serde_json::from_str(&text) {
                    Ok(frame) => frame,
                    Err(error) => { set_error(&inner, id, error.to_string()); return; }
                };
                let frame_count = {
                    let mut observation = lock(&inner.observation);
                    observation.frames.push(frame.clone());
                    observation.records[id].frames += 1;
                    observation.records[id].frames
                };
                let batch = frame.is_array();
                let response_ready = tokio::time::Instant::now() + match &fault {
                    Some(Fault::SlowResponse(duration)) => *duration,
                    _ => Duration::ZERO,
                };
                let entries = if let Value::Array(entries) = frame { entries } else { vec![frame] };
                let mut replies = Vec::new();
                let mut batch_notifications = Vec::new();
                for entry in entries {
                    let request: RpcRequest = match serde_json::from_value(entry) {
                        Ok(request) => request,
                        Err(error) => { set_error(&inner, id, error.to_string()); return; }
                    };
                    let silent = lock(&inner.observation).silent
                        || matches!(&fault, Some(Fault::Silence(duration)) if started.elapsed() < *duration);
                    let drop_frame = matches!(&fault, Some(Fault::DropAfterFrames(count)) if frame_count >= *count);
                    let injected = fault_reply(&mut fault, &request);
                    let reply = if silent || drop_frame || injected.is_some() {
                        lock(&inner.state).requests.push(request.clone());
                        injected.unwrap_or_else(Reply::silent)
                    } else {
                        lock(&inner.state).dispatch(&request)
                    };
                    inner.changed.notify_waiters();
                    if drop_frame { return; }
                    if silent { continue; }
                    let notifications = notification_frames(&reply.notifications);
                    if reply.early {
                        for notification in &notifications {
                            outgoing.push_back((response_ready, notification.clone()));
                        }
                    }
                    if let Some(outcome) = reply.outcome {
                        let response = match outcome {
                            Ok(result) => json!({"jsonrpc":"2.0", "id":request.id, "result":result}),
                            Err(error) => {
                                let mut error_value = json!({"code":error.code, "message":error.message});
                                if let Some(data) = error.data { error_value["data"] = data; }
                                json!({"jsonrpc":"2.0", "id":request.id, "error":error_value})
                            }
                        };
                        if batch { replies.push(response); }
                        else { outgoing.push_back((response_ready, response)); }
                    }
                    if !reply.early {
                        if batch {
                            batch_notifications.extend(notifications);
                        } else {
                            for notification in notifications {
                                outgoing.push_back((response_ready, notification));
                            }
                        }
                    }
                }
                if batch && !replies.is_empty() { outgoing.push_back((response_ready, Value::Array(replies))); }
                for notification in batch_notifications {
                    outgoing.push_back((response_ready, notification));
                }
            }
        }
    }
}

async fn refuse_handshake<S>(
    socket: &mut S,
    inner: &NodeInner,
    id: usize,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> Result<(), FakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            return Err(FakeError("peer closed during the refused handshake".into()));
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > 64 * 1024 {
            return Err(FakeError("fake handshake request exceeds 64 KiB".into()));
        }
    }
    let observed_headers = String::from_utf8_lossy(&request)
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            line.split_once(':').map(|(name, value)| {
                (
                    name.to_ascii_lowercase(),
                    value.trim_matches([' ', '\t']).to_owned(),
                )
            })
        })
        .collect();
    lock(&inner.observation).records[id].headers = observed_headers;
    let status =
        http::StatusCode::from_u16(status).map_err(|error| FakeError(error.to_string()))?;
    let mut response = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Refused"),
        body.len()
    )
    .into_bytes();
    for (name, value) in headers {
        let name =
            http::HeaderName::try_from(name).map_err(|error| FakeError(error.to_string()))?;
        let value = http::HeaderValue::from_bytes(value.as_bytes())
            .map_err(|error| FakeError(error.to_string()))?;
        response.extend_from_slice(name.as_str().as_bytes());
        response.extend_from_slice(b": ");
        response.extend_from_slice(value.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body.as_bytes());
    socket.write_all(&response).await?;
    socket.shutdown().await?;
    Ok(())
}

fn notification_frames(notifications: &[Notification]) -> Vec<Value> {
    notifications
        .iter()
        .map(|notification| {
            json!({"jsonrpc":"2.0", "method":"subscription", "params":{
                "subscription":notification.subscription, "result":notification.result,
            }})
        })
        .collect()
}

fn fault_reply(fault: &mut Option<Fault>, request: &RpcRequest) -> Option<Reply> {
    let failure = match fault.as_ref() {
        Some(Fault::JsonRpcError {
            method,
            code,
            message,
            data,
        }) if method == &request.method => Some(super::RpcFailure {
            code: *code,
            message: message.clone(),
            data: data.clone(),
        }),
        Some(Fault::StateDiscarded { for_hash }) => {
            let hash = format!("0x{}", hex::encode(for_hash.0));
            request
                .params
                .as_array()
                .filter(|params| params.iter().any(|value| value.as_str() == Some(&hash)))
                .map(|_| super::RpcFailure {
                    code: -32000,
                    message: format!("State already discarded for {hash}"),
                    data: None,
                })
        }
        _ => None,
    };
    if let Some(failure) = failure {
        if matches!(fault, Some(Fault::JsonRpcError { .. })) {
            *fault = None;
        }
        Some(Reply {
            outcome: Some(Err(failure)),
            notifications: Vec::new(),
            early: false,
        })
    } else {
        None
    }
}
