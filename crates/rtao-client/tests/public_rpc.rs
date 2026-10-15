// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The six client-boundary transport cases from the pinned Python source.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use rtao_client::{Client, ClientConfig, Finality, Network};
use rtao_primitives::H256;
use rtao_rpc::{RpcError, policy::classify_rpc_error};
use rtao_testkit::{
    Reply,
    fakes::{FakeChain, FakeState},
};
use serde_json::json;
use subxt_rpcs::RpcClient;

fn config() -> ClientConfig {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config
}

fn policy(message: &str, retry: &str) -> subxt_rpcs::Error {
    let refusal =
        classify_rpc_error(&json!({"code":-32005,"message":message,"data":{"retry_after":retry}}))
            .unwrap();
    subxt_rpcs::Error::Client(Box::new(RpcError::Policy(Box::new(refusal))))
}

#[tokio::test]
async fn rpc_substrate_exposes_public_policy_error() {
    let raw = FakeChain::from_state(FakeState::default());
    let message = "RPC endpoint rate limited this source";
    let data = json!({
        "retry_after": "60",
        "policy": "public\0gateway",
        "reason": "source\u{1b}quota",
        "gateway_ticket": {
            "opaque": [null, true, 18446744073709551616u128],
            "raw": "ticket\0\u{1b}bytes",
            "details": "x".repeat(600),
        },
    });
    raw.mutate_state(|state| {
        state.queue_reply(
            "chain_getBlockHash",
            Reply::error(-32005, message, Some(data.clone())),
        );
    });
    let error = Client::from_rpc(RpcClient::new(raw), config())
        .await
        .unwrap_err();
    let refusal = error.policy().unwrap();
    assert_eq!(refusal.retry_after.as_deref(), Some("60"));
    assert_eq!(
        refusal.retry_after_duration(),
        Some(std::time::Duration::from_secs(60))
    );
    assert_eq!(refusal.policy.as_deref(), Some("public gateway"));
    assert_eq!(refusal.reason.as_deref(), Some("source quota"));
    assert_eq!(
        refusal.payload.as_deref(),
        Some(&json!({
            "error": {"code": -32005, "message": message, "data": data},
        }))
    );
    let display = error.to_string();
    assert!(display.starts_with("RPC endpoint refused the request under its traffic policy: "));
    assert!(!display.contains(['\0', '\u{1b}']));
    assert!(!display.contains(&"x".repeat(600)));
}

#[tokio::test]
async fn rpc_substrate_exposes_public_blank_timeout_error() {
    let raw = FakeChain::from_state(FakeState::default());
    raw.queue_transport_error(
        "chain_getBlockHash",
        subxt_rpcs::Error::Client(Box::new(RpcError::Timeout {
            detail: String::new(),
        })),
    );
    let error = Client::from_rpc(RpcClient::new(raw), config())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the connection timed out without a response"
    );
}

#[tokio::test]
async fn rpc_substrate_stream_preserves_public_policy_metadata() {
    let raw = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(raw.clone()), config())
        .await
        .unwrap();
    raw.queue_transport_error(
        "chain_subscribeNewHeads",
        policy("stream rate exceeded", "12"),
    );
    let error = match client.blocks_strict(Finality::Best).await {
        Err(error) => error,
        Ok(_) => panic!("expected refusal"),
    };
    assert_eq!(error.policy().unwrap().retry_after.as_deref(), Some("12"));
    assert_eq!(
        error.policy().unwrap().payload.as_deref(),
        Some(&json!({"error": {
            "code": -32005,
            "message": "stream rate exceeded",
            "data": {"retry_after": "12"},
        }}))
    );
    // A following valid stream proves the failure did not corrupt its wrapper.
    let mut stream = client.blocks_strict(Finality::Best).await.unwrap();
    raw.advance(1).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn rpc_substrate_write_helper_preserves_public_policy_metadata() {
    let raw = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(raw.clone()), config())
        .await
        .unwrap();
    raw.queue_transport_error("chain_getBlock", policy("write rate exceeded", "9"));
    let error = client
        .find_extrinsic(H256([1; 32]), H256([2; 32]))
        .await
        .unwrap_err();
    assert_eq!(error.policy().unwrap().retry_after.as_deref(), Some("9"));
}

#[tokio::test]
async fn rpc_substrate_submit_preserves_policy_metadata_and_clears_nonce() {
    let raw = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(raw.clone()), config())
        .await
        .unwrap();
    client.nonces().pin_for_address("5Signer", 7).await;
    raw.queue_transport_error(
        "author_submitExtrinsic",
        policy("submit rate exceeded", "7"),
    );
    let error = client.submit_signed_raw(&[1], "5Signer").await.unwrap_err();
    assert_eq!(error.policy().unwrap().retry_after.as_deref(), Some("7"));
    assert_eq!(client.nonces().cached("5Signer").await, None);
}

#[tokio::test]
async fn rpc_substrate_submit_timeout_is_a_public_connection_error() {
    let raw = FakeChain::from_state(FakeState::default());
    let client = Client::from_rpc(RpcClient::new(raw.clone()), config())
        .await
        .unwrap();
    client.nonces().pin_for_address("5Signer", 7).await;
    raw.queue_transport_error(
        "author_submitExtrinsic",
        subxt_rpcs::Error::Client(Box::new(RpcError::MaxRetriesExceeded {
            detail: "no response from wss://rpc.example in 60s".into(),
        })),
    );
    let error = client.submit_signed_raw(&[1], "5Signer").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "no response from wss://rpc.example in 60s"
    );
    assert_eq!(client.nonces().cached("5Signer").await, None);
}

#[tokio::test]
async fn raw_submission_cancellation_and_shutdown_clear_without_changing_the_nonce() {
    use rtao_testkit::Reply;
    use std::time::Duration;
    for close in [false, true] {
        let raw = FakeChain::from_state(FakeState::default());
        let client = Client::from_rpc(RpcClient::new(raw.clone()), config())
            .await
            .unwrap();
        client.nonces().pin_for_address("5Signer", 7).await;
        raw.mutate_state(|state| state.queue_reply("author_submitExtrinsic", Reply::silent()));
        let task_client = client.clone();
        let task =
            tokio::spawn(async move { task_client.submit_signed_raw(&[1], "5Signer").await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !raw
                .requests()
                .iter()
                .any(|request| request.method == "author_submitExtrinsic")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(client.nonces().cached("5Signer").await, Some(7));
        if close {
            client.close().await;
            assert!(matches!(
                task.await.unwrap(),
                Err(rtao_client::ClientError::Rpc(RpcError::Closed))
            ));
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(client.nonces().cached("5Signer").await, None);
    }
}
