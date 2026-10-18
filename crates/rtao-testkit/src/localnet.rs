//! Disposable Docker localnets, deterministic nextest accounts and serialized Sudo.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_client::{Client, ClientConfig, ExtrinsicReport, SubmitOptions, View, ViewExt, Wait};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{CryptoType, TaoBalance};
use rtao_runtime::{
    Call,
    calls::{admin_utils, balances, utility},
    storage::drand,
};
use rtao_signer::{KeypairSigner, Signer};
use toml_edit::DocumentMut;
use url::Url;

/// Number of isolated nextest slots funded by one atomic bootstrap batch.
pub const LOCALNET_SLOTS: u16 = 64;
/// Funding per owner, sufficient for repeated subnet creation and test stake.
pub const SLOT_FUNDING: TaoBalance = TaoBalance(10_000_000_000_000);
/// Hotkey fee reserve. Total bootstrap funding stays below Alice's 1M TAO.
pub const HOTKEY_FUNDING: TaoBalance = TaoBalance(100_000_000_000);

/// Chain block timing selected by the localnet image's positional argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalnetProfile {
    /// 250 ms slots; CR scenarios raise subnet tempo to 48.
    Fast,
    /// 12 s slots; scenario setup lowers subnet tempo to 10.
    Slow,
}
impl LocalnetProfile {
    /// Image command-line argument used by upstream's localnet entrypoint.
    pub const fn argument(self) -> &'static str {
        match self {
            Self::Fast => "True",
            Self::Slow => "False",
        }
    }
    /// Stable name used by the e2e job and evidence.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Slow => "slow",
        }
    }
    /// Parse the workflow's explicit profile selector.
    pub fn parse(value: &str) -> Result<Self, LocalnetError> {
        match value {
            "fast" => Ok(Self::Fast),
            "slow" => Ok(Self::Slow),
            _ => Err(LocalnetError::InvalidInput(
                "profile must be fast or slow".into(),
            )),
        }
    }
}

/// An infrastructure, setup or chain failure from the Docker test harness.
#[derive(Debug)]
pub enum LocalnetError {
    /// Invalid endpoint, image pin, environment or nextest slot.
    InvalidInput(String),
    /// Docker failed or the node did not import its first block within 180 s.
    Infrastructure(String),
    /// No stored quicknet pulse within 30 s of chain readiness.
    DrandUnreachable,
    /// A client, composition or dispatch failure during test setup.
    Chain(String),
    /// Task-root filesystem or blocking-worker failure.
    Io(String),
}
impl fmt::Display for LocalnetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(s) | Self::Infrastructure(s) | Self::Chain(s) | Self::Io(s) => f.write_str(s),
            Self::DrandUnreachable => f.write_str("DrandUnreachable: no quicknet pulse stored within 30 seconds; localnet requires outbound HTTPS"),
        }
    }
}
impl std::error::Error for LocalnetError {}
impl From<std::io::Error> for LocalnetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// Public development signers assigned exclusively to one nextest global slot.
#[derive(Clone, Debug)]
pub struct LocalnetSlot {
    /// Zero-based global slot, checked against the 64 funded accounts.
    pub index: u16,
    /// Funded subnet owner and coldkey.
    pub owner: KeypairSigner,
    /// Funded validator hotkey.
    pub hotkey: KeypairSigner,
    /// Additional miner hotkey funded by its scenario's owner.
    pub miner: KeypairSigner,
}
impl LocalnetSlot {
    /// Derive known test-only URIs without writing a keyfile or recording secrets.
    pub fn new(index: u16) -> Result<Self, LocalnetError> {
        if index >= LOCALNET_SLOTS {
            return Err(LocalnetError::InvalidInput(
                "nextest global slot must be below 64".into(),
            ));
        }
        let uri = format!("//rtao-e2e//slot/{index}");
        let derive = |uri: &str| {
            KeypairSigner::from_uri(uri, CryptoType::Sr25519)
                .map_err(|e| LocalnetError::Chain(e.to_string()))
        };
        Ok(Self {
            index,
            owner: derive(&uri)?,
            hotkey: derive(&format!("{uri}//hot"))?,
            miner: derive(&format!("{uri}//miner"))?,
        })
    }
    /// Read nextest's unique slot; absence is an error to prevent nonce sharing.
    pub fn from_process() -> Result<Self, LocalnetError> {
        let value = std::env::var("NEXTEST_TEST_GLOBAL_SLOT").map_err(|_| {
            LocalnetError::InvalidInput(
                "NEXTEST_TEST_GLOBAL_SLOT is required; run this target through nextest".into(),
            )
        })?;
        Self::new(
            value.parse().map_err(|_| {
                LocalnetError::InvalidInput("invalid NEXTEST_TEST_GLOBAL_SLOT".into())
            })?,
        )
    }
}

struct OwnedContainer {
    id: String,
}
impl Drop for OwnedContainer {
    fn drop(&mut self) {
        // ID is docker's validated 64-digit output, never an environment name.
        if !self.id.is_empty() {
            match Command::new("docker").args(["rm", "-f", &self.id]).output() {
                Ok(output) if output.status.success() => {}
                Ok(output) => eprintln!(
                    "localnet cleanup failed for {}: {}",
                    self.id,
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => eprintln!("localnet cleanup failed for {}: {error}", self.id),
            }
        }
    }
}

/// A local test chain with shared clients and an advisory Alice submission lock.
pub struct Localnet {
    client: Client,
    endpoint: String,
    profile: LocalnetProfile,
    task_root: PathBuf,
    lock_path: PathBuf,
    container: Option<OwnedContainer>,
}
impl fmt::Debug for Localnet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Localnet")
            .field("endpoint", &self.endpoint)
            .field("profile", &self.profile)
            .field("container", &self.container.as_ref().map(|c| &c.id))
            .finish_non_exhaustive()
    }
}
impl Localnet {
    /// Adopt an injected client for offline setup tests without Docker or drand.
    /// The explicit task root bounds the Alice lock; callers own the fake client.
    pub fn from_client(
        client: Client,
        profile: LocalnetProfile,
        task_root: PathBuf,
    ) -> Result<Self, LocalnetError> {
        if !task_root.is_absolute() {
            return Err(LocalnetError::InvalidInput(
                "task root must be absolute".into(),
            ));
        }
        let endpoint = "ws://127.0.0.1:9944".to_owned();
        let lock_path = task_root
            .join("e2e")
            .join(format!("{}.alice.lock", lock_identity(&endpoint)));
        fs::create_dir_all(task_root.join("e2e"))?;
        Ok(Self {
            client,
            endpoint,
            profile,
            task_root,
            lock_path,
            container: None,
        })
    }
    /// Reuse E2E_ENDPOINT or start the default pinned image without bootstrapping.
    pub async fn start(profile: LocalnetProfile) -> Result<Self, LocalnetError> {
        if let Some(endpoint) = std::env::var_os("E2E_ENDPOINT").filter(|value| !value.is_empty()) {
            return Self::connect(endpoint.to_string_lossy().as_ref(), profile).await;
        }
        let pins = include_str!("../../../upstream.toml");
        let image = std::env::var("LOCALNET_IMAGE").unwrap_or(Self::pinned_image(pins, "pinned")?);
        Self::start_image(&image, profile, std::env::var_os("SKIP_PULL").is_some()).await
    }
    /// Resolve a named localnet lane to its authenticated image digest.
    pub fn pinned_image(pins: &str, lane: &str) -> Result<String, LocalnetError> {
        if !["pinned", "mainnet", "testnet", "devnet", "main"].contains(&lane) {
            return Err(LocalnetError::InvalidInput("unknown localnet lane".into()));
        }
        let pins: DocumentMut = pins
            .parse()
            .map_err(|e: toml_edit::TomlError| LocalnetError::InvalidInput(e.to_string()))?;
        let image = pins["localnet"]["image"]
            .as_str()
            .ok_or_else(|| LocalnetError::InvalidInput("localnet image pin missing".into()))?;
        let digest = pins["localnet"][lane]["digest"]
            .as_str()
            .ok_or_else(|| LocalnetError::InvalidInput("localnet digest pin missing".into()))?;
        if !valid_digest(digest) || image.is_empty() || image.contains(char::is_whitespace) {
            return Err(LocalnetError::InvalidInput(
                "invalid localnet image digest".into(),
            ));
        }
        Ok(format!("{image}@{digest}"))
    }
    /// Connect to an explicitly reused loopback node and wait for drand readiness.
    pub async fn connect(endpoint: &str, profile: LocalnetProfile) -> Result<Self, LocalnetError> {
        let task_root = task_root()?;
        let client = connect_client(endpoint).await?;
        let identity = lock_identity(endpoint);
        let lock_path = task_root.join("e2e").join(format!("{identity}.alice.lock"));
        fs::create_dir_all(
            lock_path
                .parent()
                .ok_or_else(|| LocalnetError::Io("Alice lock has no parent".into()))?,
        )?;
        let localnet = Self {
            client,
            endpoint: endpoint.to_owned(),
            profile,
            task_root,
            lock_path,
            container: None,
        };
        localnet.wait_drand(Duration::from_secs(30)).await?;
        Ok(localnet)
    }
    /// Start a disposable image, with one fresh-container retry for drand failure.
    pub async fn start_image(
        image: &str,
        profile: LocalnetProfile,
        skip_pull: bool,
    ) -> Result<Self, LocalnetError> {
        if image.is_empty() || image.starts_with('-') || image.contains(char::is_whitespace) {
            return Err(LocalnetError::InvalidInput("invalid LOCALNET_IMAGE".into()));
        }
        let root = task_root()?;
        if !skip_pull {
            docker(vec!["pull".into(), image.into()]).await?;
        }
        for attempt in 0..2 {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| LocalnetError::Io(e.to_string()))?
                .as_nanos();
            let name = format!("rtao-e2e-{}-{nonce}-{attempt}", std::process::id());
            let output = docker(vec![
                "run".into(),
                "-d".into(),
                "--rm".into(),
                "--label".into(),
                "io.rtao.localnet=e2e".into(),
                "--name".into(),
                name.into(),
                "-p".into(),
                "127.0.0.1::9944".into(),
                "-p".into(),
                "127.0.0.1::9945".into(),
                image.into(),
                profile.argument().into(),
            ])
            .await?;
            let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            {
                return Err(LocalnetError::Infrastructure(
                    "docker run returned an invalid container ID".into(),
                ));
            }
            let container = OwnedContainer { id };
            wait_node(&container.id, &root, Duration::from_secs(180)).await?;
            let output = docker(vec![
                "port".into(),
                container.id.clone().into(),
                "9944/tcp".into(),
            ])
            .await?;
            let endpoint = endpoint_from_port(String::from_utf8_lossy(&output.stdout).as_ref())?;
            let mut localnet = Self {
                client: connect_client(&endpoint).await?,
                endpoint: endpoint.clone(),
                profile,
                lock_path: root
                    .join("e2e")
                    .join(format!("{}.alice.lock", lock_identity(&endpoint))),
                task_root: root.clone(),
                container: Some(container),
            };
            fs::create_dir_all(root.join("e2e"))?;
            match localnet.wait_drand(Duration::from_secs(30)).await {
                Ok(()) => return Ok(localnet),
                Err(LocalnetError::DrandUnreachable) if attempt == 0 => {
                    localnet.retain_logs("drand-retry").await?;
                    localnet.client.close().await;
                    drop(localnet.container.take());
                }
                Err(error) => {
                    localnet.retain_logs("failed").await?;
                    return Err(error);
                }
            }
        }
        Err(LocalnetError::DrandUnreachable)
    }
    /// Borrow the test connection; clones share transport and the nonce manager.
    pub fn client(&self) -> &Client {
        &self.client
    }
    /// Published loopback WebSocket endpoint.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    /// Block timing selected at container startup.
    pub fn profile(&self) -> LocalnetProfile {
        self.profile
    }
    /// Container ID for CI logs and explicit cleanup, absent for a reused node.
    pub fn container_id(&self) -> Option<&str> {
        self.container.as_ref().map(|c| c.id.as_str())
    }
    /// Nextest's isolated deterministic account set.
    pub fn slot(&self) -> Result<LocalnetSlot, LocalnetError> {
        LocalnetSlot::from_process()
    }
    /// Preserve the owned container for a workflow that starts it before tests.
    ///
    /// The caller must clean up the returned exact ID. A reused node is untouched.
    pub fn detach_container(&mut self) -> Option<String> {
        self.container
            .take()
            .map(|mut owned| std::mem::take(&mut owned.id))
    }
    /// Wait until the real Drand storage reports a positive quicknet round.
    pub async fn wait_drand(&self, timeout: Duration) -> Result<(), LocalnetError> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(round) = self.client.storage(&drand::LAST_STORED_ROUND, ()).await
                    && round > 0
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await
        .map_err(|_| LocalnetError::DrandUnreachable)
    }
    /// Submit one root-origin call as Alice while holding a cross-process lock.
    ///
    /// The lock spans fresh account_nextIndex, preparation and inclusion. Clearing
    /// Alice's connection-local cache prevents stale allocations after another
    /// test process submitted under this same lock.
    ///
    /// A subnet hyperparameter call that lands while that subnet's epoch is
    /// deferred is retried in a later block; see [`Localnet::submit_admin`].
    pub async fn sudo(
        &self,
        call: rtao_runtime::CallBytes,
    ) -> Result<ExtrinsicReport, LocalnetError> {
        let mut attempt = 0;
        loop {
            let report = {
                let _lock = alice_lock(self.lock_path.clone()).await?;
                let signer = alice()?;
                self.client.nonces().clear_account(&signer.account());
                let runtime = self.client.runtime().await.map_err(chain)?;
                let outer = rtao_client::compose::sudo(&runtime, call.clone()).map_err(chain)?;
                submit_included(&self.client, outer, &signer).await?
            };
            if !admin_frozen(&report) || attempt == ADMIN_FREEZE_RETRIES {
                return require_success(report);
            }
            attempt += 1;
            self.wait_past(report.block_number).await?;
        }
    }
    /// Submit a signed call, typically a subnet-owner hyperparameter change,
    /// and wait for inclusion.
    ///
    /// The chain refuses admin calls with `AdminActionProhibitedDuringWeightsWindow`
    /// while a subnet has a pending epoch. Besides the configured freeze window,
    /// that includes the block after an epoch was deferred by the per-block
    /// epoch cap, which happens whenever several subnets fall due together.
    /// Only that transient refusal is retried, in a later block, at most
    /// eight times; every other outcome, including other dispatch failures,
    /// is returned unchanged.
    pub async fn submit_admin(
        &self,
        call: rtao_runtime::CallBytes,
        signer: &dyn Signer,
    ) -> Result<ExtrinsicReport, LocalnetError> {
        let mut attempt = 0;
        loop {
            let report = submit_included(&self.client, call.clone(), signer).await?;
            if !admin_frozen(&report) || attempt == ADMIN_FREEZE_RETRIES {
                return Ok(report);
            }
            attempt += 1;
            self.wait_past(report.block_number).await?;
        }
    }
    async fn wait_past(&self, height: Option<u64>) -> Result<(), LocalnetError> {
        let Some(height) = height else {
            return Ok(());
        };
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let best = self
                    .client
                    .at(rtao_client::BlockRef::Best)
                    .await
                    .map_err(chain)?;
                if best.block().number > height {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| LocalnetError::Chain(format!("no block after {height} within 120 seconds")))?
    }
    /// Apply eight global knobs and atomically fund all 64 owner/hotkey pairs.
    ///
    /// Workflows run this once per shared container before any nextest process.
    pub async fn bootstrap(&self) -> Result<(), LocalnetError> {
        for call in bootstrap_calls() {
            self.sudo(
                call.compose(&self.client.runtime().await.map_err(chain)?)
                    .map_err(chain)?,
            )
            .await?;
        }
        let _lock = alice_lock(self.lock_path.clone()).await?;
        let signer = alice()?;
        self.client.nonces().clear_account(&signer.account());
        let runtime = self.client.runtime().await.map_err(chain)?;
        let mut calls = Vec::with_capacity(usize::from(LOCALNET_SLOTS) * 2);
        for index in 0..LOCALNET_SLOTS {
            let slot = LocalnetSlot::new(index)?;
            for (signer, funding) in [(&slot.owner, SLOT_FUNDING), (&slot.hotkey, HOTKEY_FUNDING)] {
                calls.push(
                    balances::transfer_keep_alive(
                        DynValue::new(Value::Bytes(signer.account().0.to_vec())),
                        funding,
                    )
                    .compose(&runtime)
                    .map_err(chain)?,
                );
            }
        }
        checked_submit(
            &self.client,
            utility::batch_all(calls).compose(&runtime).map_err(chain)?,
            &signer,
        )
        .await?;
        Ok(())
    }
    /// Retain bounded diagnostic logs before a disposable container is removed.
    pub async fn retain_logs(&self, suffix: &str) -> Result<(), LocalnetError> {
        if !suffix
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(LocalnetError::InvalidInput("invalid log suffix".into()));
        }
        if let Some(id) = self.container_id() {
            let output = docker(vec![
                "logs".into(),
                "--tail".into(),
                "1000".into(),
                id.into(),
            ])
            .await?;
            let path = self
                .task_root
                .join("e2e")
                .join(format!("{id}-{suffix}.log"));
            fs::create_dir_all(self.task_root.join("e2e"))?;
            let mut bytes = output.stdout;
            bytes.extend_from_slice(&output.stderr);
            fs::write(path, bytes)?;
        }
        Ok(())
    }
}

/// Retries of a setup call refused during a subnet's pending epoch.
const ADMIN_FREEZE_RETRIES: u32 = 8;
fn admin_frozen(report: &ExtrinsicReport) -> bool {
    !report.success
        && report
            .error
            .as_ref()
            .is_some_and(|error| error.name == "AdminActionProhibitedDuringWeightsWindow")
}
fn chain(error: impl fmt::Display) -> LocalnetError {
    LocalnetError::Chain(error.to_string())
}
fn alice() -> Result<KeypairSigner, LocalnetError> {
    KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).map_err(chain)
}
fn task_root() -> Result<PathBuf, LocalnetError> {
    let root = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .ok_or_else(|| {
            LocalnetError::InvalidInput("RTAO_TEMP_ROOT must name the recorded task root".into())
        })?;
    if !root.is_absolute() {
        return Err(LocalnetError::InvalidInput(
            "RTAO_TEMP_ROOT must be absolute".into(),
        ));
    }
    Ok(root)
}
fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
}
fn lock_identity(endpoint: &str) -> String {
    hex::encode(Blake2b::<U32>::digest(endpoint.as_bytes()))
}
fn endpoint_from_port(value: &str) -> Result<String, LocalnetError> {
    let lines: Vec<_> = value.lines().filter(|s| !s.is_empty()).collect();
    let [line] = lines.as_slice() else {
        return Err(LocalnetError::Infrastructure(
            "docker port must return one loopback binding".into(),
        ));
    };
    let port: u16 = line
        .strip_prefix("127.0.0.1:")
        .ok_or_else(|| {
            LocalnetError::Infrastructure("docker port was not bound to loopback".into())
        })?
        .parse()
        .map_err(|_| LocalnetError::Infrastructure("docker returned invalid port".into()))?;
    if port == 0 {
        return Err(LocalnetError::Infrastructure(
            "docker returned port zero".into(),
        ));
    }
    Ok(format!("ws://127.0.0.1:{port}"))
}
async fn connect_client(endpoint: &str) -> Result<Client, LocalnetError> {
    let url = Url::parse(endpoint).map_err(chain)?;
    if !["ws", "wss"].contains(&url.scheme())
        || !url
            .host_str()
            .is_some_and(|h| ["127.0.0.1", "localhost", "[::1]", "::1"].contains(&h))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(LocalnetError::InvalidInput(
            "localnet setup requires a loopback WebSocket endpoint".into(),
        ));
    }
    let mut config = ClientConfig::for_network(endpoint).map_err(chain)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Client::connect(config).await.map_err(chain)
}
pub(crate) async fn checked_submit(
    client: &Client,
    call: rtao_runtime::CallBytes,
    signer: &dyn Signer,
) -> Result<ExtrinsicReport, LocalnetError> {
    require_success(submit_included(client, call, signer).await?)
}
async fn submit_included(
    client: &Client,
    call: rtao_runtime::CallBytes,
    signer: &dyn Signer,
) -> Result<ExtrinsicReport, LocalnetError> {
    let opts = SubmitOptions {
        wait: Wait::Inclusion,
        deadline: Some(Duration::from_secs(180)),
        ..SubmitOptions::default()
    };
    client.submit(call, signer, &opts).await.map_err(chain)
}
fn require_success(report: ExtrinsicReport) -> Result<ExtrinsicReport, LocalnetError> {
    if !report.success {
        return Err(LocalnetError::Chain(format!(
            "localnet setup dispatch failed: {}",
            report.message
        )));
    }
    Ok(report)
}
fn bootstrap_calls() -> Vec<Call> {
    vec![
        admin_utils::sudo_set_admin_freeze_window(0),
        admin_utils::sudo_set_owner_hparam_rate_limit(0),
        admin_utils::sudo_set_network_rate_limit(0),
        admin_utils::sudo_set_tx_rate_limit(0),
        admin_utils::sudo_set_start_call_delay(0),
        admin_utils::sudo_set_network_min_lock_cost(TaoBalance(1_000_000_000)),
        admin_utils::sudo_set_lock_reduction_interval(1),
        admin_utils::sudo_set_subnet_limit(u16::MAX),
    ]
}
async fn alice_lock(path: PathBuf) -> Result<File, LocalnetError> {
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        File::lock(&file)?;
        Ok(file)
    })
    .await
    .map_err(|e| LocalnetError::Io(e.to_string()))?
}
async fn docker(args: Vec<OsString>) -> Result<Output, LocalnetError> {
    tokio::task::spawn_blocking(move || {
        let output = Command::new("docker").args(&args).output()?;
        if !output.status.success() {
            return Err(LocalnetError::Infrastructure(format!(
                "docker failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(output)
    })
    .await
    .map_err(|e| LocalnetError::Io(e.to_string()))?
}
async fn wait_node(id: &str, root: &Path, timeout: Duration) -> Result<(), LocalnetError> {
    let started = Instant::now();
    loop {
        let health = docker(vec![
            "inspect".into(),
            "--format".into(),
            "{{if .State.Health}}{{.State.Health.Status}}{{end}}".into(),
            id.into(),
        ])
        .await?;
        let logs = docker(vec![
            "logs".into(),
            "--tail".into(),
            "200".into(),
            id.into(),
        ])
        .await?;
        let mut log_bytes = logs.stdout;
        log_bytes.extend_from_slice(&logs.stderr);
        fs::create_dir_all(root.join("e2e"))?;
        fs::write(
            root.join("e2e").join(format!("{id}-readiness.log")),
            &log_bytes,
        )?;
        if String::from_utf8_lossy(&health.stdout).trim() == "healthy"
            || String::from_utf8_lossy(&log_bytes)
                .split("Imported #1")
                .skip(1)
                .any(|s| s.starts_with(char::is_whitespace) || s.is_empty())
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(LocalnetError::Infrastructure(
                "localnet did not become healthy or import block 1 within 180 seconds".into(),
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_runtime::calls::sudo;
    #[tokio::test]
    async fn drand_wait_requires_positive_stored_round_with_a_total_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let fake = crate::FakeChain::from_state(crate::FakeState::default());
        let mut config = ClientConfig::for_network("ws://127.0.0.1:9944")?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(subxt_rpcs::RpcClient::new(fake.clone()), config).await?;
        let root = task_root()?.join(format!("localnet-drand-test-{}", std::process::id()));
        let localnet = Localnet::from_client(client, LocalnetProfile::Fast, root.clone())?;
        assert!(matches!(
            localnet.wait_drand(Duration::from_millis(1)).await,
            Err(LocalnetError::DrandUnreachable)
        ));
        fake.mutate_state(|state| state.seed(&drand::LAST_STORED_ROUND, (), 1234))?;
        localnet.wait_drand(Duration::from_secs(1)).await?;
        localnet.client.close().await;
        drop(localnet);
        fs::remove_dir(root.join("e2e"))?;
        fs::remove_dir(root)?;
        Ok(())
    }
    #[tokio::test]
    async fn alice_lock_blocks_an_independent_handle_until_inclusion_guard_drops()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = task_root()?.join(format!("localnet-lock-test-{}", std::process::id()));
        fs::create_dir_all(&root)?;
        let path = root.join("alice.lock");
        let held = alice_lock(path.clone()).await?;
        let independent = OpenOptions::new().read(true).write(true).open(&path)?;
        assert!(independent.try_lock().is_err());
        drop(held);
        independent.try_lock()?;
        drop(independent);
        fs::remove_file(path)?;
        fs::remove_dir(root)?;
        Ok(())
    }
    #[test]
    fn image_pins_profiles_ports_and_all_slot_keys_are_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        let pins = include_str!("../../../upstream.toml");
        for lane in ["pinned", "mainnet", "testnet", "devnet", "main"] {
            assert!(Localnet::pinned_image(pins, lane)?.contains("@sha256:"));
        }
        assert!(Localnet::pinned_image(pins, "unknown").is_err());
        assert_eq!(LocalnetProfile::Fast.argument(), "True");
        assert_eq!(LocalnetProfile::Slow.argument(), "False");
        assert_eq!(
            endpoint_from_port("127.0.0.1:34567\n")?,
            "ws://127.0.0.1:34567"
        );
        for bad in [
            "0.0.0.0:9944",
            "127.0.0.1:0",
            "127.0.0.1:9944\n127.0.0.1:9945",
        ] {
            assert!(endpoint_from_port(bad).is_err());
        }
        let mut keys = std::collections::BTreeSet::new();
        for index in 0..LOCALNET_SLOTS {
            let slot = LocalnetSlot::new(index)?;
            for signer in [&slot.owner, &slot.hotkey, &slot.miner] {
                assert!(keys.insert(signer.account().0));
            }
        }
        assert_eq!(keys.len(), 192);
        assert!(LocalnetSlot::new(64).is_err());
        Ok(())
    }
    #[test]
    fn global_bootstrap_calls_compose_under_actual_pinned_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        let calls = bootstrap_calls();
        assert_eq!(calls.len(), 8);
        for call in calls {
            let inner = call.compose(&runtime)?;
            let wrapped = sudo::sudo(inner).compose(&runtime)?;
            assert!(wrapped.as_ref().len() >= 4);
        }
        Ok(())
    }
}
