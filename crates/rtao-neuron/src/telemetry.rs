//! Process health and bounded Prometheus HTTP exposition.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
#[cfg(feature = "prometheus")]
#[path = "telemetry/recorder.rs"]
mod recorder;

/// Stderr logging format selected explicitly by a neuron binary.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum LogFormat {
    /// Single-line operational logs with key-value fields.
    #[default]
    Compact,
    /// JSON objects suitable for structured log collection.
    Json,
    /// Multiline logs suitable for interactive development.
    Pretty,
}

/// Install stderr tracing in a binary; libraries leave subscriber ownership to callers.
pub fn init_tracing(level: Option<crate::LogLevel>, format: LogFormat) -> anyhow::Result<()> {
    use tracing_subscriber::filter::LevelFilter;
    let level = match level.unwrap_or(crate::LogLevel::Info) {
        crate::LogLevel::Off => LevelFilter::OFF,
        crate::LogLevel::Error => LevelFilter::ERROR,
        crate::LogLevel::Warn => LevelFilter::WARN,
        crate::LogLevel::Info => LevelFilter::INFO,
        crate::LogLevel::Debug => LevelFilter::DEBUG,
        crate::LogLevel::Trace => LevelFilter::TRACE,
    };
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr);
    let result = match format {
        LogFormat::Compact => subscriber.compact().try_init(),
        LogFormat::Json => subscriber.json().try_init(),
        LogFormat::Pretty => subscriber.pretty().try_init(),
    };
    result.map_err(|error| anyhow::anyhow!("cannot initialize stderr tracing: {error}"))
}

/// Conditions required before the validator can safely accept scoring work.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Readiness {
    /// The chain transport has not terminated.
    pub connected: bool,
    /// Required call surfaces are verified against current metadata.
    pub runtime_verified: bool,
    /// The hotkey is registered in the configured subnet.
    pub registered: bool,
    /// The last-good graph is younger than twice its refresh cadence.
    pub metagraph_fresh: bool,
    /// The most recent weight attempt did not fail.
    pub last_weight_succeeded: bool,
    /// User-step errors and panics since the last successful step.
    pub consecutive_step_failures: u32,
}
impl Readiness {
    /// All readiness conditions hold; five consecutive step failures pause readiness.
    pub fn is_ready(&self) -> bool {
        self.connected
            && self.runtime_verified
            && self.registered
            && self.metagraph_fresh
            && self.last_weight_succeeded
            && self.consecutive_step_failures < 5
    }
}

#[derive(Default)]
struct Inner {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
    ready: Readiness,
}

/// Shared counters and readiness without a process-wide recorder installation.
#[derive(Clone, Default)]
pub struct Metrics(Arc<Mutex<Inner>>);
impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics")
            .field("ready", &self.readiness())
            .finish_non_exhaustive()
    }
}
impl Metrics {
    /// Install this process recorder explicitly before connecting the chain client.
    ///
    /// Binaries own the global recorder. An embedded runtime can instead reuse
    /// its host's recorder and export the framework's retained counters separately.
    #[cfg(feature = "prometheus")]
    pub fn install_recorder(&self) -> anyhow::Result<()> {
        metrics::set_global_recorder(recorder::Recorder(self.clone()))
            .map_err(|error| anyhow::anyhow!("cannot install process metrics recorder: {error}"))
    }
    /// Increment a controlled metric name, optionally containing Prometheus labels.
    pub fn increment(&self, name: &str) {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let value = inner.counters.entry(name.to_owned()).or_default();
        *value = value.saturating_add(1);
    }
    /// Publish a finite gauge; invalid samples are omitted.
    pub fn gauge(&self, name: &str, value: f64) {
        if value.is_finite() {
            self.0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .gauges
                .insert(name.to_owned(), value);
        }
    }
    /// Atomically update readiness conditions.
    pub fn update_readiness(&self, update: impl FnOnce(&mut Readiness)) {
        update(&mut self.0.lock().unwrap_or_else(|p| p.into_inner()).ready);
    }
    /// Read one consistent readiness snapshot.
    pub fn readiness(&self) -> Readiness {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .ready
            .clone()
    }
    /// Render the retained process metrics in deterministic Prometheus text format.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = String::new();
        for (name, value) in &inner.counters {
            let _ = writeln!(out, "{name} {value}");
        }
        for (name, value) in &inner.gauges {
            let _ = writeln!(out, "{name} {value}");
        }
        let _ = writeln!(out, "rtao_ready {}", u8::from(inner.ready.is_ready()));
        out
    }
    /// Bind an optional metrics listener; the runtime supervises the returned future.
    pub async fn bind(&self, addr: SocketAddr) -> std::io::Result<TelemetryServer> {
        Ok(TelemetryServer {
            listener: TcpListener::bind(addr).await?,
            metrics: self.clone(),
        })
    }
}

/// Bounded HTTP server for `/metrics`, `/healthz` and `/readyz`.
pub struct TelemetryServer {
    listener: TcpListener,
    metrics: Metrics,
}
impl TelemetryServer {
    /// Address actually bound, useful when selecting an ephemeral test port.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
    /// Serve at most 64 concurrent requests and drain for at most two seconds on shutdown.
    pub async fn run(self, shutdown: CancellationToken) -> std::io::Result<()> {
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => {},
                accepted = self.listener.accept(), if tasks.len() < 64 => {
                    let (socket, _) = accepted?;
                    let metrics = self.metrics.clone();
                    tasks.spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(5), request(socket, metrics)).await;
                    });
                }
            }
        }
        if tokio::time::timeout(Duration::from_secs(2), async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tasks.abort_all();
        }
        Ok(())
    }
}
async fn request(mut socket: TcpStream, metrics: Metrics) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let mut used = 0;
    loop {
        let read = socket.read(&mut buffer[used..]).await?;
        if read == 0 {
            return Ok(());
        }
        used += read;
        if buffer[..used]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
        if used == buffer.len() {
            return response(
                &mut socket,
                "431 Request Header Fields Too Large",
                "text/plain",
                "headers too large\n",
            )
            .await;
        }
    }
    let first = std::str::from_utf8(&buffer[..used])
        .ok()
        .and_then(|text| text.lines().next())
        .unwrap_or("");
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or("");
    let path = words.next().unwrap_or("");
    let (status, content_type, body) = if method != "GET" {
        (
            "405 Method Not Allowed",
            "text/plain",
            "GET required\n".to_owned(),
        )
    } else {
        match path {
            "/healthz" => ("200 OK", "text/plain", "alive\n".to_owned()),
            "/readyz" if metrics.readiness().is_ready() => {
                ("200 OK", "text/plain", "ready\n".to_owned())
            }
            "/readyz" => (
                "503 Service Unavailable",
                "text/plain",
                "not ready\n".to_owned(),
            ),
            "/metrics" => ("200 OK", "text/plain; version=0.0.4", metrics.render()),
            _ => ("404 Not Found", "text/plain", "not found\n".to_owned()),
        }
    };
    response(&mut socket, status, content_type, &body).await
}
async fn response(
    socket: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn health_ready_and_metrics_are_separate_and_shutdown_is_bounded() -> anyhow::Result<()> {
        let metrics = Metrics::default();
        metrics.increment("rtao_step_errors_total");
        let server = metrics.bind("127.0.0.1:0".parse()?).await?;
        let addr = server.local_addr()?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(server.run(cancel.clone()));
        async fn get(addr: SocketAddr, path: &str) -> anyhow::Result<String> {
            let mut socket = TcpStream::connect(addr).await?;
            socket
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .await?;
            let mut bytes = Vec::new();
            socket.read_to_end(&mut bytes).await?;
            Ok(String::from_utf8(bytes)?)
        }
        assert!(get(addr, "/healthz").await?.starts_with("HTTP/1.1 200"));
        assert!(get(addr, "/readyz").await?.starts_with("HTTP/1.1 503"));
        metrics.update_readiness(|ready| {
            *ready = Readiness {
                connected: true,
                runtime_verified: true,
                registered: true,
                metagraph_fresh: true,
                last_weight_succeeded: true,
                consecutive_step_failures: 4,
            }
        });
        assert!(get(addr, "/readyz").await?.starts_with("HTTP/1.1 200"));
        metrics.update_readiness(|ready| ready.consecutive_step_failures = 5);
        assert!(get(addr, "/readyz").await?.starts_with("HTTP/1.1 503"));
        assert!(
            get(addr, "/metrics")
                .await?
                .contains("rtao_step_errors_total 1\n")
        );
        cancel.cancel();
        task.await??;
        Ok(())
    }
}
