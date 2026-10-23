#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::{ClientConfig, Mortality, Network, Period, SubmitOptions, Wait};
use rtao_signer::{KeypairSigner, SignPurpose, Signer};
use rtao_testkit::{FakeChain, FakeState, Reply};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").expect("task scratch")).join(
            format!(
                "journal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        );
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn journal(&self) -> Arc<OperationJournal> {
        OperationJournal::open(self.0.join("journal.json")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// The parent kills this child after the signed attempt is durably published.
// The synthetic chain is local to the child; this tests storage/process recovery,
// while the separate lost-ack fixture tests external-chain reconciliation.
#[tokio::test]
async fn journal_process_child() {
    let Some(root) = std::env::var_os("RTAO_JOURNAL_CRASH_CHILD") else {
        return;
    };
    let root = PathBuf::from(root);
    let j = OperationJournal::open(root.join("journal.json")).unwrap();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let b = b.material(json!({"required":"retained"})).unwrap();
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    std::fs::write(root.join("ready"), b"signed-before-dispatch").unwrap();
    std::future::pending::<()>().await;
}

#[test]
fn killed_writer_releases_process_lock_without_losing_signed_intent() {
    use std::process::{Command, Stdio};
    let f = Fixture::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "journal::tests::journal_process_child"])
        .env("RTAO_JOURNAL_CRASH_CHILD", &f.0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let ready = loop {
        if f.0.join("ready").exists() {
            break true;
        }
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let locked = matches!(
        OperationJournal::open(f.0.join("journal.json")),
        Err(JournalError::Owned)
    );
    // Always reap the child before assertions or fixture cleanup.
    let _ = child.kill();
    child.wait().unwrap();
    assert!(ready, "child did not publish the signed attempt in time");
    assert!(locked);
    let j = f.journal();
    let record = j.bind("logical-op").unwrap().record().unwrap();
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].outcome, AttemptOutcome::Unresolved);
    assert_eq!(record.attempts[0].material, json!({"required":"retained"}));
    assert!(!record.attempts[0].bytes.is_empty());
    assert!(j.bind("logical-op").unwrap().can_dispatch().is_err());
}

async fn chain() -> (Client, FakeChain, KeypairSigner) {
    let fake = FakeChain::from_state(FakeState::default());
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    let client = Client::from_rpc(subxt_rpcs::RpcClient::new(fake.clone()), cfg)
        .await
        .unwrap();
    let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Ed25519).unwrap();
    (client, fake, signer)
}
fn begin(j: &Arc<OperationJournal>, client: &Client, signer: &KeypairSigner) -> JournalBinding {
    j.begin(OperationIntent {
        id: "logical-op".into(),
        correlation_id: Some("caller-revision".into()),
        genesis: client.genesis(),
        account: signer.account(),
        scope: "test".into(),
        effects: json!({"action":"remark","input":"sensitive-effect"}),
        obligation: false,
        semantic_policy: None,
    })
    .unwrap()
}
async fn prepared(
    client: &Client,
    signer: &KeypairSigner,
) -> (crate::UnsignedExtrinsic, Vec<u8>, H256) {
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let unsigned = client
        .prepare_call(
            call,
            signer,
            &SubmitOptions {
                mortality: Mortality::Mortal(Period::new(4)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (bytes, hash) = client.attach_signature(&unsigned, signature).await.unwrap();
    (unsigned, bytes.to_vec(), hash)
}
#[test]
fn exclusive_ownership_cas_pause_identity_and_reopen() {
    let f = Fixture::new();
    let j = f.journal();
    assert!(matches!(
        OperationJournal::open(f.0.join("journal.json")),
        Err(JournalError::Owned)
    ));
    let owner = j.claim("weights").unwrap();
    let other_handle = OperationJournal::new(j.store.clone()).unwrap();
    assert!(matches!(
        other_handle.claim("weights"),
        Err(JournalError::Owned)
    ));
    drop(other_handle);
    assert!(matches!(j.claim("weights"), Err(JournalError::Owned)));
    drop(owner);
    assert!(j.claim("weights").is_ok());
    let intent = OperationIntent {
        id: "op".into(),
        correlation_id: None,
        genesis: H256([0; 32]),
        account: AccountId([1; 32]),
        scope: "work".into(),
        effects: json!({"secret":"do-not-log"}),
        obligation: false,
        semantic_policy: None,
    };
    assert!(!format!("{intent:?}").contains("do-not-log"));
    let b = j.begin(intent.clone()).unwrap();
    assert!(j.begin(intent.clone()).is_ok());
    let mut changed = intent;
    changed.effects = json!("changed");
    assert!(matches!(j.begin(changed), Err(JournalError::Identity)));
    j.control(JournalControl {
        submissions_paused: true,
        allow_obligations: true,
    })
    .unwrap();
    assert!(b.can_dispatch().is_err());
    let image = j.snapshot().unwrap();
    assert!(matches!(
        j.store.compare_exchange(image.revision - 1, &image),
        Err(JournalError::Conflict)
    ));
    drop(b);
    drop(j);
    let reopened = f.journal();
    assert!(reopened.snapshot().unwrap().control.submissions_paused);
    assert_eq!(reopened.snapshot().unwrap().operations.len(), 1);
}
#[tokio::test]
async fn paused_journal_only_allows_preexisting_authorized_obligations_and_keeps_observation() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let ordinary = begin(&j, &client, &signer);
    let mut intent = ordinary.record().unwrap().intent;
    intent.id = "previously-authorized-obligation".into();
    intent.obligation = true;
    let obligation = j.begin(intent.clone()).unwrap();
    j.control(JournalControl {
        submissions_paused: true,
        allow_obligations: true,
    })
    .unwrap();
    assert!(ordinary.can_dispatch().is_err());
    assert!(obligation.can_dispatch().is_ok());
    // The allowance cannot authorize a fresh operation during the pause.
    intent.id = "new-obligation".into();
    assert!(matches!(j.begin(intent), Err(JournalError::Blocked(_))));
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    obligation.signed(&unsigned, &bytes, hash).await.unwrap();
    j.control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
    })
    .unwrap();
    assert!(obligation.can_dispatch().is_err());
    assert!(matches!(
        obligation.recover(&client, 16).await.unwrap(),
        RecoveryStatus::Unresolved(_)
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[tokio::test]
async fn stale_recovery_publication_cannot_erase_terminal_evidence() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    let stale = b.record().unwrap().attempts[0].clone();
    let mut expired = stale.clone();
    expired.outcome = AttemptOutcome::ExpiredUnexecuted;
    b.save_recovery(expired).await.unwrap();
    b.save_recovery(stale.clone()).await.unwrap();
    assert_eq!(
        b.record().unwrap().attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    // A known inclusion also wins against a late absence scan.
    j.update(|image| {
        image.operations.get_mut(b.id()).unwrap().attempts[0].outcome = AttemptOutcome::Finalized {
            block: H256([9; 32]),
            number: 101,
            success: false,
        };
        Ok(())
    })
    .unwrap();
    b.save_recovery(stale).await.unwrap();
    assert!(matches!(
        b.record().unwrap().attempts[0].outcome,
        AttemptOutcome::Finalized { success: false, .. }
    ));
    assert!(b.can_dispatch().is_err());
    client.close().await;
}

#[tokio::test]
async fn signed_bytes_are_durable_before_actual_dispatch_and_repeated_recovery_never_sends() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let b = b.material(json!({"commit":"sensitive-commit"})).unwrap();
    let call = rtao_runtime::calls::system::remark(vec![1])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let reader = b.clone();
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let r = reader.record().unwrap();
            assert_eq!(r.attempts.len(), 1);
            let bytes = request.params[0].as_str().unwrap();
            assert_eq!(bytes, format!("0x{}", hex::encode(&r.attempts[0].bytes)));
            assert_eq!(r.attempts[0].material, json!({"commit":"sensitive-commit"}));
            state.dispatch(request)
        })
    });
    let report = client
        .submit(
            call,
            &signer,
            &SubmitOptions {
                journal: Some(b.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.success);
    assert!(b.can_dispatch().is_err());
    for _ in 0..2 {
        assert!(matches!(
            b.recover(&client, 16).await.unwrap(),
            RecoveryStatus::Finalized(_)
        ));
    }
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert!(!format!("{:?}", b.record().unwrap()).contains("sensitive"));
    client.close().await;
}
#[tokio::test]
async fn crash_after_inclusion_before_ack_reopens_and_finds_actual_historical_receipt() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
            state.dispatch(request);
            Reply::silent()
        })
    });
    let call = rtao_runtime::calls::system::remark(vec![2])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let result = client
        .submit(
            call,
            &signer,
            &SubmitOptions {
                journal: Some(b.clone()),
                deadline: Some(Duration::from_millis(100)),
                ..Default::default()
            },
        )
        .await;
    assert!(result.is_err());
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        b.record().unwrap().attempts[0].outcome,
        AttemptOutcome::Unresolved
    );
    drop(b);
    drop(j);
    let j = f.journal();
    let b = j.bind("logical-op").unwrap();
    assert!(b.can_dispatch().is_err());
    assert!(matches!(b.recover(&client,8).await.unwrap(),RecoveryStatus::Finalized(r)if r.success));
    assert!(matches!(
        b.recover(&client, 8).await.unwrap(),
        RecoveryStatus::Finalized(_)
    ));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    client.close().await;
}
#[tokio::test]
async fn signed_before_dispatch_crash_and_expiry_require_complete_history() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    let expiry = unsigned.expiry().unwrap();
    drop(b);
    drop(j);
    let j = f.journal();
    let b = j.bind("logical-op").unwrap();
    fake.mutate_state(|state| {
        state.advance(expiry + 2 - state.block);
        state.queue_reply("chain_getBlock", Reply::result(Value::Null));
    });
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::HistoryUnavailable { .. })
    ));
    assert!(b.can_dispatch().is_err());
    assert!(matches!(
        b.recover(&client, 1).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::ScanBudget { .. })
    ));
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Rebuildable
    ));
    assert!(b.can_dispatch().is_ok());
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Rebuildable
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}
#[tokio::test]
async fn incomplete_or_malformed_bodies_never_prove_expired_absence() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    fake.mutate_state(|state| state.advance(unsigned.expiry().unwrap() + 2 - state.block));
    let next = b.record().unwrap().attempts[0].next;
    for body in [
        Value::Null,
        json!({"block":{}}),
        json!({"block":{"extrinsics":null}}),
        json!({"block":{"extrinsics":"not-an-array"}}),
        json!({"block":{"extrinsics":[null]}}),
        json!({"block":{"extrinsics":["0xgg"]}}),
    ] {
        fake.mutate_state(|state| state.queue_reply("chain_getBlock", Reply::result(body)));
        assert_eq!(
            match b.recover(&client, 32).await.unwrap() {
                RecoveryStatus::Unresolved(reason) => reason,
                other => panic!("incomplete body resolved an attempt: {other:?}"),
            },
            RecoveryReason::HistoryUnavailable { next }
        );
        let attempt = &b.record().unwrap().attempts[0];
        assert_eq!(attempt.next, next);
        assert_eq!(attempt.outcome, AttemptOutcome::Unresolved);
        assert_eq!(attempt.bytes, bytes);
        assert!(b.can_dispatch().is_err());
    }
    // Public lookup's inherited Python parity remains unchanged.
    fake.mutate_state(|state| {
        state.queue_reply("chain_getBlock", Reply::result(json!({"block":{}})))
    });
    assert_eq!(
        client
            .find_extrinsic(hash, unsigned.era_block_hash())
            .await
            .unwrap(),
        None
    );
    // Actual complete fixture bodies can still prove absence through expiry.
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Rebuildable
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[tokio::test]
async fn missing_list_at_actual_inclusion_retains_uncertainty_past_expiry() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
            state.dispatch(request);
            Reply::silent()
        })
    });
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    assert!(
        client
            .submit(
                call,
                &signer,
                &SubmitOptions {
                    journal: Some(b.clone()),
                    mortality: Mortality::Mortal(Period::new(4)),
                    deadline: Some(Duration::from_millis(100)),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let attempt = b.record().unwrap().attempts[0].clone();
    let (number, target) = fake.mutate_state(|state| {
        let number = state.block;
        let hash = state.block_hash(number).unwrap();
        state.advance(attempt.expiry.unwrap() + 2 - state.block);
        (number, hash)
    });
    for body in [json!({"block":{}}), json!({"block":{"extrinsics":null}})] {
        let target = target.clone();
        fake.mutate_state(|state| {
            state.set_handler("chain_getBlock", move |state, request| {
                if request.params[0].as_str() == Some(target.as_str()) {
                    Reply::result(body.clone())
                } else {
                    state.dispatch(request)
                }
            })
        });
        assert!(matches!(b.recover(&client, 32).await.unwrap(),
            RecoveryStatus::Unresolved(RecoveryReason::HistoryUnavailable { next }) if next == number));
        assert_eq!(b.record().unwrap().attempts[0].next, number);
        assert!(b.can_dispatch().is_err());
        assert!(matches!(
            client
                .reconcile(attempt.hash, attempt.start, attempt.expiry)
                .await,
            Err(crate::SubmitFailure::Client(crate::ClientError::Protocol(
                _
            )))
        ));
    }
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", |state, request| state.dispatch(request))
    });
    assert!(
        matches!(b.recover(&client, 32).await.unwrap(), RecoveryStatus::Finalized(r) if r.success)
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert!(b.can_dispatch().is_err());
    client.close().await;
}

#[tokio::test]
async fn stale_finalized_and_rpc_failure_cannot_become_absent() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    assert!(matches!(
        b.recover(&client, 1).await.unwrap(),
        RecoveryStatus::Unresolved(_)
    ));
    fake.mutate_state(|state| {
        state.set_finalized(99).unwrap();
    });
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::StaleFinalizedView)
    ));
    fake.mutate_state(|state| {
        state.set_finalized(100).unwrap();
        state.queue_reply(
            "chain_getFinalizedHead",
            Reply::error(-32603, "unavailable", None),
        );
    });
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::FinalizedUnavailable)
    ));
    assert!(b.can_dispatch().is_err());
    client.close().await;
}
#[tokio::test]
async fn concurrent_material_replacement_during_signing_and_aba_refuse_dispatch() {
    struct PausedSigner {
        inner: KeypairSigner,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    impl Signer for PausedSigner {
        fn account(&self) -> AccountId {
            self.inner.account()
        }
        fn crypto_type(&self) -> rtao_primitives::CryptoType {
            self.inner.crypto_type()
        }
        fn sign<'a>(
            &'a self,
            request: &'a rtao_signer::SigningRequest<'a>,
        ) -> rtao_signer::BoxFuture<'a, Result<rtao_signer::MultiSignature, rtao_signer::SignerError>>
        {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                self.inner.sign(request).await
            })
        }
    }
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let material_a = json!({"ciphertext":"A","round":100});
    let a = begin(&j, &client, &signer)
        .material(material_a.clone())
        .unwrap();
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = PausedSigner {
        inner: signer,
        entered: entered.clone(),
        release: release.clone(),
    };
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let opts = SubmitOptions {
        journal: Some(a.clone()),
        ..Default::default()
    };
    let replace = async {
        entered.notified().await;
        let b = j
            .bind(a.id())
            .unwrap()
            .material(json!({"ciphertext":"B","round":200}))
            .unwrap();
        release.notify_one();
        b
    };
    let (result, b) = tokio::join!(client.submit(call.clone(), &slow, &opts), replace);
    assert!(matches!(
        result,
        Err(crate::SubmitFailure::Client(crate::ClientError::Journal(
            JournalError::Conflict
        )))
    ));
    assert!(a.record().unwrap().attempts.is_empty());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    // Same bytes returning later cannot revive an earlier generation's signer.
    let selected_a = b.material(material_a.clone()).unwrap();
    assert!(matches!(
        a.check(&client, &unsigned).await,
        Err(JournalError::Conflict)
    ));
    assert!(matches!(
        a.signed(&unsigned, &bytes, hash).await,
        Err(JournalError::Conflict)
    ));
    assert!(a.record().unwrap().attempts.is_empty());
    let report = client
        .submit(
            call,
            &slow.inner,
            &SubmitOptions {
                journal: Some(selected_a.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.success);
    let record = selected_a.record().unwrap();
    assert_eq!(record.material_generation, 3);
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].material, material_a);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    client.close().await;
}

#[tokio::test]
async fn concurrent_authorization_only_one_signed_attempt_is_dispatched() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, _, _) = prepared(&client, &signer).await;
    let sig = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let options = SubmitOptions {
        journal: Some(b.clone()),
        wait: Wait::Pool,
        ..Default::default()
    };
    let (a, z) = tokio::join!(
        client.submit_signature(&unsigned, sig, &options),
        client.submit_signature(&unsigned, sig, &options)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(z.is_ok()), 1);
    assert_eq!(b.record().unwrap().attempts.len(), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    client.close().await;
}
struct Deny;
impl SemanticGuard for Deny {
    fn check<'a>(
        &'a self,
        _: &'a Client,
        _: &'a OperationIntent,
        _: &'a UnsignedExtrinsic,
    ) -> rtao_signer::BoxFuture<'a, Result<(), Incompatibility>> {
        Box::pin(async {
            Err(Incompatibility {
                code: "adapter-review".into(),
                descriptor: None,
                detail: "qualify adapter semantics".into(),
            })
        })
    }
}
#[tokio::test]
async fn semantic_guard_is_structured_and_must_be_restored_before_resume() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let mut intent = begin(&j, &client, &signer).record().unwrap().intent;
    intent.id = "semantic".into();
    intent.semantic_policy = Some("meaning-v1".into());
    let b = j.begin(intent).unwrap().with_guard(Arc::new(Deny));
    let (unsigned, _, _) = prepared(&client, &signer).await;
    assert!(b.check(&client, &unsigned).await.is_err());
    assert_eq!(
        b.record().unwrap().incompatibility.unwrap().code,
        "adapter-review"
    );
    let restored = j.bind("semantic").unwrap();
    assert!(restored.check(&client, &unsigned).await.is_err());
    assert!(b.record().unwrap().attempts.is_empty());
    client.close().await;
}

#[tokio::test]
async fn compatible_upgrade_rebuilds_unsigned_work_but_preserves_submitted_attempts() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, _, _) = prepared(&client, &signer).await;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    fake.mutate_state(|state| {
        state.set_result(
            "state_getRuntimeVersion",
            json!({"specName":"node-subtensor","specVersion":482,"transactionVersion":2,"apis":[]}),
        )
    });
    assert!(
        client
            .submit_signature(
                &unsigned,
                signature,
                &SubmitOptions {
                    journal: Some(b.clone()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(b.record().unwrap().attempts.is_empty());
    assert!(b.can_dispatch().is_ok());
    let runtime = client.refresh_runtime().await.unwrap();
    assert_eq!(runtime.spec_version(), 482);
    let call = rtao_runtime::calls::system::remark(vec![3])
        .compose(&runtime)
        .unwrap();
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
            state.dispatch(request);
            Reply::silent()
        })
    });
    assert!(
        client
            .submit(
                call,
                &signer,
                &SubmitOptions {
                    journal: Some(b.clone()),
                    deadline: Some(Duration::from_millis(100)),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let hash = b.record().unwrap().attempts[0].hash;
    assert_eq!(b.record().unwrap().attempts[0].spec_version, 482);
    fake.mutate_state(|state| {
        state.set_result(
            "state_getRuntimeVersion",
            json!({"specName":"node-subtensor","specVersion":483,"transactionVersion":3,"apis":[]}),
        )
    });
    assert!(
        matches!(b.recover(&client,16).await.unwrap(),RecoveryStatus::Finalized(r)if r.hash==hash&&r.success)
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert!(b.can_dispatch().is_err());
    client.close().await;
}

struct FailOnceStore {
    inner: NativeJournalStore,
    fail: std::sync::atomic::AtomicBool,
}
impl JournalStore for FailOnceStore {
    fn claim_scope(&self, scope: &str) -> Result<Box<dyn JournalLease>, JournalError> {
        self.inner.claim_scope(scope)
    }
    fn load(&self) -> Result<JournalImage, JournalError> {
        self.inner.load()
    }
    fn compare_exchange(&self, expected: u64, image: &JournalImage) -> Result<(), JournalError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(JournalError::Storage("injected publication failure".into()));
        }
        self.inner.compare_exchange(expected, image)
    }
}
#[tokio::test]
async fn failed_durable_write_refuses_dispatch_and_retains_intent() {
    let f = Fixture::new();
    let store = Arc::new(FailOnceStore {
        inner: NativeJournalStore::open(f.0.join("journal.json")).unwrap(),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let j = OperationJournal::new(store.clone()).unwrap();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, _, _) = prepared(&client, &signer).await;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    store.fail.store(true, Ordering::SeqCst);
    assert!(
        client
            .submit_signature(
                &unsigned,
                signature,
                &SubmitOptions {
                    journal: Some(b.clone()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(b.record().unwrap().attempts.is_empty());
    assert_eq!(
        b.record().unwrap().intent.correlation_id.as_deref(),
        Some("caller-revision")
    );
    client.close().await;
}
