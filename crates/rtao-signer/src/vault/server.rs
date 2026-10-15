// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/server.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use futures::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::derive_accept_key,
        protocol::{CloseFrame, Role, WebSocketConfig, frame::coding::CloseCode},
    },
};

use super::{
    MESSAGE_LIMIT, REQUEST_LIMIT, SEND_TIMEOUT, VaultPageError, lock, open_page, page_error,
    random_id,
};

const HTML: &[u8] = include_bytes!("../../../../vendor/upstream/vault/static/vault.html");
const SPECS_PNG: &[u8] =
    include_bytes!("../../../../vendor/upstream/vault/static/bittensor-chain-specs.png");
const CONNECTION_LIMIT: usize = 16;

/// Options for one ephemeral, loopback-only Vault page session.
#[derive(Debug, Clone)]
pub struct VaultServerOptions {
    /// Numeric loopback address. Port zero selects an ephemeral port.
    pub address: SocketAddr,
    /// Explicit permission to launch a browser when the server starts.
    pub open_browser: bool,
    /// Browser executable or macOS application; absent uses the system default.
    pub browser: Option<String>,
}
impl Default for VaultServerOptions {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
            open_browser: false,
            browser: None,
        }
    }
}

/// Per-request connection and answer deadlines.
#[derive(Debug, Clone, Copy)]
pub struct VaultRequestTimeouts {
    /// Wait for the page to authenticate; Python defaults to two minutes.
    pub connect: Duration,
    /// Wait for the scanned answer after connection; signing defaults to fifteen minutes.
    pub answer: Duration,
}
impl Default for VaultRequestTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(120),
            answer: Duration::from_secs(900),
        }
    }
}

struct Browser {
    generation: u64,
    outgoing: mpsc::Sender<String>,
    replace: watch::Sender<bool>,
}
struct Pending {
    id: u64,
    answer_kind: &'static str,
    request: String,
    result: oneshot::Sender<Result<String, VaultPageError>>,
}
struct Inner {
    browser: Option<Browser>,
    pending: Option<Pending>,
    next_generation: u64,
    next_request: u64,
}
struct Shared {
    address: SocketAddr,
    session: String,
    inner: Mutex<Inner>,
    requests: AsyncMutex<()>,
    connected: watch::Sender<bool>,
    stop: watch::Sender<bool>,
    connections: Arc<Semaphore>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}
struct Running {
    shared: Arc<Shared>,
    task: JoinHandle<Result<(), VaultPageError>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        shutdown(&self.shared);
        self.task.abort();
    }
}

/// One-shot HTTP/WebSocket page server speaking the pinned Python protocol.
///
/// Construction does not bind a socket or launch a process. At most one scan
/// is pending; requests serialize and survive a tab reconnect. Timeout,
/// cancellation and page errors retire the unfinished tab so a late scan
/// cannot answer the next request. Answers must match the requested kind.
/// Stopping or dropping the server cancels its work.
#[doc(alias = "bittensor.vault.VaultSessionServer")]
pub struct VaultSessionServer {
    options: VaultServerOptions,
    session: String,
    lifecycle: AsyncMutex<()>,
    running: Mutex<Option<Running>>,
}
impl std::fmt::Debug for VaultSessionServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultSessionServer")
            .field("address", &self.address())
            .finish_non_exhaustive()
    }
}
impl VaultSessionServer {
    /// Construct an inert session, refusing a non-loopback bind address.
    pub fn new(options: VaultServerOptions) -> Result<Self, VaultPageError> {
        if !options.address.ip().is_loopback() {
            return Err(page_error("Vault page server must bind a loopback address"));
        }
        Ok(Self {
            options,
            session: random_id()?,
            lifecycle: AsyncMutex::new(()),
            running: Mutex::new(None),
        })
    }
    /// Current listener address, including the allocated ephemeral port.
    pub fn address(&self) -> SocketAddr {
        self.running
            .lock()
            .ok()
            .and_then(|running| running.as_ref().map(|running| running.shared.address))
            .unwrap_or(self.options.address)
    }
    /// Session capability expected in the page's first WebSocket message.
    pub fn session_id(&self) -> &str {
        &self.session
    }
    /// Browser page URL including its session capability.
    pub fn http_url(&self) -> String {
        format!("http://{}/?session={}", self.address(), self.session)
    }
    /// WebSocket endpoint; the hello message still must authenticate its session.
    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.address())
    }
    pub(super) fn is_running(&self) -> bool {
        self.running.lock().ok().is_some_and(|running| {
            running.as_ref().is_some_and(|running| {
                !running.task.is_finished() && !*running.shared.stop.borrow()
            })
        })
    }

    /// Bind the ephemeral listener and, only when requested, open its page.
    pub async fn start(&self) -> Result<(), VaultPageError> {
        let _lifecycle = self.lifecycle.lock().await;
        if lock(&self.running)?.is_some() {
            return Ok(());
        }
        let listener = TcpListener::bind(self.options.address)
            .await
            .map_err(|_| page_error("cannot bind the loopback Vault page server"))?;
        let address = listener
            .local_addr()
            .map_err(|_| page_error("cannot read the Vault listener address"))?;
        let (stop, _) = watch::channel(false);
        let (connected, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            address,
            session: self.session.clone(),
            inner: Mutex::new(Inner {
                browser: None,
                pending: None,
                next_generation: 0,
                next_request: 0,
            }),
            requests: AsyncMutex::new(()),
            connected,
            stop,
            connections: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
            tasks: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/", get(page))
            .route("/index.html", get(page))
            .route("/bittensor-chain-specs.png", get(specs))
            .route("/ws", get(upgrade))
            .with_state(shared.clone());
        let mut stop = shared.stop.subscribe();
        let serving = shared.clone();
        let task = tokio::spawn(async move {
            let result = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = stop.changed().await;
            })
            .await
            .map_err(|_| page_error("Vault page listener failed"));
            shutdown(&serving);
            result
        });
        *lock(&self.running)? = Some(Running { shared, task });
        let mut startup = StartupGuard {
            server: self,
            armed: true,
        };
        if self.options.open_browser
            && let Err(error) = open_page(&self.http_url(), self.options.browser.as_deref()).await
        {
            if let Some(running) = lock(&self.running)?.take() {
                shutdown(&running.shared);
                running.task.abort();
            }
            return Err(error);
        }
        startup.armed = false;
        Ok(())
    }

    fn shared(&self) -> Result<Arc<Shared>, VaultPageError> {
        lock(&self.running)?
            .as_ref()
            .map(|running| running.shared.clone())
            .ok_or_else(|| page_error("Vault page server has not started"))
    }
    /// Wait for the authenticated page, then ask it to prepare its camera.
    pub async fn warm_up(&self, connect_timeout: Duration) -> Result<(), VaultPageError> {
        let shared = self.shared()?;
        let outgoing = wait_connected(&shared, connect_timeout, &self.http_url()).await?;
        let _ = timeout(
            SEND_TIMEOUT,
            outgoing.send(json!({"type":"warm-up"}).to_string()),
        )
        .await;
        Ok(())
    }
    /// Push rendered and raw frames, returning the signature text scanned by the page.
    ///
    /// Frames are SVG data URIs; `frames_hex` are the same UOS bytes as unprefixed
    /// hex. The summary is display-only. Serialized requests are bounded at 16 MiB.
    pub async fn request_signature(
        &self,
        frames: Vec<String>,
        frames_hex: Vec<String>,
        summary: Value,
        timeouts: VaultRequestTimeouts,
    ) -> Result<String, VaultPageError> {
        if frames.len() != frames_hex.len() {
            return Err(page_error("Vault rendered and raw frame counts differ"));
        }
        self.request(json!({"type":"sign-request","frames":frames,"framesHex":frames_hex,"summary":summary}),
            timeouts, "timed out waiting for the Vault signature scan; the transaction's era has likely expired — run the command again").await
    }
    /// Ask the page to scan an address QR and return the decoded text unchanged.
    pub async fn request_scan(
        &self,
        prompt: &str,
        timeouts: VaultRequestTimeouts,
    ) -> Result<String, VaultPageError> {
        self.request(
            json!({"type":"scan-request","prompt":prompt}),
            timeouts,
            "timed out waiting for a QR scan — run the command again",
        )
        .await
    }
    async fn request(
        &self,
        request: Value,
        timeouts: VaultRequestTimeouts,
        timeout_message: &str,
    ) -> Result<String, VaultPageError> {
        let shared = self.shared()?;
        let _operation = shared.requests.lock().await;
        if *shared.stop.borrow() {
            return Err(page_error("Vault page server stopped"));
        }
        let answer_kind = if request.get("type").and_then(Value::as_str) == Some("sign-request") {
            "signature"
        } else {
            "scanned"
        };
        let request = request.to_string();
        if request.len() > REQUEST_LIMIT {
            return Err(page_error("Vault page request exceeds the size limit"));
        }
        let (result, mut answer) = oneshot::channel();
        let (id, outgoing) = {
            let mut inner = lock(&shared.inner)?;
            inner.next_request = inner
                .next_request
                .checked_add(1)
                .ok_or_else(|| page_error("Vault request counter exhausted"))?;
            let id = inner.next_request;
            inner.pending = Some(Pending {
                id,
                answer_kind,
                request: request.clone(),
                result,
            });
            (
                id,
                inner
                    .browser
                    .as_ref()
                    .map(|browser| browser.outgoing.clone()),
            )
        };
        let _pending = PendingGuard {
            shared: shared.clone(),
            id,
        };
        if let Some(outgoing) = outgoing {
            // A closed old tab is harmless: its replacement replays the pending request.
            let _ = timeout(SEND_TIMEOUT, outgoing.send(request)).await;
        }
        let page_url = self.http_url();
        tokio::select! {
            early = &mut answer => {
                return early.map_err(|_| page_error("Vault page server stopped"))?;
            },
            connected = wait_connected(&shared, timeouts.connect, &page_url) => { connected?; }
        }
        timeout(timeouts.answer, answer)
            .await
            .map_err(|_| page_error(timeout_message))?
            .map_err(|_| page_error("Vault page server stopped"))?
    }
    /// Report chain submission success/failure to the current page, best effort.
    pub async fn report_result(&self, success: bool) -> Result<(), VaultPageError> {
        let shared = self.shared()?;
        let outgoing = lock(&shared.inner)?
            .browser
            .as_ref()
            .map(|browser| browser.outgoing.clone());
        if let Some(outgoing) = outgoing {
            let _ = timeout(
                SEND_TIMEOUT,
                outgoing.send(json!({"type":"result","success":success}).to_string()),
            )
            .await;
        }
        Ok(())
    }
    /// Cancel pending work and stop all session tasks, waiting at most five seconds.
    pub async fn stop(&self) -> Result<(), VaultPageError> {
        let _lifecycle = self.lifecycle.lock().await;
        let running = lock(&self.running)?.take();
        if let Some(mut running) = running {
            shutdown(&running.shared);
            match timeout(Duration::from_secs(5), &mut running.task).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(page_error("Vault listener task failed")),
                Err(_) => {
                    running.task.abort();
                    Err(page_error("Vault page server shutdown timed out"))
                }
            }
        } else {
            Ok(())
        }
    }
    pub(super) fn shutdown_now(&self) {
        let running = match self.running.lock() {
            Ok(mut running) => running.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(running) = running {
            shutdown(&running.shared);
            running.task.abort();
        }
    }
}
struct StartupGuard<'a> {
    server: &'a VaultSessionServer,
    armed: bool,
}
impl Drop for StartupGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.server.shutdown_now();
        }
    }
}
impl Drop for VaultSessionServer {
    fn drop(&mut self) {
        let running = match self.running.get_mut() {
            Ok(running) => running,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(running) = running.take() {
            shutdown(&running.shared);
            running.task.abort();
        }
    }
}
struct PendingGuard {
    shared: Arc<Shared>,
    id: u64,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.shared.inner.lock()
            && inner
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == self.id)
        {
            inner.pending = None;
            // The page protocol has no per-request correlation id. Retire
            // an unfinished tab as well as its queued messages so a late
            // answer cannot be consumed by the next serialized request.
            retire_browser(&self.shared, &mut inner);
        }
    }
}
fn retire_browser(shared: &Shared, inner: &mut Inner) {
    if let Some(browser) = inner.browser.take() {
        browser.replace.send_replace(true);
    }
    shared.connected.send_replace(false);
}
fn shutdown(shared: &Shared) {
    shared.stop.send_replace(true);
    shared.connected.send_replace(false);
    if let Ok(mut tasks) = shared.tasks.lock() {
        for task in tasks.drain(..) {
            task.abort();
        }
    }
    if let Ok(mut inner) = shared.inner.lock() {
        inner.browser = None;
        if let Some(pending) = inner.pending.take() {
            let _ = pending
                .result
                .send(Err(page_error("Vault page server stopped")));
        }
    }
}
async fn wait_connected(
    shared: &Shared,
    wait: Duration,
    page_url: &str,
) -> Result<mpsc::Sender<String>, VaultPageError> {
    let mut connected = shared.connected.subscribe();
    let mut stop = shared.stop.subscribe();
    timeout(wait, async {
        loop {
            if *stop.borrow() { return Err(page_error("Vault page server stopped")); }
            if let Some(outgoing) = lock(&shared.inner)?.browser.as_ref().map(|browser| browser.outgoing.clone()) { return Ok(outgoing); }
            tokio::select! {
                result = connected.changed() => { result.map_err(|_| page_error("Vault page server stopped"))?; },
                _ = stop.changed() => { return Err(page_error("Vault page server stopped")); }
            }
        }
    }).await.map_err(|_| VaultPageError(format!("the vault page never connected; open {page_url} manually")))?
}
fn own_host(headers: &HeaderMap, address: SocketAddr) -> bool {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        == Some(address.to_string().as_str())
}
fn origin_allowed(headers: &HeaderMap, address: SocketAddr) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin.to_str().ok() == Some(format!("http://{address}").as_str()),
    }
}
fn asset(
    shared: &Shared,
    headers: HeaderMap,
    content: &'static [u8],
    mime: &'static str,
) -> Response {
    if !own_host(&headers, shared.address) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Response::new(Body::from(content));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(mime));
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        "referrer-policy",
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert("x-frame-options", header::HeaderValue::from_static("DENY"));
    response
}
async fn page(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    asset(&shared, headers, HTML, "text/html; charset=utf-8")
}
async fn specs(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    asset(&shared, headers, SPECS_PNG, "image/png")
}
async fn upgrade(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request<Body>,
) -> Response {
    if *shared.stop.borrow() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !peer.ip().is_loopback()
        || !own_host(request.headers(), shared.address)
        || !origin_allowed(request.headers(), shared.address)
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let headers = request.headers();
    let header_is = |name: &str, expected: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case(expected))
    };
    let connection_upgrade = headers
        .get(header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        });
    let key = headers
        .get("sec-websocket-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let valid_key = key.len() == 24
        && base64::engine::general_purpose::STANDARD
            .decode(key)
            .is_ok_and(|bytes| bytes.len() == 16);
    if !header_is("upgrade", "websocket")
        || !header_is("sec-websocket-version", "13")
        || !connection_upgrade
        || !valid_key
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let accept = derive_accept_key(key.as_bytes());
    let Ok(permit) = shared.connections.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let upgrade = hyper::upgrade::on(&mut request);
    let tracked = shared.clone();
    let Ok(mut tasks) = tracked.tasks.lock() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if *tracked.stop.borrow() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    tasks.retain(|task| !task.is_finished());
    tasks.push(tokio::spawn(async move {
        let _permit = permit;
        if let Ok(Ok(upgraded)) = timeout(SEND_TIMEOUT, upgrade).await {
            let socket = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server,
                Some(WebSocketConfig::default().max_message_size(Some(MESSAGE_LIMIT)).max_frame_size(Some(MESSAGE_LIMIT)))).await;
            let mut stop = shared.stop.subscribe();
            if *stop.borrow() { return; }
            tokio::select! { _ = connection(socket, shared.clone()) => {}, _ = stop.changed() => {} }
        }
    }));
    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
type Socket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;
async fn close(socket: &mut Socket, code: CloseCode, reason: &'static str) {
    let _ = timeout(
        SEND_TIMEOUT,
        socket.close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })),
    )
    .await;
}
async fn send(socket: &mut Socket, text: String) -> Result<(), VaultPageError> {
    timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .map_err(|_| page_error("Vault page send timed out"))?
        .map_err(|_| page_error("Vault page disconnected"))
}
async fn connection(mut socket: Socket, shared: Arc<Shared>) {
    let hello = timeout(Duration::from_secs(10), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str::<Value>(&text).ok(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    if !hello.is_some_and(|hello| {
        hello.get("role").and_then(Value::as_str) == Some("vault")
            && hello.get("session").and_then(Value::as_str) == Some(shared.session.as_str())
    }) {
        close(&mut socket, CloseCode::Policy, "stale vault session").await;
        return;
    }
    let (outgoing, mut incoming) = mpsc::channel(4);
    let (replace, mut replaced) = watch::channel(false);
    let (generation, pending) = {
        let Ok(mut inner) = shared.inner.lock() else {
            return;
        };
        let Some(generation) = inner.next_generation.checked_add(1) else {
            return;
        };
        inner.next_generation = generation;
        if let Some(previous) = inner.browser.take() {
            previous.replace.send_replace(true);
        }
        let pending = inner
            .pending
            .as_ref()
            .map(|pending| pending.request.clone());
        inner.browser = Some(Browser {
            generation,
            outgoing,
            replace,
        });
        (generation, pending)
    };
    let _browser = BrowserGuard {
        shared: shared.clone(),
        generation,
    };
    if let Some(pending) = pending
        && send(&mut socket, pending).await.is_err()
    {
        return;
    }
    shared.connected.send_replace(true);
    loop {
        tokio::select! {
            _ = replaced.changed() => { close(&mut socket, CloseCode::Restart, "replaced by new vault tab").await; break; },
            request = incoming.recv() => {
                let Some(request) = request else { break; };
                if send(&mut socket, request).await.is_err() { break; }
            },
            reply = socket.next() => {
                let Some(Ok(message)) = reply else { break; };
                match message {
                    Message::Ping(_) | Message::Pong(_) => {},
                    Message::Text(text) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue; };
                        let answer = match value.get("type").and_then(Value::as_str) {
                            Some("signature") => value.get("signature").and_then(Value::as_str).filter(|text| !text.is_empty()).map(|text| Ok(text.to_owned())),
                            Some("scanned") => value.get("text").and_then(Value::as_str).filter(|text| !text.is_empty()).map(|text| Ok(text.to_owned())),
                            Some("error") => Some(Err(VaultPageError(value.get("message").and_then(Value::as_str).filter(|text| !text.is_empty()).unwrap_or("vault page error").chars().filter(|ch| !ch.is_control()).take(4096).collect()))),
                            _ => None,
                        };
                        if let Some(answer) = answer
                            && let Ok(mut inner) = shared.inner.lock()
                            && inner.browser.as_ref().is_some_and(|browser| browser.generation == generation) {
                                    if value.get("type").and_then(Value::as_str) != Some("error") && !inner.pending.as_ref().is_some_and(|pending| value.get("type").and_then(Value::as_str)==Some(pending.answer_kind)) { continue; }
                                    if let Some(pending) = inner.pending.take() {
                                        let failed = answer.is_err();
                                        if pending.result.send(answer).is_err() || failed {
                                            retire_browser(&shared, &mut inner);
                                        }
                                    }
                        }
                    },
                    _ => break,
                }
            }
        }
    }
}
struct BrowserGuard {
    shared: Arc<Shared>,
    generation: u64,
}
impl Drop for BrowserGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.shared.inner.lock()
            && inner
                .browser
                .as_ref()
                .is_some_and(|browser| browser.generation == self.generation)
        {
            inner.browser = None;
            self.shared.connected.send_replace(false);
        }
    }
}
