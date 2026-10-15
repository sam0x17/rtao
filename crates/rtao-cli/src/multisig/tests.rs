//! Saved-wallet semantic approvals through the production executor and FakeChain.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use rtao::{
    client::{ClientConfig, Network, View},
    core::runtime::TypeSpec,
    primitives::{Balance, IntoCoreValue, Tao},
    signer::{Coldkey, DynKeySource, Hotkey, KeypairSigner, Role, Signer, SignerError, SignerRole},
    tx::{
        ExecuteOptions, FactsTitle, Intent, IntentInterface, OriginMarker, PlanOptions, Policy,
        Root, Signed, SystemClock,
    },
};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use std::{
    collections::BTreeSet,
    marker::PhantomData,
    sync::{
        Arc,
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

trait ProbeKind: Send + Sync + 'static {
    const SPEC: &'static IntentSpec;
    type Role: Role;
    type Origin: OriginMarker;
}
struct Cold;
impl ProbeKind for Cold {
    const SPEC: &'static IntentSpec = <rtao::intents::Transfer as IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
}
struct Hot;
impl ProbeKind for Hot {
    const SPEC: &'static IntentSpec = <rtao::intents::ServeAxon as IntentInterface>::SPEC;
    type Role = Hotkey;
    type Origin = Signed;
}
struct RootCall;
impl ProbeKind for RootCall {
    // The wrapper test below guards this authenticated catalog position;
    // enabling the public M5 intent feature is unnecessary for this fixture.
    const SPEC: &'static IntentSpec = &rtao_catalog::INTENTS[58];
    type Role = Coldkey;
    type Origin = Root;
}

type PreviewInput = (&'static str, AccountId, AccountId, Vec<u8>);

struct Probe<M> {
    builds: AtomicUsize,
    build_origins: Mutex<Vec<AccountId>>,
    preflight_inputs: Mutex<Vec<PreviewInput>>,
    changing_bytes: bool,
    blocked: bool,
    all_subnets: bool,
    marker: PhantomData<M>,
}
impl<M> Default for Probe<M> {
    fn default() -> Self {
        Self {
            builds: AtomicUsize::new(0),
            build_origins: Mutex::new(Vec::new()),
            preflight_inputs: Mutex::new(Vec::new()),
            changing_bytes: false,
            blocked: false,
            all_subnets: false,
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
        })
    }
}

async fn fixture(free: u64) -> (rtao::Executor, FakeChain, Keys) {
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
    (rtao::Executor::new(client), fake, keys)
}

async fn group(executor: &rtao::Executor, keys: &Keys, threshold: u16) -> Multisig {
    executor
        .multisig(&[AccountId([9; 32]), keys.signer.account()], threshold)
        .await
        .unwrap()
}

fn decode(runtime: &RuntimeHandle, call: &CallBytes) -> Value {
    rtao::core::DynValue::new(
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

#[tokio::test]
async fn plan_preserves_the_semantic_contract_and_exact_python_dispatch_shape() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let adapter = SemanticMultisig::new(
        &semantic,
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

#[tokio::test]
async fn preflight_retains_facts_amounts_and_semantic_policy_blocks() {
    let (executor, fake, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<Cold> {
        blocked: true,
        all_subnets: true,
        ..Default::default()
    };
    let adapter = SemanticMultisig::new(
        &semantic,
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

#[tokio::test]
async fn root_and_proxy_layers_remain_inside_the_outer_multisig() {
    assert_eq!(RootCall::SPEC.name, "set_subnet_emission_enabled");
    assert_eq!(RootCall::SPEC.origin, "root");
    assert_eq!(RootCall::SPEC.signer, "coldkey");
    let (executor, _, keys) = fixture(1_000_000_000).await;
    let semantic = Probe::<RootCall>::default();
    let adapter = SemanticMultisig::new(
        &semantic,
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
    let adapter = SemanticMultisig::new(
        &semantic,
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
        .execute_dyn(&adapter, &keys, &ExecuteOptions::default())
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
    let first = SemanticMultisig::new(
        &semantic,
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        None,
    )
    .await
    .unwrap();
    let first_plan = executor
        .plan_dyn(&first, &keys, &PlanOptions::default())
        .await
        .unwrap();
    assert_eq!(first.reviewed_timepoint().unwrap(), None);
    let inner = first.reviewed_inner_call().unwrap().unwrap();
    pending(&fake, &group, &inner, AccountId([9; 32]));
    let later = SemanticMultisig::new(
        &semantic,
        executor.client(),
        group.clone(),
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(inner.clone()),
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
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6])
        .compose(&runtime)
        .unwrap();
    pending(&fake, &group, &inner, keys.signer.account());
    let adapter = SemanticMultisig::new(
        &semantic,
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(inner),
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
    assert_eq!(semantic.builds.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn observed_deposit_plus_fee_shortfall_keeps_the_chain_error_and_never_unlocks() {
    let (executor, fake, keys) = fixture(0).await;
    let semantic = Probe::<Cold>::default();
    let adapter = SemanticMultisig::new(
        &semantic,
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
            rtao::core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42).unwrap();
        state.runtime = Arc::new(upgraded);
        state.advance(2);
    });
    let before = fake.requests().len();
    let result = SemanticMultisig::new(
        &semantic,
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
    let adapter = SemanticMultisig::new(
        &semantic,
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
    let adapter = SemanticMultisig::new(
        &semantic,
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
            rtao::core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42).unwrap();
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
    let adapter = SemanticMultisig::new(
        &semantic,
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
        let (executor, fake, keys) = fixture(0).await;
        let semantic = Probe::<Cold>::default();
        let group = group(&executor, &keys, 2).await;
        let runtime = executor.client().refresh_runtime().await.unwrap();
        let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6])
            .compose(&runtime)
            .unwrap();
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
        let adapter = SemanticMultisig::new(
            &semantic,
            executor.client(),
            group,
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(inner),
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
        assert_eq!(semantic.builds.load(Ordering::SeqCst), 0);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        executor.client().close().await;
    }
}

#[tokio::test]
async fn guarded_existing_approval_preserves_the_duplicate_member_refusal() {
    let (executor, fake, keys) = fixture(0).await;
    let semantic = Probe::<Cold>::default();
    let group = group(&executor, &keys, 2).await;
    let runtime = executor.client().refresh_runtime().await.unwrap();
    let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6])
        .compose(&runtime)
        .unwrap();
    pending(&fake, &group, &inner, keys.signer.account());
    let storage_key = format!("0x{}", hex::encode(pending_key(&runtime, &group, &inner)));
    let adapter = SemanticMultisig::new(
        &semantic,
        executor.client(),
        group,
        keys.signer.account(),
        CryptoType::Ed25519,
        Some(inner),
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

struct ChangingPendingKeys<'a> {
    keys: &'a Keys,
    fake: FakeChain,
    group: Multisig,
    inner: CallBytes,
    storage_key: Vec<u8>,
    replacement: Option<MultisigTimepoint>,
}
impl KeyView for ChangingPendingKeys<'_> {
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
impl DynKeySource for ChangingPendingKeys<'_> {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            if let Some(replacement) = &self.replacement {
                pending_at(
                    &self.fake,
                    &self.group,
                    &self.inner,
                    AccountId([9; 32]),
                    replacement.clone(),
                );
            } else {
                self.fake.mutate_state(|state| {
                    state.seed_encoded(self.storage_key.clone(), None);
                });
            }
            self.keys.signer_for(role).await
        })
    }
}

fn submitted_multisig_call(runtime: &RuntimeHandle, fake: &FakeChain) -> Value {
    let extrinsic = fake.state().lock().unwrap().submissions[0].clone();
    for offset in 0..extrinsic.len() {
        if let Ok(value) = runtime
            .core()
            .decode_spec(&TypeSpec::Call, &extrinsic[offset..], true)
        {
            let call = rtao::core::DynValue::new(value).to_json();
            if call["call_module"] == "Multisig" {
                return call;
            }
        }
    }
    panic!("signed continuation contains no complete multisig call")
}

#[tokio::test]
async fn operation_changed_after_composition_keeps_stale_some_in_the_actual_signed_call() {
    for replaced in [false, true] {
        let (executor, fake, keys) = fixture(1_000_000_000).await;
        let semantic = Probe::<Cold>::default();
        let group = group(&executor, &keys, 2).await;
        let runtime = executor.client().refresh_runtime().await.unwrap();
        let inner = rtao_runtime::calls::system::remark(vec![4, 5, 6])
            .compose(&runtime)
            .unwrap();
        pending(&fake, &group, &inner, AccountId([9; 32]));
        let expected = MultisigTimepoint {
            height: 12,
            index: 3,
        };
        let adapter = SemanticMultisig::new(
            &semantic,
            executor.client(),
            group.clone(),
            keys.signer.account(),
            CryptoType::Ed25519,
            Some(inner.clone()),
        )
        .await
        .unwrap()
        .with_opening_timepoint(expected.clone());
        let changing = ChangingPendingKeys {
            keys: &keys,
            fake: fake.clone(),
            group: group.clone(),
            inner: inner.clone(),
            storage_key: pending_key(&runtime, &group, &inner),
            replacement: replaced.then_some(MultisigTimepoint {
                height: 13,
                index: 4,
            }),
        };
        executor
            .execute_dyn(&adapter, &changing, &ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        let actual = submitted_multisig_call(&runtime, &fake);
        assert_eq!(actual["call_function"], "as_multi");
        assert_eq!(arg(&actual, "maybe_timepoint"), &json!(expected));
        assert_eq!(arg(&actual, "call"), &decode(&runtime, &inner));
        assert_eq!(adapter.reviewed_timepoint().unwrap(), Some(expected));
        let now = executor
            .client()
            .storage(
                &rtao_runtime::storage::multisig::MULTISIGS,
                (group.address, inner.hash()),
            )
            .await
            .unwrap();
        assert_eq!(
            now.map(|operation| operation.when),
            changing.replacement,
            "the pending state must have changed after review"
        );
        assert_eq!(semantic.builds.load(Ordering::SeqCst), 0);
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        executor.client().close().await;
    }
}
