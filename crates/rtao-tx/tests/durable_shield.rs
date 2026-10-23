//! Shared public pipeline on the explicitly isolated synthetic genesis.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use rtao_client::{Client, ClientConfig, Network, ShieldPeriod, Wait, journal::JournalControl};
use rtao_primitives::{Balance, CryptoType, Spend};
use rtao_signer::Coldkey;
use rtao_testkit::{
    FakeChain, FakeState,
    journal_shield::{Inner, JournalLab, LabKeys, Stage},
};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Executor, Intent, IntentError, IntentInterface, IntentOutput,
    IntentPreflight, PreflightCx, ShieldOptions, Signed, TxError,
};
use serde_json::{Map, Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
static DECODED: AtomicU64 = AtomicU64::new(0);
fn path() -> PathBuf {
    PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").unwrap())
        .join(format!(
            "public-shield-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ))
        .join("journal.json")
}
struct Output;
impl IntentOutput for Output {
    fn from_data(data: &Map<String, Value>) -> Result<Self, IntentError> {
        DECODED.fetch_add(1, Ordering::SeqCst);
        if data.get("planned_marker") != Some(&json!("persisted-public-context")) {
            return Err(IntentError::InvalidArgument(
                "missing actual persisted output".into(),
            ));
        }
        Ok(Self)
    }
}
struct Probe {
    builds: Arc<AtomicU64>,
}
impl IntentInterface for Probe {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[0];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = Output;
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":Self::SPEC.name,"fixture":"durable-public"})
    }
}
impl Intent for Probe {
    fn summary(&self) -> String {
        "synthetic durable operation".into()
    }
    fn spend(&self) -> Spend {
        Spend::Bounded(Balance::from_rao(10))
    }
    async fn build<V: rtao_client::View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<Built, IntentError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let mut built =
            Built::new(rtao_runtime::calls::system::remark(vec![1, 2, 3]).compose(cx.runtime)?);
        built
            .extras
            .insert("planned_marker".into(), json!("persisted-public-context"));
        Ok(built)
    }
    async fn preflight<V: rtao_client::View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            required_free: Some(Balance::from_rao(10)),
            available_free: Some(Balance::from_rao(1_000_000_000)),
            ..Default::default()
        })
    }
}
fn options(binding: rtao_client::journal::JournalBinding) -> ShieldOptions {
    ShieldOptions {
        period: ShieldPeriod::P4,
        dropped_inner_retries: 1,
        execute: ExecuteOptions {
            journal: Some(binding),
            deadline: None,
            submit_deadline: None,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn public_typed_dynamic_and_raw_lost_ack_restart_restore_original_context_without_keys_or_resend()
 {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        for route in ["typed", "dynamic", "raw"] {
            DECODED.store(0, Ordering::SeqCst);
            let mut lab = JournalLab::new(FakeState::default(), path(), crypto)
                .await
                .unwrap();
            lab.script([Inner::LostAcknowledgment]);
            let keys = LabKeys::new(lab.signer.clone());
            let probe = Probe {
                builds: Arc::new(AtomicU64::new(0)),
            };
            let binding = lab
                .begin(
                    "lost-ack",
                    probe.to_json(),
                    json!({"required":"retained-material"}),
                )
                .unwrap();
            let opts = options(binding);
            let ex = Executor::new(lab.client.clone());
            let pending = {
                let operation = async {
                    match route {
                        "typed" => ex.submit_shielded(&probe, &keys, &opts).await.map(|_| ()),
                        "dynamic" => ex
                            .submit_shielded_dyn(&probe, &keys, &opts)
                            .await
                            .map(|_| ()),
                        _ => {
                            let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
                                .compose(&lab.client.refresh_runtime().await.unwrap())
                                .unwrap();
                            let mut raw = opts.execute.clone();
                            raw.shielded = true;
                            raw.period = Some(4);
                            ex.submit_call::<Coldkey, _>(call, &keys, &raw)
                                .await
                                .map(|_| ())
                        }
                    }
                };
                tokio::pin!(operation);
                tokio::select! {
                    early = &mut operation => panic!("submission returned before lost acknowledgment: {early:?}"),
                    _ = lab.lost_acknowledgment() => {
                        lab.node.drop_connection().unwrap();
                        operation.await
                    }
                }
            };
            assert!(
                matches!(pending, Err(TxError::PendingOperation { .. })),
                "{route}: unexpected pending state: {pending:?}"
            );
            assert_eq!(
                DECODED.load(Ordering::SeqCst),
                0,
                "pending must not decode typed outputs"
            );
            assert_eq!(lab.authors(), 1);
            assert_eq!(keys.unlocks(), 1);
            assert_eq!(lab.signer.signatures(), 2);
            let builds = probe.builds.load(Ordering::SeqCst);
            drop(opts);
            drop(ex);
            // Historical effect uses the old execution context even while
            // today's code is deliberately unqualified for new submissions.
            lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
                .unwrap();
            lab.restart().await.unwrap();
            let opts = options(lab.bind("lost-ack").unwrap());
            let ex = Executor::new(lab.client.clone());
            let recovered = match route {
                "typed" => ex
                    .submit_shielded(&probe, &keys, &opts)
                    .await
                    .map(|r| (r.success, r.extrinsic_hash, r.data)),
                "dynamic" => ex
                    .submit_shielded_dyn(&probe, &keys, &opts)
                    .await
                    .map(|r| (r.success, r.extrinsic_hash, r.data)),
                _ => {
                    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
                        .compose(&lab.client.refresh_runtime().await.unwrap())
                        .unwrap();
                    let mut raw = opts.execute.clone();
                    raw.shielded = true;
                    raw.period = Some(4);
                    ex.submit_call::<Coldkey, _>(call, &keys, &raw)
                        .await
                        .map(|r| (r.success, r.extrinsic_hash, r.data))
                }
            }
            .unwrap();
            assert!(recovered.0);
            assert!(recovered.1.is_some());
            assert_eq!(recovered.2["shielded"], true);
            if route != "raw" {
                assert_eq!(recovered.2["planned_marker"], "persisted-public-context");
            }
            assert_eq!(lab.authors(), 1);
            assert_eq!(keys.unlocks(), 1);
            assert_eq!(lab.signer.signatures(), 2);
            assert_eq!(
                probe.builds.load(Ordering::SeqCst),
                builds,
                "recovery must not rebuild in today's runtime"
            );
            assert_eq!(DECODED.load(Ordering::SeqCst), u64::from(route == "typed"));
            drop(opts);
            lab.close().await;
        }
    }
}

#[tokio::test]
async fn public_reseal_is_typed_bounded_and_failed_inner_is_never_replaced() {
    for failed in [false, true] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        if failed {
            lab.script([Inner::Include(false)]);
        } else {
            lab.script([Inner::Absent, Inner::Include(true)]);
        }
        let keys = LabKeys::new(lab.signer.clone());
        let probe = Probe {
            builds: Arc::new(AtomicU64::new(0)),
        };
        let opts = options(
            lab.begin("reseal", probe.to_json(), json!({"intent":"preserved"}))
                .unwrap(),
        );
        let result = Executor::new(lab.client.clone())
            .submit_shielded_dyn(&probe, &keys, &opts)
            .await;
        let result = result.unwrap();
        assert_eq!(result.success, !failed);
        assert_eq!(lab.authors(), if failed { 1 } else { 2 });
        assert_eq!(lab.signer.signatures(), if failed { 2 } else { 4 });
        assert!(result.fate_unknown.is_none());
        let record = lab.bind("reseal").unwrap().record().unwrap();
        assert!(record.finalized_effect().unwrap().is_some());
        if !failed {
            assert!(
                record.shield_pairs[0]
                    .extensions
                    .contains_key("rtao_shield_absence_v1")
            );
        }
        drop(opts);
        lab.close().await;
    }
}

#[tokio::test]
async fn public_pause_cancel_and_competing_preparation_preserve_original_work() {
    for stage in [Stage::Prepare, Stage::Inner, Stage::Carrier] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        let keys = Arc::new(LabKeys::new(lab.signer.clone()));
        let probe = Arc::new(Probe {
            builds: Arc::new(AtomicU64::new(0)),
        });
        let pause = lab.signer.pause(stage);
        let opts = options(
            lab.begin("cancelled", probe.to_json(), json!({"original":"material"}))
                .unwrap(),
        );

        let ex = Executor::new(lab.client.clone());
        let task_keys = keys.clone();
        let task_probe = probe.clone();
        let mut task = tokio::spawn(async move {
            ex.submit_shielded(task_probe.as_ref(), task_keys.as_ref(), &opts)
                .await
                .map(|r| r.success)
        });
        tokio::select! { _=pause.entered()=>{}, early=&mut task=>panic!("submission ended before signer gate: {early:?}") }
        assert_eq!(
            lab.bind("cancelled")
                .unwrap()
                .record()
                .unwrap()
                .attempts
                .len(),
            usize::from(stage == Stage::Carrier)
        );
        let other_keys = LabKeys::new(lab.signer.clone());
        let other = options(
            lab.begin("competing", probe.to_json(), Value::Null)
                .unwrap(),
        );
        assert!(
            Executor::new(lab.client.clone())
                .submit_shielded_dyn(probe.as_ref(), &other_keys, &other)
                .await
                .is_err()
        );
        assert_eq!(other_keys.unlocks(), 0);
        assert_eq!(lab.authors(), 0);
        drop(other);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        lab.journal()
            .unwrap()
            .control(JournalControl {
                submissions_paused: true,
                allow_obligations: false,
                extensions: Default::default(),
            })
            .unwrap();
        lab.restart().await.unwrap();
        let paused = options(lab.bind("cancelled").unwrap());
        let unlocks = keys.unlocks();
        assert!(
            Executor::new(lab.client.clone())
                .submit_shielded_dyn(probe.as_ref(), keys.as_ref(), &paused)
                .await
                .is_err()
        );
        assert_eq!(keys.unlocks(), unlocks);
        assert_eq!(lab.authors(), 0);
        drop(paused);
        lab.journal().unwrap().control(Default::default()).unwrap();
        lab.node.mutate_state(|state| {
            state.advance(1);
        });
        let resumed = options(lab.bind("cancelled").unwrap());
        assert!(
            Executor::new(lab.client.clone())
                .submit_shielded_dyn(probe.as_ref(), keys.as_ref(), &resumed)
                .await
                .unwrap()
                .success
        );
        assert_eq!(lab.authors(), 1);
        let record = lab.bind("cancelled").unwrap().record().unwrap();
        assert_eq!(
            record.shield_pairs.len(),
            if stage == Stage::Carrier { 2 } else { 1 }
        );
        if stage == Stage::Carrier {
            assert!(matches!(
                record.shield_pairs[0].phase,
                rtao_client::journal::PairPhase::Retired
            ));
        }
        assert_eq!(
            record.intent.correlation_id.as_deref(),
            Some("application:cancelled")
        );
        drop(resumed);
        lab.close().await;
    }
}

#[tokio::test]
async fn production_genesis_and_nonfinal_waits_cannot_use_synthetic_dispatch() {
    let fake = FakeChain::from_state(FakeState::default());
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(rtao_testkit::rpc::RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    assert!(client.enable_synthetic_journal_dispatch().await.is_err());
    assert!(client.check_journal_shield_dispatch().is_err());
    client.close().await;
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    let keys = LabKeys::new(lab.signer.clone());
    let probe = Probe {
        builds: Arc::new(AtomicU64::new(0)),
    };
    for wait in [Wait::Pool, Wait::Inclusion] {
        let mut opts = options(
            lab.begin(&format!("nonfinal-{wait:?}"), probe.to_json(), Value::Null)
                .unwrap(),
        );
        opts.execute.wait_for_finalization = false;
        opts.execute.wait_for_inclusion = wait == Wait::Inclusion;
        assert!(
            Executor::new(lab.client.clone())
                .submit_shielded_dyn(&probe, &keys, &opts)
                .await
                .is_err()
        );
    }
    assert_eq!(keys.unlocks(), 0);
    assert_eq!(lab.signer.signatures(), 0);
    assert_eq!(lab.authors(), 0);
    lab.close().await;
}

#[tokio::test]
async fn public_submission_future_sizes_fit_native_caller_stacks() {
    fn size<F: std::future::Future>(_: impl FnOnce() -> F) -> usize {
        std::mem::size_of::<F>()
    }
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    let keys = LabKeys::new(lab.signer.clone());
    let probe = Probe {
        builds: Arc::new(AtomicU64::new(0)),
    };
    let opts = options(lab.begin("size", probe.to_json(), Value::Null).unwrap());
    let ex = Executor::new(lab.client.clone());
    // Causal regression: embedding the 72KiB driver recursively in public
    // futures overflowed the default native test-thread stack before dispatch.
    assert!(size(|| ex.execute(&probe, &keys, &opts.execute)) < 4096);
    assert!(size(|| ex.execute_dyn(&probe, &keys, &opts.execute)) < 4096);
    assert!(size(|| ex.submit_shielded(&probe, &keys, &opts)) < 4096);
    assert!(size(|| ex.submit_shielded_dyn(&probe, &keys, &opts)) < 4096);
    let call = rtao_runtime::calls::system::remark(vec![])
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
    assert!(size(|| ex.submit_call::<Coldkey, _>(call, &keys, &opts.execute)) < 4096);
    lab.close().await;
}

#[tokio::test]
async fn public_upgrade_rebuilds_unsubmitted_work_at_each_signer_boundary() {
    for stage in [Stage::Prepare, Stage::Inner, Stage::Carrier] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        let keys = Arc::new(LabKeys::new(lab.signer.clone()));
        let probe = Arc::new(Probe {
            builds: Arc::new(AtomicU64::new(0)),
        });
        let pause = lab.signer.pause(stage);
        let mut opts = options(
            lab.begin("upgrade", probe.to_json(), json!({"original":"kept"}))
                .unwrap(),
        )
        .execute;
        opts.shielded = true;
        opts.period = Some(4);
        let ex = Executor::new(lab.client.clone());
        let k = keys.clone();
        let p = probe.clone();
        let mut task =
            tokio::spawn(async move { ex.execute_dyn(p.as_ref(), k.as_ref(), &opts).await });
        tokio::select! { _=pause.entered()=>{}, early=&mut task=>panic!("submission ended before signer gate: {early:?}") }
        assert_eq!(lab.authors(), 0);
        lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
            .unwrap();
        lab.protocol.allow(rtao_primitives::H256([43; 32]));
        pause.release();
        let result = task.await.unwrap().unwrap();
        assert!(result.success);
        let record = lab.bind("upgrade").unwrap().record().unwrap();
        assert_eq!(
            record.finalized_effect().unwrap().unwrap().spec_version,
            482
        );
        assert_eq!(
            record
                .finalized_effect()
                .unwrap()
                .unwrap()
                .transaction_version,
            2
        );
        assert_eq!(record.material, json!({"original":"kept"}));
        assert_eq!(lab.authors(), 1);
        assert_eq!(keys.unlocks(), 1);
        assert_eq!(probe.builds.load(Ordering::SeqCst), 2);
        assert!(
            record
                .shield_pairs
                .iter()
                .filter(|p| matches!(
                    p.phase,
                    rtao_client::journal::PairPhase::DispatchAuthorized { .. }
                ))
                .count()
                == 1
        );
        lab.close().await;
    }
}

#[tokio::test]
async fn unqualified_upgrade_keeps_structured_reason_and_resumes_same_operation_after_adapter_review()
 {
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Sr25519)
        .await
        .unwrap();
    let keys = LabKeys::new(lab.signer.clone());
    let probe = Probe {
        builds: Arc::new(AtomicU64::new(0)),
    };
    let opts = options(
        lab.begin("adapter-review", probe.to_json(), json!({"generation":1}))
            .unwrap(),
    );
    lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
        .unwrap();
    assert!(
        Executor::new(lab.client.clone())
            .submit_shielded_dyn(&probe, &keys, &opts)
            .await
            .is_err()
    );
    assert_eq!(
        lab.bind("adapter-review")
            .unwrap()
            .record()
            .unwrap()
            .incompatibility
            .unwrap()
            .code,
        "synthetic-code-unqualified"
    );
    assert_eq!(keys.unlocks(), 0);
    assert_eq!(lab.authors(), 0);
    assert_eq!(probe.builds.load(Ordering::SeqCst), 0);
    lab.protocol.allow(rtao_primitives::H256([43; 32]));
    assert!(
        Executor::new(lab.client.clone())
            .submit_shielded_dyn(&probe, &keys, &opts)
            .await
            .unwrap()
            .success
    );
    let record = lab.bind("adapter-review").unwrap().record().unwrap();
    assert!(record.incompatibility.is_none());
    assert_eq!(record.intent.id, "adapter-review");
    assert_eq!(record.material, json!({"generation":1}));
    assert_eq!(lab.authors(), 1);
    drop(opts);
    lab.close().await;
}

#[tokio::test]
async fn public_qualified_negative_pair_budget_cannot_be_renewed_by_recovery_or_restart() {
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    lab.script([Inner::Absent, Inner::Absent]);
    let keys = LabKeys::new(lab.signer.clone());
    let probe = Probe {
        builds: Arc::new(AtomicU64::new(0)),
    };
    let opts = options(
        lab.begin("budget", probe.to_json(), json!({"intent":"kept"}))
            .unwrap(),
    );
    let result = Executor::new(lab.client.clone())
        .submit_shielded_dyn(&probe, &keys, &opts)
        .await;
    assert!(
        matches!(
            result,
            Err(TxError::PendingOperation {
                reason: rtao_client::journal::RecoveryReason::ShieldPairBudgetExhausted {
                    maximum_pairs: 2,
                    consumed: 2
                },
                ..
            })
        ),
        "{result:?}"
    );
    assert_eq!(lab.authors(), 2);
    assert_eq!(lab.signer.signatures(), 4);
    drop(opts);
    lab.restart().await.unwrap();
    let opts = options(lab.bind("budget").unwrap());
    let unlocks = keys.unlocks();
    for _ in 0..2 {
        assert!(matches!(
            Executor::new(lab.client.clone())
                .submit_shielded_dyn(&probe, &keys, &opts)
                .await,
            Err(TxError::PendingOperation {
                reason: rtao_client::journal::RecoveryReason::ShieldPairBudgetExhausted {
                    maximum_pairs: 2,
                    consumed: 2
                },
                ..
            })
        ));
    }
    let record = lab.bind("budget").unwrap().record().unwrap();
    assert_eq!(record.shield_pairs.len(), 2);
    assert_eq!(record.attempts.len(), 4);
    assert!(
        record
            .shield_pairs
            .iter()
            .all(|p| p.extensions.contains_key("rtao_shield_absence_v1"))
    );
    assert_eq!(keys.unlocks(), unlocks);
    assert_eq!(lab.authors(), 2);
    assert_eq!(lab.signer.signatures(), 4);
    drop(opts);
    lab.close().await;
}

#[tokio::test(start_paused = true)]
async fn original_whole_operation_deadline_cancels_reseal_before_any_further_signature() {
    // Real socket I/O must not auto-advance virtual time while awaiting the
    // peer. Keep this runtime runnable until the causal reseal stage is reached.
    let keepalive = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    lab.script([Inner::Absent]);
    let second = Arc::new(std::sync::Mutex::new(None));
    let ready = Arc::new(tokio::sync::Notify::new());
    let signer = lab.signer.clone();
    let sink = second.clone();
    let entered = ready.clone();
    lab.after_author(move |_, _, _| {
        *sink.lock().unwrap() = Some(signer.pause(Stage::Prepare));
        entered.notify_one();
    });
    let keys = Arc::new(LabKeys::new(lab.signer.clone()));
    let probe = Arc::new(Probe {
        builds: Arc::new(AtomicU64::new(0)),
    });
    let mut opts = options(lab.begin("deadline", probe.to_json(), Value::Null).unwrap());
    opts.execute.deadline = Some(Duration::from_secs(5));
    let ex = Executor::new(lab.client.clone());
    let k = keys.clone();
    let p = probe.clone();
    let mut task =
        tokio::spawn(async move { ex.submit_shielded_dyn(p.as_ref(), k.as_ref(), &opts).await });
    tokio::select! { _=ready.notified()=>{}, early=&mut task=>panic!("submission ended before author hook: {early:?}") }
    let pause = second.lock().unwrap().take().unwrap();
    tokio::select! { _=pause.entered()=>{}, early=&mut task=>panic!("submission ended before reseal gate: {early:?}") }
    assert_eq!(lab.authors(), 1);
    assert_eq!(lab.signer.signatures(), 2);
    tokio::time::advance(Duration::from_secs(5)).await;
    let result = task.await.unwrap();
    tokio::time::resume();
    keepalive.abort();
    assert!(result.is_err());
    assert_eq!(lab.authors(), 1);
    assert_eq!(lab.signer.signatures(), 2);
    let record = lab.bind("deadline").unwrap().record().unwrap();
    assert_eq!(record.shield_pairs.len(), 1);
    assert!(
        record.shield_pairs[0]
            .extensions
            .contains_key("rtao_shield_absence_v1")
    );
    lab.close().await;
}

#[tokio::test]
async fn paused_obligation_policy_is_checked_again_after_carrier_signing() {
    use rtao_signer::Signer;
    for allow in [false, true] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        let keys = Arc::new(LabKeys::new(lab.signer.clone()));
        let probe = Arc::new(Probe {
            builds: Arc::new(AtomicU64::new(0)),
        });
        let binding = lab
            .journal()
            .unwrap()
            .begin(rtao_client::journal::OperationIntent {
                id: "authorized-obligation".into(),
                correlation_id: Some("retained-obligation".into()),
                genesis: lab.client.genesis(),
                account: lab.signer.account(),
                scope: "public-obligation".into(),
                effects: probe.to_json(),
                obligation: true,
                semantic_policy: None,
                extensions: Default::default(),
            })
            .unwrap()
            .with_shield_protocol(lab.protocol.clone());
        let opts = options(binding);
        let pause = lab.signer.pause(Stage::Carrier);
        let ex = Executor::new(lab.client.clone());
        let k = keys.clone();
        let p = probe.clone();
        let mut task =
            tokio::spawn(
                async move { ex.submit_shielded_dyn(p.as_ref(), k.as_ref(), &opts).await },
            );
        tokio::select! { _=pause.entered()=>{}, early=&mut task=>panic!("submission ended before carrier gate: {early:?}") }
        lab.journal()
            .unwrap()
            .control(JournalControl {
                submissions_paused: true,
                allow_obligations: allow,
                extensions: Default::default(),
            })
            .unwrap();
        pause.release();
        let result = task.await.unwrap();
        if allow {
            assert!(result.unwrap().success);
            assert_eq!(lab.authors(), 1);
        } else {
            assert!(result.is_err());
            assert_eq!(lab.authors(), 0);
        }
        let record = lab.bind("authorized-obligation").unwrap().record().unwrap();
        assert_eq!(record.attempts.len(), if allow { 2 } else { 1 });
        assert_eq!(
            record.intent.correlation_id.as_deref(),
            Some("retained-obligation")
        );
        assert_eq!(lab.signer.signatures(), 2);
        lab.close().await;
    }
}

#[tokio::test]
async fn raw_recovery_never_labels_a_different_requested_call_as_finalized() {
    for changed in [false, true] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        lab.script([Inner::LostAcknowledgment]);
        let keys = LabKeys::new(lab.signer.clone());
        let binding = lab
            .begin("raw-recovery", json!({"op":"raw"}), json!({}))
            .unwrap();
        let mut opts = options(binding).execute;
        opts.shielded = true;
        opts.period = Some(4);
        let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
            .compose(&lab.client.refresh_runtime().await.unwrap())
            .unwrap();
        let ex = Executor::new(lab.client.clone());
        let pending = {
            let operation = ex.submit_call::<Coldkey, _>(call, &keys, &opts);
            tokio::pin!(operation);
            tokio::select! {
                early = &mut operation => panic!("unexpected early result: {early:?}"),
                _ = lab.lost_acknowledgment() => {
                    lab.node.drop_connection().unwrap();
                    operation.await
                }
            }
        };
        assert!(matches!(pending, Err(TxError::PendingOperation { .. })));
        drop(opts);
        drop(ex);
        lab.restart().await.unwrap();
        let mut opts = options(lab.bind("raw-recovery").unwrap()).execute;
        opts.shielded = true;
        opts.period = Some(4);
        let call = rtao_runtime::calls::system::remark(if changed {
            vec![9, 8, 7]
        } else {
            vec![1, 2, 3]
        })
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
        opts.expected_call = Some(call.clone());
        let result = Executor::new(lab.client.clone())
            .submit_call::<Coldkey, _>(call, &keys, &opts)
            .await;
        assert_eq!(lab.authors(), 1);
        assert_eq!(keys.unlocks(), 1);
        assert_eq!(lab.signer.signatures(), 2);
        drop(opts);
        lab.close().await;
        if changed {
            assert!(
                result.is_err(),
                "different raw call received old finalized success: {result:?}"
            );
        } else {
            assert!(
                result.unwrap().success,
                "unchanged raw recovery must retain positive control"
            );
        }
    }
}

#[path = "durable_shield/identity.rs"]
mod identity;

#[path = "durable_shield/causal_identity.rs"]
mod causal_identity;

#[path = "durable_shield/deadline.rs"]
mod deadline;
