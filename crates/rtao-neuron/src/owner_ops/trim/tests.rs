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
    Switch,
    Approval(AccountId, H256, u32, u32),
    ApprovalChanged(AccountId, H256, u32, u32, ApprovalChange),
}

#[derive(Clone, Copy)]
enum ApprovalChange {
    Population,
    PendingTarget,
    Consensus,
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
            assert!(matches!(
                record["operation"].as_str(),
                Some("trim_null_subnet_batch" | "set_hyperparameter")
            ));
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
                if matches!(step, Step::Switch) {
                    assert_eq!(record["operation"], json!("set_hyperparameter"));
                    state.seed_python("SubtensorModule", "SubnetEpochConsensus", &[json!(NET.0)], json!("Yuma")).expect("Yuma seed");
                    state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([success()]) });
                    return;
                }
                if let Step::Approval(group, hash, height, index)
                    | Step::ApprovalChanged(group, hash, height, index, _) = step {
                    let hash = format!("0x{}", hex::encode(hash.0));
                    state.seed_python("Multisig", "Multisigs", &[json!(group), json!(hash)], json!({
                        "when":{"height":height,"index":index}, "deposit":280_800_000,
                        "depositor":self.account(), "approvals":[self.account()],
                    })).expect("pending approval seed");
                    if let Step::ApprovalChanged(_, _, _, _, change) = step {
                        match change {
                            ApprovalChange::Population => state.seed(&st::SUBNETWORK_N, (NET,), TARGET + 1).expect("changed population"),
                            ApprovalChange::PendingTarget => seed_target(state, Some(TARGET)).expect("changed target"),
                            ApprovalChange::Consensus => state.seed_python("SubtensorModule", "SubnetEpochConsensus", &[json!(NET.0)], json!("Yuma")).expect("changed consensus"),
                        }
                    }
                    state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([success(), {
                        "phase":"ApplyExtrinsic", "extrinsic_idx":0, "module_id":"Multisig", "event_id":"NewMultisig",
                        "attributes":{"approving":self.account(),"multisig":group,"call_hash":hash}, "topics":[],
                    }]) });
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

#[tokio::test]
async fn preview_checks_recovery_lock_without_creating_a_submission() -> anyhow::Result<()> {
    let f = fixture(300, []).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig {
            preview: true,
            ..Default::default()
        },
    )
    .await?;
    let TrimNullStep::Submit(action) = session.next_action().await? else {
        anyhow::bail!("expected preview action");
    };
    assert!(!f.recovery.exists());
    assert!(SubmissionJournal::locked(&f.recovery).is_err());
    let TrimActionKind::Batch(intent) = action.kind.as_ref() else {
        anyhow::bail!("expected guarded batch");
    };
    let plan = f
        .owner
        .executor
        .plan(intent, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    assert!(session.before_attempt(&action, &plan).is_err());
    assert!(!f.recovery.exists());
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    drop(session);
    fs::write(&f.recovery, b"unresolved")?;
    assert!(
        TrimNullSession::begin(
            f.owner.client().clone(),
            f.owner.effective_owner()?,
            NET,
            Some(TARGET),
            &f.recovery,
            TrimNullConfig {
                preview: true,
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn opt_in_yuma_switch_reuses_guarded_review_and_canonical_finality() -> anyhow::Result<()> {
    let f = fixture(TARGET, [Step::Switch]).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig {
            switch_to_yuma: true,
            ..Default::default()
        },
    )
    .await?;
    let TrimNullStep::Submit(action) = session.next_action().await? else {
        anyhow::bail!("expected Yuma action");
    };
    assert_eq!(action.operation(), "set_hyperparameter");
    let TrimActionKind::Switch(intent) = action.kind.as_ref() else {
        anyhow::bail!("expected guarded setter");
    };
    let plan = f
        .owner
        .executor
        .plan(intent, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    assert!(plan.ok(), "{:?}", plan.violations);
    session.before_attempt(&action, &plan)?;
    let operation = f.owner.operation()?;
    let receipt = operation
        .execute_trim_reviewed(intent, &ExecuteOptions::default(), &plan)
        .await?;
    session.finish_action(action, &receipt).await?;
    let TrimNullStep::Complete(report) = session.next_action().await? else {
        anyhow::bail!("expected finalized completion");
    };
    assert!(report.switched_to_yuma);
    assert!(!report.already_yuma);
    assert_eq!(report.batches, 0);
    assert!(!f.recovery.exists());
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn yuma_switch_probes_legacy_queues_and_rechecks_before_signing() -> anyhow::Result<()> {
    let f = fixture(TARGET, []).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig {
            switch_to_yuma: true,
            ..Default::default()
        },
    )
    .await?;
    let TrimNullStep::Submit(switch) = session.next_action().await? else {
        anyhow::bail!("expected setter");
    };
    f.chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "WeightCommits",
            &[json!(NET.0 + 15 * 4096), json!(AccountId([9; 32]))],
            json!([[format!("0x{}", hex::encode([3; 32])), 1, 2, 3]]),
        )
    })?;
    let TrimActionKind::Switch(intent) = switch.kind.as_ref() else {
        anyhow::bail!("expected guarded setter");
    };
    let plan = f
        .owner
        .executor
        .plan(intent, f.owner.keys.as_ref(), &PlanOptions::default())
        .await;
    assert!(plan.is_err() || plan.is_ok_and(|p| !p.ok()));
    let TrimNullStep::Submit(cleanup) = session.next_action().await? else {
        anyhow::bail!("queued commits need a bounded batch");
    };
    assert_eq!(cleanup.operation(), "trim_null_subnet_batch");
    assert!(!f.recovery.exists());
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    f.owner.client().close().await;
    Ok(())
}

#[derive(Clone)]
struct ApprovalIntent {
    semantic: Json,
    inner: rtao_runtime::CallBytes,
    other: AccountId,
    timepoint: Option<rtao_runtime::models::MultisigTimepoint>,
}
impl IntentInterface for ApprovalIntent {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[31];
    type Role = Coldkey;
    type Origin = rtao_tx::Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::multisig::AS_MULTI];
    fn to_json(&self) -> Json {
        json!({"op":"multisig_execute", "threshold":2, "other_signatories":[self.other],
            "call":self.semantic, "timepoint":self.timepoint})
    }
}
impl Intent for ApprovalIntent {
    fn summary(&self) -> String {
        "Approve retained pruning call".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let weight = DynValue::new(Value::record(vec![
            ("ref_time".into(), Value::Uint(1)),
            ("proof_size".into(), Value::Uint(0)),
        ]));
        let call = rtao_runtime::calls::multisig::as_multi(
            2,
            vec![self.other],
            self.timepoint.clone(),
            self.inner.clone(),
            weight,
        )
        .compose(cx.runtime)?;
        let mut built = Built::new(call);
        built.extras.insert(
            "multisig_call_hash".into(),
            json!(format!("0x{}", hex::encode(self.inner.hash()))),
        );
        built.extras.insert(
            "multisig_call_data".into(),
            json!(format!("0x{}", hex::encode(self.inner.as_bytes()))),
        );
        Ok(built)
    }
}

#[tokio::test]
async fn finalized_pending_approval_retains_one_journal_and_requires_same_timepoint()
-> anyhow::Result<()> {
    let f = fixture(300, []).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig::default(),
    )
    .await?;
    let TrimNullStep::Submit(action) = session.next_action().await? else {
        anyhow::bail!("expected batch");
    };
    let runtime = f.owner.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::admin_utils::sudo_trim_null_uids_batch(NET, TARGET)
        .compose(&runtime)?;
    let other = AccountId([9; 32]);
    let group = rtao_tx::multisig::Multisig::new(&runtime, &[f.owner.keys.address()?, other], 2)?;
    f.owner
        .keys
        .signer
        .steps
        .lock()
        .expect("steps")
        .push_back(Step::Approval(group.address, H256(inner.hash()), 100, 0));
    let mut approval = ApprovalIntent {
        semantic: action.intent().to_json_dyn(),
        inner,
        other,
        timepoint: None,
    };
    let plan = f
        .owner
        .executor
        .plan(&approval, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    session.before_attempt(&action, &plan)?;
    assert!(
        session.before_attempt(&action, &plan).is_err(),
        "unresolved attempt blocks repeat signing"
    );
    let receipt = f
        .owner
        .executor
        .execute(&approval, f.owner.keys.as_ref(), &ExecuteOptions::default())
        .await?;
    let error = session
        .finish_action(action.clone(), &receipt)
        .await
        .expect_err("approval is not semantic execution");
    assert!(error.to_string().contains("remaining approvals"));
    assert_eq!(session.report().batches, 0);
    let recorded: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
    assert_eq!(recorded["approval_confirmed"], json!(true));
    assert_eq!(recorded["receipt"]["inclusion"], json!("Finalized"));
    assert_eq!(
        recorded["receipt"]["events"].as_array().map(Vec::len),
        Some(receipt.events.len())
    );
    assert_eq!(
        recorded["pending_timepoint"],
        json!({"height":100,"index":0})
    );
    drop(session);
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig::default(),
    )
    .await?;
    assert!(
        session.before_attempt(&action, &plan).is_err(),
        "null opening cannot reopen retained operation"
    );
    approval.timepoint = Some(rtao_runtime::models::MultisigTimepoint {
        height: 100,
        index: 0,
    });
    let followup = f
        .owner
        .executor
        .plan(&approval, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    session.before_attempt(&action, &followup)?;
    let updated: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
    assert_eq!(updated["attempts"].as_array().map(Vec::len), Some(2));
    assert_eq!(updated["attempts"][0]["receipt"], recorded["receipt"]);
    assert_eq!(
        updated["call_hex"],
        json!(format!("0x{}", hex::encode(followup.call().as_bytes())))
    );
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
    assert!(!f.recovery.with_file_name("pruning.json.next").exists());
    f.owner
        .keys
        .signer
        .steps
        .lock()
        .expect("steps")
        .push_back(Step::Failure("InvalidValue"));
    let failed = f
        .owner
        .executor
        .execute(&approval, f.owner.keys.as_ref(), &ExecuteOptions::default())
        .await?;
    assert!(session.finish_action(action, &failed).await.is_err());
    let retained: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
    assert_eq!(retained["attempts"][0]["receipt"], recorded["receipt"]);
    assert_eq!(retained["pending_timepoint"], recorded["pending_timepoint"]);
    assert_eq!(retained["receipt"]["success"], json!(false));
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 2);
    f.owner.client().close().await;
    Ok(())
}

struct CountingSigner {
    key: KeypairSigner,
    signs: Arc<AtomicUsize>,
}
impl Signer for CountingSigner {
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
            self.signs.fetch_add(1, Ordering::SeqCst);
            self.key.sign(request).await
        })
    }
}

#[tokio::test]
async fn reviewed_signer_refuses_changed_call_or_runtime_before_underlying_signing()
-> anyhow::Result<()> {
    let f = fixture(300, []).await?;
    let runtime = f.owner.client().refresh_runtime().await?;
    let call = rtao_runtime::calls::admin_utils::sudo_trim_null_uids_batch(NET, TARGET)
        .compose(&runtime)?;
    let signs = Arc::new(AtomicUsize::new(0));
    let signer = ReviewedSigner {
        inner: Arc::new(CountingSigner {
            key: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?,
            signs: signs.clone(),
        }),
        call: call.clone(),
        genesis: f.owner.client().genesis(),
    };
    let context = rtao_signer::SigningContext {
        metadata_bytes: runtime.core().metadata_bytes.clone(),
        chain_info: rtao_core::digest::ChainInfo {
            spec_version: runtime.spec_version(),
            spec_name: "node-subtensor".into(),
            base58_prefix: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        },
        transaction_version: runtime.transaction_version(),
        genesis_hash: f.owner.client().genesis().0,
    };
    let payload = json!({"specVersion":format!("0x{:08x}", runtime.spec_version())});
    for case in 0..6 {
        let mut changed_context = context.clone();
        let mut changed_payload = payload.as_object().expect("payload").clone();
        let mut bytes = call.as_bytes().to_vec();
        match case {
            0 => bytes.push(0),
            1 => changed_context.genesis_hash = [9; 32],
            2 => changed_context.transaction_version += 1,
            3 => changed_context.chain_info.spec_version += 1,
            4 => {
                changed_payload.insert("specVersion".into(), json!(runtime.spec_version() + 1));
            }
            _ => {}
        }
        let request = SigningRequest {
            call: &bytes,
            included_in_extrinsic: &[],
            included_in_signed_data: &[],
            payload: b"reviewed payload",
            payload_json: &changed_payload,
            context: &changed_context,
            purpose: rtao_signer::SignPurpose::Extrinsic,
        };
        assert_eq!(signer.sign(&request).await.is_ok(), case == 5);
        assert_eq!(signs.load(Ordering::SeqCst), usize::from(case == 5));
    }
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn sdk_reviewed_execution_refuses_changed_call_before_private_signer_acquisition()
-> anyhow::Result<()> {
    let f = fixture(300, []).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig::default(),
    )
    .await?;
    let TrimNullStep::Submit(action) = session.next_action().await? else {
        anyhow::bail!("expected guarded batch");
    };
    let TrimActionKind::Batch(reviewed) = action.kind.as_ref() else {
        anyhow::bail!("expected guarded batch");
    };
    let plan = f
        .owner
        .executor
        .plan(reviewed, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    session.before_attempt(&action, &plan)?;
    let recorded = fs::read(&f.recovery)?;
    let mut changed = reviewed.clone();
    changed.batch.target = TARGET + 1;
    let error = f
        .owner
        .operation()?
        .execute_trim_reviewed(&changed, &ExecuteOptions::default(), &plan)
        .await
        .expect_err("a rebuilt call requires new review before private acquisition");
    assert!(error.to_string().contains("differs from the reviewed call"));
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(f.owner.keys.signer.calls.lock().expect("calls").is_empty());
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 0);
    assert_eq!(fs::read(&f.recovery)?, recorded);
    f.owner.client().close().await;
    Ok(())
}

async fn pending_hook_fixture(
    change: Option<ApprovalChange>,
) -> anyhow::Result<(
    Fixture,
    TrimNullSession,
    TrimNullAction,
    ApprovalIntent,
    Plan,
)> {
    let f = fixture(200, []).await?;
    let mut session = TrimNullSession::begin(
        f.owner.client().clone(),
        f.owner.effective_owner()?,
        NET,
        Some(TARGET),
        &f.recovery,
        TrimNullConfig::default(),
    )
    .await?;
    let TrimNullStep::Submit(action) = session.next_action().await? else {
        anyhow::bail!("expected guarded batch");
    };
    let runtime = f.owner.client().refresh_runtime().await?;
    let inner = rtao_runtime::calls::admin_utils::sudo_trim_null_uids_batch(NET, TARGET)
        .compose(&runtime)?;
    let other = AccountId([9; 32]);
    let group = rtao_tx::multisig::Multisig::new(&runtime, &[f.owner.keys.address()?, other], 2)?;
    let scripted = match change {
        Some(change) => Step::ApprovalChanged(group.address, H256(inner.hash()), 100, 0, change),
        None => Step::Approval(group.address, H256(inner.hash()), 100, 0),
    };
    f.owner
        .keys
        .signer
        .steps
        .lock()
        .expect("steps")
        .push_back(scripted);
    let approval = ApprovalIntent {
        semantic: action.intent().to_json_dyn(),
        inner,
        other,
        timepoint: None,
    };
    let plan = f
        .owner
        .executor
        .plan(&approval, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    Ok((f, session, action, approval, plan))
}

#[tokio::test]
async fn reviewed_receipt_hook_allows_next_approval_without_finishing_action() -> anyhow::Result<()>
{
    let (f, session, action, mut approval, opening_plan) = pending_hook_fixture(None).await?;
    session.before_attempt(&action, &opening_plan)?;
    let receipt = f
        .owner
        .executor
        .execute(&approval, f.owner.keys.as_ref(), &ExecuteOptions::default())
        .await?;
    session.record_reviewed_result(&action, &receipt).await?;
    let opening: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
    assert_eq!(opening["approval_confirmed"], json!(true));
    assert_eq!(
        opening["pending_timepoint"],
        json!({"height":100,"index":0})
    );
    assert_eq!(opening["receipt"]["inclusion"], json!("Finalized"));
    assert_eq!(opening["attempts"].as_array().map(Vec::len), Some(1));
    assert_eq!(session.report().batches, 0);
    assert_eq!(session.report().population, 200);
    assert_eq!(session.report().last_extrinsic_id, None);

    approval.timepoint = Some(rtao_runtime::models::MultisigTimepoint {
        height: 100,
        index: 0,
    });
    let next_plan = f
        .owner
        .executor
        .plan(&approval, f.owner.keys.as_ref(), &PlanOptions::default())
        .await?;
    // No finish_action call or new session occurs between the two approvals.
    session.before_attempt(&action, &next_plan)?;
    let next: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
    assert_eq!(next["attempts"].as_array().map(Vec::len), Some(2));
    assert_eq!(next["attempts"][0], opening["attempts"][0]);
    assert_eq!(next["pending_timepoint"], opening["pending_timepoint"]);
    assert_eq!(next["multisig"], opening["multisig"]);
    assert_eq!(next["semantic_args"], opening["semantic_args"]);
    assert_eq!(next["approval_confirmed"], json!(false));
    assert_eq!(
        next["attempts"][1]["reviewed_args"]["timepoint"],
        opening["pending_timepoint"]
    );
    assert_eq!(
        next["call_hex"],
        json!(format!("0x{}", hex::encode(next_plan.call().as_bytes())))
    );
    assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
    assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 1);
    assert!(!f.recovery.with_file_name("pruning.json.next").exists());
    f.owner.client().close().await;
    Ok(())
}

#[tokio::test]
async fn reviewed_pending_approval_with_changed_pruning_state_retains_receipt_and_refuses_next()
-> anyhow::Result<()> {
    for change in [
        ApprovalChange::Population,
        ApprovalChange::PendingTarget,
        ApprovalChange::Consensus,
    ] {
        let (f, session, action, mut approval, opening_plan) =
            pending_hook_fixture(Some(change)).await?;
        session.before_attempt(&action, &opening_plan)?;
        let receipt = f
            .owner
            .executor
            .execute(&approval, f.owner.keys.as_ref(), &ExecuteOptions::default())
            .await?;
        let error = session
            .record_reviewed_result(&action, &receipt)
            .await
            .expect_err("a pending approval cannot change pruning state");
        assert!(error.to_string().contains("pruning state changed"));
        let retained: Json = serde_json::from_slice(&fs::read(&f.recovery)?)?;
        assert_eq!(retained["approval_confirmed"], json!(false));
        assert_eq!(retained["attempts"].as_array().map(Vec::len), Some(1));
        assert_eq!(retained["receipt"]["success"], json!(true));
        assert_eq!(retained["receipt"]["inclusion"], json!("Finalized"));
        assert_eq!(
            retained["receipt"]["events"].as_array().map(Vec::len),
            Some(receipt.events.len())
        );
        assert_eq!(retained["attempts"][0]["receipt"], retained["receipt"]);
        assert_eq!(
            retained["call_hex"],
            json!(format!("0x{}", hex::encode(opening_plan.call().as_bytes())))
        );
        assert_eq!(session.report().batches, 0);
        assert_eq!(session.report().population, 200);
        approval.timepoint = Some(rtao_runtime::models::MultisigTimepoint {
            height: 100,
            index: 0,
        });
        let next_plan = f
            .owner
            .executor
            .plan(&approval, f.owner.keys.as_ref(), &PlanOptions::default())
            .await?;
        assert!(session.before_attempt(&action, &next_plan).is_err());
        assert_eq!(
            serde_json::from_slice::<Json>(&fs::read(&f.recovery)?)?,
            retained
        );
        assert_eq!(f.chain.mutate_state(|state| state.submissions.len()), 1);
        assert_eq!(f.owner.keys.unlocks.load(Ordering::SeqCst), 1);
        f.owner.client().close().await;
    }
    Ok(())
}
