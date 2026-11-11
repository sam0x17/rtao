// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Account nonce tracking shared by transaction helpers.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex as SyncMutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::future::BoxFuture;
use rtao_core::value_ext::ValueExt;
use rtao_primitives::AccountId;
use subxt_rpcs::RpcClient;
use tokio::sync::Mutex;

use crate::ClientError;

/// Ready-pool reads made while this account has a pending MEV-shield carrier,
/// before a fresh nonce is taken from `account_nextIndex` regardless.
pub const SHIELD_CARRIER_POLLS: u32 = 10;

/// Wait before retry `attempt` (from zero): one block, doubling to eight.
pub(crate) fn block_backoff(block: Duration, attempt: u32) -> Duration {
    block.saturating_mul(1 << attempt.min(3))
}

/// Waits while the submission endpoint's ready pool holds an MEV-shield
/// carrier from the account, reporting whether it slept. Fails only when the
/// client has closed or its submission endpoint cannot be opened, which the
/// nonce read needs too.
pub(crate) type CarrierWait =
    Arc<dyn Fn(AccountId) -> BoxFuture<'static, Result<bool, ClientError>> + Send + Sync>;

/// Per-account next-index cache for pipelined submissions.
///
/// An explicit or node-sourced nonce becomes the last allocated value. The next
/// allocation increments it. Failed submissions clear the signer's entry.
#[derive(Clone)]
pub struct NonceManager {
    rpc: RpcClient,
    /// Separate submission endpoint that answers nonce queries, when set.
    route: Option<Arc<crate::client::SubmitRoute>>,
    /// Shield-aware wait run before every fresh `account_nextIndex` read.
    carriers: Option<CarrierWait>,
    accounts: Arc<SyncMutex<BTreeMap<String, Arc<AccountNonce>>>>,
}

struct AccountNonce {
    fetch: Mutex<()>,
    state: SyncMutex<NonceState>,
}
#[derive(Default)]
struct NonceState {
    latest: Option<u64>,
    generation: u64,
}

fn lock<T>(mutex: &SyncMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalidate(cell: &Weak<AccountNonce>) {
    if let Some(cell) = cell.upgrade() {
        let mut state = lock(&cell.state);
        state.latest = None;
        state.generation = state.generation.wrapping_add(1);
    }
}

pub(crate) struct NonceInvalidation {
    cell: Weak<AccountNonce>,
    settled: bool,
}
impl NonceInvalidation {
    pub(crate) fn settle(mut self) {
        self.settled = true;
    }
}
impl Drop for NonceInvalidation {
    fn drop(&mut self) {
        if !self.settled {
            invalidate(&self.cell);
        }
    }
}

/// One reserved nonce, invalidating its account cache on every unsettled exit.
///
/// Settlement records pool acceptance or a resolved inclusion, regardless of
/// dispatch outcome. Cancellation, signing failures and unknown fate leave the
/// lease unsettled. Dropping it clears synchronously without spawning a task.
#[must_use = "an unsettled nonce lease clears the account cache when dropped"]
pub struct NonceLease {
    who: AccountId,
    nonce: u64,
    cell: Weak<AccountNonce>,
    settled: bool,
}

impl std::fmt::Debug for NonceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NonceLease")
            .field("who", &self.who)
            .field("nonce", &self.nonce)
            .field("settled", &self.settled)
            .finish()
    }
}
impl NonceLease {
    /// The account for which this nonce was allocated.
    pub fn account(&self) -> AccountId {
        self.who
    }
    /// The reserved nonce value.
    pub fn value(&self) -> u64 {
        self.nonce
    }
    /// Consume this lease after pool acceptance or resolved inclusion.
    pub fn settle(mut self) {
        self.settled = true;
    }
}
impl Drop for NonceLease {
    fn drop(&mut self) {
        if !self.settled {
            invalidate(&self.cell);
        }
    }
}

impl NonceManager {
    pub(crate) fn invalidate_on_drop(&self, address: &str) -> NonceInvalidation {
        NonceInvalidation {
            cell: Arc::downgrade(&self.cell(address)),
            settled: false,
        }
    }
    /// Start an empty cache over the client's transport.
    pub fn new(rpc: RpcClient) -> Self {
        Self::with_route(rpc, None)
    }

    /// Start an empty cache whose queries use the submission endpoint, if any.
    pub(crate) fn with_route(
        rpc: RpcClient,
        route: Option<Arc<crate::client::SubmitRoute>>,
    ) -> Self {
        Self {
            rpc,
            route,
            carriers: None,
            accounts: Arc::new(SyncMutex::new(BTreeMap::new())),
        }
    }

    /// Wait for the account's pending shield carriers before each fresh read.
    pub(crate) fn with_carrier_wait(mut self, wait: CarrierWait) -> Self {
        self.carriers = Some(wait);
        self
    }

    fn cell(&self, address: &str) -> Arc<AccountNonce> {
        let normalized = address
            .parse::<AccountId>()
            .map(|who| who.to_ss58(42))
            .unwrap_or_else(|_| address.to_owned());
        lock(&self.accounts)
            .entry(normalized)
            .or_insert_with(|| {
                Arc::new(AccountNonce {
                    fetch: Mutex::new(()),
                    state: SyncMutex::new(NonceState::default()),
                })
            })
            .clone()
    }

    /// The first allocated nonce, its cell and whether a fresh read waited
    /// for pending shield carriers.
    async fn allocate(
        &self,
        address: &str,
        count: u64,
    ) -> Result<(u64, Arc<AccountNonce>, bool), ClientError> {
        let cell = self.cell(address);
        let _fetch = cell.fetch.lock().await;
        let mut waited = false;
        loop {
            let (latest, generation) = {
                let state = lock(&cell.state);
                (state.latest, state.generation)
            };
            let first = match latest {
                Some(nonce) => nonce
                    .checked_add(1)
                    .ok_or_else(|| ClientError::Protocol("account nonce overflow".into()))?,
                None => {
                    let (nonce, slept) = self.query_waited(address).await?;
                    waited |= slept;
                    nonce
                }
            };
            let last = first
                .checked_add(count - 1)
                .ok_or_else(|| ClientError::Protocol("account nonce overflow".into()))?;
            let mut state = lock(&cell.state);
            // A lease dropped, clear or explicit pin occurred during the RPC.
            // Its result cannot overwrite that newer cache state.
            if state.generation != generation {
                continue;
            }
            state.latest = Some(last);
            return Ok((first, cell.clone(), waited));
        }
    }

    /// Reserve one nonce, serialized with other allocations for this account.
    pub async fn lease(&self, who: &AccountId) -> Result<NonceLease, ClientError> {
        let (nonce, cell, _) = self.allocate(&who.to_ss58(42), 1).await?;
        Ok(NonceLease {
            who: *who,
            nonce,
            cell: Arc::downgrade(&cell),
            settled: false,
        })
    }

    /// Reserve adjacent inner/carrier nonces atomically for a shield submission.
    pub async fn lease_pair(
        &self,
        who: &AccountId,
    ) -> Result<(NonceLease, NonceLease), ClientError> {
        self.lease_pair_waited(who).await.map(|(pair, _)| pair)
    }

    /// [`Self::lease_pair`], also reporting whether the fresh nonce read slept
    /// while pending shield carriers left the pool, which ages era anchors.
    pub(crate) async fn lease_pair_waited(
        &self,
        who: &AccountId,
    ) -> Result<((NonceLease, NonceLease), bool), ClientError> {
        let (nonce, cell, waited) = self.allocate(&who.to_ss58(42), 2).await?;
        let make = |nonce| NonceLease {
            who: *who,
            nonce,
            cell: Arc::downgrade(&cell),
            settled: false,
        };
        Ok(((make(nonce), make(nonce + 1)), waited))
    }

    /// Query the pool-aware nonce without reserving or modifying cached state.
    pub async fn fresh(&self, who: &AccountId) -> Result<u64, ClientError> {
        self.query(&who.to_ss58(42)).await
    }

    /// The node's `account_nextIndex` as reported, without waiting for pending
    /// shield carriers. For fee probes, where the nonce does not matter.
    pub(crate) async fn reported(&self, who: &AccountId) -> Result<u64, ClientError> {
        self.next_index(&who.to_ss58(42)).await
    }

    /// Pin an explicit nonce synchronously as the account's latest allocation.
    pub fn pin(&self, who: &AccountId, nonce: u64) {
        self.pin_address(&who.to_ss58(42), nonce);
    }

    /// Reserve and pin an explicit nonce, retaining failure invalidation.
    pub fn lease_explicit(&self, who: &AccountId, nonce: u64) -> NonceLease {
        let cell = self.cell(&who.to_ss58(42));
        {
            let mut state = lock(&cell.state);
            state.latest = Some(nonce);
            state.generation = state.generation.wrapping_add(1);
        }
        NonceLease {
            who: *who,
            nonce,
            cell: Arc::downgrade(&cell),
            settled: false,
        }
    }

    /// Clear an account synchronously after a failed or cancelled submission.
    pub fn clear_account(&self, who: &AccountId) {
        self.clear_address(&who.to_ss58(42));
    }

    fn pin_address(&self, address: &str, nonce: u64) {
        let cell = self.cell(address);
        let mut state = lock(&cell.state);
        state.latest = Some(nonce);
        state.generation = state.generation.wrapping_add(1);
    }
    fn clear_address(&self, address: &str) {
        let cell = self.cell(address);
        let mut state = lock(&cell.state);
        state.latest = None;
        state.generation = state.generation.wrapping_add(1);
    }

    /// Allocate the next nonce, or query without modifying the cache.
    pub async fn next_for(&self, address: &str, use_cache: bool) -> Result<u64, ClientError> {
        if !use_cache {
            return self.query(address).await;
        }
        self.allocate(address, 1).await.map(|(nonce, _, _)| nonce)
    }

    async fn query(&self, address: &str) -> Result<u64, ClientError> {
        self.query_waited(address).await.map(|(nonce, _)| nonce)
    }

    /// The fresh nonce and whether it waited for pending shield carriers.
    async fn query_waited(&self, address: &str) -> Result<(u64, bool), ClientError> {
        // A pending shield carrier at n holds n+1 for its encrypted inner,
        // which the pool cannot see: account_nextIndex would answer n+1 and a
        // transaction signed with it is invalidated when the pair lands.
        let mut waited = false;
        if let Some(wait) = &self.carriers
            && let Ok(who) = address.parse::<AccountId>()
        {
            waited = wait(who).await?;
        }
        Ok((self.next_index(address).await?, waited))
    }

    async fn next_index(&self, address: &str) -> Result<u64, ClientError> {
        let params = serde_json::json!([address]);
        // The pool-aware nonce comes from the node that receives submissions.
        let value = match &self.route {
            Some(route) => {
                let client = route.client().await?;
                crate::rpc_request(client.rpc(), "account_nextIndex", params).await?
            }
            None => crate::rpc_request(&self.rpc, "account_nextIndex", params).await?,
        };
        value
            .as_u64()
            .or_else(|| {
                value.as_str().and_then(|s| {
                    s.strip_prefix("0x")
                        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                        .or_else(|| s.parse().ok())
                })
            })
            .ok_or_else(|| {
                ClientError::Protocol("account_nextIndex returned an invalid nonce".into())
            })
    }

    /// Record an explicitly selected nonce as the account's latest allocation.
    pub async fn pin_for_address(&self, address: &str, nonce: u64) {
        self.pin_address(address, nonce);
    }

    /// Forget the cached next index after a failed submission.
    pub async fn clear(&self, address: &str) {
        self.clear_address(address);
    }

    /// Inspect the latest allocated nonce without contacting the node.
    pub async fn cached(&self, address: &str) -> Option<u64> {
        lock(&self.cell(address).state).latest
    }
}

impl crate::Client {
    /// Shield-aware wait used by this client's nonce manager.
    pub(crate) fn carrier_wait(
        inner: Weak<crate::client::ClientInner>,
        alive: Weak<tokio_util::sync::DropGuard>,
    ) -> CarrierWait {
        let unavailable = Arc::new(AtomicBool::new(false));
        Arc::new(move |who| {
            let (inner, alive, unavailable) = (inner.clone(), alive.clone(), unavailable.clone());
            Box::pin(async move {
                let Some(client) = crate::Client::upgrade(&inner, &alive) else {
                    return Err(rtao_jsonrpc::RpcError::Closed.into());
                };
                client.wait_for_pending_carriers(who, &unavailable).await
            })
        })
    }

    /// Wait while the submission endpoint's ready pool holds a shield carrier
    /// signed by `who`, polling at most [`SHIELD_CARRIER_POLLS`] times.
    ///
    /// The wait never skips past the inner's nonce: a carrier whose inner is
    /// dropped would leave that nonce unused, and every later transaction
    /// would wait in the future queue. The carrier leaves the pool within its
    /// era of at most eight blocks, included with its inner or expired. When
    /// the pool cannot be read or the polls run out this returns, and the
    /// caller uses `account_nextIndex` as reported. Reports whether it slept.
    async fn wait_for_pending_carriers(
        &self,
        who: AccountId,
        unavailable: &AtomicBool,
    ) -> Result<bool, ClientError> {
        if unavailable.load(Ordering::Acquire) {
            return Ok(false);
        }
        let chain = self.submit_client().await?;
        let mut block = None;
        for poll in 0..SHIELD_CARRIER_POLLS {
            let carriers = match chain.pending_carrier_nonces(who).await {
                Ok(carriers) => carriers,
                Err(error) => {
                    self.ensure_open()?;
                    if matches!(
                        &error,
                        ClientError::Rpc(rtao_jsonrpc::RpcError::JsonRpc { code: -32601, .. })
                    ) {
                        unavailable.store(true, Ordering::Release);
                    }
                    tracing::warn!(
                        %error,
                        "cannot read the transaction pool for pending MEV-shield carriers; \
                         using account_nextIndex, which cannot see a pending carrier's inner nonce"
                    );
                    return Ok(poll > 0);
                }
            };
            if carriers.is_empty() {
                return Ok(poll > 0);
            }
            if poll + 1 == SHIELD_CARRIER_POLLS {
                tracing::warn!(
                    ?carriers,
                    polls = SHIELD_CARRIER_POLLS,
                    "MEV-shield carriers from this account are still pending; using \
                     account_nextIndex, which cannot see their inner nonces"
                );
                return Ok(poll > 0);
            }
            let interval = match block {
                Some(interval) => interval,
                None => match crate::ViewExt::block_time(&chain).await {
                    Ok(interval) => *block.insert(interval),
                    Err(error) => {
                        self.ensure_open()?;
                        tracing::warn!(
                            %error,
                            ?carriers,
                            "cannot read the block time to wait for pending MEV-shield \
                             carriers; using account_nextIndex, which cannot see their inner nonces"
                        );
                        return Ok(poll > 0);
                    }
                },
            };
            let delay = block_backoff(interval, poll);
            tracing::info!(
                ?carriers,
                ?delay,
                "waiting for this account's pending MEV-shield carriers before reading its next nonce"
            );
            tokio::select! {
                biased;
                () = self.closed() => return Err(rtao_jsonrpc::RpcError::Closed.into()),
                () = tokio::time::sleep(delay) => {}
            }
        }
        Ok(true)
    }

    /// Nonces of `who`'s `MevShield.submit_encrypted` carriers in this
    /// endpoint's ready pool, matched by name through the head runtime.
    ///
    /// Only entries carrying `who`'s key are decoded. One of those that the
    /// head runtime cannot read, such as after an extrinsic format change, is
    /// logged with its error and counted as a possible carrier with an
    /// unknown nonce, so the bounded wait still applies instead of silently
    /// switching off.
    async fn pending_carrier_nonces(
        &self,
        who: AccountId,
    ) -> Result<Vec<Option<u64>>, ClientError> {
        let pending = self
            .read_request(
                rtao_jsonrpc::ReadMethod::AuthorPendingExtrinsics,
                serde_json::json!([]),
            )
            .await?;
        let entries = pending.as_array().ok_or_else(|| {
            ClientError::Protocol("author_pendingExtrinsics did not return an array".into())
        })?;
        // Any signed encoding of this account carries its 32-byte key, so
        // only those entries are decoded.
        let candidates = entries
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(|text| hex::decode(text.strip_prefix("0x").unwrap_or(text)).ok())
            .filter(|bytes| bytes.windows(32).any(|window| window == who.0))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let runtime = self.runtime_for(None).await?;
        let large = candidates.iter().map(Vec::len).sum::<usize>() > 64 * 1024;
        let decode = move || {
            candidates
                .iter()
                .filter_map(|bytes| match carrier_nonce(runtime.core(), who, bytes) {
                    Ok(carrier) => carrier,
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            account = %who,
                            "a pending extrinsic carrying this account's key does not decode \
                             under the head runtime; treating it as a possible MEV-shield carrier"
                        );
                        Some(None)
                    }
                })
                .collect::<Vec<_>>()
        };
        if large {
            crate::offload(decode).await.map_err(|error| {
                ClientError::Protocol(format!("pool decoder task failed: {error}"))
            })
        } else {
            Ok(decode())
        }
    }
}

/// The nonce of a signed `MevShield.submit_encrypted` from `who`, if `bytes`
/// is one under this runtime.
///
/// Fails when `bytes` does not decode, or is signed by `who` with a call whose
/// pallet and name cannot be read; neither proves it is not a carrier.
fn carrier_nonce(
    runtime: &rtao_core::runtime::Runtime,
    who: AccountId,
    bytes: &[u8],
) -> Result<Option<Option<u64>>, String> {
    let decoded = runtime
        .decode_extrinsic(bytes, true)
        .map_err(|error| error.to_string())?;
    // Unsigned: no account's carrier.
    let Some(address) = decoded.field("address") else {
        return Ok(None);
    };
    let signer = address
        .as_str()
        .and_then(|text| text.parse::<AccountId>().ok())
        .or_else(|| {
            address
                .value_bytes()
                .and_then(|bytes| bytes.try_into().ok())
                .map(AccountId)
        });
    if signer != Some(who) {
        return Ok(None);
    }
    let call = decoded.field("call");
    let name = |field: &str| {
        call.and_then(|call| call.field(field))
            .and_then(ValueExt::as_str)
    };
    let (Some(module), Some(function)) = (name("call_module"), name("call_function")) else {
        return Err("signed by this account, but its call's pallet or name is unreadable".into());
    };
    if module != "MevShield" || function != "submit_encrypted" {
        return Ok(None);
    }
    // A carrier without a readable nonce field still holds an inner nonce.
    Ok(Some(
        decoded
            .field("nonce")
            .and_then(ValueExt::as_u128)
            .and_then(|nonce| u64::try_from(nonce).ok()),
    ))
}
