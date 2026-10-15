//! Multisig approvals through the production executor and metadata-backed chain.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{Client, ClientConfig, Network, View, ViewExt};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{
    BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole,
};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, Reply};
use rtao_tx::{ApproveOptions, Executor, Multisig, Policy, TxError};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone)]
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
}
impl KeyView for Keys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.key.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        _role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
async fn fixture(free: u64) -> (Executor, FakeChain, Keys, rtao_client::CallBytes) {
    let keys = Keys {
        key: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        unlocks: Arc::new(AtomicUsize::new(0)),
    };
    let mut state = FakeState::default();
    state
        .seed_python(
            "System",
            "Account",
            &[json!(keys.key.account().to_string())],
            json!({"data":{"free":free}}),
        )
        .unwrap();
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
    let call = rtao_runtime::calls::system::remark(vec![4, 5, 6])
        .compose(&client.refresh_runtime().await.unwrap())
        .unwrap();
    (Executor::new(client), fake, keys, call)
}
async fn group(executor: &Executor, keys: &Keys, threshold: u16) -> Multisig {
    executor
        .multisig(&[AccountId([9; 32]), keys.key.account()], threshold)
        .await
        .unwrap()
}
async fn submitted_call(executor: &Executor, fake: &FakeChain) -> Value {
    let bytes = fake.state().lock().unwrap().submissions[0].clone();
    // The signer-transmitted call remains the exact terminal bytes of v4.
    let runtime = View::runtime(executor.client()).await.unwrap();
    for offset in 0..bytes.len() {
        if let Ok(value) =
            runtime
                .core()
                .decode_spec(&rtao_core::runtime::TypeSpec::Call, &bytes[offset..], true)
            && value.field("call_module").and_then(ValueExt::as_str) == Some("Multisig")
        {
            return rtao_core::DynValue::new(value).to_json();
        }
    }
    panic!("signed approval contains no complete Multisig call")
}
fn arg<'a>(call: &'a Value, name: &str) -> &'a Value {
    &call["call_args"]
        .as_array()
        .unwrap()
        .iter()
        .find(|arg| arg["name"] == name)
        .unwrap()["value"]
}

#[test]
fn account_derivation_uses_core_sorted_set_and_refuses_invalid_members() {
    let runtime = rtao_client::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap();
    let signer_a = AccountId([1; 32]);
    let signer_b = AccountId([2; 32]);
    let account = Multisig::new(&runtime, &[signer_b, signer_a], 2).unwrap();
    let (expected, sorted) =
        rtao_core::extrinsic::multisig_account_id(&[signer_b.0, signer_a.0], 2).unwrap();
    assert_eq!(account.address, AccountId(expected));
    assert_eq!(
        account.signatories,
        sorted.into_iter().map(AccountId).collect::<Vec<_>>()
    );
    assert_eq!(
        Multisig::new(&runtime, &[signer_a, signer_b], 2).unwrap(),
        account
    );
    assert!(Multisig::new(&runtime, &[signer_a, signer_a], 2).is_err());
    assert!(Multisig::new(&runtime, &[signer_a], 0).is_err());
    assert!(Multisig::new(&runtime, &[signer_a], 2).is_err());
    assert!(Multisig::new(&runtime, &[signer_a], 1).is_err());
}

fn signatories(count: u8) -> Vec<AccountId> {
    (0..count).map(|value| AccountId([value; 32])).collect()
}

#[tokio::test]
async fn construction_and_origin_composition_enforce_the_live_member_boundary() {
    let (executor, fake, keys, call) = fixture(1_000_000_000).await;
    let runtime = View::pin(executor.client())
        .await
        .unwrap()
        .block()
        .runtime
        .clone();
    let at_limit = signatories(100);
    let over_limit = signatories(101);
    assert!(Multisig::new(&runtime, &at_limit, 2).is_ok());
    assert!(Multisig::new(&runtime, &over_limit, 2).is_err());
    assert!(executor.multisig(&at_limit, 2).await.is_ok());
    assert!(executor.multisig(&over_limit, 2).await.is_err());

    // The pallet counts the submitting signer in addition to these members.
    assert!(
        rtao_client::compose::as_multi_threshold_1(&runtime, &signatories(99), call.clone(),)
            .is_ok()
    );
    assert!(
        rtao_client::compose::as_multi_threshold_1(&runtime, &signatories(100), call.clone(),)
            .is_err()
    );
    let estimate = executor
        .client()
        .estimate_weight(call.clone(), keys.key.account(), CryptoType::Ed25519)
        .await
        .unwrap();
    for members in [&at_limit, &over_limit] {
        let expected = members.len() == 100;
        let result = rtao_client::compose::as_multi(
            executor.client(),
            members[0],
            members,
            2,
            call.clone(),
            &estimate,
        )
        .await;
        assert_eq!(result.is_ok(), expected);
        let result = rtao_client::compose::approve_as_multi(
            executor.client(),
            members[0],
            members,
            2,
            rtao_primitives::H256([7; 32]),
            rtao_runtime::models::MultisigTimepoint {
                height: 12,
                index: 3,
            },
            &estimate,
        )
        .await;
        assert_eq!(result.is_ok(), expected);
    }
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[test]
fn account_construction_fails_closed_for_missing_malformed_and_changed_type_caps() {
    for mutation in ["missing", "truncated", "trailing", "type", "zero", "one"] {
        let mut runtime = rtao_core::runtime::Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap();
        let other_type = runtime
            .constant("Balances", "ExistentialDeposit")
            .unwrap()
            .ty;
        let constants = &mut runtime
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == "Multisig")
            .unwrap()
            .constants;
        if mutation == "missing" {
            constants.retain(|constant| constant.name != "MaxSignatories");
        } else {
            let cap = constants
                .iter_mut()
                .find(|constant| constant.name == "MaxSignatories")
                .unwrap();
            match mutation {
                "truncated" => {
                    cap.value.pop();
                }
                "trailing" => cap.value.push(0),
                "type" => cap.ty = other_type,
                "zero" => cap.value = 0u32.to_le_bytes().to_vec(),
                "one" => cap.value = 1u32.to_le_bytes().to_vec(),
                _ => unreachable!(),
            }
        }
        let runtime = rtao_client::RuntimeHandle::new(runtime).unwrap();
        assert!(
            Multisig::new(&runtime, &signatories(2), 1).is_err(),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn approval_rechecks_a_reduced_runtime_cap_before_funding_help_or_unlock() {
    let (executor, fake, keys, call) = fixture(0).await;
    let members = [keys.key.account(), AccountId([8; 32]), AccountId([9; 32])];
    for threshold in [1, 2] {
        // Each case starts with a valid group under an authenticated larger cap.
        let old_runtime = rtao_client::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap();
        let multisig = Multisig::new(&old_runtime, &members, threshold).unwrap();
        if threshold == 1 {
            fake.mutate_state(|state| {
                state
                    .seed_constant("Multisig", "MaxSignatories", json!(2))
                    .unwrap();
                let upgraded =
                    rtao_core::runtime::Runtime::parse(&state.runtime.metadata_bytes, 479, 1, 42)
                        .unwrap();
                state.runtime = Arc::new(upgraded);
                state.advance(2);
            });
        }
        let error = multisig
            .approve(&executor, call.clone(), &keys, &ApproveOptions::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("MaxSignatories (2)"), "{error}");
        assert!(!matches!(error, TxError::Chain(_)));
    }
    assert!(executor.multisig(&members, 2).await.is_err());
    assert!(executor.multisig(&members[..2], 2).await.is_ok());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    let state = fake.state();
    let state = state.lock().unwrap();
    assert!(state.submissions.is_empty());
    assert!(!state.requests.iter().any(|request| {
        request.method == "state_call"
            && request.params[0]
                .as_str()
                .is_some_and(|method| method.contains("query_info"))
    }));
}

#[tokio::test]
async fn opening_shortfall_refuses_before_unlock_with_exact_funding_help() {
    let (executor, fake, keys, call) = fixture(0).await;
    let multisig = group(&executor, &keys, 2).await;
    let error = multisig
        .approve(
            &executor,
            call,
            &keys,
            &ApproveOptions {
                signer_label: Some("treasury".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let TxError::Chain(error) = error else {
        panic!("expected observed funds error")
    };
    assert_eq!(error.code, rtao_catalog::ErrorCode::InsufficientBalance);
    let deposit = rtao_tx::multisig::multisig_deposit(executor.client(), 2)
        .await
        .unwrap()
        .rao();
    assert_eq!(deposit, 196_000_000);
    assert!(error.message.contains("τ0.196 deposit"));
    assert_eq!(
        error.remediation(),
        format!(
            "fund {} (treasury) with ≥ τ0.197 — the deposit is returned when the op executes",
            keys.key.account()
        )
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    let warning =
        rtao_tx::multisig::multisig_opening_shortfall(executor.client(), keys.key.account(), 2)
            .await
            .unwrap();
    assert!(warning.contains("≥ τ0.201"));
}

#[tokio::test]
async fn funded_opening_preserves_full_call_sorted_members_and_probe_weight() {
    let (executor, fake, keys, call) = fixture(1_000_000_000).await;
    let inner_hash = call.hash();
    let multisig = group(&executor, &keys, 2).await;
    let result = multisig
        .approve(&executor, call.clone(), &keys, &ApproveOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    let outer = submitted_call(&executor, &fake).await;
    assert_eq!(outer["call_function"], "as_multi");
    assert_eq!(arg(&outer, "threshold"), &json!(2));
    assert_eq!(
        arg(&outer, "other_signatories"),
        &json!([AccountId([9; 32]).to_string()])
    );
    assert!(arg(&outer, "maybe_timepoint").is_null());
    assert_eq!(arg(&outer, "call")["call_function"], "remark");
    assert_eq!(
        arg(&outer, "max_weight"),
        &fake.state().lock().unwrap().weight
    );
    assert_eq!(call.hash(), inner_hash);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
}

#[tokio::test]
async fn subsequent_approval_uses_pending_timepoint_and_charges_fee_without_deposit() {
    let (executor, fake, keys, call) = fixture(200_000).await;
    let multisig = group(&executor, &keys, 2).await;
    fake.mutate_state(|state|state.seed_python("Multisig","Multisigs",&[json!(multisig.address.to_string()),json!(format!("0x{}",hex::encode(call.hash())))],json!({"when":{"height":12,"index":3},"deposit":280_800_000,"depositor":AccountId([9;32]).to_string(),"approvals":[AccountId([9;32]).to_string()]})).unwrap());
    let result = multisig
        .approve(&executor, call, &keys, &ApproveOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    let outer = submitted_call(&executor, &fake).await;
    assert_eq!(
        arg(&outer, "maybe_timepoint"),
        &json!({"height":12,"index":3})
    );
    assert_eq!(outer["call_function"], "as_multi");
}

#[tokio::test]
async fn threshold_one_has_no_deposit_pending_state_or_weight_probe() {
    let (executor, fake, keys, call) = fixture(0).await;
    fake.mutate_state(|state| {
        state.set_handler("state_call", |_, _| {
            Reply::error(-32000, "no fee probe", None)
        })
    });
    let result = group(&executor, &keys, 1)
        .await
        .approve(&executor, call, &keys, &ApproveOptions::default())
        .await
        .unwrap();
    assert!(result.success);
    assert_eq!(
        submitted_call(&executor, &fake).await["call_function"],
        "as_multi_threshold_1"
    );
    assert_eq!(
        rtao_tx::multisig::multisig_opening_shortfall(executor.client(), keys.key.account(), 1)
            .await,
        None
    );
}

#[tokio::test]
async fn failed_funds_read_is_best_effort_but_raw_policy_still_refuses() {
    let (executor, fake, keys, call) = fixture(0).await;
    fake.mutate_state(|state| {
        state.queue_reply(
            "state_queryStorageAt",
            Reply::error(-32000, "temporary read failure", None),
        )
    });
    assert!(
        group(&executor, &keys, 2)
            .await
            .approve(&executor, call.clone(), &keys, &ApproveOptions::default())
            .await
            .unwrap()
            .success
    );
    let restricted = executor.with_policy(Policy::default());
    fake.mutate_state(|state| {
        state
            .seed_python(
                "System",
                "Account",
                &[json!(keys.key.account().to_string())],
                json!({"data":{"free":1_000_000_000}}),
            )
            .unwrap()
    });
    let before = fake.state().lock().unwrap().submissions.len();
    assert!(matches!(
        group(&restricted, &keys, 2)
            .await
            .approve(&restricted, call, &keys, &ApproveOptions::default())
            .await,
        Err(TxError::Policy(_))
    ));
    assert_eq!(fake.state().lock().unwrap().submissions.len(), before);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_signer_and_tampered_account_refuse_before_unlock() {
    let (executor, fake, keys, call) = fixture(1_000_000_000).await;
    let foreign = executor
        .multisig(&[AccountId([8; 32]), AccountId([9; 32])], 2)
        .await
        .unwrap();
    assert!(
        foreign
            .approve(&executor, call.clone(), &keys, &ApproveOptions::default())
            .await
            .is_err()
    );
    let mut tampered = group(&executor, &keys, 2).await;
    tampered.address = AccountId([0; 32]);
    assert!(
        tampered
            .approve(&executor, call, &keys, &ApproveOptions::default())
            .await
            .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn client_decode_scale_uses_live_call_and_strict_complete_input() {
    let (executor, _, _, call) = fixture(1_000_000_000).await;
    let decoded = executor
        .client()
        .decode_scale("Call", call.as_bytes())
        .await
        .unwrap()
        .to_json();
    assert_eq!(decoded["call_module"], "System");
    assert_eq!(decoded["call_function"], "remark");
    let mut trailing = call.as_bytes().to_vec();
    trailing.push(0);
    assert!(
        executor
            .client()
            .decode_scale("Call", &trailing)
            .await
            .is_err()
    );
    assert!(
        executor
            .client()
            .decode_scale("UnknownType", &[])
            .await
            .is_err()
    );
    assert!(executor.client().decode_scale("bool", &[2]).await.is_err());
    let decoded = executor
        .client()
        .decode_scale("u64", &123u64.to_le_bytes())
        .await
        .unwrap();
    assert_eq!(decoded.as_u128(), Some(123));
}

#[tokio::test]
async fn composition_checks_stamps_and_refuses_batch_inside_origin_wrappers() {
    let (executor, _, keys, call) = fixture(1_000_000_000).await;
    let runtime = View::runtime(executor.client()).await.unwrap();
    assert!(rtao_client::compose::as_multi_threshold_1(&runtime, &[], call.clone()).is_err());
    let batch = rtao_client::compose::batch_all(&runtime, vec![call.clone()]).unwrap();
    let proxy = rtao_client::compose::proxy(&runtime, keys.key.account(), None, batch).unwrap();
    assert!(rtao_client::compose::batch_all(&runtime, vec![proxy]).is_err());
    let sudo = rtao_client::compose::sudo(&runtime, call.clone()).unwrap();
    assert_eq!(
        rtao_client::compose::sudo(&runtime, sudo.clone()).unwrap(),
        sudo
    );
    let updated = rtao_runtime::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        2,
        42,
    )
    .unwrap();
    assert!(rtao_client::compose::proxy(&updated, keys.key.account(), None, call).is_err());
    let estimate = executor
        .client()
        .estimate_weight(sudo, keys.key.account(), CryptoType::Ed25519)
        .await
        .unwrap();
    let approval = rtao_client::compose::approve_as_multi(
        executor.client(),
        keys.key.account(),
        &[keys.key.account(), AccountId([9; 32])],
        2,
        rtao_primitives::H256([7; 32]),
        rtao_runtime::models::MultisigTimepoint {
            height: 12,
            index: 3,
        },
        &estimate,
    )
    .await
    .unwrap();
    assert_eq!(
        executor
            .client()
            .decode_scale("Call", approval.as_bytes())
            .await
            .unwrap()
            .to_json()["call_function"],
        "approve_as_multi"
    );
    assert_eq!(
        executor
            .client()
            .storage(
                &rtao_runtime::storage::multisig::MULTISIGS,
                (group(&executor, &keys, 2).await.address, [7; 32])
            )
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn shielded_approval_preserves_full_wrapper_and_propagates_finalization() {
    for threshold in [1, 2] {
        let (executor, fake, keys, call) = fixture(1_000_000_000).await;
        let multisig = group(&executor, &keys, threshold).await;
        let client = executor.client();
        let estimate = client
            .estimate_weight(call.clone(), keys.key.account(), CryptoType::Ed25519)
            .await
            .unwrap();
        let wrapper = rtao_client::compose::as_multi(
            client,
            keys.key.account(),
            &multisig.signatories,
            threshold,
            call.clone(),
            &estimate,
        )
        .await
        .unwrap();
        let opts = rtao_client::SubmitOptions {
            nonce: rtao_client::NoncePolicy::Explicit(1),
            mortality: rtao_client::Mortality::Mortal(rtao_client::Period::new(8)),
            ..Default::default()
        };
        let unsigned = client
            .prepare_call(wrapper.clone(), &keys.key, &opts)
            .await
            .unwrap();
        let signature = keys
            .key
            .sign(&unsigned.signing_request(rtao_signer::SignPurpose::ShieldInner))
            .await
            .unwrap();
        let (inner_bytes, inner_hash) =
            client.attach_signature(&unsigned, signature).await.unwrap();
        fake.mutate_state(|state|state.set_handler("chain_getBlock", move |state, request| {
            let mut reply = state.dispatch(request);
            if let Some(Ok(raw)) = &mut reply.outcome
                && raw["block"]["header"]["number"] == "0x65" {
                raw["block"]["extrinsics"].as_array_mut().unwrap().push(json!(format!("0x{}",hex::encode(&inner_bytes))));
                let success = |index|json!({"phase":"ApplyExtrinsic","extrinsic_idx":index,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]});
                state.seed_events(request.params[0].as_str().unwrap(),json!([success(0),success(1)])).unwrap();
            }
            reply
        }));
        let result = multisig
            .approve(
                &executor,
                call,
                &keys,
                &ApproveOptions {
                    shielded: true,
                    wait_for_finalization: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.inclusion, rtao_client::Inclusion::Finalized);
        assert_eq!(result.data["shielded"], true);
        assert_eq!(
            result.data["inner_extrinsic_hash"],
            format!("0x{}", hex::encode(inner_hash.0))
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        let bytes = fake.state().lock().unwrap().submissions[0].clone();
        assert!(!bytes.ends_with(wrapper.as_bytes()));
        assert_eq!(fake.state().lock().unwrap().submissions.len(), 1);
        let next = client.nonces().lease(&keys.key.account()).await.unwrap();
        assert_eq!(next.value(), 2);
        next.settle();
    }
}
