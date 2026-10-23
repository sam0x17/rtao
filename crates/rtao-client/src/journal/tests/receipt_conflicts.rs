//! Real persisted-effect recovery through the client and native journal.
//! RPC headers/events are synthetic, not independently verified consensus.
use super::*;
use subxt_rpcs::client::{RawRpcFuture, RawRpcSubscription, RpcClientT};

async fn finalized_pair(
    journal: &Arc<OperationJournal>,
    client: &Client,
    fake: &FakeChain,
    signer: &KeypairSigner,
) -> (JournalBinding, Box<crate::ExtrinsicReport>) {
    let binding = begin(journal, client, signer);
    paired_record(client, signer, &binding, 1).await;
    pair_history(fake, binding.record().unwrap().attempts, true, false);
    let RecoveryStatus::Finalized(report) = binding.recover(client, 32).await.unwrap() else {
        panic!("initial finalized inner was not observed");
    };
    assert!(report.success);
    assert!(report.shield.is_some());
    (binding, report)
}

#[tokio::test]
async fn opposite_finalized_receipt_stays_conflicting_after_native_reopen() {
    let fixture = Fixture::new();
    let journal = fixture.journal();
    let (client, fake, signer) = chain().await;
    let (binding, report) = finalized_pair(&journal, &client, &fake, &signer).await;
    let before = binding.record().unwrap();
    // Exact same signed hash, block and height, but contradictory dispatch
    // evidence returned by the persisted-effect receipt lookup.
    pair_history(&fake, before.attempts.clone(), true, true);
    assert!(matches!(
        binding.recover(&client, 0).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    let original = before.attempt(before.shield_pairs[0].inner).unwrap();
    let expected_conflict = AttemptOutcome::Finalized {
        block: report.block_hash.unwrap(),
        number: report.block_number.unwrap(),
        success: false,
        extensions: Default::default(),
    };
    let current = binding.record().unwrap();
    let retained = current.attempt(original.id).unwrap();
    assert_eq!(retained.outcome, original.outcome);
    assert_eq!(retained.evidence_conflicts, vec![expected_conflict.clone()]);
    assert_eq!(retained.bytes, original.bytes);
    assert_eq!(retained.material, original.material);
    assert_eq!(
        current.shield_pairs[0].extensions,
        before.shield_pairs[0].extensions
    );
    assert_eq!(current.intent.correlation_id, before.intent.correlation_id);
    assert!(binding.can_dispatch().is_err());
    drop(binding);
    drop(journal);

    let reopened = fixture.journal();
    let binding = reopened.bind("logical-op").unwrap();
    // Returning to the earlier RPC view must not erase the encountered
    // contradiction or authorize another submission after restart.
    pair_history(&fake, binding.record().unwrap().attempts, true, false);
    for _ in 0..2 {
        assert!(matches!(
            binding.recover(&client, 0).await.unwrap(),
            RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
        ));
    }
    let current = binding.record().unwrap();
    assert_eq!(
        current.attempt(original.id).unwrap().evidence_conflicts,
        vec![expected_conflict]
    );
    assert_eq!(current.attempts.len(), before.attempts.len());
    assert_eq!(current.shield_pairs.len(), before.shield_pairs.len());
    assert!(binding.can_dispatch().is_err());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

type Pause = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[derive(Clone)]
struct ReceiptBarrier {
    fake: FakeChain,
    pause: Arc<std::sync::Mutex<Option<Pause>>>,
}
impl RpcClientT for ReceiptBarrier {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) -> RawRpcFuture<'a, Box<serde_json::value::RawValue>> {
        Box::pin(async move {
            if method == "chain_getBlock" {
                let pause = { self.pause.lock().unwrap().take() };
                if let Some((entered, release)) = pause {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            self.fake.request_raw(method, params).await
        })
    }
    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
        unsub: &'a str,
    ) -> RawRpcFuture<'a, RawRpcSubscription> {
        self.fake.subscribe_raw(sub, params, unsub)
    }
}

#[tokio::test]
async fn observer_conflict_during_receipt_rpc_prevents_stale_finalized_result() {
    let fixture = Fixture::new();
    let journal = fixture.journal();
    let (client, fake, signer) = chain().await;
    let (binding, report) = finalized_pair(&journal, &client, &fake, &signer).await;
    let rpc = ReceiptBarrier {
        fake: fake.clone(),
        pause: Arc::new(std::sync::Mutex::new(None)),
    };
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    let reader = Client::from_rpc(subxt_rpcs::RpcClient::new(rpc.clone()), cfg)
        .await
        .unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *rpc.pause.lock().unwrap() = Some((entered, resume));
    let mut contradiction = (*report).clone();
    contradiction.success = false;
    let observation = async {
        started.await.unwrap();
        // This write completes on the actual native journal while recovery's
        // receipt RPC is suspended. The original matching RPC view remains.
        binding.observed(&contradiction).await.unwrap();
        release.send(()).unwrap();
    };
    let (status, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(binding.recover(&reader, 0), observation)
    })
    .await
    .unwrap();
    assert!(matches!(
        status.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    let current = binding.record().unwrap();
    assert!(current.finalized_effect().is_err());
    assert_eq!(current.attempts[0].evidence_conflicts.len(), 1);
    assert!(binding.can_dispatch().is_err());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    reader.close().await;
    client.close().await;
}

#[tokio::test]
async fn mismatched_receipt_height_is_refused_without_attaching_foreign_evidence() {
    let fixture = Fixture::new();
    let journal = fixture.journal();
    let (client, fake, signer) = chain().await;
    let (binding, report) = finalized_pair(&journal, &client, &fake, &signer).await;
    let before = binding.record().unwrap();
    let mut body = fake.mutate_state(|state| {
        let request = rtao_testkit::RpcRequest {
            id: json!(0),
            method: "chain_getBlock".into(),
            params: json!([crate::hash_hex(report.block_hash.unwrap())]),
        };
        state.dispatch(&request).outcome.unwrap().unwrap()
    });
    body["block"]["header"]["number"] = json!("0x66");
    fake.mutate_state(|state| state.queue_reply("chain_getBlock", Reply::result(body)));
    assert!(matches!(
        binding.recover(&client, 0).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    let after = binding.record().unwrap();
    assert_eq!(after.attempts[0].outcome, before.attempts[0].outcome);
    assert!(after.attempts[0].evidence_conflicts.is_empty());
    let RecoveryStatus::Finalized(repeated) = binding.recover(&client, 0).await.unwrap() else {
        panic!("matching uncontradicted receipt should remain recoverable");
    };
    assert_eq!(repeated.hash, report.hash);
    assert_eq!(repeated.shield, report.shield);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}
