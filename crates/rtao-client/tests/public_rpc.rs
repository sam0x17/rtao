// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Client-boundary transport cases from the pinned Python source.

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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_exposes_public_policy_error
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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_exposes_public_policy_error
#[tokio::test]
async fn rpc_substrate_policy_error_during_connect_closes_its_socket() {
    use rtao_testkit::FakeNode;
    use std::time::Duration;

    let mut node = FakeNode::start().await.unwrap();
    node.mutate_state(|state| {
        state.queue_reply(
            "chain_getBlockHash",
            Reply::error(
                -32005,
                "request rate exceeded",
                Some(json!({"retry_after": "60"})),
            ),
        );
    });
    let mut cfg = ClientConfig::new(Network::resolve_with(node.url(), |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    cfg.startup_timeout = Duration::from_secs(5);
    cfg.session.rate_limit = None;
    cfg.session.retry_forever = true;
    cfg.session.silence_timeout = Duration::from_secs(30);

    let error = Client::connect(cfg).await.unwrap_err();
    assert_eq!(error.policy().unwrap().retry_after.as_deref(), Some("60"));
    assert!(
        node.requests().iter().any(|request| {
            request.method == "chain_getBlockHash" && request.params == json!([0])
        })
    );
    // The server sent only a JSON-RPC error and remains alive here. Neither
    // node shutdown nor an explicit successful-client close can satisfy this.
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_closed(1))
        .await
        .unwrap();
    assert_eq!(node.connect_attempts(), 1);
    let connections = node.connections();
    assert_eq!(connections.len(), 1);
    assert!(connections[0].accepted && connections[0].closed);
    node.shutdown().await;
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_exposes_public_blank_timeout_error
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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_stream_preserves_public_policy_metadata
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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_write_helper_preserves_public_policy_metadata
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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_submit_preserves_policy_metadata_and_clears_nonce
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

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_substrate_submit_timeout_is_a_public_connection_error
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

#[tokio::test]
async fn public_connection_error_text_uses_python_strip_without_rewriting_typed_causes() {
    let separators = "\u{1c}\u{1d}\u{1e}\u{1f}\t";
    for (cause, expected) in [
        (
            RpcError::Timeout {
                detail: " \u{1c} timed out \u{1f}\n".into(),
            },
            "timed out",
        ),
        (
            RpcError::MaxRetriesExceeded {
                detail: "\u{1d} no response \u{1e}".into(),
            },
            "no response",
        ),
        (
            RpcError::Transport("\u{1f} transport sentinel \u{1c}".into()),
            "transport sentinel",
        ),
        (
            RpcError::Protocol("\u{1e} protocol sentinel \u{1d}".into()),
            "protocol sentinel",
        ),
        (
            RpcError::Timeout {
                detail: separators.into(),
            },
            "the connection timed out without a response",
        ),
        (
            RpcError::MaxRetriesExceeded {
                detail: separators.into(),
            },
            "the RPC connection failed without an error detail",
        ),
        (
            RpcError::Transport(separators.into()),
            "the RPC connection failed without an error detail",
        ),
        (
            RpcError::Protocol(separators.into()),
            "the RPC connection failed without an error detail",
        ),
        (
            RpcError::MaxRetriesExceeded {
                detail: String::new(),
            },
            "the RPC connection failed without an error detail",
        ),
        (
            RpcError::Handshake {
                status: 502,
                detail: "gateway sentinel \u{1f}\r".into(),
            },
            "RPC endpoint rejected the WebSocket handshake (HTTP 502): gateway sentinel",
        ),
        (
            RpcError::FateUnknown {
                hash: rtao_rpc::H256([7; 32]),
            },
            "Connection dropped while an extrinsic submission was in flight; the transaction may already be in the pool or on chain. Verify chain state before retrying.",
        ),
    ] {
        // Exercise the actual injected transport -> startup public boundary.
        let raw = FakeChain::from_state(FakeState::default());
        raw.queue_transport_error(
            "chain_getBlockHash",
            subxt_rpcs::Error::Client(Box::new(cause.clone())),
        );
        let error = Client::from_rpc(RpcClient::new(raw), config())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        let rtao_client::ClientError::Rpc(retained) = &error else {
            panic!("transport error lost its typed public cause")
        };
        assert_eq!(retained, &cause);
        assert_eq!(
            std::error::Error::source(&error).and_then(|source| source.downcast_ref::<RpcError>()),
            Some(&cause)
        );
    }
    // The separate rpc_substrate_exposes_public_policy_error fixture retains
    // independent exact payload/retry/policy/reason and safe-display assertions.
}
