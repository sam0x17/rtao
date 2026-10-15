//! Upgrade a live node while an executor holds a plan from the prior runtime.

use super::{TestResult, framework};
use futures::StreamExt;
use rtao_client::{BlockRef, View, ViewExt};
use rtao_intents::transfer::Transfer;
use rtao_primitives::{AccountId, Balance, MechId, Tao};
use rtao_runtime::{CallDescriptor, calls};
use rtao_signer::{Coldkey, KeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole};
use rtao_testkit::Localnet;
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentInterface, PreflightCx};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct SchedulerDriver {
    attempts: Arc<Mutex<Vec<rtao_neuron::weights::WeightAttempt>>>,
    cancel: CancellationToken,
    worker: Option<tokio::task::JoinHandle<Result<(), String>>>,
}
impl SchedulerDriver {
    async fn start(fixture: &framework::Fixture) -> Result<Self, Box<dyn std::error::Error>> {
        let mut scheduler = framework::scheduler(&fixture.cx).await?;
        let cx = fixture.cx.clone();
        let miner = fixture.miner_uid;
        let mut heads = cx.client.blocks(rtao_client::Finality::Finalized)?;
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let recorded = attempts.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker = tokio::spawn(async move {
            loop {
                let head = tokio::select! {
                    () = worker_cancel.cancelled() => return Ok(()),
                    head = heads.next() => head.ok_or("validator finalized stream ended")?.map_err(|error| error.to_string())?,
                };
                scheduler
                    .on_finalized(&cx, &head, |_| framework::proposal(miner))
                    .await
                    .map_err(|error| error.to_string())?;
                if let Some(attempt) = scheduler.last_attempt(MechId(0)) {
                    let mut records = recorded
                        .lock()
                        .map_err(|_| "validator observation lock poisoned")?;
                    if records.len() >= 10_000 {
                        return Err("validator observations exceeded bound".into());
                    }
                    records.push(attempt.clone());
                }
            }
        });
        Ok(Self {
            attempts,
            cancel,
            worker: Some(worker),
        })
    }
    async fn wait_for(&self, minimum_best: u64, paused: bool) -> TestResult {
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop {
            let observed = self
                .attempts
                .lock()
                .map_err(|_| "validator observation lock poisoned")?
                .iter()
                .any(|attempt| {
                    attempt.best_block >= minimum_best
                        && if paused {
                            matches!(
                                attempt.outcome,
                                rtao_neuron::weights::WeightAttemptOutcome::PausedSurface { .. }
                            )
                        } else {
                            matches!(
                                attempt.outcome,
                                rtao_neuron::weights::WeightAttemptOutcome::Submitted { .. }
                            )
                        }
                });
            if observed {
                return Ok(());
            }
            if self
                .worker
                .as_ref()
                .is_none_or(|worker| worker.is_finished())
            {
                return Err("validator scheduler stopped before the required outcome".into());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("validator did not {} at or after best block {minimum_best}; observations: {:?}", if paused { "pause" } else { "submit" }, self.attempts.lock().map_err(|_| "validator observation lock poisoned")?).into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn close(&mut self) -> TestResult {
        self.cancel.cancel();
        if let Some(mut worker) = self.worker.take() {
            match tokio::time::timeout(Duration::from_secs(180), &mut worker).await {
                Ok(result) => result?.map_err(Into::into),
                Err(error) => {
                    worker.abort();
                    let _ = worker.await;
                    Err(error.into())
                }
            }
        } else {
            Ok(())
        }
    }
}
impl Drop for SchedulerDriver {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}

struct CountingTransfer {
    inner: Transfer,
    builds: Arc<AtomicUsize>,
}
impl IntentInterface for CountingTransfer {
    const SPEC: &'static rtao_catalog::IntentSpec = Transfer::SPEC;
    type Role = <Transfer as IntentInterface>::Role;
    type Origin = <Transfer as IntentInterface>::Origin;
    type Output = <Transfer as IntentInterface>::Output;
    const WRAPS: &'static [&'static CallDescriptor] = Transfer::WRAPS;
    fn to_json(&self) -> serde_json::Value {
        self.inner.to_json()
    }
}
impl Intent for CountingTransfer {
    fn summary(&self) -> String {
        self.inner.summary()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.inner.build(cx).await
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<rtao_tx::IntentPreflight, IntentError> {
        self.inner.preflight(cx).await
    }
    fn spend(&self) -> rtao_primitives::Spend {
        self.inner.spend()
    }
}

struct GateKeys {
    signer: KeypairSigner,
    ready: Arc<Notify>,
    release: Arc<Notify>,
    unlocks: Arc<AtomicUsize>,
}
impl KeyView for GateKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl KeySource<Coldkey> for GateKeys {
    fn address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        self.ready.notify_one();
        self.release.notified().await;
        Ok(Arc::new(self.signer.clone()))
    }
}

pub async fn run(ln: &Localnet) -> TestResult {
    let source_endpoint = std::env::var("E2E_UPGRADE_ENDPOINT")?;
    let source = Localnet::connect(&source_endpoint, rtao_testkit::LocalnetProfile::Fast).await?;
    let old = ln.client().runtime().await?;
    let new = source.client().runtime().await?;
    source.client().close().await;
    if new.spec_version() <= old.spec_version() {
        eprintln!(
            "SKIP scenario_08_runtime_upgrade_mid_session: main fast-runtime spec {} does not exceed this lane's spec {}; System.set_code requires an increase",
            new.spec_version(),
            old.spec_version()
        );
        return Ok(());
    }
    let wasm_path = std::path::PathBuf::from(std::env::var("E2E_UPGRADE_WASM")?);
    let task_root = std::path::PathBuf::from(std::env::var("RTAO_TEMP_ROOT")?);
    assert!(wasm_path.is_absolute() && wasm_path.starts_with(&task_root));
    assert!(
        !wasm_path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    );
    let wasm = std::fs::read(wasm_path)?;
    assert!(!wasm.is_empty(), "main fast-runtime wasm is empty");
    let fixture = framework::Fixture::new(ln, "runtime-upgrade", false).await?;
    let result = exercise(ln, &fixture, wasm, old, new.spec_version()).await;
    let closed = fixture.close().await;
    result?;
    closed?;
    Ok(())
}

async fn exercise(
    ln: &Localnet,
    fixture: &framework::Fixture,
    wasm: Vec<u8>,
    old: rtao_runtime::RuntimeHandle,
    expected_spec: u32,
) -> TestResult {
    let mut driver = SchedulerDriver::start(fixture).await?;
    let outcome = exercise_running(ln, fixture, wasm, old, expected_spec, &driver).await;
    let closed = driver.close().await;
    outcome?;
    closed
}

async fn exercise_running(
    ln: &Localnet,
    fixture: &framework::Fixture,
    wasm: Vec<u8>,
    old: rtao_runtime::RuntimeHandle,
    expected_spec: u32,
    driver: &SchedulerDriver,
) -> TestResult {
    driver.wait_for(0, false).await?;
    let slot = ln.slot()?;
    let before = ln.client().nonces().fresh(&slot.owner.account()).await?;
    let ready = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let unlocks = Arc::new(AtomicUsize::new(0));
    let builds = Arc::new(AtomicUsize::new(0));
    let gate = GateKeys {
        signer: slot.owner.clone(),
        ready: ready.clone(),
        release: release.clone(),
        unlocks: unlocks.clone(),
    };
    let intent = CountingTransfer {
        inner: Transfer {
            dest_ss58: slot.hotkey.account().into(),
            amount_tao: Balance::<Tao>::from_rao(1_000_000).into(),
            keep_alive: true,
        },
        builds: builds.clone(),
    };
    let executor = rtao_tx::Executor::new(ln.client().clone());
    let mut execution = tokio::spawn(async move {
        executor
            .execute(
                &intent,
                &gate,
                &rtao_tx::ExecuteOptions {
                    deadline: Some(Duration::from_secs(300)),
                    submit_deadline: Some(Duration::from_secs(240)),
                    ..Default::default()
                },
            )
            .await
    });
    let transition = async {
        tokio::time::timeout(Duration::from_secs(60), ready.notified()).await?;
        assert_eq!(builds.load(Ordering::SeqCst), 1, "Executor must hold one old-runtime plan before upgrade");
        let upgrade = ln.sudo(calls::system::set_code(wasm).compose(&old)?).await?;
        assert!(upgrade.success, "runtime upgrade failed: {upgrade:?}");
        let upgrade_height = upgrade.block_number.ok_or("upgrade inclusion height absent")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        let changed = loop {
            // Exercise the production watcher/cache, without manually refreshing.
            let current = ln.client().runtime().await?;
            if current.spec_version() == expected_spec { break current; }
            if std::time::Instant::now() >= deadline { return Err("runtime codec did not switch after set_code".into()); }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_ne!(old.stamp(), changed.stamp());
        assert!(!std::ptr::eq(old.entry(), changed.entry()));
        assert!(changed.surface_report().checked > 0, "new runtime surface was not verified");
        assert_eq!(changed.core().spec_version, expected_spec);
        loop {
            let snapshot = ln.client().at(BlockRef::Finalized).await?;
            if snapshot.block().number >= upgrade_height { break; }
            if std::time::Instant::now() >= deadline { return Err("runtime upgrade did not finalize".into()); }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let descriptor = "call:SubtensorModule.set_mechanism_weights";
        if changed.surface_report().shape_matches(descriptor) {
            driver.wait_for(upgrade_height + 1, false).await?;
        } else {
            driver.wait_for(upgrade_height + 1, true).await?;
            let records = driver.attempts.lock().map_err(|_| "validator observation lock poisoned")?;
            assert!(records.iter().filter(|attempt| attempt.best_block > upgrade_height).all(|attempt| !matches!(attempt.outcome, rtao_neuron::weights::WeightAttemptOutcome::Submitted { .. })), "validator submitted against an incompatible weight surface");
            assert!(records.iter().any(|attempt| matches!(&attempt.outcome, rtao_neuron::weights::WeightAttemptOutcome::PausedSurface { descriptor: actual, .. } if actual == descriptor)), "changed weight surface was not paused");
        }
        let new_snapshot = ln.client().at(BlockRef::Best).await?;
        assert_eq!(new_snapshot.block().runtime.spec_version(), expected_spec);
        let _ = new_snapshot.storage(&rtao_runtime::storage::subtensor_module::TEMPO, (fixture.cx.netuid,)).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }.await;
    release.notify_one();
    let output = match tokio::time::timeout(Duration::from_secs(300), &mut execution).await {
        Ok(output) => output??,
        Err(error) => {
            execution.abort();
            let _ = execution.await;
            return Err(error.into());
        }
    };
    transition?;
    assert!(
        output.success,
        "Executor transfer after replan failed: {output:?}"
    );
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "Executor must replan exactly once across the upgrade"
    );
    assert_eq!(
        unlocks.load(Ordering::SeqCst),
        1,
        "replan unlocked the key again"
    );
    assert_eq!(
        ln.client().nonces().fresh(&slot.owner.account()).await?,
        before + 1
    );
    Ok(())
}
