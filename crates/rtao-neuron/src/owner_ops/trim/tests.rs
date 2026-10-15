//! Signed fake-chain batches and restart safety.
#![allow(clippy::expect_used)]

use super::*;
use crate::OwnerConfig;
use rtao_client::{ClientConfig, Network};
use rtao_primitives::{CryptoType, DefaultAccount, MechId};
use rtao_signer::{
    BoxFuture, KeyView, KeypairSigner, MultiSignature, Signer, SignerError, SigningRequest,
};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use rtao_tx::{ExecuteOptions, Executor, Policy};
use serde_json::{Value as Json, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

const NET: NetUid = NetUid(3);
const TARGET: u16 = 128;
static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> anyhow::Result<Self> {
        let root = std::env::var_os("RTAO_TEST_TMPDIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!("set RTAO_TEST_TMPDIR to the task-scoped temporary root")
            })?;
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
    fn recovery(&self) -> PathBuf {
        self.0.join("pruning.json")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Step {
    Remove,
    Cleanup,
    Stall,
    MissingProgress,
    WrongTarget,
    Oversized,
    UnchangedStorage,
    ChangedMechanisms,
    ChangedOwner,
    Failure(&'static str),
    Unknown,
    Cancel,
    CancelFinalized,
}

#[derive(Clone)]
struct JournalSigner {
    key: KeypairSigner,
    owner: AccountId,
    chain: FakeChain,
    recovery: PathBuf,
    steps: Arc<Mutex<VecDeque<Step>>>,
    population: Arc<AtomicU16>,
    calls: Arc<Mutex<Vec<Vec<u8>>>>,
    cancel: CancellationToken,
}
impl Signer for JournalSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            let record: Json = serde_json::from_slice(
                &fs::read(&self.recovery).expect("journal exists before signing"),
            )
            .expect("journal is valid JSON");
            assert_eq!(record["operation"], json!("trim_null_subnet_batch"));
            assert_eq!(record["netuid"], json!(NET.0));
            assert_eq!(record["target"], json!(TARGET));
            assert_eq!(record["owner"], json!(self.owner));
            assert_eq!(
                record["call_hex"],
                json!(format!("0x{}", hex::encode(request.call)))
            );
            self.calls
                .lock()
                .expect("calls lock")
                .push(request.call.to_vec());
            let step = self
                .steps
                .lock()
                .expect("steps lock")
                .pop_front()
                .unwrap_or(Step::Remove);
            if matches!(step, Step::Cancel) {
                self.cancel.cancel();
                futures::future::pending::<()>().await;
            }
            if matches!(step, Step::CancelFinalized) {
                self.cancel.cancel();
            }
            self.chain.mutate_state(|state| {
                if matches!(step, Step::Unknown) {
                    state.queue_submit(SubmitScript::Dropped);
                    return;
                }
                if let Step::Failure(name) = step {
                    state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([failure(name)]) });
                    return;
                }
                let before = self.population.load(Ordering::SeqCst);
                let removed = match step {
                    Step::Cleanup | Step::Stall => 0,
                    Step::Oversized => 65,
                    _ => before.saturating_sub(TARGET).min(64),
                };
                let remaining = before.saturating_sub(removed);
                let pending = (remaining > TARGET).then_some(TARGET);
                let cleared = if matches!(step, Step::Cleanup) { 2500u32 } else { 0 };
                if !matches!(step, Step::UnchangedStorage) {
                    self.population.store(remaining, Ordering::SeqCst);
                    state.seed(&st::SUBNETWORK_N, (NET,), remaining).expect("population seed");
                    seed_target(state, pending).expect("target seed");
                }
                if matches!(step, Step::ChangedMechanisms) {
                    state.seed(&st::MECHANISM_COUNT_CURRENT, (NET,), MechId(2)).expect("mechanism seed");
                }
                if matches!(step, Step::ChangedOwner) {
                    state.seed(&st::SUBNET_OWNER, (NET,), DefaultAccount::from(AccountId([2;32]))).expect("owner seed");
                }
                let mut events = vec![success()];
                if self.owner != self.account() {
                    events.push(crate::owner_ops::tests::proxy::executed());
                }
                if !matches!(step, Step::MissingProgress) {
                    let target = if matches!(step, Step::WrongTarget) { TARGET + 1 } else { TARGET };
                    events.push(json!({
                        "phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule",
                        "event_id":"NullUidsPruningProgress","topics":[],
                        "attributes":{"netuid":NET.0,"target":target,"remaining":remaining,"removed":removed,"cleared_commits":cleared}
                    }));
                }
                state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!(events) });
            });
            self.key.sign(request).await
        })
    }
}

struct Keys {
    signer: JournalSigner,
    unlocks: AtomicUsize,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([8; 32]))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok([8; 32])
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl KeySource<Coldkey> for Keys {
    fn address(&self) -> Result<AccountId, SignerError> {
        self.coldkey_address()
    }
    async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
        assert!(
            self.signer.recovery.is_file(),
            "journal secured before private-key unlock"
        );
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(self.signer.clone()))
    }
}

struct Fixture {
    _temp: Temp,
    owner: OwnerOps<Keys>,
    chain: FakeChain,
    recovery: PathBuf,
}
async fn fixture(
    population: u16,
    steps: impl IntoIterator<Item = Step>,
) -> anyhow::Result<Fixture> {
    fixture_routed(population, steps, false).await
}
async fn fixture_routed(
    population: u16,
    steps: impl IntoIterator<Item = Step>,
    proxied: bool,
) -> anyhow::Result<Fixture> {
    let temp = Temp::new()?;
    let recovery = temp.recovery();
    let key = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?;
    let mut state = FakeState::try_default()?;
    let effective = if proxied {
        crate::owner_ops::tests::proxy::REAL
    } else {
        key.account()
    };
    if proxied {
        crate::owner_ops::tests::proxy::seed_proxy(
            &mut state,
            key.account(),
            rtao_runtime::models::ProxyType::Owner,
            0,
            Some(&[("AdminUtils", "sudo_trim_null_uids_batch")]),
        )?;
    }
    state.seed(&st::NETWORKS_ADDED, (NET,), true)?;
    state.seed(&st::SUBNET_OWNER, (NET,), DefaultAccount::from(effective))?;
    state.seed(&st::SUBNETWORK_N, (NET,), population)?;
    seed_target(&mut state, None)?;
    state.seed(&st::MECHANISM_COUNT_CURRENT, (NET,), MechId(1))?;
    state.seed(&st::MIN_ALLOWED_UIDS, (NET,), 64u16)?;
    state.seed(&st::MAX_ALLOWED_UIDS, (NET,), 2000u16)?;
    state.seed_python(
        "SubtensorModule",
        "SubnetEpochConsensus",
        &[json!(NET.0)],
        json!("Null"),
    )?;
    state.seed(&st::TEMPO, (NET,), 360u16)?;
    state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 100u64)?;
    state.seed(&st::PENDING_EPOCH_AT, (NET,), 0u64)?;
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 10u16)?;
    let genesis = hex::decode(state.genesis_hash.trim_start_matches("0x"))?
        .try_into()
        .map(H256)
        .map_err(|_| anyhow::anyhow!("invalid fake genesis"))?;
    state.seed(&system::BLOCK_HASH, (0u32,), genesis)?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let cancel = CancellationToken::new();
    let keys = Arc::new(Keys {
        signer: JournalSigner {
            key,
            owner: effective,
            chain: chain.clone(),
            recovery: recovery.clone(),
            steps: Arc::new(Mutex::new(steps.into_iter().collect())),
            population: Arc::new(AtomicU16::new(population)),
            calls: Arc::new(Mutex::new(Vec::new())),
            cancel: cancel.clone(),
        },
        unlocks: AtomicUsize::new(0),
    });
    let owner = OwnerOps::new(Executor::new(client), keys)
        .with_cancellation(cancel)
        .with_config(OwnerConfig {
            operation_timeout: Some(Duration::from_secs(5)),
            trim_max_batches: 128,
        })?
        .with_execute_options(ExecuteOptions {
            reconcile: false,
            proxy_for: proxied.then_some(effective),
            force_proxy_type: proxied.then_some(rtao_runtime::models::ProxyType::Owner),
            ..Default::default()
        })?;
    Ok(Fixture {
        _temp: temp,
        owner,
        chain,
        recovery,
    })
}

fn success() -> Json {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}
fn seed_target(state: &mut FakeState, target: Option<u16>) -> anyhow::Result<()> {
    if let Some(target) = target {
        state.seed_python(
            "SubtensorModule",
            "NullPruningTarget",
            &[json!(NET.0)],
            json!(target),
        )?;
    } else {
        let info = state
            .runtime
            .storage_entry("SubtensorModule", "NullPruningTarget")
            .ok_or_else(|| anyhow::anyhow!("NullPruningTarget descriptor absent"))?;
        let key = state
            .runtime
            .storage_key(info, &[Value::Uint(u128::from(NET.0))])?;
        state.seed_encoded(key, None);
    }
    Ok(())
}
fn failure(name: &str) -> Json {
    let error = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|error| error.pallet == "SubtensorModule" && error.name == name)
        .expect("runtime error");
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":error.pallet_index,"error":[error.error_index,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}

#[tokio::test]
async fn actual_signed_batches_are_bounded_and_await_finalized_storage() -> anyhow::Result<()> {
    let mut f = fixture(300, [Step::Cleanup]).await?;
    f.owner.options.wait_for_inclusion = false;
    f.owner.options.wait_for_finalization = false;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert_eq!(
        (
            report.batches,
            report.population,
            report.removed_uids,
            report.cleared_commits
        ),
        (4, TARGET, 172, 2500)
    );
    assert!(report.last_extrinsic_id.is_some());
    assert!(!f.recovery.exists());
    assert!(f.recovery.with_file_name("pruning.json.lock").is_file());
    f.chain.mutate_state(|state| {
        assert_eq!(state.submissions.len(), 4);
        for bytes in &state.submissions {
            let decoded = state
                .runtime
                .decode_extrinsic(bytes, true)
                .expect("signed extrinsic");
            let call = decoded.field("call").expect("call");
            assert!(format!("{call:?}").contains("sudo_trim_null_uids_batch"));
        }
    });
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_batches_record_effective_owner_and_exact_wrapped_calls() -> anyhow::Result<()> {
    let f = fixture_routed(300, [Step::Cleanup], true).await?;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert_eq!(
        (report.batches, report.population, report.removed_uids),
        (4, TARGET, 172)
    );
    assert!(!f.recovery.exists());
    let runtime = f.owner.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::admin_utils::sudo_trim_null_uids_batch(NET, TARGET)
        .compose(&runtime)?;
    let expected = rtao_client::compose::proxy(
        &runtime,
        crate::owner_ops::tests::proxy::REAL,
        Some(rtao_runtime::models::ProxyType::Owner),
        inner,
    )?;
    for call in f.owner.keys.signer.calls.lock().expect("calls lock").iter() {
        assert_eq!(call, expected.as_bytes());
    }
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn proxy_unverified_progress_retains_owner_journal_and_blocks_restart() -> anyhow::Result<()>
{
    for step in [
        Step::MissingProgress,
        Step::WrongTarget,
        Step::Oversized,
        Step::UnchangedStorage,
        Step::Unknown,
    ] {
        let f = fixture_routed(300, [step], true).await?;
        assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
        let record: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
        assert_eq!(record["owner"], json!(crate::owner_ops::tests::proxy::REAL));
        assert_ne!(record["owner"], json!(f.owner.keys.signer.account()));
        let before = f.chain.mutate_state(|s| s.submissions.len());
        assert_eq!(before, 1);
        assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
        assert_eq!(f.chain.mutate_state(|s| s.submissions.len()), before);
        f.owner.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn proxy_trim_missing_permission_never_journals_or_unlocks() -> anyhow::Result<()> {
    let f = fixture_routed(300, [], true).await?;
    f.chain.mutate_state(|s| {
        crate::owner_ops::tests::proxy::seed_proxy(
            s,
            AccountId([8; 32]),
            rtao_runtime::models::ProxyType::Owner,
            0,
            None,
        )
    })?;
    assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
    assert!(!f.recovery.exists());
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(f.chain.mutate_state(|s| s.submissions.is_empty()));
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn matching_pending_target_resumes_despite_first_batch_cooldown() -> anyhow::Result<()> {
    let f = fixture(200, []).await?;
    f.chain.mutate_state(|state| -> anyhow::Result<()> {
        seed_target(state, Some(TARGET))?;
        state.seed(
            &st::TRANSACTION_KEY_LAST_BLOCK,
            (f.owner.keys.address()?, NET, 9u16),
            100u64,
        )?;
        Ok(())
    })?;
    assert_eq!(
        f.owner.trim_null(NET, TARGET, &f.recovery).await?.batches,
        2
    );
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn bounded_stop_resumes_from_chain_without_local_population_claim() -> anyhow::Result<()> {
    let mut f = fixture(300, []).await?;
    f.owner.config.trim_max_batches = 1;
    let error = f
        .owner
        .trim_null(NET, TARGET, &f.recovery)
        .await
        .expect_err("batch cap");
    assert!(error.to_string().contains("batch limit"));
    assert!(!f.recovery.exists());
    f.owner.config.trim_max_batches = 128;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert_eq!(
        (report.batches, report.removed_uids, report.population),
        (2, 108, TARGET)
    );
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn bad_receipt_or_unadvanced_storage_retains_record_and_blocks_restart() -> anyhow::Result<()>
{
    for step in [
        Step::MissingProgress,
        Step::WrongTarget,
        Step::Oversized,
        Step::UnchangedStorage,
        Step::ChangedMechanisms,
        Step::ChangedOwner,
        Step::Unknown,
    ] {
        let f = fixture(300, [step]).await?;
        assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
        assert!(f.recovery.is_file());
        let submissions = f.chain.mutate_state(|state| state.submissions.len());
        let unlocks = f.owner.keys.unlocks.load(Ordering::SeqCst);
        let error = f
            .owner
            .trim_null(NET, TARGET, &f.recovery)
            .await
            .expect_err("unresolved restart");
        assert!(error.to_string().contains("unresolved submission"));
        assert_eq!(
            f.chain.mutate_state(|state| state.submissions.len()),
            submissions
        );
        assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), unlocks);
        f.owner.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_during_signing_keeps_record_before_any_broadcast() -> anyhow::Result<()> {
    let mut f = fixture(300, [Step::Cancel]).await?;
    f.owner.config.operation_timeout = Some(Duration::from_millis(100));
    let error = f
        .owner
        .trim_null(NET, TARGET, &f.recovery)
        .await
        .expect_err("cancel");
    assert!(error.to_string().contains("timed out"));
    assert!(f.recovery.is_file());
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 0);
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn cancellation_drains_one_batch_and_retains_record_before_next_batch() -> anyhow::Result<()>
{
    let f = fixture(300, [Step::CancelFinalized]).await?;
    let error = f
        .owner
        .trim_null(NET, TARGET, &f.recovery)
        .await
        .expect_err("cancel");
    assert!(error.to_string().contains("cancelled"));
    assert!(f.recovery.is_file());
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
    assert_eq!(f.owner.keys.signer.population.load(Ordering::SeqCst), 236);
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 1);
    let restart = f.owner.clone().with_cancellation(CancellationToken::new());
    assert!(
        restart
            .trim_null(NET, TARGET, &f.recovery)
            .await
            .expect_err("record retained")
            .to_string()
            .contains("unresolved submission")
    );
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn preflight_failures_do_not_unlock_keys_or_create_records() -> anyhow::Result<()> {
    for case in 0..8 {
        let mut f = fixture(300, []).await?;
        f.owner.config.operation_timeout = Some(Duration::from_millis(80));
        f.chain.mutate_state(|state| -> anyhow::Result<()> {
            match case {
                0 => state.seed(
                    &st::SUBNET_OWNER,
                    (NET,),
                    DefaultAccount::from(AccountId([2; 32])),
                )?,
                1 => state.seed(&st::MECHANISM_COUNT_CURRENT, (NET,), MechId(0))?,
                2 => seed_target(state, Some(TARGET + 1))?,
                3 => state.seed(&st::PENDING_EPOCH_AT, (NET,), 110u64)?,
                4 => state.seed(
                    &st::TRANSACTION_KEY_LAST_BLOCK,
                    (f.owner.keys.address()?, NET, 9u16),
                    100u64,
                )?,
                5 => state.seed(&st::MAX_ALLOWED_UIDS, (NET,), TARGET - 1)?,
                6 => state.seed(&system::BLOCK_HASH, (0u32,), H256([9; 32]))?,
                _ => state.seed(&st::NETWORKS_ADDED, (NET,), false)?,
            }
            Ok(())
        })?;
        assert!(
            f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err(),
            "case {case}"
        );
        assert!(!f.recovery.exists(), "case {case}");
        assert_eq!(
            f.owner.keys.unlocks.load(Ordering::SeqCst),
            0,
            "case {case}"
        );
        assert_eq!(
            f.chain.mutate_state(|state| state.submissions.len()),
            0,
            "case {case}"
        );
        f.owner.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn root_caps_and_policy_fail_before_private_key_access() -> anyhow::Result<()> {
    let mut f = fixture(300, []).await?;
    for (netuid, target) in [(NetUid(0), TARGET), (NET, 63), (NET, 257)] {
        assert!(
            f.owner
                .trim_null(netuid, target, &f.recovery)
                .await
                .is_err()
        );
    }
    f.owner.executor = f.owner.executor.clone().with_policy(Policy {
        allowed_netuids: Some([NetUid(7)].into_iter().collect()),
        ..Default::default()
    });
    assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(!f.recovery.exists());
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn finalized_dispatch_failure_clears_record_and_does_not_repeat_payment() -> anyhow::Result<()>
{
    for name in ["InvalidValue", "TxRateLimitExceeded"] {
        let f = fixture(300, [Step::Failure(name)]).await?;
        assert!(f.owner.trim_null(NET, TARGET, &f.recovery).await.is_err());
        assert!(!f.recovery.exists());
        assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
        f.owner.client().close().await;
    }
    Ok(())
}

#[tokio::test]
async fn three_confirmed_cleanup_batches_without_progress_stop() -> anyhow::Result<()> {
    let f = fixture(300, [Step::Stall, Step::Stall, Step::Stall]).await?;
    let error = f
        .owner
        .trim_null(NET, TARGET, &f.recovery)
        .await
        .expect_err("stalled");
    assert!(error.to_string().contains("three finalized"));
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 3);
    assert!(!f.recovery.exists());
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn already_complete_uses_storage_without_signing() -> anyhow::Result<()> {
    let f = fixture(TARGET, []).await?;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert_eq!(
        (report.batches, report.removed_uids, report.population),
        (0, 0, TARGET)
    );
    assert!(!f.recovery.exists());
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn already_yuma_is_reported_without_claiming_target_population() -> anyhow::Result<()> {
    let f = fixture(200, []).await?;
    f.chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "SubnetEpochConsensus",
            &[json!(NET.0)],
            json!("Yuma"),
        )
    })?;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert!(report.already_yuma);
    assert_eq!((report.batches, report.population), (0, 200));
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn target_population_with_pending_cleanup_still_submits_a_batch() -> anyhow::Result<()> {
    let f = fixture(TARGET, []).await?;
    f.chain
        .mutate_state(|state| seed_target(state, Some(TARGET)))?;
    let report = f.owner.trim_null(NET, TARGET, &f.recovery).await?;
    assert_eq!(
        (report.batches, report.population, report.removed_uids),
        (1, TARGET, 0)
    );
    assert!(!report.already_yuma);
    assert!(!f.recovery.exists());
    f.owner.client().close().await;
    Ok(())
}

#[test]
fn advisory_lock_prevents_overlap_and_exclusive_record_survives_drop() -> anyhow::Result<()> {
    let temp = Temp::new()?;
    let path = temp.recovery();
    let first = SubmissionJournal::locked(&path)?;
    assert!(SubmissionJournal::locked(&path).is_err());
    fs::write(&path, b"unresolved")?;
    drop(first);
    let second = SubmissionJournal::locked(&path)?;
    assert!(second.check().is_err());
    assert_eq!(fs::read(&path)?, b"unresolved");
    Ok(())
}

#[cfg(unix)]
#[test]
fn recovery_symlinks_are_refused_without_touching_their_target() -> anyhow::Result<()> {
    let temp = Temp::new()?;
    let protected = temp.0.join("protected");
    fs::write(&protected, b"keep")?;
    let path = temp.recovery();
    std::os::unix::fs::symlink(&protected, &path)?;
    assert!(SubmissionJournal::locked(&path).is_err());
    assert_eq!(fs::read(&protected)?, b"keep");
    Ok(())
}
