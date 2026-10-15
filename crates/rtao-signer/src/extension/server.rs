// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/bridge.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    collections::HashMap,
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
use futures::StreamExt;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot, watch},
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
use zeroize::Zeroizing;

use super::client::{receive_json, send_text};
use super::{
    BridgeError, MESSAGE_LIMIT, REQUEST_TIMEOUT, SEND_TIMEOUT, bounded_json, error, lock, random_id,
};

const PENDING_LIMIT: usize = 32;
const CONNECTION_LIMIT: usize = 64;
const BRIDGE_HTML: &[u8] =
    include_bytes!("../../../../vendor/upstream/extension/static/bridge.html");

/// Explicit server start options. Defaults do not open a browser.
#[derive(Clone, Debug)]
pub struct BridgeOptions {
    /// Numeric loopback listener address; port zero selects an ephemeral port.
    pub address: SocketAddr,
    /// Explicit permission to launch the system browser.
    pub open_browser: bool,
    /// Browser executable/application; absent uses the system default.
    pub browser: Option<String>,
}
impl Default for BridgeOptions {
    fn default() -> Self {
        Self {
            address: SocketAddr::from(([127, 0, 0, 1], super::DEFAULT_BRIDGE_PORT)),
            open_browser: false,
            browser: None,
        }
    }
}

struct Browser {
    generation: String,
    outgoing: mpsc::Sender<String>,
    replace: watch::Sender<bool>,
}
struct Pending {
    generation: String,
    result: oneshot::Sender<Result<Value, BridgeError>>,
}
struct Inner {
    browser: Option<Browser>,
    pending: HashMap<String, Pending>,
}
struct Shared {
    address: SocketAddr,
    session: String,
    token: Zeroizing<String>,
    inner: Mutex<Inner>,
    connections: Arc<Semaphore>,
    stop: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// Loopback HTTP/WebSocket bridge, compatible with Python clients and page.
///
/// Tokens remain outside the served page. Browser connections require the
/// session in the URL, client connections require the private token file.
#[doc(alias = "bittensor.extension.BridgeServer")]
pub struct BridgeServer {
    options: BridgeOptions,
    shared: Option<Arc<Shared>>,
    task: Option<JoinHandle<Result<(), BridgeError>>>,
}

#[cfg(test)]
pub(super) struct TaskProbe(Arc<Shared>);
#[cfg(test)]
impl TaskProbe {
    pub(super) fn all_finished(&self) -> bool {
        self.0
            .tasks
            .lock()
            .is_ok_and(|tasks| tasks.iter().all(JoinHandle::is_finished))
    }
}
impl std::fmt::Debug for BridgeServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeServer")
            .field("address", &self.address())
            .field("started", &self.task.is_some())
            .finish()
    }
}
impl BridgeServer {
    #[cfg(test)]
    pub(super) fn task_probe(&self) -> Result<TaskProbe, BridgeError> {
        Ok(TaskProbe(
            self.shared
                .as_ref()
                .ok_or_else(|| error("bridge not started"))?
                .clone(),
        ))
    }
    /// Construct an inert server, refusing non-loopback addresses.
    pub fn new(address: SocketAddr) -> Result<Self, BridgeError> {
        if !address.ip().is_loopback() {
            return Err(error("extension bridge must bind a loopback address"));
        }
        Ok(Self {
            options: BridgeOptions {
                address,
                ..BridgeOptions::default()
            },
            shared: None,
            task: None,
        })
    }
    /// Current listener address, including the allocated ephemeral port.
    pub fn address(&self) -> SocketAddr {
        self.shared
            .as_ref()
            .map_or(self.options.address, |s| s.address)
    }
    /// URL of the embedded page, without its session capability.
    pub fn http_url(&self) -> String {
        super::bridge_http_url(self.address())
    }
    /// URL for authenticated WebSocket clients.
    pub fn ws_url(&self) -> String {
        super::bridge_ws_url(self.address())
    }
    /// Active session identifier. It is created only when starting the server.
    pub fn session_id(&self) -> Option<&str> {
        self.shared.as_ref().map(|s| s.session.as_str())
    }
    /// Token for a trusted local client or a mode-0600 token file.
    pub fn client_token(&self) -> Option<&str> {
        self.shared.as_ref().map(|s| s.token.as_str())
    }
    /// Session-scoped browser page URL; absent until the listener starts.
    pub fn page_url(&self) -> Option<String> {
        self.session_id()
            .map(|id| super::bridge_page_url(self.address(), id))
    }

    /// Start the server. This operation never starts a browser or daemon.
    pub async fn start(&mut self) -> Result<(), BridgeError> {
        if self.task.is_some() {
            return Ok(());
        }
        let listener = TcpListener::bind(self.options.address)
            .await
            .map_err(|_| error("cannot bind the loopback extension bridge"))?;
        let address = listener
            .local_addr()
            .map_err(|_| error("cannot read bridge listener address"))?;
        let (stop, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            address,
            session: random_id()?,
            token: Zeroizing::new(random_id()?),
            inner: Mutex::new(Inner {
                browser: None,
                pending: HashMap::new(),
            }),
            connections: Arc::new(Semaphore::new(CONNECTION_LIMIT)),
            stop,
            tasks: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/", get(page))
            .route("/index.html", get(page))
            .route("/ws", get(upgrade))
            .with_state(shared.clone());
        let mut shutdown = shared.stop.subscribe();
        self.task = Some(tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await
            .map_err(|_| error("extension bridge listener failed"))
        }));
        self.shared = Some(shared);
        Ok(())
    }

    /// Wait for a browser tab to authenticate, with a bounded caller deadline.
    pub async fn wait_until_ready(&self, wait: Duration) -> Result<(), BridgeError> {
        let shared = self
            .shared
            .as_ref()
            .ok_or_else(|| error("extension bridge has not started"))?;
        timeout(wait, async {
            loop {
                if *shared.stop.borrow() {
                    return Err(error("extension bridge stopped"));
                }
                if lock(&shared.inner)?.browser.is_some() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| error("extension bridge page is not connected"))?
    }

    /// Stop accepting work and cancel sessions, with five-second teardown phases.
    pub async fn stop(&mut self) -> Result<(), BridgeError> {
        if let Some(shared) = self.shared.take() {
            shutdown(&shared);
            let tasks = lock(&shared.tasks)?.drain(..).collect::<Vec<_>>();
            let _ = timeout(Duration::from_secs(5), async move {
                for task in tasks {
                    let _ = task.await;
                }
            })
            .await;
        }
        if let Some(mut task) = self.task.take() {
            match timeout(Duration::from_secs(5), &mut task).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(error("bridge server task failed")),
                Err(_) => {
                    task.abort();
                    Err(error("bridge server shutdown timed out"))
                }
            }
        } else {
            Ok(())
        }
    }
}
impl Drop for BridgeServer {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            shutdown(&shared);
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Start a server with explicitly chosen browser-opening behavior.
#[doc(alias = "bittensor.extension.run_bridge")]
pub async fn run_bridge(options: &BridgeOptions) -> Result<BridgeServer, BridgeError> {
    let mut server = BridgeServer::new(options.address)?;
    server.start().await?;
    if options.open_browser
        && let Some(url) = server.page_url()
    {
        super::session::open_bridge_page(&url, options.browser.as_deref()).await?;
    }
    Ok(server)
}

fn shutdown(shared: &Shared) {
    shared.stop.send_replace(true);
    if let Ok(tasks) = shared.tasks.lock() {
        for task in tasks.iter() {
            task.abort();
        }
    }
    if let Ok(mut inner) = shared.inner.lock() {
        inner.browser = None;
        for (_, pending) in inner.pending.drain() {
            let _ = pending.result.send(Err(error("extension bridge stopped")));
        }
    }
}

fn own_host(headers: &HeaderMap, address: SocketAddr) -> bool {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()) == Some(address.to_string().as_str())
}
fn origin_allowed(headers: &HeaderMap, address: SocketAddr) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin.to_str().ok() == Some(format!("http://{address}").as_str()),
    }
}
async fn page(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    if !own_host(&headers, shared.address) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = Response::new(Body::from(BRIDGE_HTML));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "referrer-policy",
        header::HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", header::HeaderValue::from_static("DENY"));
    response
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
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.eq_ignore_ascii_case(expected))
    };
    let connection_upgrade = headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| {
            s.split(',')
                .any(|s| s.trim().eq_ignore_ascii_case("upgrade"))
        });
    let key = headers
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
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
    // Hold this lock until the new task is recorded, serializing shutdown with
    // its creation. Completed handles are pruned to keep lifetime memory bounded.
    let Ok(mut tasks) = tracked.tasks.lock() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if *tracked.stop.borrow() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    tasks.retain(|task| !task.is_finished());
    let task = tokio::spawn(async move {
        let _permit = permit;
        if let Ok(Ok(upgraded)) = timeout(Duration::from_secs(5), upgrade).await {
            let socket = WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                Some(
                    WebSocketConfig::default()
                        .max_message_size(Some(MESSAGE_LIMIT))
                        .max_frame_size(Some(MESSAGE_LIMIT)),
                ),
            )
            .await;
            let mut stop = shared.stop.subscribe();
            if *stop.borrow() {
                return;
            }
            tokio::select! { _=connection(socket,shared.clone())=>{}, _=stop.changed()=>{} }
        }
    });
    tasks.push(task);
    match Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade")
        .header("sec-websocket-accept", accept)
        .body(Body::empty())
    {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
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
async fn connection(mut socket: Socket, shared: Arc<Shared>) {
    let Ok(Ok(mut hello)) = timeout(Duration::from_secs(10), receive_json(&mut socket)).await
    else {
        return;
    };
    let role = hello.get("role").and_then(Value::as_str).map(str::to_owned);
    match role.as_deref() {
        Some("bridge") => {
            if hello.get("session").and_then(Value::as_str) != Some(shared.session.as_str()) {
                close(&mut socket, CloseCode::Policy, "stale bridge session").await;
                return;
            }
            browser(socket, shared).await;
        }
        Some("client") => {
            let token = hello.get_mut("token").map(Value::take).and_then(|value| {
                if let Value::String(token) = value {
                    Some(Zeroizing::new(token))
                } else {
                    None
                }
            });
            let accepted = token.as_ref().is_some_and(|token| {
                constant_time_equal(token.as_bytes(), shared.token.as_bytes())
            });
            if !accepted {
                close(&mut socket, CloseCode::Policy, "unauthorized client").await;
                return;
            }
            if send_text(&mut socket, json!({"role":"ok"}).to_string())
                .await
                .is_err()
            {
                return;
            }
            client(socket, shared).await;
        }
        _ => close(&mut socket, CloseCode::Policy, "unknown role").await,
    }
}
fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    bool::from(a.ct_eq(b))
}

async fn browser(mut socket: Socket, shared: Arc<Shared>) {
    let Ok(generation) = random_id() else {
        return;
    };
    let (outgoing, mut incoming) = mpsc::channel(PENDING_LIMIT);
    let (replace, mut replaced) = watch::channel(false);
    if let Ok(mut inner) = shared.inner.lock() {
        if let Some(previous) = inner.browser.take() {
            previous.replace.send_replace(true);
        }
        fail_pending(&mut inner, None);
        inner.browser = Some(Browser {
            generation: generation.clone(),
            outgoing,
            replace,
        });
    } else {
        return;
    }
    loop {
        tokio::select! {
            _=replaced.changed()=>{
                close(&mut socket,CloseCode::Restart,"replaced by new bridge tab").await;
                break;
            },
            request=incoming.recv()=>{
                let Some(request)=request else{break;};
                if send_text(&mut socket,request).await.is_err(){break;}
            },
            reply=socket.next()=>{
                let Some(Ok(message))=reply else{break;};
                match message {
                    Message::Ping(_)|Message::Pong(_)=>continue,
                    Message::Text(text)=>{
                        let Ok(reply)=bounded_json(text.as_bytes()) else{break;};
                        let Some(id)=reply.get("id").and_then(Value::as_str) else{continue;};
                        if let Ok(mut inner)=shared.inner.lock()
                            && inner.pending.get(id).is_some_and(|p|p.generation==generation)
                            && let Some(pending)=inner.pending.remove(id) {
                                    let result=if let Some(value)=reply.get("error"){Err(super::wire_error(value))}
                                        else{Ok(reply.get("result").cloned().unwrap_or(Value::Null))};
                                    let _=pending.result.send(result);
                        }
                    },
                    _=>break,
                }
            },
        }
    }
    if let Ok(mut inner) = shared.inner.lock() {
        if inner
            .browser
            .as_ref()
            .is_some_and(|browser| browser.generation == generation)
        {
            inner.browser = None;
        }
        fail_pending(&mut inner, Some(&generation));
    }
}
fn fail_pending(inner: &mut Inner, generation: Option<&str>) {
    inner
        .pending
        .retain(|_, pending| generation.is_some_and(|g| pending.generation != g));
    // Dropping the sender wakes each receiver. The receiver converts closure to
    // the same bounded disconnect error; no stale tab can affect its replacement.
}

struct PendingGuard {
    shared: Arc<Shared>,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.shared.inner.lock() {
            inner.pending.remove(&self.id);
        }
    }
}
async fn forward(
    shared: Arc<Shared>,
    mut request: Value,
    wait: Duration,
) -> Result<Value, BridgeError> {
    let id = random_id()?;
    request["id"] = Value::String(id.clone());
    let text = request.to_string();
    if text.len() > MESSAGE_LIMIT {
        return Err(error("bridge message exceeds the size limit"));
    }
    let (sender, receiver) = oneshot::channel();
    {
        let mut inner = lock(&shared.inner)?;
        if inner.pending.len() >= PENDING_LIMIT {
            return Err(error("too many pending bridge requests"));
        }
        let browser = inner.browser.as_ref().ok_or_else(|| {
            error("extension bridge page is not connected; a browser tab should open automatically")
        })?;
        let generation = browser.generation.clone();
        let outgoing = browser.outgoing.clone();
        inner.pending.insert(
            id.clone(),
            Pending {
                generation,
                result: sender,
            },
        );
        if outgoing.try_send(text).is_err() {
            inner.pending.remove(&id);
            return Err(error("browser bridge request queue is full"));
        }
    }
    let _guard = PendingGuard { shared, id };
    timeout(wait, receiver)
        .await
        .map_err(|_| error("extension bridge request timed out"))?
        .map_err(|_| error("bridge page disconnected"))?
}
async fn status(shared: Arc<Shared>) -> Result<Value, BridgeError> {
    let connected = lock(&shared.inner)?.browser.is_some();
    let count = if connected {
        forward(
            shared.clone(),
            json!({"method":"accounts.list","params":{}}),
            Duration::from_secs(30),
        )
        .await
        .ok()
        .and_then(|value| {
            value
                .get("accounts")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0)
    } else {
        0
    };
    Ok(
        json!({"browser_connected":connected,"account_count":count,"http_url":super::bridge_http_url(shared.address),"session_id":shared.session}),
    )
}
async fn client(mut socket: Socket, shared: Arc<Shared>) {
    loop {
        let Ok(request) = receive_json(&mut socket).await else {
            break;
        };
        let Some(id) = request
            .get("id")
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128))
            .cloned()
        else {
            continue;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        let result = if method == "bridge.status" {
            status(shared.clone()).await
        } else if matches!(
            method,
            "accounts.list" | "extrinsic.sign" | "bytes.sign" | "transaction.result"
        ) {
            forward(shared.clone(), request, REQUEST_TIMEOUT).await
        } else {
            Err(error("unsupported bridge method"))
        };
        let reply = match result {
            Ok(result) => json!({"id":id,"result":result}),
            Err(error) => json!({"id":id,"error":{"message":error.to_string()}}),
        };
        if send_text(&mut socket, reply.to_string()).await.is_err() {
            break;
        }
    }
}
