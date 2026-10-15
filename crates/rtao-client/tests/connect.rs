//! Client connection preconditions and the lifetime of its background workers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{BlockRef, Client, ClientConfig, Network, View};
use rtao_testkit::fakes::{FakeChain, FakeNode, FakeState, Reply};
use serde_json::json;
use subxt_rpcs::RpcClient;

fn config(input: &str) -> ClientConfig {
    let mut cfg = ClientConfig::new(Network::resolve_with(input, |_| None).unwrap()).unwrap();
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    cfg.session.rate_limit = None;
    cfg
}

#[tokio::test]
async fn last_client_drop_closes_socket_and_background_subscriptions() {
    let mut node = FakeNode::start().await.unwrap();
    let client = Client::connect(config(node.url())).await.unwrap();
    let surviving_clone = client.clone();
    drop(client);
    assert!(
        surviving_clone
            .request::<serde_json::Value>("system_health", json!([]))
            .await
            .is_ok()
    );
    drop(surviving_clone);
    tokio::time::timeout(std::time::Duration::from_secs(1), node.wait_for_closed(1))
        .await
        .unwrap();
    node.shutdown().await;
}

#[tokio::test]
async fn genesis_mismatch_keeps_reads_and_refuses_signing_with_both_hashes() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| state.genesis_hash = format!("0x{}", hex::encode([9; 32])));
    let mut cfg = config("finney");
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(raw), cfg).await.unwrap();
    let error = client.ensure_signing().await.unwrap_err().to_string();
    assert!(error.contains("2f0555cc76fc2840"));
    assert!(error.contains("0909090909090909"));
    assert!(View::pin(&client).await.is_ok());
}

#[tokio::test]
async fn bad_ss58_and_missing_transaction_version_fail_at_connect() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| {
        state.queue_reply("system_properties", Reply::result(json!({"ss58Format":0})))
    });
    let mut cfg = config("local");
    cfg.track_heads = false;
    let error = Client::from_rpc(RpcClient::new(raw), cfg.clone())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ss58 format must be 42"));
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| {
        state.queue_reply(
            "state_getRuntimeVersion",
            Reply::result(json!({"specVersion":478,"specName":"node-subtensor"})),
        )
    });
    let error = Client::from_rpc(RpcClient::new(raw), cfg)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "state_getRuntimeVersion returned no transactionVersion; cannot build a codec that signs correctly"
    );
}

#[tokio::test]
async fn complete_startup_deadline_closes_unfinished_socket() {
    let mut node = FakeNode::start().await.unwrap();
    node.silence(true);
    let mut cfg = config(node.url());
    cfg.startup_timeout = std::time::Duration::from_millis(50);
    let connect = tokio::spawn(Client::connect(cfg));
    let error = connect.await.unwrap().unwrap_err().to_string();
    assert_eq!(error, "the connection timed out without a response");
    if node.connect_attempts() > 0 {
        tokio::time::timeout(std::time::Duration::from_secs(1), node.wait_for_closed(1))
            .await
            .unwrap();
    }
    node.shutdown().await;
}

#[tokio::test]
async fn number_cache_retains_only_finalized_hashes() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| state.finalized = 99);
    let mut cfg = config("local");
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(raw.clone()), cfg)
        .await
        .unwrap();
    let finalized = client.at(BlockRef::Number(99)).await.unwrap().block().hash;
    client.at(BlockRef::Number(99)).await.unwrap();
    let requests = raw.requests();
    assert_eq!(
        requests
            .iter()
            .filter(
                |request| request.method == "chain_getBlockHash" && request.params == json!([99])
            )
            .count(),
        1
    );
    let best = client.at(BlockRef::Number(100)).await.unwrap().block().hash;
    raw.mutate_state(|state| {
        let parent = state.block_hash(99).unwrap();
        let fork = state.fork(&parent).unwrap();
        state.set_best(&fork).unwrap();
    });
    let replacement = client.at(BlockRef::Number(100)).await.unwrap().block().hash;
    assert_ne!(best, replacement);
    assert_eq!(
        client.at(BlockRef::Number(99)).await.unwrap().block().hash,
        finalized
    );
}

#[tokio::test]
async fn connection_token_symbols_are_loaded_from_live_pinned_map() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.mutate_state(|state| {
        state
            .seed_typed(
                "SubtensorModule",
                "TokenSymbol",
                (rtao_primitives::NetUid(1),),
                b"SUB".to_vec(),
            )
            .unwrap()
    });
    let mut cfg = config("local");
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(raw), cfg).await.unwrap();
    assert_eq!(
        client
            .token_symbols()
            .await
            .get(&rtao_primitives::NetUid(1))
            .map(String::as_str),
        Some("SUB")
    );
}

#[tokio::test]
async fn client_tracker_follows_fake_chain_best_and_finalized_identities() {
    use futures::StreamExt;
    let raw = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(raw.clone()), config("local"))
        .await
        .unwrap();
    let mut finalized = client.blocks(rtao_client::Finality::Finalized).unwrap();
    assert_eq!(finalized.next().await.unwrap().unwrap().number, 100);
    raw.advance(2).unwrap();
    for number in [101, 102] {
        let header = tokio::time::timeout(std::time::Duration::from_secs(1), finalized.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(header.number, number);
        assert_eq!(header.hash.0[31], number as u8);
    }
    assert_eq!(client.heads().unwrap().finalized().borrow().number, 102);
    client.close().await;
}

#[tokio::test]
async fn metagraph_view_opt_out_survives_clones_snapshots_and_dynamic_views() {
    let raw = FakeChain::from_state(FakeState::default());
    let mut cfg = config("local");
    assert!(cfg.metagraph_view);
    cfg.metagraph_view = false;
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(raw), cfg).await.unwrap();
    assert!(!client.metagraph_view_enabled());
    assert!(!View::metagraph_view_enabled(&client.clone()));
    let snapshot = View::pin(&client).await.unwrap();
    assert!(!View::metagraph_view_enabled(&snapshot));
    let dynamic: &dyn rtao_client::DynView = &snapshot;
    assert!(!rtao_client::DynView::metagraph_view_enabled(dynamic));
    assert!(!View::metagraph_view_enabled(dynamic));
    let repinned = View::pin(dynamic).await.unwrap();
    assert!(!View::metagraph_view_enabled(&repinned));
}
