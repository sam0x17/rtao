// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_subnet_lifecycle.py @ 5749dbfbe4a78ccd3938d46ffae45e059488feb7 (Apache-2.0); modified by rtao.
//! Lifecycle and stake joins through real metadata, SCALE and one pinned block.
#![allow(clippy::unwrap_used)]
use rtao_client::{Client, ClientConfig, Network, View};
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{
    Namespaces, SubnetLifecycleState as State,
    reads::{lifecycle, m6_root, m6_staking, owner},
};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
async fn client(state: FakeState) -> (Client, FakeChain) {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    (
        Client::from_rpc(RpcClient::new(chain.clone()), config)
            .await
            .unwrap(),
        chain,
    )
}
#[tokio::test]
async fn lifecycle_storage_includes_dissolving_subnets_and_independent_emission() {
    let mut state = FakeState::default();
    for (id, value) in [
        (0, "Started"),
        (1, "Registered"),
        (7, "PendingDissolution"),
        (9, "Dissolving"),
    ] {
        state
            .seed_python("SubtensorModule", "SubnetState", &[json!(id)], json!(value))
            .unwrap();
    }
    state
        .seed_python(
            "SubtensorModule",
            "SubnetEmissionEnabled",
            &[json!(1)],
            json!(false),
        )
        .unwrap();
    for item in [
        "Tempo",
        "Burn",
        "SubnetworkN",
        "MaxAllowedUids",
        "SubnetTaoInEmission",
    ] {
        state.clear_default("SubtensorModule", item).unwrap();
    }
    state
        .seed_python(
            "SubtensorModule",
            "NetworksAdded",
            &[json!(99)],
            json!(false),
        )
        .unwrap();
    let (client, _) = client(state).await;
    assert_eq!(
        client.subnets().state(NetUid(9)).await.unwrap(),
        Some(State::Dissolving)
    );
    let expected = BTreeMap::from([
        (NetUid(0), State::Started),
        (NetUid(1), State::Registered),
        (NetUid(7), State::PendingDissolution),
        (NetUid(9), State::Dissolving),
    ]);
    assert_eq!(client.subnets().states().await.unwrap(), expected);
    assert_eq!(lifecycle::subnet_states(&client).await.unwrap(), expected);
    let rows = m6_root::subnets(&client).await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.netuid, r.state.unwrap()))
            .collect::<BTreeMap<_, _>>(),
        expected
    );
    assert!(!rows[1].pool_emission_enabled);
    assert!(rows[2].pool_emission_enabled);
    assert_eq!(
        rows,
        m6_root::subnet_listing(&client).await.unwrap().subnets
    );
    assert_eq!(
        lifecycle::subnet_state(&client, NetUid(99)).await.unwrap(),
        None
    );
    client.close().await;
}
#[tokio::test]
async fn legacy_lifecycle_preserves_cleanup_precedence_and_zero_first_emission() {
    let runtime = rtao_core::runtime::Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/482.v15.scale"),
        482,
        1,
        42,
    )
    .unwrap();
    let mut state = FakeState::new(Arc::new(runtime));
    for id in [0, 1, 2, 3] {
        state
            .seed_python(
                "SubtensorModule",
                "NetworksAdded",
                &[json!(id)],
                json!(true),
            )
            .unwrap();
    }
    state
        .seed_python(
            "SubtensorModule",
            "FirstEmissionBlockNumber",
            &[json!(2)],
            json!(0),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "SubtokenEnabled",
            &[json!(3)],
            json!(true),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "DissolveCleanupQueue",
            &[],
            json!([7, 9]),
        )
        .unwrap();
    // Fill the runtime's remaining cleanup cursor fields from real metadata defaults.
    let info = state
        .runtime
        .storage_entry("SubtensorModule", "CurrentDissolveCleanupStatus")
        .unwrap();
    let status = rtao_core::DynValue::new(
        rtao_testkit::fakes::fill_defaults(&state.runtime, info.value_type, json!({"netuid":9}))
            .unwrap(),
    )
    .to_json();
    state
        .seed_python(
            "SubtensorModule",
            "CurrentDissolveCleanupStatus",
            &[],
            status,
        )
        .unwrap();
    let (client, _) = client(state).await;
    assert_eq!(
        client.subnets().states().await.unwrap(),
        BTreeMap::from([
            (NetUid(0), State::Started),
            (NetUid(1), State::Registered),
            (NetUid(2), State::Started),
            (NetUid(3), State::Started),
            (NetUid(7), State::PendingDissolution),
            (NetUid(9), State::Dissolving)
        ])
    );
    client.close().await;
}
#[tokio::test]
async fn stake_state_and_price_join_share_the_snapshot() {
    let cold = AccountId([7; 32]);
    let hot = AccountId([8; 32]);
    let mut state = FakeState::default();
    state
        .seed_python(
            "SubtensorModule",
            "SubnetState",
            &[json!(9)],
            json!("PendingDissolution"),
        )
        .unwrap();
    let record = json!({"hotkey":hot,"coldkey":cold,"netuid":9,"stake":2_000_000_000u64,"is_registered":false});
    state
        .seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkey",
            json!([record.clone()]),
        )
        .unwrap();
    state
        .seed_runtime(
            "StakeInfoRuntimeApi",
            "get_stake_info_for_coldkeys",
            json!([[cold, [record]]]),
        )
        .unwrap();
    state
        .seed_runtime(
            "SwapRuntimeApi",
            "current_alpha_price_all",
            json!([{"netuid":9,"price":1_500_000_000u64}]),
        )
        .unwrap();
    let (client, _) = client(state).await;
    let snapshot = client.pin().await.unwrap();
    let recording = RecordingView::new(snapshot.clone());
    let value = owner::stake_value_for_coldkey(&recording, cold)
        .await
        .unwrap();
    assert_eq!(
        value.positions[0].subnet_state,
        Some(State::PendingDissolution)
    );
    assert_eq!(value.stake_value.rao, 3_000_000_000);
    assert_eq!(
        m6_staking::stake_for_coldkey(&recording, cold)
            .await
            .unwrap(),
        value.positions
    );
    assert_eq!(
        m6_staking::stake_for_coldkeys(&recording, vec![cold])
            .await
            .unwrap()[&cold],
        value.positions
    );
    assert_eq!(
        owner::stake_value_for_coldkeys(&recording, vec![cold])
            .await
            .unwrap()[&cold],
        value
    );
    assert_eq!(
        recording.block_hashes(),
        std::collections::BTreeSet::from([snapshot.block().hash])
    );
    client.close().await;
}
