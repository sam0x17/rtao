//! Raw HTTP/1.1 fixture: observations precede response delays and connection faults.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use http::{HeaderMap, HeaderValue, Method, header::HeaderName};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Notify},
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::TlsAcceptor;
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Handler = dyn Fn(usize, &Observed) -> Response + Send + Sync;

#[derive(Clone, Debug)]
pub struct Observed {
    pub method: Method,
    pub target: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
    pub body_delay: Duration,
    pub drop_connection: bool,
}

impl Response {
    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.into(),
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
            drop_connection: false,
        }
    }

    pub fn delayed(body: impl Into<Vec<u8>>, delay: Duration) -> Self {
        Self {
            delay,
            ..Self::json(body)
        }
    }

    pub fn disconnect() -> Self {
        Self {
            drop_connection: true,
            ..Self::json(Vec::new())
        }
    }
}

#[derive(Default)]
struct State {
    observations: Mutex<Vec<Observed>>,
    errors: Mutex<Vec<String>>,
    changed: Notify,
    active: AtomicUsize,
    peak: AtomicUsize,
}

struct Active(Arc<State>);

impl Active {
    fn new(state: Arc<State>) -> Self {
        let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
        state.peak.fetch_max(active, Ordering::SeqCst);
        Self(state)
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Server {
    pub base_url: Url,
    pub root: Option<CertificateDer<'static>>,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl Server {
    pub async fn http(
        handler: impl Fn(usize, &Observed) -> Response + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::bind(false, Arc::new(handler)).await
    }

    pub async fn https(
        handler: impl Fn(usize, &Observed) -> Response + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::bind(true, Arc::new(handler)).await
    }

    async fn bind(tls: bool, handler: Arc<Handler>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (acceptor, root) = if tls {
            let certified =
                rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
            let certificate = certified.cert.der().clone();
            let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                certified.signing_key.serialize_der(),
            ));
            let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], private)?;
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            (Some(TlsAcceptor::from(Arc::new(config))), Some(certificate))
        } else {
            (None, None)
        };
        let state = Arc::new(State::default());
        let serving = state.clone();
        let task = tokio::spawn(async move {
            // The JoinSet owns connections; aborting this listener aborts its children.
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = match accepted {
                            Ok(pair) => pair,
                            Err(error) => {
                                serving.errors.lock().await.push(error.to_string());
                                break;
                            }
                        };
                        let state = serving.clone();
                        let handler = handler.clone();
                        let acceptor = acceptor.clone();
                        connections.spawn(async move {
                            let result = if let Some(acceptor) = acceptor {
                                match acceptor.accept(socket).await {
                                    Ok(socket) => serve(socket, &state, &handler).await,
                                    // An untrusted-root test deliberately aborts its handshake.
                                    Err(_) => return,
                                }
                            } else {
                                serve(socket, &state, &handler).await
                            };
                            if let Err(error) = result {
                                state.errors.lock().await.push(error.to_string());
                                state.changed.notify_waiters();
                            }
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Ok(Self {
            base_url: Url::parse(&format!("{}://{addr}/", if tls { "https" } else { "http" }))?,
            root,
            state,
            task,
        })
    }

    pub async fn observations(&self) -> Result<Vec<Observed>> {
        let errors = self.state.errors.lock().await;
        if !errors.is_empty() {
            return Err(io::Error::other(errors.join("; ")).into());
        }
        Ok(self.state.observations.lock().await.clone())
    }

    pub async fn wait_for(&self, count: usize) -> Result<Vec<Observed>> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let notified = self.state.changed.notified();
                // Register before inspecting, avoiding a notification/check race.
                tokio::pin!(notified);
                notified.as_mut().enable();
                let observations = self.observations().await?;
                if observations.len() >= count {
                    return Ok(observations);
                }
                notified.await;
            }
        })
        .await?
    }

    pub fn peak(&self) -> usize {
        self.state.peak.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve<S>(mut socket: S, state: &Arc<State>, handler: &Arc<Handler>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let observation =
        tokio::time::timeout(Duration::from_secs(3), read_request(&mut socket)).await??;
    let active = Active::new(state.clone());
    let index = {
        let mut observations = state.observations.lock().await;
        let index = observations.len();
        observations.push(observation.clone());
        index
    };
    state.changed.notify_waiters();
    let response = handler(index, &observation);
    if response.drop_connection {
        return Ok(());
    }
    tokio::time::sleep(response.delay).await;
    let mut bytes = format!(
        "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    )
    .into_bytes();
    for (name, value) in response.headers {
        bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    // A request timeout can close the peer while a delayed response is pending.
    let _ = socket.write_all(&bytes).await;
    let _ = socket.flush().await;
    tokio::time::sleep(response.body_delay).await;
    let _ = socket.write_all(&response.body).await;
    let _ = socket.shutdown().await;
    drop(active);
    Ok(())
}

async fn read_request<S: AsyncRead + Unpin>(socket: &mut S) -> Result<Observed> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::other("fixture request headers exceed bound").into());
        }
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            return Err(io::Error::other("connection ended before request headers").into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    let text = std::str::from_utf8(&bytes[..header_end])?;
    let mut lines = text.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(|| io::Error::other("missing request line"))?;
    let mut words = first.split(' ');
    let method = Method::from_bytes(
        words
            .next()
            .ok_or_else(|| io::Error::other("missing method"))?
            .as_bytes(),
    )?;
    let target = words
        .next()
        .ok_or_else(|| io::Error::other("missing target"))?
        .to_owned();
    if words.next() != Some("HTTP/1.1") || words.next().is_some() {
        return Err(io::Error::other("unexpected request-line shape").into());
    }
    let mut headers = HeaderMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("malformed header"))?;
        headers.append(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value.trim())?,
        );
    }
    if headers.contains_key(http::header::TRANSFER_ENCODING) {
        return Err(
            io::Error::other("fixture expects a materialized body with content-length").into(),
        );
    }
    let length: usize = headers
        .get(http::header::CONTENT_LENGTH)
        .map(|value| value.to_str())
        .transpose()?
        .unwrap_or("0")
        .parse()?;
    if length > 1024 * 1024 {
        return Err(io::Error::other("fixture request body exceeds bound").into());
    }
    let total = header_end + length;
    while bytes.len() < total {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            return Err(io::Error::other("connection ended before request body").into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(Observed {
        method,
        target,
        headers,
        body: bytes[header_end..total].to_vec(),
    })
}
