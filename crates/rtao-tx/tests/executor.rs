// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_fee_filters.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exercise the executor through the production Client and encoded FakeChain.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{Client, ClientConfig, Inclusion, Network, View};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::{AccountId, Balance, CryptoType, Spend, Tao};
use rtao_signer::{Coldkey, KeySource, KeyView, KeypairSigner, Signer, SignerError};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, Reply, SubmitScript};
use rtao_tx::{
    BuildCx, Built, ExecuteOptions, Executor, Intent, IntentError, IntentInterface,
    IntentPreflight, PlanOptions, Policy, PreflightCx, Signed, TxError,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Keys {
    signer: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
    unlock_delay: Duration,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(AccountId([2; 32]))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok([2; 32])
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
        self.unlocks.fetch_add(1, Ordering::SeqCst);
        if !self.unlock_delay.is_zero() {
            std::thread::sleep(self.unlock_delay);
        }
        Ok(Arc::new(self.signer.clone()))
    }
}

struct Remark {
    spend: u64,
    delay: Duration,
    builds: Arc<AtomicUsize>,
    blocked: bool,
}
impl Default for Remark {
    fn default() -> Self {
        Self {
            spend: 0,
            delay: Duration::ZERO,
            builds: Arc::new(AtomicUsize::new(0)),
            blocked: false,
        }
    }
}
impl IntentInterface for Remark {
    const SPEC: &'static rtao_catalog::IntentSpec =
        <rtao_intents::Transfer as IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":"transfer","amount_tao":"0.000000003","netuid":1})
    }
}
impl Intent for Remark {
    fn summary(&self) -> String {
        "test review".into()
    }
    fn spend(&self) -> Spend {
        Spend::Bounded(Balance::<Tao>::from_rao(self.spend))
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let mut built =
            Built::new(rtao_runtime::calls::system::remark(vec![3, 4, 5]).compose(cx.runtime)?);
        built.extras.insert("reviewed".into(), json!(true));
        Ok(built)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            effects: vec![format!(
                "dispatch={} payer={}",
                cx.dispatch_origin, cx.fee_payer
            )],
            blocks: if self.blocked {
                vec!["preflight denied".into()]
            } else {
                vec![]
            },
            ..Default::default()
        })
    }
}

async fn fixture() -> (Executor, FakeChain, Keys) {
    let fake = FakeChain::from_state(FakeState::default());
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    let keys = Keys {
        signer: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        unlocks: Arc::new(AtomicUsize::new(0)),
        unlock_delay: Duration::ZERO,
    };
    (Executor::new(client), fake, keys)
}

#[tokio::test]
async fn plan_and_preflight_do_not_unlock_or_change_cached_nonce() {
    let (executor, fake, keys) = fixture().await;
    executor.client().nonces().pin(&keys.signer.account(), 40);
    let plan = executor
        .plan(&Remark::default(), &keys, &PlanOptions::default())
        .await
        .unwrap();
    let preview = executor
        .preflight(&Remark::default(), &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok());
    assert!(preview.estimated_fee.is_some());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&keys.signer.account().to_string())
            .await,
        Some(40)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        plan.to_json()["docs_url"],
        json!("https://www.bittensor.com/docs/tx/transfer")
    );
    assert_eq!(
        plan.to_json()["args"],
        json!({"amount_tao":"0.000000003","netuid":1})
    );
}

#[tokio::test]
async fn native_policy_and_hard_blocks_refuse_before_unlocking() {
    let (executor, fake, keys) = fixture().await;
    let opts = ExecuteOptions {
        policy: Some(Policy {
            max_spend: Some(Balance::from_rao(2)),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(matches!(
        executor
            .execute(
                &Remark {
                    spend: 3,
                    ..Default::default()
                },
                &keys,
                &opts
            )
            .await,
        Err(TxError::Policy(_))
    ));
    assert!(matches!(
        executor
            .execute(
                &Remark {
                    blocked: true,
                    ..Default::default()
                },
                &keys,
                &ExecuteOptions::default()
            )
            .await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn admission_budget_includes_private_key_resolution() {
    let (executor, fake, mut keys) = fixture().await;
    keys.unlock_delay = Duration::from_millis(25);
    assert!(matches!(
        executor
            .execute(
                &Remark::default(),
                &keys,
                &ExecuteOptions {
                    submit_deadline: Some(Duration::from_millis(10)),
                    ..Default::default()
                }
            )
            .await,
        Err(TxError::Rpc(rtao_rpc::RpcError::Timeout { .. }))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn missing_fee_cap_refuses_while_uncapped_preview_remains_available() {
    let (executor, fake, keys) = fixture().await;
    let failed_probe = || Reply::error(-32601, "fee unavailable", None);
    fake.mutate_state(|state| state.queue_reply("state_call", failed_probe()));
    let plan = executor
        .plan(&Remark::default(), &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok());
    assert!(plan.fee.is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.contains("could not estimate fee"))
    );
    fake.mutate_state(|state| state.queue_reply("state_call", failed_probe()));
    let options = ExecuteOptions {
        policy: Some(Policy {
            max_fee: Some(Balance::from_rao(u64::MAX)),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(matches!(
        executor.execute(&Remark::default(), &keys, &options).await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_submits_exact_reviewed_call_and_merges_success_extras() {
    let (executor, fake, keys) = fixture().await;
    let intent = Remark::default();
    let plan = executor
        .plan(&intent, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let result = executor
        .execute(&intent, &keys, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    assert!(
        result
            .events
            .iter()
            .any(|event| event.matches("System", "ExtrinsicSuccess"))
    );
    assert_eq!(result.inclusion, Inclusion::Finalized);
    assert_eq!(result.data["reviewed"], json!(true));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let bytes = fake.state().lock().unwrap().submissions[0].clone();
    let submitted = runtime.core().decode_extrinsic(&bytes, true).unwrap();
    let reviewed = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            plan.call().as_bytes(),
            true,
        )
        .unwrap();
    assert_eq!(submitted.field("call"), Some(&reviewed));
}

#[tokio::test]
async fn proxy_uses_real_dispatch_identity_and_outer_fee_payer() {
    let (executor, _, keys) = fixture().await;
    let real = AccountId([8; 32]);
    let plan = executor
        .plan(
            &Remark::default(),
            &keys,
            &PlanOptions {
                proxy_for: Some(real),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        plan.effects[0],
        format!("dispatch={real} payer={}", keys.signer.account())
    );
    assert_eq!(plan.extras["proxy_for"], json!(real));
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let call = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            plan.call().as_bytes(),
            true,
        )
        .unwrap();
    assert_eq!(
        call.field("call_module").and_then(ValueExt::as_str),
        Some("Proxy")
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
}

struct DriftingCall {
    builds: AtomicUsize,
    wrapper: bool,
}
impl IntentInterface for DriftingCall {
    const SPEC: &'static rtao_catalog::IntentSpec = Remark::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Remark::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"transfer"})
    }
}
impl Intent for DriftingCall {
    fn summary(&self) -> String {
        "changing reviewed call".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let value = if self.wrapper {
            0
        } else {
            self.builds.fetch_add(1, Ordering::SeqCst)
        };
        let value = u8::try_from(value)
            .map_err(|_| IntentError::validation("fixture build counter overflow"))?;
        Ok(rtao_runtime::calls::system::remark(vec![value])
            .compose(cx.runtime)?
            .into())
    }
    async fn wrap_call<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
        call: rtao_client::CallBytes,
    ) -> Result<Built, IntentError> {
        if !self.wrapper {
            return Ok(call.into());
        }
        let value = u8::try_from(self.builds.fetch_add(1, Ordering::SeqCst))
            .map_err(|_| IntentError::validation("fixture wrapper counter overflow"))?;
        let marker = rtao_runtime::calls::system::remark(vec![value]).compose(cx.runtime)?;
        Ok(rtao_runtime::calls::utility::batch_all(vec![call, marker])
            .compose(cx.runtime)?
            .into())
    }
}

#[tokio::test]
async fn reviewed_call_guard_refuses_builder_and_wrapper_drift_before_unlock() {
    for wrapper in [false, true] {
        let (executor, fake, keys) = fixture().await;
        let intent = DriftingCall {
            builds: AtomicUsize::new(0),
            wrapper,
        };
        let plan = executor
            .plan(&intent, &keys, &PlanOptions::default())
            .await
            .unwrap();
        let error = executor
            .execute(
                &intent,
                &keys,
                &ExecuteOptions {
                    expected_call: Some(plan.call().clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("differs from the reviewed call"));
        assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn reviewed_call_guard_accepts_an_unchanged_semantic_call() {
    let (executor, fake, keys) = fixture().await;
    let intent = Remark::default();
    let plan = executor
        .plan(&intent, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let result = executor
        .execute(
            &intent,
            &keys,
            &ExecuteOptions {
                expected_call: Some(plan.call().clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    let submitted = fake.state().lock().unwrap().submissions.clone();
    assert_eq!(submitted.len(), 1);
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let decoded = runtime
        .core()
        .decode_extrinsic(&submitted[0], true)
        .unwrap();
    let reviewed = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            plan.call().as_bytes(),
            true,
        )
        .unwrap();
    assert_eq!(decoded.field("call"), Some(&reviewed));
}

#[tokio::test]
async fn dispatch_failure_is_returned_without_retry_or_success_extras() {
    let (executor, fake, keys) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::InBlockThenFinalized { events:json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]) }));
    let intent = Remark::default();
    let result = executor
        .execute(
            &intent,
            &keys,
            &ExecuteOptions {
                retries: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(
        result
            .events
            .iter()
            .any(|event| event.matches("System", "ExtrinsicFailed"))
    );
    assert!(result.fee.is_none());
    assert_eq!(result.error.unwrap().name.as_deref(), Some("BadOrigin"));
    assert!(result.data.is_empty());
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejected_pool_result_does_not_claim_pool_acceptance() {
    let (executor, fake, keys) = fixture().await;
    fake.mutate_state(|state| {
        state.queue_reply(
            "author_submitAndWatchExtrinsic",
            Reply::error(1014, "opaque", None),
        )
    });
    let result = executor
        .execute(&Remark::default(), &keys, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.inclusion, Inclusion::Unobserved);
    assert!(result.fate_unknown.is_none());
    assert!(result.data.is_empty());
}

#[tokio::test]
async fn synchronous_plan_and_unlock_overruns_never_broadcast() {
    let (executor, fake, mut keys) = fixture().await;
    let timeout = Duration::from_millis(10);
    let slow = Remark {
        delay: Duration::from_millis(25),
        ..Default::default()
    };
    assert!(matches!(
        executor
            .plan(
                &slow,
                &keys,
                &PlanOptions {
                    deadline: Some(timeout),
                    ..Default::default()
                }
            )
            .await,
        Err(TxError::Rpc(rtao_rpc::RpcError::Timeout { .. }))
    ));
    assert!(matches!(
        executor
            .preflight(
                &slow,
                &keys,
                &PlanOptions {
                    deadline: Some(timeout),
                    ..Default::default()
                }
            )
            .await,
        Err(TxError::Rpc(rtao_rpc::RpcError::Timeout { .. }))
    ));
    assert!(matches!(
        executor
            .execute(
                &slow,
                &keys,
                &ExecuteOptions {
                    deadline: Some(timeout),
                    ..Default::default()
                }
            )
            .await,
        Err(TxError::Rpc(rtao_rpc::RpcError::Timeout { .. }))
    ));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    keys.unlock_delay = Duration::from_millis(25);
    assert!(matches!(
        executor
            .execute(
                &Remark::default(),
                &keys,
                &ExecuteOptions {
                    deadline: Some(timeout),
                    ..Default::default()
                }
            )
            .await,
        Err(TxError::Rpc(rtao_rpc::RpcError::Timeout { .. }))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn unknown_transmitted_fate_is_retained_as_a_typed_result() {
    let (executor, fake, keys) = fixture().await;
    fake.queue_transport_error(
        "author_submitAndWatchExtrinsic",
        rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::SubmitError::FateUnknown {
            hash: rtao_rpc::H256([0; 32]),
        })),
    );
    let result = executor
        .execute(&Remark::default(), &keys, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result.fate_unknown.is_some());
    assert_eq!(result.inclusion, Inclusion::Unobserved);
    assert!(result.message.contains("in flight"));
}

#[tokio::test]
async fn receipt_rpc_failure_keeps_recovery_identity_without_retry_for_every_executor_entry() {
    for entry in 0..4 {
        let (executor, fake, keys) = fixture().await;
        let executor = executor.with_registry(Arc::new(rtao_intents::Registry));
        fake.mutate_state(|state| {
            state.set_handler("chain_getBlock", |_, _| {
                Reply::error(-32603, "public receipt failure sentinel", None)
            });
        });
        let args = json!({
            "dest_ss58": AccountId([12; 32]).to_string(),
            "amount_tao": "0.000000003",
            "keep_alive": true,
        });
        let intent: rtao_intents::Transfer = serde_json::from_value(args.clone()).unwrap();
        let dynamic = rtao_intents::AnyIntent::from_tool("transfer", args.clone()).unwrap();
        let opts = ExecuteOptions {
            retries: 4,
            reconcile: false,
            ..Default::default()
        };
        let result = match entry {
            0 => executor.execute(&intent, &keys, &opts).await.unwrap(),
            1 => executor
                .execute_dyn(dynamic.as_dyn(), &keys.signer, &opts)
                .await
                .unwrap(),
            2 => executor
                .execute_tool("transfer", args, &keys.signer, &opts)
                .await
                .unwrap(),
            _ => {
                let call = rtao_runtime::calls::system::remark(vec![3, 4, 5])
                    .compose(&executor.client().refresh_runtime().await.unwrap())
                    .unwrap();
                executor
                    .submit_call::<Coldkey, _>(call, &keys, &opts)
                    .await
                    .unwrap()
            }
        };
        assert!(!result.success);
        assert_eq!(result.inclusion, Inclusion::Unobserved);
        assert!(result.message.contains("public receipt failure sentinel"));
        let hash = result
            .fate_unknown
            .expect("recovery identity was discarded");
        let submissions = fake.state().lock().unwrap().submissions.clone();
        assert_eq!(submissions.len(), 1, "entry {entry} was rebroadcast");
        assert_eq!(
            rtao_core::ops::extrinsic_lookup::find(
                &json!({"block":{"extrinsics":[format!("0x{}", hex::encode(&submissions[0]))]}}),
                hash.0,
            )
            .unwrap(),
            Some(0),
        );
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&keys.signer.account().to_string())
                .await,
            None,
        );
    }
}

#[tokio::test]
async fn executor_deadlines_preserve_submitted_hash_across_all_entry_points() {
    for entry in 0..3 {
        for reconcile in [false, true] {
            let (executor, fake, keys) = fixture().await;
            let executor = executor.with_registry(Arc::new(rtao_intents::Registry));
            let authors = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&authors);
            fake.mutate_state(|state| {
                state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let mut reply = state.dispatch(request);
                    reply.notifications.clear();
                    reply
                });
            });
            let args = json!({
                "dest_ss58": AccountId([12; 32]).to_string(),
                "amount_tao": "0.000000003",
                "keep_alive": true,
            });
            let intent: rtao_intents::Transfer = serde_json::from_value(args.clone()).unwrap();
            let dynamic = rtao_intents::AnyIntent::from_tool("transfer", args.clone()).unwrap();
            let opts = ExecuteOptions {
                deadline: Some(Duration::from_millis(300)),
                submit_deadline: Some(Duration::from_millis(200)),
                reconcile,
                retries: 0,
                ..Default::default()
            };
            let result = match entry {
                0 => executor.execute(&intent, &keys, &opts).await.unwrap(),
                1 => executor
                    .execute_dyn(dynamic.as_dyn(), &keys.signer, &opts)
                    .await
                    .unwrap(),
                _ => executor
                    .execute_tool("transfer", args, &keys.signer, &opts)
                    .await
                    .unwrap(),
            };
            assert!(!result.success, "entry {entry}, reconcile {reconcile}");
            assert_eq!(result.inclusion, Inclusion::Unobserved);
            let hash = result
                .fate_unknown
                .expect("submitted identity was discarded");
            let bytes = {
                let shared = fake.state();
                let state = shared.lock().unwrap();
                assert_eq!(state.submissions.len(), 1);
                state.submissions[0].clone()
            };
            assert_eq!(authors.load(Ordering::SeqCst), 1);
            assert_eq!(
                rtao_core::ops::extrinsic_lookup::find(
                    &json!({"block":{"extrinsics":[format!("0x{}", hex::encode(bytes))]}}),
                    hash.0,
                )
                .unwrap(),
                Some(0),
            );
            assert_eq!(
                executor
                    .client()
                    .nonces()
                    .cached(&keys.signer.account().to_string())
                    .await,
                None
            );
        }
    }
}

#[tokio::test]
async fn signer_address_change_is_refused_before_submission() {
    struct Wrong(Keys);
    impl KeyView for Wrong {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            self.0.hotkey_address()
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            self.0.hotkey_public_key()
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(AccountId([3; 32]))
        }
    }
    impl KeySource<Coldkey> for Wrong {
        fn address(&self) -> Result<AccountId, SignerError> {
            self.coldkey_address()
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.0.signer().await
        }
    }
    let (executor, fake, keys) = fixture().await;
    assert!(matches!(
        executor
            .execute(&Remark::default(), &Wrong(keys), &ExecuteOptions::default())
            .await,
        Err(TxError::Signer(SignerError::InvalidSignature(_)))
    ));
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn transient_pool_retry_waits_for_one_head_and_reuses_reviewed_call() {
    let (executor, fake, keys) = fixture().await;
    fake.mutate_state(|state| {
        state.queue_reply(
            "author_submitAndWatchExtrinsic",
            Reply::error(1014, "opaque", None),
        )
    });
    let intent = Remark::default();
    let options = ExecuteOptions {
        retries: 1,
        deadline: Some(Duration::from_secs(2)),
        ..Default::default()
    };
    let result = executor.execute(&intent, &keys, &options);
    let advance = async {
        loop {
            if fake
                .requests()
                .iter()
                .any(|request| request.method == "chain_subscribeNewHeads")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
                fake.advance(1).unwrap();
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    let (result, ()) = tokio::join!(result, advance);
    assert!(result.unwrap().success);
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|request| request.method == "author_submitAndWatchExtrinsic")
            .count(),
        2
    );
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|request| request.method == "account_nextIndex")
            .count(),
        3
    ); // one fee probe and one lease per attempt
}

#[tokio::test]
async fn transaction_version_change_replans_once_and_rechecks_policy() {
    runtime_stamp_change_replans_once_and_refuses_a_second_change(false).await;
}

#[tokio::test]
async fn spec_version_change_replans_once_and_refuses_a_second_change() {
    runtime_stamp_change_replans_once_and_refuses_a_second_change(true).await;
}

async fn runtime_stamp_change_replans_once_and_refuses_a_second_change(spec_change: bool) {
    fn upgrade(fake: &FakeChain, spec_change: bool, revision: u32) {
        fake.mutate_state(|state| {
            let (spec_version, transaction_version) = if spec_change {
                (478 + revision, 1)
            } else {
                (478, 1 + revision)
            };
            state.runtime = Arc::new(
                rtao_core::runtime::Runtime::parse(
                    &state.runtime.metadata_bytes,
                    spec_version,
                    transaction_version,
                    state.runtime.ss58_format,
                )
                .unwrap(),
            );
        });
    }
    #[derive(Clone)]
    struct UpgradeKeys {
        keys: Keys,
        fake: FakeChain,
        spec_change: bool,
        upgrade_while_signing: bool,
    }
    impl KeyView for UpgradeKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            self.keys.hotkey_address()
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            self.keys.hotkey_public_key()
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            self.keys.coldkey_address()
        }
    }
    impl KeySource<Coldkey> for UpgradeKeys {
        fn address(&self) -> Result<AccountId, SignerError> {
            self.keys.address()
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            upgrade(&self.fake, self.spec_change, 1);
            let signer = self.keys.signer().await?;
            if self.upgrade_while_signing {
                struct UpgradeSigner {
                    signer: Arc<dyn Signer>,
                    fake: FakeChain,
                    spec_change: bool,
                }
                impl Signer for UpgradeSigner {
                    fn account(&self) -> AccountId {
                        self.signer.account()
                    }
                    fn crypto_type(&self) -> CryptoType {
                        self.signer.crypto_type()
                    }
                    fn sign<'a>(
                        &'a self,
                        request: &'a rtao_signer::SigningRequest<'a>,
                    ) -> rtao_signer::BoxFuture<'a, Result<rtao_signer::MultiSignature, SignerError>>
                    {
                        Box::pin(async move {
                            upgrade(&self.fake, self.spec_change, 2);
                            self.signer.sign(request).await
                        })
                    }
                }
                Ok(Arc::new(UpgradeSigner {
                    signer,
                    fake: self.fake.clone(),
                    spec_change: self.spec_change,
                }))
            } else {
                Ok(signer)
            }
        }
    }
    let (executor, fake, keys) = fixture().await;
    let keys = UpgradeKeys {
        keys,
        fake: fake.clone(),
        spec_change,
        upgrade_while_signing: false,
    };
    let intent = Remark::default();
    let result = executor
        .execute(&intent, &keys, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
    assert_eq!(keys.keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    let current = executor.client().refresh_runtime().await.unwrap();
    assert_eq!(current.spec_version(), 478 + u32::from(spec_change));
    assert_eq!(current.transaction_version(), 1 + u32::from(!spec_change));
    let (executor, fake, keys) = fixture().await;
    let keys = UpgradeKeys {
        keys,
        fake: fake.clone(),
        spec_change,
        upgrade_while_signing: true,
    };
    let intent = Remark::default();
    let error = executor
        .execute(&intent, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        TxError::Chain(ref error) if error.message.contains("call runtime stamp")
    ));
    assert_eq!(intent.builds.load(Ordering::SeqCst), 2);
    assert_eq!(keys.keys.unlocks.load(Ordering::SeqCst), 1);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert_eq!(
        executor
            .client()
            .nonces()
            .cached(&keys.keys.signer.account().to_string())
            .await,
        None
    );
}

#[tokio::test]
// pyt: unit/test_intents_table.py::test_tools_catalog_matches_registry
async fn tools_project_all_authenticated_schemas_into_python_manifest_fields() {
    let (executor, _, _) = fixture().await;
    let tools = executor.tools().unwrap();
    assert_eq!(tools.len(), 83);
    assert!(tools.windows(2).all(|pair| pair[0].name < pair[1].name));
    for (spec, tool) in rtao_catalog::INTENTS.iter().zip(tools) {
        let catalog: Value = serde_json::from_str(spec.catalog_json).unwrap();
        let expected = json!({
            "name":catalog["name"],
            "summary":catalog["summary"],
            "description":catalog["description"],
            "signer":catalog["signer"],
            "origin":catalog["origin"],
            "verify":catalog["verify"],
            "wraps":catalog["wraps"],
            "docs_url":format!("https://www.bittensor.com/docs/tx/{}",spec.name.replace('_', "-")),
            "input_schema":catalog["input_schema"]
        });
        // Compare every manifest field, including signer/origin/verify/wraps.
        // Only the catalog's relative docs URL is made absolute by list_tools.
        let actual = serde_json::to_value(tool).unwrap();
        // These are independent manifest invariants, not only equality with
        // the catalog used to construct the tools. Check every exported row.
        assert!(
            actual["summary"]
                .as_str()
                .is_some_and(|summary| !summary.is_empty()),
            "{} tool summary is empty or not a string",
            spec.name
        );
        assert_eq!(
            actual["input_schema"]["type"], "object",
            "{} schema",
            spec.name
        );
        assert!(
            matches!(actual["signer"].as_str(), Some("coldkey" | "hotkey")),
            "{} tool signer is not a supported role",
            spec.name
        );
        assert_eq!(actual, expected);
    }
}

#[tokio::test]
async fn coldkey_fee_warning_precedes_failed_fee_probe_and_stays_out_of_preflight() {
    struct HotWeights;
    impl IntentInterface for HotWeights {
        const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[61];
        type Role = rtao_signer::Hotkey;
        type Origin = Signed;
        type Output = ();
        const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
            &[&rtao_runtime::calls::subtensor_module::SET_WEIGHTS];
        fn to_json(&self) -> Value {
            json!({"op":"set_weights","netuid":1})
        }
    }
    impl Intent for HotWeights {
        fn summary(&self) -> String {
            "weights".into()
        }
        async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
            Ok(Built::new(
                rtao_runtime::calls::subtensor_module::set_weights(
                    rtao_primitives::NetUid(1),
                    vec![1],
                    vec![65535],
                    0,
                )
                .compose(cx.runtime)?,
            ))
        }
        async fn warnings<V: View + ?Sized>(
            &self,
            _: &PreflightCx<'_, V>,
        ) -> Result<Vec<String>, IntentError> {
            Ok(vec!["intent warning".into()])
        }
    }
    let (executor, fake, keys) = fixture().await;
    fake.mutate_state(|state| {
        state.queue_reply("state_call", Reply::error(-32601, "fee unavailable", None))
    });
    let plan = executor
        .plan(&HotWeights, &keys.signer, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(plan.warnings[0], "intent warning");
    assert_eq!(plan.warnings[1], rtao_catalog::py::COLDKEY_FEE_WARNING);
    assert!(plan.warnings[2].starts_with("could not estimate fee:"));
    fake.mutate_state(|state| {
        state.queue_reply("state_call", Reply::error(-32601, "fee unavailable", None))
    });
    let preview = executor
        .preflight(&HotWeights, &keys.signer, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(preview.warnings, vec!["intent warning"]);
    assert!(preview.estimated_fee.is_none());
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    executor.client().close().await;
}

struct FeeFilterWallet {
    coldkey: AccountId,
    hotkey: AccountId,
    acquisitions: AtomicUsize,
}

impl KeyView for FeeFilterWallet {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.hotkey)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.hotkey.0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.coldkey)
    }
}

impl rtao_signer::DynKeySource for FeeFilterWallet {
    fn signer_for<'a>(
        &'a self,
        role: rtao_signer::SignerRole,
    ) -> rtao_signer::BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        self.acquisitions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { Err(SignerError::KeyUnavailable(role)) })
    }
}

async fn fee_filter_fixture() -> (Executor, FakeChain, FeeFilterWallet) {
    let (executor, fake, _) = fixture().await;
    // The upstream dev_wallet() uses these two public development identities.
    // Discard their test signers; planning receives public identity access only.
    let coldkey = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)
        .unwrap()
        .account();
    let hotkey = KeypairSigner::from_uri("//Alice//hot", CryptoType::Sr25519)
        .unwrap()
        .account();
    (
        executor,
        fake,
        FeeFilterWallet {
            coldkey,
            hotkey,
            acquisitions: AtomicUsize::new(0),
        },
    )
}

fn fee_filter_serve_axon() -> rtao_intents::AnyIntent {
    rtao_intents::AnyIntent::from_tool(
        "serve_axon",
        json!({"netuid":1,"ip":"203.0.113.5","port":8091}),
    )
    .unwrap()
}

fn fee_filter_expected_serve_axon() -> Vec<u8> {
    // Independent pinned SCALE: pallet/call, netuid, version, IPv4 integer,
    // port, IP kind, protocol and two reserved zero bytes.
    let mut expected = vec![7, 4];
    expected.extend_from_slice(&1_u16.to_le_bytes());
    expected.extend_from_slice(&1_u32.to_le_bytes());
    expected.extend_from_slice(&u128::from(u32::from_be_bytes([203, 0, 113, 5])).to_le_bytes());
    expected.extend_from_slice(&8091_u16.to_le_bytes());
    expected.extend_from_slice(&[4, 4, 0, 0]);
    expected
}

async fn assert_fee_filter_plan_is_public(
    executor: &Executor,
    fake: &FakeChain,
    wallet: &FeeFilterWallet,
) {
    assert_eq!(wallet.acquisitions.load(Ordering::SeqCst), 0);
    assert!(fake.mutate_state(|state| state.submissions.is_empty()));
    for account in [wallet.coldkey, wallet.hotkey] {
        assert_eq!(
            executor
                .client()
                .nonces()
                .cached(&account.to_string())
                .await,
            None
        );
    }
}

// pyt: unit/test_fee_filters.py::test_plan_warns_hotkey_coldkey_fee_routing
#[tokio::test]
async fn production_serve_axon_plan_warns_about_the_hotkey_owners_coldkey_fee() {
    let (executor, fake, wallet) = fee_filter_fixture().await;
    let intent = fee_filter_serve_axon();
    let plan = executor
        .plan_dyn(intent.as_dyn(), &wallet, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok());
    assert_eq!(plan.op, "serve_axon");
    assert_eq!(plan.signer, rtao_signer::SignerRole::Hotkey);
    assert_eq!(plan.signer_address, wallet.hotkey);
    assert_eq!(plan.call().as_bytes(), fee_filter_expected_serve_axon());
    assert_eq!(
        plan.warnings,
        ["transaction fee is charged to the hotkey owner's coldkey, not the signing hotkey"]
    );
    assert_fee_filter_plan_is_public(&executor, &fake, &wallet).await;
    executor.client().close().await;
}

// pyt: unit/test_fee_filters.py::test_plan_skips_warning_for_coldkey_signer
#[tokio::test]
async fn production_transfer_plan_has_no_hotkey_coldkey_fee_warning() {
    let (executor, fake, wallet) = fee_filter_fixture().await;
    let intent = rtao_intents::AnyIntent::from_tool(
        "transfer",
        json!({"dest_ss58":wallet.coldkey.to_string(),"amount_tao":1.0}),
    )
    .unwrap();
    let plan = executor
        .plan_dyn(intent.as_dyn(), &wallet, &PlanOptions::default())
        .await
        .unwrap();
    let mut expected = vec![5, 3, 0]; // Balances.transfer_keep_alive, MultiAddress::Id.
    expected.extend_from_slice(wallet.coldkey.as_bytes());
    expected.extend_from_slice(&[2, 40, 107, 238]); // Compact(1_000_000_000 rao).
    assert!(plan.ok());
    assert_eq!(plan.op, "transfer");
    assert_eq!(plan.signer, rtao_signer::SignerRole::Coldkey);
    assert_eq!(plan.signer_address, wallet.coldkey);
    assert_eq!(plan.call().as_bytes(), expected);
    assert!(plan.warnings.is_empty());
    assert_fee_filter_plan_is_public(&executor, &fake, &wallet).await;
    executor.client().close().await;
}

// pyt: unit/test_fee_filters.py::test_plan_skips_warning_when_proxy_for_set
#[tokio::test]
async fn production_proxy_serve_axon_plan_has_no_hotkey_coldkey_fee_warning() {
    let (executor, fake, wallet) = fee_filter_fixture().await;
    let intent = fee_filter_serve_axon();
    let plan = executor
        .plan_dyn(
            intent.as_dyn(),
            &wallet,
            &PlanOptions {
                proxy_for: Some(wallet.coldkey),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut expected = vec![16, 0, 0]; // Proxy.proxy, real MultiAddress::Id.
    expected.extend_from_slice(wallet.coldkey.as_bytes());
    expected.push(0); // force_proxy_type=None.
    expected.extend_from_slice(&fee_filter_expected_serve_axon());
    assert!(plan.ok());
    assert_eq!(plan.op, "serve_axon");
    assert_eq!(plan.signer, rtao_signer::SignerRole::Hotkey);
    assert_eq!(plan.signer_address, wallet.hotkey);
    assert_eq!(plan.call().as_bytes(), expected);
    assert_eq!(plan.extras["proxy_for"], json!(wallet.coldkey));
    assert!(plan.warnings.is_empty());
    assert_fee_filter_plan_is_public(&executor, &fake, &wallet).await;
    executor.client().close().await;
}

#[tokio::test]
async fn tool_dispatch_runs_the_feature_enabled_production_intent() {
    let (executor, fake, keys) = fixture().await;
    let executor = executor.with_registry(Arc::new(rtao_intents::Registry));
    let destination = AccountId([12; 32]);
    let spelling = destination.to_ss58(0);
    let args =
        json!({"op":"ignored","dest_ss58":spelling,"amount_tao":"0.000000003","keep_alive":true});
    let intent = rtao_intents::AnyIntent::from_tool("transfer", args.clone()).unwrap();
    let plan = executor
        .plan_dyn(intent.as_dyn(), &keys.signer, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(plan.args["dest_ss58"], json!(spelling));
    assert_eq!(plan.summary, format!("transfer τ0.000000003 to {spelling}"));
    let result = executor
        .execute_tool("transfer", args, &keys.signer, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let bytes = fake.state().lock().unwrap().submissions[0].clone();
    let decoded = runtime.core().decode_extrinsic(&bytes, true).unwrap();
    let reviewed = runtime
        .core()
        .decode_spec(
            &rtao_core::runtime::TypeSpec::Call,
            plan.call().as_bytes(),
            true,
        )
        .unwrap();
    assert_eq!(decoded.field("call"), Some(&reviewed));
    assert_eq!(
        decoded
            .field("call")
            .and_then(|call| call.field("call_function"))
            .and_then(ValueExt::as_str),
        Some("transfer_keep_alive")
    );
    assert!(
        executor
            .execute_tool(
                "unknown",
                json!({}),
                &keys.signer,
                &ExecuteOptions::default()
            )
            .await
            .is_err()
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}
