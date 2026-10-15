// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/session.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/tokens.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/daemon.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Instant, timeout},
};
use zeroize::Zeroizing;

use super::{BridgeClient, BridgeError, BridgeOptions, BridgeServer, TEXT_LIMIT, error};

/// Python-compatible locations of the bridge's private token and PID files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgePaths {
    /// Mode-0600 local client authentication token.
    pub token_file: PathBuf,
    /// Daemon PID journal; never sufficient on its own to authorize a signal.
    pub pid_file: PathBuf,
}
impl BridgePaths {
    /// Publish a running server's private token and this process's PID.
    /// The returned guard removes only its unchanged files on drop. This is
    /// explicit for foreground CLI servers as well as daemon processes.
    pub fn publish(&self, server: &BridgeServer) -> Result<BridgeFiles, BridgeError> {
        #[cfg(unix)]
        {
            let paths = Self {
                token_file: resolved_state_path(&self.token_file)?,
                pid_file: resolved_state_path(&self.pid_file)?,
            };
            if paths.token_file == paths.pid_file {
                return Err(error("bridge token and PID files must be different paths"));
            }
            let token = Zeroizing::new(
                server
                    .client_token()
                    .ok_or_else(|| error("bridge has not started"))?
                    .to_owned(),
            );
            let file = open_file(&paths.pid_file, true)?
                .ok_or_else(|| error("cannot reserve bridge PID file"))?;
            let reservation =
                nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
                    .map_err(|_| error("another bridge owns the state files"))?;
            // Nonblocking contenders can have opened an inode just before its
            // former owner removed it. Never publish under that orphaned lock.
            use std::os::unix::fs::MetadataExt;
            let held = reservation
                .metadata()
                .map_err(|_| error("cannot inspect bridge reservation"))?;
            let named = fs::symlink_metadata(&paths.pid_file)
                .map_err(|_| error("bridge reservation path changed"))?;
            if held.dev() != named.dev() || held.ino() != named.ino() || held.nlink() != 1 {
                return Err(error("bridge reservation path changed"));
            }
            let existing = read_private(&paths.pid_file, false)?;
            if existing.as_ref().is_some_and(|bytes| !bytes.is_empty())
                && let Some(pid) = read_pid(&paths)?
                && process_alive(pid)
            {
                return Err(error(
                    "bridge state files belong to a live daemon; refusing to overwrite them",
                ));
            }
            let guard = BridgeFiles {
                paths,
                token,
                pid: std::process::id().to_string(),
                _reservation: reservation,
            };
            write_private(&guard.paths.token_file, guard.token.as_bytes())?;
            write_private(&guard.paths.pid_file, guard.pid.as_bytes())?;
            Ok(guard)
        }
        #[cfg(not(unix))]
        {
            let _ = server;
            Err(error(
                "private bridge state publication is supported on Unix only",
            ))
        }
    }
    /// Resolve Python's environment overrides, otherwise `~/.bittensor/*`.
    /// This function reads environment variables but does not touch the files.
    pub fn from_environment() -> Result<Self, BridgeError> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| error("HOME is unavailable; configure explicit bridge paths"))?;
        Ok(Self::from_environment_with(&home, |name| {
            std::env::var_os(name)
        }))
    }
    /// Resolve injected environment overrides without filesystem access.
    /// Empty values select the supplied home; relative paths and literal
    /// tildes remain unchanged, matching Python's `Path` construction.
    pub fn from_environment_with(
        home: &Path,
        environment: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Self {
        let configured = |name: &str, filename: &str| {
            environment(name)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".bittensor").join(filename))
        };
        Self {
            token_file: configured("BITTENSOR_EXTENSION_BRIDGE_TOKEN", "extension_bridge.token"),
            pid_file: configured("BITTENSOR_EXTENSION_BRIDGE_PID", "extension_bridge.pid"),
        }
    }
    /// Read a bounded private token without following a final-component symlink.
    pub fn read_token(&self) -> Result<Option<Zeroizing<String>>, BridgeError> {
        let Some(bytes) = read_private(&self.token_file, true)? else {
            return Ok(None);
        };
        let token = Zeroizing::new(
            std::str::from_utf8(&bytes)
                .map_err(|_| error("bridge token is not UTF-8"))?
                .to_owned(),
        );
        // Python's str.strip includes the ASCII information separators in
        // addition to Rust's Unicode whitespace set.
        let stripped = token.trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
        });
        if stripped.is_empty() {
            return Ok(None);
        }
        if stripped.chars().any(char::is_control) {
            return Err(error("invalid bridge token file"));
        }
        Ok(Some(Zeroizing::new(stripped.to_owned())))
    }
}

/// Observe the session page and last status while extension authorization is pending.
///
/// The page carries its browser session capability, never the private client token.
/// The callback runs before an optional browser launch and at most every five seconds.
pub type BridgeWaiting = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// Explicit permission and deadlines for finding or starting a bridge.
#[derive(Clone)]
pub struct EnsureBridgeOptions {
    /// Numeric loopback HTTP base URL or WebSocket URL ending in `/ws`.
    pub url: String,
    /// Python-compatible files to reuse for authentication and daemon ownership.
    pub paths: BridgePaths,
    /// Explicit permission to spawn `rtao extension bridge --daemon` if absent.
    pub allow_spawn: bool,
    /// Explicit permission to open the browser's session-scoped page.
    pub open_browser: bool,
    /// Optional chosen browser executable or macOS application.
    pub browser: Option<String>,
    /// Restart an authenticated, verified daemon before selecting accounts.
    pub fresh: bool,
    /// Startup polling deadline, at most 60 seconds.
    pub startup_timeout: Duration,
    /// Browser/extension readiness deadline, at most five minutes.
    pub extension_timeout: Duration,
    /// Optional progress observer for manual browser authorization.
    pub on_waiting: Option<BridgeWaiting>,
}
impl std::fmt::Debug for EnsureBridgeOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnsureBridgeOptions")
            .field("url", &self.url)
            .field("paths", &self.paths)
            .field("allow_spawn", &self.allow_spawn)
            .field("open_browser", &self.open_browser)
            .field("browser", &self.browser)
            .field("fresh", &self.fresh)
            .field("startup_timeout", &self.startup_timeout)
            .field("extension_timeout", &self.extension_timeout)
            .field("on_waiting", &self.on_waiting.is_some())
            .finish()
    }
}
impl EnsureBridgeOptions {
    /// Resolve environment paths with all process-launch permissions disabled.
    pub fn from_environment() -> Result<Self, BridgeError> {
        Ok(Self {
            url: bridge_ws_url(SocketAddr::from((
                [127, 0, 0, 1],
                super::DEFAULT_BRIDGE_PORT,
            ))),
            paths: BridgePaths::from_environment()?,
            allow_spawn: false,
            open_browser: false,
            browser: None,
            fresh: false,
            startup_timeout: Duration::from_secs(15),
            extension_timeout: Duration::from_secs(180),
            on_waiting: None,
        })
    }
}

/// Plain HTTP page URL for a loopback listener.
pub fn bridge_http_url(address: SocketAddr) -> String {
    format!("http://{address}/")
}
/// WebSocket RPC URL for a loopback listener.
pub fn bridge_ws_url(address: SocketAddr) -> String {
    format!("ws://{address}/ws")
}
/// Browser page URL carrying the daemon's session capability, never its token.
pub fn bridge_page_url(address: SocketAddr, session_id: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("session", session_id)
        .finish();
    format!("{}?{query}", bridge_http_url(address))
}

async fn bridge_status(url: &str, paths: &BridgePaths) -> Result<Value, BridgeError> {
    let token = paths
        .read_token()?
        .ok_or_else(|| error("extension bridge client token is missing"))?;
    let client = BridgeClient::new(url, token.as_str())?;
    let result = timeout(
        Duration::from_secs(35),
        client.request("bridge.status", json!({})),
    )
    .await
    .map_err(|_| error("extension bridge status timed out"))?;
    client.close().await;
    result
}

/// Reuse a compatible Rust or Python daemon, or explicitly start one if allowed.
///
/// The returned URL is authenticated and has at least one authorized extension
/// account. Construction of a signer never calls this function implicitly.
#[doc(alias = "bittensor.extension.ensure_bridge")]
pub async fn ensure_bridge(options: &EnsureBridgeOptions) -> Result<String, BridgeError> {
    let url = super::client::session_url(&options.url)?;
    if options.startup_timeout > Duration::from_secs(60)
        || options.extension_timeout > Duration::from_secs(300)
    {
        return Err(error("bridge readiness deadlines exceed their limits"));
    }
    let address = super::client::target(&url)?;
    if options.fresh {
        if !options.allow_spawn {
            return Err(error(
                "fresh bridge sessions require explicit daemon-spawn permission",
            ));
        }
        // A foreground bridge is explicitly owned by its terminal. Reuse that
        // authenticated server without signalling it; only restart a verified
        // background daemon. Python daemon argv remains supported.
        if let Some(pid) = read_pid(&options.paths)?
            && process_alive(pid)
            && verify_process(pid, &options.paths.pid_file).await.is_ok()
        {
            stop_bridge_daemon(&options.paths, url.as_str()).await?;
        }
    }
    let mut status = timeout(
        options.startup_timeout.min(Duration::from_secs(5)),
        bridge_status(url.as_str(), &options.paths),
    )
    .await
    .ok()
    .and_then(Result::ok);
    if status.is_none() {
        if !options.allow_spawn {
            return Err(error(
                "extension bridge is unavailable; explicitly enable daemon startup or run rtao extension bridge",
            ));
        }
        if url.scheme() != "ws" {
            return Err(error(
                "automatic bridge startup supports plain loopback ws only",
            ));
        }
        let startup = Instant::now() + options.startup_timeout;
        if options.startup_timeout.is_zero() {
            return Err(error("extension bridge startup deadline has expired"));
        }
        timeout(
            options.startup_timeout,
            start_bridge_daemon(&options.paths, address),
        )
        .await
        .map_err(|_| error("extension bridge startup timed out"))??;
        loop {
            let remaining = startup.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(error("extension bridge did not start before the deadline"));
            }
            status = timeout(remaining, bridge_status(url.as_str(), &options.paths))
                .await
                .ok()
                .and_then(Result::ok);
            if status.is_some() {
                break;
            }
            tokio::time::sleep(
                Duration::from_millis(150).min(startup.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
    let mut status = status.ok_or_else(|| error("extension bridge did not return status"))?;
    let session = status
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| error("extension bridge returned no session id"))?;
    let page = bridge_page_url(address, session);
    if ready(&status) {
        return Ok(url.into());
    }
    let mut notice = WaitingNotice::new(options.on_waiting.as_ref());
    notice.notify(&page, &status, Instant::now());
    if options.open_browser {
        open_bridge_page(&page, options.browser.as_deref())
            .await
            .map_err(|failure| launch_page_error(&page, failure))?;
    }
    let deadline = Instant::now() + options.extension_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(error(&format!(
                "extension not ready; open the session page and authorize your wallet extension: {page}"
            )));
        }
        if let Ok(Ok(next)) = timeout(remaining, bridge_status(url.as_str(), &options.paths)).await
        {
            // Refuse a session change instead of opening a different capability.
            if next.get("session_id") != status.get("session_id") {
                return Err(error("extension bridge session changed while waiting"));
            }
            status = next;
            if ready(&status) {
                return Ok(url.into());
            }
            notice.notify(&page, &status, Instant::now());
        } else {
            notice.notify(&page, &json!({}), Instant::now());
        }
        tokio::time::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        )
        .await;
    }
}
struct WaitingNotice<'a> {
    observer: Option<&'a BridgeWaiting>,
    last: Option<Instant>,
}
impl<'a> WaitingNotice<'a> {
    fn new(observer: Option<&'a BridgeWaiting>) -> Self {
        Self {
            observer,
            last: None,
        }
    }
    fn notify(&mut self, page: &str, status: &Value, now: Instant) {
        if let Some(observer) = self.observer
            && self
                .last
                .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_secs(5))
        {
            observer(page, status);
            self.last = Some(now);
        }
    }
}
fn launch_page_error(page: &str, failure: BridgeError) -> BridgeError {
    error(&format!("{failure}; open {page} manually"))
}
fn ready(status: &Value) -> bool {
    status.get("browser_connected").and_then(Value::as_bool) == Some(true)
        && status
            .get("account_count")
            .and_then(Value::as_u64)
            .is_some_and(|n| n > 0)
}

/// Explicitly spawn an inertly configured, detached bridge daemon.
///
/// Uses `RTAO_BIN` as one executable path, otherwise `rtao` on PATH. It never
/// invokes a shell. A running Python daemon is reused by [`ensure_bridge`].
#[doc(alias = "bittensor.extension.start_bridge_daemon")]
pub async fn start_bridge_daemon(
    paths: &BridgePaths,
    address: SocketAddr,
) -> Result<(), BridgeError> {
    if paths.token_file == paths.pid_file {
        return Err(error("bridge token and PID files must be different paths"));
    }
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(error("daemon requires a fixed numeric loopback address"));
    }
    if timeout(
        Duration::from_secs(5),
        bridge_status(&bridge_ws_url(address), paths),
    )
    .await
    .is_ok_and(|result| result.is_ok())
    {
        return Ok(());
    }
    if let Ok(Ok(_)) = timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(address),
    )
    .await
    {
        return Err(error(
            "cannot start extension bridge: the loopback port is already in use by another process",
        ));
    }
    if let Some(pid) = read_pid(paths)?
        && process_alive(pid)
    {
        return Err(error(
            "bridge PID file refers to a live process; refusing to start another daemon",
        ));
    }
    let program = std::env::var_os("RTAO_BIN")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "rtao".into());
    let mut command = Command::new(program);
    command
        .args([
            "extension",
            "bridge",
            "--daemon",
            "--host",
            &address.ip().to_string(),
            "--port",
            &address.port().to_string(),
            "--pid-file",
        ])
        .arg(&paths.pid_file)
        .env("BITTENSOR_EXTENSION_BRIDGE_TOKEN", &paths.token_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command
            .as_std_mut()
            .creation_flags(0x0000_0008 | 0x0000_0200);
    }
    command
        .spawn()
        .map_err(|_| error("cannot spawn rtao extension bridge; configure RTAO_BIN"))?;
    Ok(())
}

/// Run the explicit CLI daemon path until SIGINT/SIGTERM, then clean its own files.
/// This helper does not detach or start another process.
pub async fn run_bridge_daemon(
    options: &BridgeOptions,
    paths: &BridgePaths,
) -> Result<(), BridgeError> {
    if let Some(pid) = read_pid(paths)?
        && process_alive(pid)
    {
        return Err(error(
            "bridge state files belong to a live daemon; refusing to overwrite them",
        ));
    }
    if paths.token_file == paths.pid_file {
        return Err(error("bridge token and PID files must be different paths"));
    }
    let mut server = super::run_bridge(options).await?;
    let guard = paths.publish(&server)?;
    let signals = wait_signal().await;
    let stopped = server.stop().await;
    drop(guard);
    signals.and(stopped)
}
async fn wait_signal() -> Result<(), BridgeError> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| error("cannot watch SIGTERM"))?;
        tokio::select! {result=tokio::signal::ctrl_c()=>result.map_err(|_|error("cannot watch SIGINT")), _=terminate.recv()=>Ok(())}
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|_| error("cannot watch the interrupt signal"))
    }
}
/// Ownership guard for one running bridge's explicitly published state files.
/// Files are removed on drop only if their contents still identify this session.
pub struct BridgeFiles {
    paths: BridgePaths,
    token: Zeroizing<String>,
    pid: String,
    #[cfg(unix)]
    _reservation: nix::fcntl::Flock<File>,
}

#[cfg(unix)]
fn resolved_state_path(path: &Path) -> Result<PathBuf, BridgeError> {
    let name = path
        .file_name()
        .ok_or_else(|| error("bridge state path needs a filename"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| error("bridge state path needs a parent directory"))?;
    fs::create_dir_all(parent).map_err(|_| error("cannot create bridge state directory"))?;
    Ok(fs::canonicalize(parent)
        .map_err(|_| error("cannot resolve bridge state directory"))?
        .join(name))
}

#[cfg(all(test, unix))]
pub(super) fn publication_regressions(
    server: &BridgeServer,
) -> Result<(), Box<dyn std::error::Error>> {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!("rtao-extension-publish-{}", super::random_id()?));
    fs::create_dir(&root)?;
    let _cleanup = Cleanup(root.clone());
    fs::create_dir(root.join("sub"))?;
    let aliased = BridgePaths {
        token_file: root.join("token"),
        pid_file: root.join("sub/../token"),
    };
    assert!(aliased.publish(server).is_err());
    assert!(!root.join("token").exists());
    let paths = BridgePaths {
        token_file: root.join("token"),
        pid_file: root.join("pid"),
    };
    let guard = paths.publish(server)?;
    let token = fs::read(&paths.token_file)?;
    let pid = fs::read(&paths.pid_file)?;
    assert!(paths.publish(server).is_err());
    assert_eq!(fs::read(&paths.token_file)?, token);
    assert_eq!(fs::read(&paths.pid_file)?, pid);
    drop(guard);
    assert!(!paths.token_file.exists());
    assert!(!paths.pid_file.exists());
    fs::create_dir(&paths.token_file)?;
    assert!(paths.publish(server).is_err());
    assert!(!paths.pid_file.exists());
    fs::remove_dir(&paths.token_file)?;
    // Recover the same empty reservation a crash could leave, but only while
    // holding its exclusive lock inside publish.
    fs::write(&paths.pid_file, b"")?;
    let retried = paths.publish(server)?;
    drop(retried);
    assert!(!paths.token_file.exists());
    assert!(!paths.pid_file.exists());
    Ok(())
}

#[cfg(all(test, unix))]
pub(super) fn foreground_fixture_paths()
-> Result<(BridgePaths, impl Drop), Box<dyn std::error::Error>> {
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let root =
        std::env::temp_dir().join(format!("rtao-extension-foreground-{}", super::random_id()?));
    fs::create_dir(&root)?;
    Ok((
        BridgePaths {
            token_file: root.join("token"),
            pid_file: root.join("pid"),
        },
        Cleanup(root),
    ))
}
impl Drop for BridgeFiles {
    fn drop(&mut self) {
        remove_if_equal(&self.paths.token_file, self.token.as_bytes(), true);
        // Keep the named reservation until token cleanup is complete: another
        // publisher may create a fresh PID inode immediately after unlinking it.
        remove_if_equal(&self.paths.pid_file, self.pid.as_bytes(), false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(held), Ok(named)) = (
                self._reservation.metadata(),
                fs::symlink_metadata(&self.paths.pid_file),
            ) && held.dev() == named.dev()
                && held.ino() == named.ino()
                && read_private(&self.paths.pid_file, false)
                    .ok()
                    .flatten()
                    .is_some_and(|bytes| bytes.is_empty())
            {
                let _ = fs::remove_file(&self.paths.pid_file);
            }
        }
    }
}

/// Stop only a daemon authenticated by the token and verified against its PID.
///
/// Never signals a PID solely because a file contains it. On Unix the process
/// must belong to this user and its command must identify the bridge and exact
/// PID path. An unverifiable PID is retained for inspection.
#[doc(alias = "bittensor.extension.stop_bridge_daemon")]
pub async fn stop_bridge_daemon(paths: &BridgePaths, url: &str) -> Result<bool, BridgeError> {
    let url = super::client::session_url(url)?;
    let Some(pid) = read_pid(paths)? else {
        return Ok(false);
    };
    if pid == i32::try_from(std::process::id()).unwrap_or(0) {
        return Err(error("refusing to signal the current process"));
    }
    if !process_alive(pid) {
        return Ok(false);
    }
    let token = paths
        .read_token()?
        .ok_or_else(|| error("cannot verify bridge daemon without its token"))?;
    bridge_status(url.as_str(), paths).await?;
    verify_process(pid, &paths.pid_file).await?;
    signal_terminate(pid)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if process_alive(pid) {
        return Err(error(
            "bridge daemon has not exited; retaining its PID and token files",
        ));
    }
    cleanup_stopped_files(paths, pid, token.as_bytes());
    Ok(true)
}

fn cleanup_stopped_files(paths: &BridgePaths, pid: i32, token: &[u8]) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(Some(file)) = open_file(&paths.pid_file, false) else {
            return;
        };
        let Ok(reservation) =
            nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        else {
            return;
        };
        let (Ok(held), Ok(named)) = (
            reservation.metadata(),
            fs::symlink_metadata(&paths.pid_file),
        ) else {
            return;
        };
        if held.dev() != named.dev() || held.ino() != named.ino() || held.nlink() != 1 {
            return;
        }
        if !read_private(&paths.pid_file, false)
            .ok()
            .flatten()
            .is_some_and(|bytes| bytes.as_slice() == pid.to_string().as_bytes())
        {
            return;
        }
        remove_if_equal(&paths.token_file, token, true);
        remove_if_equal(&paths.pid_file, pid.to_string().as_bytes(), false);
    }
    #[cfg(not(unix))]
    let _ = (paths, pid, token);
}

fn read_pid(paths: &BridgePaths) -> Result<Option<i32>, BridgeError> {
    let Some(bytes) = read_private(&paths.pid_file, false)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| error("invalid bridge PID file"))?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let pid = text
        .trim()
        .parse::<i32>()
        .map_err(|_| error("invalid bridge PID file"))?;
    if pid <= 1 {
        return Err(error("bridge PID file is not a daemon PID"));
    }
    Ok(Some(pid))
}
#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}
#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    true
}
#[cfg(unix)]
fn signal_terminate(pid: i32) -> Result<(), BridgeError> {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )
    .map_err(|_| error("cannot stop the verified bridge daemon"))
}
#[cfg(not(unix))]
fn signal_terminate(_pid: i32) -> Result<(), BridgeError> {
    Err(error(
        "verified daemon termination is supported on Unix only",
    ))
}

async fn verify_process(pid: i32, pid_path: &Path) -> Result<(), BridgeError> {
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/ps");
        command
            .args(["-p", &pid.to_string(), "-o", "uid=", "-o", "command="])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| error("cannot verify bridge process ownership"))?;
        let mut output = Vec::new();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| error("cannot inspect bridge process"))?;
        timeout(
            Duration::from_secs(5),
            stdout.take(65537).read_to_end(&mut output),
        )
        .await
        .map_err(|_| error("bridge process inspection timed out"))?
        .map_err(|_| error("cannot inspect bridge process"))?;
        let status = timeout(Duration::from_secs(5), child.wait())
            .await
            .map_err(|_| error("bridge process inspection timed out"))?
            .map_err(|_| error("bridge process inspection failed"))?;
        if !status.success() || output.len() > 65536 {
            return Err(error("cannot verify bridge process"));
        }
        let text = std::str::from_utf8(&output)
            .map_err(|_| error("invalid process inspection output"))?
            .trim();
        let (uid, command) = text
            .split_once(char::is_whitespace)
            .ok_or_else(|| error("cannot verify bridge process ownership"))?;
        if uid.parse::<u32>().ok() != Some(nix::unistd::geteuid().as_raw()) {
            return Err(error("bridge process belongs to another user"));
        }
        let path = pid_path
            .to_str()
            .filter(|p| !p.chars().any(char::is_control))
            .ok_or_else(|| error("cannot verify this PID path"))?;
        let command = command.trim();
        let expected_suffix = format!("--pid-file {path}");
        let bridge = command.contains(" extension bridge --daemon ")
            || command.contains(" -m bittensor.extension.daemon ");
        if !bridge || !command.ends_with(&expected_suffix) {
            return Err(error(
                "PID file does not identify this bridge daemon; retaining its files",
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, pid_path);
        Err(error(
            "verified daemon termination is supported on Unix only",
        ))
    }
}

fn open_file(path: &Path, write: bool) -> Result<Option<File>, BridgeError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(error("bridge state path is not a regular file"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !write => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error("cannot inspect bridge state file")),
    }
    let mut options = OpenOptions::new();
    options.read(!write).write(write).create(write);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| error("cannot open bridge state file"))?;
    let meta = file
        .metadata()
        .map_err(|_| error("cannot inspect bridge state file"))?;
    if !meta.is_file() {
        return Err(error("bridge state path is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != nix::unistd::geteuid().as_raw()
            || meta.mode() & 0o022 != 0
            || meta.nlink() != 1
        {
            return Err(error(
                "bridge state file has unsafe ownership or permissions",
            ));
        }
    }
    Ok(Some(file))
}
fn read_private(path: &Path, secret: bool) -> Result<Option<Zeroizing<Vec<u8>>>, BridgeError> {
    let Some(file) = open_file(path, false)? else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if secret
            && file
                .metadata()
                .map_err(|_| error("cannot inspect token permissions"))?
                .mode()
                & 0o077
                != 0
        {
            return Err(error("bridge token file must be private (mode 0600)"));
        }
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut bytes = Zeroizing::new(Vec::with_capacity(TEXT_LIMIT + 1));
    file.take((TEXT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("cannot read bridge state file"))?;
    if bytes.len() > TEXT_LIMIT {
        return Err(error("bridge state file exceeds its size limit"));
    }
    Ok(Some(bytes))
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), BridgeError> {
    if bytes.len() > TEXT_LIMIT {
        return Err(error("bridge state exceeds its size limit"));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| error("bridge state needs a parent directory"))?;
    fs::create_dir_all(parent).map_err(|_| error("cannot create bridge state directory"))?;
    let mut file =
        open_file(path, true)?.ok_or_else(|| error("cannot create bridge state file"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| error("cannot make bridge state private"))?;
    }
    file.set_len(0)
        .map_err(|_| error("cannot truncate bridge state file"))?;
    file.write_all(bytes)
        .map_err(|_| error("cannot write bridge state file"))
}
fn remove_if_equal(path: &Path, expected: &[u8], secret: bool) {
    if read_private(path, secret)
        .ok()
        .flatten()
        .is_some_and(|bytes| bytes.as_slice() == expected)
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(all(test, unix))]
pub(super) fn private_file_regressions() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!(
        "rtao-extension-private-{}-{}",
        std::process::id(),
        super::random_id()?
    ));
    fs::create_dir(&root)?;
    let _cleanup = Cleanup(root.clone());
    let token = root.join("bridge.token");
    let pid = root.join("bridge.pid");
    let paths = BridgePaths {
        token_file: token.clone(),
        pid_file: pid,
    };
    assert!(paths.read_token()?.is_none());
    for (contents, expected) in [
        ("", None),
        (" \t\r\n\u{85}\u{a0}\u{3000}", None),
        ("\u{1c}\u{1d}\u{1e}\u{1f}", None),
        ("\u{1c} \tprivate-token\r\n\u{1f}", Some("private-token")),
        (
            "\u{1d}\u{1e}private-token\u{1e}\u{1d}",
            Some("private-token"),
        ),
    ] {
        write_private(&token, contents.as_bytes())?;
        assert_eq!(
            paths.read_token()?.as_deref().map(|value| value.as_str()),
            expected
        );
    }
    for contents in ["private\u{1c}token", "private\ttoken", "private\r\ntoken"] {
        write_private(&token, contents.as_bytes())?;
        assert!(paths.read_token().is_err());
    }
    write_private(&token, b"a-private-token")?;
    assert_eq!(fs::metadata(&token)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        paths.read_token()?.as_deref().map(|s| s.as_str()),
        Some("a-private-token")
    );
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644))?;
    assert!(paths.read_token().is_err());
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600))?;
    write_private(&token, &[0xff])?;
    assert!(paths.read_token().is_err());
    fs::write(&token, vec![b'a'; TEXT_LIMIT + 1])?;
    assert!(paths.read_token().is_err());
    fs::remove_file(&token)?;
    let unrelated = root.join("unrelated");
    fs::write(&unrelated, b"preserve this")?;
    fs::set_permissions(&unrelated, fs::Permissions::from_mode(0o600))?;
    symlink(&unrelated, &token)?;
    assert!(write_private(&token, b"overwrite").is_err());
    assert_eq!(fs::read(&unrelated)?, b"preserve this");
    fs::remove_file(&token)?;
    fs::hard_link(&unrelated, &token)?;
    assert!(write_private(&token, b"overwrite").is_err());
    assert_eq!(fs::read(&unrelated)?, b"preserve this");
    Ok(())
}

pub(crate) async fn open_bridge_page(url: &str, browser: Option<&str>) -> Result<(), BridgeError> {
    crate::browser::open_page(url, browser)
        .await
        .map_err(|failure| {
            error(match failure {
                #[cfg(not(unix))]
                crate::browser::OpenError::Unsupported => {
                    "explicit browser opening is unsupported on this platform"
                }
                #[cfg(unix)]
                crate::browser::OpenError::Selection => {
                    "cannot select the bridge browser; open the session URL manually"
                }
                #[cfg(unix)]
                crate::browser::OpenError::Spawn => {
                    "cannot open the bridge page; open the session URL manually"
                }
            })
        })
}

#[cfg(test)]
mod environment_tests {
    use super::*;
    use crate::browser::browser_command_with;

    const URL: &str = "http://127.0.0.1:8090/?session=fixture";

    #[test]
    fn waiting_notice_preserves_page_status_and_five_second_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let events = seen.clone();
        let observer: BridgeWaiting = Arc::new(move |page, status| {
            if let Ok(mut events) = events.lock() {
                events.push((page.to_owned(), status.clone()));
            }
        });
        let mut notice = WaitingNotice::new(Some(&observer));
        let now = Instant::now();
        let initial = json!({"browser_connected":false,"account_count":0,"session_id":"fixture"});
        let waiting = json!({"browser_connected":true,"account_count":0,"session_id":"fixture"});
        notice.notify(URL, &initial, now);
        notice.notify(URL, &waiting, now + Duration::from_millis(4999));
        notice.notify(URL, &waiting, now + Duration::from_secs(5));
        notice.notify(URL, &json!({}), now + Duration::from_millis(9999));
        notice.notify(URL, &json!({}), now + Duration::from_secs(10));
        assert_eq!(
            *seen.lock().map_err(|_| "observer lock")?,
            [
                (URL.to_owned(), initial),
                (URL.to_owned(), waiting),
                (URL.to_owned(), json!({}))
            ]
        );
        Ok(())
    }

    #[test]
    fn failed_browser_launch_retains_the_same_encoded_manual_page() {
        let page = bridge_page_url(SocketAddr::from(([127, 0, 0, 1], 8090)), "a&b");
        assert_eq!(
            launch_page_error(&page, error("cannot open the bridge page")).to_string(),
            "cannot open the bridge page; open http://127.0.0.1:8090/?session=a%26b manually"
        );
    }

    #[test]
    fn bridge_path_overrides_preserve_empty_relative_tilde_and_whitespace_semantics() {
        let home = Path::new("/injected/home");
        for value in [None, Some("")] {
            let paths = BridgePaths::from_environment_with(home, |_| value.map(Into::into));
            assert_eq!(
                paths.token_file,
                home.join(".bittensor/extension_bridge.token")
            );
            assert_eq!(paths.pid_file, home.join(".bittensor/extension_bridge.pid"));
        }
        for (token, pid) in [
            ("relative/token", "relative/pid"),
            ("~/token", "~/pid"),
            (" token ", " pid "),
            ("/absolute/token", "/absolute/pid"),
        ] {
            let paths = BridgePaths::from_environment_with(home, |name| match name {
                "BITTENSOR_EXTENSION_BRIDGE_TOKEN" => Some(token.into()),
                "BITTENSOR_EXTENSION_BRIDGE_PID" => Some(pid.into()),
                _ => None,
            });
            assert_eq!(paths.token_file, Path::new(token));
            assert_eq!(paths.pid_file, Path::new(pid));
        }
        let paths = BridgePaths::from_environment_with(home, |name| {
            (name == "BITTENSOR_EXTENSION_BRIDGE_PID").then(|| "isolated.pid".into())
        });
        assert_eq!(paths.pid_file, Path::new("isolated.pid"));
        assert_eq!(
            paths.token_file,
            home.join(".bittensor/extension_bridge.token")
        );
    }

    #[test]
    fn explicit_browser_uses_truthiness_then_environment_and_preserves_custom_bytes() {
        for explicit in [None, Some("")] {
            let mut looked_up = Vec::new();
            let selected = browser_command_with(
                URL,
                explicit,
                || Some("environment-browser".into()),
                false,
                |name| {
                    looked_up.push(name.to_owned());
                    Some(PathBuf::from("/resolved/env-browser"))
                },
            );
            assert_eq!(looked_up, ["environment-browser"]);
            assert_eq!(selected.program, Path::new("/resolved/env-browser"));
            assert_eq!(selected.arguments, [URL]);
        }
        for raw in [
            " custom browser ",
            "$(do-not-execute)",
            " ",
            "relative/custom-browser",
        ] {
            let mut environment_reads = 0;
            let selected = browser_command_with(
                URL,
                Some(raw),
                || {
                    environment_reads += 1;
                    None
                },
                false,
                |name| {
                    assert_eq!(name, raw);
                    Some(PathBuf::from(name))
                },
            );
            assert_eq!(selected.program, Path::new(raw));
            assert_eq!(selected.arguments, [URL]);
            assert_eq!(environment_reads, 0);
        }
    }

    #[test]
    fn linux_aliases_select_the_first_executable_in_python_order() {
        for (alias, candidates) in [
            ("firefox", &["firefox", "firefox-esr"][..]),
            ("\u{1c}FF\u{1f}", &["firefox", "firefox-esr"][..]),
            (
                " chrome ",
                &[
                    "google-chrome",
                    "google-chrome-stable",
                    "chromium",
                    "chromium-browser",
                ][..],
            ),
            (
                "google-chrome",
                &[
                    "google-chrome",
                    "google-chrome-stable",
                    "chromium",
                    "chromium-browser",
                ][..],
            ),
            ("brave", &["brave-browser", "brave"][..]),
            ("edge", &["microsoft-edge", "microsoft-edge-stable"][..]),
        ] {
            for first_available in 0..candidates.len() {
                let mut visited = Vec::new();
                let selected = browser_command_with(
                    URL,
                    Some(alias),
                    || None,
                    false,
                    |name| {
                        visited.push(name.to_owned());
                        (name == candidates[first_available])
                            .then(|| Path::new("/resolved").join(name))
                    },
                );
                assert_eq!(visited, candidates[..=first_available]);
                assert_eq!(
                    selected.program,
                    Path::new("/resolved").join(candidates[first_available])
                );
                assert_eq!(selected.arguments, [URL]);
            }
            let mut visited = Vec::new();
            let selected = browser_command_with(
                URL,
                Some(alias),
                || None,
                false,
                |name| {
                    visited.push(name.to_owned());
                    None
                },
            );
            assert_eq!(visited, candidates);
            assert_eq!(selected.program, Path::new("xdg-open"));
            assert_eq!(selected.arguments, [URL]);
        }
    }

    #[test]
    fn system_browser_and_macos_app_selection_need_no_executable_probe() {
        for macos in [false, true] {
            for choice in [None, Some(""), Some(" default "), Some("SYSTEM")] {
                let mut probed = Vec::new();
                let selected = browser_command_with(
                    URL,
                    choice,
                    || None,
                    macos,
                    |name| {
                        probed.push(name.to_owned());
                        None
                    },
                );
                assert_eq!(
                    selected.program,
                    Path::new(if macos { "/usr/bin/open" } else { "xdg-open" })
                );
                assert_eq!(selected.arguments, [URL]);
                assert!(probed.is_empty());
            }
        }
        for (choice, app) in [
            (" FF ", "Firefox"),
            ("google-chrome", "Google Chrome"),
            ("chromium", "Chromium"),
            ("brave", "Brave Browser"),
            ("edge", "Microsoft Edge"),
            ("safari", "Safari"),
            (" custom browser ", " custom browser "),
            (" ", " "),
        ] {
            let mut probed = Vec::new();
            let selected = browser_command_with(
                URL,
                Some(choice),
                || None,
                true,
                |name| {
                    probed.push(name.to_owned());
                    None
                },
            );
            assert_eq!(selected.program, Path::new("/usr/bin/open"));
            assert_eq!(selected.arguments, ["-a", app, URL]);
            assert!(probed.is_empty());
        }
    }
}
