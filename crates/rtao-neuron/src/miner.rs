//! Miner HTTP serving, persistent state and supervised chain services.

#[cfg(test)]
mod tests;

use crate::{
    AxonConfig, AxonPublishOutcome, AxonPublisher, Metrics, NeuronConfig, NeuronCx, NeuronRuntime,
    PruneRisk, StateCell, StateStore,
};
use axum::{Router, extract::DefaultBodyLimit};
use rtao_btauth::{
    InMemoryNonceStore, NonceStore, Verifier, VerifyConfig,
    server::{BtAuthLayer, DEFAULT_BODY_LIMIT, MetagraphPolicy, PerHotkeyLimiter},
};
use rtao_client::ViewExt;
use rtao_metagraph::{MetagraphDiff, SyncPolicy};
use rtao_signer::KeyView;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet};
use tokio_util::sync::CancellationToken;

/// Application routes and persistent state supplied by a subnet miner.
pub trait Miner: Send + Sync + 'static {
    /// Serializable state shared with HTTP handlers and persisted on shutdown.
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    /// Build routes after state recovery and [`Self::on_start`].
    fn router(&self, cx: &NeuronCx, state: StateCell<Self::State>) -> Router;
    /// Default admission requires subnet membership and a validator permit.
    fn caller_policy(&self, cx: &NeuronCx) -> MetagraphPolicy {
        MetagraphPolicy::validators(cx.metagraph.clone())
    }
    /// Initialize resources and state before the HTTP listener accepts requests.
    fn on_start(
        &self,
        _cx: &NeuronCx,
        _state: &StateCell<Self::State>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }
    /// React to registration replacement, deregistration and other graph changes.
    fn on_metagraph(&self, _diff: &MetagraphDiff, _state: &mut Self::State) {}
    /// Application state schema; changed when checkpoints become incompatible.
    fn state_schema_version(&self) -> u32 {
        1
    }
}

/// Miner service configuration; construction performs no wallet or network I/O.
pub struct MinerRuntime<M: Miner> {
    config: NeuronConfig,
    miner: Arc<M>,
    injected: Option<NeuronCx>,
    axon: AxonConfig,
    bind: SocketAddr,
    metrics: Metrics,
    body_limit: usize,
    nonce_store: Arc<dyn NonceStore>,
    verify: VerifyConfig,
    limiter: Arc<PerHotkeyLimiter>,
    listener: Option<tokio::net::TcpListener>,
}
impl NeuronRuntime<()> {
    /// Configure a miner; chain and wallet I/O starts only in [`MinerRuntime::run`].
    pub fn miner<M: Miner>(config: NeuronConfig, miner: M) -> MinerRuntime<M> {
        let axon = AxonConfig::for_neuron(&config);
        let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.axon_port);
        MinerRuntime {
            config,
            miner: Arc::new(miner),
            injected: None,
            axon,
            bind,
            metrics: Metrics::default(),
            body_limit: DEFAULT_BODY_LIMIT,
            nonce_store: Arc::new(InMemoryNonceStore::new()),
            verify: VerifyConfig::default(),
            limiter: Arc::new(PerHotkeyLimiter::default()),
            listener: None,
        }
    }
}
impl<M: Miner> MinerRuntime<M> {
    /// Adopt explicitly connected services; the miner policy is applied in `run`.
    pub fn with_context(mut self, cx: NeuronCx) -> Self {
        self.injected = Some(cx);
        self
    }
    /// Share the application's explicitly installed metrics recorder.
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = metrics;
        self
    }
    /// Configure endpoint advertisement and optional certificate rotation.
    pub fn with_axon(mut self, axon: AxonConfig) -> Self {
        self.axon = axon;
        self
    }
    /// Select the local socket address; the advertised endpoint remains separate.
    pub fn with_bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }
    /// Adopt an already bound listener, useful for embedding and socket activation.
    pub fn with_listener(mut self, listener: tokio::net::TcpListener) -> Self {
        self.listener = Some(listener);
        self
    }
    /// Set the authentication and inner axum body bounds together.
    pub fn with_body_limit(mut self, bytes: usize) -> Self {
        self.body_limit = bytes;
        self
    }
    /// Share an in-memory or distributed store without weakening replay retention.
    pub fn with_nonce_store(mut self, store: Arc<dyn NonceStore>) -> Self {
        self.nonce_store = store;
        self
    }
    /// Configure freshness, receiver binding and wrapped-signature acceptance.
    pub fn with_verification(mut self, verify: VerifyConfig) -> Self {
        self.verify = verify;
        self
    }
    /// Replace the default bounded per-hotkey rate limiter.
    pub fn with_limiter(mut self, limiter: Arc<PerHotkeyLimiter>) -> Self {
        self.limiter = limiter;
        self
    }

    /// Serve authenticated routes until cancellation; persist after graceful shutdown.
    pub async fn run(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.body_limit > 0, "miner body limit must be nonzero");
        self.axon.validate()?;
        let mut publisher = AxonPublisher::new(self.axon)?;
        let mut cx = match self.injected {
            Some(cx) => cx,
            None => {
                crate::runtime::initialize(&self.config, self.metrics, SyncPolicy::miner(), "miner")
                    .await?
            }
        };
        let preparation = async {
            anyhow::ensure!(
                cx.netuid == self.config.netuid,
                "injected neuron context belongs to a different subnet"
            );
            cx.executor = cx
                .executor
                .clone()
                .with_policy(crate::runtime::validator_policy(cx.netuid));
            let account = cx.hotkey.hotkey_address()?;
            let schema = self.miner.state_schema_version();
            anyhow::ensure!(schema > 0, "miner state schema version must be positive");
            let store = Arc::new(StateStore::<M::State>::new(
                cx.state_dir.clone(),
                cx.netuid,
                account,
                schema,
            ));
            let state = StateCell::new(match store.load().await? {
                Some(loaded) => loaded.state,
                None => M::State::default(),
            });
            self.miner.on_start(&cx, &state).await?;
            let verifier: Arc<Verifier<dyn NonceStore>> =
                Arc::new(Verifier::new(account, self.verify, self.nonce_store)?);
            let router = self
                .miner
                .router(&cx, state.clone())
                .layer(DefaultBodyLimit::max(self.body_limit))
                .layer(
                    BtAuthLayer::new(verifier)
                        .with_policy(Arc::new(self.miner.caller_policy(&cx)))
                        .with_limiter(self.limiter)
                        .with_body_limit(self.body_limit),
                );
            let listener = match self.listener {
                Some(listener) => listener,
                None => tokio::net::TcpListener::bind(self.bind).await?,
            };
            tracing::info!(address=%listener.local_addr()?,"miner HTTP listener ready");
            let telemetry = if let Some(addr) = self.config.metrics_addr {
                Some(cx.metrics.bind(addr).await?)
            } else {
                None
            };
            Ok::<_, anyhow::Error>((store, state, router, listener, telemetry))
        }
        .await;
        let (store, state, router, listener, telemetry) = match preparation {
            Ok(prepared) => prepared,
            Err(error) => {
                cx.close().await;
                return Err(error);
            }
        };
        let signals = crate::shutdown::spawn_signals(cx.shutdown.clone());
        let mut tasks = JoinSet::<anyhow::Result<()>>::new();
        let http_cancel = cx.shutdown.clone();
        tasks.spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(http_cancel.cancelled_owned())
                .await?;
            Ok(())
        });
        let (heads, _finalized) = watch::channel(cx.epochs.status().block);
        let finalized_cx = cx.clone();
        tasks.spawn(
            async move { crate::runtime::finalized_worker(finalized_cx, heads, "miner").await },
        );
        let graph_cx = cx.clone();
        let miner = self.miner.clone();
        let graph_state = state.clone();
        tasks.spawn(async move { graph_worker(miner, graph_state, graph_cx).await });
        let save_cx = cx.clone();
        let save_state = state.clone();
        let periodic_store = store.clone();
        tasks.spawn(async move {
            crate::runtime::save_worker(periodic_store, save_state, save_cx).await
        });
        let publisher_cx = cx.clone();
        tasks.spawn(async move { publisher_worker(&mut publisher, publisher_cx).await });
        let prune_cx = cx.clone();
        tasks.spawn(async move { prune_worker(prune_cx).await });
        let telemetry_cancel = CancellationToken::new();
        if let Some(server) = telemetry {
            let cancel = telemetry_cancel.clone();
            tasks.spawn(async move {
                server.run(cancel).await?;
                Ok(())
            });
        }
        let result = tokio::select! {biased;()=cx.shutdown.cancelled()=>Ok(()),finished=tasks.join_next()=>match finished {
            Some(Ok(Ok(()))) if cx.shutdown.is_cancelled()=>Ok(()),Some(Ok(Ok(())))=>Err(anyhow::anyhow!("miner framework service stopped unexpectedly")),
            Some(Ok(Err(error)))=>Err(error),Some(Err(error))=>Err(error.into()),None=>Err(anyhow::anyhow!("miner framework services all stopped"))
        }};
        cx.shutdown.cancel();
        telemetry_cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(30), async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        let save = store.save(&state, cx.epochs.status().block).await;
        cx.close().await;
        signals.abort();
        let _ = signals.await;
        save?;
        result
    }
}
async fn graph_worker<M: Miner>(
    miner: Arc<M>,
    state: StateCell<M::State>,
    cx: NeuronCx,
) -> anyhow::Result<()> {
    let mut diffs = cx.metagraph.subscribe();
    let mut health = cx.metagraph.health();
    let mut epochs = cx.epochs.subscribe();
    let block_time = cx
        .client
        .at(rtao_client::BlockRef::Finalized)
        .await?
        .block_time()
        .await?;
    anyhow::ensure!(!block_time.is_zero(), "chain block time must be nonzero");
    let fresh_limit = (Duration::from_secs(1200).as_nanos() / block_time.as_nanos())
        .min(u128::from(u64::MAX)) as u64;
    loop {
        tokio::select! {biased;()=cx.shutdown.cancelled()=>return Ok(()),diff=diffs.recv()=>match diff {
            Ok(diff)=>miner.on_metagraph(&diff,&mut state.write()),Err(error)=>return Err(error.into())
        },change=health.changed()=>{change?;},change=epochs.changed()=>{change?;}}
        let graph = cx.metagraph.load();
        let age = cx.epochs.status().block.saturating_sub(graph.at.number);
        let registered = graph.by_hotkey(&cx.hotkey.hotkey_address()?).is_some();
        cx.metrics.gauge("rtao_metagraph_age_blocks", age as f64);
        cx.metrics.update_readiness(|ready| {
            ready.registered = registered;
            ready.metagraph_fresh =
                !health.borrow().unhealthy && !health.borrow().stopped && age <= fresh_limit;
        });
    }
}
async fn publisher_worker(publisher: &mut AxonPublisher, cx: NeuronCx) -> anyhow::Result<()> {
    let mut epochs = cx.epochs.subscribe();
    let mut timer = tokio::time::interval(publisher.config().recheck);
    timer.tick().await;
    let mut due = None;
    loop {
        match publisher.poll(&cx.executor, &cx.hotkey, cx.netuid).await {
            Ok(AxonPublishOutcome::Scheduled { retry_at }) => {
                due = Some(retry_at);
                cx.metrics
                    .gauge("rtao_axon_retry_at_block", retry_at as f64);
            }
            Ok(AxonPublishOutcome::Published { .. }) => {
                due = None;
                cx.metrics.increment("rtao_axon_published_total");
            }
            Ok(AxonPublishOutcome::Unchanged) => {
                due = None;
            }
            Ok(AxonPublishOutcome::Unresolved { extrinsic_hash }) => {
                tracing::warn!(hash=?extrinsic_hash,"axon publication requires reconciliation");
            }
            Err(error) => {
                cx.metrics.increment("rtao_axon_errors_total");
                tracing::warn!(%error,"axon publication will retry");
            }
        }
        loop {
            tokio::select! {biased;()=cx.shutdown.cancelled()=>{
                match publisher.reset(&cx.executor,&cx.hotkey,cx.netuid).await {
                    Ok(Some(AxonPublishOutcome::Scheduled {retry_at}))=>tracing::warn!(retry_at,"axon reset remains rate limited at shutdown"),
                    Ok(Some(AxonPublishOutcome::Unresolved {extrinsic_hash}))=>tracing::warn!(hash=?extrinsic_hash,"axon reset has unresolved fate at shutdown"),
                    Err(error)=>{cx.metrics.increment("rtao_axon_errors_total");tracing::warn!(%error,"axon reset failed during shutdown");return Err(error.into());},
                    _=>{}
                }
                return Ok(());
            },_=timer.tick()=>break,change=epochs.changed()=>{change?;if due.is_some_and(|block|epochs.borrow().block>=block){break;}}}
        }
    }
}
async fn prune_worker(cx: NeuronCx) -> anyhow::Result<()> {
    let hotkey = cx.hotkey.hotkey_address()?;
    let mut epochs = cx.epochs.subscribe();
    loop {
        match PruneRisk::from_client(&cx.client, cx.netuid, hotkey).await {
            Ok(risk) => {
                risk.publish_metrics(&cx.metrics);
            }
            Err(error) => {
                cx.metrics.increment("rtao_prune_monitor_errors_total");
                tracing::warn!(%error,"pruning risk refresh failed");
            }
        }
        tokio::select! {biased;()=cx.shutdown.cancelled()=>return Ok(()),change=epochs.changed()=>{change?;}}
    }
}
