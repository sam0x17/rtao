//! API-independent causal controls: identical source compiles before and after the correction.
use super::*;
use rtao_primitives::{AccountId, IntoCoreValue};
use rtao_runtime::models::ProxyType;

#[tokio::test]
async fn causal_direct_client_historical_call_identity() {
    let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
        .await
        .unwrap();
    lab.script([Inner::LostAcknowledgment]);
    let mut opts = rtao_client::ShieldOptions {
        period: ShieldPeriod::P4,
        wait: Wait::Finalization,
        submit: rtao_client::SubmitOptions {
            journal: Some(
                lab.begin("causal-direct", json!({"original":1}), Value::Null)
                    .unwrap(),
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    let old = lab.client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&old)
        .unwrap();
    {
        let f = lab
            .client
            .submit_shielded(call.clone(), lab.signer.as_ref(), &opts);
        tokio::pin!(f);
        let r = tokio::select! { r=&mut f=>panic!("before lost ack: {r:?}"), _=lab.lost_acknowledgment()=>{lab.node.drop_connection().unwrap();f.await} };
        assert!(matches!(
            r,
            Err(rtao_client::SubmitFailure::PendingShield { .. })
        ));
    }
    opts.submit.journal = None;
    lab.upgrade(482, 2, rtao_primitives::H256([43; 32]))
        .unwrap();
    lab.restart().await.unwrap();
    opts.submit.journal = Some(lab.bind("causal-direct").unwrap());
    let good = lab
        .client
        .submit_shielded(call, lab.signer.as_ref(), &opts)
        .await
        .unwrap();
    assert!(good.success);
    let other = rtao_runtime::calls::system::remark(vec![9, 8, 7])
        .compose(&old)
        .unwrap();
    let changed = lab
        .client
        .submit_shielded(other, lab.signer.as_ref(), &opts)
        .await;
    assert_eq!(lab.authors(), 1);
    assert_eq!(lab.signer.signatures(), 2);
    assert!(
        changed.is_err(),
        "different direct call returned historical success: {changed:?}"
    );
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
async fn causal_typed_dynamic_historical_origin_and_expected_identity() {
    let mut false_successes = Vec::new();
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
            match kind {
                "proxy" => {
                    request.execute.proxy_for = Some(AccountId([9; 32]));
                }
                "force" => {
                    request.execute.force_proxy_type = Some(ProxyType::NonTransfer);
                }
                "expected" => {
                    request.execute.expected_call = Some(
                        rtao_runtime::calls::system::remark(vec![9])
                            .compose(&old_runtime)
                            .unwrap(),
                    );
                }
                _ => (),
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
                if let Ok(recovered) = result {
                    false_successes.push((dynamic, kind));
                    assert!(recovered.0);
                }
            }
            assert_eq!(builds.load(Ordering::SeqCst), 1);
            assert_eq!(keys.unlocks(), 1);
            assert_eq!(lab.signer.signatures(), 2);
            assert_eq!(lab.authors(), 1);
        }
        drop(opts);
        drop(ex);
        lab.close().await;
    }
    assert!(
        false_successes.is_empty(),
        "different recovered requests returned success: {false_successes:?}"
    );
}
