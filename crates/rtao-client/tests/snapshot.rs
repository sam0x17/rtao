// Derived from RaoFoundation/subtensor sdk/python/bittensor/snapshot.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Snapshot siblings and connection-local display helpers.

#![allow(clippy::unwrap_used, clippy::panic)] // Invalid fixtures must fail assertions.

use rtao_client::{BlockRef, Client, ClientConfig, ClientError, Network, View, ViewExt};
use rtao_primitives::{DynBalance, NetUid};
use rtao_testkit::{FakeChain, FakeState, RecordingView, rpc::RpcClient};
use serde_json::json;

async fn setup() -> (Client, FakeChain) {
    let mut state = FakeState::default();
    state
        .seed_typed(
            "SubtensorModule",
            "TokenSymbol",
            (NetUid::ROOT,),
            b"ignored".to_vec(),
        )
        .unwrap();
    state
        .seed_typed(
            "SubtensorModule",
            "TokenSymbol",
            (NetUid(3),),
            "λ".as_bytes().to_vec(),
        )
        .unwrap();
    state
        .seed_python("Timestamp", "Now", &[], json!(1234))
        .unwrap();
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

// pyt: bittensor/snapshot.py::Snapshot.at preserves self for an omitted or
// equal height, and creates a sibling for another height. Native views retain
// the resolved hash and carry their descriptor observer into that sibling.
#[tokio::test]
async fn snapshot_siblings_retain_observers_and_equal_references_need_no_rpc() {
    let (client, chain) = setup().await;
    let recording = RecordingView::new(client.at(BlockRef::Best).await.unwrap());
    let snapshot = recording.pin().await.unwrap();
    let hash = snapshot.block().hash;
    let before = chain.requests();
    for reference in [
        None,
        Some(BlockRef::Number(100)),
        Some(BlockRef::Hash(hash)),
    ] {
        assert_eq!(snapshot.at(reference).await.unwrap().block().hash, hash);
    }
    assert_eq!(chain.requests(), before);

    let sibling = snapshot.at(Some(BlockRef::Number(99))).await.unwrap();
    assert_eq!(sibling.block().number, 99);
    assert_ne!(sibling.block().hash, hash);
    recording.clear();
    sibling.timestamp().await.unwrap();
    let touches = recording.touches();
    assert_eq!(touches.len(), 1);
    assert_eq!(touches[0].pallet_or_api, "Timestamp");
    assert_eq!(touches[0].item, "Now");
    assert_eq!(touches[0].at, Some(sibling.block().hash));

    client.close().await;
    let before = chain.requests();
    for reference in [
        None,
        Some(BlockRef::Number(100)),
        Some(BlockRef::Hash(hash)),
    ] {
        assert_eq!(snapshot.at(reference).await.unwrap().block().hash, hash);
    }
    assert!(matches!(
        snapshot.at(Some(BlockRef::Number(99))).await,
        Err(ClientError::Rpc(rtao_rpc::RpcError::Closed))
    ));
    assert_eq!(chain.requests(), before);
}

// pyt: bittensor/_substrate.py::Substrate.balance and
// bittensor/snapshot.py::Snapshot.balance/token_symbols. Amounts use the
// approved native u64 contract; display metadata remains connection-scoped.
#[tokio::test]
async fn client_and_snapshot_balance_helpers_keep_symbols_without_rpc_after_close() {
    let (client, chain) = setup().await;
    let snapshot = client.at(BlockRef::Best).await.unwrap();
    let expected_symbols = client.token_symbols().await;
    assert_eq!(
        expected_symbols.get(&NetUid(3)).map(String::as_str),
        Some("λ")
    );
    assert_eq!(
        expected_symbols.get(&NetUid::ROOT).map(String::as_str),
        Some("ignored")
    );
    assert!(!expected_symbols.contains_key(&NetUid(42)));
    let before = chain.requests();
    for closed in [false, true] {
        if closed {
            client.close().await;
        }
        assert_eq!(snapshot.token_symbols().await, expected_symbols);
        let mut copied = snapshot.token_symbols().await;
        copied.clear();
        assert_eq!(client.token_symbols().await, expected_symbols);
        for value in [
            client.balance(1_000_000_001, NetUid(3)).await,
            snapshot.balance(1_000_000_001, NetUid(3)).await,
        ] {
            assert_eq!(value.rao, 1_000_000_001);
            assert_eq!(value.netuid, NetUid(3));
            assert_eq!(value.to_string(), "1.000000001λ");
        }
        for value in [
            client.balance(u64::MAX, NetUid(3)).await,
            snapshot.balance(u64::MAX, NetUid(3)).await,
        ] {
            assert_eq!(value.rao, u64::MAX);
            assert_eq!(value.netuid, NetUid(3));
        }
        for netuid in [NetUid::ROOT, NetUid(42)] {
            let expected = DynBalance::from_rao(1_000_000_001, netuid);
            for value in [
                client.balance(1_000_000_001, netuid).await,
                snapshot.balance(1_000_000_001, netuid).await,
            ] {
                assert!(value.symbol.is_none());
                assert_eq!(value.to_string(), expected.to_string());
            }
        }
        assert_eq!(chain.requests(), before);
    }
}
