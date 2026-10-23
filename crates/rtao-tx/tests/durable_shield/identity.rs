//! Actual public recovery identity at the synthetic native TCP/file boundary.
use super::*;
use rtao_client::journal::{JournalError, JournalImage, RecoveryContextError as Reason};
use rtao_primitives::{AccountId, IntoCoreValue};
use rtao_runtime::models::ProxyType;
use rtao_signer::{Signer, SignerRole};
use rtao_testkit::journal_shield::LabSigner;

fn refused(error: &TxError, reason: Reason, hash: rtao_primitives::H256) {
    assert!(
        matches!(error.primary_cause(),TxError::RecoveryContext {reason:actual} if *actual==reason),
        "{error:?}"
    );
    let receipt = error
        .submitted_receipt()
        .expect("original receipt retained");
    assert_eq!(receipt.extrinsic_hash, Some(hash));
    assert!(receipt.success);
    assert_eq!(receipt.inclusion, rtao_client::Inclusion::Finalized);
}
async fn raw_pending(
    lab: &JournalLab,
    keys: &LabKeys,
    call: rtao_client::CallBytes,
    opts: &ExecuteOptions,
) {
    let ex = Executor::new(lab.client.clone());
    let future = ex.submit_call::<Coldkey, _>(call, keys, opts);
    tokio::pin!(future);
    let result = tokio::select! {
        early=&mut future=>panic!("returned before lost ACK: {early:?}"),
        _=lab.lost_acknowledgment()=>{lab.node.drop_connection().unwrap();future.await}
    };
    assert!(matches!(result, Err(TxError::PendingOperation { .. })));
}
// Only a closed task-owned fixture is edited to represent an older supported
// journal lacking optional adapter evidence. Signed bytes/outcomes/material and
// authorization are unchanged. This is not a production migration or repair.
fn legacy(path: &std::path::Path, id: &str, origin: bool) {
    let mut image: JournalImage = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let record = image.operations.get_mut(id).unwrap();
    let inner = record
        .attempts
        .iter_mut()
        .find(|a| matches!(a.role, rtao_client::journal::AttemptRole::Inner { .. }))
        .unwrap();
    if origin {
        let context = inner
            .extensions
            .get_mut("rtao_submission_context_v1")
            .unwrap()
            .as_object_mut()
            .unwrap();
        context.insert("version".into(), json!(1));
        context.remove("proxy_for");
        context.remove("force_proxy_type");
    } else {
        inner.extensions.remove("call_hash");
        inner
            .extensions
            .get_mut("rtao_submission_context_v1")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("call_hash");
    }
    image.validate().unwrap();
    std::fs::write(path, serde_json::to_vec(&image).unwrap()).unwrap();
}

#[tokio::test]
async fn raw_expected_call_and_legacy_identity_refuse_with_receipt_in_both_routes() {
    let disk = path();
    let mut lab = JournalLab::new(FakeState::default(), &disk, CryptoType::Ed25519)
        .await
        .unwrap();
    lab.script([Inner::LostAcknowledgment]);
    let keys = LabKeys::new(lab.signer.clone());
    let mut opts = options(
        lab.begin(
            "raw-context",
            json!({"request":"raw"}),
            json!({"original":1}),
        )
        .unwrap(),
    )
    .execute;
    opts.shielded = true;
    opts.period = Some(4);
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
    raw_pending(&lab, &keys, call.clone(), &opts).await;
    let original = lab.bind("raw-context").unwrap().record().unwrap().attempts[0].hash;
    drop(opts);
    lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
        .unwrap();
    lab.restart().await.unwrap();
    let mut opts = options(lab.bind("raw-context").unwrap()).execute;
    opts.shielded = true;
    opts.period = Some(4);
    let other = rtao_runtime::calls::system::remark(vec![9, 8, 7])
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
    for dynamic in [false, true] {
        let ex = Executor::new(lab.client.clone());
        opts.expected_call = None;
        let good = if dynamic {
            ex.submit_call_dyn(call.clone(), &keys, SignerRole::Coldkey, &opts)
                .await
        } else {
            ex.submit_call::<Coldkey, _>(call.clone(), &keys, &opts)
                .await
        }
        .unwrap();
        assert!(good.success);
        assert_eq!(good.extrinsic_hash, Some(original));
        opts.expected_call = Some(other.clone());
        let bad = if dynamic {
            ex.submit_call_dyn(call.clone(), &keys, SignerRole::Coldkey, &opts)
                .await
        } else {
            ex.submit_call::<Coldkey, _>(call.clone(), &keys, &opts)
                .await
        }
        .unwrap_err();
        refused(&bad, Reason::ExpectedCallMismatch, original);
        let changed = if dynamic {
            ex.submit_call_dyn(other.clone(), &keys, SignerRole::Coldkey, &opts)
                .await
        } else {
            ex.submit_call::<Coldkey, _>(other.clone(), &keys, &opts)
                .await
        }
        .unwrap_err();
        refused(&changed, Reason::CallMismatch, original);
    }
    drop(opts);
    lab.release_for_process().await.unwrap();
    legacy(&disk, "raw-context", false);
    lab.restart().await.unwrap();
    let mut opts = options(lab.bind("raw-context").unwrap()).execute;
    opts.shielded = true;
    opts.period = Some(4);
    let bad = Executor::new(lab.client.clone())
        .submit_call::<Coldkey, _>(call, &keys, &opts)
        .await
        .unwrap_err();
    refused(&bad, Reason::MissingCallHash, original);
    assert_eq!(lab.authors(), 1);
    assert_eq!(keys.unlocks(), 1);
    assert_eq!(lab.signer.signatures(), 2);
    assert_eq!(
        lab.bind("raw-context").unwrap().record().unwrap().material,
        json!({"original":1})
    );
    drop(opts);
    lab.close().await;
}

#[tokio::test]
async fn direct_client_recovery_checks_call_account_and_keeps_receipt_without_executor_context() {
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    lab.script([Inner::LostAcknowledgment]);
    let opts = rtao_client::ShieldOptions {
        period: ShieldPeriod::P4,
        wait: Wait::Finalization,
        submit: rtao_client::SubmitOptions {
            journal: Some(
                lab.begin("direct", json!({"direct":"original"}), Value::Null)
                    .unwrap(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
    {
        let operation = lab
            .client
            .submit_shielded(call.clone(), lab.signer.as_ref(), &opts);
        tokio::pin!(operation);
        let pending = tokio::select! {early=&mut operation=>panic!("early: {early:?}"),_=lab.lost_acknowledgment()=>{lab.node.drop_connection().unwrap();operation.await}};
        assert!(matches!(
            pending,
            Err(rtao_client::SubmitFailure::PendingShield { .. })
        ));
    }
    let original = lab.bind("direct").unwrap().record().unwrap().attempts[0].hash;
    drop(opts);
    lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
        .unwrap();
    lab.restart().await.unwrap();
    let opts = rtao_client::ShieldOptions {
        period: ShieldPeriod::P4,
        wait: Wait::Finalization,
        submit: rtao_client::SubmitOptions {
            journal: Some(lab.bind("direct").unwrap()),
            ..Default::default()
        },
        ..Default::default()
    };
    let other = rtao_runtime::calls::system::remark(vec![9, 8, 7])
        .compose(&lab.client.refresh_runtime().await.unwrap())
        .unwrap();
    let wrong = LabSigner::new(CryptoType::Sr25519).unwrap();
    assert_ne!(wrong.account(), lab.signer.account());
    for (requested, signer, reason) in [
        (
            other,
            lab.signer.as_ref() as &dyn Signer,
            Reason::CallMismatch,
        ),
        (call.clone(), &wrong as &dyn Signer, Reason::AccountMismatch),
    ] {
        let result = lab
            .client
            .submit_shielded(requested, signer, &opts)
            .await
            .unwrap_err();
        let rtao_client::SubmitFailure::RecoveryContext {
            receipt,
            cause: JournalError::RecoveryContext(actual),
        } = result
        else {
            panic!("lost known historical receipt");
        };
        assert_eq!(actual, reason);
        assert_eq!(receipt.hash, original);
        assert!(receipt.success);
    }
    assert_eq!(
        lab.client
            .submit_shielded(call.clone(), lab.signer.as_ref(), &opts)
            .await
            .unwrap()
            .hash,
        original
    );
    // A direct Client request has no executor adapter snapshot. The Executor
    // must refuse inventing one even though literal call identity is known.
    let keys = LabKeys::new(lab.signer.clone());
    let mut raw = options(lab.bind("direct").unwrap()).execute;
    raw.shielded = true;
    raw.period = Some(4);
    let error = Executor::new(lab.client.clone())
        .submit_call::<Coldkey, _>(call, &keys, &raw)
        .await
        .unwrap_err();
    refused(&error, Reason::MissingExecutorContext, original);
    assert_eq!(keys.unlocks(), 0);
    assert_eq!(wrong.preparations(), 0);
    assert_eq!(wrong.signatures(), 0);
    assert_eq!(lab.signer.signatures(), 2);
    assert_eq!(lab.authors(), 1);
    drop(raw);
    drop(opts);
    lab.close().await;
}

struct IntentProbe<const N: usize> {
    arg: u8,
    builds: Arc<AtomicU64>,
}
impl<const N: usize> IntentInterface for IntentProbe<N> {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[N];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":Self::SPEC.name,"arg":self.arg})
    }
}
impl<const N: usize> Intent for IntentProbe<N> {
    fn summary(&self) -> String {
        "historical identity fixture".into()
    }
    fn spend(&self) -> Spend {
        Spend::Bounded(Balance::from_rao(10))
    }
    async fn build<V: rtao_client::View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<Built, IntentError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let mut b =
            Built::new(rtao_runtime::calls::system::remark(vec![self.arg]).compose(cx.runtime)?);
        b.extras.insert("saved".into(), json!(self.arg));
        Ok(b)
    }
    async fn preflight<V: rtao_client::View + ?Sized>(
        &self,
        _: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            required_free: Some(Balance::from_rao(10)),
            available_free: Some(Balance::from_rao(1_000_000_000)),
            ..Default::default()
        })
    }
}
#[tokio::test]
async fn typed_dynamic_restore_original_dispatch_options_intent_and_review_without_modern_build() {
    for dynamic in [false, true] {
        let disk = path();
        let mut lab = JournalLab::new(FakeState::default(), &disk, CryptoType::Ed25519)
            .await
            .unwrap();
        lab.script([Inner::LostAcknowledgment]);
        let keys = LabKeys::new(lab.signer.clone());
        let builds = Arc::new(AtomicU64::new(0));
        let intent = IntentProbe::<0> {
            arg: 1,
            builds: builds.clone(),
        };
        let mut opts = options(
            lab.begin(
                "intent-context",
                intent.to_json(),
                json!({"saved":"material"}),
            )
            .unwrap(),
        );
        opts.execute.proxy_for = Some(AccountId([8; 32]));
        opts.execute.force_proxy_type = Some(ProxyType::Any);
        let old_runtime = lab.client.refresh_runtime().await.unwrap();
        let original_call = rtao_runtime::calls::proxy::proxy(
            rtao_core::DynValue::new(AccountId([8; 32]).into_core_value()),
            Some(ProxyType::Any),
            rtao_runtime::calls::system::remark(vec![1])
                .compose(&old_runtime)
                .unwrap(),
        )
        .compose(&old_runtime)
        .unwrap();
        let ex = Executor::new(lab.client.clone());
        {
            let operation = async {
                if dynamic {
                    ex.submit_shielded_dyn(&intent, &keys, &opts)
                        .await
                        .map(|_| ())
                } else {
                    ex.submit_shielded(&intent, &keys, &opts).await.map(|_| ())
                }
            };
            tokio::pin!(operation);
            let result = tokio::select! {early=&mut operation=>panic!("before author: {early:?}"),_=lab.lost_acknowledgment()=>{lab.node.drop_connection().unwrap();operation.await}};
            assert!(matches!(result, Err(TxError::PendingOperation { .. })));
        }
        let original = lab
            .bind("intent-context")
            .unwrap()
            .record()
            .unwrap()
            .attempts[0]
            .hash;
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        drop(opts);
        drop(ex);
        lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
            .unwrap();
        lab.restart().await.unwrap();
        let mut opts = options(lab.bind("intent-context").unwrap());
        opts.execute.proxy_for = Some(AccountId([8; 32]));
        opts.execute.force_proxy_type = Some(ProxyType::Any);
        opts.execute.expected_call = Some(original_call);
        let ex = Executor::new(lab.client.clone());
        for kind in ["same", "args", "intent", "proxy", "force", "expected"] {
            let mut request = opts.clone();
            let expected_reason = match kind {
                "proxy" => {
                    request.execute.proxy_for = Some(AccountId([9; 32]));
                    Reason::OriginMismatch
                }
                "force" => {
                    request.execute.force_proxy_type = Some(ProxyType::NonTransfer);
                    Reason::OriginMismatch
                }
                "expected" => {
                    request.execute.expected_call = Some(
                        rtao_runtime::calls::system::remark(vec![9])
                            .compose(&old_runtime)
                            .unwrap(),
                    );
                    Reason::ExpectedCallMismatch
                }
                _ => Reason::ExecutorMismatch,
            };
            let changed = IntentProbe::<0> {
                arg: if kind == "args" { 2 } else { 1 },
                builds: builds.clone(),
            };
            let other = IntentProbe::<1> {
                arg: 1,
                builds: builds.clone(),
            };
            let result = if dynamic {
                ex.submit_shielded_dyn(
                    if kind == "intent" {
                        &other as &dyn rtao_tx::DynIntent
                    } else {
                        &changed
                    },
                    &keys,
                    &request,
                )
                .await
                .map(|r| (r.success, r.data))
            } else if kind == "intent" {
                ex.submit_shielded(&other, &keys, &request)
                    .await
                    .map(|r| (r.success, r.data))
            } else {
                ex.submit_shielded(&changed, &keys, &request)
                    .await
                    .map(|r| (r.success, r.data))
            };
            if kind == "same" {
                let result = result.unwrap();
                assert!(result.0);
                assert_eq!(result.1["saved"], 1);
            } else {
                refused(&result.unwrap_err(), expected_reason, original);
            }
            assert_eq!(builds.load(Ordering::SeqCst), 1);
            assert_eq!(keys.unlocks(), 1);
            assert_eq!(lab.signer.signatures(), 2);
            assert_eq!(lab.authors(), 1);
        }
        drop(opts);
        drop(ex);
        lab.release_for_process().await.unwrap();
        legacy(&disk, "intent-context", true);
        lab.restart().await.unwrap();
        let mut opts = options(lab.bind("intent-context").unwrap());
        opts.execute.proxy_for = Some(AccountId([8; 32]));
        opts.execute.force_proxy_type = Some(ProxyType::Any);
        let error = Executor::new(lab.client.clone())
            .submit_shielded_dyn(&intent, &keys, &opts)
            .await
            .unwrap_err();
        refused(&error, Reason::MissingExecutorContext, original);
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(lab.authors(), 1);
        drop(opts);
        lab.close().await;
    }
}
