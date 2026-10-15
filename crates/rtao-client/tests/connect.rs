//! Client connection preconditions and the lifetime of its background workers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rtao_client::{BlockRef, Client, ClientConfig, ClientError, Finality, Network, View};
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

fn assert_closed<T>(result: Result<T, ClientError>) {
    assert!(matches!(
        result,
        Err(ClientError::Rpc(rtao_rpc::RpcError::Closed))
    ));
}

#[tokio::test]
async fn closed_clones_refuse_all_snapshot_references_and_strict_subscriptions_without_rpc() {
    let raw = FakeChain::from_state(FakeState::default());
    let mut cfg = config("local");
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(raw.clone()), cfg)
        .await
        .unwrap();
    let hash = client.at(BlockRef::Number(99)).await.unwrap().block().hash;
    let clone = client.clone();
    client.close().await;
    let before = raw.requests();

    for reference in [
        BlockRef::Hash(hash),
        BlockRef::Number(99),
        BlockRef::Best,
        BlockRef::Finalized,
    ] {
        assert_closed(clone.at(reference).await);
    }
    assert_closed(View::pin(&clone).await);
    for finality in [Finality::Best, Finality::Finalized] {
        assert_closed(clone.blocks_strict(finality).await);
    }
    assert_eq!(raw.requests(), before);
}

#[tokio::test]
async fn close_cancels_pending_snapshot_headers_for_every_block_reference() {
    use std::time::Duration;

    for reference in [
        BlockRef::Hash(rtao_primitives::H256([7; 32])),
        BlockRef::Number(99),
        BlockRef::Best,
        BlockRef::Finalized,
    ] {
        let raw = FakeChain::from_state(FakeState::default());
        let mut cfg = config("local");
        cfg.track_heads = false;
        let client = Client::from_rpc(RpcClient::new(raw.clone()), cfg)
            .await
            .unwrap();
        raw.mutate_state(|state| state.queue_reply("chain_getHeader", Reply::silent()));
        let before = raw.requests().len();
        let pending_client = client.clone();
        let pending = tokio::spawn(async move { pending_client.at(reference).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !raw.requests()[before..]
                .iter()
                .any(|request| request.method == "chain_getHeader")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client.close().await;
        let after_close = raw.requests();
        assert_closed(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(raw.requests(), after_close);
    }
}

struct PendingHeadSubscription {
    chain: FakeChain,
    admitted: std::sync::Arc<tokio::sync::Notify>,
}

impl subxt_rpcs::RpcClientT for PendingHeadSubscription {
    fn request_raw<'a>(
        &'a self,
        method: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, Box<serde_json::value::RawValue>> {
        self.chain.request_raw(method, params)
    }

    fn subscribe_raw<'a>(
        &'a self,
        sub: &'a str,
        params: Option<Box<serde_json::value::RawValue>>,
        unsub: &'a str,
    ) -> subxt_rpcs::client::RawRpcFuture<'a, subxt_rpcs::client::RawRpcSubscription> {
        Box::pin(async move {
            let subscription = self.chain.subscribe_raw(sub, params, unsub).await?;
            self.admitted.notify_one();
            std::future::pending::<()>().await;
            Ok(subscription)
        })
    }
}

#[tokio::test]
async fn close_cancels_strict_subscription_admission_and_drops_its_open_transport_stream() {
    use std::{sync::Arc, time::Duration};

    let raw = FakeChain::from_state(FakeState::default());
    let admitted = Arc::new(tokio::sync::Notify::new());
    let transport = PendingHeadSubscription {
        chain: raw.clone(),
        admitted: admitted.clone(),
    };
    let mut cfg = config("local");
    cfg.track_heads = false;
    let client = Client::from_rpc(RpcClient::new(transport), cfg)
        .await
        .unwrap();
    let pending_client = client.clone();
    let pending = tokio::spawn(async move { pending_client.blocks_strict(Finality::Best).await });
    tokio::time::timeout(Duration::from_secs(1), admitted.notified())
        .await
        .unwrap();
    client.close().await;
    assert_closed(
        tokio::time::timeout(Duration::from_secs(1), pending)
            .await
            .unwrap()
            .unwrap(),
    );
    let requests = raw.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "chain_subscribeNewHeads")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "chain_unsubscribeNewHeads")
            .count(),
        1
    );
}

#[tokio::test]
async fn close_ends_open_strict_streams_and_unsubscribes_even_with_an_injected_transport() {
    use futures::StreamExt;
    use std::time::Duration;

    for (finality, unsubscribe) in [
        (Finality::Best, "chain_unsubscribeNewHeads"),
        (Finality::Finalized, "chain_unsubscribeFinalizedHeads"),
    ] {
        let raw = FakeChain::from_state(FakeState::default());
        let mut cfg = config("local");
        cfg.track_heads = false;
        let client = Client::from_rpc(RpcClient::new(raw.clone()), cfg)
            .await
            .unwrap();
        let mut stream = client.blocks_strict(finality).await.unwrap();
        client.close().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            raw.requests()
                .iter()
                .filter(|request| request.method == unsubscribe)
                .count(),
            1
        );
    }
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
