// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public commitment scans exercise live SCALE keys, values and immutable snapshots.

#![allow(clippy::unwrap_used)] // Assertions must fail when fake-chain fixtures or responses are invalid.

use rtao_client::{BlockRef, Client, ClientConfig, Network, View};
use rtao_primitives::{AccountId, NetUid, Uid};
use rtao_reads::{
    Namespaces,
    commitments::{CommitmentStatus, fetch_commitment_map, fetch_commitments},
};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use serde_json::json;

fn config() -> ClientConfig {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config
}

fn seed() -> (FakeChain, AccountId, AccountId) {
    let mut state = FakeState::default();
    let alice = AccountId([1; 32]);
    let bob = AccountId([2; 32]);
    state
        .seed_python(
            "Commitments",
            "CommitmentOf",
            &[json!(42), json!(alice.to_string())],
            json!({
                "block": 0, "deposit": 7, "info": {"fields": [{"Raw5": "0x706c61696e"}]},
            }),
        )
        .unwrap();
    state
        .seed_python(
            "Commitments",
            "RevealedCommitments",
            &[json!(42), json!(bob.to_string())],
            json!([["0x1064617461", 0]]),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "Uids",
            &[json!(42), json!(alice.to_string())],
            json!(7),
        )
        .unwrap();
    (FakeChain::from_state(state), alice, bob)
}

#[tokio::test]
async fn live_and_revealed_only_records_use_core_accounts_and_same_hash() {
    let (chain, alice, bob) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let start = chain.requests().len();
    let records = client.subnets().commitments(NetUid(42)).await.unwrap();
    assert_eq!(records.len(), 2);
    let committed = records.get(&alice.to_string()).unwrap();
    assert_eq!(committed.value(), Some("plain"));
    assert_eq!(committed.uid, Some(Uid(7)));
    assert_eq!(committed.deposit.rao(), 7);
    assert_eq!(committed.status(), CommitmentStatus::Plain);
    let revealed = records.get(&bob.to_string()).unwrap();
    assert_eq!(revealed.value(), Some("data"));
    assert_eq!(revealed.uid, None);
    assert_eq!(revealed.status(), CommitmentStatus::Revealed);
    let state_requests = chain
        .requests()
        .into_iter()
        .skip(start)
        .filter(|request| {
            matches!(
                request.method.as_str(),
                "state_getKeysPaged" | "state_queryStorageAt"
            )
        })
        .collect::<Vec<_>>();
    assert!(!state_requests.is_empty());
    let hashes = state_requests
        .iter()
        .map(|request| {
            request
                .params
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .as_str()
                .unwrap()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(hashes.len(), 1);
    assert!(
        !chain
            .requests()
            .iter()
            .skip(start)
            .any(|request| request.method == "state_call")
    );
    client.close().await;
}

#[tokio::test]
async fn existing_snapshot_retains_its_block_after_head_advances() {
    let (chain, alice, _) = seed();
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config())
        .await
        .unwrap();
    let snapshot = client.at(BlockRef::Best).await.unwrap();
    let hash = format!("0x{}", hex::encode(snapshot.block().hash.0));
    chain.advance(2).unwrap();
    let start = chain.requests().len();
    let records = fetch_commitments(&snapshot, NetUid(42)).await.unwrap();
    assert_eq!(
        records.get(&alice.to_string()).unwrap().queried_block,
        snapshot.block().number
    );
    for request in chain.requests().iter().skip(start).filter(|request| {
        matches!(
            request.method.as_str(),
            "state_getKeysPaged" | "state_queryStorageAt"
        )
    }) {
        assert_eq!(
            request.params.as_array().unwrap().last().unwrap(),
            &json!(hash)
        );
    }
    let no_uids = fetch_commitment_map(&snapshot, NetUid(42), false)
        .await
        .unwrap();
    assert!(no_uids.values().all(|record| record.uid.is_none()));
    assert_eq!(View::pinned(&snapshot).unwrap().hash, snapshot.block().hash);
    client.close().await;
}
