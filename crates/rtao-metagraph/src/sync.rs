//! Finalized, epoch-driven graph refreshes with atomic last-good snapshots.

use crate::{Metagraph, MetagraphOptions, SubnetsMetagraphExt};
use arc_swap::ArcSwap;
use futures::StreamExt;
use rtao_client::{BlockRef, Client, ClientError, EventRecord, Finality, Snapshot, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, MechId, NetUid, Uid};
use rtao_reads::{Namespaces, ReadError};
use rtao_runtime::{SelectiveMetagraphIndex as Field, storage::subtensor_module::LAST_EPOCH_BLOCK};
use std::{
    collections::BTreeSet,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// Fields to fetch for each synchronized snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preset {
    /// All runtime fields, with Python's full graph projection.
    Full,
    /// Scoring, endpoint and weight-scheduling fields needed by validators.
    Validator,
    /// Membership, permits and stake needed by miners.
    Miner,
    /// An explicit field selection. `Hotkeys` is required for synchronization.
    Custom(Vec<Field>),
}
impl Preset {
    /// Return the requested fields, or `None` for a full graph.
    pub fn fields(&self) -> Option<Vec<Field>> {
        match self {
            Self::Full => None,
            Self::Custom(fields) => Some(fields.clone()),
            Self::Miner => Some(vec![
                Field::Hotkeys,
                Field::ValidatorPermit,
                Field::TotalStake,
                Field::Validators,
            ]),
            Self::Validator => Some(vec![
                Field::Block,
                Field::Tempo,
                Field::LastStep,
                Field::BlocksSinceLastStep,
                Field::WeightsVersion,
                Field::WeightsRateLimit,
                Field::ActivityCutoff,
                Field::CommitRevealWeightsEnabled,
                Field::CommitRevealPeriod,
                Field::NumUids,
                Field::Hotkeys,
                Field::Axons,
                Field::Active,
                Field::ValidatorPermit,
                Field::LastUpdate,
                Field::Emission,
                Field::Dividends,
                Field::Incentives,
                Field::BlockAtRegistration,
                Field::TotalStake,
            ]),
        }
    }
}
/// When the last-good graph should be refreshed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncCadence {
    /// Refresh when `LastEpochBlock` advances, with a block-count fallback.
    EveryEpoch {
        /// Maximum finalized blocks between successful refreshes.
        fallback_blocks: u32,
    },
    /// Refresh after this many finalized blocks since the last success.
    EveryBlocks(u32),
    /// Refresh at an elapsed-time interval, using the latest finalized block.
    Interval(Duration),
}
/// Synchronization options for one subnet mechanism.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncPolicy {
    /// Refresh schedule, evaluated against finalized state.
    pub cadence: SyncCadence,
    /// Runtime fields fetched for every snapshot.
    pub preset: Preset,
    /// Include registered and unregistered commitments at the same block.
    pub commitments: bool,
    /// Coalesce relevant finalized events into a refresh after one second.
    pub early_on_events: bool,
    /// Consecutive failures before publishing an unhealthy status.
    pub reconnect_after_failures: u32,
}
impl Default for SyncPolicy {
    fn default() -> Self {
        Self::validator()
    }
}
impl SyncPolicy {
    /// Full epoch snapshots with commitments, for owner monitoring.
    pub fn full() -> Self {
        Self {
            preset: Preset::Full,
            commitments: true,
            ..Self::validator()
        }
    }
    /// Validator preset: each epoch, with a 100-block fallback and no commitments.
    pub fn validator() -> Self {
        Self {
            cadence: SyncCadence::EveryEpoch {
                fallback_blocks: 100,
            },
            preset: Preset::Validator,
            commitments: false,
            early_on_events: false,
            reconnect_after_failures: 3,
        }
    }
    /// Miner preset: every ten minutes and after relevant finalized events.
    pub fn miner() -> Self {
        Self {
            cadence: SyncCadence::Interval(Duration::from_secs(600)),
            preset: Preset::Miner,
            early_on_events: true,
            ..Self::validator()
        }
    }
    fn validate(&self) -> std::result::Result<(), SyncError> {
        match self.cadence {
            SyncCadence::EveryEpoch { fallback_blocks: 0 } | SyncCadence::EveryBlocks(0) => {
                return Err(SyncError::InvalidPolicy(
                    "block cadence must be positive".into(),
                ));
            }
            SyncCadence::Interval(duration) if duration < Duration::from_secs(30) => {
                return Err(SyncError::InvalidPolicy(
                    "interval must be at least 30 seconds".into(),
                ));
            }
            _ => {}
        }
        if self.reconnect_after_failures == 0 {
            return Err(SyncError::InvalidPolicy(
                "failure threshold must be positive".into(),
            ));
        }
        if let Some(fields) = self.preset.fields() {
            if !fields.contains(&Field::Hotkeys) {
                return Err(SyncError::InvalidPolicy(
                    "selective sync requires Hotkeys".into(),
                ));
            }
            if fields.iter().collect::<BTreeSet<_>>().len() != fields.len() {
                return Err(SyncError::InvalidPolicy(
                    "selective fields must not repeat".into(),
                ));
            }
        }
        Ok(())
    }
}
/// Membership and endpoint changes between two successful graph snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetagraphDiff {
    /// Previous successful snapshot's finalized block height.
    pub from_block: u64,
    /// New successful snapshot's finalized block height.
    pub to_block: u64,
    /// Newly populated UIDs and their hotkeys.
    pub registered: Vec<(Uid, AccountId)>,
    /// Reused UIDs, retaining both hotkeys for score resets.
    pub replaced: Vec<(Uid, AccountId, AccountId)>,
    /// Removed UIDs and their former hotkeys.
    pub deregistered: Vec<(Uid, AccountId)>,
    /// UIDs whose validator permit changed from false to true.
    pub permits_gained: Vec<Uid>,
    /// UIDs whose validator permit changed from true to false.
    pub permits_lost: Vec<Uid>,
    /// UIDs whose complete axon value changed, including IP and port.
    pub axons_changed: Vec<Uid>,
}
impl MetagraphDiff {
    /// Compare complete UID identities without copying the graph's raw response.
    pub fn between(old: &Metagraph, new: &Metagraph) -> Self {
        let mut diff = Self {
            from_block: old.at.number,
            to_block: new.at.number,
            ..Self::default()
        };
        for index in 0..old.len().max(new.len()) {
            let Ok(index) = u16::try_from(index) else {
                break;
            };
            let uid = Uid(index);
            match (old.hotkey(uid), new.hotkey(uid)) {
                (None, Some(new)) => diff.registered.push((uid, *new)),
                (Some(old), None) => diff.deregistered.push((uid, *old)),
                (Some(old), Some(new)) if old != new => diff.replaced.push((uid, *old, *new)),
                _ => {}
            }
            let old = old.neuron(uid);
            let new = new.neuron(uid);
            match (
                old.as_ref().is_some_and(|neuron| neuron.validator_permit()),
                new.as_ref().is_some_and(|neuron| neuron.validator_permit()),
            ) {
                (false, true) => diff.permits_gained.push(uid),
                (true, false) => diff.permits_lost.push(uid),
                _ => {}
            }
            if old.map(|neuron| neuron.axon_value()) != new.map(|neuron| neuron.axon_value()) {
                diff.axons_changed.push(uid);
            }
        }
        diff
    }
    /// UIDs whose stored score belongs to a replaced or deregistered hotkey.
    pub fn score_resets(&self) -> impl Iterator<Item = Uid> + '_ {
        self.replaced
            .iter()
            .map(|(uid, _, _)| *uid)
            .chain(self.deregistered.iter().map(|(uid, _)| *uid))
    }
}
/// Retained synchronization health, available before a receiver subscribes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncHealth {
    /// Finalized height of the last published, successful graph.
    pub last_success: u64,
    /// Failed attempts since the last success, including epoch-clock reads.
    pub consecutive_failures: u32,
    /// Whether the configured failure threshold has been reached.
    pub unhealthy: bool,
    /// Most recent failure; cleared after a successful refresh.
    pub last_error: Option<String>,
    /// Whether cancellation or a terminal stream failure stopped the worker.
    pub stopped: bool,
}
/// Startup or policy errors. Refresh failures appear in [`SyncHealth`].
#[derive(Debug)]
pub enum SyncError {
    /// Invalid cadence, selection or identifier.
    InvalidPolicy(String),
    /// Client snapshot, epoch-clock or finalized-stream failure.
    Client(ClientError),
    /// Runtime metagraph or commitment fetch failure.
    Read(ReadError),
    /// No nonempty, correctly identified graph was available.
    Snapshot(String),
    /// Cancellation won before a startup snapshot was ready.
    Cancelled,
}
impl fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(message) | Self::Snapshot(message) => formatter.write_str(message),
            Self::Client(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Cancelled => formatter.write_str("metagraph synchronization cancelled"),
        }
    }
}
impl std::error::Error for SyncError {}
impl From<ClientError> for SyncError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<ReadError> for SyncError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

struct Lifetime {
    cancel: CancellationToken,
    stopped: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self
            .task
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            task.abort();
        }
    }
}
/// Lock-free access to the last-good graph and the worker's lifecycle.
/// Clones share one refresh worker; dropping the last handle cancels it.
#[derive(Clone)]
pub struct MetagraphHandle {
    graph: Arc<ArcSwap<Metagraph>>,
    alive: Arc<Lifetime>,
    diffs: broadcast::Sender<MetagraphDiff>,
    health: watch::Receiver<SyncHealth>,
}
impl fmt::Debug for MetagraphHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetagraphHandle")
            .field("at", &self.load().at)
            .field("health", &*self.health.borrow())
            .finish_non_exhaustive()
    }
}
impl MetagraphHandle {
    /// Load one stable shared snapshot without locking or decoding.
    pub fn load(&self) -> Arc<Metagraph> {
        self.graph.load_full()
    }
    /// Subscribe to diffs emitted after the initial snapshot.
    /// On a lag error, reload the current graph before using per-UID scores.
    pub fn subscribe(&self) -> broadcast::Receiver<MetagraphDiff> {
        self.diffs.subscribe()
    }
    /// Watch retained health, including the configured failure threshold.
    pub fn health(&self) -> watch::Receiver<SyncHealth> {
        self.health.clone()
    }
    /// Cancel the worker and wait until its owned client and pending reads drop.
    /// Existing loads continue to serve the last-good graph.
    pub async fn shutdown(&self) {
        self.alive.cancel.cancel();
        let task = self
            .alive
            .task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.alive.stopped.cancelled().await;
    }
}
/// Factory for an epoch-driven, validator-ready graph synchronization worker.
pub struct MetagraphSync;
impl MetagraphSync {
    /// Fetch a nonempty finalized snapshot before returning a live handle.
    /// The worker publishes complete replacements only after a successful read.
    pub async fn spawn(
        client: Client,
        netuid: NetUid,
        mechid: MechId,
        policy: SyncPolicy,
        cancel: CancellationToken,
    ) -> std::result::Result<(MetagraphHandle, broadcast::Receiver<MetagraphDiff>), SyncError> {
        policy.validate()?;
        NetUid::new(netuid.0).map_err(|error| SyncError::InvalidPolicy(error.to_string()))?;
        MechId::new(mechid.0).map_err(|error| SyncError::InvalidPolicy(error.to_string()))?;
        let blocks = client.blocks(Finality::Finalized)?;
        let cancel = cancel.child_token();
        let (graph, epoch) = tokio::select! {
            biased;
            ()=cancel.cancelled()=>return Err(SyncError::Cancelled),
            initial=async {
                let snapshot=client.at(BlockRef::Finalized).await?;
                let epoch=epoch(&snapshot,&policy,netuid).await;
                Ok::<_,SyncError>((fetch(&snapshot,netuid,mechid,&policy).await?,epoch))
            }=>initial?,
        };
        let initial_error = epoch.as_ref().err().map(ToString::to_string);
        let epoch = epoch.ok().flatten();
        let initial_failures = u32::from(initial_error.is_some());
        let health = SyncHealth {
            last_success: graph.at.number,
            consecutive_failures: initial_failures,
            unhealthy: initial_failures >= policy.reconnect_after_failures,
            last_error: initial_error,
            stopped: false,
        };
        let graph = Arc::new(ArcSwap::from_pointee(graph));
        let (diffs, receiver) = broadcast::channel(64);
        let (health_tx, health_rx) = watch::channel(health);
        let stopped = CancellationToken::new();
        let worker = Worker {
            client,
            netuid,
            mechid,
            policy,
            graph: graph.clone(),
            diffs: diffs.clone(),
            health: health_tx.clone(),
            cancel: cancel.clone(),
            _exit: StopOnDrop {
                stopped: stopped.clone(),
                health: health_tx,
            },
            epoch,
        };
        let task = tokio::spawn(worker.run(blocks));
        let alive = Arc::new(Lifetime {
            cancel,
            stopped,
            task: Mutex::new(Some(task)),
        });
        Ok((
            MetagraphHandle {
                graph,
                alive,
                diffs,
                health: health_rx,
            },
            receiver,
        ))
    }
}
async fn epoch(
    snapshot: &Snapshot,
    policy: &SyncPolicy,
    netuid: NetUid,
) -> std::result::Result<Option<u64>, SyncError> {
    if matches!(policy.cadence, SyncCadence::EveryEpoch { .. }) {
        Ok(Some(snapshot.storage(&LAST_EPOCH_BLOCK, (netuid,)).await?))
    } else {
        Ok(None)
    }
}
async fn fetch(
    snapshot: &Snapshot,
    netuid: NetUid,
    mechid: MechId,
    policy: &SyncPolicy,
) -> std::result::Result<Metagraph, SyncError> {
    let options = MetagraphOptions {
        commitments: policy.commitments,
        ..MetagraphOptions::default()
    };
    let graph = if let Some(fields) = policy.preset.fields() {
        snapshot
            .subnets()
            .selective_metagraph_with(netuid, mechid, &fields, options)
            .await?
    } else {
        snapshot
            .subnets()
            .mechagraph_with(netuid, mechid, options)
            .await?
    };
    let graph =
        graph.ok_or_else(|| SyncError::Snapshot("subnet metagraph is unavailable".into()))?;
    if graph.is_empty() {
        return Err(SyncError::Snapshot(
            "refusing an empty synchronized metagraph".into(),
        ));
    }
    if graph.header.netuid != netuid
        || graph.header.mechid != mechid
        || graph
            .source_netuid()
            .map_err(|error| SyncError::Snapshot(error.to_string()))?
            != netuid
    {
        return Err(SyncError::Snapshot(
            "metagraph mechanism differs from request".into(),
        ));
    }
    let mut keys = BTreeSet::new();
    for uid in 0..graph.len() {
        let uid = Uid(u16::try_from(uid)
            .map_err(|_| SyncError::Snapshot("graph exceeds UID range".into()))?);
        let key = graph
            .hotkey(uid)
            .ok_or_else(|| SyncError::Snapshot("graph hotkey is absent".into()))?;
        if !keys.insert(*key) {
            return Err(SyncError::Snapshot(
                "graph repeats a registered hotkey".into(),
            ));
        }
        let _ = graph.by_hotkey(key);
    }
    Ok(graph)
}
struct Worker {
    client: Client,
    netuid: NetUid,
    mechid: MechId,
    policy: SyncPolicy,
    graph: Arc<ArcSwap<Metagraph>>,
    diffs: broadcast::Sender<MetagraphDiff>,
    health: watch::Sender<SyncHealth>,
    cancel: CancellationToken,
    _exit: StopOnDrop,
    epoch: Option<u64>,
}
struct StopOnDrop {
    stopped: CancellationToken,
    health: watch::Sender<SyncHealth>,
}
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.health.send_modify(|health| health.stopped = true);
        self.stopped.cancel();
    }
}
// A fixed window coalesces bursts without postponing a refresh indefinitely.
const EVENT_DEBOUNCE: Duration = Duration::from_secs(1);
const EVENT_RETRY_DELAY: Duration = Duration::from_secs(30);

fn relevant_event(event: &EventRecord, netuid: NetUid, graph: &Metagraph) -> bool {
    if event.module_id != "SubtensorModule" {
        return false;
    }
    let subnet_matches =
        |value: Option<&Value>| value.and_then(ValueExt::as_u128) == Some(u128::from(netuid.0));
    match event.event_id.as_str() {
        "NeuronRegistered" | "AxonServed" => subnet_matches(
            event
                .attributes
                .field("netuid")
                .or_else(|| event.attributes.tuple_item(0)),
        ),
        name if name.starts_with("HotkeySwapped") => {
            if let Some(subnet) = event.attributes.field("netuid") {
                return subnet_matches(Some(subnet));
            }
            ["old_hotkey", "new_hotkey"].iter().any(|name| {
                let Some(value) = event.attributes.field(name) else {
                    return false;
                };
                let account = value
                    .as_str()
                    .and_then(|value| value.parse::<AccountId>().ok())
                    .or_else(|| {
                        value
                            .value_bytes()
                            .and_then(|bytes| bytes.try_into().ok())
                            .map(AccountId)
                    });
                account.is_some_and(|account| graph.by_hotkey(&account).is_some())
            })
        }
        _ => false,
    }
}

enum Trigger {
    Block(BlockRef),
    Interval,
    Event,
}
impl Worker {
    fn failure(&self, error: impl fmt::Display) {
        self.health.send_modify(|health| {
            health.consecutive_failures = health.consecutive_failures.saturating_add(1);
            health.unhealthy = health.consecutive_failures >= self.policy.reconnect_after_failures;
            health.last_error = Some(error.to_string());
        });
    }
    async fn run(mut self, mut blocks: rtao_client::head::BlockStream) {
        let duration = match self.policy.cadence {
            SyncCadence::Interval(duration) => duration,
            _ => Duration::from_secs(30),
        };
        let timer = tokio::time::sleep(duration);
        tokio::pin!(timer);
        let event_timer = tokio::time::sleep(EVENT_DEBOUNCE);
        tokio::pin!(event_timer);
        let mut event_pending = false;
        let mut last_event_block = self.graph.load().at.number;
        loop {
            let trigger = tokio::select! {
                biased;
                ()=self.cancel.cancelled()=>break,
                ()=self.client.closed()=>break,
                ()=&mut timer,if matches!(self.policy.cadence,SyncCadence::Interval(_))=>Some(Trigger::Interval),
                ()=&mut event_timer,if event_pending=>Some(Trigger::Event),
                next=blocks.next()=>match next {
                    Some(Ok(header))=>Some(Trigger::Block(BlockRef::Hash(header.hash))),
                    Some(Err(error))=>{self.failure(error);None},
                    None=>{self.failure("finalized block stream ended");break;},
                },
            };
            let Some(trigger) = trigger else {
                continue;
            };
            let interval_due = matches!(trigger, Trigger::Interval);
            let event_due = matches!(trigger, Trigger::Event);
            let scan_events = matches!(trigger, Trigger::Block(_)) && self.policy.early_on_events;
            if event_due {
                // Keep the invalidation until publication succeeds, spacing
                // retries so a failed response cannot create a tight loop.
                event_timer
                    .as_mut()
                    .reset(tokio::time::Instant::now() + EVENT_RETRY_DELAY);
            }
            let reference = match trigger {
                Trigger::Block(reference) => reference,
                Trigger::Interval | Trigger::Event => BlockRef::Finalized,
            };
            let refresh = async {
                let snapshot = self.client.at(reference).await?;
                if snapshot.block().number <= self.graph.load().at.number {
                    return Ok(None);
                }
                if scan_events && snapshot.block().number > last_event_block {
                    last_event_block = snapshot.block().number;
                    match snapshot.events().await {
                        Ok(events)
                            if events.records().iter().any(|event| {
                                relevant_event(event, self.netuid, &self.graph.load())
                            }) =>
                        {
                            if !event_pending {
                                event_pending = true;
                                event_timer
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + EVENT_DEBOUNCE);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.failure(error);
                            // The block may contain a membership change we
                            // could not decode. Refresh conservatively.
                            if !event_pending {
                                event_pending = true;
                                event_timer
                                    .as_mut()
                                    .reset(tokio::time::Instant::now() + EVENT_DEBOUNCE);
                            }
                        }
                    }
                }
                let observed = match epoch(&snapshot, &self.policy, self.netuid).await {
                    Ok(epoch) => epoch,
                    Err(error) => {
                        self.failure(error);
                        None
                    }
                };
                let elapsed = snapshot
                    .block()
                    .number
                    .saturating_sub(self.graph.load().at.number);
                let due = match self.policy.cadence {
                    SyncCadence::EveryEpoch { fallback_blocks } => {
                        observed
                            .zip(self.epoch)
                            .is_some_and(|(current, last)| current > last)
                            || (observed.is_some() && self.epoch.is_none())
                            || elapsed >= u64::from(fallback_blocks)
                    }
                    SyncCadence::EveryBlocks(blocks) => elapsed >= u64::from(blocks),
                    SyncCadence::Interval(_) => interval_due,
                };
                if !due && !event_due {
                    return Ok(None);
                }
                let graph = fetch(&snapshot, self.netuid, self.mechid, &self.policy).await?;
                Ok::<_, SyncError>(Some((graph, observed)))
            };
            let result = tokio::select! {biased;()=self.cancel.cancelled()=>break,()=self.client.closed()=>break,result=refresh=>result};
            match result {
                Ok(Some((graph, epoch))) => {
                    event_pending = false;
                    let graph = Arc::new(graph);
                    let previous = self.graph.swap(graph.clone());
                    let diff = MetagraphDiff::between(&previous, &graph);
                    self.epoch = epoch.or(self.epoch);
                    self.health.send_modify(|health| {
                        health.last_success = graph.at.number;
                        health.consecutive_failures = 0;
                        health.unhealthy = false;
                        health.last_error = None;
                    });
                    let _ = self.diffs.send(diff);
                }
                Ok(None) => {
                    if event_due {
                        event_pending = false;
                    }
                }
                Err(error) => {
                    self.failure(error);
                    if scan_events && !event_pending {
                        event_pending = true;
                        event_timer
                            .as_mut()
                            .reset(tokio::time::Instant::now() + EVENT_DEBOUNCE);
                    }
                }
            }
            if interval_due {
                timer.as_mut().reset(tokio::time::Instant::now() + duration);
            }
        }
    }
}
