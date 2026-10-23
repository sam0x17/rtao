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
        extensions: Default::default(),
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
        extensions: Default::default(),
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
        extensions: Default::default(),
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
        extensions: Default::default(),
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
        extensions: Default::default(),
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
    // A known inclusion also wins against a late unresolved scan. Use separate
    // work: an expiry proof contradicted by inclusion must be retained as a
    // conflict in schema3, never overwritten to make this control pass.
    let mut intent = b.record().unwrap().intent;
    intent.id = "known-inclusion".into();
    let b = j.begin(intent).unwrap();
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    let stale = b.record().unwrap().attempts[0].clone();
    j.update(|image| {
        image.operations.get_mut(b.id()).unwrap().attempts[0].outcome = AttemptOutcome::Finalized {
            block: H256([9; 32]),
            number: 101,
            success: false,
            extensions: Default::default(),
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
                extensions: Default::default(),
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

// Schema3 fixtures inject retained pairs, not author hooks. Actual shield author
// dispatch remains deliberately refused; these exercise the real recovery path.
async fn signed_component(
    client: &Client,
    signer: &KeypairSigner,
    call: crate::CallBytes,
    nonce: u64,
    id: u64,
    role: AttemptRole,
) -> SignedAttempt {
    let unsigned = client
        .prepare_call(
            call,
            signer,
            &SubmitOptions {
                nonce: crate::NoncePolicy::Explicit(nonce),
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
    SignedAttempt {
        id,
        role,
        material_generation: Some(0),
        hash,
        bytes: bytes.to_vec(),
        nonce: unsigned.nonce(),
        spec_version: unsigned.call().stamp().spec_version,
        transaction_version: unsigned.call().stamp().transaction_version,
        start: unsigned.submission_head(),
        expiry: unsigned.expiry(),
        era_anchor: unsigned.era_block_hash(),
        era: rtao_core::codec::value::to_corpus_json(unsigned.era()),
        metadata_hash: unsigned.metadata_hash(),
        material: json!({"private-marker":"retained-inner"}),
        next: unsigned.submission_head(),
        finalized: None,
        outcome: AttemptOutcome::Unresolved,
        evidence_conflicts: Vec::new(),
        extensions: Default::default(),
    }
}
async fn paired_record(client: &Client, signer: &KeypairSigner, b: &JournalBinding, count: u32) {
    let selected = b
        .clone()
        .material(json!({"private-marker":"retained-inner"}))
        .unwrap();
    let generation = selected.record().unwrap().material_generation;
    let mut attempts = Vec::new();
    let mut pairs = Vec::new();
    for pair in 0..u64::from(count) {
        let mut inner = signed_component(
            client,
            signer,
            rtao_runtime::calls::system::remark(vec![pair as u8])
                .compose(&client.refresh_runtime().await.unwrap())
                .unwrap(),
            pair * 2 + 1,
            pair * 2,
            AttemptRole::Inner { pair },
        )
        .await;
        inner.material_generation = Some(generation);
        let sealed = crate::ShieldCiphertext::seal(&[0; 1184], &inner.bytes)
            .await
            .unwrap();
        let mut carrier = signed_component(
            client,
            signer,
            rtao_runtime::calls::mev_shield::submit_encrypted(sealed.into_bytes())
                .compose(&client.refresh_runtime().await.unwrap())
                .unwrap(),
            pair * 2,
            pair * 2 + 1,
            AttemptRole::Carrier { pair },
        )
        .await;
        carrier.material_generation = Some(generation);
        pairs.push(ShieldPair {
            id: pair,
            material_generation: generation,
            inner: inner.id,
            carrier: Some(carrier.id),
            phase: PairPhase::Prepared,
            extensions: Default::default(),
        });
        attempts.extend([inner, carrier]);
    }
    b.journal
        .update(|image| {
            let revision = image.revision + 1;
            let record = image.operations.get_mut(b.id()).unwrap();
            record.mode = ExecutionMode::Shielded(ShieldPolicy {
                protocol: "synthetic-signed-inner-only".into(),
                period: 4,
                maximum_pairs: count,
                extensions: Default::default(),
            });
            for pair in &mut pairs {
                pair.phase = PairPhase::DispatchAuthorized { revision };
            }
            record.shield_pairs = pairs;
            record.attempts = attempts;
            Ok(())
        })
        .unwrap();
}
fn pair_history(
    fake: &FakeChain,
    attempts: Vec<SignedAttempt>,
    include_inner: bool,
    failed_inner: bool,
) {
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome
                && raw["block"]["header"]["number"].as_str() == Some("0x65") {
                let mut events = Vec::new();
                for attempt in &attempts {
                    if !include_inner && matches!(attempt.role, AttemptRole::Inner { .. }) { continue; }
                    let list = raw["block"]["extrinsics"].as_array_mut().unwrap();
                    let index = list.len();
                    list.push(json!(format!("0x{}", hex::encode(&attempt.bytes))));
                    let failed = failed_inner && matches!(attempt.role, AttemptRole::Inner { .. });
                    events.push(if failed {
                        json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Token":"FundsUnavailable"},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
                    } else {
                        json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
                    });
                }
                state.seed_events(request.params[0].as_str().unwrap(), json!(events)).unwrap();
            }
            reply
        });
        state.advance(10);
    });
}
#[tokio::test]
async fn schema3_actual_signed_pair_recovers_inner_not_vector_tail_or_carrier_receipt() {
    for failed in [false, true] {
        let f = Fixture::new();
        let j = f.journal();
        let (client, fake, signer) = chain().await;
        let b = begin(&j, &client, &signer);
        paired_record(&client, &signer, &b, 1).await;
        let record = b.record().unwrap();
        let inner_hash = record.attempts[0].hash;
        let carrier_hash = record.attempts[1].hash;
        pair_history(&fake, record.attempts, true, failed);
        drop(b);
        drop(j);
        let reopened = f.journal();
        let b = reopened.bind("logical-op").unwrap();
        for _ in 0..2 {
            let RecoveryStatus::Finalized(receipt) = b.recover(&client, 32).await.unwrap() else {
                panic!("no inner receipt");
            };
            assert_eq!(receipt.hash, inner_hash);
            assert_ne!(receipt.hash, carrier_hash);
            assert_eq!(receipt.inclusion, crate::Inclusion::Finalized);
            assert_eq!(receipt.success, !failed);
        }
        assert!(b.can_dispatch().is_err());
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        assert!(!format!("{:?}", b.record().unwrap()).contains("private-marker"));
        client.close().await;
    }
}
#[tokio::test]
async fn schema3_carrier_only_complete_expiry_and_repeated_recovery_never_rebuild_or_send() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    paired_record(&client, &signer, &b, 1).await;
    pair_history(&fake, b.record().unwrap().attempts, false, false);
    for _ in 0..3 {
        assert!(matches!(
            b.recover(&client, 64).await.unwrap(),
            RecoveryStatus::Unresolved(RecoveryReason::ShieldAdapterRequired)
        ));
    }
    let record = b.record().unwrap();
    assert_eq!(record.attempts[0].outcome, AttemptOutcome::Unresolved);
    assert!(matches!(
        record.attempts[1].outcome,
        AttemptOutcome::Finalized { .. }
    ));
    assert!(record.finalized_effect().unwrap().is_none());
    assert!(b.can_dispatch().is_err());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}
#[tokio::test]
async fn schema3_pairs_share_finite_body_budget_and_missing_body_preserves_target_cursor() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    paired_record(&client, &signer, &b, 2).await;
    fake.mutate_state(|s| s.advance(10));
    let before = b.record().unwrap();
    fake.mutate_state(|s| s.queue_reply("chain_getBlock", Reply::result(json!({"block":{}}))));
    assert!(matches!(
        b.recover(&client, 1).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::HistoryUnavailable { .. })
    ));
    assert_eq!(
        b.record().unwrap().attempts[0].next,
        before.attempts[0].next
    );
    for _ in 0..12 {
        let calls = fake
            .state()
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.method == "chain_getBlock")
            .count();
        let status = b.recover(&client, 1).await.unwrap();
        assert!(matches!(status, RecoveryStatus::Unresolved(_)));
        let after = fake
            .state()
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.method == "chain_getBlock")
            .count();
        assert!(after - calls <= 1, "per-target budget multiplication");
    }
    assert!(
        b.record()
            .unwrap()
            .attempts
            .iter()
            .all(|a| a.outcome == AttemptOutcome::Unresolved)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}
#[tokio::test]
async fn schema3_graph_refuses_ambiguous_identity_role_generation_nonce_and_budget_without_write() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    paired_record(&client, &signer, &b, 1).await;
    let original = std::fs::read(f.0.join("journal.json")).unwrap();
    for variant in 0..8 {
        assert!(
            j.update(|image| {
                let r = image.operations.get_mut(b.id()).unwrap();
                match variant {
                    0 => r.attempts[1].id = 0,
                    1 => r.attempts[1].role = AttemptRole::Inner { pair: 0 },
                    2 => r.shield_pairs[0].inner = 99,
                    3 => r.attempts[1].material_generation = Some(r.material_generation + 1),
                    4 => r.attempts[1].nonce = 90,
                    5 => {
                        if let ExecutionMode::Shielded(policy) = &mut r.mode {
                            policy.maximum_pairs = 0;
                        }
                    }
                    6 => r.mode = ExecutionMode::Ordinary,
                    _ => r.attempts[0].bytes.push(42),
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(std::fs::read(f.0.join("journal.json")).unwrap(), original);
    }
    client.close().await;
}
#[tokio::test]
async fn schema3_contradictory_terminal_evidence_survives_reopen_and_blocks_effect_recovery() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    paired_record(&client, &signer, &b, 1).await;
    let mut negative = b.record().unwrap().attempts[0].clone();
    negative.outcome = AttemptOutcome::ExpiredUnexecuted;
    b.save_recovery(negative).await.unwrap();
    pair_history(&fake, b.record().unwrap().attempts, true, false);
    let hash = b.record().unwrap().attempts[0].hash;
    let block = client
        .at(crate::BlockRef::Number(101))
        .await
        .unwrap()
        .block()
        .hash;
    let report = client.resolve_extrinsic(hash, block, true).await.unwrap();
    b.observed(&report).await.unwrap();
    b.observed(&report).await.unwrap();
    assert_eq!(b.record().unwrap().attempts[0].evidence_conflicts.len(), 1);
    drop(b);
    drop(j);
    let reopened = f.journal();
    let b = reopened.bind("logical-op").unwrap();
    assert!(matches!(
        b.recover(&client, 32).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    assert_eq!(
        b.record().unwrap().attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    assert!(matches!(
        b.record().unwrap().attempts[0].evidence_conflicts[0],
        AttemptOutcome::Finalized { success: true, .. }
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[tokio::test]
async fn schema3_migration_retains_nested_unknown_evidence_and_legacy_certainty() {
    for schema in [1, 2] {
        let f = Fixture::new();
        let j = f.journal();
        let (client, fake, signer) = chain().await;
        let b = begin(&j, &client, &signer);
        let (unsigned, bytes, hash) = prepared(&client, &signer).await;
        b.signed(&unsigned, &bytes, hash).await.unwrap();
        let mut no_attempt = b.record().unwrap().intent;
        no_attempt.id = "not-signed-yet".into();
        j.begin(no_attempt).unwrap();
        drop(b);
        drop(j);
        let path = f.0.join("journal.json");
        let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["schema_version"] = json!(schema);
        for record in raw["operations"].as_object_mut().unwrap().values_mut() {
            record.as_object_mut().unwrap().remove("mode");
            record.as_object_mut().unwrap().remove("shield_pairs");
            record
                .as_object_mut()
                .unwrap()
                .remove("material_generation");
            for attempt in record["attempts"].as_array_mut().unwrap() {
                for name in ["id", "role", "material_generation", "evidence_conflicts"] {
                    attempt.as_object_mut().unwrap().remove(name);
                }
            }
        }
        raw["extra-envelope"] = json!({"evidence":[1,2]});
        raw["control"]["extra-control"] = json!({"opaque":"preserve"});
        raw["operations"]["logical-op"]["extra-record"] = json!(["retained"]);
        raw["operations"]["logical-op"]["intent"]["extra-intent"] = json!({"opaque":[3,4]});
        raw["operations"]["logical-op"]["attempts"][0]["extra-attempt"] =
            json!({"evidence":"never-drop"});
        raw["operations"]["logical-op"]["attempts"][0]["outcome"] = json!({"Finalized":{
            "block": unsigned.era_block_hash(), "number":unsigned.submission_head(), "success":false,
            "extra-receipt":{"opaque":"retain"}
        }});
        let prior_revision = raw["revision"].as_u64().unwrap();
        std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let migrated = f.journal();
        let b = migrated.bind("logical-op").unwrap();
        let r = b.record().unwrap();
        assert_eq!(r.mode, ExecutionMode::Ordinary);
        assert_eq!(r.attempts[0].id, 0);
        assert_eq!(r.attempts[0].role, AttemptRole::Ordinary);
        assert_eq!(r.attempts[0].material_generation, None);
        assert_eq!(r.attempts[0].bytes, bytes);
        assert_eq!(r.attempts[0].hash, hash);
        assert!(matches!(
            r.attempts[0].outcome,
            AttemptOutcome::Finalized { success: false, .. }
        ));
        assert!(b.can_dispatch().is_err());
        assert_eq!(migrated.snapshot().unwrap().revision, prior_revision + 1);
        assert_eq!(
            migrated
                .bind("not-signed-yet")
                .unwrap()
                .record()
                .unwrap()
                .mode,
            ExecutionMode::Unselected
        );
        // Exercise another real CAS after migration, not just serialization.
        migrated
            .control(JournalControl {
                submissions_paused: true,
                ..Default::default()
            })
            .unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for pointer in [
            "/extra-envelope",
            "/control/extra-control",
            "/operations/logical-op/extra-record",
            "/operations/logical-op/intent/extra-intent",
            "/operations/logical-op/attempts/0/extra-attempt",
            "/operations/logical-op/attempts/0/outcome/Finalized/extra-receipt",
        ] {
            assert_eq!(after.pointer(pointer), raw.pointer(pointer), "{pointer}");
        }
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        client.close().await;
    }
}
#[tokio::test]
async fn schema3_legacy_reserved_field_collisions_and_unknown_schema_refuse_without_mutation() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    begin(&j, &client, &signer);
    drop(j);
    let path = f.0.join("journal.json");
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    raw["schema_version"] = json!(2);
    // A legacy extension with a newly reserved name is NOT overwritten into a
    // guessed ordinary mode, even when it looks like a compatible enum.
    raw["operations"]["logical-op"]["mode"] = json!("Ordinary");
    for variant in 0..3 {
        let mut candidate = raw.clone();
        if variant == 1 {
            candidate["schema_version"] = json!(99);
        }
        if variant == 2 {
            candidate["schema_version"] = json!(3);
            candidate["operations"]["logical-op"]["mode"] = json!("FutureEffectProtocol");
        }
        let bytes = serde_json::to_vec(&candidate).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(OperationJournal::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    client.close().await;
}

#[tokio::test]
async fn schema3_transition_cannot_erase_authorized_pair_or_mutate_a_valid_signed_graph() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    paired_record(&client, &signer, &b, 1).await;
    let original = std::fs::read(f.0.join("journal.json")).unwrap();
    for variant in 0..6 {
        assert!(
            j.update(|image| {
                if variant == 0 {
                    image.operations.remove(b.id());
                    return Ok(());
                }
                let r = image.operations.get_mut(b.id()).unwrap();
                match variant {
                    1 => {
                        r.mode = ExecutionMode::Unselected;
                        r.attempts.clear();
                        r.shield_pairs.clear();
                    }
                    2 => r.shield_pairs[0].phase = PairPhase::Retired,
                    3 => r.intent.effects = json!({"different-effect":true}),
                    4 => {
                        r.attempts[0].nonce += 2;
                        r.attempts[1].nonce += 2;
                    }
                    _ => {
                        if let ExecutionMode::Shielded(p) = &mut r.mode {
                            p.maximum_pairs += 1;
                        }
                    }
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(std::fs::read(f.0.join("journal.json")).unwrap(), original);
    }
    client.close().await;
}

#[tokio::test]
async fn schema3_ordinary_conflict_blocks_dispatch_and_receipt_extensions_are_not_logged() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    let mut absent = b.record().unwrap().attempts[0].clone();
    absent.outcome = AttemptOutcome::ExpiredUnexecuted;
    b.save_recovery(absent).await.unwrap();
    assert!(b.can_dispatch().is_ok());
    let mut included = b.record().unwrap().attempts[0].clone();
    included.outcome = AttemptOutcome::Finalized {
        block: unsigned.era_block_hash(),
        number: unsigned.submission_head(),
        success: true,
        extensions: BTreeMap::from([(
            "unknown-evidence".into(),
            json!("sensitive-receipt-marker"),
        )]),
    };
    b.save_recovery(included.clone()).await.unwrap();
    assert!(b.can_dispatch().is_err());
    assert!(matches!(
        b.recover(&client, 0).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    assert!(!format!("{:?}", included).contains("sensitive-receipt-marker"));
    assert!(!format!("{:?}", included.outcome).contains("sensitive-receipt-marker"));
    // A repeated observation of the same physical fact without unknown fields
    // does not manufacture a second contradictory receipt.
    if let AttemptOutcome::Finalized { extensions, .. } = &mut included.outcome {
        extensions.clear();
    }
    b.save_recovery(included).await.unwrap();
    assert_eq!(b.record().unwrap().attempts[0].evidence_conflicts.len(), 1);
    client.close().await;
}

#[tokio::test]
async fn schema3_review_duplicate_names_refuse_before_migration_without_losing_original_evidence() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    drop(b);
    drop(j);
    let path = f.0.join("journal.json");
    let current = std::fs::read(&path).unwrap();
    let mut legacy: Value = serde_json::from_slice(&current).unwrap();
    legacy["schema_version"] = json!(2);
    let record = &mut legacy["operations"]["logical-op"];
    record.as_object_mut().unwrap().remove("mode");
    record.as_object_mut().unwrap().remove("shield_pairs");
    for attempt in record["attempts"].as_array_mut().unwrap() {
        for key in ["id", "role", "material_generation", "evidence_conflicts"] {
            attempt.as_object_mut().unwrap().remove(key);
        }
        attempt["opaque"] = json!({"proof":"retained-marker"});
    }
    let record_text = record.to_string();
    let attempt_text = record["attempts"][0].to_string();
    let valid = legacy.to_string();
    let duplicates = [
        format!("{},\"operations\":{{}}}}", valid.strip_suffix('}').unwrap()),
        format!(
            "{},\"operat\\u0069ons\":{{}}}}",
            valid.strip_suffix('}').unwrap()
        ),
        valid.replace(
            &record_text,
            &format!(
                "{},\"attempts\":[]}}",
                record_text.strip_suffix('}').unwrap()
            ),
        ),
        valid.replace(
            &attempt_text,
            &format!(
                "{},\"outcome\":\"ExpiredUnexecuted\"}}",
                attempt_text.strip_suffix('}').unwrap()
            ),
        ),
        valid.replace(
            "\"proof\":\"retained-marker\"",
            "\"proof\":\"retained-marker\",\"pr\\u006fof\":\"lost-marker\"",
        ),
        // Current-schema reads use the same strict byte boundary, not only migration.
        String::from_utf8(current).unwrap().replace(
            "\"outcome\":\"Unresolved\"",
            "\"outcome\":\"Unresolved\",\"outcome\":\"ExpiredUnexecuted\"",
        ),
    ];
    for (variant, duplicate) in duplicates.iter().enumerate() {
        assert!(
            serde_json::from_str::<Value>(duplicate).is_ok(),
            "fixture is valid last-wins JSON"
        );
        std::fs::write(&path, duplicate).unwrap();
        for _ in 0..2 {
            let result = OperationJournal::open(&path);
            assert!(
                result.is_err(),
                "duplicate variant {variant} erased evidence instead of refusing"
            );
            let message = result.err().unwrap().to_string();
            assert!(!message.contains("retained-marker") && !message.contains("lost-marker"));
            assert_eq!(std::fs::read(&path).unwrap(), duplicate.as_bytes());
        }
    }
    for invalid in [
        format!("{valid}{{}}"),
        format!("{}0{}", "[".repeat(256), "]".repeat(256)),
        "{\"truncated\": ".into(),
    ] {
        std::fs::write(&path, &invalid).unwrap();
        assert!(OperationJournal::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), invalid.as_bytes());
    }
    // A uniquely named, otherwise identical legacy image still migrates/reopens.
    std::fs::write(&path, valid).unwrap();
    let reopened = f.journal();
    let r = reopened.bind("logical-op").unwrap().record().unwrap();
    assert_eq!(r.attempts[0].bytes, bytes);
    assert_eq!(r.attempts[0].outcome, AttemptOutcome::Unresolved);
    assert_eq!(
        r.attempts[0].extensions.get("opaque"),
        Some(&json!({"proof":"retained-marker"}))
    );
    client.close().await;
}

#[tokio::test]
async fn schema3_review_concurrent_replacement_cannot_return_stale_rebuildable_readiness() {
    use serde_json::value::RawValue;
    use subxt_rpcs::client::{RawRpcFuture, RawRpcSubscription, RpcClientT};
    struct PausedHistory {
        fake: FakeChain,
        once: std::sync::atomic::AtomicBool,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    impl RpcClientT for PausedHistory {
        fn request_raw<'a>(
            &'a self,
            method: &'a str,
            params: Option<Box<RawValue>>,
        ) -> RawRpcFuture<'a, Box<RawValue>> {
            Box::pin(async move {
                let reply = self.fake.request_raw(method, params).await?;
                if method == "chain_getBlock" && self.once.swap(false, Ordering::SeqCst) {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                Ok(reply)
            })
        }
        fn subscribe_raw<'a>(
            &'a self,
            method: &'a str,
            params: Option<Box<RawValue>>,
            unsubscribe: &'a str,
        ) -> RawRpcFuture<'a, RawRpcSubscription> {
            self.fake.subscribe_raw(method, params, unsubscribe)
        }
    }
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    fake.mutate_state(|s| s.advance(unsigned.expiry().unwrap() + 2 - s.block));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let stalled = Client::from_rpc(
        subxt_rpcs::RpcClient::new(PausedHistory {
            fake: fake.clone(),
            once: std::sync::atomic::AtomicBool::new(true),
            entered: entered.clone(),
            release: release.clone(),
        }),
        config,
    )
    .await
    .unwrap();
    let replacement = async {
        entered.notified().await;
        assert!(matches!(
            b.recover(&client, 32).await.unwrap(),
            RecoveryStatus::Rebuildable
        ));
        let (fresh, bytes, hash) = prepared(&client, &signer).await;
        b.signed(&fresh, &bytes, hash).await.unwrap();
        assert_eq!(b.record().unwrap().attempts.len(), 2);
        release.notify_one();
    };
    let (result, ()) = tokio::join!(b.recover(&stalled, 32), replacement);
    assert!(
        matches!(
            result.unwrap(),
            RecoveryStatus::Unresolved(RecoveryReason::PendingAttempt { attempt: 1 })
        ),
        "new unresolved attempt falsely became rebuildable"
    );
    assert_eq!(
        b.record().unwrap().attempts[1].outcome,
        AttemptOutcome::Unresolved
    );
    assert!(b.can_dispatch().is_err());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
    stalled.close().await;
}

fn image_for_schema(mut raw: Value, schema: u32) -> Value {
    raw["schema_version"] = json!(schema);
    if schema < JOURNAL_SCHEMA {
        for record in raw["operations"].as_object_mut().unwrap().values_mut() {
            record.as_object_mut().unwrap().remove("mode");
            record.as_object_mut().unwrap().remove("shield_pairs");
            for attempt in record["attempts"].as_array_mut().unwrap() {
                for key in ["id", "role", "material_generation", "evidence_conflicts"] {
                    attempt.as_object_mut().unwrap().remove(key);
                }
            }
        }
    }
    raw
}
#[tokio::test]
async fn schema3_magic_object_syntax_cannot_normalize_original_opaque_evidence() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    drop(b);
    drop(j);
    let path = f.0.join("journal.json");
    let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let probes = [
        ("Number", r#"{"$serde_json::private::Number":"1"}"#),
        (
            "escaped-Number",
            r#"{"$serde_json::private::Numb\u0065r":"1"}"#,
        ),
        (
            "RawValue",
            r#"{"$serde_json::private::RawValue":"{\"proof\":1}"}"#,
        ),
        (
            "escaped-RawValue",
            r#"{"$serde_json::private::RawVal\u0075e":"{\"proof\":1}"}"#,
        ),
        (
            "RawValue-inner-duplicates",
            r#"{"$serde_json::private::RawValue":"{\"proof\":1,\"proof\":2}"}"#,
        ),
    ];
    let mut accepted = Vec::new();
    for schema in [2, JOURNAL_SCHEMA] {
        for (label, literal) in probes {
            for material in [false, true] {
                let mut candidate = image_for_schema(original.clone(), schema);
                if material {
                    candidate["operations"]["logical-op"]["attempts"][0]["material"] =
                        json!([{"shape-probe":null}]);
                } else {
                    candidate["shape-probe"] = Value::Null;
                }
                let text = candidate.to_string().replace(
                    "\"shape-probe\":null",
                    &format!("\"shape-probe\":{literal}"),
                );
                std::fs::write(&path, &text).unwrap();
                for _ in 0..2 {
                    match OperationJournal::open(&path) {
                        Ok(handle) => {
                            // Baseline records actual acceptance/normalization
                            // for every probe, rather than stopping at one case.
                            let _ = handle.snapshot().unwrap();
                            accepted.push((schema, label, material));
                        }
                        Err(error) => {
                            assert!(!error.to_string().contains("proof"));
                            assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
                        }
                    }
                }
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "literal magic-object inputs accepted: {accepted:?}"
    );
    client.close().await;
}
#[tokio::test]
async fn schema3_magic_refusal_keeps_actual_numbers_normal_objects_and_marker_strings() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, _, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let (unsigned, bytes, hash) = prepared(&client, &signer).await;
    b.signed(&unsigned, &bytes, hash).await.unwrap();
    drop(b);
    drop(j);
    let path = f.0.join("journal.json");
    let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let numeric = r#"[9223372036854775808123456789012345,-9223372036854775808123456789012345,1.2345678901234567890123456789,1e1000,-1.23e-1000]"#;
    let expected: Value = serde_json::from_str(numeric).unwrap();
    for schema in [2, JOURNAL_SCHEMA] {
        let mut raw = image_for_schema(original.clone(), schema);
        raw["numeric-probe"] = Value::Null;
        raw["marker-string"] = json!(r#"{"$serde_json::private::Number":"1"}"#);
        raw["ordinary-object"] = json!({"before":true,"marker":"$serde_json::private::RawValue", "$serde_json::private::NumberSuffix":"retained","after":null});
        let text = raw.to_string().replace(
            "\"numeric-probe\":null",
            &format!("\"numeric-probe\":{numeric}"),
        );
        std::fs::write(&path, text).unwrap();
        let handle = f.journal();
        let image = handle.snapshot().unwrap();
        assert_eq!(image.extensions.get("numeric-probe"), Some(&expected));
        assert_eq!(
            image.extensions.get("marker-string"),
            raw.get("marker-string")
        );
        assert_eq!(
            image.extensions.get("ordinary-object"),
            raw.get("ordinary-object")
        );
        handle
            .control(JournalControl {
                submissions_paused: true,
                ..Default::default()
            })
            .unwrap();
        drop(handle);
        let reopened = f.journal();
        assert_eq!(
            reopened.snapshot().unwrap().extensions.get("numeric-probe"),
            Some(&expected)
        );
    }
    client.close().await;
}

#[tokio::test]
async fn schema3_magic_material_or_shadow_fields_cannot_publish_an_unreadable_image() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let path = f.0.join("journal.json");
    let original = std::fs::read(&path).unwrap();
    for material in [
        json!({"$serde_json::private::Number":"1"}),
        json!({"nested":[{"$serde_json::private::RawValue":"null"}]}),
    ] {
        assert!(b.clone().material(material).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(b.record().unwrap().material_generation, 0);
    }
    assert!(
        j.update(|image| {
            image.extensions.insert("revision".into(), json!(0));
            Ok(())
        })
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let numeric: Value = serde_json::from_str("9223372036854775808123456789012345").unwrap();
    let selected = b.material(numeric.clone()).unwrap();
    assert_eq!(selected.record().unwrap().material, numeric);
    assert_eq!(selected.record().unwrap().material_generation, 1);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    drop(selected);
    drop(j);
    let reopened = f.journal();
    assert_eq!(
        reopened
            .bind("logical-op")
            .unwrap()
            .record()
            .unwrap()
            .material,
        numeric
    );
    client.close().await;
}

struct GatedSigner {
    inner: KeypairSigner,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Signer for GatedSigner {
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
async fn another_client(fake: &FakeChain) -> Client {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Client::from_rpc(subxt_rpcs::RpcClient::new(fake.clone()), config)
        .await
        .unwrap()
}
#[tokio::test]
async fn journal_account_reservation_excludes_other_ids_and_independent_client_nonce_caches() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let other = another_client(&fake).await;
    let a = begin(&j, &client, &signer);
    let mut intent = a.record().unwrap().intent;
    intent.id = "second-logical-id".into();
    intent.scope = "other-scheduler".into();
    let b = j.begin(intent).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = GatedSigner {
        inner: signer,
        entered: entered.clone(),
        release: release.clone(),
    };
    let call = rtao_runtime::calls::system::remark(vec![4])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let options = SubmitOptions {
        journal: Some(a.clone()),
        ..Default::default()
    };
    let options_b = SubmitOptions {
        journal: Some(b.clone()),
        ..Default::default()
    };
    let compete = async {
        entered.notified().await;
        assert!(matches!(
            other.submit(call.clone(), &slow.inner, &options_b).await,
            Err(crate::SubmitFailure::Client(crate::ClientError::Journal(
                JournalError::Owned
            )))
        ));
        assert!(b.record().unwrap().attempts.is_empty());
        release.notify_one();
    };
    let (first, ()) = tokio::join!(client.submit(call.clone(), &slow, &options), compete);
    assert!(first.unwrap().success);
    assert!(
        other
            .submit(call, &slow.inner, &options_b)
            .await
            .unwrap()
            .success
    );
    let first_nonce = a.record().unwrap().attempts[0].nonce;
    assert_eq!(b.record().unwrap().attempts[0].nonce, first_nonce + 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 2);
    client.close().await;
    other.close().await;
}
#[tokio::test]
async fn journal_account_reservation_does_not_block_a_different_signing_account() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let other = another_client(&fake).await;
    let a = begin(&j, &client, &signer);
    let key_b = KeypairSigner::from_seed(&[8; 32], rtao_primitives::CryptoType::Ed25519).unwrap();
    let mut intent = a.record().unwrap().intent;
    intent.id = "separate-account".into();
    intent.account = key_b.account();
    let b = j.begin(intent).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let slow = GatedSigner {
        inner: signer,
        entered: entered.clone(),
        release: release.clone(),
    };
    let call = rtao_runtime::calls::system::remark(vec![5])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    let options = SubmitOptions {
        journal: Some(a.clone()),
        ..Default::default()
    };
    let independent = async {
        entered.notified().await;
        assert!(
            other
                .submit(
                    call.clone(),
                    &key_b,
                    &SubmitOptions {
                        journal: Some(b.clone()),
                        ..Default::default()
                    }
                )
                .await
                .unwrap()
                .success
        );
        assert!(a.record().unwrap().attempts.is_empty());
        release.notify_one();
    };
    let (first, ()) = tokio::join!(client.submit(call.clone(), &slow, &options), independent);
    assert!(first.unwrap().success);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 2);
    client.close().await;
    other.close().await;
}
#[tokio::test]
async fn scoped_binding_cannot_reacquire_an_owner_after_its_original_reservation_ends() {
    let f = Fixture::new();
    let j = f.journal();
    let (client, fake, signer) = chain().await;
    let b = begin(&j, &client, &signer);
    let owner = b.reserve().unwrap();
    let scoped = owner.binding();
    let nested = scoped.reserve().unwrap();
    drop(owner);
    assert!(scoped.reserve().is_ok());
    drop(nested);
    assert!(matches!(scoped.reserve(), Err(JournalError::Owned)));
    let fresh = b.reserve().unwrap();
    drop(fresh);
    let call = rtao_runtime::calls::system::remark(vec![6])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    assert!(matches!(
        client
            .submit(
                call,
                &signer,
                &SubmitOptions {
                    journal: Some(scoped),
                    ..Default::default()
                }
            )
            .await,
        Err(crate::SubmitFailure::Client(crate::ClientError::Journal(
            JournalError::Owned
        )))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}
