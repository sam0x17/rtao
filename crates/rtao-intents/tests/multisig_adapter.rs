// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_intents_table.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_multisig_safety.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public typed multisig adapters exercised through the executor and FakeChain.
#![cfg(feature = "all")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_catalog::IntentSpec;
use rtao_client::{Client, ClientConfig, DynView, Network, View};
use rtao_core::runtime::TypeSpec;
use rtao_intents::{MultisigAdapter, MultisigContinuation, MultisigThreshold1Adapter};
use rtao_primitives::{AccountId, Balance, CryptoType, IntoCoreValue, NetUid, Spend, Tao};
use rtao_runtime::{CallBytes, CallDescriptor, RuntimeHandle, models::MultisigTimepoint};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, Hotkey, KeySource, KeyView, KeypairSigner, Role, Signer,
    SignerError, SignerRole,
};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use rtao_tx::{
    BuildCx, Built, DynIntent, ExecuteOptions, Executor, FactsTitle, Intent, IntentError,
    IntentInterface, IntentPreflight, IntentSafety, Multisig, OriginMarker, OriginView,
    PlanOptions, Policy, PolicySubject, PreflightCx, Root, Signed, SystemClock, TxError,
};
use serde_json::{Value, json};
use smallvec::SmallVec;
use std::{
    collections::BTreeSet,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Keys {
    signer: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
    roles: Mutex<Vec<SignerRole>>,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.roles.lock().unwrap().push(role);
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.signer.clone()) as Arc<dyn Signer>)
        })
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

trait ProbeKind: Send + Sync + 'static {
    const SPEC: &'static IntentSpec;
    type Role: Role;
    type Origin: OriginMarker;
}
#[derive(Clone)]
struct Cold;
impl ProbeKind for Cold {
    const SPEC: &'static IntentSpec = <rtao_intents::Transfer as IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
}
#[derive(Clone)]
struct Hot;
impl ProbeKind for Hot {
    const SPEC: &'static IntentSpec = <rtao_intents::ServeAxon as IntentInterface>::SPEC;
    type Role = Hotkey;
    type Origin = Signed;
}
#[derive(Clone)]
struct RootCall;
impl ProbeKind for RootCall {
    // The wrapper test below guards this authenticated catalog position;
    // enabling the public M5 intent feature is unnecessary for this fixture.
    const SPEC: &'static IntentSpec = &rtao_catalog::INTENTS[59];
    type Role = Coldkey;
    type Origin = Root;
}

type PreviewInput = (&'static str, AccountId, AccountId, Vec<u8>);

#[derive(Clone)]
struct Probe<M> {
    builds: Arc<AtomicUsize>,
    build_origins: Arc<Mutex<Vec<AccountId>>>,
    preflight_inputs: Arc<Mutex<Vec<PreviewInput>>>,
    changing_bytes: bool,
    blocked: bool,
    all_subnets: bool,
    state_safety: bool,
    safety_origins: Arc<Mutex<Vec<(AccountId, &'static str)>>>,
    marker: PhantomData<M>,
}
impl<M> Default for Probe<M> {
    fn default() -> Self {
        Self {
            builds: Arc::new(AtomicUsize::new(0)),
            build_origins: Arc::new(Mutex::new(Vec::new())),
            preflight_inputs: Arc::new(Mutex::new(Vec::new())),
            changing_bytes: false,
            blocked: false,
            all_subnets: false,
            state_safety: false,
            safety_origins: Arc::new(Mutex::new(Vec::new())),
            marker: PhantomData,
        }
    }
}
impl<M: ProbeKind> IntentInterface for Probe<M> {
    const SPEC: &'static IntentSpec = M::SPEC;
    type Role = M::Role;
    type Origin = M::Origin;
    type Output = ();
    const WRAPS: &'static [&'static CallDescriptor] = &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op": M::SPEC.name, "amount_tao": "0.00000005", "netuid": 7})
    }
}
impl<M: ProbeKind> Intent for Probe<M> {
    fn summary(&self) -> String {
        "send 50 rao to the reviewed recipient".into()
    }
    fn spend(&self) -> Spend {
        Spend::Bounded(Balance::<Tao>::from_rao(50))
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        smallvec::smallvec![NetUid(7), NetUid(8)]
    }
    fn affects_all_subnets(&self) -> bool {
        self.all_subnets
    }
    async fn safety_at<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> Result<IntentSafety, IntentError> {
        let mut safety = self.safety()?;
        if self.state_safety {
            self.safety_origins
                .lock()
                .unwrap()
                .push((cx.coldkey_address()?, cx.spec.name));
            safety.spend = Spend::Bounded(Balance::from_rao(75));
            safety.netuids.push(NetUid(42));
            safety.contains_raw_calls = true;
            safety.mev_shield_required = true;
        }
        Ok(safety)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let origin = if M::SPEC.signer == "hotkey" {
            cx.hotkey_address(None)?
        } else {
            cx.coldkey_address()?
        };
        self.build_origins.lock().unwrap().push(origin);
        let build = self.builds.fetch_add(1, Ordering::SeqCst);
        let data = if self.changing_bytes {
            vec![u8::try_from(build).unwrap()]
        } else {
            vec![4, 5, 6]
        };
        let mut built = Built::new(rtao_runtime::calls::system::remark(data).compose(cx.runtime)?);
        built
            .extras
            .insert("semantic_extra".into(), json!("preserved"));
        Ok(built)
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        self.preflight_inputs.lock().unwrap().push((
            cx.spec.name,
            cx.dispatch_origin,
            cx.fee_payer,
            cx.call.as_bytes().to_vec(),
        ));
        Ok(IntentPreflight {
            effects: vec!["semantic effect".into()],
            warnings: vec!["semantic warning".into()],
            blocks: if self.blocked {
                vec!["semantic hard stop".into()]
            } else {
                vec![]
            },
            required_free: Some(Balance::from_rao(25)),
            available_free: Some(Balance::from_rao(30)),
            estimated_fee: Some(Balance::from_rao(123)),
            facts: vec![("reserved".into(), "25 rao".into())],
            facts_title: FactsTitle::Custom("semantic facts".into()),
            preview: serde_json::Map::new(),
        })
    }
}

async fn fixture(free: u64) -> (Executor, FakeChain, Keys) {
    let keys = Keys {
        signer: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        unlocks: Arc::new(AtomicUsize::new(0)),
        roles: Mutex::new(Vec::new()),
    };
    let mut state = FakeState::default();
    for member in [keys.signer.account(), AccountId([9; 32])] {
        state
            .seed_python(
                "System",
                "Account",
                &[json!(member.to_string())],
                json!({"data": {"free": free}}),
            )
            .unwrap();
    }
    let fake = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
        .await
        .unwrap();
    (Executor::new(client), fake, keys)
}

async fn group(executor: &Executor, keys: &Keys, threshold: u16) -> Multisig {
    executor
        .multisig(&[AccountId([9; 32]), keys.signer.account()], threshold)
        .await
        .unwrap()
}

async fn continuation(
    semantic: Probe<Cold>,
    executor: &Executor,
    keys: &Keys,
    group: &Multisig,
) -> MultisigContinuation {
    MultisigAdapter::new(
        semantic,
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap()
    .review(executor, keys, &PlanOptions::default())
    .await
    .unwrap()
    .1
}

fn set_free(fake: &FakeChain, keys: &Keys, free: u64) {
    fake.mutate_state(|state| {
        state
            .seed_python(
                "System",
                "Account",
                &[json!(keys.signer.account().to_string())],
                json!({"data":{"free":free}}),
            )
            .unwrap();
    });
}

fn decode(runtime: &RuntimeHandle, call: &CallBytes) -> Value {
    rtao_core::DynValue::new(
        runtime
            .core()
            .decode_spec(&TypeSpec::Call, call.as_bytes(), true)
            .unwrap(),
    )
    .to_json()
}

fn arg<'a>(call: &'a Value, name: &str) -> &'a Value {
    &call["call_args"]
        .as_array()
        .unwrap()
        .iter()
        .find(|arg| arg["name"] == name)
        .unwrap()["value"]
}

fn pending(fake: &FakeChain, group: &Multisig, inner: &CallBytes, approved: AccountId) {
    pending_at(
        fake,
        group,
        inner,
        approved,
        MultisigTimepoint {
            height: 12,
            index: 3,
        },
    );
}

fn pending_at(
    fake: &FakeChain,
    group: &Multisig,
    inner: &CallBytes,
    approved: AccountId,
    timepoint: MultisigTimepoint,
) {
    fake.mutate_state(|state| {
        state
            .seed_python(
                "Multisig",
                "Multisigs",
                &[
                    json!(group.address.to_string()),
                    json!(format!("0x{}", hex::encode(inner.hash()))),
                ],
                json!({
                    "when": timepoint,
                    "deposit": 280_800_000,
                    "depositor": approved,
                    "approvals": [approved],
                }),
            )
            .unwrap();
    });
}

fn pending_key(runtime: &RuntimeHandle, group: &Multisig, call: &CallBytes) -> Vec<u8> {
    let runtime = runtime.core();
    let storage = runtime.storage_entry("Multisig", "Multisigs").unwrap();
    runtime
        .storage_key(
            storage,
            &[
                group.address.into_core_value(),
                call.hash().into_core_value(),
            ],
        )
        .unwrap()
}

fn storage_keys_requested(fake: &FakeChain, since: usize) -> Vec<String> {
    fake.requests()
        .into_iter()
        .skip(since)
        .flat_map(|request| match request.method.as_str() {
            // Typed pinned reads use fetch -> storage_raw_many, including a
            // single-key fetch. Count every requested key in the batch.
            "state_queryStorageAt" => request.params[0]
                .as_array()
                .expect("state_queryStorageAt requires an array of keys")
                .iter()
                .map(|key| key.as_str().expect("storage key must be hex").to_owned())
                .collect::<Vec<_>>(),
            "state_getStorage" | "state_getStorageAt" => vec![
                request.params[0]
                    .as_str()
                    .expect("storage key must be hex")
                    .to_owned(),
            ],
            _ => Vec::new(),
        })
        .collect()
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_multisig_claim_reads_multisig_state_but_checks_member_balance
// Native adaptation: the Probe records group dispatch_origin and member
// fee_payer at the adapter boundary. The separate wrapped ClaimRoot reserve/read
// fixture checks those roles in the actual intent; this is compositional
// coverage, not a combined multisig ClaimRoot run.
#[tokio::test]
async fn plan_preserves_the_semantic_contract_and_exact_python_dispatch_shape() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let plan = executor
        .plan_dyn(&adapter, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(
        plan.to_json(),
        json!({
            "op": "multisig_execute",
            "summary": "multisig 2-of-2 execute: send 50 rao to the reviewed recipient",
            "signer": "coldkey",
            "signer_address": keys.signer.account(),
            "args": {
                "threshold": 2,
                "other_signatories": [AccountId([9; 32])],
                "call": semantic.to_json(),
                "timepoint": null,
            },
            "fee_tao": 0.000000123,
            "spend_tao": 0.00000005,
            "spend_unbounded": false,
            "effects": ["semantic effect", "multisig 2-of-2 execute transfer"],
            "warnings": ["semantic warning"],
            "violations": [],
            "ok": true,
            "preview": {},
            "docs_url": format!("{}/tx/multisig-execute", rtao_catalog::py::DOCS_URL),
        })
    );
    assert_eq!(*semantic.build_origins.lock().unwrap(), vec![group.address]);
    let inputs = semantic.preflight_inputs.lock().unwrap();
    assert_eq!(inputs[0].0, "transfer");
    assert_eq!(inputs[0].1, group.address);
    assert_eq!(inputs[0].2, keys.signer.account());
    assert_eq!(inputs[0].3, plan.call().as_bytes());
    assert_eq!(plan.extras["semantic_extra"], "preserved");
    let inner = adapter.reviewed_inner_call().unwrap().unwrap();
    assert_eq!(
        plan.extras["multisig_call_hash"],
        format!("0x{}", hex::encode(inner.hash()))
    );
    assert_eq!(
        plan.extras["multisig_call_data"],
        format!("0x{}", hex::encode(inner.as_bytes()))
    );
    assert_eq!(plan.call().stamp(), inner.stamp());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_saved_multisig_preserves_inner_policy_and_mev_contract
#[tokio::test]
async fn unbounded_multisig_policy_checks_match_the_semantic_plan_before_unlock() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    fake.mutate_state(|state| {
        state
            .seed_python("SubtensorModule", "Burn", &[json!(7)], json!(2_000_000))
            .unwrap();
        state
            .seed_python(
                "SubtensorModule",
                "CollateralLockShare",
                &[json!(7)],
                json!(0),
            )
            .unwrap();
    });
    let hotkey = AccountId([4; 32]);
    let semantic = rtao_intents::BurnedRegister {
        netuid: NetUid(7),
        hotkey_ss58: Some(hotkey.into()),
    };
    let adapter = MultisigThreshold1Adapter::new(
        semantic,
        executor.client(),
        group(&executor, &keys, 1).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let policy = Policy {
        max_spend: Some(Balance::from_rao(1_000_000_000)),
        allowed_netuids: Some(BTreeSet::from([NetUid(1)])),
        ..Default::default()
    };
    let expected = vec![
        "burned_register spends an amount that cannot be bounded before execution; max_spend_tao τ1.000000000 blocks it".to_owned(),
        "netuid 7 not in allowed_netuids [1]".to_owned(),
    ];
    let direct = policy.check(&adapter, None);
    assert_eq!(direct, expected);
    let plan = executor
        .plan(
            &adapter,
            &keys,
            &PlanOptions {
                policy: Some(policy.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(plan.op, "multisig_threshold_1");
    assert_eq!(plan.spend, Spend::Unbounded);
    assert_eq!(plan.violations, expected);
    assert_eq!(plan.violations, direct);
    assert!(!plan.ok());
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let outer = decode(&runtime, plan.call());
    assert_eq!(outer["call_module"], "Multisig");
    assert_eq!(outer["call_function"], "as_multi_threshold_1");
    let inner = rtao_runtime::calls::subtensor_module::burned_register(NetUid(7), hotkey)
        .compose(&runtime)
        .unwrap();
    assert_eq!(arg(&outer, "call"), &decode(&runtime, &inner));
    let error = executor
        .execute(
            &adapter,
            &keys,
            &ExecuteOptions {
                policy: Some(policy),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let TxError::Policy(error) = error else {
        panic!("unbounded multisig execution did not retain its policy refusal");
    };
    assert_eq!(error.violations, expected);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(keys.roles.lock().unwrap().is_empty());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    executor.client().close().await;
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_wrapped_registration_completion_uses_multisig_origin
#[tokio::test]
async fn wrapped_registration_completion_uses_group_origin_in_both_transports() {
    for shielded in [false, true] {
        let (executor, fake, keys) = fixture(1_000_000_000).await;
        let group = group(&executor, &keys, 1).await;
        let explicit_hotkey = AccountId([5; 32]);
        let registered = NetUid(9);
        assert_ne!(group.address, keys.signer.account());
        assert_ne!(explicit_hotkey, group.address);
        assert_ne!(explicit_hotkey, keys.signer.account());
        fake.mutate_state(|state| {
            state
                .seed(
                    &rtao_runtime::storage::subtensor_module::SUBNET_OWNER,
                    (registered,),
                    rtao_primitives::DefaultAccount::from(group.address),
                )
                .unwrap();
            state
                .seed(
                    &rtao_runtime::storage::subtensor_module::SUBNET_OWNER_HOTKEY,
                    (registered,),
                    rtao_primitives::DefaultAccount::from(explicit_hotkey),
                )
                .unwrap();
            state
                .seed(
                    &rtao_runtime::storage::subtensor_module::SUBNET_LOCKED,
                    (registered,),
                    rtao_primitives::TaoBalance(777),
                )
                .unwrap();
            state
                .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
                .unwrap();
        });
        let adapter = MultisigThreshold1Adapter::new(
            rtao_intents::RegisterSubnet {
                hotkey_ss58: Some(explicit_hotkey.into()),
            },
            executor.client(),
            group,
            keys.signer.account(),
            CryptoType::Ed25519,
            None,
        )
        .await
        .unwrap();
        let plan = executor
            .plan(&adapter, &keys, &PlanOptions::default())
            .await
            .unwrap();
        assert!(plan.ok(), "{:?}", plan.violations);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        let success = |index| json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]});
        let later_blocks = Arc::new(AtomicUsize::new(0));
        if shielded {
            // Match the deterministic Ed25519 inner extrinsic to its real
            // paired nonce and P8 era, as the shield receipt fixture does.
            let prepared = executor
                .client()
                .prepare_call(
                    plan.call().clone(),
                    &keys.signer,
                    &rtao_client::SubmitOptions {
                        nonce: rtao_client::NoncePolicy::Explicit(1),
                        mortality: rtao_client::Mortality::Mortal(rtao_client::Period::new(8)),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            let signature = keys
                .signer
                .sign(&prepared.signing_request(rtao_signer::SignPurpose::ShieldInner))
                .await
                .unwrap();
            let (inner_bytes, _) = executor
                .client()
                .attach_signature(&prepared, signature)
                .await
                .unwrap();
            let advanced = Arc::clone(&later_blocks);
            fake.mutate_state(|state| {
                state.set_handler("chain_getBlock", move |state, request| {
                    let mut reply = state.dispatch(request);
                    if let Some(Ok(raw)) = &mut reply.outcome
                        && raw["block"]["header"]["number"] == "0x65"
                    {
                        raw["block"]["extrinsics"]
                            .as_array_mut()
                            .unwrap()
                            .push(json!(format!("0x{}", hex::encode(&inner_bytes))));
                        state
                            .seed_events(
                                request.params[0].as_str().unwrap(),
                                json!([success(0), success(1)]),
                            )
                            .unwrap();
                        if state.block == 101 {
                            // The successful inner receipt has no registration
                            // event. Only deferred completion can find block102.
                            state.advance(1);
                            let hash = state.block_hash(102).unwrap();
                            state
                                .seed_events(
                                    hash,
                                    json!([{
                                        "phase":"Finalization","extrinsic_idx":null,
                                        "module_id":"SubtensorModule","event_id":"NetworkAdded",
                                        "attributes":[registered.0,0],"topics":[]
                                    }]),
                                )
                                .unwrap();
                            advanced.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    reply
                });
            });
        } else {
            fake.mutate_state(|state| {
                state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                    events: json!([
                        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NetworkAdded","attributes":[registered.0,0],"topics":[]},
                        success(0)
                    ]),
                });
            });
        }
        let options = ExecuteOptions {
            wait_for_finalization: false,
            registration_timeout: Some(std::time::Duration::from_secs(3)),
            deadline: Some(std::time::Duration::from_secs(10)),
            ..Default::default()
        };
        let result = if shielded {
            executor
                .submit_shielded(
                    &adapter,
                    &keys,
                    &rtao_tx::ShieldOptions {
                        execute: options,
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
        } else {
            executor.execute(&adapter, &keys, &options).await.unwrap()
        };
        assert!(result.success, "{}", result.message);
        assert_eq!(result.message, "Subnet 9 registered.");
        assert_eq!(result.data["netuid"], registered.0);
        assert_eq!(result.data["registration_mode"], "immediate");
        assert_eq!(result.data["registration_price_rao"], 777);
        assert_eq!(
            result.data["registered_at_block"],
            if shielded { 102 } else { 101 }
        );
        assert_eq!(
            result.extrinsic_id.as_deref(),
            Some(if shielded { "101-0001" } else { "101-0000" })
        );
        assert_eq!(result.inclusion, rtao_client::Inclusion::InBlock);
        assert_eq!(
            result
                .events
                .iter()
                .any(|event| event.matches("SubtensorModule", "NetworkAdded")),
            !shielded
        );
        assert_eq!(later_blocks.load(Ordering::SeqCst), usize::from(shielded));
        if shielded {
            assert_eq!(result.data["shielded"], true);
            assert!(
                fake.requests()
                    .iter()
                    .any(|request| request.method == "chain_subscribeNewHeads")
            );
        }
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(*keys.roles.lock().unwrap(), vec![SignerRole::Coldkey]);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        executor.client().close().await;
    }
}

#[tokio::test]
async fn preflight_retains_facts_amounts_and_semantic_policy_blocks() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold> {
        blocked: true,
        all_subnets: true,
        ..Default::default()
    };
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let options = PlanOptions {
        policy: Some(Policy {
            max_spend: Some(Balance::from_rao(49)),
            allowed_netuids: Some(BTreeSet::from([NetUid(7)])),
            ..Default::default()
        }),
        ..Default::default()
    };
    let plan = executor.plan_dyn(&adapter, &keys, &options).await.unwrap();
    assert_eq!(plan.violations.len(), 4);
    assert!(plan.violations[0].contains("spend"));
    assert!(plan.violations[1].contains("transfer acts on all subnets"));
    assert!(plan.violations[2].contains("netuid 8"));
    assert_eq!(plan.violations[3], "semantic hard stop");
    assert_eq!(PolicySubject::spend(&adapter), Intent::spend(&semantic));
    assert_eq!(
        PolicySubject::touches_netuids(&adapter),
        Intent::touches_netuids(&semantic)
    );
    assert!(PolicySubject::affects_all_subnets(&adapter));
    let preview = executor
        .preflight_dyn(&adapter, &keys, &options)
        .await
        .unwrap();
    assert_eq!(preview.required_free, Some(Balance::from_rao(25)));
    assert_eq!(preview.available_free, Some(Balance::from_rao(30)));
    assert_eq!(preview.estimated_fee, Some(Balance::from_rao(123)));
    assert_eq!(preview.facts, vec![("reserved".into(), "25 rao".into())]);
    assert_eq!(
        preview.facts_title,
        FactsTitle::Custom("semantic facts".into())
    );
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error, TxError::Policy(_)));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

// pyt: sdk/python/tests/unit/test_multisig_safety.py::test_saved_multisig_composes_sudo_and_proxy_inside_multisig
// Native input adaptation: a Root probe emits System.remark to isolate
// Multisig -> Proxy -> Sudo nesting. Actual AdminUtils [7]/enabled=true bytes
// and pending timepoint 12/3 are pinned by separate existing fixtures; this
// fixture does not reproduce the full pending AdminUtils transcript.
#[tokio::test]
async fn root_and_proxy_layers_remain_inside_the_outer_multisig() {
    assert_eq!(RootCall::SPEC.name, "set_subnet_emission_enabled");
    assert_eq!(RootCall::SPEC.origin, "root");
    assert_eq!(RootCall::SPEC.signer, "coldkey");
    let (executor, _, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<RootCall>::default();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let real = AccountId([11; 32]);
    let plan = executor
        .plan_dyn(
            &adapter,
            &keys,
            &PlanOptions {
                proxy_for: Some(real),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let outer = decode(&runtime, plan.call());
    assert_eq!(outer["call_module"], "Multisig");
    assert_eq!(outer["call_function"], "as_multi");
    let proxy = arg(&outer, "call");
    assert_eq!(proxy["call_module"], "Proxy");
    let sudo = arg(proxy, "call");
    assert_eq!(sudo["call_module"], "Sudo");
    assert_eq!(arg(sudo, "call")["call_module"], "System");
    assert_eq!(*semantic.build_origins.lock().unwrap(), vec![real]);
    assert_eq!(semantic.preflight_inputs.lock().unwrap()[0].1, real);
    assert_eq!(plan.extras["proxy_for"], json!(real));
    assert_eq!(
        decode(&runtime, &adapter.reviewed_inner_call().unwrap().unwrap()),
        *proxy
    );
}

#[tokio::test]
async fn hotkey_semantics_use_the_group_account_and_only_unlock_a_member_coldkey() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Hot>::default();
    let group = group(&executor, &keys, 1).await;
    let adapter = MultisigThreshold1Adapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let plan = executor
        .plan_dyn(&adapter, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(plan.op, "multisig_threshold_1");
    assert_eq!(plan.signer, SignerRole::Coldkey);
    assert_eq!(
        plan.args,
        json!({"other_signatories": [AccountId([9; 32])], "call": semantic.to_json()})
    );
    assert_eq!(*semantic.build_origins.lock().unwrap(), vec![group.address]);
    assert!(!plan.extras.contains_key("multisig_call_hash"));
    let before_execute = fake.requests();
    // Threshold one does not run the deposit/funds probe during composition.
    assert!(
        !before_execute
            .iter()
            .any(|request| request.method == "payment_queryInfo")
    );
    let result = executor
        .execute(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(*keys.roles.lock().unwrap(), vec![SignerRole::Coldkey]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn pending_timepoint_and_immutable_later_round_call_prevent_semantic_rebuilding() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold> {
        changing_bytes: true,
        ..Default::default()
    };
    let group = group(&executor, &keys, 2).await;
    let first = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let (first_plan, proof) = first
        .review(&executor, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(first.reviewed_timepoint().unwrap(), None);
    let inner = first.reviewed_inner_call().unwrap().unwrap();
    pending(&fake, &group, &inner, AccountId([9; 32]));
    let later = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof),
    )
    .await
    .unwrap()
    .with_opening_timepoint(MultisigTimepoint {
        height: 12,
        index: 3,
    });
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let storage_key = format!("0x{}", hex::encode(pending_key(&runtime, &group, &inner)));
    let before = fake.requests().len();
    let plan = executor
        .plan_dyn(&later, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
    assert_eq!(
        plan.extras["multisig_call_hash"],
        first_plan.extras["multisig_call_hash"]
    );
    assert_eq!(
        plan.extras["multisig_call_data"],
        first_plan.extras["multisig_call_data"]
    );
    assert_eq!(
        later.reviewed_inner_call().unwrap().unwrap().as_bytes(),
        inner.as_bytes()
    );
    assert_eq!(plan.args["timepoint"], json!({"height": 12, "index": 3}));
    assert_eq!(
        later.reviewed_timepoint().unwrap(),
        Some(MultisigTimepoint {
            height: 12,
            index: 3,
        })
    );
    assert_eq!(
        storage_keys_requested(&fake, before)
            .iter()
            .filter(|key| **key == storage_key)
            .count(),
        1,
        "a guarded approval must read its pending operation once"
    );
    assert_eq!(
        arg(&decode(&runtime, plan.call()), "maybe_timepoint"),
        &json!({"height": 12, "index": 3})
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplicate_pending_approval_refuses_before_weight_estimation_or_unlock() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let proof = continuation(semantic.clone(), &executor, &keys, &group).await;
    let inner = proof.call().clone();
    set_free(&fake, &keys, 0);
    pending(&fake, &group, &inner, keys.signer.account());
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof),
    )
    .await
    .unwrap();
    let before = fake.requests().len();
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already approved pending call"));
    assert!(
        !fake.requests()[before..]
            .iter()
            .any(|request| request.method == "payment_queryInfo")
    );
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn observed_deposit_plus_fee_shortfall_keeps_the_chain_error_and_never_unlocks() {
    let (executor, fake, keys) = fixture(0).await;
    let semantic = Probe::<Cold>::default();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    let TxError::Chain(error) = error else {
        panic!("funds check lost its typed chain error: {error}");
    };
    assert_eq!(error.code, rtao_catalog::ErrorCode::InsufficientBalance);
    assert!(
        error
            .message
            .contains("multisig deposit plus the transaction fee")
    );
    assert!(error.remediation().contains("deposit is returned"));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn actual_runtime_cap_refuses_a_previously_valid_group_before_fee_or_funds_probes() {
    let (executor, fake, keys) = fixture(0).await;
    let semantic = Probe::<Cold>::default();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let group = Multisig::new(
        &runtime,
        &[
            keys.signer.account(),
            AccountId([8; 32]),
            AccountId([9; 32]),
        ],
        2,
    )
    .unwrap();
    fake.mutate_state(|state| {
        state
            .seed_constant("Multisig", "MaxSignatories", json!(2))
            .unwrap();
        let upgraded =
            rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42).unwrap();
        state.runtime = Arc::new(upgraded);
        state.advance(2);
    });
    let before = fake.requests().len();
    let result = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await;
    let error = match result {
        Ok(_) => panic!("reduced runtime cap accepted a three-member group"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("MaxSignatories (2)"));
    assert!(!fake.requests()[before..].iter().any(|request| matches!(
        request.method.as_str(),
        "payment_queryInfo" | "state_getStorage" | "state_getStorageAt"
    )));
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn pending_storage_errors_propagate_and_do_not_turn_into_an_opening_approval() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6])
        .compose(&runtime)
        .unwrap();
    let storage_key = format!("0x{}", hex::encode(pending_key(&runtime, &group, &inner)));
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let before = fake.requests().len();
    fake.mutate_state(|state| {
        let expected_key = storage_key.clone();
        state.set_handler("state_queryStorageAt", move |_, request| {
            assert_eq!(request.params[0], json!([expected_key]));
            assert!(
                request.params[1].as_str().is_some(),
                "pending read must be pinned"
            );
            Reply::error(-32602, "pending lookup refused", None)
        });
    });
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("pending lookup refused"));
    assert_eq!(storage_keys_requested(&fake, before), vec![storage_key]);
    assert!(semantic.preflight_inputs.lock().unwrap().is_empty());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn wrapping_rechecks_a_cap_reduced_after_adapter_construction() {
    let (executor, fake, keys) = fixture(0).await;
    let semantic = Probe::<Cold>::default();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let group = Multisig::new(
        &runtime,
        &[
            keys.signer.account(),
            AccountId([8; 32]),
            AccountId([9; 32]),
        ],
        2,
    )
    .unwrap();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    fake.mutate_state(|state| {
        state
            .seed_constant("Multisig", "MaxSignatories", json!(2))
            .unwrap();
        let upgraded =
            rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42).unwrap();
        state.runtime = Arc::new(upgraded);
        state.advance(2);
    });
    let before = fake.requests().len();
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("MaxSignatories (2)"));
    assert!(!fake.requests()[before..].iter().any(|request| matches!(
        request.method.as_str(),
        "payment_queryInfo" | "state_getStorage" | "state_getStorageAt"
    )));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn opening_warning_precedes_semantic_warnings_and_preserves_semantic_facts() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let plan = executor
        .plan_dyn(&adapter, &keys, &PlanOptions::default())
        .await
        .unwrap();
    fake.mutate_state(|state| {
        state
            .seed_python(
                "System",
                "Account",
                &[json!(keys.signer.account().to_string())],
                json!({"data": {"free": 0}}),
            )
            .unwrap();
    });
    let snapshot = View::pin(executor.client()).await.unwrap();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let cx = PreflightCx {
        view: &snapshot as &dyn DynView,
        runtime: &runtime,
        clock: &SystemClock,
        spec: adapter.spec(),
        client: executor.client(),
        dispatch_origin: adapter.group().address,
        fee_payer: keys.signer.account(),
        call: plan.call(),
    };
    let preview = adapter.preflight_dyn(&cx).await.unwrap();
    assert_eq!(preview.warnings.len(), 2);
    assert!(preview.warnings[0].contains("opening approval reserves"));
    assert_eq!(preview.warnings[1], "semantic warning");
    assert_eq!(preview.facts, vec![("reserved".into(), "25 rao".into())]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn guarded_later_round_refuses_absent_or_replaced_opening_before_funds_or_unlock() {
    for replaced in [false, true] {
        let (executor, fake, keys) = fixture(1_000_000_000).await;
        let semantic = Probe::<Cold>::default();
        let group = group(&executor, &keys, 2).await;
        let runtime = executor.client().refresh_runtime().await.unwrap();
        let proof = continuation(semantic.clone(), &executor, &keys, &group).await;
        let inner = proof.call().clone();
        set_free(&fake, &keys, 0);
        if replaced {
            pending_at(
                &fake,
                &group,
                &inner,
                AccountId([9; 32]),
                MultisigTimepoint {
                    height: 13,
                    index: 4,
                },
            );
        }
        let storage_key = format!("0x{}", hex::encode(pending_key(&runtime, &group, &inner)));
        let adapter = MultisigAdapter::new(
            semantic.clone(),
            executor.client(),
            group,
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(proof),
        )
        .await
        .unwrap()
        .with_opening_timepoint(MultisigTimepoint {
            height: 12,
            index: 3,
        });
        let before = fake.requests().len();
        let error = executor
            .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(&error, TxError::SigningDisabled { .. }), "{error}");
        assert!(
            error
                .to_string()
                .contains(if replaced { "replaced" } else { "absent" })
        );
        let storage_reads = storage_keys_requested(&fake, before);
        assert_eq!(
            storage_reads.len(),
            1,
            "a refused continuation must not read member funds"
        );
        assert_eq!(storage_reads[0], storage_key);
        assert_eq!(adapter.reviewed_timepoint().unwrap(), None);
        assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        executor.client().close().await;
    }
}

#[tokio::test]
async fn guarded_existing_approval_preserves_the_duplicate_member_refusal() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let proof = continuation(semantic.clone(), &executor, &keys, &group).await;
    let inner = proof.call().clone();
    set_free(&fake, &keys, 0);
    pending(&fake, &group, &inner, keys.signer.account());
    let storage_key = format!("0x{}", hex::encode(pending_key(&runtime, &group, &inner)));
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof),
    )
    .await
    .unwrap()
    .with_opening_timepoint(MultisigTimepoint {
        height: 12,
        index: 3,
    });
    let before = fake.requests().len();
    let error = executor
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already approved"), "{error}");
    assert_eq!(
        storage_keys_requested(&fake, before),
        vec![storage_key],
        "a duplicate approval must read only the pending operation"
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn constructors_reject_malformed_groups_members_and_wrong_adapter_thresholds() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let member = keys.signer.account();
    let valid = group(&executor, &keys, 2).await;
    let mut wrong_address = valid.clone();
    wrong_address.address = AccountId([6; 32]);
    let mut duplicate = valid.clone();
    duplicate.signatories = vec![member, member];
    let mut zero_threshold = valid.clone();
    zero_threshold.threshold = 0;
    let mut excessive_threshold = valid.clone();
    excessive_threshold.threshold = 3;
    let mut single_member = valid.clone();
    single_member.signatories = vec![member];
    for malformed in [
        wrong_address,
        duplicate,
        zero_threshold,
        excessive_threshold,
        single_member,
    ] {
        assert!(
            MultisigAdapter::new(
                semantic.clone(),
                executor.client(),
                malformed,
                member,
                CryptoType::Ed25519,
                None,
            )
            .await
            .is_err()
        );
    }
    assert!(
        MultisigAdapter::new(
            semantic.clone(),
            executor.client(),
            valid.clone(),
            AccountId([6; 32]),
            CryptoType::Ed25519,
            None,
        )
        .await
        .is_err()
    );
    assert!(
        MultisigThreshold1Adapter::new(
            semantic.clone(),
            executor.client(),
            valid,
            member,
            CryptoType::Ed25519,
            None,
        )
        .await
        .is_err()
    );
    assert!(
        MultisigAdapter::new(
            semantic.clone(),
            executor.client(),
            group(&executor, &keys, 1).await,
            member,
            CryptoType::Ed25519,
            None,
        )
        .await
        .is_err()
    );
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn typed_approval_returns_the_exact_reviewed_call_in_multisig_output() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let adapter = MultisigAdapter::new(
        semantic,
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let plan = executor
        .plan(&adapter, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let reviewed = adapter.reviewed_inner_call().unwrap().unwrap();
    let result = executor
        .execute(
            &adapter,
            &keys,
            &ExecuteOptions {
                expected_call: Some(plan.call().clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(
        result.output.call_data.as_deref(),
        Some(reviewed.as_bytes())
    );
    assert_eq!(
        result.output.call_hash.map(|hash| hash.0),
        Some(reviewed.hash())
    );
    assert_eq!(result.data["semantic_extra"], "preserved");
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn pinned_calls_refuse_a_new_signing_runtime_at_construction_and_review() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let proof = continuation(semantic.clone(), &executor, &keys, &group).await;
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof.clone()),
    )
    .await
    .unwrap();
    fake.mutate_state(|state| {
        let upgraded =
            rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42).unwrap();
        state.runtime = Arc::new(upgraded);
        state.advance(2);
    });
    assert!(
        executor
            .plan_dyn(&adapter, &keys, &PlanOptions::default())
            .await
            .is_err()
    );
    assert!(
        MultisigAdapter::new(
            semantic.clone(),
            executor.client(),
            group,
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(proof),
        )
        .await
        .is_err()
    );
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn both_adapters_forward_state_dependent_safety_with_the_semantic_spec_and_origin() {
    let (executor, _, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold> {
        state_safety: true,
        ..Default::default()
    };
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let one = MultisigThreshold1Adapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 1).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let snapshot = View::pin(executor.client()).await.unwrap();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let real = AccountId([12; 32]);
    for (intent, origin) in [
        (&adapter as &dyn DynIntent, adapter.group().address),
        (&one as &dyn DynIntent, real),
    ] {
        let cx = BuildCx {
            view: &snapshot as &dyn DynView,
            origin: OriginView::Account(origin),
            runtime: &runtime,
            clock: &SystemClock,
            spec: intent.spec(),
        };
        let safety = intent.safety_at_dyn(&cx).await.unwrap();
        assert_eq!(safety.op, "transfer");
        assert_eq!(safety.spend, Spend::Bounded(Balance::from_rao(75)));
        assert!(safety.netuids.contains(&NetUid(42)));
        assert!(safety.contains_raw_calls);
        assert!(safety.mev_shield_required);
    }
    assert_eq!(
        *semantic.safety_origins.lock().unwrap(),
        vec![(adapter.group().address, "transfer"), (real, "transfer"),]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn continuations_refuse_different_semantic_arguments_and_canonical_groups() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let group = group(&executor, &keys, 2).await;
    let transfer = rtao_intents::Transfer {
        dest_ss58: AccountId([3; 32]).into(),
        amount_tao: Balance::<Tao>::from_rao(50).into(),
        keep_alive: true,
    };
    let initial = MultisigAdapter::new(
        transfer.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let (_, proof) = initial
        .review(&executor, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let mut different_amount = transfer.clone();
    different_amount.amount_tao = Balance::<Tao>::from_rao(1).into();
    let mut different_destination = transfer.clone();
    different_destination.dest_ss58 = AccountId([4; 32]).into();
    for changed in [different_amount, different_destination] {
        let error = match MultisigAdapter::new(
            changed,
            executor.client(),
            group.clone(),
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(proof.clone()),
        )
        .await
        {
            Ok(_) => panic!("an unrelated semantic intent inherited reviewed bytes"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("reviewed semantic intent"));
    }
    let other_group = executor
        .multisig(&[keys.signer.account(), AccountId([8; 32])], 2)
        .await
        .unwrap();
    assert!(
        MultisigAdapter::new(
            transfer,
            executor.client(),
            other_group,
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(proof),
        )
        .await
        .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn directly_wrapping_an_unrelated_call_cannot_mint_a_semantic_continuation() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group(&executor, &keys, 2).await,
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let snapshot = View::pin(executor.client()).await.unwrap();
    let dangerous = rtao_runtime::calls::balances::transfer_all(
        rtao_core::DynValue::new(AccountId([3; 32]).into_core_value()),
        false,
    )
    .compose(&runtime)
    .unwrap();
    let cx = BuildCx {
        view: &snapshot,
        origin: OriginView::Account(adapter.group().address),
        runtime: &runtime,
        clock: &SystemClock,
        spec: <Probe<Cold> as IntentInterface>::SPEC,
    };
    Intent::wrap_call(&adapter, &cx, dangerous.clone())
        .await
        .unwrap();
    assert_eq!(
        adapter.reviewed_inner_call().unwrap().unwrap().hash(),
        dangerous.hash()
    );
    // The only public proof producer performs its own complete executor review.
    let (_, proof) = adapter
        .review(&executor, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let expected = rtao_runtime::calls::system::remark(vec![4, 5, 6])
        .compose(&runtime)
        .unwrap();
    assert_eq!(proof.call().as_bytes(), expected.as_bytes());
    assert_ne!(proof.call().hash(), dangerous.hash());
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn hotkey_batch_continuations_preserve_dynamic_origin_and_exact_bytes_at_both_thresholds() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let batch = rtao_intents::Batch::new(vec![json!({"op":"reset_axon","netuid":7})]).unwrap();
    assert_eq!(
        <rtao_intents::Batch as IntentInterface>::SPEC.signer,
        "coldkey"
    );
    assert_eq!(Intent::signer_role(&batch).unwrap(), SignerRole::Hotkey);
    let many = group(&executor, &keys, 2).await;
    let initial = MultisigAdapter::new(
        batch.clone(),
        executor.client(),
        many.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let (_, proof) = initial
        .review(&executor, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let retained = proof.call().clone();
    let later = MultisigAdapter::new(
        batch.clone(),
        executor.client(),
        many,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof),
    )
    .await
    .unwrap();
    let plan = executor
        .plan(&later, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok(), "{:?}", plan.violations);
    assert_eq!(
        later.reviewed_inner_call().unwrap().unwrap().as_bytes(),
        retained.as_bytes()
    );

    let one = group(&executor, &keys, 1).await;
    let initial = MultisigThreshold1Adapter::new(
        batch.clone(),
        executor.client(),
        one.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let (_, proof) = initial
        .review(&executor, &keys, &PlanOptions::default())
        .await
        .unwrap();
    let retained = proof.call().clone();
    let later = MultisigThreshold1Adapter::new(
        batch,
        executor.client(),
        one,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof),
    )
    .await
    .unwrap();
    let plan = executor
        .plan(&later, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert!(plan.ok(), "{:?}", plan.violations);
    assert_eq!(
        later.reviewed_inner_call().unwrap().unwrap().as_bytes(),
        retained.as_bytes()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    executor.client().close().await;
}

#[tokio::test]
async fn pinned_proxy_layers_require_the_original_effective_semantic_origin() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let initial = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let real = AccountId([11; 32]);
    let options = PlanOptions {
        proxy_for: Some(real),
        ..Default::default()
    };
    let (_, proof) = initial.review(&executor, &keys, &options).await.unwrap();
    let adapter = MultisigAdapter::new(
        semantic.clone(),
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(proof.clone()),
    )
    .await
    .unwrap();
    for proxy_for in [None, Some(AccountId([12; 32]))] {
        let error = executor
            .plan_dyn(
                &adapter,
                &keys,
                &PlanOptions {
                    proxy_for,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("reviewed dispatch origin"));
    }
    let plan = executor.plan_dyn(&adapter, &keys, &options).await.unwrap();
    assert_eq!(
        plan.extras["multisig_call_data"],
        format!("0x{}", hex::encode(proof.call().as_bytes()))
    );
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

// pyt: unit/test_intents_table.py::TestExecuteFlow::test_submit_shielded_wraps_root_in_sudo
// pyt: unit/test_intents_table.py::TestExecuteFlow::test_submit_shielded_wraps_proxy
// These production intents prove wrapping before shield signing. The supplied
// carrier acceptance does not establish decryption, dispatch or runtime effects.
#[tokio::test]
async fn production_root_and_proxy_shield_sign_the_wrapped_semantic_call_before_the_carrier() {
    use rtao_signer::{MultiSignature, SignPurpose, SigningRequest};

    type SigningRecords = Vec<(SignPurpose, Vec<u8>)>;

    #[derive(Clone)]
    struct Capture {
        key: KeypairSigner,
        requests: Arc<Mutex<SigningRecords>>,
    }
    impl Signer for Capture {
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
                self.requests
                    .lock()
                    .unwrap()
                    .push((request.purpose, request.call.to_vec()));
                self.key.sign(request).await
            })
        }
    }

    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        for root in [true, false] {
            let (executor, fake, keys) = fixture(u64::MAX).await;
            let real = AccountId([3; 32]);
            fake.mutate_state(|state| {
                state
                    .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
                    .unwrap();
                state
                    .seed_python(
                        "System",
                        "Account",
                        &[json!(real)],
                        json!({"data":{"free":u64::MAX}}),
                    )
                    .unwrap();
                state.queue_submit(rtao_testkit::SubmitScript::Statuses(vec![json!("ready")]));
            });
            let signer = Capture {
                key: keys.signer.clone(),
                requests: Arc::new(Mutex::new(Vec::new())),
            };
            let intent: Box<dyn DynIntent> = if root {
                Box::new(rtao_intents::SetSubnetEmissionEnabled {
                    netuids: vec![NetUid(1)],
                    enabled: true,
                })
            } else {
                Box::new(rtao_intents::Transfer {
                    dest_ss58: real.into(),
                    amount_tao: Balance::<Tao>::from_rao(1_000_000_000).into(),
                    keep_alive: true,
                })
            };
            let options = rtao_tx::ShieldOptions {
                execute: ExecuteOptions {
                    proxy_for: (!root).then_some(real),
                    wait_for_inclusion: false,
                    wait_for_finalization: false,
                    ..Default::default()
                },
                ..Default::default()
            };
            let result = executor
                .submit_shielded_dyn(intent.as_ref(), &signer, &options)
                .await
                .unwrap();
            assert!(result.success, "{}", result.message);
            assert_eq!(result.inclusion, rtao_client::Inclusion::PoolAccepted);
            assert_eq!(result.data["shielded"], true);
            if root {
                assert!(!result.data.contains_key("proxy_for"));
            } else {
                assert_eq!(result.data["proxy_for"], json!(real));
            }
            let signed = signer.requests.lock().unwrap().clone();
            assert_eq!(
                signed.iter().map(|request| request.0).collect::<Vec<_>>(),
                [SignPurpose::ShieldInner, SignPurpose::ShieldCarrier]
            );
            // Fixed v478 SCALE, independent of intent build and wrapper helpers.
            // Root: Sudo12/0 -> AdminUtils19/94, netuid1, enabled=true.
            // Proxy: Proxy16/0, real Id, None, Balances5/3, dest Id, compact1TAO.
            let expected = if root {
                vec![12, 0, 19, 94, 1, 0, 1]
            } else {
                let mut bytes = vec![16, 0, 0];
                bytes.extend_from_slice(real.as_bytes());
                bytes.extend_from_slice(&[0, 5, 3, 0]);
                bytes.extend_from_slice(real.as_bytes());
                bytes.extend_from_slice(&[2, 40, 107, 238]);
                bytes
            };
            assert_eq!(signed[0].1, expected);
            let runtime = executor.client().refresh_runtime().await.unwrap();
            let inner = rtao_core::DynValue::new(
                runtime
                    .core()
                    .decode_spec(&TypeSpec::Call, &signed[0].1, true)
                    .unwrap(),
            )
            .to_json();
            assert_eq!(inner["call_module"], if root { "Sudo" } else { "Proxy" });
            assert_eq!(inner["call_function"], if root { "sudo" } else { "proxy" });
            let nested = arg(&inner, "call");
            assert_eq!(
                nested["call_module"],
                if root { "AdminUtils" } else { "Balances" }
            );
            assert_eq!(
                nested["call_function"],
                if root {
                    "sudo_set_subnet_emission_enabled"
                } else {
                    "transfer_keep_alive"
                }
            );
            if root {
                assert_eq!(arg(nested, "netuid"), &json!(1));
                assert_eq!(arg(nested, "enabled"), &json!(true));
            }
            let submissions = fake.state().lock().unwrap().submissions.clone();
            assert_eq!(submissions.len(), 1);
            assert!(submissions[0].ends_with(&signed[1].1));
            let outer = runtime
                .core()
                .decode_extrinsic(&submissions[0], true)
                .unwrap();
            use rtao_core::value_ext::ValueExt;
            assert_eq!(
                outer.field("address").and_then(ValueExt::as_str),
                Some(signer.account().to_string().as_str())
            );
            let carrier = runtime
                .core()
                .decode_spec(&TypeSpec::Call, &signed[1].1, true)
                .unwrap();
            assert_eq!(outer.field("call"), Some(&carrier));
            let carrier = rtao_core::DynValue::new(carrier).to_json();
            assert_eq!(carrier["call_module"], "MevShield");
            assert_eq!(carrier["call_function"], "submit_encrypted");
            assert!(signed[1].1.starts_with(&[30, 1]));
            assert_eq!(
                fake.requests()
                    .iter()
                    .filter(|request| request.method.starts_with("author_submit"))
                    .count(),
                1
            );
            executor.client().close().await;
        }
    })
    .await
    .unwrap();
}
