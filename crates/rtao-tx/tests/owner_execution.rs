//! Real owner and registration intents through signed executor submissions.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{
    Client, ClientConfig, Inclusion, Mortality, Network, NoncePolicy, Period, SubmitOptions, View,
};
use rtao_intents::hyperparameters::owner_rate_key;
use rtao_intents::{BurnedRegister, RegisterSubnet, SetHyperparameter};
use rtao_primitives::{
    AccountId, Balance, CryptoType, DefaultAccount, IntoCoreValue, NetUid, TaoBalance,
};
use rtao_runtime::{CallBytes, calls::admin_utils, storage::subtensor_module as st};
use rtao_signer::{
    BoxFuture, Coldkey, KeySource, KeyView, KeypairSigner, MultiSignature, SignPurpose, Signer,
    SignerError, SigningRequest,
};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use rtao_tx::{
    BuildCx, Built, DynIntent, ExecuteOptions, Executor, Intent, IntentError, IntentInterface,
    IntentPreflight, Multisig, OriginView, Policy, PreflightCx, ShieldOptions, Signed, TxError,
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

const NET: NetUid = NetUid(3);
const HOTKEY: AccountId = AccountId([8; 32]);

type AfterSign = Box<dyn FnOnce() + Send>;
type SignedCalls = Vec<(SignPurpose, Vec<u8>)>;

#[derive(Clone)]
struct InnerCapture {
    client: Client,
    calls: Vec<CallBytes>,
    bytes: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[derive(Clone)]
struct RecordingSigner {
    key: KeypairSigner,
    requests: Arc<Mutex<SignedCalls>>,
    after_first_sign: Arc<Mutex<Option<AfterSign>>>,
    delay: Duration,
    inner: Option<InnerCapture>,
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
            self.requests
                .lock()
                .unwrap()
                .push((request.purpose, request.call.to_vec()));
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let signature = self.key.sign(request).await?;
            if request.purpose == SignPurpose::ShieldInner
                && let Some(capture) = &self.inner
            {
                let index = capture.bytes.lock().unwrap().len();
                let nonce = u64::from_str_radix(
                    request.payload_json["nonce"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("0x"),
                    16,
                )
                .unwrap();
                let prepared = capture
                    .client
                    .prepare_call(
                        capture.calls[index].clone(),
                        &self.key,
                        &SubmitOptions {
                            nonce: NoncePolicy::Explicit(nonce),
                            mortality: Mortality::Mortal(Period::new(8)),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(prepared.call().as_bytes(), request.call);
                assert_eq!(prepared.payload(), request.payload);
                let (bytes, _) = capture
                    .client
                    .attach_signature(&prepared, signature)
                    .await
                    .unwrap();
                capture.bytes.lock().unwrap().push(bytes.to_vec());
            }
            let after = self.after_first_sign.lock().unwrap().take();
            if let Some(after) = after {
                after();
            }
            Ok(signature)
        })
    }
}

struct Keys {
    signer: RecordingSigner,
    unlocks: AtomicUsize,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(HOTKEY)
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(HOTKEY.0)
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
        Ok(Arc::new(self.signer.clone()))
    }
}

async fn fixture() -> (Executor, FakeChain, Keys) {
    let key = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap();
    let mut state = FakeState::default();
    state
        .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
        .unwrap();
    state
        .seed_constant("Aura", "SlotDuration", json!(1))
        .unwrap();
    state.seed(&st::BURN, (NET,), TaoBalance(65_535)).unwrap();
    state
        .seed(&st::COLLATERAL_LOCK_SHARE, (NET,), 1u16)
        .unwrap();
    state
        .seed(
            &st::SUBNET_OWNER,
            (NET,),
            DefaultAccount::from(key.account()),
        )
        .unwrap();
    state.seed(&st::TEMPO, (NET,), 360u16).unwrap();
    state.seed(&st::LAST_EPOCH_BLOCK, (NET,), 100u64).unwrap();
    state.seed(&st::PENDING_EPOCH_AT, (NET,), 0u64).unwrap();
    state.seed(&st::ADMIN_FREEZE_WINDOW, (), 10u16).unwrap();
    state
        .seed(&st::OWNER_HYPERPARAM_RATE_LIMIT, (), 2u16)
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(NET.0)],
            json!([1000, 60000]),
        )
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
                key,
                requests: Arc::new(Mutex::new(Vec::new())),
                after_first_sign: Arc::new(Mutex::new(None)),
                delay: Duration::ZERO,
                inner: None,
            },
            unlocks: AtomicUsize::new(0),
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
fn included() -> ExecuteOptions {
    ExecuteOptions {
        wait_for_finalization: false,
        ..Default::default()
    }
}
fn burned() -> BurnedRegister {
    BurnedRegister {
        netuid: NET,
        hotkey_ss58: None,
    }
}
fn alpha() -> SetHyperparameter {
    SetHyperparameter {
        netuid: NET,
        name: "alpha_low".into(),
        value: json!(2000),
    }
}
fn success(index: u32) -> Value {
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}
fn rate_failure(index: u32) -> Value {
    let error = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|error| error.pallet == "SubtensorModule" && error.name == "TxRateLimitExceeded")
        .unwrap();
    json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"Module":{"index":error.pallet_index,"error":[error.error_index,0,0,0]}},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
}
fn changed_alpha(fake: FakeChain) -> AfterSign {
    Box::new(move || {
        fake.mutate_state(|state| {
            state
                .seed_python(
                    "SubtensorModule",
                    "AlphaValues",
                    &[json!(NET.0)],
                    json!([1000, 61000]),
                )
                .unwrap();
        })
    })
}

// Model the public execution-adapter boundary used by saved multisig intents.
// The approval has its own signed-operation spec while policy and owner
// preflight continue to use the inner semantic intent.
struct OwnerApproval {
    semantic: SetHyperparameter,
    client: Client,
    group: Multisig,
    member: AccountId,
}
impl IntentInterface for OwnerApproval {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[31];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::multisig::AS_MULTI];
    fn to_json(&self) -> Value {
        json!({"op": Self::SPEC.name, "call": self.semantic.to_json()})
    }
}
impl Intent for OwnerApproval {
    fn summary(&self) -> String {
        self.semantic.summary()
    }
    fn semantic_intent(&self) -> &dyn DynIntent {
        &self.semantic
    }
    fn origin_view<'a>(&'a self, _keys: &'a dyn KeyView) -> OriginView<'a> {
        OriginView::Account(self.group.address)
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let built = self.semantic.build(cx).await?;
        self.wrap_call(cx, built.call).await
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        self.semantic.preflight(cx).await
    }
    async fn wrap_call<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
        call: CallBytes,
    ) -> Result<Built, IntentError> {
        let estimate = self
            .client
            .estimate_weight(call.clone(), self.member, CryptoType::Ed25519)
            .await?;
        Ok(Built::new(
            rtao_client::compose::as_multi(
                cx.view,
                self.member,
                &self.group.signatories,
                self.group.threshold,
                call,
                &estimate,
            )
            .await?,
        ))
    }
}

#[tokio::test]
async fn burned_registration_requires_the_shield_even_when_disabled_in_options() {
    let (ex, fake, keys) = fixture().await;
    let result = ex
        .execute(
            &burned(),
            &keys,
            &ExecuteOptions {
                shielded: false,
                ..pool()
            },
        )
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.inclusion, Inclusion::PoolAccepted);
    assert_eq!(result.data["shielded"], true);
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let expected = rtao_runtime::calls::subtensor_module::burned_register(NET, HOTKEY)
        .compose(&runtime)
        .unwrap();
    let requests = keys.signer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0],
        (SignPurpose::ShieldInner, expected.as_bytes().to_vec())
    );
    assert_eq!(requests[1].0, SignPurpose::ShieldCarrier);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn registration_policy_and_owner_preflight_refuse_before_unlock() {
    for policy in [
        Policy {
            max_spend: Some(Balance::from_rao(u64::MAX)),
            ..Default::default()
        },
        Policy {
            max_fee: Some(Balance::ZERO),
            ..Default::default()
        },
        Policy {
            allowed_netuids: Some(BTreeSet::from([NetUid(4)])),
            ..Default::default()
        },
    ] {
        let (ex, fake, keys) = fixture().await;
        assert!(matches!(
            ex.execute(
                &burned(),
                &keys,
                &ExecuteOptions {
                    policy: Some(policy),
                    ..pool()
                }
            )
            .await,
            Err(TxError::Policy(_))
        ));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(keys.signer.requests.lock().unwrap().is_empty());
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        ex.client().close().await;
    }
    for freeze in [false, true] {
        let (ex, fake, keys) = fixture().await;
        fake.mutate_state(|state| {
            if freeze {
                state.seed(&st::PENDING_EPOCH_AT, (NET,), 110u64).unwrap();
            } else {
                state
                    .seed(
                        &st::LAST_RATE_LIMITED_BLOCK,
                        (owner_rate_key(NET, "AlphaValues"),),
                        99u64,
                    )
                    .unwrap();
            }
        });
        let error = ex.execute(&alpha(), &keys, &pool()).await.unwrap_err();
        assert!(matches!(error, TxError::Policy(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains(if freeze { "freeze" } else { "cooldown" })
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(fake.state().lock().unwrap().submissions.is_empty());
        ex.client().close().await;
    }
}

#[tokio::test]
async fn burned_registration_unknown_fate_preserves_the_only_carrier_hash() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|state| state.queue_submit(SubmitScript::Statuses(vec![json!("ready")])));
    let result = ex
        .execute(
            &burned(),
            &keys,
            &ExecuteOptions {
                deadline: Some(Duration::from_millis(100)),
                retries: 4,
                ..included()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    let hash = result.fate_unknown.unwrap();
    let submissions = fake.state().lock().unwrap().submissions.clone();
    assert_eq!(submissions.len(), 1);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 2);
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
    ex.client().close().await;
}

#[tokio::test]
async fn owner_rate_failure_rebuilds_the_real_alpha_call_once_from_fresh_state() {
    let (ex, fake, keys) = fixture().await;
    *keys.signer.after_first_sign.lock().unwrap() = Some(changed_alpha(fake.clone()));
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([rate_failure(0)]),
        })
    });
    let result = ex
        .execute(
            &alpha(),
            &keys,
            &ExecuteOptions {
                retries: 4,
                ..included()
            },
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.message);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 2);
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let requests = keys.signer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].1,
        admin_utils::sudo_set_alpha_values(NET, 2000, 60000)
            .compose(&runtime)
            .unwrap()
            .as_bytes()
    );
    assert_eq!(
        requests[1].1,
        admin_utils::sudo_set_alpha_values(NET, 2000, 61000)
            .compose(&runtime)
            .unwrap()
            .as_bytes()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn completed_multisig_owner_rate_failure_preserves_receipt_without_reapproval() {
    let (ex, fake, keys) = fixture().await;
    assert_eq!(OwnerApproval::SPEC.name, "multisig_execute");
    let member = keys.signer.account();
    let approved = AccountId([9; 32]);
    let group = ex.multisig(&[member, approved], 2).await.unwrap();
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let inner = admin_utils::sudo_set_alpha_values(NET, 2000, 60000)
        .compose(&runtime)
        .unwrap();
    let timepoint = rtao_runtime::models::MultisigTimepoint {
        height: 90,
        index: 1,
    };
    let pending_key = runtime
        .core()
        .storage_key(
            runtime
                .core()
                .storage_entry("Multisig", "Multisigs")
                .unwrap(),
            &[
                group.address.into_core_value(),
                inner.hash().into_core_value(),
            ],
        )
        .unwrap();
    fake.mutate_state(|state| {
        state
            .seed(&st::SUBNET_OWNER, (NET,), DefaultAccount::from(group.address))
            .unwrap();
        state
            .seed_python(
                "Multisig",
                "Multisigs",
                &[
                    json!(group.address),
                    json!(format!("0x{}", hex::encode(inner.hash()))),
                ],
                json!({"when":timepoint,"deposit":280_800_000,"depositor":approved,"approvals":[approved]}),
            )
            .unwrap();
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([
                {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{
                    "approving":member,"timepoint":timepoint,"multisig":group.address,
                    "call_hash":format!("0x{}",hex::encode(inner.hash())),
                    "result":{"Err":rate_failure(0)["attributes"]["dispatch_error"].clone()}
                },"topics":[]},
                success(0)
            ]),
        });
    });
    assert_eq!(
        ex.client()
            .storage(
                &rtao_runtime::storage::multisig::MULTISIGS,
                (group.address, inner.hash()),
            )
            .await
            .unwrap()
            .unwrap()
            .when,
        timepoint
    );
    let estimate = ex
        .client()
        .estimate_weight(inner.clone(), member, CryptoType::Ed25519)
        .await
        .unwrap();
    let expected = rtao_client::compose::as_multi(
        ex.client(),
        member,
        &group.signatories,
        group.threshold,
        inner.clone(),
        &estimate,
    )
    .await
    .unwrap();
    let after = fake.clone();
    *keys.signer.after_first_sign.lock().unwrap() = Some(Box::new(move || {
        // Final approval consumes the pending operation even when its inner
        // dispatch fails. Rebuilding now would encode a new opening approval.
        after.mutate_state(|state| state.seed_encoded(pending_key, None));
    }));
    let result = ex
        .execute(
            &OwnerApproval {
                semantic: alpha(),
                client: ex.client().clone(),
                group: group.clone(),
                member,
            },
            &keys,
            &ExecuteOptions {
                retries: 4,
                ..included()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(
        result.error.as_ref().unwrap().name.as_deref(),
        Some("TxRateLimitExceeded")
    );
    assert_eq!(result.inclusion, Inclusion::InBlock);
    assert_eq!(result.extrinsic_id.as_deref(), Some("101-0000"));
    assert!(result.block_hash.is_some());
    assert!(result.fate_unknown.is_none());
    assert!(
        result
            .events
            .iter()
            .any(|event| event.matches("Multisig", "MultisigExecuted"))
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(
        keys.signer.requests.lock().unwrap().as_slice(),
        &[(SignPurpose::Extrinsic, expected.as_bytes().to_vec())]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert!(
        ex.client()
            .storage(
                &rtao_runtime::storage::multisig::MULTISIGS,
                (group.address, inner.hash()),
            )
            .await
            .unwrap()
            .is_none()
    );
    ex.client().close().await;
}

#[tokio::test]
async fn owner_rate_failure_gets_only_one_retry_despite_the_pool_retry_setting() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|state| {
        for _ in 0..3 {
            state.queue_submit(SubmitScript::InBlockThenFinalized {
                events: json!([rate_failure(0)]),
            });
        }
    });
    let result = ex
        .execute(
            &alpha(),
            &keys,
            &ExecuteOptions {
                retries: 4,
                ..included()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(
        result.error.unwrap().name.as_deref(),
        Some("TxRateLimitExceeded")
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 2);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 2);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn owner_replan_rechecks_the_window_before_a_second_submission() {
    let (ex, fake, keys) = fixture().await;
    let after = fake.clone();
    *keys.signer.after_first_sign.lock().unwrap() = Some(Box::new(move || {
        after.mutate_state(|state| {
            state
                .seed(
                    &st::LAST_RATE_LIMITED_BLOCK,
                    (owner_rate_key(NET, "AlphaValues"),),
                    100u64,
                )
                .unwrap();
        })
    }));
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([rate_failure(0)]),
        })
    });
    let error = ex.execute(&alpha(), &keys, &included()).await.unwrap_err();
    assert!(matches!(error, TxError::Policy(_)), "{error}");
    assert!(error.to_string().contains("cooldown"));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn owner_retry_keeps_the_original_total_deadline() {
    let (ex, fake, mut keys) = fixture().await;
    keys.signer.delay = Duration::from_millis(130);
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([rate_failure(0)]),
        })
    });
    let error = ex
        .execute(
            &alpha(),
            &keys,
            &ExecuteOptions {
                deadline: Some(Duration::from_millis(220)),
                ..included()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, TxError::Rpc(_)), "{error}");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 2);
    ex.client().close().await;
}

#[tokio::test]
async fn shield_owner_rate_failure_also_replans_once_with_a_fresh_alpha_pair() {
    let (ex, fake, mut keys) = fixture().await;
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let bytes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    keys.signer.inner = Some(InnerCapture {
        client: ex.client().clone(),
        bytes: Arc::clone(&bytes),
        calls: vec![
            admin_utils::sudo_set_alpha_values(NET, 2000, 60000)
                .compose(&runtime)
                .unwrap(),
            admin_utils::sudo_set_alpha_values(NET, 2000, 61000)
                .compose(&runtime)
                .unwrap(),
        ],
    });
    *keys.signer.after_first_sign.lock().unwrap() = Some(changed_alpha(fake.clone()));
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome {
                let height = raw["block"]["header"]["number"]
                    .as_str()
                    .and_then(|number| {
                        u64::from_str_radix(number.trim_start_matches("0x"), 16).ok()
                    });
                if let Some(index) = height
                    .and_then(|height| height.checked_sub(101))
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < 2)
                    && let Some(inner) = bytes.lock().unwrap().get(index)
                {
                    raw["block"]["extrinsics"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(format!("0x{}", hex::encode(inner))));
                    state
                        .seed_events(
                            request.params[0].as_str().unwrap(),
                            json!([
                                success(0),
                                if index == 0 {
                                    rate_failure(1)
                                } else {
                                    success(1)
                                }
                            ]),
                        )
                        .unwrap();
                }
            }
            reply
        })
    });
    let result = ex
        .submit_shielded(
            &alpha(),
            &keys,
            &ShieldOptions {
                execute: included(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.message);
    assert_eq!(result.data["shielded"], true);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 2);
    let requests = keys.signer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].0, SignPurpose::ShieldInner);
    assert_eq!(requests[1].0, SignPurpose::ShieldCarrier);
    assert_eq!(requests[2].0, SignPurpose::ShieldInner);
    assert_ne!(requests[0].1, requests[2].1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn subnet_registration_executor_populates_the_immediate_typed_result() {
    let (ex, fake, keys) = fixture().await;
    let registered = NetUid(9);
    fake.mutate_state(|state| {
        state.seed(&st::SUBNET_OWNER, (registered,), DefaultAccount::from(keys.signer.account())).unwrap();
        state.seed(&st::SUBNET_OWNER_HOTKEY, (registered,), DefaultAccount::from(HOTKEY)).unwrap();
        state.seed(&st::SUBNET_LOCKED, (registered,), TaoBalance(777)).unwrap();
        state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([
            {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NetworkAdded","attributes":[registered.0,0],"topics":[]}, success(0)
        ]) });
    });
    let result = ex
        .execute(&RegisterSubnet { hotkey_ss58: None }, &keys, &included())
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(result.message, "Subnet 9 registered.");
    assert_eq!(result.data["netuid"], 9);
    assert_eq!(result.data["registration_mode"], "immediate");
    assert_eq!(result.data["registered_at_block"], 101);
    assert_eq!(result.data["registration_price_rao"], 777);
    assert_eq!(result.output.netuid, Some(registered));
    assert_eq!(result.output.mode.as_deref(), Some("immediate"));
    assert_eq!(result.output.registered_at_block, Some(101));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn clear_owner_rate_replan_opt_out_returns_first_included_failure() {
    let (ex, fake, keys) = fixture().await;
    fake.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([rate_failure(0)]),
        })
    });
    let result = ex
        .execute(
            &alpha(),
            &keys,
            &ExecuteOptions {
                retries: 4,
                replan_owner_rate_limit: false,
                ..included()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(
        result.error.unwrap().name.as_deref(),
        Some("TxRateLimitExceeded")
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    assert_eq!(keys.signer.requests.lock().unwrap().len(), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}

#[tokio::test]
async fn shield_owner_rate_replan_opt_out_signs_and_submits_only_one_pair() {
    let (ex, fake, mut keys) = fixture().await;
    let runtime = ex.client().refresh_runtime().await.unwrap();
    let bytes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    keys.signer.inner = Some(InnerCapture {
        client: ex.client().clone(),
        bytes: Arc::clone(&bytes),
        calls: vec![
            admin_utils::sudo_set_alpha_values(NET, 2000, 60000)
                .compose(&runtime)
                .unwrap(),
        ],
    });
    fake.mutate_state(|state| {
        state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome {
                let height = raw["block"]["header"]["number"]
                    .as_str()
                    .and_then(|number| {
                        u64::from_str_radix(number.trim_start_matches("0x"), 16).ok()
                    });
                if height == Some(101)
                    && let Some(inner) = bytes.lock().unwrap().first()
                {
                    raw["block"]["extrinsics"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(format!("0x{}", hex::encode(inner))));
                    state
                        .seed_events(
                            request.params[0].as_str().unwrap(),
                            json!([success(0), rate_failure(1)]),
                        )
                        .unwrap();
                }
            }
            reply
        })
    });
    let result = ex
        .submit_shielded(
            &alpha(),
            &keys,
            &ShieldOptions {
                execute: ExecuteOptions {
                    replan_owner_rate_limit: false,
                    ..included()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(
        result.error.unwrap().name.as_deref(),
        Some("TxRateLimitExceeded")
    );
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
    let requests = keys.signer.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, SignPurpose::ShieldInner);
    assert_eq!(requests[1].0, SignPurpose::ShieldCarrier);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    ex.client().close().await;
}
