// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Socket-level transport parity and supervisor regression tests.
//!
//! The six upstream `test_rpc_substrate_*` declarations require the public
//! client and transaction layers. Their ports belong to those layers and are
//! deliberately not counted as transport coverage here. The policy module
//! ports the five classifier-only declarations, including all parameter cases.
//!
//! All six public-boundary declarations are now exercised through production
//! Client APIs in `crates/rtao-client/tests/public_rpc.rs`, including the
//! submission error metadata and account nonce invalidation cases.

use std::{error::Error, future::pending, time::Duration};

use blake2::{Blake2b, Digest, digest::consts::U32};
use futures::StreamExt;
use rtao_rpc::{
    EndpointPool, ReadMethod, RpcError, Session, SessionConfig, SessionState, SubmitError,
    SubscriptionKind,
};
use rtao_testkit::{FakeNode, FakeState, Fault, Reply, RpcRequest};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use url::Url;

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

trait Handlers {
    fn set_handler(
        &self,
        method: &str,
        handler: impl FnMut(&mut FakeState, &RpcRequest) -> Reply + Send + 'static,
    );
    fn set_result(&self, method: &str, result: Value);
    fn queue_reply(&self, method: &str, reply: Reply);
}

impl Handlers for FakeNode {
    fn set_handler(
        &self,
        method: &str,
        handler: impl FnMut(&mut FakeState, &RpcRequest) -> Reply + Send + 'static,
    ) {
        self.mutate_state(|state| state.set_handler(method, handler));
    }
    fn set_result(&self, method: &str, result: Value) {
        self.mutate_state(|state| state.set_result(method, result));
    }
    fn queue_reply(&self, method: &str, reply: Reply) {
        self.mutate_state(|state| state.queue_reply(method, reply));
    }
}

fn config(node: &FakeNode) -> Result<SessionConfig> {
    Ok(SessionConfig {
        pool: EndpointPool {
            urls: vec![Url::parse(node.url())?],
            archive: Vec::new(),
        },
        silence_timeout: Duration::from_millis(500),
        ..SessionConfig::default()
    })
}

async fn connect(node: &FakeNode) -> Result<Session> {
    Ok(Session::connect(config(node)?).await?)
}

fn echo(node: &FakeNode) {
    node.set_handler("system_chain", |_, request| {
        Reply::result(request.params[0].clone())
    });
}

async fn wait_method_requests(node: &FakeNode, method: &str, count: usize) -> Result {
    timeout(Duration::from_secs(5), async {
        loop {
            if node
                .requests()
                .iter()
                .filter(|r| r.method == method)
                .count()
                >= count
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await?;
    Ok(())
}

async fn reconnecting(session: &Session) -> Result {
    let mut state = session.state();
    timeout(
        Duration::from_secs(2),
        state.wait_for(|value| matches!(value, SessionState::Reconnecting { .. })),
    )
    .await??;
    Ok(())
}

struct StalledHandshake {
    url: Url,
    task: JoinHandle<std::io::Result<()>>,
}

impl StalledHandshake {
    async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("ws://{}", listener.local_addr()?))?;
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            pending::<()>().await;
            drop(socket);
            Ok(())
        });
        Ok(Self { url, task })
    }
}

impl Drop for StalledHandshake {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_request_response_roundtrip
// pyt: sdk/python/tests/unit/test_rpc.py::test_default_websocket_connect_returns_connection
#[tokio::test]
async fn request_response_roundtrip() -> Result {
    let node = FakeNode::start().await?;
    node.set_result("system_chain", json!("Bittensor"));
    let session = connect(&node).await?;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "Bittensor"
    );
    assert_eq!(node.connections().len(), 1);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_concurrent_requests_correlate_by_id
#[tokio::test]
async fn concurrent_requests_correlate_by_id() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("system_chain", |_, _| Reply::silent());
    let session = connect(&node).await?;
    let mut tasks = Vec::new();
    for index in 0..50_u64 {
        let session = session.clone();
        tasks.push(tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([index]))
                .await
        }));
    }
    wait_method_requests(&node, "system_chain", 50).await?;
    let requests: Vec<_> = node
        .requests()
        .into_iter()
        .filter(|r| r.method == "system_chain")
        .collect();
    for request in requests.iter().rev() {
        node.push_frame(json!({"jsonrpc":"2.0", "id":request.id, "result":request.params[0]}))?;
    }
    for (index, task) in tasks.into_iter().enumerate() {
        assert_eq!(timeout(Duration::from_secs(2), task).await???, index as u64);
    }
    let mut ids: Vec<_> = requests
        .into_iter()
        .map(|request| {
            request
                .id
                .as_u64()
                .ok_or("request ID was not an unsigned integer")
        })
        .collect::<std::result::Result<_, _>>()?;
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 50);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_batch_preserves_order_and_demuxes
#[tokio::test]
async fn batch_preserves_order_and_demuxes() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("system_chain", |_, _| Reply::silent());
    let session = connect(&node).await?;
    let batch = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request_many(
                    (0..10)
                        .map(|i| (ReadMethod::SystemChain, json!([i])))
                        .collect(),
                )
                .await
        })
    };
    wait_method_requests(&node, "system_chain", 10).await?;
    let responses = node
        .requests()
        .into_iter()
        .filter(|r| r.method == "system_chain")
        .rev()
        .map(|request| json!({"jsonrpc":"2.0", "id":request.id, "result":request.params[0]}))
        .collect();
    node.push_frame(Value::Array(responses))?;
    let values = timeout(Duration::from_secs(2), batch).await???;
    for (index, result) in values.into_iter().enumerate() {
        assert_eq!(serde_json::from_str::<u64>(result?.get())?, index as u64);
    }
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        10
    );
    let batches: Vec<_> = node.frames().into_iter().filter(Value::is_array).collect();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0].as_array().ok_or("batch was not an array")?.len(),
        10
    );
    assert_eq!(node.connections().len(), 1);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_rpc_error_raises_substrate_request_exception
#[tokio::test]
async fn rpc_error_preserves_server_details() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply("system_chain", Reply::error(1, "boom", None));
    let session = connect(&node).await?;
    let error = session
        .request::<Value>(ReadMethod::SystemChain, json!([]))
        .await
        .err()
        .ok_or("request unexpectedly succeeded")?;
    assert!(matches!(&error, RpcError::JsonRpc { code: 1, message, .. } if message == "boom"));
    assert!(error.to_string().contains("boom"));
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_policy_rpc_error_preserves_server_details
#[tokio::test]
async fn policy_rpc_error_preserves_server_details() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply(
        "system_chain",
        Reply::error(
            -32004,
            "request rate exceeded",
            Some(json!({"policy":"public-rpc","reason":"weighted_rate","retry_after":30})),
        ),
    );
    let session = connect(&node).await?;
    let error = session
        .request::<Value>(ReadMethod::SystemChain, json!([]))
        .await
        .err()
        .ok_or("request unexpectedly succeeded")?;
    match error {
        RpcError::Policy(ref refusal) => {
            assert!(refusal.message.contains("request rate exceeded"));
            assert_eq!(refusal.policy.as_deref(), Some("public-rpc"));
            assert_eq!(refusal.reason.as_deref(), Some("weighted_rate"));
            assert_eq!(refusal.retry_after.as_deref(), Some("30"));
        }
        _ => return Err(format!("unexpected error: {error}").into()),
    }
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_websocket_rate_limit_preserves_handshake_response
// pyt: sdk/python/tests/unit/test_rpc.py::test_websocket_access_policy_preserves_handshake_response
// pyt: sdk/python/tests/unit/test_rpc.py::test_duplicate_policy_headers_remain_a_terminal_refusal
// pyt: sdk/python/tests/unit/test_rpc.py::test_handshake_diagnostics_strip_terminal_controls
// pyt: sdk/python/tests/unit/test_rpc.py::test_policy_refusal_is_not_retried_even_with_retry_forever
#[tokio::test]
async fn handshake_policy_refusal_headers() -> Result {
    for status in [403, 429] {
        let node = FakeNode::with_faults(
            std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
            rtao_testkit::FaultPlan::new(vec![Fault::HandshakeRefuse {
                status,
                headers: vec![
                    ("X-RateLimit-Policy".into(), "ws-\u{202e}upgrade".into()),
                    ("X-RateLimit-Policy".into(), "duplicate-value".into()),
                    ("X-RateLimit-Reason".into(), "blocked-source".into()),
                    ("Retry-After".into(), "60".into()),
                ],
                body: "{\"error\":\"slow down\\u001b]8;;https://evil.example\\u0007\\u202eclick\"}"
                    .into(),
            }]),
        )
        .await?;
        let fallback = FakeNode::start().await?;
        let mut cfg = config(&node)?;
        cfg.pool.urls.push(Url::parse(fallback.url())?);
        cfg.retry_forever = true;
        let error = timeout(Duration::from_secs(2), Session::connect(cfg))
            .await?
            .err()
            .ok_or("handshake unexpectedly succeeded")?;
        match error {
            RpcError::Policy(ref refusal) => {
                assert_eq!(refusal.status_code, Some(status));
                assert_eq!(refusal.policy.as_deref(), Some("ws- upgrade"));
                assert_eq!(refusal.reason.as_deref(), Some("blocked-source"));
                assert_eq!(refusal.retry_after.as_deref(), Some("60"));
                assert!(refusal.message.contains(&format!("HTTP {status}")));
                assert!(refusal.message.contains("slow down"));
                assert!(!refusal.message.contains(['\u{1b}', '\u{7}', '\u{202e}']));
            }
            _ => return Err(format!("unexpected handshake error: {error}").into()),
        }
        assert_eq!(node.connect_attempts(), 1);
        assert_eq!(fallback.connect_attempts(), 0);
    }
    // Python falls back to X-RateLimit-Reset if Retry-After is absent.
    let node = FakeNode::with_faults(
        std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
        rtao_testkit::FaultPlan::new(vec![Fault::HandshakeRefuse {
            status: 429,
            headers: vec![("X-RateLimit-Reset".into(), "120".into())],
            body: String::new(),
        }]),
    )
    .await?;
    let error = Session::connect(config(&node)?)
        .await
        .err()
        .ok_or("handshake unexpectedly succeeded")?;
    assert!(
        matches!(error, RpcError::Policy(ref refusal) if refusal.retry_after.as_deref() == Some("120"))
    );
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_non_policy_websocket_rejection_preserves_status_and_body
#[tokio::test]
async fn non_policy_websocket_rejection_preserves_status_and_body() -> Result {
    let node = FakeNode::with_faults(
        std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
        rtao_testkit::FaultPlan::new(vec![Fault::HandshakeRefuse {
            status: 502,
            headers: Vec::new(),
            body: "upstream temporarily unavailable".into(),
        }]),
    )
    .await?;
    let error = Session::connect(config(&node)?)
        .await
        .err()
        .ok_or("handshake unexpectedly succeeded")?;
    assert!(
        matches!(&error, RpcError::Handshake { status: 502, detail } if detail.contains("upstream temporarily unavailable"))
    );
    assert!(error.to_string().contains("HTTP 502"));
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_websocket_handshake_timeout_has_a_message
// pyt: sdk/python/tests/unit/test_rpc.py::test_websocket_handshake_timeout_redacts_endpoint_secrets
#[tokio::test]
async fn websocket_handshake_timeout_has_a_message_and_redacts_secrets() -> Result {
    let server = StalledHandshake::start().await?;
    let mut endpoint = server.url.clone();
    endpoint
        .set_username("user")
        .map_err(|()| "endpoint username rejected")?;
    endpoint
        .set_password(Some("password"))
        .map_err(|()| "endpoint password rejected")?;
    endpoint.set_path("/private");
    endpoint.set_query(Some("api_key=secret"));
    let cfg = SessionConfig {
        pool: EndpointPool {
            urls: vec![endpoint],
            archive: Vec::new(),
        },
        handshake_timeout: Duration::from_millis(20),
        ..SessionConfig::default()
    };
    let error = timeout(Duration::from_secs(2), Session::connect(cfg))
        .await?
        .err()
        .ok_or("handshake unexpectedly succeeded")?;
    let message = error.to_string();
    assert!(
        message.contains("no response during the WebSocket handshake"),
        "{message}"
    );
    assert!(message.contains("127.0.0.1"));
    for secret in ["user", "password", "private", "api_key", "secret"] {
        assert!(
            !message.contains(secret),
            "endpoint secret leaked: {secret}"
        );
    }
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_policy_close_is_sanitized_and_not_retried
#[tokio::test]
async fn close_1008_is_terminal() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("system_chain", |_, _| Reply::silent());
    let mut cfg = config(&node)?;
    cfg.retry_forever = true;
    let session = Session::connect(cfg).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::SystemChain, json!([]))
                .await
        })
    };
    wait_method_requests(&node, "system_chain", 1).await?;
    node.close_connection(1008, "slow\u{1b}[2J\u{202e}down")?;
    let error = timeout(Duration::from_secs(2), request)
        .await??
        .err()
        .ok_or("request unexpectedly succeeded")?;
    match error {
        RpcError::Policy(ref refusal) => {
            assert_eq!(refusal.status_code, Some(1008));
            assert!(refusal.message.contains("WebSocket 1008"));
            assert!(!refusal.message.contains(['\u{1b}', '\u{202e}']));
            assert_eq!(refusal.reason.as_deref(), Some("slow [2J down"));
        }
        _ => return Err(format!("unexpected close error: {error}").into()),
    }
    assert_eq!(node.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_state_discarded_classified
#[tokio::test]
async fn state_discarded_classified() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply(
        "state_getStorage",
        Reply::error(
            4003,
            "Client error: Api called for an unknown Block: State already discarded for 0xabc123",
            None,
        ),
    );
    let session = connect(&node).await?;
    let error = session
        .request::<Value>(ReadMethod::StateGetStorage, json!(["0x"]))
        .await
        .err()
        .ok_or("request unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::StateDiscarded { ref block } if block == "0xabc123"));
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_subscription_streams_updates
#[tokio::test]
async fn subscription_streams_updates() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply(
        "chain_subscribeNewHeads",
        Reply::subscription(
            "sub-1",
            vec![json!({"number":1}), json!({"number":2})],
            false,
        ),
    );
    node.set_result("chain_unsubscribeNewHeads", json!(true));
    let session = connect(&node).await?;
    let mut sub = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    let first = timeout(Duration::from_secs(2), sub.next())
        .await?
        .ok_or("subscription ended")??;
    let second = timeout(Duration::from_secs(2), sub.next())
        .await?
        .ok_or("subscription ended")??;
    assert_eq!([first, second], [json!({"number":1}), json!({"number":2})]);
    sub.unsubscribe().await;
    wait_method_requests(&node, "chain_unsubscribeNewHeads", 1).await?;
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_subscription_update_racing_ahead_of_response_is_buffered
#[tokio::test]
async fn subscription_update_racing_ahead_of_response_is_buffered() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply(
        "chain_subscribeNewHeads",
        Reply::subscription("sub-early", vec![json!({"number":7})], true),
    );
    let session = connect(&node).await?;
    let mut sub = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    assert_eq!(
        timeout(Duration::from_secs(2), sub.next())
            .await?
            .ok_or("subscription ended")??,
        json!({"number":7})
    );
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_reconnect_resubmits_pending_request
#[tokio::test]
async fn reconnect_resubmits_pending_request() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::silent());
    let session = connect(&node).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<String>(ReadMethod::SystemChain, json!(["hello"]))
                .await
        })
    };
    wait_method_requests(&node, "system_chain", 1).await?;
    node.drop_connection()?;
    assert_eq!(timeout(Duration::from_secs(3), request).await???, "hello");
    let requests: Vec<_> = node
        .requests()
        .into_iter()
        .filter(|r| r.method == "system_chain")
        .collect();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].id, requests[1].id);
    assert_eq!(node.connections().len(), 2);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_reconnect_fails_open_subscriptions
#[tokio::test]
async fn subscription_lost_on_reconnect() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply("chain_subscribeNewHeads", Reply::result(json!("sub-1")));
    let session = connect(&node).await?;
    let mut sub = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    node.drop_connection()?;
    let error = timeout(Duration::from_secs(2), sub.next())
        .await?
        .ok_or("subscription silently ended")?
        .err()
        .ok_or("subscription unexpectedly produced an update")?;
    assert!(matches!(error, RpcError::SubscriptionLost));
    assert!(error.to_string().contains("may already be on chain"));
    assert!(
        error
            .to_string()
            .contains("verify balances before retrying")
    );
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_connection_refused_rotates_to_fallback
#[tokio::test]
async fn connection_refused_rotates_to_fallback() -> Result {
    let primary = FakeNode::start().await?;
    primary.refuse_next(1);
    let fallback = FakeNode::start().await?;
    fallback.set_result("system_chain", json!("pong"));
    let mut cfg = config(&primary)?;
    cfg.pool.urls.push(Url::parse(fallback.url())?);
    let session = Session::connect(cfg).await?;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "pong"
    );
    assert!(
        matches!(&*session.state().borrow(), SessionState::Connected { url } if url == &Url::parse(fallback.url())?)
    );
    assert_eq!(primary.connect_attempts(), 1);
    assert_eq!(fallback.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_all_endpoints_down_raises_without_retry_forever
#[tokio::test]
async fn all_endpoints_down_raises_without_retry_forever() -> Result {
    let primary = FakeNode::start().await?;
    primary.refuse_next(100);
    let fallback = FakeNode::start().await?;
    fallback.refuse_next(100);
    let mut cfg = config(&primary)?;
    cfg.pool.urls.push(Url::parse(fallback.url())?);
    let error = Session::connect(cfg)
        .await
        .err()
        .ok_or("connection unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::MaxRetriesExceeded { .. }));
    assert_eq!(primary.connect_attempts(), 1);
    assert_eq!(fallback.connect_attempts(), 1);
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_retry_forever_keeps_cycling_until_a_connect_lands
#[tokio::test]
async fn retry_forever_keeps_cycling_until_a_connect_lands() -> Result {
    let primary = FakeNode::start().await?;
    primary.refuse_next(2);
    let fallback = FakeNode::start().await?;
    fallback.refuse_next(1);
    fallback.set_result("system_chain", json!("pong"));
    let mut cfg = config(&primary)?;
    cfg.pool.urls.push(Url::parse(fallback.url())?);
    cfg.retry_forever = true;
    let session = timeout(Duration::from_secs(6), Session::connect(cfg)).await??;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "pong"
    );
    assert_eq!(primary.connect_attempts(), 2);
    assert_eq!(fallback.connect_attempts(), 2);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_response_timeout_triggers_reconnect_and_resubmission
#[tokio::test]
async fn response_timeout_triggers_reconnect_and_resubmission() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::silent());
    let mut cfg = config(&node)?;
    cfg.silence_timeout = Duration::from_millis(20);
    let session = Session::connect(cfg).await?;
    assert_eq!(
        timeout(
            Duration::from_secs(3),
            session.request::<String>(ReadMethod::SystemChain, json!(["back"]))
        )
        .await??,
        "back"
    );
    assert_eq!(node.connect_attempts(), 2);
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        2
    );
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_timeouts_exhaust_into_max_retries_exceeded
#[tokio::test]
async fn timeouts_exhaust_into_max_retries_exceeded() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("system_chain", |_, _| Reply::silent());
    let mut cfg = config(&node)?;
    cfg.silence_timeout = Duration::from_millis(20);
    cfg.max_retries = 2;
    let session = Session::connect(cfg).await?;
    let error = timeout(
        Duration::from_secs(3),
        session.request::<Value>(ReadMethod::SystemChain, json!(["x"])),
    )
    .await?
    .err()
    .ok_or("request unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::MaxRetriesExceeded { .. }));
    assert!(error.to_string().contains("no response from"));
    assert!(error.to_string().contains("127.0.0.1"));
    assert_eq!(node.connections().len(), 2);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_no_double_send_when_request_races_a_reconnect
#[tokio::test]
async fn no_double_send_when_request_races_a_reconnect() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    let session = connect(&node).await?;
    node.drop_connection()?;
    reconnecting(&session).await?;
    assert_eq!(
        timeout(
            Duration::from_secs(3),
            session.request::<String>(ReadMethod::SystemChain, json!(["once"]))
        )
        .await??,
        "once"
    );
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        1
    );
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_late_notification_after_unsubscribe_is_dropped
#[tokio::test]
async fn late_notification_after_unsubscribe_is_dropped() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply("chain_subscribeNewHeads", Reply::result(json!("sub-late")));
    node.set_result("chain_unsubscribeNewHeads", json!(true));
    let session = connect(&node).await?;
    let mut sub = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    let id = sub.id().to_owned();
    sub.unsubscribe().await;
    node.push_subscription_update(&id, json!({"number":99}))?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let statistics = session.statistics().await?;
    assert_eq!(statistics.early_notification_ids, 0);
    assert_eq!(statistics.subscriptions, 0);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_failure_counter_resets_on_healthy_traffic
#[tokio::test]
async fn failure_counter_resets_on_healthy_traffic() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    let mut cfg = config(&node)?;
    cfg.max_retries = 2;
    let session = Session::connect(cfg).await?;
    for index in 0..6 {
        node.queue_reply("system_chain", Reply::silent());
        let request = {
            let session = session.clone();
            tokio::spawn(async move {
                session
                    .request::<String>(ReadMethod::SystemChain, json!(["ok"]))
                    .await
            })
        };
        wait_method_requests(&node, "system_chain", index * 2 + 1).await?;
        node.drop_connection()?;
        assert_eq!(timeout(Duration::from_secs(3), request).await???, "ok");
    }
    assert_eq!(node.connections().len(), 7);
    session.close().await;
    Ok(())
}

// pyt: sdk/python/tests/unit/test_rpc.py::test_close_fails_pending_requests
#[tokio::test]
async fn close_fails_pending_requests() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("system_chain", |_, _| Reply::silent());
    let session = connect(&node).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::SystemChain, json!(["x"]))
                .await
        })
    };
    wait_method_requests(&node, "system_chain", 1).await?;
    session.close().await;
    let error = timeout(Duration::from_secs(1), request)
        .await??
        .err()
        .ok_or("request unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::Closed));
    assert!(error.to_string().contains("closed"));
    Ok(())
}

#[tokio::test]
async fn reconnect_resends_reads_not_sent_submits() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::silent());
    node.queue_reply("author_submitExtrinsic", Reply::silent());
    let session = connect(&node).await?;
    let read = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<String>(ReadMethod::SystemChain, json!(["read"]))
                .await
        })
    };
    let submit = {
        let session = session.clone();
        tokio::spawn(async move { session.submit(&[1, 2, 3]).await })
    };
    wait_method_requests(&node, "system_chain", 1).await?;
    wait_method_requests(&node, "author_submitExtrinsic", 1).await?;
    node.drop_connection()?;
    assert_eq!(timeout(Duration::from_secs(3), read).await???, "read");
    let error = timeout(Duration::from_secs(2), submit)
        .await??
        .err()
        .ok_or("submission unexpectedly succeeded")?;
    match error {
        SubmitError::FateUnknown { hash } => {
            assert_eq!(hash.as_bytes(), &Blake2b::<U32>::digest([1, 2, 3])[..])
        }
        other => return Err(format!("expected fate unknown, got {other}").into()),
    }
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "author_submitExtrinsic")
            .count(),
        1
    );
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        2
    );
    node.set_result(
        "author_submitExtrinsic",
        json!(format!("0x{}", "11".repeat(32))),
    );
    node.drop_connection()?;
    reconnecting(&session).await?;
    timeout(Duration::from_secs(3), session.submit(&[4, 5, 6])).await??;
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "author_submitExtrinsic")
            .count(),
        2
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn cancelled_unsent_submit_is_never_transmitted() -> Result {
    let node = FakeNode::start().await?;
    node.set_result(
        "author_submitExtrinsic",
        json!(format!("0x{}", "11".repeat(32))),
    );
    let session = connect(&node).await?;
    node.hold_handshakes(true);
    node.drop_connection()?;
    reconnecting(&session).await?;
    let submit = {
        let session = session.clone();
        tokio::spawn(async move { session.submit(&[1, 2, 3]).await })
    };
    timeout(Duration::from_secs(2), async {
        while session.statistics().await?.pending != 1 {
            tokio::task::yield_now().await;
        }
        Ok::<(), RpcError>(())
    })
    .await??;
    submit.abort();
    let result = submit.await;
    assert!(result.err().is_some_and(|error| error.is_cancelled()));
    node.hold_handshakes(false);
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !node
            .requests()
            .iter()
            .any(|r| r.method == "author_submitExtrinsic")
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn large_single_frame_response() -> Result {
    let node = FakeNode::start().await?;
    let payload = "a".repeat(20 * 1024 * 1024);
    node.set_result("system_chain", json!(payload));
    let mut cfg = config(&node)?;
    cfg.silence_timeout = Duration::from_secs(5);
    let session = Session::connect(cfg).await?;
    let response = timeout(
        Duration::from_secs(10),
        session.request::<String>(ReadMethod::SystemChain, json!([])),
    )
    .await??;
    assert_eq!(response.len(), 20 * 1024 * 1024);
    assert!(response.bytes().all(|byte| byte == b'a'));
    assert_eq!(node.connections().len(), 1);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn wss_connect_with_extra_roots() -> Result {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let node = FakeNode::start_tls().await?;
    node.set_result("system_chain", json!("Bittensor"));
    let mut cfg = config(&node)?;
    cfg.extra_roots.push(
        node.extra_root()
            .ok_or("TLS fake did not expose its root")?,
    );
    let session = Session::connect(cfg).await?;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "Bittensor"
    );
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn last_session_clone_drop_closes_socket() -> Result {
    let node = FakeNode::start().await?;
    let session = connect(&node).await?;
    let last = session.clone();
    drop(session);
    assert_eq!(
        node.connections()
            .iter()
            .filter(|connection| connection.closed)
            .count(),
        0
    );
    drop(last);
    timeout(Duration::from_secs(1), node.wait_for_closed(1)).await?;
    Ok(())
}

#[tokio::test]
async fn state_discarded_uses_archive_once() -> Result {
    let primary = FakeNode::start().await?;
    let archive = FakeNode::start().await?;
    let discarded = || Reply::error(4003, "State already discarded for 0xabc123", None);
    primary.queue_reply("state_getStorage", discarded());
    archive.set_result("state_getStorage", json!("0x1234"));
    let mut cfg = config(&primary)?;
    cfg.pool.archive.push(Url::parse(archive.url())?);
    let session = Session::connect(cfg).await?;
    assert_eq!(
        archive.connect_attempts(),
        0,
        "archive connected before a historical read needed it"
    );
    assert_eq!(
        session
            .request::<String>(ReadMethod::StateGetStorage, json!(["0xkey", "0xabc123"]))
            .await?,
        "0x1234"
    );
    assert_eq!(archive.connect_attempts(), 1);
    primary.queue_reply("state_getStorage", discarded());
    archive.queue_reply("state_getStorage", discarded());
    let error = session
        .request::<Value>(ReadMethod::StateGetStorage, json!(["0xkey", "0xabc123"]))
        .await
        .err()
        .ok_or("archive unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::StateDiscarded { .. }));
    assert_eq!(
        primary
            .requests()
            .iter()
            .filter(|r| r.method == "state_getStorage")
            .count(),
        2
    );
    assert_eq!(
        archive
            .requests()
            .iter()
            .filter(|r| r.method == "state_getStorage")
            .count(),
        2
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn unknown_transmitted_method_is_never_resent() -> Result {
    let node = FakeNode::start().await?;
    node.set_handler("provider_write", |_, _| Reply::silent());
    let session = connect(&node).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request_method::<Value>("provider_write", json!([]))
                .await
        })
    };
    wait_method_requests(&node, "provider_write", 1).await?;
    node.drop_connection()?;
    let error = timeout(Duration::from_secs(2), request)
        .await??
        .err()
        .ok_or("unknown method unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::ConnectionLost { resent: false }));
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "provider_write")
            .count(),
        1
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn request_many_retains_other_results_when_one_member_fails() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::error(1, "first failed", None));
    let session = connect(&node).await?;
    let results = session
        .request_many(vec![
            (ReadMethod::SystemChain, json!([0])),
            (ReadMethod::SystemChain, json!([1])),
        ])
        .await?;
    let mut results = results.into_iter();
    assert!(matches!(
        results.next().ok_or("missing first batch result")?,
        Err(RpcError::JsonRpc { code: 1, .. })
    ));
    let second = results.next().ok_or("missing second batch result")??;
    assert_eq!(serde_json::from_str::<u64>(second.get())?, 1);
    assert!(results.next().is_none());
    let frames_before = node.frames().len();
    assert!(session.request_many(Vec::new()).await?.is_empty());
    assert_eq!(
        node.frames().len(),
        frames_before,
        "empty batch sent a frame"
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn subscription_overflow_retains_newest_and_unsolicited_updates_are_not_buffered() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply(
        "chain_subscribeNewHeads",
        Reply::subscription(
            "sub-overflow",
            (0..5).map(|n| json!({"number":n})).collect(),
            true,
        ),
    );
    let mut cfg = config(&node)?;
    cfg.subscription_buffer = 2;
    cfg.early_notification_ids = 2;
    let session = Session::connect(cfg).await?;
    let mut sub = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    assert_eq!(
        timeout(Duration::from_secs(1), sub.next())
            .await?
            .ok_or("subscription ended")??,
        json!({"number":3})
    );
    assert_eq!(
        timeout(Duration::from_secs(1), sub.next())
            .await?
            .ok_or("subscription ended")??,
        json!({"number":4})
    );
    for number in 0..10 {
        node.push_subscription_update(format!("unclaimed-{number}"), json!(number))?;
    }
    // A response behind all notifications establishes that they were routed.
    let _: Value = session.request(ReadMethod::SystemHealth, json!([])).await?;
    let statistics = session.statistics().await?;
    assert_eq!(statistics.early_notification_ids, 0);
    assert_eq!(statistics.early_notification_bytes, 0);
    assert_eq!(statistics.subscription_retained_bytes, 0);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn extrinsic_watch_loss_is_fate_unknown_and_preserves_raw_error_type() -> Result {
    use subxt_rpcs::client::RpcClientT;

    assert!("author_submitExtrinsic".parse::<ReadMethod>().is_err());
    assert!(
        "author_submitAndWatchExtrinsic"
            .parse::<ReadMethod>()
            .is_err()
    );
    let node = FakeNode::start().await?;
    node.queue_reply(
        "author_submitAndWatchExtrinsic",
        Reply::result(json!("watch-1")),
    );
    let session = connect(&node).await?;
    let params = serde_json::value::to_raw_value(&json!(["0x010203"]))?;
    let mut raw = session
        .subscribe_raw(
            "author_submitAndWatchExtrinsic",
            Some(params),
            "author_unwatchExtrinsic",
        )
        .await?;
    node.drop_connection()?;
    let error = timeout(Duration::from_secs(2), raw.stream.next())
        .await?
        .ok_or("watch silently ended")?
        .err()
        .ok_or("watch unexpectedly produced a status")?;
    let subxt_rpcs::Error::Client(error) = error else {
        return Err("raw watch did not preserve its boxed submission error".into());
    };
    let SubmitError::FateUnknown { hash } = error
        .downcast_ref::<SubmitError>()
        .ok_or("raw watch error was not a SubmitError")?
    else {
        return Err("raw watch error did not report fate unknown".into());
    };
    assert_eq!(hash.as_bytes(), &Blake2b::<U32>::digest([1, 2, 3])[..]);
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "author_submitAndWatchExtrinsic")
            .count(),
        1
    );
    session.close().await;
    Ok(())
}

async fn spin_until(label: &str, mut ready: impl FnMut() -> bool) -> Result {
    let started = std::time::Instant::now();
    while !ready() {
        if started.elapsed() > Duration::from_secs(2) {
            return Err(format!(
                "socket condition {label} did not settle while the test clock was paused"
            )
            .into());
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}

#[tokio::test]
async fn pool_rotation_backoff_and_exhaustion() -> Result {
    assert_eq!(SessionConfig::default().max_retries, 5);
    let primary = FakeNode::start().await?;
    let fallback = FakeNode::start().await?;
    primary.set_handler("system_chain", |_, _| Reply::silent());
    fallback.set_handler("system_chain", |_, _| Reply::silent());
    // Discovery is diagnostic traffic. Silence it so its late arrival cannot
    // move a deadline while this test advances the clock at exact boundaries.
    primary.set_handler("rpc_methods", |_, _| Reply::silent());
    fallback.set_handler("rpc_methods", |_, _| Reply::silent());
    let mut cfg = config(&primary)?;
    cfg.max_retries = 2;
    cfg.silence_timeout = Duration::from_secs(1);
    cfg.pool.urls.push(Url::parse(fallback.url())?);
    let session = Session::connect(cfg).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::SystemChain, json!([]))
                .await
        })
    };
    wait_method_requests(&primary, "system_chain", 1).await?;
    tokio::time::pause();
    for failure in 1..=4_u32 {
        tokio::time::advance(Duration::from_millis(1001)).await;
        spin_until("failure state", || match &*session.state().borrow() {
            SessionState::Reconnecting { attempt } => *attempt == failure,
            SessionState::Failed { .. } => failure == 4,
            _ => false,
        })
        .await?;
        if failure == 4 {
            break;
        }
        let before = primary.connect_attempts() + fallback.connect_attempts();
        tokio::time::advance(Duration::from_millis(u64::from(failure) * 250 - 1)).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            primary.connect_attempts() + fallback.connect_attempts(),
            before,
            "reconnect started before backoff elapsed"
        );
        // Tokio rounds timer deadlines to millisecond ticks. Check the opening
        // tick after the boundary, while the previous assertion guards early sends.
        tokio::time::advance(Duration::from_millis(2)).await;
        spin_until("TCP attempt", || {
            primary.connect_attempts() + fallback.connect_attempts() == before + 1
        })
        .await?;
        spin_until("resent frame", || {
            primary
                .requests()
                .iter()
                .chain(fallback.requests().iter())
                .filter(|r| r.method == "system_chain")
                .count()
                == failure as usize + 1
        })
        .await?;
        assert_eq!(primary.connect_attempts(), 2);
        assert_eq!(
            fallback.connect_attempts(),
            if failure == 1 {
                0
            } else {
                failure as usize - 1
            }
        );
    }
    let error = request
        .await?
        .err()
        .ok_or("silenced pool unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::MaxRetriesExceeded { .. }));
    assert_eq!(primary.connect_attempts(), 2);
    assert_eq!(fallback.connect_attempts(), 2);
    session.close().await;
    tokio::time::resume();
    Ok(())
}

#[tokio::test]
async fn max_in_flight_bounds_requests_and_discards_cancelled_queued_submit() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::silent());
    node.set_result(
        "author_submitExtrinsic",
        json!(format!("0x{}", "11".repeat(32))),
    );
    let mut cfg = config(&node)?;
    cfg.max_in_flight = 1;
    let session = Session::connect(cfg).await?;
    let first = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([1]))
                .await
        })
    };
    wait_method_requests(&node, "system_chain", 1).await?;
    let mut reads = Vec::new();
    for index in 2..=3_u64 {
        let session = session.clone();
        reads.push(tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([index]))
                .await
        }));
    }
    let submit = {
        let session = session.clone();
        tokio::spawn(async move { session.submit(&[9, 9, 9]).await })
    };
    timeout(Duration::from_secs(1), async {
        while session.statistics().await?.pending != 4 {
            tokio::task::yield_now().await;
        }
        Ok::<(), RpcError>(())
    })
    .await??;
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        1
    );
    assert!(
        !node
            .requests()
            .iter()
            .any(|r| r.method == "author_submitExtrinsic")
    );
    submit.abort();
    assert!(submit.await.err().is_some_and(|error| error.is_cancelled()));
    assert_eq!(session.statistics().await?.pending, 3);
    let original = node
        .requests()
        .into_iter()
        .find(|r| r.method == "system_chain")
        .ok_or("first request was not recorded")?;
    node.push_frame(json!({"jsonrpc":"2.0", "id":original.id, "result":1}))?;
    assert_eq!(timeout(Duration::from_secs(1), first).await???, 1);
    for (offset, read) in reads.into_iter().enumerate() {
        assert_eq!(
            timeout(Duration::from_secs(1), read).await???,
            offset as u64 + 2
        );
    }
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        3
    );
    assert!(
        !node
            .requests()
            .iter()
            .any(|r| r.method == "author_submitExtrinsic")
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn caller_deadline_cancels_pending_read_before_reconnect() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.queue_reply("system_chain", Reply::silent());
    let session = connect(&node).await?;
    let error = session
        .request_with_timeout::<Value>(
            ReadMethod::SystemChain,
            json!(["gone"]),
            Duration::from_millis(20),
        )
        .await
        .err()
        .ok_or("unanswered request unexpectedly succeeded")?;
    assert!(matches!(error, RpcError::Timeout { .. }));
    assert_eq!(session.statistics().await?.pending, 0);
    node.drop_connection()?;
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    assert_eq!(
        node.requests()
            .iter()
            .filter(|r| r.method == "system_chain")
            .count(),
        1
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn cancelled_subscription_response_is_unsubscribed_before_caller_claims_it() -> Result {
    let node = FakeNode::start().await?;
    node.queue_reply("chain_subscribeNewHeads", Reply::silent());
    node.set_result("chain_unsubscribeNewHeads", json!(true));
    let session = connect(&node).await?;
    let mut subscribe = Box::pin(session.subscribe::<Value>(SubscriptionKind::NewHeads));
    assert!(futures::poll!(&mut subscribe).is_pending());
    wait_method_requests(&node, "chain_subscribeNewHeads", 1).await?;
    let request = node
        .requests()
        .into_iter()
        .find(|r| r.method == "chain_subscribeNewHeads")
        .ok_or("subscribe request was not recorded")?;
    node.push_frame(json!({"jsonrpc":"2.0", "id":request.id, "result":"unclaimed-sub"}))?;
    timeout(Duration::from_secs(1), async {
        while session.statistics().await?.subscriptions != 1 {
            tokio::task::yield_now().await;
        }
        Ok::<(), RpcError>(())
    })
    .await??;
    // The response is in the caller's oneshot, but its future has not been
    // polled again to construct the public Subscription and claim ownership.
    drop(subscribe);
    wait_method_requests(&node, "chain_unsubscribeNewHeads", 1).await?;
    let unsubscribe = node
        .requests()
        .into_iter()
        .find(|r| r.method == "chain_unsubscribeNewHeads")
        .ok_or("cancelled subscription was not unsubscribed")?;
    assert_eq!(unsubscribe.params, json!(["unclaimed-sub"]));
    assert_eq!(session.statistics().await?.subscriptions, 0);
    node.push_subscription_update("unclaimed-sub", json!({"number":99}))?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(session.statistics().await?.early_notification_ids, 0);
    session.close().await;
    Ok(())
}

#[cfg(feature = "rpc-rate-limit")]
#[tokio::test]
async fn ratelimit_paces_public_presets() -> Result {
    use std::sync::{Arc, Mutex};

    let node = FakeNode::start().await?;
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    for method in ["rpc_methods", "system_chain"] {
        let arrivals = arrivals.clone();
        node.set_handler(method, move |_, request| {
            if let Ok(mut arrivals) = arrivals.lock() {
                arrivals.push((request.method.clone(), std::time::Instant::now()));
            }
            if request.method == "rpc_methods" {
                Reply::result(json!({"version":1,"methods":["system_chain"]}))
            } else {
                Reply::result(json!("FakeChain"))
            }
        });
    }
    let mut cfg = SessionConfig::public(EndpointPool::new(Url::parse(node.url())?));
    cfg.silence_timeout = Duration::from_secs(5);
    let session = Session::connect(cfg).await?;
    let results = timeout(Duration::from_secs(5), async {
        tokio::join!(
            session.request::<String>(ReadMethod::SystemChain, json!([])),
            session.request::<String>(ReadMethod::SystemChain, json!([])),
        )
    })
    .await?;
    assert_eq!(results.0?, "FakeChain");
    assert_eq!(results.1?, "FakeChain");
    session.close().await;
    let arrivals = arrivals
        .lock()
        .map_err(|_| "arrival recorder was poisoned")?;
    assert_eq!(arrivals.len(), 3);
    assert_eq!(arrivals[0].0, "rpc_methods");
    // Discovery and both simultaneously queued reads share the same preset.
    // Allow 100ms for socket scheduling, while rejecting the old eight-token
    // startup burst and a limiter that omits the diagnostic discovery request.
    for pair in arrivals.windows(2) {
        assert!(pair[1].1.duration_since(pair[0].1) >= Duration::from_millis(900));
    }
    Ok(())
}

#[cfg(feature = "rpc-rate-limit")]
#[tokio::test]
async fn rate_bucket_prioritizes_submissions_and_drops_cancelled_queue_entries() -> Result {
    let node = FakeNode::start().await?;
    echo(&node);
    node.set_result(
        "author_submitExtrinsic",
        json!(format!("0x{}", "11".repeat(32))),
    );
    let mut cfg = config(&node)?;
    cfg.rate_limit = Some(rtao_rpc::RateLimit {
        per_second: 4.0,
        burst: 1,
    });
    let session = Session::connect(cfg).await?;
    assert_eq!(
        session
            .request::<u64>(ReadMethod::SystemChain, json!([0]))
            .await?,
        0
    );
    let read = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([1]))
                .await
        })
    };
    let cancelled = {
        let session = session.clone();
        tokio::spawn(async move { session.submit(&[9, 9, 9]).await })
    };
    let submit = {
        let session = session.clone();
        tokio::spawn(async move { session.submit(&[1, 2, 3]).await })
    };
    timeout(Duration::from_secs(1), async {
        while session.statistics().await?.pending != 3 {
            tokio::task::yield_now().await;
        }
        Ok::<(), RpcError>(())
    })
    .await??;
    cancelled.abort();
    assert!(
        cancelled
            .await
            .err()
            .is_some_and(|error| error.is_cancelled())
    );
    assert_eq!(session.statistics().await?.pending, 2);
    timeout(Duration::from_secs(2), submit).await???;
    assert_eq!(timeout(Duration::from_secs(2), read).await???, 1);
    let requests: Vec<_> = node
        .requests()
        .into_iter()
        .filter(|r| r.method != "rpc_methods")
        .collect();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, "system_chain");
    assert_eq!(requests[1].method, "author_submitExtrinsic");
    assert_eq!(requests[1].params, json!(["0x010203"]));
    assert_eq!(requests[2].method, "system_chain");
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn plaintext_session_rejects_invalid_extra_root_before_connecting() -> Result {
    let node = FakeNode::start().await?;
    let mut cfg = config(&node)?;
    cfg.extra_roots.push(vec![0_u8; 8].into());
    let error = Session::connect(cfg)
        .await
        .err()
        .ok_or("invalid root accepted")?;
    assert!(
        matches!(error, RpcError::Protocol(ref detail) if detail.starts_with("invalid extra RPC root certificate:"))
    );
    assert_eq!(node.connect_attempts(), 0);
    Ok(())
}

#[tokio::test]
async fn plaintext_primary_can_fail_over_to_trusted_wss() -> Result {
    let primary = FakeNode::start().await?;
    primary.refuse_next(100);
    let fallback = FakeNode::start_tls().await?;
    fallback.set_result("system_chain", json!("secure fallback"));
    let mut cfg = config(&primary)?;
    cfg.pool.urls.push(Url::parse(fallback.url())?);
    cfg.extra_roots
        .push(fallback.extra_root().ok_or("TLS fake root missing")?);
    let session = Session::connect(cfg).await?;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "secure fallback"
    );
    assert!(
        matches!(&*session.state().borrow(), SessionState::Connected { url } if url == &Url::parse(fallback.url())?)
    );
    assert_eq!(primary.connect_attempts(), 1);
    assert_eq!(fallback.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn plaintext_primary_keeps_trusted_wss_archive_lazy() -> Result {
    let primary = FakeNode::start().await?;
    let archive = FakeNode::start_tls().await?;
    primary.queue_reply(
        "state_getStorage",
        Reply::error(4003, "State already discarded for 0xabc123", None),
    );
    archive.set_result("state_getStorage", json!("0x1234"));
    let mut cfg = config(&primary)?;
    cfg.pool.archive.push(Url::parse(archive.url())?);
    cfg.extra_roots
        .push(archive.extra_root().ok_or("TLS fake root missing")?);
    let session = Session::connect(cfg).await?;
    assert_eq!(archive.connect_attempts(), 0);
    assert_eq!(
        session
            .request::<String>(ReadMethod::StateGetStorage, json!(["0xkey", "0xabc123"]))
            .await?,
        "0x1234"
    );
    assert_eq!(archive.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn untrusted_wss_certificate_is_rejected() -> Result {
    let node = FakeNode::start_tls().await?;
    let mut cfg = config(&node)?;
    cfg.max_retries = 1;
    let error = Session::connect(cfg)
        .await
        .err()
        .ok_or("untrusted TLS server accepted")?;
    assert!(matches!(error, RpcError::MaxRetriesExceeded { .. }));
    assert!(node.requests().is_empty());
    Ok(())
}
