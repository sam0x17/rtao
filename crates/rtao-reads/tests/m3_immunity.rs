//! Native immunity-period values through metadata-backed storage and read dispatch.
#![allow(clippy::unwrap_used)] // Invalid fixtures must fail the test assertions.

use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind};
use rtao_primitives::NetUid;
use rtao_reads::{
    Namespaces, ReadError, ReadName, dispatch, fill_defaults, reads::hyperparameters,
};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, RecordingView};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

async fn client(state: FakeState) -> (Client, FakeChain) {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (client, chain)
}

fn params(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn seed(state: &mut FakeState, netuid: u16, value: u16) {
    state
        .seed_python(
            "SubtensorModule",
            "ImmunityPeriod",
            &[json!(netuid)],
            json!(value),
        )
        .unwrap();
}

#[tokio::test]
async fn native_values_and_metadata_default_match_all_public_entry_points() {
    let mut state = FakeState::new(FakeState::default().runtime);
    seed(&mut state, 0, 0);
    seed(&mut state, u16::MAX, u16::MAX);
    let (client, _) = client(state).await;

    // Netuid 1 has no stored override, so the runtime's default applies.
    for (netuid, expected) in [(0, 0u16), (u16::MAX, u16::MAX), (1, 4096)] {
        let actual = dispatch(
            &client,
            ReadName::ImmunityPeriod,
            &params(json!({"netuid":netuid})),
        )
        .await
        .unwrap();
        assert_eq!(actual.as_u64(), Some(u64::from(expected)));
        assert!(!actual.is_f64());
        assert_eq!(
            hyperparameters::immunity_period(&client, NetUid(netuid))
                .await
                .unwrap(),
            expected
        );
        let typed: u16 = client
            .hyperparameters()
            .immunity_period(NetUid(netuid))
            .await
            .unwrap();
        assert_eq!(typed, expected);
    }
    client.close().await;
}

#[tokio::test]
async fn optional_signature_default_and_invalid_native_keys_do_no_io() {
    let (client, chain) = client(FakeState::default()).await;
    let name = ReadName::ImmunityPeriod;
    assert_eq!(
        fill_defaults(name, &Map::new()).unwrap(),
        params(json!({"netuid":null}))
    );
    let supplied = params(json!({"netuid":0}));
    assert_eq!(fill_defaults(name, &supplied).unwrap(), supplied);
    let before = chain.requests().len();
    for args in [
        json!({}),
        json!({"netuid":null}),
        json!({"netuid":-1}),
        json!({"netuid":65536}),
        json!({"netuid":1.5}),
        json!({"netuid":1,"unexpected":true}),
    ] {
        assert!(matches!(
            dispatch(&client, name, &params(args)).await,
            Err(ReadError::InvalidParam { .. })
        ));
    }
    assert_eq!(chain.requests().len(), before);
    client.close().await;
}

#[tokio::test]
async fn pinned_samples_keep_old_state_and_cover_exact_descriptor_hits() {
    let mut state = FakeState::default();
    seed(&mut state, 1, 12);
    let (client, chain) = client(state).await;
    let old = client.at(BlockRef::Number(100)).await.unwrap();
    chain.advance(1).unwrap();
    chain.mutate_state(|state| seed(state, 1, 33));
    let current = client.at(BlockRef::Number(101)).await.unwrap();
    assert_ne!(old.block().hash, current.block().hash);

    let name = ReadName::ImmunityPeriod;
    let mut union = BTreeSet::new();
    for (snapshot, expected) in [(old, 12), (current, 33)] {
        let hash = snapshot.block().hash;
        let view = RecordingView::new(snapshot);
        let before = chain.requests().len();
        assert_eq!(
            dispatch(&view, name, &params(json!({"netuid":1})))
                .await
                .unwrap(),
            json!(expected)
        );
        assert_eq!(view.block_hashes(), BTreeSet::from([hash]));
        let touches = view.touches();
        assert!(!touches.is_empty());
        for touch in touches {
            assert_eq!(touch.at, Some(hash));
            assert!(name.spec().hits.iter().any(|hit| hit.matches(&touch)));
            union.insert((touch.kind, touch.pallet_or_api, touch.item));
        }
        let requests = chain.requests();
        let storage: Vec<_> = requests[before..]
            .iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .collect();
        assert_eq!(storage.len(), 1);
        assert_eq!(
            storage[0].params[1],
            json!(format!("0x{}", hex::encode(hash.0)))
        );
    }
    let expected: BTreeSet<_> = name
        .spec()
        .hits
        .iter()
        .map(|hit| {
            let kind = match hit.kind {
                rtao_reads::HitKind::Storage => ReadKind::Storage,
                rtao_reads::HitKind::RuntimeApi => ReadKind::RuntimeApi,
                rtao_reads::HitKind::Constant => ReadKind::Constant,
            };
            (kind, hit.container.to_owned(), hit.name.to_owned())
        })
        .collect();
    assert_eq!(union, expected);
    assert_eq!(
        union,
        BTreeSet::from([(
            ReadKind::Storage,
            "SubtensorModule".to_owned(),
            "ImmunityPeriod".to_owned(),
        )])
    );
    client.close().await;
}
