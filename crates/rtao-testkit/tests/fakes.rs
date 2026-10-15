//! Verify that both fakes use encoded state and that faults cross real sockets.

use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use rtao_primitives::H256;
use rtao_testkit::{FakeChain, FakeNode, FakeState, Fault, FaultPlan, Notification, Reply};
use serde_json::{Value, json};
use subxt_rpcs::{
    Error as RpcError,
    client::{RpcClient, RpcClientT, rpc_params},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async, connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message, protocol::frame::coding::CloseCode},
};

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn receive(socket: &mut Socket) -> Result<Value> {
    let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await?
        .ok_or("socket ended before its response")??;
    Ok(serde_json::from_str(message.to_text()?)?)
}

async fn request(socket: &mut Socket, id: u64, method: &str, params: Value) -> Result<Value> {
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
                .to_string()
                .into(),
        ))
        .await?;
    receive(socket).await
}

#[tokio::test]
async fn socket_and_direct_clients_share_encoded_storage() -> Result {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let chain = FakeChain::new(state.clone());
    chain.mutate_state(|state| state.seed_encoded(vec![1, 2, 3], Some(vec![9, 8, 7])));
    let node = FakeNode::with_state(state).await?;
    let rpc = RpcClient::new(chain);
    let direct: Value = rpc
        .request("state_getStorage", rpc_params!["0x010203"])
        .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    let response = request(&mut socket, 41, "state_getStorage", json!(["0x010203"])).await?;
    assert_eq!(direct, json!("0x090807"));
    assert_eq!(response["result"], direct);
    assert_eq!(node.requests().len(), 2);
    assert_eq!(node.frames().len(), 1);
    socket.close(None).await?;
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_closed(1)).await?;
    Ok(())
}

#[tokio::test]
async fn batch_is_one_frame_with_correlated_results_and_errors() -> Result {
    let node = FakeNode::start().await?;
    node.mutate_state(|state| state.set_result("echo", json!("same")));
    let (mut socket, _) = connect_async(node.url()).await?;
    socket
        .send(Message::Text(
            json!([
                {"jsonrpc":"2.0","id":7,"method":"echo","params":[]},
                {"jsonrpc":"2.0","id":3,"method":"no_such_method","params":[]},
            ])
            .to_string()
            .into(),
        ))
        .await?;
    let frame = receive(&mut socket).await?;
    assert_eq!(frame[0]["id"], 7);
    assert_eq!(frame[0]["result"], "same");
    assert_eq!(frame[1]["id"], 3);
    assert_eq!(frame[1]["error"]["code"], -32601);
    assert_eq!(node.requests().len(), 2);
    assert_eq!(node.connections()[0].frames, 1);
    Ok(())
}

#[tokio::test]
async fn tungstenite_030_preserves_rejected_http_headers_and_body() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([Fault::HandshakeRefuse {
            status: 429,
            headers: vec![
                ("X-RateLimit-Policy".into(), "storage-work".into()),
                ("X-RateLimit-Policy".into(), "ws-\u{202e}upgrade".into()),
                ("Retry-After".into(), "17".into()),
            ],
            body: "slow down".into(),
        }]),
    )
    .await?;
    let outcome = connect_async(node.url()).await;
    let Err(WsError::Http(response)) = outcome else {
        return Err("expected a full HTTP response error".into());
    };
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(response.headers()["X-RateLimit-Policy"], "storage-work");
    let policy_headers = response
        .headers()
        .get_all("X-RateLimit-Policy")
        .iter()
        .map(|value| value.as_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        policy_headers,
        vec![b"storage-work".as_slice(), "ws-\u{202e}upgrade".as_bytes()]
    );
    assert_eq!(response.headers()["Retry-After"], "17");
    assert_eq!(response.body().as_deref(), Some(b"slow down".as_slice()));
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_closed(1)).await?;
    assert!(!node.connections()[0].accepted);
    Ok(())
}

#[tokio::test]
async fn tls_node_and_client_use_explicit_ring_without_a_process_default() -> Result {
    let before = rustls::crypto::CryptoProvider::get_default().is_some();
    let node = FakeNode::start_tls().await?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(node.extra_root().ok_or("missing fake TLS certificate")?)?;
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let (mut socket, response) = connect_async_tls_with_config(
        node.url(),
        None,
        false,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await?;
    assert_eq!(response.status().as_u16(), 101);
    assert_eq!(
        request(&mut socket, 1, "system_chain", json!([])).await?["result"],
        "FakeChain"
    );
    assert_eq!(
        rustls::crypto::CryptoProvider::get_default().is_some(),
        before
    );
    Ok(())
}

#[tokio::test]
async fn abrupt_drop_records_request_before_closing_without_response() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([Fault::DropAfterFrames(1)]),
    )
    .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":1,"method":"system_chain","params":[]})
                .to_string()
                .into(),
        ))
        .await?;
    let incoming = tokio::time::timeout(Duration::from_secs(1), socket.next()).await?;
    assert!(incoming.is_none() || incoming.is_some_and(|item| item.is_err()));
    assert_eq!(node.requests().len(), 1);
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_closed(1)).await?;
    Ok(())
}

#[tokio::test]
async fn policy_close_has_actual_rfc6455_1008_code() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([Fault::Close {
            code: 1008,
            reason: "gateway policy".into(),
        }]),
    )
    .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    let incoming = socket.next().await.ok_or("missing close frame")??;
    let Message::Close(Some(frame)) = incoming else {
        return Err("expected close frame".into());
    };
    assert_eq!(frame.code, CloseCode::Policy);
    assert_eq!(frame.reason.as_str(), "gateway policy");
    Ok(())
}

#[tokio::test]
async fn method_fault_is_consumed_once_while_pruned_hash_is_precise() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([
            Fault::JsonRpcError {
                method: "system_chain".into(),
                code: -32005,
                message: "limited".into(),
                data: Some(json!({"retryAfter":3})),
            },
            Fault::StateDiscarded {
                for_hash: H256([17; 32]),
            },
        ]),
    )
    .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    assert_eq!(
        request(&mut socket, 1, "system_chain", json!([])).await?["error"]["data"]["retryAfter"],
        3
    );
    assert_eq!(
        request(&mut socket, 2, "system_chain", json!([])).await?["result"],
        "FakeChain"
    );
    socket.close(None).await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    let hash = format!("0x{}", hex::encode([17; 32]));
    assert_eq!(
        request(&mut socket, 3, "state_getStorage", json!(["0x01", hash])).await?["error"]["code"],
        -32000
    );
    assert!(
        request(
            &mut socket,
            4,
            "state_getStorage",
            json!(["0x01", format!("0x{}", hex::encode([18; 32]))])
        )
        .await?
        .get("result")
        .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn silence_records_frames_without_consuming_handler_scripts() -> Result {
    let node = FakeNode::start().await?;
    node.mutate_state(|state| state.queue_reply("echo", Reply::result(json!("first"))));
    let (mut socket, _) = connect_async(node.url()).await?;
    node.silence(true);
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":1,"method":"echo","params":[]})
                .to_string()
                .into(),
        ))
        .await?;
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_requests(1)).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), socket.next())
            .await
            .is_err()
    );
    node.silence(false);
    assert_eq!(
        request(&mut socket, 2, "echo", json!([])).await?["result"],
        "first"
    );
    Ok(())
}

#[tokio::test]
async fn early_notifications_precede_subscription_response() -> Result {
    let node = FakeNode::start().await?;
    node.mutate_state(|state| {
        state.set_handler("heads", |_state, _request| Reply {
            outcome: Some(Ok(json!("sub42"))),
            notifications: vec![Notification {
                subscription: "sub42".into(),
                result: json!({"number":"0x1"}),
            }],
            early: true,
        })
    });
    let (mut socket, _) = connect_async(node.url()).await?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":7,"method":"heads","params":[]})
                .to_string()
                .into(),
        ))
        .await?;
    assert_eq!(
        receive(&mut socket).await?["params"]["subscription"],
        "sub42"
    );
    assert_eq!(receive(&mut socket).await?["id"], 7);
    node.push_subscription_update("sub42", json!({"number":"0x2"}))?;
    assert_eq!(
        receive(&mut socket).await?["params"]["result"]["number"],
        "0x2"
    );
    Ok(())
}

#[tokio::test]
async fn handshake_barrier_and_next_refusal_are_observable() -> Result {
    let node = FakeNode::start().await?;
    node.hold_handshakes(true);
    let url = node.url().to_owned();
    let connecting = tokio::spawn(async move { connect_async(url).await });
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_connections(1)).await?;
    assert!(!node.connections()[0].accepted);
    assert!(!connecting.is_finished());
    node.hold_handshakes(false);
    let (mut socket, _) = connecting.await??;
    socket.close(None).await?;
    node.refuse_next(1);
    assert!(connect_async(node.url()).await.is_err());
    let (_socket, _) = connect_async(node.url()).await?;
    assert_eq!(node.connect_attempts(), 3);
    Ok(())
}

#[tokio::test]
async fn slow_response_delays_reply_after_recording_the_request() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([Fault::SlowResponse(Duration::from_millis(60))]),
    )
    .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    let started = tokio::time::Instant::now();
    assert_eq!(
        request(&mut socket, 1, "system_chain", json!([])).await?["result"],
        "FakeChain"
    );
    assert!(started.elapsed() >= Duration::from_millis(60));
    Ok(())
}

#[tokio::test]
async fn delayed_response_keeps_drop_and_push_commands_live() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new([Fault::SlowResponse(Duration::from_secs(60))]),
    )
    .await?;
    let (mut socket, _) = connect_async(node.url()).await?;
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":1,"method":"system_chain","params":[]})
                .to_string()
                .into(),
        ))
        .await?;
    tokio::time::timeout(Duration::from_secs(1), node.wait_for_requests(1)).await?;
    node.push_frame(json!({"independent":"frame"}))?;
    assert_eq!(receive(&mut socket).await?["independent"], "frame");
    node.drop_connection()?;
    let incoming = tokio::time::timeout(Duration::from_secs(1), socket.next()).await?;
    assert!(incoming.is_none() || incoming.is_some_and(|item| item.is_err()));
    Ok(())
}

#[derive(Debug)]
struct FateMarker;
impl fmt::Display for FateMarker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("fate marker")
    }
}
impl Error for FateMarker {}

#[tokio::test]
async fn direct_transport_preserves_boxed_errors_and_targeted_subscriptions() -> Result {
    let chain = FakeChain::default();
    chain.queue_transport_error(
        "author_submitExtrinsic",
        RpcError::Client(Box::new(FateMarker)),
    );
    let outcome = chain.request_raw("author_submitExtrinsic", None).await;
    let Err(RpcError::Client(error)) = outcome else {
        return Err("missing client transport error".into());
    };
    assert!(error.downcast_ref::<FateMarker>().is_some());
    chain.mutate_state(|state| {
        state.set_result("heads", json!("a"));
        state.set_result("other_heads", json!("b"));
    });
    let mut heads = chain.subscribe_raw("heads", None, "unheads").await?;
    let mut other = chain.subscribe_raw("other_heads", None, "unheads").await?;
    chain.publish_to("a", json!({"number":"0x2"}))?;
    let update = heads.stream.next().await.ok_or("missing head")??;
    assert_eq!(
        serde_json::from_str::<Value>(update.get())?["number"],
        "0x2"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), other.stream.next())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn direct_heads_advance_with_finality_lag_and_unsubscribe_on_drop() -> Result {
    let chain = FakeChain::default();
    chain.mutate_state(|state| state.set_finalized(97))?;
    let rpc = RpcClient::new(chain.clone());
    let mut best = rpc
        .subscribe::<Value>(
            "chain_subscribeNewHeads",
            rpc_params![],
            "chain_unsubscribeNewHeads",
        )
        .await?;
    let mut finalized = rpc
        .subscribe::<Value>(
            "chain_subscribeFinalizedHeads",
            rpc_params![],
            "chain_unsubscribeFinalizedHeads",
        )
        .await?;
    assert_eq!(
        best.next().await.ok_or("missing initial best head")??["number"],
        "0x64"
    );
    assert_eq!(
        finalized
            .next()
            .await
            .ok_or("missing initial finalized head")??["number"],
        "0x61"
    );
    chain.advance(2)?;
    assert_eq!(
        best.next().await.ok_or("missing advanced best head")??["number"],
        "0x65"
    );
    assert_eq!(
        best.next().await.ok_or("missing advanced best head")??["number"],
        "0x66"
    );
    assert_eq!(
        finalized
            .next()
            .await
            .ok_or("missing advanced finalized head")??["number"],
        "0x62"
    );
    assert_eq!(
        finalized
            .next()
            .await
            .ok_or("missing advanced finalized head")??["number"],
        "0x63"
    );
    drop(best);
    assert!(
        chain
            .requests()
            .iter()
            .any(|request| request.method == "chain_unsubscribeNewHeads")
    );
    Ok(())
}

#[tokio::test]
async fn direct_requests_and_other_subscriptions_route_existing_updates() -> Result {
    let chain = FakeChain::default();
    chain.mutate_state(|state| {
        state.set_result("first", json!("existing"));
        state.set_handler("trigger", |_, _| Reply {
            outcome: Some(Ok(json!(true))),
            early: true,
            notifications: vec![Notification {
                subscription: "existing".into(),
                result: json!(1),
            }],
        });
        state.set_handler("second", |_, _| Reply {
            outcome: Some(Ok(json!("new"))),
            early: false,
            notifications: vec![
                Notification {
                    subscription: "existing".into(),
                    result: json!(2),
                },
                Notification {
                    subscription: "new".into(),
                    result: json!(3),
                },
            ],
        });
    });
    let mut first = chain.subscribe_raw("first", None, "unsub").await?;
    let _ = chain.request_raw("trigger", None).await?;
    assert_eq!(
        first
            .stream
            .next()
            .await
            .ok_or("missing request notification")??
            .get(),
        "1"
    );
    let mut second = chain.subscribe_raw("second", None, "unsub").await?;
    assert_eq!(
        first
            .stream
            .next()
            .await
            .ok_or("missing other subscription notification")??
            .get(),
        "2"
    );
    assert_eq!(
        second
            .stream
            .next()
            .await
            .ok_or("missing new subscription notification")??
            .get(),
        "3"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_concurrent_advancement_preserves_every_head_after_registration() -> Result {
    let chain = FakeChain::default();
    let rpc = RpcClient::new(chain.clone());
    let mut heads = rpc
        .subscribe::<Value>(
            "chain_subscribeNewHeads",
            rpc_params![],
            "chain_unsubscribeNewHeads",
        )
        .await?;
    let mut advances = Vec::new();
    for _ in 0..4 {
        let chain = chain.clone();
        advances.push(std::thread::spawn(move || {
            for _ in 0..25 {
                chain.advance(1)?;
                std::thread::yield_now();
            }
            Ok::<(), serde_json::Error>(())
        }));
    }
    for advance in advances {
        advance.join().map_err(|_| "advancement thread failed")??;
    }
    for number in 100..=200 {
        let head = tokio::time::timeout(Duration::from_secs(1), heads.next())
            .await?
            .ok_or("head subscription ended")??;
        assert_eq!(head["number"], format!("0x{number:x}"));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), heads.next())
            .await
            .is_err()
    );
    Ok(())
}
