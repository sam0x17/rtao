//! M2 reads through actual metadata, SCALE state and the public dispatcher.
#![allow(clippy::unwrap_used, clippy::panic)] // Invalid pinned fixtures must fail the assertions.

use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind, View};
use rtao_primitives::{AccountId, MechId, NetUid, Uid};
use rtao_reads::{Namespaces, ReadError, ReadName, dispatch, fill_defaults, reads::m2};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, RecordingView};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn config() -> ClientConfig {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config
}
async fn client(state: FakeState) -> (Client, FakeChain) {
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    (client, chain)
}
fn seeded() -> FakeState {
    let mut state = FakeState::default();
    for (name, value) in [
        ("Tempo", json!(7)),
        ("LastEpochBlock", json!(95)),
        ("BlocksSinceLastStep", json!(5)),
        ("PendingEpochAt", json!(103)),
        ("SubnetEpochIndex", json!(8)),
        ("MaxWeightsLimit", json!(32768)),
        ("MinAllowedWeights", json!(2)),
        ("RevealPeriodEpochs", json!(3)),
        ("WeightsSetRateLimit", json!(11)),
        ("CommitRevealWeightsEnabled", json!(true)),
        ("MechanismCountCurrent", json!(16)),
        ("LastUpdate", json!([91, 105])),
    ] {
        state
            .seed_python("SubtensorModule", name, &[json!(1)], value)
            .unwrap();
    }
    state
        .seed_python("Timestamp", "Now", &[], json!(1_700_000_000_123u64))
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "Uids",
            &[json!(1), json!(AccountId([7; 32]).to_ss58(42))],
            json!(9),
        )
        .unwrap();
    state
        .seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_next_epoch_start_block",
            json!(104),
        )
        .unwrap();
    state
        .seed_runtime("SubnetInfoRuntimeApi", "get_metagraph", json!(null))
        .unwrap();
    state
}
fn params(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn all_read_names_have_authenticated_defaults_and_parameters() {
    assert_eq!(ReadName::ALL.len(), 102);
    let names: BTreeSet<_> = ReadName::ALL.iter().map(|name| name.as_str()).collect();
    assert_eq!(names.len(), 102);
    for name in ReadName::ALL {
        assert_eq!(name.as_str().parse::<ReadName>().unwrap(), name);
        let spec = name.catalog_spec();
        assert_eq!(spec.name, name.as_str());
        let contract = name.spec();
        assert_eq!(contract.name, spec.name);
        assert_eq!(contract.namespace, spec.namespace);
        assert_eq!(contract.category.as_str(), spec.category);
        assert_eq!(contract.params.len(), spec.params.len());
        let catalog: Value = serde_json::from_str(spec.catalog_json).unwrap();
        for hit in catalog["hits"].as_array().into_iter().flatten() {
            let matches_raw = |actual: &rtao_reads::Hit| {
                let kind = match actual.kind {
                    rtao_reads::HitKind::Storage => "storage",
                    rtao_reads::HitKind::RuntimeApi => "runtime",
                    rtao_reads::HitKind::Constant => "constant",
                };
                hit["kind"] == kind
                    && hit["container"] == actual.container
                    && hit["name"] == actual.name
            };
            let witnesses: Vec<_> = rtao_reads::READ_HIT_REPLACEMENTS
                .iter()
                .filter(|replacement| {
                    replacement.read == spec.name && matches_raw(&replacement.from)
                })
                .collect();
            if witnesses.is_empty() {
                assert!(
                    contract.hits.iter().any(matches_raw),
                    "{}: missing raw source hit {hit}",
                    spec.name
                );
            } else {
                assert_eq!(
                    witnesses.len(),
                    1,
                    "{}: duplicate source replacement witness",
                    spec.name
                );
                let replacement = witnesses[0];
                assert!(!replacement.divergence.is_empty());
                assert!(!contract.hits.contains(&replacement.from));
                assert!(contract.hits.contains(&replacement.to));
            }
        }
        let defaults = fill_defaults(name, &Map::new()).unwrap();
        for field in spec.params {
            assert_eq!(
                defaults.get(field.name),
                field
                    .default_json
                    .map(|value| serde_json::from_str::<Value>(value).unwrap())
                    .as_ref()
            );
        }
    }
    assert_eq!(
        fill_defaults(ReadName::Weights, &params(json!({"netuid":1}))).unwrap(),
        params(json!({"netuid":1,"mechid":0}))
    );
    assert_eq!(
        fill_defaults(
            ReadName::Neurons,
            &params(json!({"netuid":1,"lite":false,"mechid":null}))
        )
        .unwrap()["lite"],
        json!(false)
    );
    assert_eq!(
        fill_defaults(
            ReadName::Neurons,
            &params(json!({"netuid":1,"lite":false,"mechid":null}))
        )
        .unwrap()["mechid"],
        json!(null)
    );
}

#[tokio::test]
async fn every_ported_read_has_exact_hits_and_one_state_hash() {
    let (client, chain) = client(seeded()).await;
    let samples = [
        (ReadName::BlockTime, json!({})),
        (ReadName::Timestamp, json!({})),
        (ReadName::BlocksSinceLastStep, json!({"netuid":1})),
        (ReadName::BlocksSinceLastUpdate, json!({"netuid":1,"uid":0})),
        (ReadName::EpochStatus, json!({"netuid":1})),
        (ReadName::MaxWeightLimit, json!({"netuid":1})),
        (ReadName::MinAllowedWeights, json!({"netuid":1})),
        (ReadName::RevealPeriod, json!({"netuid":1})),
        (ReadName::WeightsRateLimit, json!({"netuid":1})),
        (
            ReadName::Uid,
            json!({"netuid":1,"hotkey_ss58":AccountId([7;32]).to_ss58(42)}),
        ),
        (ReadName::CommitRevealEnabled, json!({"netuid":1})),
        (ReadName::MechanismCount, json!({"netuid":1})),
        (ReadName::Weights, json!({"netuid":1})),
        (ReadName::Metagraph, json!({"netuid":1})),
        (ReadName::TokenSymbols, json!({})),
    ];
    for (name, args) in samples {
        let mut union = BTreeSet::new();
        for block in [100, 99] {
            let snapshot = client.at(BlockRef::Number(block)).await.unwrap();
            let hash = snapshot.block().hash;
            let view = RecordingView::new(snapshot);
            let before = chain.requests().len();
            dispatch(&view, name, &params(args.clone())).await.unwrap();
            let touches = view.touches();
            for touch in &touches {
                assert_eq!(touch.at, Some(hash), "{name}");
                assert!(
                    name.spec().hits.iter().any(|hit| hit.matches(touch)),
                    "{name}: unexpected {touch:?}"
                );
                union.insert((touch.kind, touch.pallet_or_api.clone(), touch.item.clone()));
            }
            assert_eq!(view.block_hashes().len(), 1, "{name}");
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
                        "{name} {request:?}"
                    );
                }
            }
        }
        assert_eq!(
            union.len(),
            name.spec().hits.len(),
            "{name}: sample union must equal H"
        );
    }
    client.close().await;
}

#[tokio::test]
async fn pending_reads_are_explicit_and_do_no_io() {
    let (client, chain) = client(seeded()).await;
    let pending: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/read_params.json"
    ))
    .unwrap();
    let all: BTreeSet<_> = ReadName::ALL.into_iter().collect();
    assert_eq!(all.len(), 102);
    let expected_pending: BTreeSet<_> = all
        .iter()
        .copied()
        .filter(|name| !name.interface_metadata().implemented)
        .collect();
    let before = chain.requests().len();
    let mut seen = BTreeSet::new();
    let mut checked = BTreeSet::new();
    for row in pending["reads"].as_array().unwrap() {
        let name: ReadName = row["name"].as_str().unwrap().parse().unwrap();
        assert!(seen.insert(name), "duplicate extracted read {name}");
        if name.interface_metadata().implemented {
            continue;
        }
        let mut args = Map::new();
        for field in name.catalog_spec().params {
            if field.default_json.is_some() {
                continue;
            }
            let value = if field.name.ends_with("_ss58") {
                json!(AccountId([7; 32]).to_ss58(42))
            } else if field.name.ends_with("_ss58s") {
                json!([AccountId([7; 32]).to_ss58(42)])
            } else {
                match field.json_type.unwrap() {
                    "integer" => json!(1),
                    "number" => json!(1.0),
                    "boolean" => json!(false),
                    "string" => json!("value"),
                    "array" => json!([]),
                    kind => panic!("unknown parameter kind {kind}"),
                }
            };
            args.insert(field.name.into(), value);
        }
        assert!(
            matches!(dispatch(&client,name,&args).await,Err(ReadError::NotYetPorted(actual))if actual==name),
            "{name}"
        );
        assert!(checked.insert(name));
    }
    assert_eq!(seen, all, "every catalog read must be considered");
    assert_eq!(checked, expected_pending, "every pending read must refuse");
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn thirteen_reads_preserve_python_outputs_and_float_tokens() {
    let (client, _) = client(seeded()).await;
    let samples = [
        (ReadName::BlockTime, json!({}), json!(12.0)),
        (
            ReadName::Timestamp,
            json!({}),
            json!({"block":100,"unix":1_700_000_000.123f64,"iso":"2023-11-14T22:13:20.123000+00:00"}),
        ),
        (ReadName::BlocksSinceLastStep, json!({"netuid":1}), json!(5)),
        (
            ReadName::BlocksSinceLastUpdate,
            json!({"netuid":1,"uid":1}),
            json!(-5),
        ),
        (
            ReadName::EpochStatus,
            json!({"netuid":1}),
            json!({"netuid":1,"block":100,"tempo":7,"epoch_index":8,"last_epoch_block":95,"blocks_since_last_step":5,"pending_epoch_at":103,"next_epoch_start_block":104,"blocks_remaining":4,"seconds_remaining":48.0}),
        ),
        (ReadName::MaxWeightLimit, json!({"netuid":1}), json!(32768)),
        (ReadName::MinAllowedWeights, json!({"netuid":1}), json!(2)),
        (ReadName::RevealPeriod, json!({"netuid":1}), json!(3)),
        (ReadName::WeightsRateLimit, json!({"netuid":1}), json!(11)),
        (
            ReadName::Uid,
            json!({"netuid":1,"hotkey_ss58":AccountId([7;32]).to_ss58(42)}),
            json!(9),
        ),
        (
            ReadName::CommitRevealEnabled,
            json!({"netuid":1}),
            json!(true),
        ),
        (ReadName::MechanismCount, json!({"netuid":1}), json!(16)),
        (ReadName::Weights, json!({"netuid":1}), json!({})),
    ];
    for (name, args, expected) in samples {
        let actual = dispatch(&client, name, &params(args)).await.unwrap();
        rtao_primitives::json::compare(&expected, &actual).unwrap();
    }
    assert_eq!(
        client
            .hyperparameters()
            .max_weight_limit(NetUid(1))
            .await
            .unwrap(),
        32768
    );
    assert_eq!(
        client
            .epochs()
            .blocks_since_last_step(NetUid(1))
            .await
            .unwrap(),
        5
    );
    assert!(matches!(
        dispatch(
            &client,
            ReadName::Weights,
            &params(json!({"netuid":1,"unexpected":2}))
        )
        .await,
        Err(ReadError::InvalidParam { .. })
    ));
    assert!(matches!(
        dispatch(&client, ReadName::MinAllowedWeights, &Map::new()).await,
        Err(ReadError::InvalidParam { .. })
    ));
    client.close().await;
}

#[tokio::test]
async fn last_update_handles_absence_negative_indices_and_future_values() {
    let (client, chain) = client(seeded()).await;
    assert_eq!(
        m2::blocks_since_last_update(&client, NetUid(1), Uid(1))
            .await
            .unwrap(),
        Some(-5)
    );
    assert_eq!(
        m2::blocks_since_last_update_index(&client, NetUid(1), -1)
            .await
            .unwrap(),
        Some(-5)
    );
    assert_eq!(
        m2::blocks_since_last_update_index(&client, NetUid(1), -2)
            .await
            .unwrap(),
        Some(9)
    );
    assert_eq!(
        m2::blocks_since_last_update(&client, NetUid(1), Uid(2))
            .await
            .unwrap(),
        None
    );
    assert!(matches!(
        m2::blocks_since_last_update_index(&client, NetUid(1), -3).await,
        Err(ReadError::InvalidParam { .. })
    ));
    let huge = serde_json::from_str::<Value>(
        "{\"netuid\":1,\"uid\":1606938044258990275541962092341162602522202993782792835301376}",
    )
    .unwrap();
    assert_eq!(
        dispatch(&client, ReadName::BlocksSinceLastUpdate, &params(huge))
            .await
            .unwrap(),
        Value::Null
    );
    let huge_negative = serde_json::from_str::<Value>(
        "{\"netuid\":1,\"uid\":-1606938044258990275541962092341162602522202993782792835301376}",
    )
    .unwrap();
    assert!(matches!(
        dispatch(
            &client,
            ReadName::BlocksSinceLastUpdate,
            &params(huge_negative)
        )
        .await,
        Err(ReadError::InvalidParam { .. })
    ));
    assert_eq!(
        client
            .epochs()
            .blocks_since_last_update(NetUid(1), Uid(0))
            .await
            .unwrap(),
        Some(9)
    );
    chain.mutate_state(|state| {
        state
            .seed_python("SubtensorModule", "LastUpdate", &[json!(1)], json!([]))
            .unwrap()
    });
    assert_eq!(
        m2::blocks_since_last_update_index(&client, NetUid(1), i64::MIN)
            .await
            .unwrap(),
        None
    );
    client.close().await;
}

#[tokio::test]
async fn sparse_weights_normalize_complete_duplicate_rows_and_keep_zero_rows() {
    let mut state = seeded();
    state
        .seed_python(
            "SubtensorModule",
            "Weights",
            &[json!(4097), json!(9)],
            json!([[1, 1], [1, 2], [2, 3]]),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "Weights",
            &[json!(4097), json!(2)],
            json!([[7, 0]]),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "Weights",
            &[json!(1), json!(8)],
            json!([[5, 99]]),
        )
        .unwrap();
    let (client, _) = client(state).await;
    let matrix = m2::weights(&client, NetUid(1), MechId(1)).await.unwrap();
    assert_eq!(
        matrix.keys().copied().collect::<Vec<_>>(),
        vec![Uid(2), Uid(9)]
    );
    assert!(matrix[&Uid(2)].is_empty());
    assert_eq!(matrix[&Uid(9)][&Uid(1)], 1.0 / 3.0);
    assert_eq!(matrix[&Uid(9)][&Uid(2)], 0.5);
    assert!(m2::weights(&client, NetUid(4096), MechId(0)).await.is_err());
    assert!(m2::weights(&client, NetUid(1), MechId(16)).await.is_err());
    client.close().await;
}

#[tokio::test]
async fn epoch_status_uses_one_snapshot_and_clamps_or_omits_remaining_time() {
    let (client, chain) = client(seeded()).await;
    let snapshot = client.at(BlockRef::Number(100)).await.unwrap();
    let view = RecordingView::new(snapshot);
    chain.advance(3).unwrap();
    chain.mutate_state(|state| {
        state
            .seed_python("SubtensorModule", "Tempo", &[json!(1)], json!(99))
            .unwrap()
    });
    let status = m2::epoch_status(&view, NetUid(1)).await.unwrap();
    assert_eq!(status.block, 100);
    assert_eq!(status.tempo, 7);
    assert_eq!(view.block_hashes().len(), 1);
    let touches = view.touches();
    assert_eq!(
        touches
            .iter()
            .map(|touch| (
                touch.kind,
                touch.pallet_or_api.as_str(),
                touch.item.as_str()
            ))
            .collect::<BTreeSet<_>>()
            .len(),
        7
    );
    assert!(touches.iter().any(|touch| touch.kind == ReadKind::Constant
        && touch.item == "SlotDuration"
        && touch.at.is_some()));
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_next_epoch_start_block",
                json!(100),
            )
            .unwrap();
        state
            .seed_python("SubtensorModule", "PendingEpochAt", &[json!(1)], json!(0))
            .unwrap();
    });
    let overdue = m2::epoch_status(&client, NetUid(1)).await.unwrap();
    assert_eq!(overdue.blocks_remaining, Some(0));
    assert_eq!(overdue.seconds_remaining, Some(0.0));
    assert_eq!(overdue.pending_epoch_at, None);
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_next_epoch_start_block",
                json!(null),
            )
            .unwrap()
    });
    let disabled = m2::epoch_status(&client, NetUid(1)).await.unwrap();
    assert_eq!(disabled.blocks_remaining, None);
    assert_eq!(disabled.seconds_remaining, None);
    client.close().await;
}

#[tokio::test]
async fn recording_covers_empty_maps_snapshot_clones_constants_and_raw_keys() {
    let (client, _) = client(seeded()).await;
    let view = RecordingView::new(client.at(BlockRef::Number(100)).await.unwrap());
    assert!(
        m2::weights(&view, NetUid(42), MechId(0))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(view.touches().iter().any(|touch| touch.item == "Weights"));
    let snapshot = view.pin().await.unwrap();
    let clone = snapshot.clone();
    m2::timestamp(&clone).await.unwrap();
    m2::block_time(&clone).await.unwrap();
    let runtime = clone.block().runtime.core();
    let info = runtime.storage_entry("SubtensorModule", "Tempo").unwrap();
    let key = runtime
        .storage_key(info, &[rtao_core::codec::Value::Uint(1)])
        .unwrap();
    clone.storage_raw(&key).await.unwrap();
    assert!(view.touches().iter().any(|touch| touch.item == "Now"));
    assert!(
        view.touches()
            .iter()
            .any(|touch| touch.item == "SlotDuration")
    );
    assert!(view.touches().iter().any(|touch| touch.item == "Tempo"));
    assert_eq!(view.block_hashes().len(), 1);
    view.clear();
    assert!(view.touches().is_empty());
    client.close().await;
}

#[tokio::test]
async fn fast_blocks_and_whole_second_timestamps_keep_python_json() {
    let mut state = seeded();
    state
        .seed_constant("Aura", "SlotDuration", json!(250))
        .unwrap();
    state
        .seed_python("Timestamp", "Now", &[], json!(1_700_000_000_000u64))
        .unwrap();
    let (client, _) = client(state).await;
    assert_eq!(m2::block_time(&client).await.unwrap(), 0.25);
    let value = m2::timestamp(&client).await.unwrap();
    assert_eq!(value.iso, "2023-11-14T22:13:20+00:00");
    let serialized = serde_json::to_string(&value).unwrap();
    assert!(serialized.contains("1700000000.0"));
    client.close().await;
}

#[tokio::test]
async fn block_duration_uses_pythons_float_divisor_after_integer_conversion() {
    let mut state = seeded();
    state
        .seed_constant("Aura", "SlotDuration", json!(9_007_199_254_740_995u64))
        .unwrap();
    let (client, _) = client(state).await;
    let expected = 9_007_199_254_740.996f64;
    assert_eq!(m2::block_time(&client).await.unwrap(), expected);
    assert_eq!(
        m2::epoch_status(&client, NetUid(1))
            .await
            .unwrap()
            .seconds_remaining,
        Some(4.0 * expected)
    );
    client.close().await;
}

#[tokio::test]
async fn distant_timestamp_uses_cpython_float_to_microsecond_rounding() {
    let mut state = seeded();
    state
        .seed_python("Timestamp", "Now", &[], json!(253_402_300_799_999u64))
        .unwrap();
    let (client, chain) = client(state).await;
    let timestamp = m2::timestamp(&client).await.unwrap();
    assert_eq!(timestamp.iso, "9999-12-31T23:59:59.998993+00:00");
    chain.mutate_state(|state| {
        state
            .seed_python("Timestamp", "Now", &[], json!(253_402_300_800_000u64))
            .unwrap()
    });
    assert!(m2::timestamp(&client).await.is_err());
    client.close().await;
}
