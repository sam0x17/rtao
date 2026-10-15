//! Epoch refresh, UID score-reset data and cancellation over the real client.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use blake2::{Blake2b, Digest, digest::consts::U8};
use rtao_client::{Client, ClientConfig, Network};
use rtao_core::{codec::Value, runtime::Runtime};
use rtao_metagraph::{
    DecodePath, DecodePreference, Metagraph, MetagraphDiff, MetagraphOptions, MetagraphSync,
    Preset, SnapshotInfo, SubnetsMetagraphExt, SyncCadence, SyncPolicy,
};
use rtao_primitives::{AccountId, H256, MechId, NetUid, Uid};
use rtao_reads::Namespaces;
use rtao_runtime::SelectiveMetagraphIndex as Field;
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, Reply};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn value(keys: &[u8], permit: &[bool], ports: &[u16]) -> Value {
    let fields = vec![
        ("netuid", Value::Int(1)),
        ("block", Value::Int(100)),
        ("tempo", Value::Int(10)),
        ("last_step", Value::Int(90)),
        ("blocks_since_last_step", Value::Int(10)),
        ("network_registered_at", Value::Int(1)),
        ("owner_hotkey", Value::Bytes(vec![1; 32])),
        ("owner_coldkey", Value::Bytes(vec![2; 32])),
        ("num_uids", Value::Int(keys.len() as i128)),
        (
            "hotkeys",
            Value::List(
                keys.iter()
                    .map(|key| Value::Bytes(vec![*key; 32]))
                    .collect(),
            ),
        ),
        (
            "validator_permit",
            Value::List(permit.iter().copied().map(Value::Bool).collect()),
        ),
        (
            "axons",
            Value::List(
                ports
                    .iter()
                    .map(|port| {
                        Value::record(vec![
                            ("block".into(), Value::Int(100)),
                            ("version".into(), Value::Int(1)),
                            ("ip".into(), Value::Int(0x7f000001)),
                            ("port".into(), Value::Int(i128::from(*port))),
                            ("ip_type".into(), Value::Int(4)),
                            ("protocol".into(), Value::Int(0)),
                            ("placeholder1".into(), Value::Int(0)),
                            ("placeholder2".into(), Value::Int(0)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    Value::record(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}
fn graph(keys: &[u8], permit: &[bool], ports: &[u16], number: u64) -> Metagraph {
    Metagraph::from_dynamic(
        NetUid(1),
        value(keys, permit, ports),
        SnapshotInfo {
            number,
            hash: H256([number as u8; 32]),
            spec_version: 478,
        },
    )
    .unwrap()
}
async fn setup() -> (FakeChain, Client) {
    let runtime = Arc::new(
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap(),
    );
    let mut state = FakeState::new(runtime);
    let api = Blake2b::<U8>::digest(b"SubnetInfoRuntimeApi");
    state.set_result("state_getRuntimeVersion",json!({"specName":"node-subtensor","implName":"fake-subtensor","authoringVersion":1,"specVersion":478,"implVersion":1,"transactionVersion":1,"stateVersion":1,"apis":[[format!("0x{}",hex::encode(api)),2]]}));
    let mut selected =
        rtao_core::codec::value::to_corpus_json(&value(&[11, 12], &[false, true], &[80, 81]));
    for name in ["owner_hotkey", "owner_coldkey", "network_registered_at"] {
        selected.as_object_mut().unwrap().remove(name);
    }
    state
        .seed_runtime("SubnetInfoRuntimeApi", "get_selective_mechagraph", selected)
        .unwrap();
    state
        .seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_mechagraph",
            rtao_core::codec::value::to_corpus_json(&value(&[11, 12], &[false, true], &[80, 81])),
        )
        .unwrap();
    state
        .seed_python("SubtensorModule", "LastEpochBlock", &[json!(1)], json!(90))
        .unwrap();
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (chain, client)
}
fn epoch(chain: &FakeChain, value: u64) {
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "LastEpochBlock",
                &[json!(1)],
                json!(value),
            )
            .unwrap()
    });
}
async fn diff(receiver: &mut tokio::sync::broadcast::Receiver<MetagraphDiff>) -> MetagraphDiff {
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .expect("worker did not refresh")
        .unwrap()
}
async fn settle() -> () {
    tokio::time::sleep(Duration::from_millis(40)).await;
}

fn advance_events(chain: &FakeChain, events: serde_json::Value) -> u64 {
    let (header, number) = chain.mutate_state(|state| {
        let header = state.advance(1).pop().unwrap();
        let number = state.finalized;
        let hash = state.block_hash(number).unwrap();
        state.seed_events(hash, events).unwrap();
        (header, number)
    });
    chain
        .publish_method("chain_subscribeFinalizedHeads", header)
        .unwrap();
    number
}

fn event(name: &str, attributes: serde_json::Value) -> serde_json::Value {
    json!({"phase":"Initialization", "module_id":"SubtensorModule",
        "event_id":name, "attributes":attributes, "topics":[]})
}

async fn drain_worker() {
    // Keep paused-clock tests runnable while the immediate fake RPCs complete.
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

fn selective_calls(chain: &FakeChain) -> Vec<rtao_testkit::RpcRequest> {
    chain
        .requests()
        .into_iter()
        .filter(|request| {
            request.method == "state_call"
                && request.params[0] == "SubnetInfoRuntimeApi_get_selective_mechagraph"
        })
        .collect()
}

#[test]
fn miner_defaults_match_the_four_fields_and_ten_minute_cadence() {
    let policy = SyncPolicy::miner();
    assert_eq!(policy.preset, Preset::Miner);
    assert_eq!(
        policy.cadence,
        SyncCadence::Interval(Duration::from_secs(600))
    );
    assert!(!policy.commitments);
    assert!(policy.early_on_events);
    assert_eq!(
        policy.preset.fields().unwrap(),
        vec![
            Field::Hotkeys,
            Field::ValidatorPermit,
            Field::TotalStake,
            Field::Validators
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn miner_interval_fetches_exact_selection_after_ten_minutes() {
    let (chain, client) = setup().await;
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::miner(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    drain_worker().await;
    chain.advance(1).unwrap();
    drain_worker().await;
    tokio::time::advance(Duration::from_secs(599)).await;
    drain_worker().await;
    assert!(receiver.try_recv().is_err());
    assert_eq!(selective_calls(&chain).len(), 1);
    let fields = Preset::Miner.fields().unwrap();
    let mut expected = vec![1, 0, 0, 16]; // netuid, mechid, SCALE length four.
    expected.extend(
        fields
            .iter()
            .flat_map(|field| u16::from(field.index()).to_le_bytes()),
    );
    assert_eq!(
        selective_calls(&chain)[0].params[1],
        json!(format!("0x{}", hex::encode(expected)))
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(diff(&mut receiver).await.to_block, 101);
    assert_eq!(selective_calls(&chain).len(), 2);
    handle.shutdown().await;
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn relevant_finalized_events_coalesce_and_ignore_unrelated_subnets_and_hotkeys() {
    let (chain, client) = setup().await;
    let start = chain.requests().len();
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::miner(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    drain_worker().await;
    let account = |byte| AccountId([byte; 32]).to_ss58(42);
    advance_events(
        &chain,
        json!([
            event("NeuronRegistered", json!([2, 2, account(44)])),
            event("AxonServed", json!([2, account(44)])),
            event(
                "HotkeySwapped",
                json!({"coldkey":account(1), "old_hotkey":account(44), "new_hotkey":account(45)})
            ),
            event(
                "HotkeySwappedOnSubnet",
                json!({"coldkey":account(1), "old_hotkey":account(11), "new_hotkey":account(45), "netuid":2})
            ),
        ]),
    );
    drain_worker().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    drain_worker().await;
    assert!(receiver.try_recv().is_err());
    assert_eq!(selective_calls(&chain).len(), 1);
    advance_events(
        &chain,
        json!([
            event("NeuronRegistered", json!([1, 2, account(44)])),
            event("AxonServed", json!([1, account(44)])),
        ]),
    );
    drain_worker().await;
    tokio::time::advance(Duration::from_millis(500)).await;
    advance_events(
        &chain,
        json!([
            event(
                "HotkeySwapped",
                json!({"coldkey":account(1), "old_hotkey":account(11), "new_hotkey":account(99)})
            ),
            event(
                "HotkeySwappedOnSubnet",
                json!({"coldkey":account(1), "old_hotkey":account(11), "new_hotkey":account(99), "netuid":1})
            ),
        ]),
    );
    drain_worker().await;
    tokio::time::advance(Duration::from_millis(499)).await;
    drain_worker().await;
    assert!(receiver.try_recv().is_err());
    assert_eq!(selective_calls(&chain).len(), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(diff(&mut receiver).await.to_block, 103);
    assert_eq!(selective_calls(&chain).len(), 2);
    assert!(receiver.try_recv().is_err());
    for attributes in [
        event("AxonServed", json!([1, account(44)])),
        event(
            "HotkeySwapped",
            json!({"coldkey":account(1), "old_hotkey":account(11), "new_hotkey":account(99)}),
        ),
        event(
            "HotkeySwapped",
            json!({"coldkey":account(1), "old_hotkey":account(99), "new_hotkey":account(12)}),
        ),
        event(
            "HotkeySwappedOnSubnet",
            json!({"coldkey":account(1), "old_hotkey":account(99), "new_hotkey":account(98), "netuid":1}),
        ),
    ] {
        let number = advance_events(&chain, json!([attributes]));
        drain_worker().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(diff(&mut receiver).await.to_block, number);
        assert!(receiver.try_recv().is_err());
    }
    let last = selective_calls(&chain).pop().unwrap();
    assert_eq!(
        last.params[2],
        json!(format!("0x{}", hex::encode(handle.load().at.hash.0)))
    );
    assert!(
        !chain
            .requests()
            .iter()
            .skip(start)
            .any(|request| request.method == "state_getKeysPaged")
    );
    handle.shutdown().await;
    client.close().await;
}

#[tokio::test]
async fn miner_shutdown_interrupts_an_unanswered_event_read_and_keeps_last_good() {
    let (chain, client) = setup().await;
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::miner(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let initial = handle.load();
    let before = chain.requests().len();
    chain.mutate_state(|state| state.queue_reply("state_getStorage", Reply::silent()));
    chain.advance(1).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if chain
                .requests()
                .iter()
                .skip(before)
                .any(|request| request.method == "state_getStorage")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&initial, &handle.load()));
    assert!(handle.health().borrow().stopped);
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_event_read_refreshes_conservatively_and_retains_health_until_success() {
    let (chain, client) = setup().await;
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::miner(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let initial = handle.load();
    drain_worker().await;
    chain.mutate_state(|state| state.queue_reply("state_getStorage", Reply::result(json!("0xff"))));
    chain.advance(1).unwrap();
    drain_worker().await;
    assert_eq!(handle.health().borrow().consecutive_failures, 1);
    assert!(Arc::ptr_eq(&initial, &handle.load()));
    assert!(receiver.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(diff(&mut receiver).await.to_block, 101);
    assert_eq!(handle.health().borrow().consecutive_failures, 0);
    handle.shutdown().await;
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_event_graph_refresh_keeps_invalidation_and_retries_after_thirty_seconds() {
    let (chain, client) = setup().await;
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::miner(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let initial = handle.load();
    drain_worker().await;
    advance_events(
        &chain,
        json!([event(
            "NeuronRegistered",
            json!([1, 2, AccountId([44; 32]).to_ss58(42)])
        )]),
    );
    drain_worker().await;
    chain.mutate_state(|state| state.queue_reply("state_call", Reply::result(json!("0x00"))));
    tokio::time::advance(Duration::from_secs(1)).await;
    drain_worker().await;
    assert_eq!(handle.health().borrow().consecutive_failures, 1);
    assert!(Arc::ptr_eq(&initial, &handle.load()));
    assert!(receiver.try_recv().is_err());
    assert_eq!(selective_calls(&chain).len(), 2);
    chain.advance(1).unwrap();
    drain_worker().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    drain_worker().await;
    assert!(receiver.try_recv().is_err());
    assert_eq!(selective_calls(&chain).len(), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(diff(&mut receiver).await.to_block, 102);
    assert_eq!(selective_calls(&chain).len(), 3);
    assert_eq!(handle.health().borrow().consecutive_failures, 0);
    handle.shutdown().await;
    client.close().await;
}

#[test]
fn uid_changes_report_complete_identity_and_reset_only_reused_or_removed_scores() {
    let old = graph(&[11, 12, 13], &[false, true, true], &[80, 81, 82], 100);
    let new = graph(&[11, 99], &[true, false], &[80, 90], 110);
    let change = MetagraphDiff::between(&old, &new);
    assert_eq!(change.from_block, 100);
    assert_eq!(change.to_block, 110);
    assert_eq!(
        change.replaced,
        vec![(Uid(1), AccountId([12; 32]), AccountId([99; 32]))]
    );
    assert_eq!(change.deregistered, vec![(Uid(2), AccountId([13; 32]))]);
    assert!(change.registered.is_empty());
    assert_eq!(change.permits_gained, vec![Uid(0)]);
    assert_eq!(change.permits_lost, vec![Uid(1), Uid(2)]);
    assert_eq!(change.axons_changed, vec![Uid(1), Uid(2)]);
    assert_eq!(
        change.score_resets().collect::<Vec<_>>(),
        vec![Uid(1), Uid(2)]
    );
    let growth = MetagraphDiff::between(
        &new,
        &graph(&[11, 99, 42], &[true, false, false], &[80, 90, 92], 111),
    );
    assert_eq!(growth.registered, vec![(Uid(2), AccountId([42; 32]))]);
    assert!(growth.score_resets().next().is_none());
}
#[test]
fn validator_defaults_match_the_architecture_selection() {
    let policy = SyncPolicy::default();
    assert_eq!(
        policy.cadence,
        SyncCadence::EveryEpoch {
            fallback_blocks: 100
        }
    );
    assert_eq!(policy.preset, Preset::Validator);
    assert!(!policy.commitments);
    assert!(!policy.early_on_events);
    assert_eq!(policy.reconnect_after_failures, 3);
    let fields = policy.preset.fields().unwrap();
    assert_eq!(fields.len(), 20);
    assert!(fields.contains(&Field::Hotkeys));
    assert!(fields.contains(&Field::TotalStake));
    assert!(!fields.contains(&Field::AlphaStake));
}
#[tokio::test]
async fn selective_read_preserves_raw_absences_and_the_requested_mechanism() {
    let (chain, client) = setup().await;
    let start = chain.requests().len();
    let graph = client
        .subnets()
        .selective_metagraph_with(
            NetUid(1),
            MechId(2),
            &Preset::Validator.fields().unwrap(),
            MetagraphOptions {
                commitments: false,
                ..MetagraphOptions::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.header.mechid, MechId(2));
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    assert_eq!(graph.len(), 2);
    assert!(graph.commitments.is_none());
    let raw = graph.to_json().unwrap()["raw"].clone();
    assert!(raw["name"].is_null());
    assert!(raw["owner_hotkey"].is_null());
    let calls = chain
        .requests()
        .into_iter()
        .filter(|request| {
            request.method == "state_call"
                && request.params[0] == "SubnetInfoRuntimeApi_get_selective_mechagraph"
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].params.as_array().unwrap().last().unwrap(),
        &json!(format!("0x{}", hex::encode(graph.at.hash.0)))
    );
    assert!(
        !chain
            .requests()
            .iter()
            .skip(start)
            .any(|request| request.method == "state_getKeysPaged")
    );
    client.close().await;
}
#[tokio::test]
async fn epoch_advance_refreshes_once_and_block_fallback_refreshes_without_an_epoch_change() {
    let (chain, client) = setup().await;
    let policy = SyncPolicy {
        cadence: SyncCadence::EveryEpoch { fallback_blocks: 3 },
        ..SyncPolicy::validator()
    };
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        policy,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let initial = handle.load();
    assert_eq!(initial.at.number, 100);
    chain.advance(1).unwrap();
    settle().await;
    assert!(receiver.try_recv().is_err());
    assert!(Arc::ptr_eq(&initial, &handle.load()));
    epoch(&chain, 101);
    chain.advance(1).unwrap();
    let refreshed = diff(&mut receiver).await;
    assert_eq!(refreshed.to_block, 102);
    chain.advance(1).unwrap();
    settle().await;
    assert!(receiver.try_recv().is_err());
    chain.advance(2).unwrap();
    assert_eq!(diff(&mut receiver).await.to_block, 105);
    handle.shutdown().await;
    assert!(handle.health().borrow().stopped);
    client.close().await;
}
#[tokio::test]
async fn failures_keep_last_good_mark_health_after_three_and_success_recovers() {
    let (chain, client) = setup().await;
    let policy = SyncPolicy {
        cadence: SyncCadence::EveryBlocks(1),
        ..SyncPolicy::validator()
    };
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        policy,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let initial = handle.load();
    for count in 1..=3 {
        chain.mutate_state(|state| {
            state.queue_reply(
                "state_call",
                Reply::error(-32000, "fixture graph read failed", None),
            )
        });
        chain.advance(1).unwrap();
        settle().await;
        assert!(Arc::ptr_eq(&initial, &handle.load()));
        assert!(receiver.try_recv().is_err());
        assert_eq!(handle.health().borrow().consecutive_failures, count);
    }
    assert!(handle.health().borrow().unhealthy);
    chain.advance(1).unwrap();
    assert_eq!(diff(&mut receiver).await.to_block, 104);
    assert_eq!(handle.health().borrow().consecutive_failures, 0);
    assert!(!handle.health().borrow().unhealthy);
    assert!(handle.health().borrow().last_error.is_none());
    handle.shutdown().await;
    client.close().await;
}
#[tokio::test]
async fn empty_refresh_retains_good_graph_and_empty_startup_returns_no_handle() {
    let (chain, client) = setup().await;
    let policy = SyncPolicy {
        cadence: SyncCadence::EveryBlocks(1),
        ..SyncPolicy::validator()
    };
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        policy.clone(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_selective_mechagraph",
                rtao_core::codec::value::to_corpus_json(&value(&[], &[], &[])),
            )
            .unwrap()
    });
    chain.advance(1).unwrap();
    settle().await;
    assert_eq!(handle.load().len(), 2);
    assert_eq!(handle.health().borrow().consecutive_failures, 1);
    assert!(receiver.try_recv().is_err());
    assert!(
        MetagraphSync::spawn(
            client.clone(),
            NetUid(1),
            MechId(0),
            policy,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    handle.shutdown().await;
    client.close().await;
}
#[tokio::test]
async fn cancelled_bootstrap_and_last_handle_drop_release_the_refresh_worker() {
    let (chain, client) = setup().await;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        MetagraphSync::spawn(
            client.clone(),
            NetUid(1),
            MechId(0),
            SyncPolicy::validator(),
            cancelled
        )
        .await
        .is_err()
    );
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            cadence: SyncCadence::EveryBlocks(1),
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let mut health = handle.health();
    let clone = handle.clone();
    drop(handle);
    chain.advance(1).unwrap();
    settle().await;
    assert!(!health.borrow().stopped);
    drop(clone);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !health.borrow().stopped {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let before = chain
        .requests()
        .iter()
        .filter(|request| request.method == "state_call")
        .count();
    chain.advance(2).unwrap();
    settle().await;
    assert_eq!(
        chain
            .requests()
            .iter()
            .filter(|request| request.method == "state_call")
            .count(),
        before
    );
    client.close().await;
}
#[tokio::test]
async fn invalid_policies_fail_before_a_graph_rpc_and_full_preset_uses_mechanism_api() {
    let (chain, client) = setup().await;
    let before = chain.requests().len();
    for policy in [
        SyncPolicy {
            cadence: SyncCadence::EveryBlocks(0),
            ..SyncPolicy::validator()
        },
        SyncPolicy {
            cadence: SyncCadence::Interval(Duration::from_secs(29)),
            ..SyncPolicy::validator()
        },
        SyncPolicy {
            preset: Preset::Custom(vec![Field::Axons]),
            ..SyncPolicy::validator()
        },
        SyncPolicy {
            preset: Preset::Custom(vec![Field::Hotkeys, Field::Hotkeys]),
            ..SyncPolicy::validator()
        },
    ] {
        assert!(
            MetagraphSync::spawn(
                client.clone(),
                NetUid(1),
                MechId(0),
                policy,
                CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
    assert_eq!(chain.requests().len(), before);
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            preset: Preset::Full,
            commitments: false,
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(handle.load().len(), 2);
    assert!(
        chain
            .requests()
            .iter()
            .any(|request| request.params[0] == "SubnetInfoRuntimeApi_get_mechagraph")
    );
    handle.shutdown().await;
    client.close().await;
}

#[tokio::test(start_paused = true)]
async fn interval_refresh_obeys_thirty_second_minimum_without_head_driven_fetches() {
    let (chain, client) = setup().await;
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            cadence: SyncCadence::Interval(Duration::from_secs(30)),
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    tokio::task::yield_now().await;
    chain.advance(1).unwrap();
    tokio::time::advance(Duration::from_secs(29)).await;
    tokio::task::yield_now().await;
    assert!(receiver.try_recv().is_err());
    assert_eq!(handle.load().at.number, 100);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(diff(&mut receiver).await.to_block, 101);
    handle.shutdown().await;
    client.close().await;
}
#[tokio::test]
async fn cancellation_interrupts_an_unanswered_graph_read_and_preserves_the_snapshot() {
    let (chain, client) = setup().await;
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            cadence: SyncCadence::EveryBlocks(1),
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let before = chain.requests().len();
    chain.mutate_state(|state| state.queue_reply("state_call", Reply::silent()));
    chain.advance(1).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if chain
                .requests()
                .iter()
                .skip(before)
                .any(|request| request.method == "state_call")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), handle.shutdown())
        .await
        .unwrap();
    assert_eq!(handle.load().at.number, 100);
    assert!(handle.health().borrow().stopped);
    client.close().await;
}
#[tokio::test]
async fn epoch_read_failure_retains_bootstrap_health_and_still_uses_block_fallback() {
    let (chain, client) = setup().await;
    chain.mutate_state(|state| {
        state.queue_reply(
            "state_queryStorageAt",
            Reply::error(-32000, "epoch unavailable", None),
        )
    });
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            cadence: SyncCadence::EveryEpoch { fallback_blocks: 2 },
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(handle.health().borrow().consecutive_failures, 1);
    for _ in 0..2 {
        chain.mutate_state(|state| {
            state.queue_reply(
                "state_queryStorageAt",
                Reply::error(-32000, "epoch unavailable", None),
            )
        });
        chain.advance(1).unwrap();
        settle().await;
    }
    assert_eq!(diff(&mut receiver).await.to_block, 102);
    assert_eq!(handle.load().len(), 2);
    handle.shutdown().await;
    client.close().await;
}
#[tokio::test]
async fn immediate_last_handle_drop_marks_stopped_even_before_worker_first_poll() {
    let (_, client) = setup().await;
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::validator(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let mut health = handle.health();
    drop(handle);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !health.borrow().stopped {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    client.close().await;
}

#[tokio::test]
async fn duplicate_hotkeys_and_wrong_subnet_never_replace_the_good_snapshot() {
    let (chain, client) = setup().await;
    let (handle, mut receiver) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy {
            cadence: SyncCadence::EveryBlocks(1),
            ..SyncPolicy::validator()
        },
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let original = handle.load();
    let duplicate =
        rtao_core::codec::value::to_corpus_json(&value(&[11, 11], &[false, true], &[80, 81]));
    let mut wrong =
        rtao_core::codec::value::to_corpus_json(&value(&[11, 12], &[false, true], &[80, 81]));
    wrong["netuid"] = json!(3);
    for malformed in [duplicate, wrong] {
        chain.mutate_state(|state| {
            state
                .seed_runtime(
                    "SubnetInfoRuntimeApi",
                    "get_selective_mechagraph",
                    malformed,
                )
                .unwrap()
        });
        chain.advance(1).unwrap();
        settle().await;
        assert!(Arc::ptr_eq(&original, &handle.load()));
        assert!(receiver.try_recv().is_err());
    }
    assert_eq!(handle.health().borrow().consecutive_failures, 2);
    handle.shutdown().await;
    client.close().await;
}
#[tokio::test]
async fn shared_client_close_stops_the_worker_without_replacing_a_snapshot() {
    let (_, client) = setup().await;
    let (handle, _) = MetagraphSync::spawn(
        client.clone(),
        NetUid(1),
        MechId(0),
        SyncPolicy::validator(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let snapshot = handle.load();
    let mut health = handle.health();
    client.close().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !health.borrow().stopped {
            health.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(Arc::ptr_eq(&snapshot, &handle.load()));
    handle.shutdown().await;
}

#[test]
fn selective_fraction_guard_uses_its_actual_option_field_and_allows_unselected_prices() {
    let mut core = Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap();
    let method = core
        .apis
        .iter()
        .find(|api| api.name == "SubnetInfoRuntimeApi")
        .unwrap()
        .methods
        .iter()
        .find(|method| method.name == "get_selective_mechagraph")
        .unwrap();
    let output = method.output;
    let encode = |present| {
        let mut value =
            rtao_core::codec::value::to_corpus_json(&value(&[11, 12], &[false, true], &[80, 81]));
        if present {
            value["moving_price"] = json!({"bits":4321});
        }
        let value = rtao_testkit::fakes::fill_defaults(&core, output, value).unwrap();
        core.encode_spec(&rtao_core::runtime::TypeSpec::Id(output), &value)
            .unwrap()
    };
    let with_price = encode(true);
    let without_price = encode(false);
    let selective = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|last| last == "SelectiveMetagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(fields) = &selective.ty.type_def else {
        panic!("selective composite")
    };
    let option = fields
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some("moving_price"))
        .unwrap()
        .ty
        .id;
    let scale_info::TypeDef::Variant(variants) = &core.types.resolve(option).unwrap().type_def
    else {
        panic!("price option")
    };
    let fixed = variants
        .variants
        .iter()
        .find(|variant| variant.name == "Some")
        .unwrap()
        .fields[0]
        .ty
        .id;
    let original = core.types.resolve(fixed).unwrap().type_params[0]
        .ty
        .unwrap()
        .id;
    let different = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty.path.segments == ["substrate_fixed", "FixedI128"]
                && ty.ty.type_params[0].ty.unwrap().id != original
        })
        .unwrap()
        .ty
        .type_params[0]
        .ty
        .unwrap();
    core.types
        .types
        .iter_mut()
        .find(|ty| ty.id == fixed)
        .unwrap()
        .ty
        .type_params[0]
        .ty = Some(different);
    let runtime = rtao_runtime::RuntimeHandle::new_with_api_versions(
        core,
        std::collections::BTreeMap::from([("SubnetInfoRuntimeApi".into(), 2)]),
    )
    .unwrap();
    for decoder in [
        DecodePreference::Auto,
        DecodePreference::Dynamic,
        DecodePreference::Typed,
    ] {
        let result = Metagraph::decode_selective(
            bytes::Bytes::from(with_price.clone()),
            &runtime,
            NetUid(1),
            MechId(0),
            SnapshotInfo::default(),
            MetagraphOptions {
                decoder,
                commitments: false,
                ..MetagraphOptions::default()
            },
        );
        assert!(
            result.is_err(),
            "decoder {decoder:?} accepted changed Frac: {:?}",
            result
                .unwrap()
                .map(|graph| graph.to_json().unwrap()["raw"]["moving_price"].clone())
        );
    }
    let graph = Metagraph::decode_selective(
        bytes::Bytes::from(without_price),
        &runtime,
        NetUid(1),
        MechId(0),
        SnapshotInfo::default(),
        MetagraphOptions {
            commitments: false,
            ..MetagraphOptions::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Dynamic);
    assert_eq!(graph.len(), 2);
    assert!(graph.to_json().unwrap()["raw"]["moving_price"].is_null());
}
