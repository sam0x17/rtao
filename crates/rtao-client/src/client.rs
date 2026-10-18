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
    /// Allow the exact-shape borrowed metagraph decoder for this connection.
    ///
    /// Setting this to false applies to the client, its snapshots and dynamic
    /// views. Per-read options and `RTAO_METAGRAPH_VIEW=0` can also disable it.
    pub metagraph_view: bool,
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
            metagraph_view: true,
        })
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
    cancelled: CancellationToken,
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
    pub async fn connect(config: ClientConfig) -> Result<Self, ClientError> {
        let timeout = config.startup_timeout;
        let startup = async {
            let session = Session::connect(config.session.clone())
                .await
                .map_err(ClientError::from)?;
            let rpc = RpcClient::new(session.clone());
            match Self::initialize(rpc, config, Some(session.clone())).await {
                Ok(client) => Ok(client),
                Err(error) => {
                    session.close().await;
                    Err(error)
                }
            }
        };
        tokio::time::timeout(timeout, startup).await.map_err(|_| {
            ClientError::from(rtao_rpc::RpcError::Timeout {
                detail: String::new(),
            })
        })?
    }

    /// Initialize over a supplied RPC implementation, including fake chains.
    pub async fn from_rpc(rpc: RpcClient, config: ClientConfig) -> Result<Self, ClientError> {
        tokio::time::timeout(config.startup_timeout, Self::initialize(rpc, config, None))
            .await
            .map_err(|_| {
                ClientError::from(rtao_rpc::RpcError::Timeout {
                    detail: String::new(),
                })
            })?
    }

    async fn initialize(
        rpc: RpcClient,
        config: ClientConfig,
        session: Option<Session>,
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
        let properties = rpc_request(&rpc, "system_properties", json!([])).await?;
        let ss58 = properties
            .get("ss58Format")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                ClientError::Protocol("system_properties returned no ss58Format".into())
            })?;
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
        let runtime = cache.runtime_at_state(finalized).await?;
        let head_runtime = cache.refresh_head().await?;
        let _ = runtime.surface_report();
        let _ = head_runtime.surface_report();
        if !crate::compat::runtime_validated(genesis, &runtime) {
            // Evidence can only cover pinned public networks; a local or
            // custom development chain is expected to be outside it.
            if crate::compat::pinned_public_genesis(genesis) {
                tracing::warn!(
                    spec_version = runtime.spec_version(),
                    "connected runtime is outside the compiled validated compatibility evidence"
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
            let best_hash = parse_hash(&rpc_request(&rpc, "chain_getBlockHash", json!([])).await?)?;
            let best_raw =
                rpc_request(&rpc, "chain_getHeader", json!([hash_hex(best_hash)])).await?;
            let finalized_raw =
                rpc_request(&rpc, "chain_getHeader", json!([hash_hex(finalized)])).await?;
            let best_header =
                crate::BlockHeader::from_json_with_hash(best_raw, best_hash).map_err(head_error)?;
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
        let alive = Arc::new(cancelled.clone().drop_guard());
        let inner = Arc::new(ClientInner {
            rpc: rpc.clone(),
            session,
            config,
            genesis,
            signing_disabled,
            cache,
            nonces: NonceManager::new(rpc),
            cancelled,
            heads,
            token_symbols: tokio::sync::RwLock::new(std::collections::BTreeMap::new()),
            hash_cache: tokio::sync::Mutex::new(HashCache::default()),
        });
        let client = Self {
            inner: inner.clone(),
            _alive: alive,
        };
        // This worker holds no lifetime guard. Dropping the last Client clone
        // wakes it and closes the underlying session before releasing state.
        tokio::spawn(async move {
            inner.cancelled.cancelled().await;
            if let Some(session) = &inner.session {
                session.close().await;
            }
        });
        client.start_runtime_watcher();
        client.load_token_symbols().await;
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
            let page = View::keys_paged(&snapshot, &prefix, 128, start.as_deref()).await?;
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

    fn start_runtime_watcher(&self) {
        if !self.inner.config.track_heads {
            return;
        }
        let inner = self.inner.clone();
        let mut finalized = self.inner.heads.as_ref().map(crate::HeadTracker::finalized);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(12));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = inner.cancelled.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = inner.cache.refresh_head().await {
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
                            if let Err(error) = inner.cache.runtime_at_state(hash).await {
                                tracing::warn!(%error,"finalized runtime version refresh failed");
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
                        Some(Ok(_)) => {
                            inner.cache.invalidate_head().await;
                            if let Err(error) = inner.cache.refresh_head().await {
                                tracing::warn!(%error,"runtime upgrade refresh failed");
                            }
                        }
                        Some(Err(error)) => {
                            tracing::warn!(%error,"runtime subscription lost");
                            break;
                        }
                        None => break,
                    }
                }
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

    /// Resolve a requested block once and retain its parent-selected runtime.
    pub async fn at(&self, block: crate::BlockRef) -> Result<crate::Snapshot, ClientError> {
        self.ensure_open()?;
        let hash = match block {
            crate::BlockRef::Hash(hash) => hash,
            crate::BlockRef::Best => {
                parse_hash(&self.request("chain_getBlockHash", json!([])).await?)?
            }
            crate::BlockRef::Finalized => {
                parse_hash(&self.request("chain_getFinalizedHead", json!([])).await?)?
            }
            crate::BlockRef::Number(number) => self.hash_for_number(number).await?,
        };
        let raw: Value = self
            .request("chain_getHeader", json!([hash_hex(hash)]))
            .await?;
        if raw.is_null() {
            return Err(ClientError::BlockNotFound(hash));
        }
        let header = crate::BlockHeader::from_json_with_hash(raw, hash).map_err(head_error)?;
        let runtime = self.runtime_for(Some(hash)).await?;
        Ok(crate::Snapshot::new(
            self.clone(),
            crate::PinnedBlock {
                number: header.number,
                hash,
                runtime,
            },
        ))
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
        let raw: Value = self
            .read_request(
                rtao_rpc::ReadMethod::ChainGetBlock,
                json!([hash_hex(block_hash)]),
            )
            .await?;
        if raw.is_null() {
            return Err(ClientError::BlockNotFound(block_hash));
        }
        rtao_core::ops::extrinsic_lookup::find(&raw, extrinsic_hash.0)
            .map_err(rtao_core::CoreError::from)
            .map_err(ClientError::from)
    }
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
        // Let the watcher's immediate periodic check finish before changing state.
        // Its next twelve-second tick cannot satisfy the five-second assertion.
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
                if versions >= 2
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
                rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 480, 2, 42)
                    .unwrap(),
            );
        });
        raw.publish_method(
            "state_subscribeRuntimeVersion",
            json!({"specVersion":480, "transactionVersion":2}),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            // Only inspect the installed cache until the watcher has completed.
            // This never fetches metadata or polls an in-flight refresh for it.
            loop {
                if let Some(runtime) = client.inner.cache.peek_head().await
                    && runtime.spec_version() == 480
                    && runtime.transaction_version() == 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let requests_before_observation = raw.requests().len();
            let runtime = View::runtime(&client).await.unwrap();
            assert_eq!(runtime.spec_version(), 480);
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
            assert_eq!(old_runtime.spec_version(), 479);
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
