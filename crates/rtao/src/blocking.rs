// Derived from RaoFoundation/subtensor sdk/python/bittensor/sync.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_subtensor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Blocking access to the single asynchronous SDK implementation.
//!
//! Construction is cold. The first chain operation creates a private runtime
//! thread and connects there. Arguments crossing that thread are owned; the
//! caller's Tokio runtime is never entered or blocked on by this facade.
//! Closing a started clone closes the shared connection and its worker. Dropping the
//! last facade, namespace owner, snapshot or iterator also stops the worker.

use std::{
    collections::BTreeMap,
    future::{Future, IntoFuture},
    ops::Deref,
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use rtao_client::{
    BlockHeader, BlockRef, Client, ClientConfig, ClientError, Finality, PinnedBlock, RuntimeHandle,
    Snapshot, View, ViewExt,
    blocking::{BlockingError, BlockingRuntime},
};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, CryptoType, DynBalance, MechId, NetUid};
use rtao_signer::{DynKeySource, KeySource, Role, SignerRole};
use rtao_tx::{
    ExecuteOptions, ExtrinsicResult, Intent, IntentPreflight, Plan, PlanOptions, Policy,
    PrepareOptions, ShieldOptions, TxError,
};

#[path = "generated/blocking_namespaces.rs"]
mod namespaces;
pub use namespaces::*;

/// A worker, connection, read or transaction failure from a blocking call.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The private runtime could not run the operation.
    Runtime(BlockingError),
    /// Transport, metadata or SCALE decoding failed.
    Client(ClientError),
    /// A curated or catalog read failed.
    Read(rtao_reads::ReadError),
    /// Planning, signing or submission failed.
    Transaction(TxError),
    /// An included extrinsic failed its chain dispatch.
    Chain(rtao_tx::ChainError),
    /// A named or default wallet could not be resolved.
    Wallet(rtao_wallet::WalletError),
    /// The Subtensor instance has already selected the other execution mode.
    Mode(&'static str),
    /// Internal shared state was poisoned by an earlier panic.
    Poisoned,
    /// A block or time wait exceeded its caller-supplied limit.
    Timeout,
    /// A block stream ended before the requested condition became true.
    StreamEnded,
    /// A block subscription failed or reported a chain reorganization.
    Head(rtao_client::HeadError),
    /// Public registration mining failed.
    #[cfg(feature = "pow")]
    Pow(rtao_intents::pow::PowError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
            Self::Read(error) => error.fmt(f),
            Self::Transaction(error) => error.fmt(f),
            Self::Chain(error) => error.fmt(f),
            Self::Wallet(error) => error.fmt(f),
            Self::Head(error) => error.fmt(f),
            Self::Mode(message) => f.write_str(message),
            Self::Poisoned => f.write_str("blocking client shared state is poisoned"),
            Self::Timeout => f.write_str("blocking client wait timed out"),
            Self::StreamEnded => f.write_str("block stream ended before the wait completed"),
            #[cfg(feature = "pow")]
            Self::Pow(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::Client(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Transaction(error) => Some(error),
            Self::Chain(error) => Some(error),
            Self::Wallet(error) => Some(error),
            Self::Head(error) => Some(error),
            #[cfg(feature = "pow")]
            Self::Pow(error) => Some(error),
            _ => None,
        }
    }
}

macro_rules! error_from {
    ($($source:ty => $variant:ident),+ $(,)?) => {$ (
        impl From<$source> for Error {
            fn from(error: $source) -> Self { Self::$variant(error) }
        }
    )+};
}
error_from!(
    BlockingError => Runtime,
    ClientError => Client,
    rtao_reads::ReadError => Read,
    TxError => Transaction,
    rtao_tx::ChainError => Chain,
    rtao_wallet::WalletError => Wallet,
    rtao_client::HeadError => Head,
);

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Client, ClientError>> + Send>>;
type Connector = dyn Fn(ClientConfig) -> ConnectFuture + Send + Sync;

const CLIENT_CLOSE_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone)]
enum CloseFailure {
    Runtime(BlockingError),
    Timeout,
}

impl CloseFailure {
    fn into_error(self) -> Error {
        match self {
            Self::Runtime(error) => Error::Runtime(error),
            Self::Timeout => Error::Timeout,
        }
    }
}

struct Shared {
    config: ClientConfig,
    connector: Arc<Connector>,
    runtime: BlockingRuntime,
    client: tokio::sync::Mutex<Option<Client>>,
    token_symbols: Mutex<BTreeMap<NetUid, String>>,
    policy: Mutex<Option<Policy>>,
    // Held through cleanup and worker shutdown. Repeated callers receive the
    // same recorded result, rather than observing an early closed flag.
    close_state: Mutex<Option<Result<(), CloseFailure>>>,
    closed: AtomicBool,
    // 0: cold, 1: blocking, 2: async. Plain SyncClient always uses 0/1.
    mode: AtomicU8,
}

impl Shared {
    fn choose_blocking(&self) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BlockingError::Closed.into());
        }
        self.mode
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .or_else(|mode| if mode == 1 { Ok(mode) } else { Err(mode) })
            .map(|_| ())
            .map_err(|_| {
                Error::Mode(
                    "this Subtensor selected async mode; use the returned async Client or construct a fresh blocking client",
                )
            })
    }

    async fn connected(&self) -> Result<Client, Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(BlockingError::Closed.into());
        }
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        let client = (self.connector)(self.config.clone()).await?;
        let symbols = client.token_symbols().await;
        if self.closed.load(Ordering::Acquire) {
            client.close().await;
            return Err(BlockingError::Closed.into());
        }
        *self.token_symbols.lock().map_err(|_| Error::Poisoned)? = symbols;
        *slot = Some(client.clone());
        Ok(client)
    }

    fn policy(&self) -> Result<Option<Policy>, Error> {
        self.policy
            .lock()
            .map(|policy| policy.clone())
            .map_err(|_| Error::Poisoned)
    }

    async fn executor(&self) -> Result<crate::Executor, Error> {
        let executor = crate::Executor::new(self.connected().await?);
        Ok(match self.policy()? {
            Some(policy) => executor.with_policy(policy),
            None => executor,
        })
    }
}

fn close_started(shared: &Arc<Shared>, cleanup_wait: Duration) -> Result<(), CloseFailure> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let runtime = shared.runtime.clone();
    let cleanup_shared = shared.clone();
    let coordinator = std::thread::Builder::new()
        .name("rtao-blocking-close".into())
        .spawn(move || {
            let cleanup = runtime.run(async move {
                if let Some(client) = cleanup_shared.client.lock().await.take() {
                    client.close().await;
                }
            });
            let _ = sender.send(cleanup);
        });
    match coordinator {
        Ok(coordinator) => {
            // The deadline belongs to the caller, not the reactor being
            // stopped: it still expires if a future monopolizes a worker poll.
            let (cleanup, completed) = match receiver.recv_timeout(cleanup_wait) {
                Ok(result) => (result.map_err(CloseFailure::Runtime), true),
                Err(mpsc::RecvTimeoutError::Timeout) => (Err(CloseFailure::Timeout), false),
                Err(mpsc::RecvTimeoutError::Disconnected) => (
                    Err(CloseFailure::Runtime(BlockingError::WorkerPanicked)),
                    true,
                ),
            };
            // close() cancels run()'s result waiter before waiting for the
            // runtime thread. The coordinator can finish even if that runtime
            // thread cannot yet observe cancellation. A poisoned runtime lock
            // can prevent cancellation; do not wait indefinitely in that case.
            let shutdown = shared.runtime.close().map_err(CloseFailure::Runtime);
            let completed = completed
                || !matches!(
                    receiver.recv_timeout(Duration::from_secs(1)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                );
            let acknowledged = if completed {
                Ok(())
            } else {
                Err(CloseFailure::Runtime(BlockingError::ShutdownTimeout))
            };
            // An acknowledgement completes the cleanup operation, but native
            // thread teardown may still be pending. Detach rather than add
            // an unbounded join after the caller's bounded waits.
            drop(coordinator);
            shutdown.and(acknowledged).and(cleanup)
        }
        Err(error) => {
            // Failure to create the cleanup coordinator must still stop the
            // existing worker and release every outstanding synchronous call.
            shared.runtime.close().map_err(CloseFailure::Runtime)?;
            Err(CloseFailure::Runtime(BlockingError::ThreadStart(format!(
                "cleanup coordinator: {error}"
            ))))
        }
    }
}

/// Owned async view moved into the private worker for generated wrappers.
#[derive(Clone)]
pub(crate) enum AsyncView {
    Client(Client),
    Snapshot(Snapshot),
}

impl AsyncView {
    fn client(&self) -> &Client {
        match self {
            Self::Client(client) => client,
            Self::Snapshot(snapshot) => snapshot.client(),
        }
    }
}

impl View for AsyncView {
    fn read_observer(&self) -> Option<&dyn rtao_client::ReadObserver> {
        match self {
            Self::Client(client) => client.read_observer(),
            Self::Snapshot(snapshot) => snapshot.read_observer(),
        }
    }
    fn metagraph_view_enabled(&self) -> bool {
        match self {
            Self::Client(client) => client.metagraph_view_enabled(),
            Self::Snapshot(snapshot) => snapshot.metagraph_view_enabled(),
        }
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        match self {
            Self::Client(client) => client.pinned(),
            Self::Snapshot(snapshot) => snapshot.pinned(),
        }
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        match self {
            Self::Client(client) => client.pin().await,
            Self::Snapshot(snapshot) => snapshot.pin().await,
        }
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        match self {
            Self::Client(client) => View::runtime(client).await,
            Self::Snapshot(snapshot) => View::runtime(snapshot).await,
        }
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        match self {
            Self::Client(client) => client.storage_raw(key).await,
            Self::Snapshot(snapshot) => snapshot.storage_raw(key).await,
        }
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        match self {
            Self::Client(client) => client.storage_raw_many(keys).await,
            Self::Snapshot(snapshot) => snapshot.storage_raw_many(keys).await,
        }
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        match self {
            Self::Client(client) => client.keys_paged(prefix, count, start).await,
            Self::Snapshot(snapshot) => snapshot.keys_paged(prefix, count, start).await,
        }
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        match self {
            Self::Client(client) => client.state_call(method, args).await,
            Self::Snapshot(snapshot) => snapshot.state_call(method, args).await,
        }
    }
}

/// Shared dispatch context used by handwritten and generated sync namespaces.
#[derive(Clone)]
pub(crate) struct SyncView {
    shared: Arc<Shared>,
    snapshot: Option<Snapshot>,
}

impl SyncView {
    pub(crate) fn with_view<F, Fut, T, E>(&self, operation: F) -> Result<T, Error>
    where
        F: FnOnce(AsyncView) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        T: Send + 'static,
        E: Into<Error> + Send + 'static,
    {
        self.shared.choose_blocking()?;
        let shared = self.shared.clone();
        let snapshot = self.snapshot.clone();
        self.shared.runtime.run(async move {
            let view = match snapshot {
                Some(snapshot) => AsyncView::Snapshot(snapshot),
                None => AsyncView::Client(shared.connected().await?),
            };
            operation(view).await.map_err(Into::into)
        })?
    }

    fn with_executor<F, Fut, T>(&self, operation: F) -> Result<T, Error>
    where
        F: FnOnce(crate::Executor) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, TxError>> + Send + 'static,
        T: Send + 'static,
    {
        self.shared.choose_blocking()?;
        let shared = self.shared.clone();
        self.shared.runtime.run(async move {
            operation(shared.executor().await?)
                .await
                .map_err(Error::from)
        })?
    }
}

/// Blocking client with all fourteen read namespaces and the async executor.
///
/// Every clone shares one lazy connection and private runtime. Use owned
/// intents and `Arc` key sources: an operation must remain valid while its
/// future executes on the private thread.
#[derive(Clone)]
pub struct SyncClient {
    pub(crate) view: SyncView,
}

impl SyncClient {
    /// Construct without starting a thread, touching disk or opening a socket.
    pub fn new(config: ClientConfig) -> Self {
        Self::with_connector(config, Client::connect)
    }

    /// Resolve a network name or URL without connecting.
    pub fn for_network(network: &str) -> Result<Self, Error> {
        Ok(Self::new(ClientConfig::for_network(network)?))
    }

    /// Inject async connection construction, performed on the private worker.
    ///
    /// This permits fake RPC transports without constructing a second SDK or
    /// borrowing a runtime from the calling thread.
    pub fn with_connector<F, Fut>(config: ClientConfig, connector: F) -> Self
    where
        F: Fn(ClientConfig) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Client, ClientError>> + Send + 'static,
    {
        Self {
            view: SyncView {
                shared: Arc::new(Shared {
                    config,
                    connector: Arc::new(move |config| Box::pin(connector(config))),
                    runtime: BlockingRuntime::new(),
                    client: tokio::sync::Mutex::new(None),
                    token_symbols: Mutex::new(BTreeMap::new()),
                    policy: Mutex::new(None),
                    close_state: Mutex::new(None),
                    closed: AtomicBool::new(false),
                    mode: AtomicU8::new(0),
                }),
                snapshot: None,
            },
        }
    }

    /// Network identity selected at construction; this performs no I/O.
    pub fn network(&self) -> &rtao_client::Network {
        &self.view.shared.config.network
    }

    /// Configured primary endpoint; this performs no I/O.
    pub fn endpoint(&self) -> &str {
        self.network().endpoint()
    }

    /// Whether the worker has been started by a blocking operation.
    pub fn is_started(&self) -> bool {
        self.view.shared.runtime.is_started()
    }

    /// Whether explicit close has permanently closed this shared facade.
    pub fn is_closed(&self) -> bool {
        self.view.shared.closed.load(Ordering::Acquire)
    }

    /// Connect once. Concurrent callers share the same connection attempt.
    pub fn connect(&self) -> Result<(), Error> {
        self.view.with_view(|_| async { Ok::<_, Error>(()) })
    }

    /// Close a started connection and worker, even when client cleanup fails.
    ///
    /// This is idempotent and invalidates every started clone and snapshot.
    /// Python's cold close is a no-op; it neither connects nor prevents a later
    /// first use, including selecting async mode on a cold Subtensor.
    /// Graceful client cleanup has a five-second caller-side bound, followed
    /// by the runtime's bounded shutdown. Concurrent closes share its result.
    pub fn close(&self) -> Result<(), Error> {
        self.close_with_timeout(CLIENT_CLOSE_WAIT)
    }

    fn close_with_timeout(&self, cleanup_wait: Duration) -> Result<(), Error> {
        let shared = &self.view.shared;
        // A worker must never wait for a caller that is trying to stop that
        // same worker. Reject before taking the facade lock or changing flags.
        if shared.runtime.owns_current_thread() {
            return Err(BlockingError::OwnThread.into());
        }
        let mut close_state = shared.close_state.lock().map_err(|_| Error::Poisoned)?;
        if let Some(result) = &*close_state {
            return result.clone().map_err(CloseFailure::into_error);
        }
        if shared.mode.load(Ordering::Acquire) == 2 {
            return Err(Error::Mode(
                "this Subtensor selected async mode; close the returned async Client",
            ));
        }
        if !shared.runtime.is_started() && shared.mode.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        shared.closed.store(true, Ordering::Release);
        let result = if shared.runtime.is_started() {
            close_started(shared, cleanup_wait)
        } else {
            shared.runtime.close().map_err(CloseFailure::Runtime)
        };
        *close_state = Some(result.clone());
        result.map_err(CloseFailure::into_error)
    }

    /// Copy the default semantic policy without connecting.
    pub fn policy(&self) -> Result<Option<Policy>, Error> {
        self.view.shared.policy()
    }

    /// Replace the policy applied to subsequent executor operations.
    pub fn set_policy(&self, policy: Option<Policy>) -> Result<(), Error> {
        *self
            .view
            .shared
            .policy
            .lock()
            .map_err(|_| Error::Poisoned)? = policy;
        Ok(())
    }

    /// Resolve a head, block number or hash into a read-only blocking snapshot.
    pub fn at(&self, block: BlockRef) -> Result<SyncSnapshot, Error> {
        let snapshot = self
            .view
            .with_view(move |view| async move { view.client().at(block).await })?;
        Ok(SyncSnapshot {
            block: snapshot.block().clone(),
            view: SyncView {
                shared: self.view.shared.clone(),
                snapshot: Some(snapshot),
            },
        })
    }

    /// Read the current best chain height afresh.
    pub fn block(&self) -> Result<u64, Error> {
        Ok(self.at(BlockRef::Best)?.block().number)
    }

    /// Read the current finalized chain height afresh.
    pub fn finalized_block(&self) -> Result<u64, Error> {
        Ok(self.at(BlockRef::Finalized)?.block().number)
    }

    /// Read the runtime version through the client's upgrade-aware cache.
    pub fn spec_version(&self) -> Result<u32, Error> {
        self.view.with_view(|view| async move {
            Ok::<_, ClientError>(view.runtime().await?.spec_version())
        })
    }

    /// Read the selected block's timestamp as a UTC system time.
    pub fn timestamp(&self, block: Option<BlockRef>) -> Result<SystemTime, Error> {
        match block {
            Some(block) => self.at(block)?.timestamp(),
            None => self
                .view
                .with_view(|view| async move { view.timestamp().await }),
        }
    }

    /// UTC time of the current head, freshly read on every call.
    pub fn time(&self) -> Result<SystemTime, Error> {
        self.timestamp(None)
    }

    /// Decode a complete live SCALE type, refusing trailing input.
    pub fn decode_scale(&self, type_name: String, bytes: Vec<u8>) -> Result<DynValue, Error> {
        self.view.with_view(move |view| async move {
            view.client().decode_scale(&type_name, &bytes).await
        })
    }

    /// Follow new headers, blocking only while the iterator requests an item.
    pub fn blocks(&self, finality: Finality) -> Result<Blocks, Error> {
        let stream = self
            .view
            .with_view(move |view| async move { view.client().blocks(finality) })?;
        Ok(Blocks {
            shared: self.view.shared.clone(),
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
            ended: false,
        })
    }

    /// Wait for a target height, or the next block when the target is omitted.
    pub fn wait_for_block(
        &self,
        block: Option<u64>,
        timeout: Option<Duration>,
    ) -> Result<BlockHeader, Error> {
        self.view.with_view(move |view| async move {
            bounded_wait(timeout, async move {
                let client = view.client();
                let target = match block {
                    Some(block) => block,
                    None => client
                        .at(BlockRef::Best)
                        .await?
                        .block()
                        .number
                        .checked_add(1)
                        .ok_or_else(|| {
                            ClientError::Protocol("next block number exceeds u64".into())
                        })?,
                };
                let mut blocks = client.blocks(Finality::Best)?;
                while let Some(header) = blocks.next().await {
                    let header = header?;
                    if header.number >= target {
                        return Ok(header);
                    }
                }
                Err(Error::StreamEnded)
            })
            .await
        })
    }

    /// Wait until the chain's timestamp reaches a UTC system time.
    ///
    /// Large gaps sleep on the private worker before opening a subscription;
    /// completion always uses chain time, including each received block's time.
    pub fn wait_for_timestamp(
        &self,
        when: SystemTime,
        timeout: Option<Duration>,
    ) -> Result<BlockHeader, Error> {
        self.view.with_view(move |view| async move {
            bounded_wait(timeout, async move {
                let client = view.client();
                let slot = client.block_time().await?;
                let now = client.timestamp().await?;
                if let Ok(gap) = when.duration_since(now)
                    && gap > slot.saturating_mul(4)
                {
                    tokio::time::sleep(gap.saturating_sub(slot.saturating_mul(2))).await;
                }
                let mut blocks = client.blocks(Finality::Best)?;
                while let Some(header) = blocks.next().await {
                    let header = header?;
                    let snapshot = client.at(BlockRef::Hash(header.hash)).await?;
                    if snapshot.timestamp().await? >= when {
                        return Ok(header);
                    }
                }
                Err(Error::StreamEnded)
            })
            .await
        })
    }

    /// Wait for an observed epoch run, following LastEpochBlock rather than a prediction.
    pub fn wait_for_epoch(
        &self,
        netuid: NetUid,
        timeout: Option<Duration>,
    ) -> Result<EpochEvent, Error> {
        self.view.with_view(move |view| async move {
            bounded_wait(timeout, async move {
                use rtao_runtime::storage::subtensor_module as st;
                let client = view.client();
                let (tempo, baseline) = client
                    .fetch((st::TEMPO.at((netuid,)), st::LAST_EPOCH_BLOCK.at((netuid,))))
                    .await?;
                if tempo == 0 {
                    return Err(rtao_reads::ReadError::InvalidParam {
                        name: "netuid".into(),
                        reason: format!("subnet {netuid} has tempo 0 and never runs epochs"),
                    }
                    .into());
                }
                let mut blocks = client.blocks(Finality::Best)?;
                while let Some(header) = blocks.next().await {
                    let header = header?;
                    let snapshot = client.at(BlockRef::Hash(header.hash)).await?;
                    let value = snapshot.storage(&st::LAST_EPOCH_BLOCK, (netuid,)).await?;
                    if value > baseline {
                        return Ok(EpochEvent {
                            netuid,
                            block: value,
                            previous: baseline,
                        });
                    }
                }
                Err(Error::StreamEnded)
            })
            .await
        })
    }

    /// Plan a typed intent without acquiring its private signer.
    pub fn plan<I, K>(&self, intent: I, keys: Arc<K>, options: PlanOptions) -> Result<Plan, Error>
    where
        I: Intent + 'static,
        K: KeySource<I::Role> + 'static,
    {
        self.view.with_executor(move |executor| async move {
            executor.plan(&intent, keys.as_ref(), &options).await
        })
    }

    /// Plan an operation with a runtime-selected signing role.
    pub fn plan_dyn(
        &self,
        intent: Arc<dyn rtao_tx::DynIntent>,
        keys: Arc<dyn DynKeySource>,
        options: PlanOptions,
    ) -> Result<Plan, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .plan_dyn(intent.as_ref(), keys.as_ref(), &options)
                .await
        })
    }

    /// Run typed structural and fee preflight without unlocking a signer.
    pub fn preflight<I, K>(
        &self,
        intent: I,
        keys: Arc<K>,
        options: PlanOptions,
    ) -> Result<IntentPreflight, Error>
    where
        I: Intent + 'static,
        K: KeySource<I::Role> + 'static,
    {
        self.view.with_executor(move |executor| async move {
            executor.preflight(&intent, keys.as_ref(), &options).await
        })
    }

    /// Preview an operation with a runtime-selected signing role.
    pub fn preflight_dyn(
        &self,
        intent: Arc<dyn rtao_tx::DynIntent>,
        keys: Arc<dyn DynKeySource>,
        options: PlanOptions,
    ) -> Result<IntentPreflight, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .preflight_dyn(intent.as_ref(), keys.as_ref(), &options)
                .await
        })
    }

    /// Execute a typed intent using the async executor and shared nonce state.
    pub fn execute<I, K>(
        &self,
        intent: I,
        keys: Arc<K>,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult<I::Output>, Error>
    where
        I: Intent + 'static,
        K: KeySource<I::Role> + 'static,
        I::Output: Send + 'static,
    {
        self.view.with_executor(move |executor| async move {
            executor.execute(&intent, keys.as_ref(), &options).await
        })
    }

    /// Execute an operation using dynamic runtime role checks.
    pub fn execute_dyn(
        &self,
        intent: Arc<dyn rtao_tx::DynIntent>,
        keys: Arc<dyn DynKeySource>,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .execute_dyn(intent.as_ref(), keys.as_ref(), &options)
                .await
        })
    }

    /// Parse and execute one JSON tool invocation through the generated registry.
    pub fn execute_tool(
        &self,
        op: String,
        args: serde_json::Value,
        keys: Arc<dyn DynKeySource>,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .execute_tool(&op, args, keys.as_ref(), &options)
                .await
        })
    }

    /// Submit a typed intent in the same authenticated MEV shield as async use.
    pub fn submit_shielded<I, K>(
        &self,
        intent: I,
        keys: Arc<K>,
        options: ShieldOptions,
    ) -> Result<ExtrinsicResult<I::Output>, Error>
    where
        I: Intent + 'static,
        K: KeySource<I::Role> + 'static,
        I::Output: Send + 'static,
    {
        self.view.with_executor(move |executor| async move {
            executor
                .submit_shielded(&intent, keys.as_ref(), &options)
                .await
        })
    }

    /// Submit a dynamic intent in the MEV shield with runtime role checks.
    pub fn submit_shielded_dyn(
        &self,
        intent: Arc<dyn rtao_tx::DynIntent>,
        keys: Arc<dyn DynKeySource>,
        options: ShieldOptions,
    ) -> Result<ExtrinsicResult, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .submit_shielded_dyn(intent.as_ref(), keys.as_ref(), &options)
                .await
        })
    }

    /// Submit runtime-stamped raw bytes under a typed key role and raw-call policy.
    pub fn submit_call<R, K>(
        &self,
        call: rtao_client::CallBytes,
        keys: Arc<K>,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult, Error>
    where
        R: Role,
        K: KeySource<R> + 'static,
    {
        self.view.with_executor(move |executor| async move {
            executor
                .submit_call::<R, K>(call, keys.as_ref(), &options)
                .await
        })
    }

    /// Submit raw bytes using explicit runtime role selection.
    pub fn submit_call_dyn(
        &self,
        call: rtao_client::CallBytes,
        keys: Arc<dyn DynKeySource>,
        role: SignerRole,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .submit_call_dyn(call, keys.as_ref(), role, &options)
                .await
        })
    }

    /// Prepare a public signing payload without resolving private keys.
    pub fn prepare_call(
        &self,
        call: rtao_client::CallBytes,
        address: AccountId,
        options: PrepareOptions,
    ) -> Result<rtao_client::UnsignedExtrinsic, Error> {
        self.view.with_executor(move |executor| async move {
            executor.prepare_call(call, address, &options).await
        })
    }

    /// Verify and submit an external signature under the executor's raw policy.
    pub fn submit_signature(
        &self,
        unsigned: rtao_client::UnsignedExtrinsic,
        signature: rtao_signer::MultiSignature,
        options: ExecuteOptions,
    ) -> Result<ExtrinsicResult, Error> {
        self.view.with_executor(move |executor| async move {
            executor
                .submit_signature(&unsigned, signature, &options)
                .await
        })
    }

    /// Compose typed metadata arguments using the connected runtime.
    pub fn compose(&self, call: rtao_runtime::Call) -> Result<rtao_client::CallBytes, Error> {
        self.view.with_view(move |view| async move {
            let runtime = view.runtime().await?;
            call.compose(&runtime).map_err(ClientError::from)
        })
    }

    /// Estimate the weight and fee without resolving a private signer.
    pub fn estimate_weight(
        &self,
        call: rtao_client::CallBytes,
        address: AccountId,
        crypto_type: CryptoType,
    ) -> Result<rtao_core::ops::fee::FeeEstimate, Error> {
        self.view.with_view(move |view| async move {
            view.client()
                .estimate_weight(call, address, crypto_type)
                .await
        })
    }

    /// Estimate the outer MEV shield carrier's fee.
    pub fn estimate_shielded_carrier_fee(
        &self,
        fee_payer: AccountId,
    ) -> Result<rtao_primitives::Balance<rtao_primitives::Tao>, Error> {
        self.view.with_view(move |view| async move {
            view.client().estimate_shielded_carrier_fee(fee_payer).await
        })
    }

    /// Validate and derive a multisig account against the current runtime.
    pub fn multisig(
        &self,
        signatories: Vec<AccountId>,
        threshold: u16,
    ) -> Result<SyncMultisig, Error> {
        let multisig = self.view.with_executor(move |executor| async move {
            executor.multisig(&signatories, threshold).await
        })?;
        Ok(SyncMultisig {
            client: self.clone(),
            multisig,
        })
    }

    /// Enabled JSON tools. Pure catalog inspection performs no connection.
    pub fn tools(&self) -> Result<&'static [rtao_tx::ToolSpec], Error> {
        static TOOLS: OnceLock<Result<Vec<rtao_tx::ToolSpec>, String>> = OnceLock::new();
        match TOOLS.get_or_init(|| {
            rtao_catalog::INTENTS
                .iter()
                .map(|spec| {
                    let mut tool: rtao_tx::ToolSpec = serde_json::from_str(spec.catalog_json)
                        .map_err(|error| error.to_string())?;
                    tool.docs_url = format!(
                        "{}/tx/{}",
                        rtao_catalog::py::DOCS_URL,
                        spec.name.replace('_', "-")
                    );
                    Ok(tool)
                })
                .collect()
        }) {
            Ok(tools) => Ok(tools),
            Err(error) => Err(TxError::from(rtao_tx::IntentError::InvalidArgument(format!(
                "authenticated tool catalog is invalid: {error}"
            )))
            .into()),
        }
    }

    /// Mine a public registration challenge using the existing async miner.
    #[cfg(feature = "pow")]
    pub fn mine_pow_registration(
        &self,
        netuid: NetUid,
        hotkey: AccountId,
        coldkey: AccountId,
        config: rtao_intents::pow::PowConfig,
        cancellation: rtao_intents::pow::CancellationToken,
    ) -> Result<rtao_intents::pow::PowProof, Error> {
        self.view.with_view(move |view| async move {
            let solver = rtao_intents::pow::PowSolver::new(config).map_err(Error::Pow)?;
            solver
                .solve(view.client(), netuid, hotkey, coldkey, &cancellation)
                .await
                .map_err(Error::Pow)
        })
    }
}

impl Default for SyncClient {
    fn default() -> Self {
        Self::new(ClientConfig::default())
    }
}

impl std::fmt::Debug for SyncClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncClient")
            .field("network", self.network())
            .field("started", &self.is_started())
            .field("closed", &self.is_closed())
            .finish()
    }
}

/// Read-only blocking facade whose operations retain one immutable block hash.
#[derive(Clone)]
pub struct SyncSnapshot {
    pub(crate) view: SyncView,
    block: PinnedBlock,
}

impl SyncSnapshot {
    /// The immutable block identity and its parent-selected runtime.
    pub fn block(&self) -> &PinnedBlock {
        &self.block
    }

    /// Clone this snapshot, or resolve a sibling from the same connection.
    pub fn at(&self, block: Option<BlockRef>) -> Result<Self, Error> {
        match block {
            None => Ok(self.clone()),
            Some(BlockRef::Hash(hash)) if hash == self.block().hash => Ok(self.clone()),
            Some(BlockRef::Number(number)) if number == self.block().number => Ok(self.clone()),
            Some(block) => SyncClient {
                view: SyncView {
                    shared: self.view.shared.clone(),
                    snapshot: None,
                },
            }
            .at(block),
        }
    }

    /// Read this snapshot's UTC block timestamp.
    pub fn timestamp(&self) -> Result<SystemTime, Error> {
        self.view
            .with_view(|view| async move { view.timestamp().await })
    }
}

impl std::fmt::Debug for SyncSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSnapshot")
            .field("block", self.block())
            .finish()
    }
}

macro_rules! common_reads {
    ($target:ty) => {
        impl $target {
            /// Authenticated read catalog; this performs no I/O.
            pub fn reads(&self) -> &'static [rtao_reads::ReadSpec] {
                &rtao_reads::READ_SPECS
            }

            /// Dispatch a JSON read using the same catalog parser as async use.
            pub fn read(
                &self,
                name: rtao_reads::ReadName,
                params: serde_json::Map<String, serde_json::Value>,
            ) -> Result<serde_json::Value, Error> {
                self.view.with_view(move |view| async move {
                    rtao_reads::dispatch(&view, name, &params).await
                })
            }

            /// Copy cached connection symbols without I/O or worker startup.
            pub fn token_symbols(&self) -> Result<BTreeMap<NetUid, String>, Error> {
                self.view
                    .shared
                    .token_symbols
                    .lock()
                    .map(|symbols| symbols.clone())
                    .map_err(|_| Error::Poisoned)
            }

            /// Tag a native amount with the connected token symbol, without I/O.
            pub fn balance(&self, rao: u64, netuid: NetUid) -> DynBalance {
                let amount = DynBalance::from_rao(rao, netuid);
                if netuid == NetUid::ROOT {
                    return amount;
                }
                let symbols = self
                    .view
                    .shared
                    .token_symbols
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match symbols.get(&netuid) {
                    Some(symbol) => amount.with_symbol(symbol.clone()),
                    None => amount,
                }
            }

            /// Query live storage by pallet and item name.
            pub fn query(
                &self,
                pallet: String,
                item: String,
                params: Vec<Value>,
            ) -> Result<DynValue, Error> {
                self.view.with_view(move |view| async move {
                    view.storage_dyn(&pallet, &item, &params).await
                })
            }

            /// Collect a pinned storage map in lexicographic encoded-key order.
            pub fn query_map(
                &self,
                pallet: String,
                item: String,
                params: Vec<Value>,
            ) -> Result<Vec<(Vec<DynValue>, DynValue)>, Error> {
                self.view.with_view(move |view| async move {
                    view.query_map_dyn(&pallet, &item, params)
                        .try_collect()
                        .await
                })
            }

            /// Read multiple keys in one pinned, coalesced storage request.
            pub fn query_batch(
                &self,
                pallet: String,
                item: String,
                param_sets: Vec<Vec<Value>>,
            ) -> Result<Vec<DynValue>, Error> {
                self.view.with_view(move |view| async move {
                    view.query_batch_dyn(&pallet, &item, param_sets).await
                })
            }

            /// Invoke a modern or frozen archive runtime API at this view.
            pub fn runtime(
                &self,
                api: String,
                method: String,
                params: Vec<Value>,
            ) -> Result<DynValue, Error> {
                self.view.with_view(move |view| async move {
                    view.call_dyn(&api, &method, &params).await
                })
            }

            /// Decode a runtime constant without a storage RPC.
            pub fn constant(&self, pallet: String, item: String) -> Result<DynValue, Error> {
                self.view.with_view(move |view| async move {
                    let runtime = view.runtime().await?;
                    let info = runtime
                        .core()
                        .constant(&pallet, &item)
                        .cloned()
                        .ok_or_else(|| {
                            ClientError::from(rtao_core::CoreError::NotInRuntime(format!(
                                "constant {pallet}.{item}"
                            )))
                        })?;
                    let offload = info.value.len() > 64 * 1024;
                    let decode = move || {
                        runtime
                            .core()
                            .decode_spec(
                                &rtao_core::runtime::TypeSpec::Id(info.ty),
                                &info.value,
                                true,
                            )
                            .map(DynValue::new)
                            .map_err(rtao_core::CoreError::from)
                            .map_err(ClientError::from)
                    };
                    if offload {
                        rtao_core::offload(decode).await.map_err(|error| {
                            ClientError::Protocol(format!("constant decoder task failed: {error}"))
                        })?
                    } else {
                        decode()
                    }
                })
            }

            /// Runtime block duration in seconds, preserving Python float conversion.
            pub fn block_time(&self) -> Result<f64, Error> {
                self.view.with_view(|view| async move {
                    rtao_reads::reads::chain::block_time(&view).await
                })
            }

            /// Read a block's header, UTC timestamp and metadata-decoded extrinsics.
            /// An omitted block retains a snapshot's already pinned identity.
            pub fn block_info(
                &self,
                block: Option<u64>,
            ) -> Result<Option<rtao_client::BlockInfo>, Error> {
                self.view.with_view(move |view| async move {
                    match view {
                        AsyncView::Client(client) => client.block_info(block).await,
                        AsyncView::Snapshot(snapshot) => snapshot.block_info(block).await,
                    }
                })
            }

            /// Whether the runtime uses the pinned Python fast block duration.
            pub fn is_fast_blocks(&self) -> Result<bool, Error> {
                Ok(self.block_time()? == 0.25)
            }
        }
    };
}
common_reads!(SyncClient);
common_reads!(SyncSnapshot);

impl SyncBalances<'_> {
    /// Free native TAO balance of one coldkey.
    pub fn get(&self, address: AccountId) -> Result<DynBalance, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::owner::balance(&view, address).await
        })
    }

    /// Free TAO balances in one pinned batch, collapsing duplicate addresses.
    pub fn get_many(
        &self,
        addresses: Vec<AccountId>,
    ) -> Result<BTreeMap<AccountId, DynBalance>, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::owner::balances(&view, addresses).await
        })
    }
}

impl SyncStaking<'_> {
    /// Exact subnet-denominated stake held by one coldkey/hotkey pair.
    pub fn get(
        &self,
        coldkey_ss58: AccountId,
        hotkey_ss58: AccountId,
        netuid: NetUid,
    ) -> Result<DynBalance, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::m6_staking::stake(&view, coldkey_ss58, hotkey_ss58, netuid).await
        })
    }

    /// Every runtime-reported stake position, retaining order and denomination.
    pub fn positions(
        &self,
        coldkey_ss58: AccountId,
    ) -> Result<Vec<rtao_reads::reads::owner::StakePosition>, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::m6_staking::stake_for_coldkey(&view, coldkey_ss58).await
        })
    }
}

impl SyncSubnets<'_> {
    /// Subnet tempo, registration burn and neuron count at one block.
    pub fn info(&self, netuid: NetUid) -> Result<rtao_reads::reads::m6_root::SubnetInfo, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::m6_root::subnet(&view, netuid).await
        })
    }

    /// Every subnet using four pinned batched maps.
    pub fn all(&self) -> Result<Vec<rtao_reads::reads::m6_root::SubnetInfo>, Error> {
        self.view
            .with_view(|view| async move { rtao_reads::reads::m6_root::subnets(&view).await })
    }

    /// Full typed graph and commitments at one immutable block.
    pub fn metagraph(&self, netuid: NetUid) -> Result<Option<rtao_metagraph::Metagraph>, Error> {
        self.metagraph_with(netuid, rtao_metagraph::MetagraphOptions::default())
    }

    /// Typed graph with explicit commitment and decoder options.
    pub fn metagraph_with(
        &self,
        netuid: NetUid,
        options: rtao_metagraph::MetagraphOptions,
    ) -> Result<Option<rtao_metagraph::Metagraph>, Error> {
        self.view.with_view(move |view| async move {
            use rtao_metagraph::SubnetsMetagraphExt;
            rtao_reads::Subnets::new(&view)
                .metagraph_with(netuid, options)
                .await
        })
    }

    /// Typed graph of one mechanism with explicit commitment/decoder options.
    pub fn mechagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        options: rtao_metagraph::MetagraphOptions,
    ) -> Result<Option<rtao_metagraph::Metagraph>, Error> {
        self.view.with_view(move |view| async move {
            use rtao_metagraph::SubnetsMetagraphExt;
            rtao_reads::Subnets::new(&view)
                .mechagraph_with(netuid, mechid, options)
                .await
        })
    }

    /// Select graph fields while retaining the same pinned decoding contract.
    pub fn selective_metagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        fields: Vec<rtao_runtime::SelectiveMetagraphIndex>,
        options: rtao_metagraph::MetagraphOptions,
    ) -> Result<Option<rtao_metagraph::Metagraph>, Error> {
        self.view.with_view(move |view| async move {
            use rtao_metagraph::SubnetsMetagraphExt;
            rtao_reads::Subnets::new(&view)
                .selective_metagraph_with(netuid, mechid, &fields, options)
                .await
        })
    }

    /// Current and revealed commitments, including deregistered hotkeys.
    pub fn commitments(
        &self,
        netuid: NetUid,
    ) -> Result<rtao_reads::commitments::CommitmentMap, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::commitments::fetch_commitments(&view, netuid).await
        })
    }
}

impl SyncNeurons<'_> {
    /// Neurons ordered by UID from one full or lite runtime API response.
    pub fn all(
        &self,
        netuid: NetUid,
        lite: bool,
    ) -> Result<Vec<rtao_reads::reads::m6_accounts::Neuron>, Error> {
        self.view.with_view(move |view| async move {
            rtao_reads::reads::m6_accounts::neurons(&view, netuid, lite).await
        })
    }
}

/// Blocking header iterator. Dropping it releases its subscription receiver.
pub struct Blocks {
    shared: Arc<Shared>,
    stream: Arc<tokio::sync::Mutex<rtao_client::BlockStream>>,
    ended: bool,
}

/// A subnet epoch whose advancement was observed in chain storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochEvent {
    /// Subnet whose epoch ran.
    pub netuid: NetUid,
    /// Observed LastEpochBlock after the run.
    pub block: u64,
    /// LastEpochBlock at the beginning of the wait.
    pub previous: u64,
}

async fn bounded_wait<T>(
    timeout: Option<Duration>,
    wait: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    match timeout {
        Some(limit) => tokio::time::timeout(limit, wait)
            .await
            .map_err(|_| Error::Timeout)?,
        None => wait.await,
    }
}

impl Iterator for Blocks {
    type Item = Result<BlockHeader, Error>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        if let Err(error) = self.shared.choose_blocking() {
            self.ended = true;
            return Some(Err(error));
        }
        let stream = self.stream.clone();
        match self
            .shared
            .runtime
            .run(async move { stream.lock().await.next().await })
        {
            Ok(Some(header)) => Some(header.map_err(Error::from)),
            Ok(None) => {
                self.ended = true;
                None
            }
            Err(error) => {
                self.ended = true;
                Some(Err(error.into()))
            }
        }
    }
}

/// Reusable blocking multisig account with the async approval implementation.
#[derive(Clone)]
pub struct SyncMultisig {
    client: SyncClient,
    multisig: rtao_tx::Multisig,
}

impl SyncMultisig {
    /// Canonical signer set, threshold and derived address; no I/O.
    pub fn account(&self) -> &rtao_tx::Multisig {
        &self.multisig
    }

    /// Approve and, when threshold is reached, execute the complete inner call.
    pub fn approve(
        &self,
        call: rtao_client::CallBytes,
        keys: Arc<dyn DynKeySource>,
        options: rtao_tx::ApproveOptions,
    ) -> Result<ExtrinsicResult, Error> {
        let multisig = self.multisig.clone();
        self.client.view.with_executor(move |executor| async move {
            multisig
                .approve(&executor, call, keys.as_ref(), &options)
                .await
        })
    }
}

/// Classic entry point: blocking methods directly, or consume it with `.await`.
///
/// The first blocking operation locks all clones into blocking mode. Awaiting
/// locks them into async mode, connects on the caller's runtime and returns
/// the ordinary [`Client`]. Merely inspecting namespaces or Debug stays cold.
#[derive(Clone, Default)]
pub struct Subtensor(SyncClient);

impl Subtensor {
    /// Construct a cold dual-mode facade with an explicit connection config.
    pub fn new(config: ClientConfig) -> Self {
        Self(SyncClient::new(config))
    }

    /// Resolve a named network or endpoint without connecting.
    pub fn for_network(network: &str) -> Result<Self, Error> {
        Ok(Self(SyncClient::for_network(network)?))
    }

    /// Wrap a cold client, including one with an injected fake connector.
    pub fn from_sync(client: SyncClient) -> Self {
        Self(client)
    }

    /// Select async mode and return the single async implementation.
    pub async fn into_async(self) -> Result<Client, Error> {
        let shared = &self.0.view.shared;
        if shared.closed.load(Ordering::Acquire) {
            return Err(BlockingError::Closed.into());
        }
        shared.mode.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .or_else(|mode| if mode == 2 { Ok(mode) } else { Err(mode) })
            .map_err(|_| Error::Mode(
                "this Subtensor selected blocking mode; construct a fresh Subtensor for async use"
            ))?;
        shared.connected().await
    }
}

impl Deref for Subtensor {
    type Target = SyncClient;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoFuture for Subtensor {
    type Output = Result<Client, Error>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.into_async())
    }
}

impl std::fmt::Debug for Subtensor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match self.0.view.shared.mode.load(Ordering::Acquire) {
            1 => "blocking",
            2 => "async",
            _ => "cold",
        };
        f.debug_struct("Subtensor")
            .field("network", self.network())
            .field("mode", &mode)
            .finish()
    }
}

static SHARED_CLIENTS: OnceLock<Mutex<BTreeMap<String, SyncClient>>> = OnceLock::new();

fn shared_client(network: &str) -> Result<SyncClient, Error> {
    let config = ClientConfig::for_network(network)?;
    let endpoint = config.network.endpoint().to_owned();
    let mut clients = SHARED_CLIENTS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Poisoned)?;
    Ok(clients
        .entry(endpoint)
        .or_insert_with(|| SyncClient::new(config))
        .clone())
}

/// Close and remove every module-level cached client.
///
/// The registry is drained before cleanup, so failure to stop one worker does
/// not prevent cleanup of the others. The first cleanup error is returned.
pub fn close_shared_clients() -> Result<(), Error> {
    let clients = {
        let mut registry = SHARED_CLIENTS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::Poisoned)?;
        std::mem::take(&mut *registry)
    };
    let mut first_error = None;
    for client in clients.into_values() {
        if let Err(error) = client.close() {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Set validator weights through an endpoint-shared client and hotkey source.
///
/// The existing intent conforms weights, selects plaintext or commit/reveal,
/// preflights, signs and submits. Included dispatch failures become an error.
/// Pass `()` for the default wallet, a wallet name or `"wallet/hotkey"` string,
/// a [`WeightsWallet`] selection, a wallet object, or an `Arc` key source.
/// Resolving these inputs never reads a keyfile or unlocks a signer; the
/// executor obtains public identity and completes review before private access.
pub fn set_weights<S>(
    network: &str,
    intent: rtao_intents::SetWeights,
    source: S,
) -> Result<ExtrinsicResult<rtao_tx::WeightsSubmitted>, Error>
where
    S: WeightsKeySource,
{
    set_weights_with_retries(network, intent, source, 2)
}

/// Set validator weights with an explicit transient-pool retry count.
pub fn set_weights_with_retries<S>(
    network: &str,
    intent: rtao_intents::SetWeights,
    source: S,
    retries: u32,
) -> Result<ExtrinsicResult<rtao_tx::WeightsSubmitted>, Error>
where
    S: WeightsKeySource,
{
    let keys = source.into_weights_keys()?;
    shared_client(network)?
        .execute(
            intent,
            keys,
            ExecuteOptions {
                retries,
                ..Default::default()
            },
        )?
        .raise_for_failure()
        .map_err(Error::from)
}

/// Wallet selection for the module-level validator weights helper.
///
/// Names use the pinned Python wallet root `~/.bittensor/wallets`. Supply an
/// existing [`rtao_wallet::Wallet`] for an explicit alternative root. This
/// carrier remains cold and never reads or decrypts wallet files.
#[derive(Clone, Debug, Default)]
pub enum WeightsWallet {
    /// Python's default wallet and hotkey names.
    #[default]
    Default,
    /// A wallet name, optionally paired with an explicit hotkey name.
    Named {
        /// Wallet name, including the optional `wallet/hotkey` shorthand.
        name: String,
        /// Explicit hotkey; cannot also appear in the wallet name.
        hotkey: Option<String>,
    },
    /// Caller-owned wallet, including its selected hotkey and unlock policy.
    Wallet(rtao_wallet::Wallet),
}

impl WeightsWallet {
    /// Choose a wallet name or the `wallet/hotkey` shorthand without I/O.
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named {
            name: name.into(),
            hotkey: None,
        }
    }

    /// Pair a named/default wallet with an explicit hotkey name.
    ///
    /// As in Python, a wallet object already carries its hotkey. Resolving a
    /// slash-name together with an explicit hotkey also refuses before I/O.
    pub fn with_hotkey(self, hotkey: impl Into<String>) -> Result<Self, Error> {
        match self {
            Self::Default => Ok(Self::Named { name: "default".into(), hotkey: Some(hotkey.into()) }),
            Self::Named { name, .. } => Ok(Self::Named { name, hotkey: Some(hotkey.into()) }),
            Self::Wallet(_) => Err(rtao_wallet::WalletError::InvalidName(
                "hotkey= only combines with a wallet name; a wallet object already carries its hotkey".into(),
            ).into()),
        }
    }
}

/// Cold conversion accepted by [`set_weights`] and [`set_weights_with_retries`].
///
/// The built-in conversions preserve laziness: public identity reads and
/// private signer acquisition stay inside the existing transaction executor.
pub trait WeightsKeySource {
    /// Owned key source supporting only the role required by SetWeights.
    type Keys: KeySource<rtao_signer::Hotkey> + 'static;
    /// Resolve names/layout without reading or unlocking wallet files.
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error>;
}

impl<K: KeySource<rtao_signer::Hotkey> + 'static> WeightsKeySource for Arc<K> {
    type Keys = K;
    fn into_weights_keys(self) -> Result<Arc<K>, Error> {
        Ok(self)
    }
}

impl WeightsKeySource for WeightsWallet {
    type Keys = rtao_wallet::Wallet;
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error> {
        let wallet = match self {
            Self::Default => rtao_wallet::Wallet::default_wallet()?,
            Self::Named { name, hotkey } => {
                let name = if name.is_empty() {
                    "default".into()
                } else {
                    name
                };
                if name.contains('/') && hotkey.is_some() {
                    return Err(rtao_wallet::WalletError::InvalidName(
                        "give the hotkey either in the wallet name ('cold/hot') or as an explicit hotkey, not both".into(),
                    ).into());
                }
                rtao_wallet::Wallet::new(
                    name,
                    hotkey.unwrap_or_else(|| "default".into()),
                    "~/.bittensor/wallets",
                )?
            }
            Self::Wallet(wallet) => wallet,
        };
        Ok(Arc::new(wallet))
    }
}

impl WeightsKeySource for () {
    type Keys = rtao_wallet::Wallet;
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error> {
        WeightsWallet::Default.into_weights_keys()
    }
}

impl WeightsKeySource for &str {
    type Keys = rtao_wallet::Wallet;
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error> {
        WeightsWallet::named(self).into_weights_keys()
    }
}

impl WeightsKeySource for String {
    type Keys = rtao_wallet::Wallet;
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error> {
        WeightsWallet::named(self).into_weights_keys()
    }
}

impl WeightsKeySource for rtao_wallet::Wallet {
    type Keys = Self;
    fn into_weights_keys(self) -> Result<Arc<Self::Keys>, Error> {
        Ok(Arc::new(self))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use rtao_primitives::{Balance, Tao};
    use rtao_signer::{Coldkey, KeyView, Signer, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    const COLD: AccountId = AccountId([7; 32]);
    const OTHER: AccountId = AccountId([8; 32]);

    fn config() -> ClientConfig {
        let mut config = ClientConfig::for_network("local").unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        config
    }

    fn setup() -> (SyncClient, FakeChain, Arc<AtomicUsize>) {
        let mut state = FakeState::default();
        state
            .seed_python(
                "System",
                "Account",
                &[json!(COLD)],
                json!({
                    "data": {"free": 10_000_000_000u64}
                }),
            )
            .unwrap();
        state
            .seed_constant("Aura", "SlotDuration", json!(250))
            .unwrap();
        state
            .seed_python("Timestamp", "Now", &[], json!(1_700_000_000_123u64))
            .unwrap();
        let chain = FakeChain::from_state(state);
        let backend = chain.clone();
        let starts = Arc::new(AtomicUsize::new(0));
        let connected = starts.clone();
        let client = SyncClient::with_connector(config(), move |config| {
            connected.fetch_add(1, Ordering::SeqCst);
            let backend = backend.clone();
            async move { Client::from_rpc(RpcClient::new(backend), config).await }
        });
        (client, chain, starts)
    }

    fn inspect_all_namespaces(client: &SyncClient) {
        let _ = client.balances();
        let _ = client.chain();
        let _ = client.collateral();
        let _ = client.delegation();
        let _ = client.epochs();
        let _ = client.hyperparameters();
        let _ = client.identity();
        let _ = client.leasing();
        let _ = client.locks();
        let _ = client.neurons();
        let _ = client.prices();
        let _ = client.staking();
        let _ = client.subnets();
        let _ = client.weights();
    }

    // pyt: unit/test_facades.py::test_namespace_lists_agree,
    // test_sync_client_constructs_offline; D-027 includes collateral.
    #[test]
    fn sync_client_has_all_namespaces() {
        let (client, chain, starts) = setup();
        inspect_all_namespaces(&client);
        assert_eq!(client.reads().len(), 102);
        assert_eq!(client.tools().unwrap().len(), 83);
        assert!(format!("{client:?}").contains("started: false"));
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(chain.requests().is_empty());
        assert!(!client.is_started());
        client.close().unwrap();
        client.close().unwrap();
        assert!(!client.is_started());
        assert!(!client.is_closed());
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        client.connect().unwrap();
        client.close().unwrap();
        assert!(client.is_closed());
        assert!(matches!(
            client.connect(),
            Err(Error::Runtime(BlockingError::Closed))
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn balance_factories_keep_cached_symbols_without_starting_cold_work_or_rpc() {
        let (client, chain, starts) = setup();
        let netuid = NetUid(3);
        let rao = 1_000_000_001;
        assert!(client.token_symbols().unwrap().is_empty());
        assert_eq!(
            client.balance(rao, netuid).to_string(),
            DynBalance::from_rao(rao, netuid).to_string()
        );
        assert!(chain.requests().is_empty());
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(!client.is_started());
        chain.mutate_state(|state| {
            state
                .seed_typed(
                    "SubtensorModule",
                    "TokenSymbol",
                    (NetUid::ROOT,),
                    b"root symbol is ignored".to_vec(),
                )
                .unwrap();
            state
                .seed_typed(
                    "SubtensorModule",
                    "TokenSymbol",
                    (netuid,),
                    "λ".as_bytes().to_vec(),
                )
                .unwrap();
        });
        client.connect().unwrap();
        let snapshot = client.at(BlockRef::Best).unwrap();
        let requests = chain.requests().len();
        let expected_symbols = BTreeMap::from([
            (NetUid::ROOT, "root symbol is ignored".into()),
            (netuid, "λ".into()),
        ]);
        assert_eq!(client.token_symbols().unwrap(), expected_symbols);
        assert_eq!(snapshot.token_symbols().unwrap(), expected_symbols);
        let mut copied_symbols = client.token_symbols().unwrap();
        copied_symbols.clear();
        assert_eq!(client.token_symbols().unwrap(), expected_symbols);
        for value in [client.balance(rao, netuid), snapshot.balance(rao, netuid)] {
            assert_eq!(value.rao, rao);
            assert_eq!(value.netuid, netuid);
            assert_eq!(value.to_string(), "1.000000001λ");
        }
        for value in [
            client.balance(rao, NetUid::ROOT),
            snapshot.balance(rao, NetUid::ROOT),
        ] {
            assert!(value.symbol.is_none());
            assert_eq!(value.to_string(), "τ1.000000001");
        }
        for value in [
            client.balance(rao, NetUid(42)),
            snapshot.balance(rao, NetUid(42)),
        ] {
            assert_eq!(
                value.to_string(),
                DynBalance::from_rao(rao, NetUid(42)).to_string()
            );
        }
        assert_eq!(chain.requests().len(), requests);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        client.close().unwrap();
        assert_eq!(client.token_symbols().unwrap(), expected_symbols);
        assert_eq!(snapshot.token_symbols().unwrap(), expected_symbols);
        assert_eq!(chain.requests().len(), requests);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    // Python SyncClient._call connects before invoking query_batch, even for
    // an empty batch. The batch itself then skips metadata and storage RPC.
    #[test]
    fn empty_named_batch_connects_once_then_skips_metadata_and_rpc() {
        let (client, chain, starts) = setup();
        assert!(
            client
                .query_batch("UnknownPallet".into(), "UnknownStorage".into(), vec![])
                .unwrap()
                .is_empty()
        );
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(client.is_started());
        assert!(!chain.requests().is_empty());
        let snapshot = client.at(BlockRef::Best).unwrap();
        let before = chain.requests();
        assert!(
            client
                .query_batch("UnknownPallet".into(), "UnknownStorage".into(), vec![])
                .unwrap()
                .is_empty()
        );
        assert!(
            snapshot
                .query_batch("UnknownPallet".into(), "UnknownStorage".into(), vec![])
                .unwrap()
                .is_empty()
        );
        assert_eq!(chain.requests(), before);
        client.close().unwrap();
        assert!(matches!(
            client.query_batch("UnknownPallet".into(), "UnknownStorage".into(), vec![]),
            Err(Error::Runtime(BlockingError::Closed))
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn first_read_connects_once_and_generic_batch_uses_one_storage_rpc() {
        let (client, chain, starts) = setup();
        assert_eq!(client.balances().get(COLD).unwrap().rao, 10_000_000_000);
        client.connect().unwrap();
        let before = chain.requests().len();
        let result = client
            .query_batch(
                "System".into(),
                "Account".into(),
                vec![
                    vec![Value::Str(COLD.to_string())],
                    vec![Value::Str(OTHER.to_string())],
                    vec![Value::Str(COLD.to_string())],
                ],
            )
            .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(
            result[0].to_json()["data"]["free"],
            json!(10_000_000_000u64)
        );
        assert_eq!(result[1].to_json()["data"]["free"], json!(0));
        assert_eq!(result[2].to_json(), result[0].to_json());
        let requests = chain.requests();
        assert_eq!(
            requests[before..]
                .iter()
                .filter(|request| request.method == "state_queryStorageAt")
                .count(),
            1
        );
        let batch = requests[before..]
            .iter()
            .find(|request| request.method == "state_queryStorageAt")
            .unwrap();
        assert_eq!(batch.params[0].as_array().unwrap().len(), 2);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(client.is_fast_blocks().unwrap());
        client.close().unwrap();
    }

    #[test]
    fn snapshot_namespaces_and_generic_accessors_retain_the_same_hash() {
        let (client, chain, starts) = setup();
        let snapshot = client.at(BlockRef::Best).unwrap();
        let _ = snapshot.balances();
        let _ = snapshot.chain();
        let _ = snapshot.collateral();
        let _ = snapshot.delegation();
        let _ = snapshot.epochs();
        let _ = snapshot.hyperparameters();
        let _ = snapshot.identity();
        let _ = snapshot.leasing();
        let _ = snapshot.locks();
        let _ = snapshot.neurons();
        let _ = snapshot.prices();
        let _ = snapshot.staking();
        let _ = snapshot.subnets();
        let _ = snapshot.weights();
        assert_eq!(snapshot.balances().get(COLD).unwrap().rao, 10_000_000_000);
        let old_hash = snapshot.block().hash;
        chain.advance(1).unwrap();
        chain.mutate_state(|state| {
            state
                .seed_python(
                    "System",
                    "Account",
                    &[json!(COLD)],
                    json!({
                        "data": {"free": 42}
                    }),
                )
                .unwrap();
        });
        let before = chain.requests().len();
        assert_eq!(snapshot.balances().get(COLD).unwrap().rao, 10_000_000_000);
        assert_eq!(
            snapshot
                .query(
                    "System".into(),
                    "Account".into(),
                    vec![Value::Str(COLD.to_string()),]
                )
                .unwrap()
                .to_json()["data"]["free"],
            json!(10_000_000_000u64)
        );
        let expected = json!(format!(
            "0x{}",
            old_hash
                .0
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        let requests = chain.requests();
        let storage: Vec<_> = requests[before..]
            .iter()
            .filter(|request| {
                matches!(
                    request.method.as_str(),
                    "state_getStorage" | "state_queryStorageAt"
                )
            })
            .collect();
        assert!(!storage.is_empty());
        assert!(storage.iter().all(|request| request.params[1] == expected));
        assert_eq!(snapshot.at(None).unwrap().block().hash, old_hash);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        client.close().unwrap();
        assert!(matches!(
            snapshot.timestamp(),
            Err(Error::Runtime(BlockingError::Closed))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_calls_are_usable_from_a_tokio_caller_without_nested_block_on() {
        let (client, _, starts) = setup();
        let caller = std::thread::current().id();
        let worker = client
            .view
            .with_view(|_| async move { Ok::<_, Error>(std::thread::current().id()) })
            .unwrap();
        assert_ne!(caller, worker);
        assert_eq!(client.balances().get(COLD).unwrap().rao, 10_000_000_000);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        client.close().unwrap();
    }

    #[test]
    fn concurrent_first_calls_share_one_lazy_connection() {
        let (client, _, starts) = setup();
        let first = client.clone();
        let second = client.clone();
        let a = std::thread::spawn(move || first.balances().get(COLD));
        let b = std::thread::spawn(move || second.balances().get(COLD));
        assert_eq!(a.join().unwrap().unwrap().rao, 10_000_000_000);
        assert_eq!(b.join().unwrap().unwrap().rao, 10_000_000_000);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        client.close().unwrap();
    }

    #[tokio::test]
    async fn subtensor_mode_is_explicit_and_attribute_inspection_stays_cold() {
        let (client, _, starts) = setup();
        let sub = Subtensor::from_sync(client);
        inspect_all_namespaces(&sub);
        assert!(format!("{sub:?}").contains("cold"));
        sub.close().unwrap();
        assert!(!sub.is_closed());
        let retained = sub.clone();
        let asynchronous = sub.await.unwrap();
        assert!(!retained.is_started());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(matches!(retained.connect(), Err(Error::Mode(_))));
        assert!(matches!(retained.close(), Err(Error::Mode(_))));
        asynchronous.close().await;

        let (client, _, _) = setup();
        let sub = Subtensor::from_sync(client);
        sub.connect().unwrap();
        let retained = sub.clone();
        assert!(matches!(sub.await, Err(Error::Mode(_))));
        retained.close().unwrap();
    }

    #[test]
    fn failed_connect_can_be_closed_without_another_connection_attempt() {
        let starts = Arc::new(AtomicUsize::new(0));
        let attempts = starts.clone();
        let client = SyncClient::with_connector(config(), move |_| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(ClientError::Protocol("injected connection failure".into())) }
        });
        assert!(matches!(client.connect(), Err(Error::Client(_))));
        assert!(client.is_started());
        client.close().unwrap();
        assert!(client.is_closed());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn worker_thread_close_refuses_without_losing_the_normal_cleanup_path() {
        let (client, _, _) = setup();
        let nested = client.clone();
        client
            .view
            .with_view(move |_| async move {
                assert!(matches!(
                    nested.close(),
                    Err(Error::Runtime(BlockingError::OwnThread))
                ));
                Ok::<_, Error>(())
            })
            .unwrap();
        assert!(!client.is_closed());
        assert_eq!(client.balances().get(COLD).unwrap().rao, 10_000_000_000);
        client.close().unwrap();
        assert!(client.is_closed());
    }

    #[test]
    fn stalled_worker_poll_does_not_strand_close_and_concurrent_closes_share_its_result() {
        struct ReleaseOnDrop(Option<mpsc::Sender<()>>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let (client, _, _) = setup();
        client.connect().unwrap();
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let release = ReleaseOnDrop(Some(release_sender));
        let active_client = client.clone();
        let (active_sender, active_receiver) = mpsc::channel();
        let active = std::thread::spawn(move || {
            let result = active_client.view.with_view(move |_| async move {
                started_sender.send(()).unwrap();
                // Deliberately block one poll: a timer on this same worker
                // cannot fire until the fixture releases it.
                release_receiver.recv().unwrap();
                Ok::<_, Error>(())
            });
            active_sender.send(result).unwrap();
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let first_client = client.clone();
        let second_client = client.clone();
        let (first_sender, first_receiver) = mpsc::channel();
        let (second_sender, second_receiver) = mpsc::channel();
        let first = std::thread::spawn(move || {
            first_sender
                .send(first_client.close_with_timeout(Duration::from_millis(50)))
                .unwrap();
        });
        let second = std::thread::spawn(move || {
            second_sender
                .send(second_client.close_with_timeout(Duration::from_millis(50)))
                .unwrap();
        });

        // The caller-side timeout must reach runtime.close() and wake the
        // outstanding call before this test allows the worker poll to return.
        let cancelled = active_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(matches!(
            cancelled,
            Err(Error::Runtime(BlockingError::TaskCancelled))
        ));
        assert!(client.is_closed());
        assert!(client.view.shared.runtime.is_closed());
        assert!(matches!(
            first_receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(matches!(
            second_receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(release);
        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            Err(Error::Timeout)
        ));
        assert!(matches!(
            second_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap(),
            Err(Error::Timeout)
        ));
        assert!(matches!(client.close(), Err(Error::Timeout)));
        active.join().unwrap();
        first.join().unwrap();
        second.join().unwrap();
    }

    #[test]
    fn default_named_and_existing_weights_wallets_resolve_without_keyfile_access() {
        let default = ().into_weights_keys().unwrap();
        assert_eq!(default.reference().name(), "default");
        assert_eq!(default.reference().hotkey_name(), "default");
        let empty = "".into_weights_keys().unwrap();
        assert_eq!(empty.reference(), default.reference());
        let explicit = WeightsWallet::named("")
            .with_hotkey("validator")
            .unwrap()
            .into_weights_keys()
            .unwrap();
        assert_eq!(explicit.reference().name(), "default");
        assert_eq!(explicit.reference().hotkey_name(), "validator");
        let named = WeightsWallet::named("weights-helper-nonexistent-wallet")
            .with_hotkey("validator")
            .unwrap()
            .into_weights_keys()
            .unwrap();
        assert_eq!(
            named.reference().name(),
            "weights-helper-nonexistent-wallet"
        );
        assert_eq!(named.reference().hotkey_name(), "validator");
        let compound = "weights-helper-nonexistent-wallet/validator"
            .into_weights_keys()
            .unwrap();
        assert_eq!(compound.reference(), named.reference());
        let wallet = rtao_wallet::Wallet::new(
            "weights-helper-nonexistent-wallet",
            "validator",
            "/rtao-nonexistent-wallet-fixture",
        )
        .unwrap();
        let existing = wallet.clone().into_weights_keys().unwrap();
        assert_eq!(existing.reference(), wallet.reference());
        assert!(matches!(
            WeightsWallet::Wallet(wallet.clone()).with_hotkey("other"),
            Err(Error::Wallet(_))
        ));
        assert!(matches!(
            WeightsWallet::named("cold/hot")
                .with_hotkey("default")
                .unwrap()
                .into_weights_keys(),
            Err(Error::Wallet(_))
        ));

        let intent = || {
            serde_json::from_value::<rtao_intents::SetWeights>(json!({
                "netuid": 1, "weights": {"0": 1.0}
            }))
            .unwrap()
        };
        for result in [
            set_weights("missing-network", intent(), ()),
            set_weights(
                "missing-network",
                intent(),
                "weights-helper-nonexistent-wallet/validator",
            ),
            set_weights("missing-network", intent(), wallet),
        ] {
            // No missing-keyfile or unlock error can precede selecting the
            // connection on which operation review would take place.
            assert!(matches!(
                result,
                Err(Error::Client(ClientError::Protocol(_)))
            ));
        }
    }

    #[test]
    fn shared_clients_use_one_cold_entry_per_resolved_endpoint_and_drain_on_close() {
        let named = shared_client("finney").unwrap();
        let explicit = shared_client(named.endpoint()).unwrap();
        assert!(Arc::ptr_eq(&named.view.shared, &explicit.view.shared));
        assert!(!named.is_started());
        close_shared_clients().unwrap();
        assert!(!named.is_started());
        assert!(!explicit.is_started());
        let replacement = shared_client("finney").unwrap();
        assert!(!Arc::ptr_eq(&named.view.shared, &replacement.view.shared));
        assert!(!replacement.is_started());
        close_shared_clients().unwrap();
    }

    struct PublicOnly {
        acquisitions: AtomicUsize,
    }
    impl KeyView for PublicOnly {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(OTHER)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(OTHER.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(COLD)
        }
    }
    impl KeySource<Coldkey> for PublicOnly {
        fn address(&self) -> Result<AccountId, SignerError> {
            Ok(COLD)
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.acquisitions.fetch_add(1, Ordering::SeqCst);
            panic!("blocking preview attempted a private-key acquisition");
        }
    }

    #[test]
    fn typed_preview_uses_the_real_executor_without_acquiring_private_keys() {
        let (client, chain, _) = setup();
        let keys = Arc::new(PublicOnly {
            acquisitions: AtomicUsize::new(0),
        });
        let intent = rtao_intents::Transfer {
            dest_ss58: OTHER.into(),
            amount_tao: Balance::<Tao>::from_rao(42).into(),
            keep_alive: true,
        };
        let plan = client
            .plan(intent.clone(), keys.clone(), PlanOptions::default())
            .unwrap();
        let preview = client
            .preflight(intent, keys.clone(), PlanOptions::default())
            .unwrap();
        assert!(plan.ok());
        assert!(!plan.call().as_bytes().is_empty());
        assert!(preview.blocks.is_empty());
        assert_eq!(keys.acquisitions.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        client.close().unwrap();
    }
}
