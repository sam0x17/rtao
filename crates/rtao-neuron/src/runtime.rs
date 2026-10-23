//! Validator services supervised under a shared shutdown token.

#[cfg(feature = "validator")]
use crate::{ClockGuard, StepPolicy, SystemClock, WeightsProposal};
use crate::{EpochClock, EpochStatus, Metrics, NeuronConfig, StateCell, StateStore};
use futures::StreamExt;
use rtao_btauth::client::SignedClient;
use rtao_client::{BlockRef, Client, ClientConfig, Finality, ViewExt};
#[cfg(feature = "validator")]
use rtao_metagraph::MetagraphDiff;
use rtao_metagraph::{MetagraphHandle, MetagraphSync, SyncPolicy};
use rtao_primitives::{AccountId, MechId, NetUid};
use rtao_signer::{KeyView, Signer};
use rtao_tx::Executor;
use rtao_wallet::HotkeyFile;
use serde::{Serialize, de::DeserializeOwned};
#[cfg(feature = "validator")]
use std::future::Future;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;
#[cfg(feature = "validator")]
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Public chain, signing, graph and HTTP services available to scoring callbacks.
#[derive(Clone)]
pub struct NeuronCx {
    /// Shared verified chain connection.
    pub client: Client,
    /// Policy-limited execution of hotkey intents.
    pub executor: Executor,
    /// The only subnet permitted by the neuron policy.
    pub netuid: NetUid,
    /// Hotkey-only key source; coldkey intents cannot use it.
    pub hotkey: HotkeyFile,
    /// Atomically published last-good validator graph.
    pub metagraph: MetagraphHandle,
    /// Actual finalized epoch state.
    pub epochs: EpochClock,
    /// Bounded btauth/1 signed fan-out client.
    pub http: SignedClient,
    /// Runtime cancellation; callbacks may use a child token.
    pub shutdown: CancellationToken,
    /// Shared readiness and bounded process metrics.
    pub metrics: Metrics,
    /// Operator state directory; keyfiles live outside this directory.
    pub state_dir: PathBuf,
}
impl NeuronCx {
    /// Connect verified chain, hotkey, metagraph and signed HTTP services.
    ///
    /// The configured hotkey must already be registered. This starts the graph
    /// sync worker, but leaves scoring, weight scheduling and persistence to the
    /// caller or an injected runtime.
    pub async fn connect(config: &NeuronConfig) -> anyhow::Result<Self> {
        #[cfg(feature = "validator")]
        {
            initialize(
                config,
                Metrics::default(),
                SyncPolicy::validator(),
                "validator",
            )
            .await
        }
        #[cfg(not(feature = "validator"))]
        {
            Self::connect_miner(config).await
        }
    }
    /// Connect with the miner metagraph preset and miner state directory.
    #[cfg(feature = "miner")]
    pub async fn connect_miner(config: &NeuronConfig) -> anyhow::Result<Self> {
        initialize(config, Metrics::default(), SyncPolicy::miner(), "miner").await
    }
    /// Cancel context work and close its graph worker and chain connection.
    pub async fn close(&self) {
        self.shutdown.cancel();
        self.metagraph.shutdown().await;
        self.client.close().await;
        self.metrics
            .update_readiness(|ready| ready.connected = false);
    }
}

/// Scoring hooks implemented by a subnet validator.
///
/// Every asynchronous callback is `Send`; a [`StateCell`] guard cannot cross
/// an await. Weight generation stays synchronous and reads one consistent state.
#[cfg(feature = "validator")]
pub trait Validator: Send + Sync + 'static {
    /// Serializable shared score state persisted at startup and shutdown.
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    /// Initialize application resources after framework checks and state recovery.
    fn on_start(
        &self,
        _cx: &NeuronCx,
        _state: &StateCell<Self::State>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }
    /// Query miners and update application scores.
    fn step(
        &self,
        cx: &NeuronCx,
        state: &StateCell<Self::State>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Produce a row without network I/O or holding state across asynchronous work.
    fn weights(&self, cx: &NeuronCx, state: &Self::State, mechanism: MechId) -> WeightsProposal;
    /// Optional application semantic contract checked in the current signing runtime.
    /// Matching metadata shapes alone do not establish unchanged application meaning.
    fn weight_semantics(&self) -> Option<(String, Arc<dyn rtao_client::journal::SemanticGuard>)> {
        None
    }
    /// Drop or reset scores after registration identities change.
    fn on_metagraph(&self, _diff: &MetagraphDiff, _state: &mut Self::State) {}
    /// Set scoring cadence, timeout and failure backoff.
    fn step_policy(&self) -> StepPolicy {
        StepPolicy::default()
    }
    /// Application checkpoint schema, changed when saved state becomes incompatible.
    fn state_schema_version(&self) -> u32 {
        1
    }
}

/// Constructors for supervised validator and miner services.
pub struct NeuronRuntime<V = ()> {
    #[cfg(feature = "validator")]
    config: NeuronConfig,
    #[cfg(feature = "validator")]
    validator: Arc<V>,
    #[cfg(feature = "validator")]
    injected: Option<NeuronCx>,
    #[cfg(feature = "validator")]
    weight_policy: crate::WeightPolicy,
    #[cfg(feature = "validator")]
    weight_journal: Option<Arc<rtao_client::journal::OperationJournal>>,
    #[cfg(feature = "validator")]
    metrics: Metrics,
    #[cfg(feature = "validator")]
    clock: Arc<dyn crate::Clock>,
    #[cfg(not(feature = "validator"))]
    marker: std::marker::PhantomData<fn() -> V>,
}
#[cfg(feature = "validator")]
impl<V: Validator> NeuronRuntime<V> {
    /// Configure a validator; chain and wallet I/O begins only in [`Self::run`].
    pub fn validator(config: NeuronConfig, validator: V) -> Self {
        Self {
            config,
            validator: Arc::new(validator),
            injected: None,
            weight_policy: crate::WeightPolicy::default(),
            weight_journal: None,
            metrics: Metrics::default(),
            clock: Arc::new(SystemClock),
        }
    }
    /// Adopt explicitly configured services, useful for embedded runtimes and fakes.
    pub fn with_context(mut self, cx: NeuronCx) -> Self {
        self.injected = Some(cx);
        self
    }
    /// Configure a fixed version key and optional cadence override.
    ///
    /// A fixed key preserves the owner's forced-upgrade policy. The framework
    /// never copies the chain's required version into a proposal automatically.
    pub fn with_weight_policy(mut self, policy: crate::WeightPolicy) -> Self {
        self.weight_policy = policy;
        self
    }
    /// Share the durable journal with the application for inspection and pause.
    /// Otherwise the runtime opens its native journal under the state directory.
    pub fn with_weight_journal(
        mut self,
        journal: Arc<rtao_client::journal::OperationJournal>,
    ) -> Self {
        self.weight_journal = Some(journal);
        self
    }
    /// Share the recorder explicitly installed by the binary before chain startup.
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = metrics;
        self
    }
    /// Select an injected timelock clock; chain clocks are refreshed on each best head.
    pub fn with_clock(mut self, clock: Arc<dyn crate::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Run until cancellation or a framework-fatal failure; user-step failures retry.
    pub async fn run(self) -> anyhow::Result<()> {
        self.validator.step_policy().validate()?;
        let mut cx = match self.injected {
            Some(cx) => cx,
            None => {
                initialize(
                    &self.config,
                    self.metrics,
                    SyncPolicy::validator(),
                    "validator",
                )
                .await?
            }
        };
        anyhow::ensure!(
            cx.netuid == self.config.netuid,
            "injected neuron context belongs to a different subnet"
        );
        cx.executor = cx
            .executor
            .clone()
            .with_policy(validator_policy(cx.netuid))
            .with_clock(Arc::new(ExecutorClock(self.clock.clone())));
        let account = cx.hotkey.hotkey_address()?;
        let schema = self.validator.state_schema_version();
        anyhow::ensure!(
            schema > 0,
            "validator state schema version must be positive"
        );
        let store = Arc::new(StateStore::<V::State>::new(
            cx.state_dir.clone(),
            cx.netuid,
            account,
            schema,
        ));
        let state = StateCell::new(match store.load().await? {
            Some(loaded) => loaded.state,
            None => V::State::default(),
        });
        self.validator.on_start(&cx, &state).await?;
        let (heads, finalized) = watch::channel(cx.epochs.status().block);
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        let clock_guard = ClockGuard::default();
        let supervisor_cx = cx.clone();
        let validator = self.validator.clone();
        let score_state = state.clone();
        tasks.spawn(crate::step::supervise(
            move || {
                let validator = validator.clone();
                let state = score_state.clone();
                let cx = supervisor_cx.clone();
                async move { validator.step(&cx, &state).await }
            },
            finalized,
            self.validator.step_policy(),
            cx.metrics.clone(),
            cx.shutdown.clone(),
        ));
        let head_cx = cx.clone();
        tasks.spawn(async move { finalized_worker(head_cx, heads, "validator").await });
        let graph_cx = cx.clone();
        let graph_state = state.clone();
        let validator = self.validator.clone();
        tasks.spawn(async move { graph_worker(validator, graph_state, graph_cx).await });
        let best_cx = cx.clone();
        let guard = clock_guard.clone();
        let clock = self.clock.clone();
        tasks.spawn(async move { clock_worker(best_cx, guard, clock).await });
        let save_cx = cx.clone();
        let save_state = state.clone();
        let periodic_store = store.clone();
        tasks.spawn(async move { save_worker(periodic_store, save_state, save_cx).await });
        let telemetry_cancel = CancellationToken::new();
        let mut telemetry = if let Some(addr) = self.config.metrics_addr {
            let server = cx.metrics.bind(addr).await?;
            let cancel = telemetry_cancel.clone();
            Some(tokio::spawn(async move { server.run(cancel).await }))
        } else {
            None
        };
        let signals = crate::shutdown::spawn_signals(cx.shutdown.clone());
        let scheduler_cx = cx.clone();
        let scheduler_state = state.clone();
        let validator = self.validator.clone();
        let mut scheduler = tokio::spawn(async move {
            crate::weights::run_scheduler(
                validator,
                scheduler_state,
                scheduler_cx,
                clock_guard,
                self.clock,
                self.weight_policy,
                self.weight_journal,
            )
            .await
        });
        let mut scheduler_finished = false;
        let mut telemetry_finished = false;
        let telemetry_enabled = telemetry.is_some();
        let result = tokio::select! {
                biased;
                () = cx.shutdown.cancelled() => Ok(()),
                finished = async {
                    match telemetry.as_mut() {
                        Some(task) => task.await,
                        None => futures::future::pending().await,
                    }
                }, if telemetry_enabled => {
                    telemetry_finished = true;
                    match finished {
                        Ok(Ok(())) => Err(anyhow::anyhow!("telemetry service stopped unexpectedly")),
                        Ok(Err(error)) => Err(error.into()),
                        Err(error) => Err(error.into()),
                    }
                }
                finished = &mut scheduler => {
                    scheduler_finished = true;
                    match finished {
                        Ok(Ok(())) if cx.shutdown.is_cancelled() => Ok(()),
                        Ok(Ok(())) => Err(anyhow::anyhow!("weight service stopped unexpectedly")),
                        Ok(Err(error)) => Err(error),
                        Err(error) => Err(error.into()),
                    }
                }
                finished = tasks.join_next() => {
                    match finished {
                        Some(Ok(Ok(()))) if cx.shutdown.is_cancelled() => Ok(()),
                        Some(Ok(Ok(()))) => Err(anyhow::anyhow!("framework service stopped unexpectedly")),
                        Some(Ok(Err(error))) => Err(error),
                        Some(Err(error)) => Err(error.into()),
                        None => Err(anyhow::anyhow!("framework services all stopped")),
                    }
                }
        };
        cx.shutdown.cancel();
        // Scoring is cancelled immediately. A weight already in submission may
        // finish its bounded reconciliation before the final persisted snapshot.
        if !scheduler_finished
            && tokio::time::timeout(Duration::from_secs(60), &mut scheduler)
                .await
                .is_err()
        {
            scheduler.abort();
            let _ = scheduler.await;
        }
        cx.metrics.update_readiness(|ready| ready.connected = false);
        if tokio::time::timeout(Duration::from_secs(5), async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        let save = store.save(&state, cx.epochs.status().block).await;
        telemetry_cancel.cancel();
        if let Some(telemetry) = telemetry.filter(|_| !telemetry_finished) {
            let _ = telemetry.await;
        }
        cx.metagraph.shutdown().await;
        cx.client.close().await;
        signals.abort();
        let _ = signals.await;
        save?;
        result
    }
}

/// Resolve the configured network, endpoint pools and TLS connection policy.
///
/// This performs no wallet or registration I/O and is shared by the diagnostic
/// connection check and the neuron runtimes.
pub fn client_config(config: &NeuronConfig) -> anyhow::Result<ClientConfig> {
    let mut chain = ClientConfig::for_network(&config.network)?;
    chain.head_policy = rtao_client::HeadPolicy::Finalized;
    if let Some(endpoint) = &config.chain_endpoint {
        chain.session.pool.urls = vec![endpoint.clone()];
    }
    if let Some(fallbacks) = &config.fallback_endpoints {
        chain.session.pool.urls.truncate(1);
        for endpoint in fallbacks
            .iter()
            .filter(|endpoint| endpoint.as_str() != "none")
        {
            chain.session.pool.urls.push(endpoint.parse()?);
        }
    }
    if let Some(archive) = &config.archive_endpoints {
        chain.session.pool.archive = archive
            .iter()
            .filter(|endpoint| endpoint.as_str() != "none")
            .map(|endpoint| endpoint.parse())
            .collect::<Result<_, _>>()?;
    }
    chain.session.retry_forever = config.retry_forever;
    if let Some(endpoint) = &config.submit_endpoint {
        chain.set_submit_endpoint(endpoint)?;
    }
    Ok(chain)
}
pub(crate) async fn initialize(
    config: &NeuronConfig,
    metrics: Metrics,
    policy: SyncPolicy,
    kind: &'static str,
) -> anyhow::Result<NeuronCx> {
    let chain = client_config(config)?;
    let client = Client::connect(chain).await?;
    initialize_with_client(config, metrics, policy, kind, client).await
}
pub(crate) async fn initialize_with_client(
    config: &NeuronConfig,
    metrics: Metrics,
    policy: SyncPolicy,
    kind: &'static str,
    client: Client,
) -> anyhow::Result<NeuronCx> {
    initialize_with_http_factory(config, metrics, policy, kind, client, |signer| {
        Ok(SignedClient::new(signer)?)
    })
    .await
}
pub(crate) async fn initialize_with_http_factory(
    config: &NeuronConfig,
    metrics: Metrics,
    policy: SyncPolicy,
    kind: &'static str,
    client: Client,
    make_http: impl FnOnce(Arc<dyn Signer>) -> anyhow::Result<SignedClient>,
) -> anyhow::Result<NeuronCx> {
    let initialized = async {
        client.ensure_signing().await?;
        let wallet = config.wallet_reference()?;
        let hotkey = HotkeyFile::new(wallet.hotkey_path());
        let account = hotkey.hotkey_address()?;
        let shutdown = CancellationToken::new();
        let initial = client.at(BlockRef::Finalized).await?;
        if initial
            .storage(
                &rtao_runtime::storage::subtensor_module::UIDS,
                (config.netuid, account),
            )
            .await?
            .is_none()
        {
            anyhow::ensure!(
                config.wait_for_registration,
                "{kind} hotkey is not registered in subnet {}; use --wait-for-registration to wait",
                config.netuid.0
            );
            let signals = crate::shutdown::spawn_signals(shutdown.clone());
            let registered = wait_registered(&client, config.netuid, account, &shutdown).await;
            signals.abort();
            let _ = signals.await;
            registered?;
        }
        let signer = hotkey.unlock().await?;
        let http = make_http(signer)?;
        let (metagraph, _) = MetagraphSync::spawn(
            client.clone(),
            config.netuid,
            MechId(0),
            policy,
            shutdown.clone(),
        )
        .await?;
        let graph = metagraph.load();
        anyhow::ensure!(
            graph.by_hotkey(&account).is_some(),
            "{kind} hotkey is not registered in subnet {}",
            config.netuid.0
        );
        let (initial, verified, spec_version) =
            epoch_at(&client, config.netuid, BlockRef::Finalized, kind).await?;
        let epochs = EpochClock::new(initial);
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is required to resolve the state directory"))?;
        let state_dir = config.state_directory(std::path::Path::new(&home), kind)?;
        metrics.update_readiness(|ready| {
            ready.connected = true;
            ready.runtime_verified = verified;
            ready.registered = true;
            ready.metagraph_fresh = true;
            ready.last_weight_succeeded = true;
        });
        metrics.gauge("rtao_spec_version", f64::from(spec_version));
        let executor = validator_executor(client.clone(), config.netuid);
        Ok(NeuronCx {
            client: client.clone(),
            executor,
            netuid: config.netuid,
            hotkey,
            metagraph,
            epochs,
            http,
            shutdown,
            metrics,
            state_dir,
        })
    }
    .await;
    if initialized.is_err() {
        client.close().await;
    }
    initialized
}
async fn wait_registered(
    client: &Client,
    netuid: NetUid,
    hotkey: AccountId,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let operation = async {
        let mut heads = client.blocks(Finality::Finalized)?;
        loop {
            let snapshot = client.at(BlockRef::Finalized).await?;
            if snapshot
                .storage(
                    &rtao_runtime::storage::subtensor_module::UIDS,
                    (netuid, hotkey),
                )
                .await?
                .is_some()
            {
                return Ok(());
            }
            let next = tokio::select! {biased;()=shutdown.cancelled()=>return Err(anyhow::anyhow!("registration wait cancelled")),next=heads.next()=>next};
            let header =
                next.ok_or_else(|| anyhow::anyhow!("finalized registration stream ended"))??;
            let snapshot = client.at(BlockRef::Hash(header.hash)).await?;
            // The state check also recovers a registration event lost during reconnect.
            if let Ok(events) = snapshot.events().await
                && events.records().iter().any(|event| {
                    event.module_id == "SubtensorModule" && event.event_id == "NeuronRegistered"
                })
            {
                tracing::debug!(
                    block = header.number,
                    "registration event observed while waiting for this hotkey"
                );
            }
        }
    };
    tokio::select! {biased;()=shutdown.cancelled()=>Err(anyhow::anyhow!("registration wait cancelled")),result=operation=>result}
}
pub(crate) fn validator_executor(client: Client, netuid: NetUid) -> Executor {
    rtao_intents::executor(client).with_policy(validator_policy(netuid))
}
pub(crate) fn validator_policy(netuid: NetUid) -> rtao_tx::Policy {
    rtao_tx::Policy {
        max_spend: Some(rtao_primitives::Balance::<rtao_primitives::Tao>::from_rao(
            0,
        )),
        allowed_netuids: Some(std::collections::BTreeSet::from([netuid])),
        allow_raw_calls: false,
        ..Default::default()
    }
}
#[cfg(feature = "validator")]
struct ExecutorClock(Arc<dyn crate::Clock>);
#[cfg(feature = "validator")]
impl rtao_tx::Clock for ExecutorClock {
    fn now(&self) -> std::time::SystemTime {
        let duration = Duration::try_from_secs_f64(self.0.now_unix()).unwrap_or_default();
        std::time::UNIX_EPOCH
            .checked_add(duration)
            .unwrap_or(std::time::UNIX_EPOCH)
    }
}
async fn epoch_at(
    client: &Client,
    netuid: NetUid,
    block: BlockRef,
    kind: &str,
) -> anyhow::Result<(EpochStatus, bool, u32)> {
    use rtao_runtime::storage::subtensor_module::{LAST_EPOCH_BLOCK, SUBNET_EPOCH_INDEX, TEMPO};
    let snapshot = client.at(block).await?;
    let (index, last_epoch_block, tempo) = snapshot
        .fetch((
            SUBNET_EPOCH_INDEX.at((netuid,)),
            LAST_EPOCH_BLOCK.at((netuid,)),
            TEMPO.at((netuid,)),
        ))
        .await?;
    let report = snapshot.block().runtime.surface_report();
    let verified = if kind == "miner" {
        report.shape_matches("call:SubtensorModule.serve_axon")
            && report.shape_matches("call:SubtensorModule.serve_axon_tls")
    } else {
        report.shape_matches("call:SubtensorModule.commit_timelocked_mechanism_weights")
            && report.shape_matches("call:SubtensorModule.set_mechanism_weights")
    };
    Ok((
        EpochStatus {
            block: snapshot.block().number,
            index,
            last_epoch_block,
            tempo,
        },
        verified,
        snapshot.block().runtime.spec_version(),
    ))
}
pub(crate) async fn finalized_worker(
    cx: NeuronCx,
    heads: watch::Sender<u64>,
    kind: &'static str,
) -> anyhow::Result<()> {
    let mut stream = cx.client.blocks(Finality::Finalized)?;
    loop {
        tokio::select! {
            biased;
            () = cx.shutdown.cancelled() => return Ok(()),
            header = stream.next() => {
                let header = header.ok_or_else(|| anyhow::anyhow!("finalized head stream ended"))??;
                let (status, verified, spec_version) = match epoch_at(&cx.client, cx.netuid, BlockRef::Hash(header.hash), kind).await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        cx.metrics.increment("rtao_epoch_sync_errors_total");
                        cx.metrics.update_readiness(|ready| ready.connected = false);
                        tracing::warn!(%error, block = header.number, "epoch state refresh failed; retaining the last observation");
                        continue;
                    }
                };
                cx.epochs.observe(status); heads.send_replace(header.number);
                cx.metrics.gauge("rtao_spec_version", f64::from(spec_version));
                cx.metrics.update_readiness(|ready| { ready.connected = true; ready.runtime_verified = verified; });
                cx.metrics.gauge("rtao_chain_head{kind=\"finalized\"}", header.number as f64);
            }
        }
    }
}
#[cfg(feature = "validator")]
async fn graph_worker<V: Validator>(
    validator: Arc<V>,
    state: StateCell<V::State>,
    cx: NeuronCx,
) -> anyhow::Result<()> {
    let mut diffs = cx.metagraph.subscribe();
    let mut health = cx.metagraph.health();
    let mut epochs = cx.epochs.subscribe();
    loop {
        tokio::select! {
            biased;
            () = cx.shutdown.cancelled() => return Ok(()),
            diff = diffs.recv() => match diff {
                Ok(diff) => validator.on_metagraph(&diff, &mut state.write()),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return Err(anyhow::anyhow!("metagraph diff watch lagged; cannot safely retain UID scores")),
                Err(error) => return Err(error.into()),
            },
            change = health.changed() => { change?; },
            change = epochs.changed() => { change?; },
        }
        let graph = cx.metagraph.load();
        let age = cx.epochs.status().block.saturating_sub(graph.at.number);
        let registered = graph.by_hotkey(&cx.hotkey.hotkey_address()?).is_some();
        let fresh =
            !health.borrow().unhealthy && age <= graph.header.tempo.clamp(1, 100).saturating_mul(2);
        cx.metrics.gauge("rtao_metagraph_age_blocks", age as f64);
        cx.metrics.update_readiness(|ready| {
            ready.registered = registered;
            ready.metagraph_fresh = fresh;
        });
    }
}
#[cfg(feature = "validator")]
async fn clock_worker(
    cx: NeuronCx,
    guard: ClockGuard,
    clock: Arc<dyn crate::Clock>,
) -> anyhow::Result<()> {
    use crate::Clock;
    let mut best = cx
        .client
        .heads()
        .ok_or_else(|| anyhow::anyhow!("best-head tracking required"))?
        .best();
    loop {
        let header = best.borrow_and_update().clone();
        let timestamp = async {
            cx.client
                .at(BlockRef::Hash(header.hash))
                .await?
                .storage(&rtao_runtime::storage::timestamp::NOW, ())
                .await
        }
        .await;
        let timestamp = match timestamp {
            Ok(timestamp) => Some(timestamp),
            Err(error) => {
                cx.metrics.increment("rtao_clock_sync_errors_total");
                tracing::warn!(%error, block = header.number, "chain clock refresh failed; retaining the last anchor");
                None
            }
        };
        if let Some(timestamp) = timestamp {
            let health = guard.observe(SystemClock.now_unix(), timestamp)?;
            clock.observe_timestamp(timestamp);
            cx.metrics
                .gauge("rtao_chain_head{kind=\"best\"}", header.number as f64);
            cx.metrics.gauge(
                "rtao_finality_lag_blocks",
                header.number.saturating_sub(cx.epochs.status().block) as f64,
            );
            cx.metrics
                .gauge("rtao_clock_skew_seconds", health.skew_seconds);
            if health.warning {
                tracing::warn!(
                    skew_seconds = health.skew_seconds,
                    "host clock differs from chain timestamp"
                );
            }
        }
        tokio::select! { biased; () = cx.shutdown.cancelled() => return Ok(()), changed = best.changed() => changed? }
    }
}
pub(crate) async fn save_worker<S>(
    store: Arc<StateStore<S>>,
    state: StateCell<S>,
    cx: NeuronCx,
) -> anyhow::Result<()>
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    let mut timer = tokio::time::interval(Duration::from_secs(300));
    timer.tick().await;
    loop {
        tokio::select! {
            biased;
            () = cx.shutdown.cancelled() => return Ok(()),
            _ = timer.tick() => {
                let start = std::time::Instant::now();
                store.save(&state, cx.epochs.status().block).await?;
                cx.metrics.gauge("rtao_state_save_seconds", start.elapsed().as_secs_f64());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::Network;
    use rtao_primitives::AccountId;
    use rtao_signer::{BoxFuture, DynKeySource, Signer, SignerError, SignerRole};
    use rtao_testkit::rpc::RpcClient;
    use rtao_testkit::{FakeChain, FakeState};
    use rtao_tx::{ExecuteOptions, TxError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct PublicHotkey {
        unlocks: AtomicUsize,
    }
    impl KeyView for PublicHotkey {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(AccountId([7; 32]))
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok([7; 32])
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
        }
    }
    impl DynKeySource for PublicHotkey {
        fn signer_for<'a>(
            &'a self,
            _role: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(SignerError::KeyView("unexpected private-key unlock".into())) })
        }
    }

    #[tokio::test]
    async fn validator_executor_parses_tools_and_enforces_policy_before_unlock()
    -> anyhow::Result<()> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let executor = validator_executor(client.clone(), NetUid(1));
        let keys = PublicHotkey::default();
        let result = executor
            .execute_tool(
                "reset_axon",
                serde_json::json!({"netuid": 2}),
                &keys,
                &ExecuteOptions::default(),
            )
            .await;
        client.close().await;

        let Err(TxError::Policy(error)) = result else {
            anyhow::bail!("expected parsed tool to be refused by validator policy: {result:?}");
        };
        assert_eq!(error.violations, ["netuid 2 not in allowed_netuids [1]"]);
        let policy = executor
            .policy()
            .ok_or_else(|| anyhow::anyhow!("validator executor has no policy"))?;
        assert_eq!(
            policy.max_spend,
            Some(rtao_primitives::Balance::<rtao_primitives::Tao>::from_rao(
                0
            ))
        );
        assert!(!policy.allow_raw_calls);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.requests().iter().all(|request| {
            !matches!(
                request.method.as_str(),
                "author_submitExtrinsic" | "author_submitAndWatchExtrinsic"
            )
        }));
        Ok(())
    }

    #[tokio::test]
    async fn registration_wait_recovers_from_a_finalized_head_without_an_event()
    -> anyhow::Result<()> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let account = AccountId([7; 32]);
        let shutdown = CancellationToken::new();
        let waiting_client = client.clone();
        let waiting_shutdown = shutdown.clone();
        let mut task = tokio::spawn(async move {
            wait_registered(&waiting_client, NetUid(1), account, &waiting_shutdown).await
        });
        tokio::task::yield_now().await;
        let header = chain.mutate_state(|state| -> anyhow::Result<_> {
            state.advance(1);
            state.set_finalized(state.block)?;
            state.seed_typed("SubtensorModule", "Uids", (NetUid(1), account), 0u16)?;
            let hash = state
                .block_hash(state.block)
                .ok_or_else(|| anyhow::anyhow!("new fixture hash"))?;
            state
                .header(&hash)
                .ok_or_else(|| anyhow::anyhow!("new fixture header"))
        })?;
        chain.publish_method("chain_subscribeFinalizedHeads", header)?;
        let result = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
        shutdown.cancel();
        if result.is_err() {
            task.abort();
            let _ = task.await;
        }
        client.close().await;
        result???;
        Ok(())
    }
    #[tokio::test]
    async fn cancelled_registration_wait_never_reads_or_unlocks() -> anyhow::Result<()> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let count = chain.requests().len();
        let result = wait_registered(&client, NetUid(1), AccountId([7; 32]), &shutdown).await;
        assert_eq!(chain.requests().len(), count);
        client.close().await;
        assert!(result.is_err());
        Ok(())
    }
}
