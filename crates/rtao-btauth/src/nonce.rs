//! Atomic, bounded replay protection for accepted btauth requests.

use std::collections::{HashMap, HashSet, VecDeque, hash_map::RandomState};
use std::fmt;
use std::future::Future;
use std::hash::BuildHasher;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use num_bigint::BigInt;
use rtao_primitives::AccountId;

const SHARD_COUNT: usize = 16;

/// A sendable, boxed future used at the object-safe nonce-store boundary.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A replay-store failure, distinct from a request that reuses a nonce.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    /// A capacity limit prevents remembering another live nonce.
    ///
    /// No existing live entry is evicted to admit the request.
    Full,
    /// The store cannot safely complete an atomic operation.
    Unavailable(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("nonce store is full"),
            Self::Unavailable(reason) => write!(f, "nonce store is unavailable: {reason}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Atomic replay protection shared by a verifier's accepted requests.
///
/// Implementations must atomically check and insert the `(hotkey, nonce_ns)`
/// pair. `Ok(true)` means the pair was stored; `Ok(false)` means it was already
/// present. A failed insertion must not remove an unexpired pair. Shared
/// backends can perform asynchronous I/O through this object-safe interface.
#[doc(alias = "bittensor.http_auth.NonceStore")]
pub trait NonceStore: Send + Sync {
    /// The minimum time accepted pairs remain protected against replay.
    fn retention(&self) -> Duration;

    /// Atomically accept a previously unseen pair or report a replay.
    ///
    /// Nonces retain the full signed integer parsed from the wire. Borrow the
    /// integer until insertion; replay attempts need no key allocation.
    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce_ns: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>>;
}

/// Retention and hard capacity limits for [`InMemoryNonceStore`].
///
/// Defaults retain pairs for 60 seconds, permit 1,024 entries per hotkey, and
/// permit 65,536 entries across all hotkeys. These finite defaults are a
/// starting point for a single server. Configure the per-hotkey limit to at
/// least `ceil(limiter_rate * retention_seconds) + burst` for the server's
/// intended traffic, and size the global limit for its admitted hotkeys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NonceStoreConfig {
    /// Time after insertion before a pair may be accepted again.
    ///
    /// A zero duration is allowed, matching Python's store. A verifier must
    /// separately require retention to cover its freshness window.
    pub retention: Duration,
    /// Maximum number of unexpired nonces belonging to one hotkey.
    pub max_per_hotkey: usize,
    /// Maximum number of unexpired pairs across the whole store.
    pub max_entries: usize,
}

impl Default for NonceStoreConfig {
    fn default() -> Self {
        Self {
            retention: Duration::from_secs(60),
            max_per_hotkey: 1_024,
            max_entries: 65_536,
        }
    }
}

/// Invalid capacity configuration for an in-memory nonce store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreConfigError {
    /// The global entry limit must be positive.
    ZeroMaxEntries,
    /// The per-hotkey entry limit must be positive.
    ZeroMaxPerHotkey,
}

impl fmt::Display for StoreConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMaxEntries => f.write_str("max_entries must be greater than zero"),
            Self::ZeroMaxPerHotkey => f.write_str("max_per_hotkey must be greater than zero"),
        }
    }
}

impl std::error::Error for StoreConfigError {}

trait MonotonicClock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

struct SystemClock(Instant);

impl MonotonicClock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

struct Expiry {
    inserted_at: Duration,
    hotkey: AccountId,
    nonce_ns: Arc<BigInt>,
}

#[derive(Default)]
struct Shard {
    seen: HashMap<AccountId, HashSet<Arc<BigInt>>>,
    expiry: VecDeque<Expiry>,
    last_now: Duration,
}

/// A per-process nonce store with sharded sets and FIFO expiry queues.
///
/// Every hotkey belongs to one of 16 independently locked shards. A shared
/// atomic counter enforces the global limit across concurrent insertions.
/// Expiry uses monotonic elapsed time, independent of the request timestamp
/// and wall-clock changes. Entries expire lazily; capacity pressure also
/// sweeps other shards so idle expired entries do not block new requests.
/// Each retained pair has exactly one expiry-queue entry; replay attempts do
/// not extend retention or append entries. Empty hotkey sets are removed.
///
/// At either limit, new pairs return [`StoreError::Full`] without evicting
/// live pairs. Duplicates still return `Ok(false)` at capacity. The future
/// performs a short synchronous memory operation and requires no async
/// runtime. Use shared storage for multiple processes or hosts.
#[doc(alias = "bittensor.http_auth.InMemoryNonceStore")]
pub struct InMemoryNonceStore {
    config: NonceStoreConfig,
    shards: [Mutex<Shard>; SHARD_COUNT],
    count: AtomicUsize,
    hash: RandomState,
    clock: Arc<dyn MonotonicClock>,
}

impl Default for InMemoryNonceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryNonceStore {
    /// Construct a store with [`NonceStoreConfig::default`].
    pub fn new() -> Self {
        Self::from_config(
            NonceStoreConfig::default(),
            Arc::new(SystemClock(Instant::now())),
        )
    }

    /// Construct a store with explicit retention and capacity limits.
    ///
    /// Both capacity limits must be positive. Retention may be zero; a
    /// verifier checks its freshness-window invariant separately.
    pub fn with_config(config: NonceStoreConfig) -> Result<Self, StoreConfigError> {
        Self::validate_config(config)?;
        Ok(Self::from_config(
            config,
            Arc::new(SystemClock(Instant::now())),
        ))
    }

    /// The immutable retention and capacity configuration of this store.
    pub fn config(&self) -> &NonceStoreConfig {
        &self.config
    }

    fn validate_config(config: NonceStoreConfig) -> Result<(), StoreConfigError> {
        if config.max_entries == 0 {
            return Err(StoreConfigError::ZeroMaxEntries);
        }
        if config.max_per_hotkey == 0 {
            return Err(StoreConfigError::ZeroMaxPerHotkey);
        }
        Ok(())
    }

    fn from_config(config: NonceStoreConfig, clock: Arc<dyn MonotonicClock>) -> Self {
        Self {
            config,
            shards: std::array::from_fn(|_| Mutex::new(Shard::default())),
            count: AtomicUsize::new(0),
            hash: RandomState::new(),
            clock,
        }
    }

    fn shard_index(&self, hotkey: &AccountId) -> usize {
        (self.hash.hash_one(hotkey) % SHARD_COUNT as u64) as usize
    }

    fn lock_shard(&self, index: usize) -> Result<MutexGuard<'_, Shard>, StoreError> {
        self.shards[index]
            .lock()
            .map_err(|_| StoreError::Unavailable("nonce store shard lock is poisoned".into()))
    }

    fn expire(&self, shard: &mut Shard) -> Duration {
        // Clamp defensively even for a clock implementation that goes back.
        // Reading under the lock keeps each queue ordered by insertion time.
        let now = self.clock.elapsed().max(shard.last_now);
        shard.last_now = now;
        while shard
            .expiry
            .front()
            .is_some_and(|entry| now.saturating_sub(entry.inserted_at) >= self.config.retention)
        {
            let Some(entry) = shard.expiry.pop_front() else {
                break;
            };
            let empty = if let Some(nonces) = shard.seen.get_mut(&entry.hotkey) {
                if nonces.remove(&entry.nonce_ns) {
                    self.count.fetch_sub(1, Ordering::AcqRel);
                }
                if nonces.capacity() > nonces.len().saturating_mul(4).max(16) {
                    nonces.shrink_to(nonces.len().saturating_mul(2));
                }
                nonces.is_empty()
            } else {
                false
            };
            if empty {
                shard.seen.remove(&entry.hotkey);
            }
        }
        if shard.expiry.is_empty() {
            // Release allocations for shards that no longer hold any pairs.
            shard.expiry = VecDeque::new();
            shard.seen = HashMap::new();
        } else {
            // Keep allocation proportional to live state after large bursts,
            // rather than retaining a high-water mark for every hotkey.
            if shard.expiry.capacity() > shard.expiry.len().saturating_mul(4).max(16) {
                shard.expiry.shrink_to(shard.expiry.len().saturating_mul(2));
            }
            if shard.seen.capacity() > shard.seen.len().saturating_mul(4).max(16) {
                shard.seen.shrink_to(shard.seen.len().saturating_mul(2));
            }
        }
        now
    }

    fn reserve(&self) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        while count < self.config.max_entries {
            match self.count.compare_exchange_weak(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => count = current,
            }
        }
        false
    }

    fn check_and_store_now(
        &self,
        hotkey: &AccountId,
        nonce_ns: &BigInt,
    ) -> Result<bool, StoreError> {
        let index = self.shard_index(hotkey);
        for attempt in 0..2 {
            let mut shard = self.lock_shard(index)?;
            let now = self.expire(&mut shard);
            if let Some(nonces) = shard.seen.get(hotkey) {
                if nonces.contains(nonce_ns) {
                    return Ok(false);
                }
                if nonces.len() >= self.config.max_per_hotkey {
                    return Err(StoreError::Full);
                }
            }
            if self.reserve() {
                let nonce_ns = Arc::new(nonce_ns.clone());
                shard
                    .seen
                    .entry(*hotkey)
                    .or_default()
                    .insert(Arc::clone(&nonce_ns));
                shard.expiry.push_back(Expiry {
                    inserted_at: now,
                    hotkey: *hotkey,
                    nonce_ns,
                });
                return Ok(true);
            }
            drop(shard);
            if attempt == 0 {
                // Never hold two shard locks: concurrent capacity sweeps
                // cannot deadlock, and the target is rechecked afterwards.
                for other in 0..SHARD_COUNT {
                    if other != index {
                        let mut other_shard = self.lock_shard(other)?;
                        self.expire(&mut other_shard);
                    }
                }
            }
        }
        Err(StoreError::Full)
    }
}

impl NonceStore for InMemoryNonceStore {
    fn retention(&self) -> Duration {
        self.config.retention
    }

    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce_ns: &'a BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move { self.check_and_store_now(hotkey, nonce_ns) })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicU64;
    use std::task::{Context, Poll, Waker};

    #[derive(Default)]
    struct TestClock(AtomicU64);

    impl TestClock {
        fn set_ns(&self, elapsed: u64) {
            self.0.store(elapsed, Ordering::Release);
        }
    }

    impl MonotonicClock for TestClock {
        fn elapsed(&self) -> Duration {
            Duration::from_nanos(self.0.load(Ordering::Acquire))
        }
    }

    fn ready<T>(mut future: BoxFuture<'_, T>) -> T {
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("in-memory store unexpectedly yielded"),
        }
    }

    fn hotkey(value: u64) -> AccountId {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&value.to_be_bytes());
        AccountId(bytes)
    }

    fn store(config: NonceStoreConfig) -> (InMemoryNonceStore, Arc<TestClock>) {
        InMemoryNonceStore::validate_config(config).expect("valid test config");
        let clock = Arc::new(TestClock::default());
        (
            InMemoryNonceStore::from_config(config, clock.clone()),
            clock,
        )
    }

    fn counts(store: &InMemoryNonceStore) -> (usize, usize) {
        let mut seen = 0;
        let mut queued = 0;
        for shard in &store.shards {
            let shard = shard.lock().expect("unpoisoned test shard");
            let shard_seen: usize = shard.seen.values().map(HashSet::len).sum();
            assert_eq!(shard_seen, shard.expiry.len());
            assert!(shard.seen.values().all(|nonces| !nonces.is_empty()));
            seen += shard_seen;
            queued += shard.expiry.len();
        }
        assert_eq!(seen, store.count.load(Ordering::Acquire));
        assert!(seen <= store.config.max_entries);
        (seen, queued)
    }

    #[test]
    fn defaults_are_bounded_and_retention_is_python_default() {
        let store = InMemoryNonceStore::new();
        assert_eq!(store.retention(), Duration::from_secs(60));
        assert_eq!(store.config().max_per_hotkey, 1_024);
        assert_eq!(store.config().max_entries, 65_536);
        let _: &dyn NonceStore = &store;
    }

    #[test]
    fn zero_capacities_are_invalid_but_zero_retention_is_valid() {
        assert!(matches!(
            InMemoryNonceStore::with_config(NonceStoreConfig {
                max_entries: 0,
                ..Default::default()
            }),
            Err(StoreConfigError::ZeroMaxEntries)
        ));
        assert!(matches!(
            InMemoryNonceStore::with_config(NonceStoreConfig {
                max_per_hotkey: 0,
                ..Default::default()
            }),
            Err(StoreConfigError::ZeroMaxPerHotkey)
        ));
        assert!(
            InMemoryNonceStore::with_config(NonceStoreConfig {
                retention: Duration::ZERO,
                ..Default::default()
            })
            .is_ok()
        );
    }

    #[test]
    fn replay_is_scoped_to_hotkey_and_nonce() {
        let (store, _) = store(NonceStoreConfig::default());
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(false)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(2), &BigInt::from(42))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(43))),
            Ok(true)
        );
        assert_eq!(counts(&store), (3, 3));
    }

    #[test]
    fn signed_and_oversized_keys_retain_exact_identity_at_bounds_and_expiry() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(10),
            max_per_hotkey: 2,
            max_entries: 2,
        });
        let key = hotkey(1);
        let negative = BigInt::from(-1);
        let oversized = BigInt::from(u64::MAX) + BigInt::from(1);
        assert_eq!(ready(store.check_and_store(&key, &negative)), Ok(true));
        assert_eq!(ready(store.check_and_store(&key, &oversized)), Ok(true));
        assert_eq!(ready(store.check_and_store(&key, &negative)), Ok(false));
        assert_eq!(ready(store.check_and_store(&key, &oversized)), Ok(false));
        assert_eq!(
            ready(store.check_and_store(&key, &BigInt::from(u64::MAX))),
            Err(StoreError::Full)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(2), &negative)),
            Err(StoreError::Full)
        );
        clock.set_ns(9);
        assert_eq!(ready(store.check_and_store(&key, &negative)), Ok(false));
        clock.set_ns(10);
        assert_eq!(ready(store.check_and_store(&key, &negative)), Ok(true));
        assert_eq!(ready(store.check_and_store(&key, &oversized)), Ok(true));
        assert_eq!(store.count.load(Ordering::Acquire), 2);
    }

    #[test]
    fn retention_expires_at_boundary_without_extending_on_replays() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(10),
            ..Default::default()
        });
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(true)
        );
        clock.set_ns(9);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(false)
        );
        assert_eq!(counts(&store), (1, 1));
        clock.set_ns(10);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(true)
        );
        assert_eq!(counts(&store), (1, 1));
    }

    #[test]
    fn expiry_uses_monotonic_insertion_time_not_nonce_timestamp() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(10),
            ..Default::default()
        });
        clock.set_ns(100);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(0))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(u64::MAX))),
            Ok(true)
        );
        clock.set_ns(109);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(0))),
            Ok(false)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(u64::MAX))),
            Ok(false)
        );
        clock.set_ns(1);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(0))),
            Ok(false)
        );
        clock.set_ns(110);
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(0))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(u64::MAX))),
            Ok(true)
        );
    }

    #[test]
    fn zero_retention_can_accept_immediate_repeat_without_queue_growth() {
        let (store, _) = store(NonceStoreConfig {
            retention: Duration::ZERO,
            max_entries: 1,
            max_per_hotkey: 1,
        });
        for _ in 0..100 {
            assert_eq!(
                ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
                Ok(true)
            );
            assert_eq!(counts(&store), (1, 1));
        }
    }

    #[test]
    fn per_hotkey_limit_rejects_new_pairs_without_reopening_replays() {
        let (store, _) = store(NonceStoreConfig {
            max_per_hotkey: 1,
            max_entries: 4,
            ..Default::default()
        });
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(43))),
            Err(StoreError::Full)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Ok(false)
        );
        assert_eq!(
            ready(store.check_and_store(&hotkey(2), &BigInt::from(43))),
            Ok(true)
        );
        assert_eq!(counts(&store), (2, 2));
    }

    #[test]
    fn global_limit_preserves_all_live_pairs_and_reclaims_other_shards() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(10),
            max_entries: 1,
            max_per_hotkey: 2,
        });
        let first = hotkey(1);
        let other = (2..1_000)
            .map(hotkey)
            .find(|key| store.shard_index(key) != store.shard_index(&first))
            .expect("hotkeys occupy more than one shard");
        assert_eq!(
            ready(store.check_and_store(&first, &BigInt::from(42))),
            Ok(true)
        );
        assert_eq!(
            ready(store.check_and_store(&other, &BigInt::from(43))),
            Err(StoreError::Full)
        );
        assert_eq!(
            ready(store.check_and_store(&first, &BigInt::from(42))),
            Ok(false)
        );
        clock.set_ns(10);
        assert_eq!(
            ready(store.check_and_store(&other, &BigInt::from(43))),
            Ok(true)
        );
        assert_eq!(counts(&store), (1, 1));
    }

    #[test]
    fn concurrent_replay_has_exactly_one_winner() {
        let (store, _) = store(NonceStoreConfig::default());
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(33));
        let handles: Vec<_> = (0..32)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ready(store.check_and_store(&hotkey(1), &BigInt::from(42)))
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker completed"))
            .collect();
        assert_eq!(
            results.iter().filter(|result| **result == Ok(true)).count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Ok(false))
                .count(),
            31
        );
        assert_eq!(counts(&store), (1, 1));
    }

    fn concurrent_capacity(max_per_hotkey: usize, max_entries: usize, one_hotkey: bool) {
        let (store, _) = store(NonceStoreConfig {
            max_per_hotkey,
            max_entries,
            ..Default::default()
        });
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(33));
        let handles: Vec<_> = (0..32)
            .map(|nonce| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let key = hotkey(if one_hotkey { 0 } else { nonce });
                    barrier.wait();
                    (
                        key,
                        nonce,
                        ready(store.check_and_store(&key, &BigInt::from(nonce))),
                    )
                })
            })
            .collect();
        barrier.wait();
        let mut accepted = 0;
        for handle in handles {
            let (key, nonce, result) = handle.join().expect("worker completed");
            match result {
                Ok(true) => {
                    accepted += 1;
                    assert_eq!(
                        ready(store.check_and_store(&key, &BigInt::from(nonce))),
                        Ok(false)
                    );
                }
                Err(StoreError::Full) => {}
                result => panic!("unexpected capacity result: {result:?}"),
            }
        }
        let bound = if one_hotkey {
            max_per_hotkey.min(max_entries)
        } else {
            max_entries
        };
        assert_eq!(accepted, bound);
        assert_eq!(counts(&store), (bound, bound));
    }

    #[test]
    fn concurrent_inserts_cannot_exceed_global_limit() {
        concurrent_capacity(32, 5, false);
    }

    #[test]
    fn concurrent_inserts_cannot_exceed_per_hotkey_limit() {
        concurrent_capacity(3, 32, true);
    }

    #[test]
    fn repeated_expiry_removes_hotkey_sets_and_bounds_queue_storage() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(1),
            max_per_hotkey: 2,
            max_entries: 4,
        });
        for elapsed in 0..2_000 {
            clock.set_ns(elapsed);
            assert_eq!(
                ready(store.check_and_store(&hotkey(elapsed), &BigInt::from(elapsed))),
                Ok(true)
            );
            let (seen, queued) = counts(&store);
            assert!(seen <= 4);
            assert_eq!(queued, seen);
        }
        clock.set_ns(2_000);
        for index in 0..SHARD_COUNT {
            let mut shard = store.lock_shard(index).expect("unpoisoned shard");
            store.expire(&mut shard);
        }
        assert_eq!(counts(&store), (0, 0));
        for shard in &store.shards {
            let shard = shard.lock().expect("unpoisoned shard");
            assert_eq!(shard.seen.capacity(), 0);
            assert_eq!(shard.expiry.capacity(), 0);
        }
    }

    #[test]
    fn expiry_releases_burst_allocations_with_live_entries_remaining() {
        let (store, clock) = store(NonceStoreConfig {
            retention: Duration::from_nanos(10),
            max_per_hotkey: 512,
            max_entries: 512,
        });
        let key = hotkey(1);
        for nonce in 0..256 {
            assert_eq!(
                ready(store.check_and_store(&key, &BigInt::from(nonce))),
                Ok(true)
            );
        }
        clock.set_ns(1);
        assert_eq!(
            ready(store.check_and_store(&key, &BigInt::from(256))),
            Ok(true)
        );
        clock.set_ns(10);
        assert_eq!(
            ready(store.check_and_store(&key, &BigInt::from(256))),
            Ok(false)
        );
        assert_eq!(counts(&store), (1, 1));
        let shard = store
            .lock_shard(store.shard_index(&key))
            .expect("unpoisoned shard");
        assert!(shard.expiry.capacity() <= 16);
        assert!(shard.seen[&key].capacity() <= 16);
    }

    #[test]
    fn poisoned_shard_fails_closed() {
        let (store, _) = store(NonceStoreConfig::default());
        let store = Arc::new(store);
        let worker = store.clone();
        let index = store.shard_index(&hotkey(1));
        let poisoned = std::thread::spawn(move || {
            let _lock = worker.shards[index].lock().expect("unpoisoned shard");
            panic!("simulate interrupted store mutation");
        });
        assert!(poisoned.join().is_err());
        assert!(matches!(
            ready(store.check_and_store(&hotkey(1), &BigInt::from(42))),
            Err(StoreError::Unavailable(_))
        ));
    }
}
