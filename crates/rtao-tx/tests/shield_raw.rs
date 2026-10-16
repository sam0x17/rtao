// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_multisig_safety.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exercise shield and raw entry points through signed FakeChain submissions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{Client, ClientConfig, Inclusion, Network, View};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::{AccountId, Balance, CryptoType, NetUid, Spend, Tao};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, Hotkey, KeySource, KeyView, KeypairSigner, MultiSignature,
    SignPurpose, Signer, SignerError, SignerRole, SigningRequest,
};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Executor, Intent, IntentError, IntentInterface,
    IntentPreflight, Policy, PreflightCx, ShieldOptions, ShieldPeriod, Signed, TxError,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

type SignedRequest = (SignPurpose, Vec<u8>, rtao_signer::SignerPayloadJson);

#[derive(Clone)]
struct RecordingSigner {
    key: KeypairSigner,
    requests: Arc<Mutex<Vec<SignedRequest>>>,
}
impl Signer for RecordingSigner {
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
            self.requests.lock().unwrap().push((
                request.purpose,
                request.call.to_vec(),
                request.payload_json.clone(),
            ));
            self.key.sign(request).await
        })
    }
}
struct Keys {
    signer: RecordingSigner,
    unlocks: AtomicUsize,
    allowed: SignerRole,
    change_address: bool,
    delay: Duration,
    upgrade: Option<FakeChain>,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
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
        self.signer_for(SignerRole::Coldkey).await
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            if role != self.allowed {
                return Err(SignerError::RoleUnavailable(role));
            }
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            if let Some(fake) = &self.upgrade {
                fake.mutate_state(|s|s.set_result("state_getRuntimeVersion",json!({"specName":"node-subtensor","specVersion":478,"transactionVersion":2,"apis":[]})));
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            if self.change_address {
                return Ok(
                    Arc::new(KeypairSigner::from_seed(&[9; 32], CryptoType::Ed25519).unwrap())
                        as Arc<dyn Signer>,
                );
            }
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
    }
}
async fn fixture() -> (Executor, FakeChain, Keys) {
    let mut state = FakeState::default();
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    state
        .seed_constant("Aura", "SlotDuration", json!(1))
        .unwrap();
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    (
        Executor::new(client),
        fake,
        Keys {
            signer: RecordingSigner {
                key: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
                requests: Arc::new(Mutex::new(vec![])),
            },
            unlocks: AtomicUsize::new(0),
            allowed: SignerRole::Coldkey,
            change_address: false,
            delay: Duration::ZERO,
            upgrade: None,
        },
    )
}
fn pool() -> ExecuteOptions {
    ExecuteOptions {
        wait_for_inclusion: false,
        wait_for_finalization: false,
        ..Default::default()
    }
}
async fn remark(ex: &Executor) -> rtao_client::CallBytes {
    rtao_runtime::calls::system::remark(vec![42])
        .compose(&ex.client().refresh_runtime().await.unwrap())
        .unwrap()
}
#[derive(Default)]
struct Required {
    required: Option<u64>,
    available: Option<u64>,
    builds: AtomicUsize,
    dispatch: Option<AccountId>,
}
impl IntentInterface for Required {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[0];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":"add_collateral"})
    }
}
impl Intent for Required {
    fn summary(&self) -> String {
        "required shield".into()
    }
    fn spend(&self) -> Spend {
        Spend::Bounded(Balance::from_rao(10))
    }
    fn touches_netuids(&self) -> smallvec::SmallVec<[NetUid; 2]> {
        smallvec::smallvec![NetUid(7)]
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let mut built =
            Built::new(rtao_runtime::calls::system::remark(vec![3, 4, 5]).compose(cx.runtime)?);
        built.extras.insert("reviewed".into(), json!(true));
        Ok(built)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        if let Some(dispatch) = self.dispatch {
            assert_eq!(cx.dispatch_origin, dispatch);
        }
        Ok(IntentPreflight {
            required_free: self.required.map(Balance::from_rao),
            available_free: self.available.map(Balance::from_rao),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn typed_and_dynamic_raw_roles_sign_the_exact_call() {
    for dynamic in [false, true] {
        let (ex, fake, keys) = fixture().await;
        let call = remark(&ex).await;
        let result = if dynamic {
            ex.submit_call_dyn(call.clone(), &keys, SignerRole::Coldkey, &pool())
                .await
        } else {
            ex.submit_call::<Coldkey, _>(call.clone(), &keys, &pool())
                .await
        }
        .unwrap();
        assert!(result.success);
        assert_eq!(result.inclusion, Inclusion::PoolAccepted);
        assert_eq!(keys.signer.requests.lock().unwrap()[0].1, call.as_bytes());
        let bytes = fake.state().lock().unwrap().submissions[0].clone();
        let runtime = ex.client().refresh_runtime().await.unwrap();
        let decoded = runtime.core().decode_extrinsic(&bytes, true).unwrap();
        let reviewed = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .unwrap();
        assert_eq!(decoded.field("call"), Some(&reviewed));
    }
    let (ex, fake, keys) = fixture().await;
    assert!(matches!(
        ex.submit_call_dyn(remark(&ex).await, &keys, SignerRole::Hotkey, &pool())
            .await,
        Err(TxError::Signer(SignerError::RoleUnavailable(
            SignerRole::Hotkey
        )))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    let result = ex
        .submit_call::<Hotkey, _>(remark(&ex).await, &keys.signer, &pool())
        .await
        .unwrap();
    assert!(result.success);
}

#[tokio::test]
async fn raw_review_guard_checks_bytes_and_both_runtime_stamps_before_unlock() {
    for dynamic in [false, true] {
        for drift in ["none", "bytes", "spec", "transaction"] {
            let (ex, fake, keys) = fixture().await;
            let call = remark(&ex).await;
            let runtime = ex.client().refresh_runtime().await.unwrap();
            let expected = match drift {
                "bytes" => rtao_runtime::calls::system::remark(vec![99])
                    .compose(&runtime)
                    .unwrap(),
                "spec" | "transaction" => {
                    let changed = rtao_runtime::RuntimeHandle::parse(
                        &runtime.core().metadata_bytes,
                        runtime.spec_version() + u32::from(drift == "spec"),
                        runtime.transaction_version() + u32::from(drift == "transaction"),
                        42,
                    )
                    .unwrap();
                    let expected = rtao_runtime::calls::system::remark(vec![42])
                        .compose(&changed)
                        .unwrap();
                    assert_eq!(expected.as_bytes(), call.as_bytes());
                    assert_eq!(expected.hash(), call.hash());
                    assert_ne!(expected.stamp(), call.stamp());
                    expected
                }
                _ => call.clone(),
            };
            let opts = ExecuteOptions {
                expected_call: Some(expected),
                ..pool()
            };
            let result = if dynamic {
                ex.submit_call_dyn(call.clone(), &keys, SignerRole::Coldkey, &opts)
                    .await
            } else {
                ex.submit_call::<Coldkey, _>(call.clone(), &keys, &opts)
                    .await
            };
            if drift == "none" {
                assert!(result.unwrap().success);
                assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
                assert_eq!(keys.signer.requests.lock().unwrap()[0].1, call.as_bytes());
                assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("differs from the reviewed call")
                );
                assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
                assert!(keys.signer.requests.lock().unwrap().is_empty());
                assert!(fake.state().lock().unwrap().submissions.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn matching_raw_review_does_not_bypass_the_raw_policy() {
    let (ex, fake, keys) = fixture().await;
    let call = remark(&ex).await;
    let ex = ex.with_policy(Policy::default());
    let error = ex
        .submit_call_dyn(
            call.clone(),
            &keys,
            SignerRole::Coldkey,
            &ExecuteOptions {
                expected_call: Some(call),
                ..pool()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, TxError::Policy(_)));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn shielded_review_guard_checks_the_rebuilt_call_before_private_signing() {
    for changed in [false, true] {
        let (ex, fake, keys) = fixture().await;
        let intent = Required::default();
        let plan = ex.plan(&intent, &keys, &Default::default()).await.unwrap();
        let expected = if changed {
            remark(&ex).await
        } else {
            plan.call().clone()
        };
        let result = ex
            .submit_shielded_dyn(
                &intent,
                &keys,
                &ShieldOptions {
                    execute: ExecuteOptions {
                        expected_call: Some(expected),
                        ..pool()
                    },
                    ..Default::default()
                },
            )
            .await;
        if changed {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("differs from the reviewed call")
            );
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert!(keys.signer.requests.lock().unwrap().is_empty());
            assert!(fake.state().lock().unwrap().submissions.is_empty());
        } else {
            assert!(result.unwrap().success);
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
            let requests = keys.signer.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].0, SignPurpose::ShieldInner);
            assert_eq!(requests[0].1, plan.call().as_bytes());
            assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        }
    }
}

#[tokio::test]
async fn raw_opt_in_is_checked_before_unlock_or_fee_probe() {
    let (ex, fake, keys) = fixture().await;
    let call = remark(&ex).await;
    let ex = ex.with_policy(Policy {
        max_fee: Some(Balance::ZERO),
        max_spend: Some(Balance::ZERO),
        allowed_netuids: Some(BTreeSet::new()),
        ..Default::default()
    });
    assert!(matches!(
        ex.submit_call::<Coldkey, _>(call.clone(), &keys, &pool())
            .await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(
        !fake
            .requests()
            .iter()
            .any(|r| r.method == "account_nextIndex")
    );
    let opts = ExecuteOptions {
        policy: Some(Policy {
            allow_raw_calls: true,
            ..Default::default()
        }),
        ..pool()
    };
    assert!(
        ex.submit_call::<Coldkey, _>(call, &keys, &opts)
            .await
            .unwrap()
            .success
    );
}

#[tokio::test]
async fn shielded_raw_periods_are_exact_and_default_maps_to_eight() {
    for period in [None, Some(128), Some(8), Some(4)] {
        let (ex, fake, keys) = fixture().await;
        let opts = ExecuteOptions {
            period,
            shielded: true,
            ..pool()
        };
        let result = ex
            .submit_call::<Coldkey, _>(remark(&ex).await, &keys, &opts)
            .await
            .unwrap();
        assert!(result.success);
        let requests = keys.signer.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0, SignPurpose::ShieldInner);
        assert_eq!(requests[1].0, SignPurpose::ShieldCarrier);
        assert_eq!(requests[0].2["era"], requests[1].2["era"]);
        assert_eq!(
            requests[0].2["era"],
            json!(if period == Some(4) {
                "0x0100"
            } else {
                "0x4200"
            })
        );
        assert_eq!(result.data["shielded"], true);
        assert!(
            result.data["inner_extrinsic_hash"]
                .as_str()
                .unwrap()
                .starts_with("0x")
        );
        let decoded = ex
            .client()
            .refresh_runtime()
            .await
            .unwrap()
            .core()
            .decode_extrinsic(&fake.state().lock().unwrap().submissions[0], true)
            .unwrap();
        assert_eq!(
            decoded
                .field("call")
                .unwrap()
                .field("call_module")
                .and_then(ValueExt::as_str),
            Some("MevShield")
        );
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        assert_eq!(
            ex.client()
                .nonces()
                .cached(&keys.signer.account().to_string())
                .await,
            Some(1)
        );
        assert!(
            ex.submit_call::<Coldkey, _>(remark(&ex).await, &keys, &pool())
                .await
                .unwrap()
                .success
        );
        assert_eq!(
            keys.signer.requests.lock().unwrap()[2].2["nonce"],
            json!("0x00000002")
        );
    }
    for period in [0, 5, 16, 64, 129] {
        let (ex, fake, keys) = fixture().await;
        let opts = ExecuteOptions {
            period: Some(period),
            shielded: true,
            ..pool()
        };
        assert!(matches!(
            ex.submit_call::<Coldkey, _>(remark(&ex).await, &keys, &opts)
                .await,
            Err(TxError::Intent(IntentError::InvalidArgument(_)))
        ));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_required_mev_shield_survives_multisig_dispatch
// Native adaptation: Required is a probe rather than BurnedRegister wrapped
// in a threshold-one adapter. This pins the required-shield redirect; typed
// adapter safety and unchanged ExecuteOptions cloning cover the remaining
// forwarding contract, rather than an identical combined transcript.
#[tokio::test]
async fn required_shield_routes_before_plan_and_preserves_extras() {
    let (ex, fake, keys) = fixture().await;
    let intent = Required::default();
    let result = ex.execute(&intent, &keys, &pool()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["reviewed"], true);
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert!(
        !fake
            .requests()
            .iter()
            .any(|r| r.method == "state_call" && r.params[0] == "TransactionPaymentApi_query_info")
    );
    assert_eq!(
        keys.signer.requests.lock().unwrap()[0].0,
        SignPurpose::ShieldInner
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn shield_semantic_policy_and_carrier_funds_fail_before_unlock() {
    for policy in [
        Policy {
            max_spend: Some(Balance::from_rao(9)),
            ..Default::default()
        },
        Policy {
            allowed_netuids: Some(BTreeSet::from([NetUid(1)])),
            ..Default::default()
        },
        Policy {
            max_fee: Some(Balance::ZERO),
            ..Default::default()
        },
    ] {
        let (ex, fake, keys) = fixture().await;
        let opts = ExecuteOptions {
            policy: Some(policy),
            ..pool()
        };
        assert!(matches!(
            ex.execute(&Required::default(), &keys, &opts).await,
            Err(TxError::Policy(_))
        ));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
    for failed_probe in [false, true] {
        let (ex, fake, keys) = fixture().await;
        if failed_probe {
            fake.mutate_state(|s| {
                s.queue_reply("state_call", Reply::error(-32603, "unavailable", None))
            });
        }
        let intent = Required {
            required: Some(100),
            available: Some(100),
            ..Default::default()
        };
        let error = ex.execute(&intent, &keys, &pool()).await.unwrap_err();
        assert!(matches!(error, TxError::Policy(_)));
        assert!(error.to_string().contains(if failed_probe {
            "could not verify free TAO"
        } else {
            "carrier fee"
        }));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn explicit_shield_dyn_honors_fee_cap_role_and_period() {
    let (ex, fake, keys) = fixture().await;
    let opts = ShieldOptions {
        period: ShieldPeriod::P4,
        execute: pool(),
        ..Default::default()
    };
    assert!(
        ex.submit_shielded_dyn(&Required::default(), &keys, &opts)
            .await
            .unwrap()
            .success
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    let (ex, fake, keys) = fixture().await;
    let opts = ShieldOptions {
        max_fee: Some(Balance::<Tao>::ZERO),
        execute: pool(),
        ..Default::default()
    };
    assert!(matches!(
        ex.submit_shielded(&Required::default(), &keys, &opts).await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn shield_never_retries_transient_carrier_rejection() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|s| s.queue_submit(SubmitScript::Invalid("Stale".into())));
    let opts = ExecuteOptions {
        retries: 4,
        wait_for_finalization: false,
        ..Default::default()
    };
    let result = ex
        .execute(&Required::default(), &keys, &opts)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 2);
    assert_eq!(
        ex.client()
            .nonces()
            .cached(&keys.signer.account().to_string())
            .await,
        None
    );
}

#[tokio::test]
async fn shield_era_failure_maps_to_typed_error() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|s| {
        s.set_finalized(94).unwrap();
    });
    assert!(matches!(
        ex.execute(&Required::default(), &keys, &pool()).await,
        Err(TxError::ShieldEraExpired {
            period: 8,
            finality_lag: 6
        })
    ));
    assert!(keys.signer.requests.lock().unwrap().is_empty());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn raw_and_shield_refuse_changed_signer_deadline_and_closed_client() {
    for shielded in [false, true] {
        let (ex, fake, mut keys) = fixture().await;
        keys.change_address = true;
        let opts = ExecuteOptions { shielded, ..pool() };
        assert!(matches!(
            ex.submit_call::<Coldkey, _>(remark(&ex).await, &keys, &opts)
                .await,
            Err(TxError::Signer(_))
        ));
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        let (ex, fake, mut keys) = fixture().await;
        keys.delay = Duration::from_millis(20);
        let opts = ExecuteOptions {
            shielded,
            submit_deadline: Some(Duration::from_millis(5)),
            ..pool()
        };
        assert!(
            ex.submit_call::<Coldkey, _>(remark(&ex).await, &keys, &opts)
                .await
                .is_err()
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        let (ex, fake, keys) = fixture().await;
        let call = remark(&ex).await;
        ex.client().close().await;
        assert!(matches!(
            ex.submit_call::<Coldkey, _>(call, &keys, &opts).await,
            Err(TxError::ConnectionNotReady)
        ));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn shield_unknown_fate_keeps_attempted_carrier_identity_without_retry() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|s| s.queue_submit(SubmitScript::Statuses(vec![json!("ready")])));
    let opts = ExecuteOptions {
        deadline: Some(Duration::from_millis(100)),
        retries: 4,
        wait_for_finalization: false,
        ..Default::default()
    };
    let result = ex
        .execute(&Required::default(), &keys, &opts)
        .await
        .unwrap();
    assert!(!result.success);
    let hash = result.fate_unknown.unwrap();
    let submissions = fake.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 1);
    assert_eq!(
        rtao_core::ops::extrinsic_lookup::find(
            &json!({"block":{"extrinsics":[format!("0x{}",hex::encode(&submissions[0]))]}}),
            hash.0
        )
        .unwrap(),
        Some(0)
    );
    assert_eq!(
        ex.client()
            .nonces()
            .cached(&keys.signer.account().to_string())
            .await,
        None
    );
}

#[tokio::test]
async fn shield_unavailable_inner_fee_is_a_policy_error_before_unlock() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|s| {
        s.queue_reply("state_call", Reply::error(-32603, "fee unavailable", None))
    });
    let opts = ShieldOptions {
        max_fee: Some(Balance::from_rao(u64::MAX)),
        execute: pool(),
        ..Default::default()
    };
    let error = ex
        .submit_shielded(&Required::default(), &keys, &opts)
        .await
        .unwrap_err();
    assert!(matches!(error, TxError::Policy(_)));
    assert!(
        error
            .to_string()
            .contains("could not estimate fee to enforce max_fee_tao")
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_required_mev_shield_passes_proxy_for
// Native adaptation: this probe pins proxy origin and inspects the signed inner
// Proxy wrapper. The Python test's BOB/Staking kwargs use typed option
// forwarding; this fixture does not run that full registration/multisig
// combination or assert a Staking proxy category.
#[tokio::test]
async fn shield_keeps_proxy_origin_and_reviewed_inner_call() {
    let (ex, fake, keys) = fixture().await;
    let real = AccountId([15; 32]);
    let intent = Required {
        dispatch: Some(real),
        ..Default::default()
    };
    let opts = ShieldOptions {
        execute: ExecuteOptions {
            proxy_for: Some(real),
            ..pool()
        },
        ..Default::default()
    };
    let result = ex.submit_shielded(&intent, &keys, &opts).await.unwrap();
    assert!(result.success);
    assert_eq!(result.data["proxy_for"], json!(real));
    let inner = keys.signer.requests.lock().unwrap()[0].1.clone();
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let decoded = runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, &inner, true)
        .unwrap();
    assert_eq!(
        decoded.field("call_module").and_then(ValueExt::as_str),
        Some("Proxy")
    );
    assert_eq!(
        decoded.field("call_function").and_then(ValueExt::as_str),
        Some("proxy")
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn shield_runtime_stamp_change_rebuilds_once_and_reuses_resolved_signer() {
    let (ex, fake, mut keys) = fixture().await;
    keys.upgrade = Some(fake.clone());
    let intent = Required::default();
    let result = ex.execute(&intent, &keys, &pool()).await.unwrap();
    assert!(result.success);
    assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        ex.client()
            .refresh_runtime()
            .await
            .unwrap()
            .transaction_version(),
        2
    );
}

#[tokio::test]
async fn missing_inner_retains_shield_metadata_and_reviewed_extras() {
    let (ex, fake, keys) = fixture().await;
    // Noninclusion is definite only after every era height is available and
    // expiry is finalized. A static head would correctly retain unknown fate.
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlockHash", |state, request| {
            if !state.submissions.is_empty()
                && let Some(height) = request.params[0].as_u64()
                && height > state.block
                && height <= 108
            {
                state.advance(height - state.block);
            }
            state.dispatch(request)
        });
    });
    let result = ex
        .submit_shielded(&Required::default(), &keys, &ShieldOptions::default())
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result.error.is_some());
    assert_eq!(result.fate_unknown, None);
    assert!(
        result
            .message
            .contains("not included before its era expired")
    );
    assert_eq!(result.data["shielded"], true);
    assert_eq!(result.data["reviewed"], true);
    assert!(
        result.data["inner_extrinsic_hash"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    let requests = fake.requests();
    let finality_check = requests
        .iter()
        .rposition(|request| request.method == "chain_getFinalizedHead")
        .expect("finalized expiry must be checked");
    let mut scanned: Vec<_> = requests[finality_check + 1..]
        .iter()
        .filter(|request| request.method == "chain_getBlockHash")
        .filter_map(|request| request.params[0].as_u64())
        .collect();
    scanned.sort_unstable();
    scanned.dedup();
    assert_eq!(scanned, (100..=108).collect::<Vec<_>>());
}

#[tokio::test]
async fn raw_admission_deadline_includes_initial_runtime_validation() {
    for dynamic in [false, true] {
        for shielded in [false, true] {
            let (ex, fake, keys) = fixture().await;
            let call = remark(&ex).await;
            fake.mutate_state(|s| {
                s.set_handler("state_getRuntimeVersion", |state, request| {
                    std::thread::sleep(Duration::from_millis(25));
                    state.dispatch(request)
                })
            });
            let opts = ExecuteOptions {
                shielded,
                submit_deadline: Some(Duration::from_millis(5)),
                ..pool()
            };
            let result = if dynamic {
                ex.submit_call_dyn(call, &keys, SignerRole::Coldkey, &opts)
                    .await
            } else {
                ex.submit_call::<Coldkey, _>(call, &keys, &opts).await
            };
            assert!(matches!(result, Err(TxError::Rpc(_))));
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert!(fake.state().lock().unwrap().submissions.is_empty());
        }
    }
}
