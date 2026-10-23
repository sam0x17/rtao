// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Connection initialization and connection-scoped state.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use subxt_rpcs::RpcClient;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::{ClientError, Network, NonceManager, RuntimeCache, RuntimeHandle};
use crate::{hash_hex, parse_hash, rpc_request};
use rtao_primitives::H256;
use rtao_rpc::{Session, SessionConfig};

/// Network, transport, and local-cache options for a connection.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Named network identity and extracted endpoint defaults.
    pub network: Network,
    /// Supervised transport configuration. Endpoint overrides replace presets.
    pub session: SessionConfig,
    /// Head used when creating a snapshot from this client.
    pub head_policy: crate::HeadPolicy,
    /// Metadata disk cache directory; `None` disables the disk cache.
    pub runtime_cache_dir: Option<PathBuf>,
    /// Shared btcli display cache. `None` avoids local display-cache I/O.
    pub config_store: Option<crate::config::Store>,
    /// Maximum time allowed for the complete startup sequence.
    pub startup_timeout: Duration,
    /// Start best/finalized subscriptions and runtime upgrade monitoring.
    pub track_heads: bool,
    /// Serve every best-block read made through the client itself from the
    /// block of its first such read (rtao addition).
    ///
    /// A one-shot read command then reports one consistent block and skips a
    /// header request per read. Leave it off for long-lived clients and for
    /// submissions, which need fresh reads. Explicit snapshots and other block
    /// references are unaffected.
    pub read_at_one_block: bool,
    /// Allow the exact-shape borrowed metagraph decoder for this connection.
    ///
    /// Setting this to false applies to the client, its snapshots and dynamic
    /// views. Per-read options and `RTAO_METAGRAPH_VIEW=0` can also disable it.
    pub metagraph_view: bool,
    /// Optional separate transport for transaction submission (rtao addition).
    ///
    /// When set, the account nonce query, the fee estimate, the `author_*`
    /// submission and watch calls, and the receipt reads at the inclusion
    /// block (with the finality waits that resolve a receipt) use this
    /// endpoint. Every other read stays on [`Self::session`]. Reads can then
    /// come from a pruned, archive or indexing endpoint while transactions go
    /// through a full node that sees the pool and the newest blocks. The
    /// connection opens on first use and must serve the same genesis.
    pub submit: Option<SessionConfig>,
}

impl ClientConfig {
    /// Resolve a network preset and apply its pacing and endpoint defaults.
    pub fn new(network: Network) -> Result<Self, ClientError> {
        let pool = network
            .endpoint_pool()
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        let session = SessionConfig {
            pool,
            rate_limit: network.rate_limit(),
            ..SessionConfig::default()
        };
        Ok(Self {
            network,
            session,
            head_policy: crate::HeadPolicy::Best,
            runtime_cache_dir: RuntimeCache::default_disk_dir(),
            config_store: crate::config::Paths::from_process()
                .ok()
                .map(crate::config::Store::new),
            startup_timeout: Duration::from_secs(30),
            track_heads: true,
            read_at_one_block: false,
            metagraph_view: true,
            submit: None,
        })
    }

    /// Route submissions through a separate endpoint (a name or ws(s) URL).
    ///
    /// The endpoint gets its own pool and the pacing any endpoint gets: a
    /// public preset URL is paced as that preset, and a custom URL is pinned
    /// and unpaced. It copies `session.retry_forever`, so set that first.
    /// Headers and other transport options of the main session are not copied.
    pub fn set_submit_endpoint(&mut self, endpoint: &str) -> Result<(), ClientError> {
        let network =
            Network::resolve(endpoint).map_err(|error| ClientError::Protocol(error.to_string()))?;
        let pool = network
            .endpoint_pool()
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        self.submit = Some(SessionConfig {
            pool,
            rate_limit: network.rate_limit(),
            retry_forever: self.session.retry_forever,
            ..SessionConfig::default()
        });
        Ok(())
    }

    /// The configuration of the separate submission connection.
    fn submit_client_config(&self, session: SessionConfig) -> Self {
        Self {
            network: self.network.clone(),
            session,
            head_policy: crate::HeadPolicy::Best,
            runtime_cache_dir: self.runtime_cache_dir.clone(),
            config_store: None,
            startup_timeout: self.startup_timeout,
            track_heads: self.track_heads,
            read_at_one_block: false,
            metagraph_view: self.metagraph_view,
            submit: None,
        }
    }

    /// Resolve a network name or explicit WebSocket endpoint.
    pub fn for_network(input: &str) -> Result<Self, ClientError> {
        Self::new(
            Network::resolve(input).map_err(|error| ClientError::Protocol(error.to_string()))?,
        )
    }
}

impl Default for ClientConfig {
    #[allow(clippy::expect_used)] // Generated literal defaults are verified by the extraction gate.
    fn default() -> Self {
        Self::new(Network::default_with(|_| None).expect("extracted default network is valid"))
            .expect("extracted default pool is valid")
    }
}

struct ClientInner {
    rpc: RpcClient,
    session: Option<Session>,
    config: ClientConfig,
    genesis: H256,
    signing_disabled: Option<String>,
    cache: RuntimeCache,
    nonces: NonceManager,
    heads: Option<crate::HeadTracker>,
    token_symbols: tokio::sync::RwLock<std::collections::BTreeMap<rtao_primitives::NetUid, String>>,
    hash_cache: tokio::sync::Mutex<HashCache>,
    /// The block every best read uses under `read_at_one_block`.
    best_pin: tokio::sync::Mutex<Option<crate::PinnedBlock>>,
    /// Separate submission connection, when configured.
    submit: Option<Arc<SubmitRoute>>,
    cancelled: CancellationToken,
}

/// Lazily connected submission endpoint shared by a client and its nonces.
///
/// A read-only use of the client never opens it.
pub(crate) struct SubmitRoute {
    config: ClientConfig,
    /// Supplied transport (fake chains); otherwise a session is connected.
    rpc: Option<RpcClient>,
    genesis: H256,
    client: tokio::sync::OnceCell<Client>,
    /// The owning client's shutdown token.
    cancelled: CancellationToken,
}

impl SubmitRoute {
    /// The connected submission client, opening it on first use.
    pub(crate) async fn client(&self) -> Result<Client, ClientError> {
        if self.cancelled.is_cancelled() {
            return Err(rtao_rpc::RpcError::Closed.into());
        }
        let client = tokio::select! {
            biased;
            () = self.cancelled.cancelled() => return Err(rtao_rpc::RpcError::Closed.into()),
            // Boxed: the connection future is large and would otherwise be
            // inlined into every submission future that may open it.
            client = self.client.get_or_try_init(|| Box::pin(self.connect())) => client?,
        };
        Ok(client.clone())
    }

    async fn connect(&self) -> Result<Client, ClientError> {
        let config = self.config.clone();
        let client = match &self.rpc {
            Some(rpc) => Client::from_rpc_inner(rpc.clone(), None, config, false).await,
            None => Client::connect_inner(config, false).await,
        }
        .map_err(submit_endpoint_context)?;
        if client.genesis() != self.genesis {
            client.close_transport().await;
            return Err(ClientError::Protocol(format!(
                "submit endpoint serves a different chain: genesis {}, expected {}",
                hash_hex(client.genesis()),
                hash_hex(self.genesis)
            )));
        }
        if self.cancelled.is_cancelled() {
            client.close_transport().await;
            return Err(rtao_rpc::RpcError::Closed.into());
        }
        Ok(client)
    }

    async fn close(&self) {
        if let Some(client) = self.client.get() {
            client.close_transport().await;
        }
    }
}

/// Name the submit endpoint in a failure to open it.
///
/// The error keeps its variant, so retry and fate classification are
/// unchanged; only a diagnostic gains the endpoint's role.
fn submit_endpoint_context(error: ClientError) -> ClientError {
    use rtao_rpc::RpcError;
    const ROLE: &str = "submit endpoint";
    let named = |detail: String| {
        if detail.is_empty() {
            ROLE.to_owned()
        } else if detail.starts_with(ROLE) {
            detail
        } else {
            format!("{ROLE}: {detail}")
        }
    };
    match error {
        ClientError::Rpc(error) => ClientError::Rpc(match error {
            RpcError::MaxRetriesExceeded { detail } => RpcError::MaxRetriesExceeded {
                detail: named(detail),
            },
            RpcError::Timeout { detail } => RpcError::Timeout {
                detail: named(detail),
            },
            RpcError::Handshake { status, detail } => RpcError::Handshake {
                status,
                detail: named(detail),
            },
            RpcError::Protocol(detail) => RpcError::Protocol(named(detail)),
            RpcError::Transport(detail) => RpcError::Transport(named(detail)),
            other => other,
        }),
        ClientError::Protocol(detail) => ClientError::Protocol(named(detail)),
        other => other,
    }
}

/// Asynchronous chain connection and its immutable network identity.
///
/// Each clone shares the connection. The last user-facing clone cancels every
/// background worker. Workers retain the inner state without retaining the
/// lifetime guard, so they cannot keep the socket open indefinitely.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
    _alive: Arc<DropGuard>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("network", &self.inner.config.network)
            .field("genesis", &hash_hex(self.inner.genesis))
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Connect a supervised WebSocket session and verify the chain preconditions.
    ///
    /// A configured [`ClientConfig::submit`] endpoint connects on first use.
    pub async fn connect(config: ClientConfig) -> Result<Self, ClientError> {
        Self::connect_inner(config, true).await
    }

    async fn connect_inner(config: ClientConfig, symbols: bool) -> Result<Self, ClientError> {
        let timeout = config.startup_timeout;
        let startup = async {
            let session = Session::connect(config.session.clone())
                .await
                .map_err(ClientError::from)?;
            let rpc = RpcClient::new(session.clone());
            match Self::initialize(rpc, config, Some(session.clone()), None).await {
                Ok(client) => Ok(client),
                Err(error) => {
                    session.close().await;
                    Err(error)
                }
            }
        };
        let client = tokio::time::timeout(timeout, startup)
            .await
            .map_err(|_| startup_timeout_error(timeout))??;
        if symbols {
            client.load_token_symbols_within(timeout).await;
        }
        Ok(client)
    }

    /// Initialize over an explicit native session, retaining author-write custody
    /// capability. The client owns shutdown of this session. A supplied opaque
    /// RpcClient alone cannot attest this capability, even if it wraps a Session.
    pub async fn from_session(session: Session, config: ClientConfig) -> Result<Self, ClientError> {
        let timeout = config.startup_timeout;
        let rpc = RpcClient::new(session.clone());
        let client =
            tokio::time::timeout(timeout, Self::initialize(rpc, config, Some(session), None))
                .await
                .map_err(|_| startup_timeout_error(timeout))??;
        client.load_token_symbols_within(timeout).await;
        Ok(client)
    }

    /// Initialize over a supplied RPC implementation, including fake chains.
    ///
    /// A configured [`ClientConfig::submit`] endpoint is connected as a
    /// session on first use.
    pub async fn from_rpc(rpc: RpcClient, config: ClientConfig) -> Result<Self, ClientError> {
        Self::from_rpc_inner(rpc, None, config, true).await
    }

    /// Initialize over supplied read and submission transports.
    ///
    /// `submit` takes the place of [`ClientConfig::submit`] and is initialized
    /// on first use, like a configured submit endpoint.
    pub async fn from_rpc_with_submit(
        rpc: RpcClient,
        submit: RpcClient,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        Self::from_rpc_inner(rpc, Some(submit), config, true).await
    }

    async fn from_rpc_inner(
        rpc: RpcClient,
        submit: Option<RpcClient>,
        config: ClientConfig,
        symbols: bool,
    ) -> Result<Self, ClientError> {
        let timeout = config.startup_timeout;
        let client = tokio::time::timeout(timeout, Self::initialize(rpc, config, None, submit))
            .await
            .map_err(|_| startup_timeout_error(timeout))??;
        if symbols {
            client.load_token_symbols_within(timeout).await;
        }
        Ok(client)
    }

    async fn initialize(
        rpc: RpcClient,
        config: ClientConfig,
        session: Option<Session>,
        submit_rpc: Option<RpcClient>,
    ) -> Result<Self, ClientError> {
        let genesis = parse_hash(&rpc_request(&rpc, "chain_getBlockHash", json!([0])).await?)?;
        let expected = config
            .network
            .genesis()
            .map_err(|error| ClientError::Protocol(error.to_string()))?;
        let signing_disabled = expected
            .filter(|expected| *expected != genesis)
            .map(|expected| {
                format!(
                    "genesis mismatch: expected {}, connected {}",
                    hash_hex(expected),
                    hash_hex(genesis)
                )
            });
        // A verified public genesis fixes the ss58 format; skip the paced request.
        let ss58 = if expected == Some(genesis) {
            rtao_catalog::py::SS58_FORMAT
        } else {
            rpc_request(&rpc, "system_properties", json!([]))
                .await?
                .get("ss58Format")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ClientError::Protocol("system_properties returned no ss58Format".into())
                })?
        };
        if ss58 != rtao_catalog::py::SS58_FORMAT {
            return Err(ClientError::Protocol(format!(
                "ss58 format must be 42, found {ss58}"
            )));
        }
        let finalized = parse_hash(&rpc_request(&rpc, "chain_getFinalizedHead", json!([])).await?)?;
        let cache = RuntimeCache::new(
            rpc.clone(),
            genesis,
            ss58 as u16,
            config.runtime_cache_dir.clone(),
        );
        // The head runtime is resolved on first use (signing always rechecks
        // it), so a read-only command does not spend a paced request on it.
        let runtime = cache.runtime_at_state(finalized).await?;
        let _ = runtime.surface_report();
        if !crate::compat::runtime_validated(genesis, &runtime) {
            // Evidence can only cover pinned public networks; a local or
            // custom development chain is expected to be outside it. Testnet
            // and devnet routinely run ahead of the mainnet runtime rtao pins,
            // so only mainnet warrants a warning.
            let validated = crate::compat::validated_spec_range().map_or_else(
                || "none".to_owned(),
                |(min, max)| {
                    if min == max {
                        min.to_string()
                    } else {
                        format!("{min}-{max}")
                    }
                },
            );
            if crate::compat::mainnet_genesis(genesis) {
                tracing::warn!(
                    "mainnet runs runtime spec {}, which this rtao build has not validated \
                     (validated: {validated}); check for an rtao update",
                    runtime.spec_version()
                );
            } else if crate::compat::pinned_public_genesis(genesis) {
                tracing::info!(
                    "connected runtime spec {} is outside this build's validated mainnet \
                     runtime (validated: {validated})",
                    runtime.spec_version()
                );
            } else {
                tracing::debug!(
                    spec_version = runtime.spec_version(),
                    "connected runtime is outside the compiled validated compatibility evidence"
                );
            }
        }
        let cancelled = CancellationToken::new();
        let heads = if config.track_heads {
            // The best header's hash is derived from its encoding, saving a
            // separate chain_getBlockHash request.
            let best_raw = rpc_request(&rpc, "chain_getHeader", json!([])).await?;
            let finalized_raw =
                rpc_request(&rpc, "chain_getHeader", json!([hash_hex(finalized)])).await?;
            let best_header = crate::BlockHeader::from_json(best_raw).map_err(head_error)?;
            let finalized_header =
                crate::BlockHeader::from_json_with_hash(finalized_raw, finalized)
                    .map_err(head_error)?;
            Some(
                crate::HeadTracker::start(
                    Arc::new(rpc.clone()),
                    best_header,
                    finalized_header,
                    cancelled.child_token(),
                )
                .await
                .map_err(head_error)?,
            )
        } else {
            None
        };
        let submit_session = match (&submit_rpc, &config.submit) {
            (Some(_), _) => Some(config.session.clone()),
            (None, session) => session.clone(),
        };
        let submit = submit_session.map(|session| {
            Arc::new(SubmitRoute {
                config: config.submit_client_config(session),
                rpc: submit_rpc,
                genesis,
                client: tokio::sync::OnceCell::new(),
                cancelled: cancelled.clone(),
            })
        });
        let alive = Arc::new(cancelled.clone().drop_guard());
        let inner = Arc::new(ClientInner {
            rpc: rpc.clone(),
            session,
            config,
            genesis,
            signing_disabled,
            cache,
            nonces: NonceManager::with_route(rpc, submit.clone()),
            cancelled,
            heads,
            token_symbols: tokio::sync::RwLock::new(std::collections::BTreeMap::new()),
            hash_cache: tokio::sync::Mutex::new(HashCache::default()),
            best_pin: tokio::sync::Mutex::new(None),
            submit,
        });
        let client = Self {
            inner: inner.clone(),
            _alive: alive,
        };
        // This worker holds no lifetime guard. Dropping the last Client clone
        // wakes it and closes the underlying session before releasing state.
        tokio::spawn(async move {
            inner.cancelled.cancelled().await;
            if let Some(submit) = &inner.submit {
                submit.close().await;
            }
            if let Some(session) = &inner.session {
                session.close().await;
            }
        });
        client.start_runtime_watcher(Some((
            runtime.spec_version(),
            runtime.transaction_version(),
        )));
        Ok(client)
    }

    /// The immutable connected chain genesis hash.
    pub fn genesis(&self) -> H256 {
        self.inner.genesis
    }

    /// Resolved network configuration.
    pub fn network(&self) -> &Network {
        &self.inner.config.network
    }

    /// Whether this connection permits the borrowed metagraph decoder.
    pub fn metagraph_view_enabled(&self) -> bool {
        self.inner.config.metagraph_view
    }

    /// Shared nonce allocations for this connection.
    pub fn nonces(&self) -> &NonceManager {
        &self.inner.nonces
    }

    /// The supplied transport, for advanced callers and integrations.
    pub fn rpc(&self) -> &RpcClient {
        &self.inner.rpc
    }

    /// Whether a separate submission endpoint is configured.
    pub fn has_submit_endpoint(&self) -> bool {
        self.inner.submit.is_some()
    }

    /// The connection that resolves receipts: the submission endpoint's client
    /// when one is configured (connecting it on first use), else this client.
    ///
    /// A receipt's inclusion block comes from the submission endpoint, and the
    /// read endpoint may not have imported or finalized it yet. Follow-up reads
    /// that must agree with a receipt (its block, finality and canonical
    /// height) can run here as one coherent view of that chain.
    pub async fn receipt_client(&self) -> Result<Self, ClientError> {
        self.submit_client().await
    }

    /// Snapshot at a receipt's inclusion block.
    ///
    /// It reads from this endpoint when it has the block. When it does not and
    /// a submission endpoint is configured, the snapshot comes from the
    /// submission endpoint that reported the block. [`crate::Snapshot::client`]
    /// is the connection that served it. Without a submission endpoint this is
    /// exactly `self.at(BlockRef::Hash(block))`.
    pub async fn at_receipt(&self, block: H256) -> Result<crate::Snapshot, ClientError> {
        match self.at(crate::BlockRef::Hash(block)).await {
            Err(ClientError::BlockNotFound(_)) if self.inner.submit.is_some() => {
                self.submit_client()
                    .await?
                    .at(crate::BlockRef::Hash(block))
                    .await
            }
            result => result,
        }
    }

    /// The connection that carries submissions: the submission endpoint's
    /// client when one is configured (connecting it on first use), else this
    /// client itself.
    pub(crate) async fn submit_client(&self) -> Result<Self, ClientError> {
        self.ensure_open()?;
        match &self.inner.submit {
            Some(route) => route.client().await,
            None => Ok(self.clone()),
        }
    }

    /// Actual submission endpoint's native author session, with queue/write
    /// ownership capability. Genesis is checked by the normal submission route.
    /// This is a transport capability, not consensus or protocol qualification.
    /// Opaque injected author transports refuse rather than silently losing the
    /// durable ownership contract; use connect or from_session for native custody.
    pub async fn native_author_session(&self) -> Result<Session, ClientError> {
        self.optional_native_author_session().await?.ok_or_else(|| {
            ClientError::Journal(crate::journal::JournalError::Blocked(
                "durable author requires an explicit native Session".into(),
            ))
        })
    }

    pub(crate) async fn fresh_signing_metadata(&self, at: H256) -> Result<Vec<u8>, ClientError> {
        self.inner.cache.fresh_signing_metadata(at).await
    }

    pub(crate) async fn optional_native_author_session(
        &self,
    ) -> Result<Option<Session>, ClientError> {
        Ok(self.submit_client().await?.inner.session.clone())
    }

    /// Recovering best and finalized head tracker, when enabled in configuration.
    pub fn heads(&self) -> Option<&crate::HeadTracker> {
        self.inner.heads.as_ref()
    }

    /// Connection-scoped chain token symbols used for balance display.
    pub async fn token_symbols(
        &self,
    ) -> std::collections::BTreeMap<rtao_primitives::NetUid, String> {
        self.inner.token_symbols.read().await.clone()
    }

    /// Tag a chain-native balance with this connection's cached display symbol.
    ///
    /// This only reads connection-local memory and remains available after close.
    /// Root TAO always uses its canonical symbol.
    pub async fn balance(
        &self,
        rao: u64,
        netuid: rtao_primitives::NetUid,
    ) -> rtao_primitives::DynBalance {
        let balance = rtao_primitives::DynBalance::from_rao(rao, netuid);
        if netuid == rtao_primitives::NetUid::ROOT {
            return balance;
        }
        match self.inner.token_symbols.read().await.get(&netuid) {
            Some(symbol) => balance.with_symbol(symbol.clone()),
            None => balance,
        }
    }

    /// Display symbols are optional, so a slow scan (hundreds of subnets on a
    /// paced public endpoint) has its own bound outside the startup budget.
    async fn load_token_symbols_within(&self, bound: Duration) {
        if tokio::time::timeout(bound, self.load_token_symbols())
            .await
            .is_err()
        {
            tracing::debug!("display token symbol scan timed out");
        }
    }

    async fn load_token_symbols(&self) {
        let network = self.network().label();
        if let Some(store) = &self.inner.config.config_store
            && store.token_symbols_fresh(network).await.unwrap_or(false)
            && let Ok(symbols) = store.load_token_symbols(network).await
        {
            *self.inner.token_symbols.write().await = symbols.chain_values().values;
            return;
        }
        match self.scan_token_symbols().await {
            Ok(symbols) => {
                *self.inner.token_symbols.write().await = symbols.clone();
                if let Some(store) = &self.inner.config.config_store {
                    let _ = store.save_token_symbols(network, symbols.into()).await;
                }
            }
            Err(error) => tracing::debug!(%error,"display token symbols unavailable"),
        }
    }

    async fn scan_token_symbols(
        &self,
    ) -> Result<std::collections::BTreeMap<rtao_primitives::NetUid, String>, ClientError> {
        use crate::View;
        use rtao_core::{codec::Value as CoreValue, value_ext::ValueExt};
        let snapshot = View::pin(self).await?;
        let runtime = snapshot.block().runtime.clone();
        let entry = runtime
            .storage_entry("SubtensorModule", "TokenSymbol")
            .ok_or_else(|| ClientError::RuntimeApiUnavailable {
                api: "SubtensorModule".into(),
                method: "TokenSymbol".into(),
            })?;
        let prefix = runtime
            .core()
            .storage_key(entry, &[])
            .map_err(rtao_core::CoreError::from)?;
        let mut start = None;
        let mut symbols = std::collections::BTreeMap::new();
        loop {
            let page = View::keys_paged(&snapshot, &prefix, 1000, start.as_deref()).await?;
            if page.is_empty() {
                break;
            }
            let values = View::storage_raw_many(&snapshot, &page).await?;
            for (key, bytes) in page.iter().zip(values) {
                let keys = runtime
                    .core()
                    .decode_storage_key_params(entry, key, 0)
                    .map_err(rtao_core::CoreError::from)?;
                let Some(raw) = keys
                    .first()
                    .and_then(CoreValue::as_u128)
                    .and_then(|raw| u16::try_from(raw).ok())
                else {
                    return Err(ClientError::Protocol(
                        "TokenSymbol key is not a chain netuid".into(),
                    ));
                };
                let netuid = rtao_primitives::NetUid::new(raw)
                    .map_err(|error| ClientError::Protocol(error.to_string()))?;
                let value = rtao_core::ops::storage::decode_storage_bytes(
                    runtime.core(),
                    entry,
                    bytes.as_deref(),
                )
                .map_err(rtao_core::CoreError::from)?;
                let text = if let Some(text) = value.as_str() {
                    text.to_owned()
                } else if let Some(bytes) = value.value_bytes() {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    return Err(ClientError::Protocol(
                        "TokenSymbol value is not UTF-8 bytes".into(),
                    ));
                };
                symbols.insert(netuid, text);
            }
            let last = page.last().cloned();
            if last == start {
                return Err(ClientError::Protocol(
                    "TokenSymbol map cursor did not advance".into(),
                ));
            }
            start = last;
        }
        Ok(symbols)
    }

    /// Follow headers while preserving the transport's strict stream failures.
    pub async fn blocks_strict(
        &self,
        finality: crate::Finality,
    ) -> Result<crate::head::BlockStream, ClientError> {
        use futures::StreamExt;

        self.ensure_open()?;
        let stream = tokio::select! {
            biased;
            () = self.closed() => return Err(rtao_rpc::RpcError::Closed.into()),
            stream = crate::head::HeadProvider::subscribe(&self.inner.rpc, finality) => {
                stream.map_err(head_error)?
            }
        };
        // A supplied transport may finish admission while also closing the
        // client. Refusing here drops its newly opened subscription.
        self.ensure_open()?;
        let cancelled = self.inner.cancelled.clone();
        Ok(Box::pin(futures::stream::unfold(
            (stream, cancelled),
            |(mut stream, cancelled)| async move {
                tokio::select! {
                    biased;
                    () = cancelled.cancelled() => None,
                    item = stream.next() => item.map(|item| (item, (stream, cancelled))),
                }
            },
        )))
    }

    /// Follow advancing headers with reconnect backfill and explicit best reorgs.
    pub fn blocks(
        &self,
        finality: crate::Finality,
    ) -> Result<crate::head::BlockStream, ClientError> {
        self.inner
            .heads
            .as_ref()
            .map(|heads| heads.blocks(finality))
            .ok_or_else(|| ClientError::Protocol("head tracking is disabled".into()))
    }

    fn start_runtime_watcher(&self, mut finalized_runtime: Option<(u32, u32)>) {
        if !self.inner.config.track_heads {
            return;
        }
        // Public endpoints are paced at about one request per second, so the
        // watchers avoid requests whose answer is already known: the runtime
        // subscription delivers versions itself, polling runs only while it is
        // down, and a finalized block is rechecked only while its runtime may
        // differ from the head's.
        let followed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let inner = self.inner.clone();
        let following = followed.clone();
        let mut finalized = self.inner.heads.as_ref().map(crate::HeadTracker::finalized);
        tokio::spawn(async move {
            let period = Duration::from_secs(12);
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = inner.cancelled.cancelled() => break,
                    _ = interval.tick() => {
                        if following.load(std::sync::atomic::Ordering::Acquire) {
                            continue;
                        }
                        if let Err(error) = inner.cache.refresh_head().await
                            && !inner.cancelled.is_cancelled()
                        {
                            tracing::warn!(%error,"runtime version refresh failed");
                        }
                    }
                    changed = async {
                        match &mut finalized {
                            Some(watch) => watch.changed().await.ok(),
                            None => std::future::pending().await,
                        }
                    } => {
                        if changed.is_none() { finalized = None; }
                        else if let Some(watch) = &finalized {
                            let hash = watch.borrow().hash;
                            let head = inner.cache.peek_head().await.map(|runtime| {
                                (runtime.spec_version(), runtime.transaction_version())
                            });
                            if head.is_some() && head == finalized_runtime {
                                continue;
                            }
                            match inner.cache.runtime_at_state(hash).await {
                                Ok(runtime) => {
                                    finalized_runtime =
                                        Some((runtime.spec_version(), runtime.transaction_version()));
                                }
                                // A refresh in flight at close fails with the session; stay quiet.
                                Err(error) if !inner.cancelled.is_cancelled() => {
                                    tracing::warn!(%error,"finalized runtime version refresh failed");
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
            }
        });
        let inner = self.inner.clone();
        tokio::spawn(async move {
            loop {
                let subscription = tokio::select! {
                    () = inner.cancelled.cancelled() => break,
                    subscription = inner.rpc.subscribe::<Value>("state_subscribeRuntimeVersion",subxt_rpcs::client::RpcParams::new(),"state_unsubscribeRuntimeVersion") => subscription,
                };
                let Ok(mut subscription) = subscription else {
                    tokio::select! {
                        () = inner.cancelled.cancelled() => break,
                        () = tokio::time::sleep(Duration::from_secs(12)) => continue,
                    }
                };
                loop {
                    let item = tokio::select! {
                        () = inner.cancelled.cancelled() => return,
                        item = subscription.next() => item,
                    };
                    match item {
                        Some(Ok(version)) => match inner.cache.follow_head(&version).await {
                            Ok(_) => {
                                followed.store(true, std::sync::atomic::Ordering::Release);
                                inner.cache.set_head_followed(true).await;
                            }
                            Err(error) => {
                                if !inner.cancelled.is_cancelled() {
                                    tracing::warn!(%error,"runtime upgrade refresh failed");
                                }
                                break;
                            }
                        },
                        Some(Err(error)) => {
                            tracing::warn!(%error,"runtime subscription lost");
                            break;
                        }
                        None => break,
                    }
                }
                followed.store(false, std::sync::atomic::Ordering::Release);
                inner.cache.set_head_followed(false).await;
                inner.cache.invalidate_head().await;
            }
        });
    }

    /// Refuse signing for a wrong network or unreadable signed extensions.
    pub async fn ensure_signing(&self) -> Result<(), ClientError> {
        self.ensure_open()?;
        if let Some(reason) = &self.inner.signing_disabled {
            return Err(ClientError::SigningDisabled {
                reason: reason.clone(),
            });
        }
        self.runtime_for(None)
            .await?
            .signing_status()
            .map_err(|error| ClientError::SigningDisabled {
                reason: error.to_string(),
            })
    }

    /// Cancel background tasks and close this shared connection.
    pub async fn close(&self) {
        self.inner.cancelled.cancel();
        if let Some(submit) = &self.inner.submit {
            submit.close().await;
        }
        self.close_transport().await;
    }

    /// Close this connection alone; a submission client has no route of its own.
    async fn close_transport(&self) {
        self.inner.cancelled.cancel();
        if let Some(session) = &self.inner.session {
            session.close().await;
        }
    }

    /// Observe connection shutdown without obtaining a cancellation capability.
    pub async fn closed(&self) {
        self.inner.cancelled.cancelled().await;
    }

    /// Whether this shared connection has been closed.
    pub fn is_closed(&self) -> bool {
        self.inner.cancelled.is_cancelled()
    }

    pub(crate) fn ensure_open(&self) -> Result<(), ClientError> {
        if self.inner.cancelled.is_cancelled() {
            Err(rtao_rpc::RpcError::Closed.into())
        } else {
            Ok(())
        }
    }

    /// Issue an RPC through the same public error boundary used by typed reads.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<T, ClientError> {
        self.ensure_open()?;
        let response = tokio::select! {
            biased;
            () = self.closed() => return Err(rtao_rpc::RpcError::Closed.into()),
            response = rpc_request(&self.inner.rpc, method, params) => response?,
        };
        serde_json::from_value(response).map_err(|error| ClientError::Protocol(error.to_string()))
    }

    pub(crate) async fn read_request(
        &self,
        method: rtao_rpc::ReadMethod,
        params: Value,
    ) -> Result<Value, ClientError> {
        self.request(method.as_str(), params).await
    }

    pub(crate) async fn runtime_for(&self, at: Option<H256>) -> Result<RuntimeHandle, ClientError> {
        self.ensure_open()?;
        tokio::select! {
            biased;
            () = self.closed() => Err(rtao_rpc::RpcError::Closed.into()),
            runtime = self.inner.cache.runtime_at(at) => runtime,
        }
    }
    /// Recheck the node's head runtime before preparing or attaching a signature.
    pub async fn refresh_runtime(&self) -> Result<RuntimeHandle, ClientError> {
        self.ensure_open()?;
        self.inner.cache.invalidate_head().await;
        tokio::select! {
            biased;
            () = self.closed() => Err(rtao_rpc::RpcError::Closed.into()),
            runtime = self.inner.cache.refresh_head() => runtime,
        }
    }

    pub(crate) fn head_policy(&self) -> crate::HeadPolicy {
        self.inner.config.head_policy
    }

    /// Whether head reads through the client share the first read's block.
    pub(crate) fn reads_at_one_block(&self) -> bool {
        self.inner.config.read_at_one_block
    }

    /// Resolve a requested block once and retain its parent-selected runtime.
    pub async fn at(&self, block: crate::BlockRef) -> Result<crate::Snapshot, ClientError> {
        self.ensure_open()?;
        if matches!(block, crate::BlockRef::Best) && self.reads_at_one_block() {
            // Holding the slot while resolving also makes concurrent first
            // reads share one request.
            let mut slot = self.inner.best_pin.lock().await;
            if let Some(pinned) = slot.as_ref() {
                return Ok(crate::Snapshot::new(self.clone(), pinned.clone()));
            }
            let snapshot = self.resolve_at(block).await?;
            *slot = Some(snapshot.block().clone());
            return Ok(snapshot);
        }
        self.resolve_at(block).await
    }

    async fn resolve_at(&self, block: crate::BlockRef) -> Result<crate::Snapshot, ClientError> {
        // Public endpoints are paced, so the best header is read in one request
        // with its hash derived from the encoding, and the runtime lookup reuses
        // the header's parent instead of fetching the header again.
        let header = match block {
            crate::BlockRef::Best => {
                let raw: Value = self.request("chain_getHeader", json!([])).await?;
                crate::BlockHeader::from_json(raw).map_err(head_error)?
            }
            crate::BlockRef::Hash(hash) => self.header_by_hash(hash).await?,
            crate::BlockRef::Finalized => {
                let hash = parse_hash(&self.request("chain_getFinalizedHead", json!([])).await?)?;
                self.header_by_hash(hash).await?
            }
            crate::BlockRef::Number(number) => {
                let hash = self.hash_for_number(number).await?;
                self.header_by_hash(hash).await?
            }
        };
        let hash = header.hash;
        let runtime = match header.parent_hash {
            Some(parent) => {
                self.ensure_open()?;
                tokio::select! {
                    biased;
                    () = self.closed() => return Err(rtao_rpc::RpcError::Closed.into()),
                    runtime = self.inner.cache.runtime_at_parent(hash, parent) => runtime?,
                }
            }
            None => self.runtime_for(Some(hash)).await?,
        };
        Ok(crate::Snapshot::new(
            self.clone(),
            crate::PinnedBlock {
                number: header.number,
                hash,
                runtime,
            },
        ))
    }

    async fn header_by_hash(&self, hash: H256) -> Result<crate::BlockHeader, ClientError> {
        let raw: Value = self
            .request("chain_getHeader", json!([hash_hex(hash)]))
            .await?;
        if raw.is_null() {
            return Err(ClientError::BlockNotFound(hash));
        }
        crate::BlockHeader::from_json_with_hash(raw, hash).map_err(head_error)
    }

    async fn hash_for_number(&self, number: u64) -> Result<H256, ClientError> {
        self.ensure_open()?;
        let mut cache = tokio::select! {
            biased;
            () = self.closed() => return Err(rtao_rpc::RpcError::Closed.into()),
            cache = self.inner.hash_cache.lock() => cache,
        };
        if cache
            .finalized
            .as_ref()
            .is_none_or(|(_, checked)| checked.elapsed() >= Duration::from_secs(12))
        {
            let hash = parse_hash(&self.request("chain_getFinalizedHead", json!([])).await?)?;
            let raw = self
                .request("chain_getHeader", json!([hash_hex(hash)]))
                .await?;
            let header = crate::BlockHeader::from_json_with_hash(raw, hash).map_err(head_error)?;
            cache.finalized = Some((header.number, tokio::time::Instant::now()));
        }
        let finalized = cache
            .finalized
            .as_ref()
            .is_some_and(|(height, _)| number <= *height);
        if finalized && let Some(hash) = cache.hashes.get(&number) {
            return Ok(*hash);
        }
        let value: Value = self.request("chain_getBlockHash", json!([number])).await?;
        if value.is_null() {
            return Err(ClientError::Protocol(format!(
                "Block not found for height {number}"
            )));
        }
        let hash = parse_hash(&value)?;
        if finalized {
            cache.hashes.insert(number, hash);
            cache.order.push_back(number);
            if cache.order.len() > 512
                && let Some(old) = cache.order.pop_front()
            {
                cache.hashes.remove(&old);
            }
        }
        Ok(hash)
    }

    /// Submit already-signed bytes while preserving transport metadata and
    /// clearing the signer's nonce on failure, cancellation, or shutdown.
    pub async fn submit_signed_raw(
        &self,
        bytes: &[u8],
        signer_address: &str,
    ) -> Result<H256, ClientError> {
        let guard = self.inner.nonces.invalidate_on_drop(signer_address);
        self.ensure_signing().await?;
        let hash: Value = self
            .submit_client()
            .await?
            .request(
                "author_submitExtrinsic",
                json!([format!("0x{}", hex::encode(bytes))]),
            )
            .await?;
        let hash = parse_hash(&hash)?;
        guard.settle();
        Ok(hash)
    }

    /// Locate already included bytes by their Blake2-256 identity.
    ///
    /// The returned index is the input to parent-runtime receipt decoding.
    /// A missing extrinsic returns `None` rather than inventing an outcome.
    pub async fn find_extrinsic(
        &self,
        extrinsic_hash: H256,
        block_hash: H256,
    ) -> Result<Option<u32>, ClientError> {
        self.lookup_extrinsic(extrinsic_hash, block_hash, false)
            .await
    }

    /// Negative transmission evidence requires an explicitly complete body.
    pub(crate) async fn find_extrinsic_complete(
        &self,
        extrinsic_hash: H256,
        block_hash: H256,
    ) -> Result<Option<u32>, ClientError> {
        self.lookup_extrinsic(extrinsic_hash, block_hash, true)
            .await
    }

    async fn lookup_extrinsic(
        &self,
        extrinsic_hash: H256,
        block_hash: H256,
        complete: bool,
    ) -> Result<Option<u32>, ClientError> {
        let raw: Value = self
            .read_request(
                rtao_rpc::ReadMethod::ChainGetBlock,
                json!([hash_hex(block_hash)]),
            )
            .await?;
        if raw.is_null() {
            return Err(ClientError::BlockNotFound(block_hash));
        }
        if complete
            && raw
                .pointer("/block/extrinsics")
                .and_then(Value::as_array)
                .is_none()
        {
            return Err(ClientError::Protocol(
                "finalized recovery requires a complete extrinsics array".into(),
            ));
        }
        rtao_core::ops::extrinsic_lookup::find(&raw, extrinsic_hash.0)
            .map_err(rtao_core::CoreError::from)
            .map_err(ClientError::from)
    }
}

fn startup_timeout_error(timeout: Duration) -> ClientError {
    rtao_rpc::RpcError::Timeout {
        detail: format!(
            "client startup did not finish within {}s; the endpoint may be slow or \
             throttling this connection",
            timeout.as_secs_f64()
        ),
    }
    .into()
}

fn head_error(error: crate::head::HeadError) -> ClientError {
    match error {
        crate::head::HeadError::Rpc(error) => ClientError::from(error),
        error => ClientError::Protocol(error.to_string()),
    }
}

#[derive(Default)]
struct HashCache {
    finalized: Option<(u64, tokio::time::Instant)>,
    hashes: std::collections::BTreeMap<u64, H256>,
    order: std::collections::VecDeque<u64>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::{View, ViewExt};
    use rtao_testkit::fakes::{FakeChain, FakeState, SubscriptionScript};

    fn config(input: &str) -> ClientConfig {
        let mut config =
            ClientConfig::new(Network::resolve_with(input, |_| None).unwrap()).unwrap();
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.session.rate_limit = None;
        config
    }

    #[test]
    fn submit_endpoint_context_names_the_role_and_keeps_the_variant() {
        use rtao_rpc::RpcError;
        let wrap = |error: RpcError| match submit_endpoint_context(error.into()) {
            ClientError::Rpc(error) => error,
            other => panic!("variant changed: {other:?}"),
        };
        assert!(matches!(
            wrap(RpcError::Transport("refused".into())),
            RpcError::Transport(detail) if detail == "submit endpoint: refused"
        ));
        assert!(matches!(
            wrap(RpcError::Protocol("bad frame".into())),
            RpcError::Protocol(detail) if detail == "submit endpoint: bad frame"
        ));
        assert!(matches!(
            wrap(RpcError::Timeout { detail: "handshake".into() }),
            RpcError::Timeout { detail } if detail == "submit endpoint: handshake"
        ));
        assert!(matches!(
            wrap(RpcError::MaxRetriesExceeded { detail: "down".into() }),
            RpcError::MaxRetriesExceeded { detail } if detail == "submit endpoint: down"
        ));
        assert!(matches!(
            wrap(RpcError::Handshake { status: 403, detail: String::new() }),
            RpcError::Handshake { status: 403, detail } if detail == "submit endpoint"
        ));
        // Variants without a diagnostic of their own pass through unchanged.
        assert_eq!(wrap(RpcError::Closed), RpcError::Closed);
        assert_eq!(
            wrap(RpcError::ConnectionLost { resent: false }),
            RpcError::ConnectionLost { resent: false }
        );
        let fate = RpcError::FateUnknown {
            hash: Default::default(),
        };
        assert_eq!(wrap(fate.clone()), fate);
        let hash = H256([3; 32]);
        assert!(matches!(
            submit_endpoint_context(ClientError::Protocol("no runtime".into())),
            ClientError::Protocol(detail) if detail == "submit endpoint: no runtime"
        ));
        assert!(matches!(
            submit_endpoint_context(ClientError::BlockNotFound(hash)),
            ClientError::BlockNotFound(found) if found == hash
        ));
    }

    #[test]
    fn submit_endpoint_gets_its_own_pool_and_endpoint_pacing() {
        let mut cfg = config("local");
        assert!(cfg.submit.is_none());
        cfg.session.retry_forever = true;
        cfg.set_submit_endpoint("wss://node.example:443").unwrap();
        let submit = cfg.submit.clone().unwrap();
        assert_eq!(submit.pool.urls.len(), 1);
        assert_eq!(submit.pool.urls[0].host_str(), Some("node.example"));
        assert!(submit.rate_limit.is_none(), "a custom endpoint is unpaced");
        assert!(submit.retry_forever);
        // The main session's pool is untouched.
        assert_eq!(cfg.session.pool.urls, config("local").session.pool.urls);
        cfg.set_submit_endpoint("finney").unwrap();
        assert!(
            cfg.submit.unwrap().rate_limit.is_some(),
            "a public preset is paced like any public endpoint"
        );
        assert!(config("local").set_submit_endpoint("not a url").is_err());
    }

    #[tokio::test]
    async fn read_at_one_block_serves_every_head_read_from_the_first() {
        let raw = FakeChain::from_state(FakeState::default());
        let mut cfg = config("local");
        cfg.track_heads = false;
        cfg.read_at_one_block = true;
        let client = Client::from_rpc(RpcClient::new(raw.clone()), cfg)
            .await
            .unwrap();
        let headers = || {
            raw.requests()
                .iter()
                .filter(|request| request.method == "chain_getHeader")
                .count()
        };
        let first = View::pin(&client).await.unwrap().block().hash;
        raw.advance(3).unwrap();
        let (again, best) = futures::join!(View::pin(&client), client.at(crate::BlockRef::Best));
        assert_eq!(again.unwrap().block().hash, first);
        assert_eq!(best.unwrap().block().hash, first);
        // Startup's display-symbol scan already pinned the block the command reuses.
        assert_eq!(
            headers(),
            1,
            "one header request for startup and the command"
        );
        // Other block references still resolve live.
        let finalized = client.at(crate::BlockRef::Finalized).await.unwrap();
        assert_ne!(finalized.block().hash, first);
        client.close().await;
    }

    #[tokio::test]
    async fn steady_state_runtime_watch_makes_no_per_block_version_requests() {
        use std::time::Duration;

        let raw = FakeChain::from_state(FakeState::default());
        let client = Client::from_rpc(RpcClient::new(raw.clone()), config("local"))
            .await
            .unwrap();
        let versions = || {
            raw.requests()
                .iter()
                .filter(|request| request.method == "state_getRuntimeVersion")
                .count()
        };
        let settle = || async {
            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        settle().await;
        // The subscription's initial item is installed without a version request.
        assert!(
            raw.requests()
                .iter()
                .any(|request| request.method == "state_subscribeRuntimeVersion")
        );
        // Startup checks finalized and head once each; the display symbol scan
        // resolves the runtime of the block it pins.
        let startup = versions();
        assert!(startup <= 3, "{startup} startup version requests");
        raw.advance(6).unwrap();
        settle().await;
        assert_eq!(
            versions(),
            startup,
            "unchanged runtimes need no per-block recheck"
        );
        client.close().await;
    }

    #[tokio::test]
    async fn runtime_version_subscription_refreshes_codec_without_explicit_refresh() {
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };

        let started = Instant::now();
        let raw = FakeChain::from_state(FakeState::default());
        // Suppress the initial version item so the only subscription notification
        // under test is the upgrade published below.
        raw.mutate_state(|state| {
            state.queue_subscription(
                "state_subscribeRuntimeVersion",
                SubscriptionScript::new(vec![]),
            );
        });
        let client = Client::from_rpc(RpcClient::new(raw.clone()), config("local"))
            .await
            .unwrap();
        let old_runtime = View::runtime(&client).await.unwrap();
        let descriptor = &rtao_runtime::constants::system::BLOCK_HASH_COUNT;
        let old_constant = client.constant(descriptor).await.unwrap();
        let old_call = rtao_runtime::calls::system::remark(vec![])
            .compose(&old_runtime)
            .unwrap();
        // Wait for the subscription before changing state. The fallback poll's
        // first tick is twelve seconds away and cannot satisfy the five-second
        // assertion.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let requests = raw.requests();
                let versions = requests
                    .iter()
                    .filter(|request| {
                        request.method == "state_getRuntimeVersion"
                            && request.params == json!([null])
                    })
                    .count();
                if versions >= 1
                    && requests
                        .iter()
                        .any(|request| request.method == "state_subscribeRuntimeVersion")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        raw.mutate_state(|state| {
            state
                .seed_constant("System", "BlockHashCount", json!(old_constant + 1))
                .unwrap();
            state.runtime = Arc::new(
                rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 2, 42)
                    .unwrap(),
            );
        });
        raw.publish_method(
            "state_subscribeRuntimeVersion",
            json!({"specVersion":479, "transactionVersion":2}),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            // Only inspect the installed cache until the watcher has completed.
            // This never fetches metadata or polls an in-flight refresh for it.
            loop {
                if let Some(runtime) = client.inner.cache.peek_head().await
                    && runtime.spec_version() == 479
                    && runtime.transaction_version() == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let requests_before_observation = raw.requests().len();
            let runtime = View::runtime(&client).await.unwrap();
            assert_eq!(runtime.spec_version(), 479);
            assert_eq!(runtime.transaction_version(), 2);
            assert_ne!(
                runtime.core().metadata_bytes,
                old_runtime.core().metadata_bytes
            );
            assert_eq!(client.constant(descriptor).await.unwrap(), old_constant + 1);
            let upgraded_call = rtao_runtime::calls::system::remark(vec![])
                .compose(&runtime)
                .unwrap();
            assert_eq!(upgraded_call.stamp(), runtime.stamp());
            assert!(old_call.ensure_runtime(&runtime).is_err());
            assert_eq!(old_runtime.spec_version(), 481);
            assert_eq!(old_runtime.transaction_version(), 1);
            assert_eq!(
                raw.requests().len(),
                requests_before_observation,
                "foreground observations must use the head installed by the watcher"
            );
        })
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "upgrade observation must precede the second periodic check"
        );
        client.close().await;
    }
}
