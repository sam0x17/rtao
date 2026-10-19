//! Production validator and weight-scheduler contracts against a hosted localnet.

use super::{TestResult, localnet};
use futures::StreamExt;
use rtao_client::{BlockHeader, BlockRef, Client, ClientConfig, Finality, Snapshot, View, ViewExt};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_metagraph::{MetagraphSync, SyncPolicy};
use rtao_neuron::{
    ChainClock, Clock, ClockGuard, EpochClock, EpochStatus, Metrics, NeuronConfig, NeuronCx,
    NeuronRuntime, StateCell, Validator, VersionKey, WeightCadence, WeightPolicy, WeightScheduler,
    WeightsInput, WeightsProposal, weights::WeightAttemptOutcome,
};
use rtao_primitives::{AccountId, CryptoType, MechId, NetUid, NetUidStorageIndex, Ratio, Uid};
use rtao_runtime::{calls::admin_utils, storage::subtensor_module as st};
use rtao_signer::Signer;
use rtao_testkit::{BuiltScenario, Localnet, LocalnetProfile, Scenario};
use rtao_wallet::{RegenerateFrom, SecretString, Wallet, WriteOptions};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio_util::sync::CancellationToken;

type Result<T> = rtao_neuron::anyhow::Result<T>;

pub(crate) type Fixture = FrameworkFixture;
pub(crate) trait FixtureMode {
    fn mechanisms(self) -> u8;
}
impl FixtureMode for u8 {
    fn mechanisms(self) -> u8 {
        self
    }
}
impl FixtureMode for &str {
    fn mechanisms(self) -> u8 {
        1
    }
}

/// Services and disposable development keys shared by framework scenarios.
pub(crate) struct FrameworkFixture {
    pub(crate) cx: NeuronCx,
    pub(crate) config: NeuronConfig,
    pub(crate) setup: BuiltScenario,
    pub(crate) netuid: NetUid,
    pub(crate) miner_uid: Uid,
    pub(crate) validator_uid: Uid,
    pub(crate) version: u64,
    pub(crate) clock: Arc<ChainClock>,
    root: PathBuf,
}

impl FrameworkFixture {
    pub(crate) async fn new(ln: &Localnet, mode: impl FixtureMode, cr: bool) -> Result<Self> {
        let mechanisms = mode.mechanisms();
        let setup = Scenario::new(ln)
            .owner()
            .subnet()
            .start()
            .validator()
            .miner()
            .stake(rtao_testkit::VALIDATOR_STAKE)
            .commit_reveal(cr)
            .mechanisms(MechId(mechanisms))
            .build()
            .await?;
        let netuid = setup
            .netuid
            .ok_or_else(|| rtao_neuron::anyhow::anyhow!("scenario has no subnet"))?;
        for call in [
            admin_utils::sudo_set_immunity_period(netuid, 0),
            admin_utils::sudo_set_commit_reveal_weights_interval(netuid, 1),
        ] {
            let report = ln
                .submit_admin(
                    call.compose(&ln.client().runtime().await?)?,
                    &setup.slot.owner,
                )
                .await?;
            rtao_neuron::anyhow::ensure!(
                report.success,
                "framework subnet configuration failed: {report:?}"
            );
            let included = report.block_number.ok_or_else(|| {
                rtao_neuron::anyhow::anyhow!("configuration has no inclusion height")
            })?;
            let mut finalized = ln.client().blocks(Finality::Finalized)?;
            let deadline = Instant::now() + Duration::from_secs(240);
            while ln.client().at(BlockRef::Finalized).await?.block().number < included {
                let _ = next_head(&mut finalized, deadline).await?;
            }
        }
        let task_root = PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT")
                .ok_or_else(|| rtao_neuron::anyhow::anyhow!("RTAO_TEMP_ROOT is required"))?,
        );
        let task_root = task_root.canonicalize()?;
        let root = task_root.join(format!(
            "framework-{}-{}-{}",
            std::process::id(),
            setup.slot.index,
            netuid.0
        ));
        std::fs::create_dir(&root)?;
        // These are deterministic localnet development URIs, never operator wallets.
        std::fs::write(
            root.join("disposable-development-fixture"),
            b"rtao hosted localnet fixture\n",
        )?;
        match Self::prepare(ln, setup, netuid, root.clone()).await {
            Ok(fixture) => {
                if let Err(error) = wait_permit(&fixture).await {
                    fixture.close().await?;
                    return Err(error);
                }
                Ok(fixture)
            }
            Err(error) => {
                remove_development_fixture(&root)?;
                Err(error)
            }
        }
    }

    async fn prepare(
        ln: &Localnet,
        setup: BuiltScenario,
        netuid: NetUid,
        root: PathBuf,
    ) -> Result<Self> {
        let chain_endpoint: url::Url = ln.endpoint().parse()?;
        let wallet_root = root.join("wallets");
        let wallet = Wallet::new("development", "validator", &wallet_root)?;
        let account = wallet
            .regenerate_hotkey(
                RegenerateFrom::Uri(SecretString::new(format!(
                    "//rtao-e2e//slot/{}//hot",
                    setup.slot.index
                ))),
                CryptoType::Sr25519,
                WriteOptions::hotkey(),
            )
            .await?;
        rtao_neuron::anyhow::ensure!(
            account == setup.slot.hotkey.account(),
            "development key differs from registered validator"
        );
        let mut client_config = ClientConfig::for_network(ln.endpoint())?;
        client_config.runtime_cache_dir = None;
        client_config.config_store = None;
        let client = Client::connect(client_config).await?;
        let initialized = async {
            client.ensure_signing().await?;
            let hotkey = wallet.hotkey_only();
            let signer = hotkey.unlock().await?;
            let http = rtao_btauth::client::SignedClient::new(signer)?;
            let snapshot = client.at(BlockRef::Finalized).await?;
            let (validator_uid, miner_uid, version, index, last_epoch_block, tempo) = snapshot
                .fetch((
                    st::UIDS.at((netuid, account)),
                    st::UIDS.at((netuid, setup.slot.miner.account())),
                    st::WEIGHTS_VERSION_KEY.at((netuid,)),
                    st::SUBNET_EPOCH_INDEX.at((netuid,)),
                    st::LAST_EPOCH_BLOCK.at((netuid,)),
                    st::TEMPO.at((netuid,)),
                ))
                .await?;
            let validator_uid = Uid(validator_uid
                .ok_or_else(|| rtao_neuron::anyhow::anyhow!("validator UID missing"))?);
            let miner_uid =
                Uid(miner_uid.ok_or_else(|| rtao_neuron::anyhow::anyhow!("miner UID missing"))?);
            let millis = snapshot
                .timestamp()
                .await?
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_millis();
            let clock = Arc::new(ChainClock::new(u64::try_from(millis)?));
            let shutdown = CancellationToken::new();
            let (metagraph, _) = MetagraphSync::spawn(
                client.clone(),
                netuid,
                MechId(0),
                SyncPolicy::validator(),
                shutdown.clone(),
            )
            .await?;
            let metrics = Metrics::default();
            let surfaces = snapshot.block().runtime.surface_report();
            metrics.update_readiness(|ready| {
                ready.connected = true;
                ready.runtime_verified = surfaces
                    .shape_matches("call:SubtensorModule.commit_timelocked_mechanism_weights")
                    && surfaces.shape_matches("call:SubtensorModule.set_mechanism_weights");
                ready.registered = true;
                ready.metagraph_fresh = true;
                ready.last_weight_succeeded = true;
            });
            let state_dir = root.join("state");
            let cx = NeuronCx {
                executor: rtao_tx::Executor::new(client.clone()),
                client: client.clone(),
                netuid,
                hotkey,
                metagraph,
                epochs: EpochClock::new(EpochStatus {
                    block: snapshot.block().number,
                    index,
                    last_epoch_block,
                    tempo,
                }),
                http,
                shutdown,
                metrics,
                state_dir: state_dir.clone(),
            };
            let config = NeuronConfig {
                network: "local".into(),
                chain_endpoint: Some(chain_endpoint),
                wallet: "development".into(),
                hotkey: "validator".into(),
                wallet_path: Some(wallet_root),
                netuid,
                fallback_endpoints: Some(vec!["none".into()]),
                archive_endpoints: Some(vec!["none".into()]),
                submit_endpoint: None,
                retry_forever: false,
                wait_for_registration: false,
                axon_port: 0,
                axon_external_ip: None,
                axon_allow_private: false,
                state_dir: Some(state_dir),
                metrics_addr: None,
                log: None,
            };
            Ok(Self {
                cx,
                config,
                setup,
                netuid,
                miner_uid,
                validator_uid,
                version,
                clock,
                root,
            })
        }
        .await;
        if initialized.is_err() {
            client.close().await;
        }
        initialized
    }

    pub(crate) fn proposal(&self) -> WeightsProposal {
        proposal(self.miner_uid)
    }

    pub(crate) fn scheduler(&self) -> Result<WeightScheduler> {
        WeightScheduler::new(
            policy(self.version),
            self.clock.clone(),
            ClockGuard::default(),
        )
    }

    /// Refresh the production chain clock before manually driving the scheduler.
    pub(crate) async fn observe_clock(&self) -> Result<()> {
        let snapshot = self.cx.client.at(BlockRef::Best).await?;
        let millis = snapshot
            .timestamp()
            .await?
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_millis();
        self.clock.observe_timestamp(u64::try_from(millis)?);
        Ok(())
    }

    /// Call after joining every runtime using this context.
    pub(crate) async fn cleanup(self) -> Result<()> {
        self.close().await
    }

    pub(crate) async fn close(&self) -> Result<()> {
        self.cx.close().await;
        // Preserve bounded public observations outside the disposable wallet/state root.
        let evidence = self.root.with_extension("metrics.txt");
        std::fs::write(evidence, self.cx.metrics.render())?;
        remove_development_fixture(&self.root)
    }
}

pub(crate) fn proposal(miner: Uid) -> WeightsProposal {
    WeightsProposal::Set(WeightsInput::ByUid(BTreeMap::from([(miner, 1.0)])))
}

pub(crate) async fn scheduler(cx: &NeuronCx) -> Result<WeightScheduler> {
    let version = cx
        .client
        .storage(&st::WEIGHTS_VERSION_KEY, (cx.netuid,))
        .await?;
    let clock = Arc::new(rtao_neuron::SystemClock);
    let guard = ClockGuard::default();
    let millis = cx
        .client
        .at(BlockRef::Best)
        .await?
        .timestamp()
        .await?
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis();
    guard.observe(clock.now_unix(), u64::try_from(millis)?)?;
    WeightScheduler::new(policy(version), clock, guard)
}

fn remove_development_fixture(root: &Path) -> Result<()> {
    rtao_neuron::anyhow::ensure!(
        root.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("framework-")),
        "unexpected fixture path"
    );
    rtao_neuron::anyhow::ensure!(
        root.join("disposable-development-fixture").is_file(),
        "fixture marker missing"
    );
    fn inspect(path: &Path) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        rtao_neuron::anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "fixture contains a live symlink: {}",
            path.display()
        );
        if metadata.is_dir() {
            for child in std::fs::read_dir(path)? {
                inspect(&child?.path())?;
            }
        }
        Ok(())
    }
    // The fixture starts no child process, launchd unit, or virtual environment;
    // its only live dependency is the context, closed before this inspection.
    inspect(root)?;
    std::fs::remove_dir_all(root)?;
    Ok(())
}

fn policy(version: u64) -> WeightPolicy {
    WeightPolicy {
        version_key: VersionKey::Fixed(version),
        cadence: Some(WeightCadence::OncePerEpoch { offset_blocks: 3 }),
    }
}

#[derive(Clone)]
struct MinerScorer {
    miner: Uid,
    enabled: Arc<AtomicBool>,
}
impl Validator for MinerScorer {
    type State = u64;
    async fn step(&self, _cx: &NeuronCx, state: &StateCell<u64>) -> Result<()> {
        *state.write() += 1;
        Ok(())
    }
    fn weights(&self, _cx: &NeuronCx, _state: &u64, _mechanism: MechId) -> WeightsProposal {
        if self.enabled.load(Ordering::SeqCst) {
            WeightsProposal::Set(WeightsInput::ByUid(BTreeMap::from([(self.miner, 1.0)])))
        } else {
            WeightsProposal::HoldPrevious
        }
    }
}

fn event_fields(attributes: &Value) -> &[Value] {
    match attributes {
        Value::List(values) | Value::Tuple(values) => values,
        _ => &[],
    }
}
fn account_matches(value: &Value, account: AccountId) -> bool {
    value.value_bytes().as_deref() == Some(account.as_bytes().as_slice())
        || value
            .as_str()
            .is_some_and(|s| s.parse::<AccountId>().ok() == Some(account))
}

#[derive(Default)]
struct CommitLedger {
    commits: BTreeMap<u16, Vec<(u64, u64)>>,
    reveals: BTreeMap<u16, Vec<u64>>,
}
impl CommitLedger {
    async fn observe(&mut self, snapshot: &Snapshot, fixture: &FrameworkFixture) -> Result<()> {
        let events = snapshot.events().await?;
        let epoch = snapshot
            .storage(&st::SUBNET_EPOCH_INDEX, (fixture.netuid,))
            .await?;
        for event in events.records() {
            let fields = event_fields(&event.attributes);
            let commit = event.matches("SubtensorModule", "TimelockedWeightsCommitted");
            let reveal = event.matches("SubtensorModule", "TimelockedWeightsRevealed");
            let (who, index) = if commit && fields.len() == 4 {
                (&fields[0], &fields[1])
            } else if reveal && fields.len() == 2 {
                (&fields[1], &fields[0])
            } else {
                continue;
            };
            if !account_matches(who, fixture.setup.slot.hotkey.account()) {
                continue;
            }
            let index = u16::try_from(
                index
                    .as_u128()
                    .ok_or_else(|| rtao_neuron::anyhow::anyhow!("commit/reveal index missing"))?,
            )?;
            if NetUidStorageIndex(index).split().0 != fixture.netuid {
                continue;
            }
            if commit {
                self.commits
                    .entry(index)
                    .or_default()
                    .push((epoch, snapshot.block().number));
            } else {
                self.reveals
                    .entry(index)
                    .or_default()
                    .push(snapshot.block().number);
            }
        }
        Ok(())
    }
    fn count(&self) -> usize {
        self.commits.values().map(Vec::len).sum()
    }
    fn all_revealed(&self) -> bool {
        self.commits
            .iter()
            .all(|(index, commits)| self.reveals.get(index).map_or(0, Vec::len) == commits.len())
    }
}

async fn next_head(
    stream: &mut rtao_client::BlockStream,
    deadline: Instant,
) -> Result<BlockHeader> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| rtao_neuron::anyhow::anyhow!("framework scenario exceeded its deadline"))?;
    tokio::time::timeout(remaining, stream.next())
        .await?
        .ok_or_else(|| rtao_neuron::anyhow::anyhow!("finalized stream ended"))?
        .map_err(Into::into)
}

async fn wait_permit(fixture: &FrameworkFixture) -> Result<()> {
    let mut blocks = fixture.cx.client.blocks(Finality::Finalized)?;
    let deadline = Instant::now() + Duration::from_secs(360);
    loop {
        let permits = fixture
            .cx
            .client
            .storage(&st::VALIDATOR_PERMIT, (fixture.netuid,))
            .await?;
        if permits.get(usize::from(fixture.validator_uid.0)) == Some(&true) {
            return Ok(());
        }
        let _ = next_head(&mut blocks, deadline).await?;
    }
}

pub(crate) async fn scenario_01() -> TestResult {
    let ln = localnet().await?;
    let fixture = FrameworkFixture::new(&ln, 1_u8, true).await?;
    let result = run_epochs(&ln, &fixture).await;
    fixture.cleanup().await?;
    result.map_err(Into::into)
}

async fn run_epochs(ln: &Localnet, fixture: &FrameworkFixture) -> Result<()> {
    wait_permit(fixture).await?;
    let count = if ln.profile() == LocalnetProfile::Fast {
        20
    } else {
        3
    };
    let key = NetUidStorageIndex::new(fixture.netuid, MechId(0))?;
    let initial = fixture.cx.client.at(BlockRef::Finalized).await?;
    let incentive = initial.storage(&st::INCENTIVE, (key,)).await?;
    let mut height = initial.block().number + 1;
    let mut blocks = fixture.cx.client.blocks(Finality::Finalized)?;
    let enabled = Arc::new(AtomicBool::new(true));
    let runtime = NeuronRuntime::validator(
        fixture.config.clone(),
        MinerScorer {
            miner: fixture.miner_uid,
            enabled: enabled.clone(),
        },
    )
    .with_context(fixture.cx.clone())
    .with_weight_policy(policy(fixture.version))
    .with_clock(fixture.clock.clone());
    let mut running = tokio::spawn(runtime.run());
    let mut runtime_finished = false;
    let deadline = Instant::now() + Duration::from_secs(1500);
    let mut ledger = CommitLedger::default();
    let mut incentive_moved = false;
    let result = async {
        loop {
            let head = tokio::select! {
                result = &mut running => { runtime_finished = true; result??; rtao_neuron::anyhow::bail!("runtime stopped before completing epochs"); }
                head = next_head(&mut blocks, deadline) => head?,
            };
            // Read every finalized block, including any gap while RPC work ran.
            for number in height..=head.number {
                let snapshot = fixture.cx.client.at(BlockRef::Number(number)).await?;
                ledger.observe(&snapshot, fixture).await?;
                let current = snapshot.storage(&st::INCENTIVE, (key,)).await?;
                incentive_moved |= current != incentive
                    && current.get(usize::from(fixture.miner_uid.0)).is_some_and(|value| value.0 > 0);
            }
            height = head.number + 1;
            rtao_neuron::anyhow::ensure!(ledger.count() <= count, "runtime submitted more than the requested {count} epochs");
            if ledger.count() == count {
                enabled.store(false, Ordering::SeqCst);
                if ledger.all_revealed() && incentive_moved { break; }
            }
        }
        let commits = ledger.commits.get(&key.0).ok_or_else(|| rtao_neuron::anyhow::anyhow!("no framework commitments"))?;
        rtao_neuron::anyhow::ensure!(commits.len() == count && ledger.reveals[&key.0].len() == count, "every accepted commitment must reveal");
        let epochs: BTreeSet<_> = commits.iter().map(|(epoch, _)| *epoch).collect();
        rtao_neuron::anyhow::ensure!(epochs.len() == count, "multiple commits in one actual epoch");
        rtao_neuron::anyhow::ensure!(epochs.last().zip(epochs.first()).is_some_and(|(last, first)| last - first + 1 == count as u64), "framework skipped an epoch");
        let stored = fixture.cx.client.storage(&st::WEIGHTS, (key, fixture.validator_uid.0)).await?;
        rtao_neuron::anyhow::ensure!(stored == vec![(fixture.miner_uid.0, u16::MAX)], "revealed miner weights differ: {stored:?}");
        rtao_neuron::anyhow::ensure!(!fixture.cx.metrics.render().contains("rtao_weights_reveal_missed_total"), "framework reported a missed reveal");
        Ok(())
    }.await;
    fixture.cx.shutdown.cancel();
    if !runtime_finished {
        match tokio::time::timeout(Duration::from_secs(70), &mut running).await {
            Ok(finished) => {
                finished??;
            }
            Err(_) => {
                running.abort();
                let _ = running.await;
                rtao_neuron::anyhow::bail!("runtime did not shut down");
            }
        }
    }
    result
}

pub(crate) async fn scenario_03() -> TestResult {
    let ln = localnet().await?;
    let fixture = FrameworkFixture::new(&ln, 2_u8, true).await?;
    let result = mechanisms(&fixture).await;
    fixture.cleanup().await?;
    result.map_err(Into::into)
}

async fn mechanisms(fixture: &FrameworkFixture) -> Result<()> {
    wait_permit(fixture).await?;
    let mut scheduler = fixture.scheduler()?;
    let mut blocks = fixture.cx.client.blocks(Finality::Finalized)?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let start = fixture
        .cx
        .client
        .at(BlockRef::Finalized)
        .await?
        .block()
        .number;
    let mut height = start + 1;
    let mut ledger = CommitLedger::default();
    let mut accepted = BTreeSet::new();
    loop {
        let head = next_head(&mut blocks, deadline).await?;
        fixture.observe_clock().await?;
        scheduler
            .on_finalized(&fixture.cx, &head, |mech| {
                if accepted.contains(&mech) {
                    WeightsProposal::HoldPrevious
                } else {
                    fixture.proposal()
                }
            })
            .await?;
        for mech in [MechId(0), MechId(1)] {
            if let Some(attempt) = scheduler.last_attempt(mech) {
                match &attempt.outcome {
                    WeightAttemptOutcome::Submitted { .. } => {
                        accepted.insert(mech);
                    }
                    WeightAttemptOutcome::Refused(error) => rtao_neuron::anyhow::bail!("{error}"),
                    WeightAttemptOutcome::Failed(error)
                    | WeightAttemptOutcome::Unresolved(error) => {
                        rtao_neuron::anyhow::bail!("{error}")
                    }
                    _ => {}
                }
            }
        }
        for number in height..=head.number {
            ledger
                .observe(
                    &fixture.cx.client.at(BlockRef::Number(number)).await?,
                    fixture,
                )
                .await?;
        }
        height = head.number + 1;
        if accepted.len() == 2 && ledger.count() == 2 && ledger.all_revealed() {
            break;
        }
    }
    for mech in [MechId(0), MechId(1)] {
        let index = NetUidStorageIndex::new(fixture.netuid, mech)?;
        let commits = &ledger.commits[&index.0];
        rtao_neuron::anyhow::ensure!(
            commits.len() == 1 && ledger.reveals[&index.0].len() == 1,
            "mechanism {} must have one observed commit/reveal",
            mech.0
        );
        let last = fixture
            .cx
            .client
            .storage(&st::LAST_UPDATE, (index,))
            .await?;
        rtao_neuron::anyhow::ensure!(
            last.get(usize::from(fixture.validator_uid.0)) == Some(&commits[0].1),
            "LastUpdate does not match mechanism {} commit inclusion",
            mech.0
        );
        let weights = fixture
            .cx
            .client
            .storage(&st::WEIGHTS, (index, fixture.validator_uid.0))
            .await?;
        rtao_neuron::anyhow::ensure!(
            weights == vec![(fixture.miner_uid.0, u16::MAX)],
            "mechanism {} weights did not reveal",
            mech.0
        );
    }
    rtao_neuron::anyhow::ensure!(
        scheduler
            .reveals()
            .outcomes()
            .iter()
            .filter(|outcome| outcome.missed.is_none())
            .count()
            == 2,
        "scheduler did not attribute both reveals"
    );
    Ok(())
}

pub(crate) async fn scenario_05() -> TestResult {
    let ln = localnet().await?;
    let fixture = FrameworkFixture::new(&ln, 1_u8, false).await?;
    let result = version_upgrade(&ln, &fixture).await;
    fixture.cleanup().await?;
    result.map_err(Into::into)
}

async fn version_upgrade(ln: &Localnet, fixture: &FrameworkFixture) -> Result<()> {
    wait_permit(fixture).await?;
    let mut scheduler = fixture.scheduler()?;
    let mut blocks = fixture.cx.client.blocks(Finality::Finalized)?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let first = drive_until_submitted(&mut scheduler, fixture, &mut blocks, deadline).await?;
    let upgraded = fixture
        .version
        .checked_add(1)
        .ok_or_else(|| rtao_neuron::anyhow::anyhow!("version overflow"))?;
    let setter = admin_utils::sudo_set_weights_version_key(fixture.netuid, upgraded)
        .compose(&ln.client().runtime().await?)?;
    let changed = ln.submit_admin(setter, &fixture.setup.slot.owner).await?;
    rtao_neuron::anyhow::ensure!(changed.success, "owner version bump failed: {changed:?}");
    let nonce = fixture
        .cx
        .client
        .nonces()
        .fresh(&fixture.setup.slot.hotkey.account())
        .await?;
    loop {
        let head = next_head(&mut blocks, deadline).await?;
        scheduler
            .on_finalized(&fixture.cx, &head, |_| fixture.proposal())
            .await?;
        if let Some(attempt) = scheduler.last_attempt(MechId(0))
            && let WeightAttemptOutcome::Refused(refusal) = &attempt.outcome
        {
            rtao_neuron::anyhow::ensure!(
                refusal.name == "IncorrectWeightVersionKey"
                    && refusal.remediation.contains("configure")
                    && refusal.remediation.contains(&upgraded.to_string()),
                "version refusal lacks actionable remediation: {refusal}"
            );
            break;
        }
    }
    rtao_neuron::anyhow::ensure!(
        fixture
            .cx
            .client
            .nonces()
            .fresh(&fixture.setup.slot.hotkey.account())
            .await?
            == nonce,
        "framework submitted despite version refusal"
    );
    scheduler.update_policy(policy(upgraded));
    let resumed = drive_until_submitted(&mut scheduler, fixture, &mut blocks, deadline).await?;
    rtao_neuron::anyhow::ensure!(
        resumed > first,
        "reconfiguration did not resume weight submission"
    );
    Ok(())
}

async fn drive_until_submitted(
    scheduler: &mut WeightScheduler,
    fixture: &FrameworkFixture,
    blocks: &mut rtao_client::BlockStream,
    deadline: Instant,
) -> Result<u64> {
    loop {
        let head = next_head(blocks, deadline).await?;
        fixture.observe_clock().await?;
        scheduler
            .on_finalized(&fixture.cx, &head, |_| fixture.proposal())
            .await?;
        if let Some(attempt) = scheduler.last_attempt(MechId(0)) {
            match &attempt.outcome {
                WeightAttemptOutcome::Submitted { block_number, .. } => return Ok(*block_number),
                WeightAttemptOutcome::Refused(error) => rtao_neuron::anyhow::bail!("{error}"),
                WeightAttemptOutcome::Failed(error) | WeightAttemptOutcome::Unresolved(error) => {
                    rtao_neuron::anyhow::bail!("{error}")
                }
                _ => {}
            }
        }
    }
}

pub(crate) async fn scenario_06() -> TestResult {
    let ln = localnet().await?;
    let original = ln
        .client()
        .storage(&st::COMMIT_REVEAL_WEIGHTS_VERSION, ())
        .await?;
    let fixture = FrameworkFixture::new(&ln, 1_u8, true).await?;
    let result = cr_version_gate(&ln, &fixture).await;
    let restored = async {
        ln.sudo(
            admin_utils::sudo_set_commit_reveal_version(original)
                .compose(&ln.client().runtime().await?)?,
        )
        .await
        .map_err(rtao_neuron::anyhow::Error::from)
    }
    .await;
    fixture.cleanup().await?;
    if !restored?.success {
        return Err("global CR version restoration failed".into());
    }
    result.map_err(Into::into)
}

async fn cr_version_gate(ln: &Localnet, fixture: &FrameworkFixture) -> Result<()> {
    wait_permit(fixture).await?;
    let changed = ln
        .sudo(
            admin_utils::sudo_set_commit_reveal_version(5)
                .compose(&ln.client().runtime().await?)?,
        )
        .await?;
    rtao_neuron::anyhow::ensure!(
        changed.success,
        "global CR version change failed: {changed:?}"
    );
    let nonce = fixture
        .cx
        .client
        .nonces()
        .fresh(&fixture.setup.slot.hotkey.account())
        .await?;
    let index = NetUidStorageIndex::new(fixture.netuid, MechId(0))?;
    let last = fixture
        .cx
        .client
        .storage(&st::LAST_UPDATE, (index,))
        .await?;
    let mut blocks = fixture.cx.client.blocks(Finality::Finalized)?;
    let head = next_head(&mut blocks, Instant::now() + Duration::from_secs(120)).await?;
    let mut scheduler = fixture.scheduler()?;
    scheduler
        .on_finalized(&fixture.cx, &head, |_| fixture.proposal())
        .await?;
    let refusal = scheduler
        .last_attempt(MechId(0))
        .and_then(|attempt| match &attempt.outcome {
            WeightAttemptOutcome::Refused(refusal) => Some(refusal),
            _ => None,
        })
        .ok_or_else(|| {
            rtao_neuron::anyhow::anyhow!("framework did not refuse unsupported CR version")
        })?;
    rtao_neuron::anyhow::ensure!(
        refusal.name == "IncorrectCommitRevealVersion"
            && refusal.remediation.contains("5")
            && refusal.remediation.contains("upgrade"),
        "wrong CR version remediation: {refusal}"
    );
    rtao_neuron::anyhow::ensure!(
        fixture
            .cx
            .client
            .nonces()
            .fresh(&fixture.setup.slot.hotkey.account())
            .await?
            == nonce,
        "framework submitted unsupported CR version"
    );
    rtao_neuron::anyhow::ensure!(
        fixture
            .cx
            .client
            .storage(&st::LAST_UPDATE, (index,))
            .await?
            == last,
        "framework refusal changed LastUpdate"
    );
    let intent = rtao_intents::SetWeights {
        netuid: fixture.netuid,
        mechid: MechId(0),
        version_key: fixture.version,
        raw_u16: false,
        uids: Some(vec![fixture.miner_uid]),
        weights: Some(rtao_intents::WeightsArg::List(vec![Ratio::new(1.0)?])),
    };
    // Python-compatible SetWeights intentionally lets the chain enforce this gate.
    let outcome = fixture
        .cx
        .executor
        .execute(
            &intent,
            &fixture.cx.hotkey,
            &rtao_tx::ExecuteOptions {
                deadline: Some(Duration::from_secs(240)),
                ..Default::default()
            },
        )
        .await?;
    rtao_neuron::anyhow::ensure!(
        !outcome.success
            && outcome
                .error
                .as_ref()
                .is_some_and(|error| error.name.as_deref() == Some("IncorrectCommitRevealVersion")),
        "typed SetWeights did not receive the chain dispatch error: {outcome:?}"
    );
    Ok(())
}
