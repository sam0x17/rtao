// Derived from RaoFoundation/subtensor sdk/python/bittensor/multisig.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Multisig derived-authority protection and protected derivatives (D-154).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{BlockRef, CallBytes, Client, ClientConfig, Network, RuntimeHandle};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::{AccountId, AccountMode};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use rtao_tx::{hashed, multisig::Multisig};
use serde_json::json;

const STRONG: AccountId = AccountId([1; 32]);
const WEAK: AccountId = AccountId([2; 32]);

fn state(strong: bool, enabled: bool) -> FakeState {
    let mut state = FakeState::try_default().unwrap();
    state
        .seed_python("AdminUtils", "HashedAccountsEnabled", &[], json!(enabled))
        .unwrap();
    if strong {
        state
            .seed_python(
                "HashedAccounts",
                "MlDsaAccounts",
                &[json!(STRONG.to_string())],
                json!({}),
            )
            .unwrap();
    }
    state
}

async fn client(state: FakeState) -> Client {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Client::from_rpc(RpcClient::new(FakeChain::from_state(state)), config)
        .await
        .unwrap()
}

fn decoded(runtime: &RuntimeHandle, call: &CallBytes) -> (String, String, rtao_core::codec::Value) {
    let value = runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
        .unwrap();
    (
        value
            .field("call_module")
            .and_then(ValueExt::as_str)
            .unwrap()
            .to_owned(),
        value
            .field("call_function")
            .and_then(ValueExt::as_str)
            .unwrap()
            .to_owned(),
        value,
    )
}

#[tokio::test]
async fn a_quorum_with_a_finalized_quantum_resistant_member_registers_then_checks_its_authority() {
    let client = client(state(true, true)).await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let runtime = snapshot.block().runtime.clone();
    let group = Multisig::with_mode(&runtime, &[STRONG, WEAK], 2, AccountMode::Legacy).unwrap();
    let status = group.protection_status(&client).await.unwrap();
    assert!(status.eligible && !status.registered && !status.safe_derivation);
    let call = group.protection_call(&client, STRONG).await.unwrap();
    let (module, function, _) = decoded(&runtime, &call);
    assert_eq!(
        (module.as_str(), function.as_str()),
        ("HashedAccounts", "register_derived_sponsored")
    );
    // Funding an eligible group before its registration is final refuses.
    let error = group.funding_call(&client, 5).await.unwrap_err();
    assert!(error.to_string().contains("protection_call"), "{error}");
    assert!(group.funding_call(&client, 0).await.is_err());
    // A safe group needs no registration.
    let safe = Multisig::with_mode(&runtime, &[STRONG, WEAK], 2, AccountMode::Safe).unwrap();
    assert!(
        safe.protection_status(&client)
            .await
            .unwrap()
            .safe_derivation
    );
    assert!(safe.protection_call(&client, STRONG).await.is_err());
    let payment = safe.funding_call(&client, 5).await.unwrap();
    assert_eq!(decoded(&runtime, &payment).1, "transfer_keep_alive");
    client.close().await;
}

#[tokio::test]
async fn a_registered_authority_is_checked_and_guards_funding_atomically() {
    let mut state = state(true, true);
    let runtime_probe = client(FakeState::try_default().unwrap()).await;
    let runtime = runtime_probe
        .at(BlockRef::Finalized)
        .await
        .unwrap()
        .block()
        .runtime
        .clone();
    runtime_probe.close().await;
    let group = Multisig::with_mode(&runtime, &[STRONG, WEAK], 2, AccountMode::Legacy).unwrap();
    state
        .seed_python(
            "HashedAccounts",
            "DerivedAuthorities",
            &[json!(group.address.to_string())],
            json!({
                "descriptor": {"Multisig": {
                    "threshold": 2,
                    "signatories": [STRONG.to_string(), WEAK.to_string()],
                }},
                "pq_dependencies": [STRONG.to_string()],
                "depth": 1,
                "nodes": 2,
            }),
        )
        .unwrap();
    let client = client(state).await;
    let status = group.protection_status(&client).await.unwrap();
    assert!(status.eligible && status.registered);
    let check = group.protection_call(&client, STRONG).await.unwrap();
    assert_eq!(decoded(&runtime, &check).1, "check_derived");
    let funding = group.funding_call(&client, 5).await.unwrap();
    let (module, function, value) = decoded(&runtime, &funding);
    assert_eq!(
        (module.as_str(), function.as_str()),
        ("Utility", "batch_all")
    );
    let text = format!("{value:?}");
    assert!(text.contains("check_derived") && text.contains("transfer_keep_alive"));
    client.close().await;
}

#[tokio::test]
async fn weak_quorums_and_disabled_chains_are_refused() {
    // No member has a quantum-resistant authority: no quorum is protected.
    let client = client(state(false, true)).await;
    let runtime = client
        .at(BlockRef::Finalized)
        .await
        .unwrap()
        .block()
        .runtime
        .clone();
    let group = Multisig::with_mode(&runtime, &[STRONG, WEAK], 1, AccountMode::Legacy).unwrap();
    assert!(!group.protection_status(&client).await.unwrap().eligible);
    let error = group.protection_call(&client, STRONG).await.unwrap_err();
    assert!(
        error.to_string().contains("quantum-resistant member"),
        "{error}"
    );
    // An ineligible group is funded with a plain transfer.
    assert_eq!(
        decoded(&runtime, &group.funding_call(&client, 5).await.unwrap()).1,
        "transfer_keep_alive"
    );
    client.close().await;
    let client = self::client(state(true, false)).await;
    let error = group.protection_call(&client, STRONG).await.unwrap_err();
    assert!(error.to_string().contains("not enabled"), "{error}");
    client.close().await;
}

#[tokio::test]
async fn derivatives_follow_the_parent_derivation_and_protection() {
    let client = client(state(true, true)).await;
    let snapshot = client.at(BlockRef::Finalized).await.unwrap();
    let runtime = snapshot.block().runtime.clone();
    let inner = rtao_runtime::calls::system::remark(vec![1, 2, 3])
        .compose(&runtime)
        .unwrap();
    for (parent, mode, expected) in [
        (WEAK, None, "as_derivative"),
        (STRONG, None, "as_protected_derivative"),
        (STRONG, Some(AccountMode::Legacy), "as_protected_derivative"),
        (WEAK, Some(AccountMode::Safe), "as_safe_derivative"),
    ] {
        let call = hashed::compose_derivative(&snapshot, &runtime, parent, 7, inner.clone(), mode)
            .await
            .unwrap();
        assert_eq!(decoded(&runtime, &call).1, expected);
    }
    client.close().await;
}
