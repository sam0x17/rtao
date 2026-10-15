//! Delegation and leasing projections through authenticated metadata and SCALE state.
#![allow(clippy::unwrap_used, clippy::panic)] // Broken authenticated fixtures must fail the test.

use std::collections::BTreeSet;

use futures::TryStreamExt;
use rtao_client::{BlockRef, Client, ClientConfig, Network, View, ViewExt};
use rtao_core::codec::Value as CoreValue;
use rtao_primitives::{AccountId, DynBalance, IntoCoreValue, NetUid};
use rtao_reads::{ReadError, ReadName, reads::m6_delegation as reads};
use rtao_runtime::storage as st;
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use serde_json::{Value, json};

const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OTHER: AccountId = AccountId([9; 32]);
const NET: NetUid = NetUid(1);

async fn client(state: FakeState) -> (Client, FakeChain) {
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (client, chain)
}

fn delegate_fixture(hotkey: AccountId, take: u16) -> Value {
    json!({
        "delegate_ss58": hotkey,
        "owner_ss58": COLD,
        "take": take,
        "nominators": [[OTHER, [[1, 3], [2, 5]]], [COLD, []]],
        "registrations": [3, 1, 3],
        "validator_permits": [3, 1],
        "return_per_1000": u64::MAX,
        "total_daily_return": 0
    })
}

fn seed() -> FakeState {
    let mut state = FakeState::default();
    for (item, keys, value) in [
        (
            "ChildKeys",
            vec![json!(HOT), json!(1)],
            json!([[u64::MAX, OTHER], [0, COLD], [3, OTHER]]),
        ),
        (
            "ParentKeys",
            vec![json!(HOT), json!(1)],
            json!([[1, COLD], [u64::MAX, OTHER]]),
        ),
        (
            "PendingChildKeys",
            vec![json!(1), json!(HOT)],
            json!([[[u64::MAX, OTHER], [0, COLD]], u64::MAX]),
        ),
        ("ChildkeyThresholdSuspended", vec![json!(HOT)], json!([])),
    ] {
        state
            .seed_python("SubtensorModule", item, &keys, value)
            .unwrap();
    }
    for (id, end, share) in [(11u32, Some(123u32), 100), (2, None, 0), (1, Some(0), 17)] {
        state
            .seed_python(
                "SubtensorModule",
                "SubnetLeases",
                &[json!(id)],
                json!({"beneficiary": OTHER, "coldkey": COLD, "hotkey": HOT, "emissions_share": share, "end_block": end, "netuid": 1, "cost": u64::MAX}),
            )
            .unwrap();
        state
            .seed_python(
                "Crowdloan",
                "Crowdloans",
                &[json!(id)],
                json!({"creator": COLD, "deposit": u64::MAX, "min_contribution": 0, "end": u32::MAX, "cap": u64::MAX, "funds_account": AccountId::ZERO, "raised": 1000000001u64, "target_address": (id == 11).then_some(HOT), "call": null, "finalized": id == 2, "contributors_count": 2}),
            )
            .unwrap();
    }
    for (id, account, amount) in [
        (11u32, OTHER, u64::MAX),
        (11, COLD, 0),
        (11, HOT, 1000000001),
        (12, COLD, 9999999999),
    ] {
        state
            .seed_python(
                "Crowdloan",
                "Contributions",
                &[json!(id), json!(account)],
                json!(amount),
            )
            .unwrap();
    }
    state
        .seed_runtime(
            "DelegateInfoRuntimeApi",
            "get_delegates",
            json!([delegate_fixture(OTHER, 0), delegate_fixture(HOT, u16::MAX)]),
        )
        .unwrap();
    state
        .seed_runtime(
            "DelegateInfoRuntimeApi",
            "get_delegate",
            delegate_fixture(HOT, u16::MAX),
        )
        .unwrap();
    state
}

const NAMES: [ReadName; 12] = [
    ReadName::ChildkeyThresholdSuspended,
    ReadName::Children,
    ReadName::Delegate,
    ReadName::Delegates,
    ReadName::IsDelegate,
    ReadName::Parents,
    ReadName::PendingChildren,
    ReadName::Crowdloan,
    ReadName::CrowdloanContributors,
    ReadName::Crowdloans,
    ReadName::Lease,
    ReadName::Leases,
];

// These call the port directly until generated namespace adoption; the public catalog's
// authoritative hit set already exists and is checked independently of dispatch.
async fn run<V: View + ?Sized>(view: &V, name: ReadName) -> Result<Value, ReadError> {
    macro_rules! output {
        ($call:expr) => {
            Ok(serde_json::to_value($call.await?).unwrap())
        };
    }
    match name {
        ReadName::ChildkeyThresholdSuspended => {
            output!(reads::childkey_threshold_suspended(view, HOT))
        }
        ReadName::Children => output!(reads::children(view, HOT, NET)),
        ReadName::Delegate => output!(reads::delegate(view, HOT)),
        ReadName::Delegates => output!(reads::delegates(view)),
        ReadName::IsDelegate => output!(reads::is_delegate(view, HOT)),
        ReadName::Parents => output!(reads::parents(view, HOT, NET)),
        ReadName::PendingChildren => output!(reads::pending_children(view, HOT, NET)),
        ReadName::Crowdloan => output!(reads::crowdloan(view, 11)),
        ReadName::CrowdloanContributors => output!(reads::crowdloan_contributors(view, 11)),
        ReadName::Crowdloans => output!(reads::crowdloans(view)),
        ReadName::Lease => output!(reads::lease(view, 11)),
        ReadName::Leases => output!(reads::leases(view)),
        _ => panic!("unexpected delegation/leasing fixture {name}"),
    }
}

async fn check_hits<V: View>(view: RecordingView<V>, chain: &FakeChain, name: ReadName) -> Value {
    let before = chain.requests().len();
    let output = run(&view, name).await.unwrap();
    let hashes = view.block_hashes();
    assert_eq!(hashes.len(), 1, "{name}: one immutable state");
    let hash = *hashes.first().unwrap();
    let mut observed = BTreeSet::new();
    for touch in view.touches() {
        assert_eq!(touch.at, Some(hash), "{name}");
        assert!(
            name.spec().hits.iter().any(|hit| hit.matches(&touch)),
            "{name}: unexpected {touch:?}"
        );
        observed.insert((touch.kind, touch.pallet_or_api, touch.item));
    }
    assert_eq!(observed.len(), name.spec().hits.len(), "{name}: all hits");
    for request in &chain.requests()[before..] {
        if [
            "state_getStorage",
            "state_queryStorageAt",
            "state_call",
            "state_getKeysPaged",
        ]
        .contains(&request.method.as_str())
        {
            assert_eq!(
                request.params.as_array().unwrap().last(),
                Some(&json!(format!("0x{}", hex::encode(hash.0)))),
                "{name}: {request:?}"
            );
        }
    }
    output
}

#[tokio::test]
async fn all_twelve_reads_cover_exact_descriptors_at_head_and_pinned_history() {
    let (client, chain) = client(seed()).await;
    for name in NAMES {
        let head = check_hits(RecordingView::new(client.clone()), &chain, name).await;
        let snapshot = client.at(BlockRef::Number(99)).await.unwrap();
        let historical = check_hits(RecordingView::new(snapshot), &chain, name).await;
        assert_eq!(head, historical, "{name}: fake state is unchanged");
    }
    client.close().await;
}

#[tokio::test]
async fn delegates_keep_native_boundaries_and_runtime_sequence_without_deduplication() {
    let (client, _) = client(seed()).await;
    let delegates = reads::delegates(&client).await.unwrap();
    assert_eq!(delegates.len(), 2);
    assert_eq!(delegates[0].hotkey, OTHER);
    assert_eq!(delegates[0].take, 0.0);
    assert_eq!(delegates[1].hotkey, HOT);
    assert_eq!(delegates[1].take, 1.0);
    assert_eq!(delegates[1].owner, COLD);
    assert_eq!(delegates[1].nominators, 2);
    assert_eq!(delegates[1].registrations, [NetUid(3), NET, NetUid(3)]);
    assert_eq!(delegates[1].validator_permits, [NetUid(3), NET]);
    assert_eq!(delegates[1].return_per_1000.rao, u64::MAX);
    assert_eq!(delegates[1].return_per_1000.netuid, NetUid::ROOT);
    assert_eq!(delegates[1].total_daily_return.rao, 0);
    assert_eq!(
        reads::delegate(&client, HOT).await.unwrap(),
        Some(delegates[1].clone())
    );
    assert!(reads::is_delegate(&client, HOT).await.unwrap());
    client.close().await;
}

#[tokio::test]
async fn optional_delegate_and_empty_delegate_list_preserve_absence() {
    let mut state = seed();
    state
        .seed_runtime("DelegateInfoRuntimeApi", "get_delegate", Value::Null)
        .unwrap();
    state
        .seed_runtime("DelegateInfoRuntimeApi", "get_delegates", json!([]))
        .unwrap();
    let (client, _) = client(state).await;
    assert!(reads::delegate(&client, HOT).await.unwrap().is_none());
    assert!(!reads::is_delegate(&client, HOT).await.unwrap());
    assert!(reads::delegates(&client).await.unwrap().is_empty());
    client.close().await;
}

#[tokio::test]
async fn child_relations_keep_exact_proportions_and_distinguish_present_unit_markers() {
    let (client, _) = client(seed()).await;
    assert_eq!(
        reads::children(&client, HOT, NET).await.unwrap(),
        [(u64::MAX, OTHER), (0, COLD), (3, OTHER)]
    );
    assert_eq!(
        reads::parents(&client, HOT, NET).await.unwrap(),
        [(1, COLD), (u64::MAX, OTHER)]
    );
    let pending = reads::pending_children(&client, HOT, NET).await.unwrap();
    assert_eq!(pending.children, [(u64::MAX, OTHER), (0, COLD)]);
    assert_eq!(pending.cooldown_block, u64::MAX);
    assert!(
        reads::childkey_threshold_suspended(&client, HOT)
            .await
            .unwrap()
    );
    assert!(
        !reads::childkey_threshold_suspended(&client, OTHER)
            .await
            .unwrap()
    );
    assert!(
        reads::children(&client, OTHER, NET)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reads::parents(&client, OTHER, NET)
            .await
            .unwrap()
            .is_empty()
    );
    let missing = reads::pending_children(&client, OTHER, NET).await.unwrap();
    assert!(missing.children.is_empty());
    assert_eq!(missing.cooldown_block, 0);
    client.close().await;
}

#[tokio::test]
// upstream-e2e: sdk/bittensor-core/tests/e2e.rs::test_leases_read
async fn leases_preserve_cost_precision_perpetual_expiry_and_map_order() {
    let state = seed();
    let mut expected_ids = [1u32, 2, 11].map(|id| {
        let entry = state
            .runtime
            .storage_entry("SubtensorModule", "SubnetLeases")
            .unwrap();
        let key = state
            .runtime
            .storage_key(entry, &[id.into_core_value()])
            .unwrap();
        (key, id)
    });
    expected_ids.sort_by(|left, right| left.0.cmp(&right.0));
    let (client, _) = client(state).await;
    let lease = reads::lease(&client, 11).await.unwrap().unwrap();
    assert_eq!(lease.lease_id, 11);
    assert_eq!(
        (lease.beneficiary, lease.coldkey, lease.hotkey),
        (OTHER, COLD, HOT)
    );
    assert_eq!(lease.emissions_share, 100);
    assert_eq!(lease.end_block, Some(123));
    assert_eq!(lease.netuid, NET);
    assert_eq!(lease.cost.rao, u64::MAX);
    assert_eq!(lease.cost.netuid, NetUid::ROOT);
    assert_eq!(
        reads::lease(&client, 2).await.unwrap().unwrap().end_block,
        None
    );
    assert_eq!(
        reads::lease(&client, 1).await.unwrap().unwrap().end_block,
        Some(0)
    );
    assert!(reads::lease(&client, u32::MAX).await.unwrap().is_none());
    let leases = reads::leases(&client).await.unwrap();
    assert_eq!(
        leases.iter().map(|row| row.lease_id).collect::<Vec<_>>(),
        expected_ids.map(|(_, id)| id)
    );
    assert_eq!(leases.iter().find(|row| row.lease_id == 11), Some(&lease));
    client.close().await;
}

#[tokio::test]
async fn crowdloans_sort_ids_but_contributors_keep_prefix_map_order_and_python_float_values() {
    let (client, _) = client(seed()).await;
    let details = reads::crowdloan(&client, 11).await.unwrap().unwrap();
    assert_eq!(details.creator, COLD);
    assert_eq!(details.deposit.rao, u64::MAX);
    assert_eq!(details.min_contribution.rao, 0);
    assert_eq!(details.cap.rao, u64::MAX);
    assert_eq!(details.raised.rao, 1_000_000_001);
    assert_eq!(details.end, u32::MAX);
    assert!(!details.finalized);
    assert_eq!(details.target_address, Some(HOT));
    assert_eq!(details.funds_account, Some(AccountId::ZERO));
    let second = reads::crowdloan(&client, 2).await.unwrap().unwrap();
    assert!(second.finalized);
    assert_eq!(second.target_address, None);
    assert!(reads::crowdloan(&client, u32::MAX).await.unwrap().is_none());
    let summaries = reads::crowdloans(&client).await.unwrap();
    assert_eq!(
        summaries.iter().map(|row| row.id).collect::<Vec<_>>(),
        [1, 2, 11]
    );
    assert_eq!(summaries[2].creator, COLD);
    assert_eq!(summaries[2].raised_tao, 1.000000001);
    assert_eq!(
        summaries[2].cap_tao,
        DynBalance::from_rao(u64::MAX, NetUid::ROOT).amount()
    );
    assert!(summaries[1].finalized);
    let contributors = reads::crowdloan_contributors(&client, 11).await.unwrap();
    assert_eq!(
        contributors
            .iter()
            .map(|row| row.contributor)
            .collect::<Vec<_>>(),
        [COLD, HOT, OTHER]
    );
    assert_eq!(contributors[0].amount_tao, 0.0);
    assert_eq!(contributors[1].amount_tao, 1.000000001);
    assert_eq!(
        contributors[2].amount_tao,
        DynBalance::from_rao(u64::MAX, NetUid::ROOT).amount()
    );
    assert!(
        reads::crowdloan_contributors(&client, 99)
            .await
            .unwrap()
            .is_empty()
    );
    client.close().await;
}

#[tokio::test]
async fn empty_maps_return_empty_lists_at_one_recorded_hash() {
    // FakeState defaults have no seeded map keys; runtime single-read defaults do
    // not invent entries in an enumerated storage map.
    let (client, chain) = client(FakeState::default()).await;
    for name in [
        ReadName::Leases,
        ReadName::Crowdloans,
        ReadName::CrowdloanContributors,
    ] {
        let result = check_hits(RecordingView::new(client.clone()), &chain, name).await;
        assert_eq!(result, json!([]));
    }
    client.close().await;
}

#[tokio::test]
async fn malformed_present_storage_and_runtime_payloads_are_errors_not_empty_defaults() {
    for (name, pallet, item, keys) in [
        (
            ReadName::ChildkeyThresholdSuspended,
            "SubtensorModule",
            "ChildkeyThresholdSuspended",
            vec![HOT.into_core_value()],
        ),
        (
            ReadName::Children,
            "SubtensorModule",
            "ChildKeys",
            vec![HOT.into_core_value(), NET.into_core_value()],
        ),
        (
            ReadName::Parents,
            "SubtensorModule",
            "ParentKeys",
            vec![HOT.into_core_value(), NET.into_core_value()],
        ),
        (
            ReadName::PendingChildren,
            "SubtensorModule",
            "PendingChildKeys",
            vec![NET.into_core_value(), HOT.into_core_value()],
        ),
        (
            ReadName::Lease,
            "SubtensorModule",
            "SubnetLeases",
            vec![11u32.into_core_value()],
        ),
        (
            ReadName::Leases,
            "SubtensorModule",
            "SubnetLeases",
            vec![11u32.into_core_value()],
        ),
        (
            ReadName::Crowdloan,
            "Crowdloan",
            "Crowdloans",
            vec![11u32.into_core_value()],
        ),
        (
            ReadName::Crowdloans,
            "Crowdloan",
            "Crowdloans",
            vec![11u32.into_core_value()],
        ),
        (
            ReadName::CrowdloanContributors,
            "Crowdloan",
            "Contributions",
            vec![11u32.into_core_value(), HOT.into_core_value()],
        ),
    ] {
        let mut state = seed();
        let entry = state.runtime.storage_entry(pallet, item).unwrap();
        let key = state.runtime.storage_key(entry, &keys).unwrap();
        state.seed_encoded(key, Some(vec![255]));
        let (client, _) = client(state).await;
        assert!(
            run(&client, name).await.is_err(),
            "{name}: reject malformed present SCALE"
        );
        client.close().await;
    }
    for name in [
        ReadName::Delegate,
        ReadName::IsDelegate,
        ReadName::Delegates,
    ] {
        let mut state = seed();
        if name == ReadName::Delegates {
            state.seed_runtime_encoded("DelegateInfoRuntimeApi_get_delegates", vec![], vec![4]);
        } else {
            state.seed_runtime_encoded(
                "DelegateInfoRuntimeApi_get_delegate",
                HOT.0.to_vec(),
                vec![2],
            );
        }
        let (client, _) = client(state).await;
        assert!(
            run(&client, name).await.is_err(),
            "{name}: reject malformed option/vector"
        );
        client.close().await;
    }
}

#[tokio::test]
async fn contributor_prefix_scan_matches_native_keys_without_fetching_other_crowdloans() {
    let (client, chain) = client(seed()).await;
    let view = RecordingView::new(client.clone());
    let before = chain.requests().len();
    let contributors = reads::crowdloan_contributors(&view, 11).await.unwrap();
    assert_eq!(contributors.len(), 3);
    let snapshot = client.at(BlockRef::Best).await.unwrap();
    let raw: Vec<_> = snapshot
        .storage_iter(&st::crowdloan::CONTRIBUTIONS, (11u32,))
        .try_collect()
        .await
        .unwrap();
    assert_eq!(raw.len(), 3);
    for request in &chain.requests()[before..] {
        if request.method == "state_getKeysPaged" {
            let prefix = request.params[0].as_str().unwrap();
            // Pallet/item prefix + hashed fixed u32 ID; no free account key.
            assert_eq!((prefix.len() - 2) / 2, 44);
        }
    }
    assert_eq!(view.block_hashes().len(), 1);
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    // Keep a primitive-free-key shape assertion independent of public projection.
    assert!(matches!(raw[0].0[0].as_core(), CoreValue::Str(_)));
    client.close().await;
}
