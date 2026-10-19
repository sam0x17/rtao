// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/extrinsics.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Account nonce tracking shared by transaction helpers.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex as SyncMutex, MutexGuard, Weak},
};

use rtao_primitives::AccountId;
use subxt_rpcs::RpcClient;
use tokio::sync::Mutex;

use crate::ClientError;

/// Per-account next-index cache for pipelined submissions.
///
/// An explicit or node-sourced nonce becomes the last allocated value. The next
/// allocation increments it. Failed submissions clear the signer's entry.
#[derive(Clone)]
pub struct NonceManager {
    rpc: RpcClient,
    /// Separate submission endpoint that answers nonce queries, when set.
    route: Option<Arc<crate::client::SubmitRoute>>,
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
            accounts: Arc::new(SyncMutex::new(BTreeMap::new())),
        }
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

    async fn allocate(
        &self,
        address: &str,
        count: u64,
    ) -> Result<(u64, Arc<AccountNonce>), ClientError> {
        let cell = self.cell(address);
        let _fetch = cell.fetch.lock().await;
        loop {
            let (latest, generation) = {
                let state = lock(&cell.state);
                (state.latest, state.generation)
            };
            let first = match latest {
                Some(nonce) => nonce
                    .checked_add(1)
                    .ok_or_else(|| ClientError::Protocol("account nonce overflow".into()))?,
                None => self.query(address).await?,
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
            return Ok((first, cell.clone()));
        }
    }

    /// Reserve one nonce, serialized with other allocations for this account.
    pub async fn lease(&self, who: &AccountId) -> Result<NonceLease, ClientError> {
        let (nonce, cell) = self.allocate(&who.to_ss58(42), 1).await?;
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
        let (nonce, cell) = self.allocate(&who.to_ss58(42), 2).await?;
        let make = |nonce| NonceLease {
            who: *who,
            nonce,
            cell: Arc::downgrade(&cell),
            settled: false,
        };
        Ok((make(nonce), make(nonce + 1)))
    }

    /// Query the pool-aware nonce without reserving or modifying cached state.
    pub async fn fresh(&self, who: &AccountId) -> Result<u64, ClientError> {
        self.query(&who.to_ss58(42)).await
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
        self.allocate(address, 1).await.map(|(nonce, _)| nonce)
    }

    async fn query(&self, address: &str) -> Result<u64, ClientError> {
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
