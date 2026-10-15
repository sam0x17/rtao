//! Socket regressions for transport lifecycle and connection-scoped identities.
use futures::{SinkExt, StreamExt};
use rtao_rpc::{ReadMethod, RpcError, Session, SessionConfig, SessionState, SubscriptionKind};
use rtao_testkit::{FakeNode, FakeState, Fault, FaultPlan, Reply};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use url::Url;
type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
fn config(node: &FakeNode) -> Result<SessionConfig> {
    Ok(SessionConfig {
        silence_timeout: Duration::from_millis(200),
        ..SessionConfig::new(Url::parse(node.url())?)
    })
}
async fn wait(mut ready: impl FnMut() -> bool) -> Result {
    timeout(Duration::from_secs(2), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}
fn result(node: &FakeNode, method: &str, value: Value) {
    node.mutate_state(|state| state.set_result(method, value));
}

#[tokio::test]
async fn idle_connection_starts_a_fresh_deadline_for_submission() -> Result {
    let node = FakeNode::with_faults(
        Arc::new(Mutex::new(FakeState::default())),
        FaultPlan::new(vec![Fault::SlowResponse(Duration::from_millis(40))]),
    )
    .await?;
    let hash = format!("0x{}", "11".repeat(32));
    result(&node, "author_submitExtrinsic", json!(hash));
    let session = Session::connect(config(&node)?).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let submitted = timeout(Duration::from_secs(1), session.submit(&[1, 2, 3])).await??;
    assert_eq!(format!("{submitted:#x}"), hash);
    assert_eq!(node.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn stale_subscription_owners_cannot_cancel_reused_identity() -> Result {
    let node = FakeNode::start().await?;
    result(&node, "chain_subscribeNewHeads", json!("0"));
    result(&node, "chain_unsubscribeNewHeads", json!(true));
    let session = Session::connect(config(&node)?).await?;
    let old = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    node.drop_connection()?;
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    let mut states = session.state();
    timeout(
        Duration::from_secs(2),
        states.wait_for(|state| matches!(state, SessionState::Connected { .. })),
    )
    .await??;
    let mut current = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    drop(old);
    let _: Value = session.request(ReadMethod::SystemHealth, json!([])).await?;
    assert_eq!(session.statistics().await?.subscriptions, 1);
    node.push_subscription_update("0", json!({"number":7}))?;
    assert_eq!(
        timeout(Duration::from_secs(1), current.next())
            .await?
            .ok_or("current subscription ended")??,
        json!({"number":7})
    );
    assert!(
        !node
            .requests()
            .iter()
            .any(|request| request.method == "chain_unsubscribeNewHeads")
    );
    node.drop_connection()?;
    timeout(Duration::from_secs(2), node.wait_for_connections(3)).await?;
    timeout(
        Duration::from_secs(2),
        states.wait_for(|state| matches!(state, SessionState::Connected { .. })),
    )
    .await??;
    let mut newest = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    current.unsubscribe().await;
    let _: Value = session.request(ReadMethod::SystemHealth, json!([])).await?;
    assert_eq!(session.statistics().await?.subscriptions, 1);
    node.push_subscription_update("0", json!({"number":8}))?;
    assert_eq!(
        timeout(Duration::from_secs(1), newest.next())
            .await?
            .ok_or("newest subscription ended")??,
        json!({"number":8})
    );
    assert!(
        !node
            .requests()
            .iter()
            .any(|request| request.method == "chain_unsubscribeNewHeads")
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn old_closed_identity_does_not_discard_new_early_notification() -> Result {
    let node = FakeNode::start().await?;
    result(&node, "chain_subscribeNewHeads", json!("0"));
    result(&node, "chain_unsubscribeNewHeads", json!(true));
    let session = Session::connect(config(&node)?).await?;
    let mut old = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    old.unsubscribe().await;
    assert_eq!(session.statistics().await?.closed_subscription_ids, 1);
    node.drop_connection()?;
    timeout(Duration::from_secs(2), node.wait_for_connections(2)).await?;
    let mut states = session.state();
    timeout(
        Duration::from_secs(2),
        states.wait_for(|state| matches!(state, SessionState::Connected { .. })),
    )
    .await??;
    node.mutate_state(|state| {
        state.queue_reply(
            "chain_subscribeNewHeads",
            Reply::subscription("0", vec![json!({"number":9})], true),
        )
    });
    let mut new = session
        .subscribe::<Value>(SubscriptionKind::NewHeads)
        .await?;
    assert_eq!(
        timeout(Duration::from_secs(1), new.next())
            .await?
            .ok_or("new subscription ended")??,
        json!({"number":9})
    );
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn close_releases_initialized_archive_while_session_handle_is_retained() -> Result {
    let primary = FakeNode::start().await?;
    let archive = FakeNode::start().await?;
    primary.mutate_state(|state| {
        state.set_handler("state_getStorage", |_, _| {
            Reply::error(4003, "State already discarded for 0xabc", None)
        })
    });
    result(&archive, "state_getStorage", json!("0x01"));
    let mut cfg = config(&primary)?;
    cfg.pool.archive.push(Url::parse(archive.url())?);
    let session = Session::connect(cfg).await?;
    assert_eq!(
        session
            .request::<String>(ReadMethod::StateGetStorage, json!(["0xkey", "0xabc"]))
            .await?,
        "0x01"
    );
    timeout(Duration::from_secs(1), session.close()).await?;
    timeout(Duration::from_secs(1), primary.wait_for_closed(1)).await?;
    timeout(Duration::from_secs(1), archive.wait_for_closed(1)).await?;
    assert!(matches!(&*session.state().borrow(), SessionState::Closed));
    assert!(matches!(
        session
            .archive()
            .request::<Value>(ReadMethod::SystemHealth, json!([]))
            .await,
        Err(RpcError::Closed)
    ));
    Ok(())
}

#[tokio::test]
async fn close_cancels_archive_connection_initialization() -> Result {
    let primary = FakeNode::start().await?;
    let archive = FakeNode::start().await?;
    archive.hold_handshakes(true);
    primary.mutate_state(|state| {
        state.set_handler("state_getStorage", |_, _| {
            Reply::error(4003, "State already discarded for 0xabc", None)
        })
    });
    let mut cfg = config(&primary)?;
    cfg.pool.archive.push(Url::parse(archive.url())?);
    let session = Session::connect(cfg).await?;
    let request = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<Value>(ReadMethod::StateGetStorage, json!(["0xkey", "0xabc"]))
                .await
        })
    };
    wait(|| archive.connect_attempts() == 1).await?;
    timeout(Duration::from_secs(1), session.close()).await?;
    assert!(matches!(
        timeout(Duration::from_secs(1), request).await??,
        Err(RpcError::Closed)
    ));
    archive.hold_handshakes(false);
    timeout(Duration::from_secs(1), archive.wait_for_closed(1)).await?;
    Ok(())
}

#[tokio::test]
async fn backpressured_socket_write_does_not_stall_close_or_owner_drop() -> Result {
    for explicit_close in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (ready, ready_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut socket = accept_async(socket).await?;
            let discovery = socket.next().await.ok_or("discovery not received")??;
            assert!(discovery.to_text()?.contains("rpc_methods"));
            let _ = ready.send(());
            let _ = release_rx.await;
            loop {
                match socket.next().await {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        });
        let cfg = SessionConfig {
            silence_timeout: Duration::from_secs(30),
            ..SessionConfig::new(Url::parse(&format!("ws://{address}"))?)
        };
        let session = Session::connect(cfg).await?;
        ready_rx.await?;
        let request = {
            let session = session.clone();
            tokio::spawn(async move {
                session
                    .request::<Value>(
                        ReadMethod::StateCall,
                        json!(["big", "a".repeat(32 * 1024 * 1024)]),
                    )
                    .await
            })
        };
        wait(|| !request.is_finished()).await?;
        timeout(Duration::from_secs(2), async {
            while session.statistics().await?.pending == 0 {
                tokio::task::yield_now().await;
            }
            Ok::<(), RpcError>(())
        })
        .await??;
        // Let the socket consume the available TCP send buffer; the peer has
        // stopped reading after discovery and cannot consume the large frame.
        tokio::time::sleep(Duration::from_millis(40)).await;
        if explicit_close {
            timeout(Duration::from_secs(1), session.close()).await?;
            assert!(matches!(
                timeout(Duration::from_secs(1), request).await??,
                Err(RpcError::Closed)
            ));
        } else {
            request.abort();
            let _ = request.await;
            drop(session);
        }
        let _ = release.send(());
        timeout(Duration::from_secs(1), server).await???;
    }
    Ok(())
}

#[tokio::test]
async fn binary_discovery_does_not_reset_unanswered_request_retry_budget() -> Result {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (socket, _) = listener.accept().await?;
            let mut socket = accept_async(socket).await?;
            while let Some(frame) = socket.next().await {
                let frame = frame?;
                if frame.is_close() {
                    break;
                }
                if let Ok(text) = frame.to_text() {
                    let request: Value = serde_json::from_str(text)?;
                    if request.get("method").and_then(Value::as_str) == Some("rpc_methods") {
                        let response = json!({"jsonrpc":"2.0","id":0,"result":{"methods":["rpc_methods","system_chain"]}});
                        socket
                            .send(Message::Binary(response.to_string().into_bytes().into()))
                            .await?;
                    }
                }
            }
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    });
    let cfg = SessionConfig {
        max_retries: 2,
        silence_timeout: Duration::from_millis(40),
        ..SessionConfig::new(Url::parse(&format!("ws://{address}"))?)
    };
    let session = Session::connect(cfg).await?;
    let error = timeout(
        Duration::from_secs(2),
        session.request::<Value>(ReadMethod::SystemChain, json!([])),
    )
    .await?
    .err()
    .ok_or("silent request answered")?;
    assert!(matches!(error, RpcError::MaxRetriesExceeded { .. }));
    timeout(Duration::from_secs(1), server).await???;
    session.close().await;
    Ok(())
}

#[cfg(feature = "rpc-rate-limit")]
#[tokio::test]
async fn reconnect_retransmissions_consume_tokens_before_connected_gate_opens() -> Result {
    let node = FakeNode::start().await?;
    let times = Arc::new(Mutex::new(Vec::<std::time::Instant>::new()));
    let recorded = times.clone();
    node.mutate_state(|state| {
        state.set_handler("system_chain", move |_, request| {
            let Ok(mut times) = recorded.lock() else {
                return Reply::error(1, "time capture poisoned", None);
            };
            times.push(std::time::Instant::now());
            if times.len() <= 2 {
                Reply::silent()
            } else {
                Reply::result(request.params[0].clone())
            }
        });
    });
    let mut cfg = config(&node)?;
    cfg.silence_timeout = Duration::from_secs(3);
    cfg.rate_limit = Some(rtao_rpc::RateLimit {
        per_second: 4.0,
        burst: 3,
    });
    let session = Session::connect(cfg).await?;
    let first = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([1]))
                .await
        })
    };
    let second = {
        let session = session.clone();
        tokio::spawn(async move {
            session
                .request::<u64>(ReadMethod::SystemChain, json!([2]))
                .await
        })
    };
    wait(|| times.lock().map(|times| times.len() == 2).unwrap_or(false)).await?;
    node.drop_connection()?;
    assert_eq!(timeout(Duration::from_secs(3), first).await???, 1);
    assert_eq!(timeout(Duration::from_secs(3), second).await???, 2);
    {
        let captured = times.lock().map_err(|_| "time capture poisoned")?;
        assert_eq!(captured.len(), 4);
        assert!(
            captured[3].duration_since(captured[2]) >= Duration::from_millis(180),
            "replays bypassed token spacing"
        );
    }
    // The diagnostic request follows the replays and consumes its own token;
    // until that completes the supervisor does not publish Connected.
    let mut states = session.state();
    timeout(
        Duration::from_secs(1),
        states.wait_for(|state| matches!(state, SessionState::Connected { .. })),
    )
    .await??;
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn raw_adapter_and_failed_batch_members_retry_archive_once() -> Result {
    use subxt_rpcs::client::RpcClientT;
    let primary = FakeNode::start().await?;
    let archive = FakeNode::start().await?;
    primary.mutate_state(|state| {
        state.set_handler("state_getStorage", |_, _| {
            Reply::error(4003, "State already discarded for 0xabc", None)
        })
    });
    result(&primary, "system_chain", json!("Bittensor"));
    result(&archive, "state_getStorage", json!("0x01"));
    let mut cfg = config(&primary)?;
    cfg.pool.archive.push(Url::parse(archive.url())?);
    let session = Session::connect(cfg).await?;
    let raw = session
        .request_raw(
            "state_getStorage",
            Some(serde_json::value::to_raw_value(&json!(["0xkey", "0xabc"]))?),
        )
        .await?;
    assert_eq!(serde_json::from_str::<String>(raw.get())?, "0x01");
    let replies = session
        .request_many(vec![
            (ReadMethod::StateGetStorage, json!(["0xkey", "0xabc"])),
            (ReadMethod::SystemChain, json!([])),
        ])
        .await?;
    let mut replies = replies.into_iter();
    assert_eq!(
        serde_json::from_str::<String>(replies.next().ok_or("missing storage reply")??.get())?,
        "0x01"
    );
    assert_eq!(
        serde_json::from_str::<String>(replies.next().ok_or("missing chain reply")??.get())?,
        "Bittensor"
    );
    assert!(replies.next().is_none());
    assert_eq!(
        archive
            .requests()
            .iter()
            .filter(|request| request.method == "state_getStorage")
            .count(),
        2
    );
    assert_eq!(archive.connect_attempts(), 1);
    session.close().await;
    Ok(())
}

#[tokio::test]
async fn fragmented_handshake_refusal_preserves_details_over_ws_and_trusted_wss() -> Result {
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    async fn reject<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, chunked: bool) -> Result {
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).await? == 0 || request.len() >= 8192 {
                return Err("incomplete test upgrade request".into());
            }
            request.push(byte[0]);
        }
        let body = br#"{"message":"deployment blocked"}"#;
        let framing = if chunked {
            "Transfer-Encoding: chunked\r\n".to_string()
        } else {
            format!("Content-Length: {}\r\n", body.len())
        };
        stream.write_all(format!("HTTP/1.1 403 Forbidden\r\n{framing}X-RateLimit-Policy: first\r\nX-RateLimit-Policy: second\r\nRetry-After: 12\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        stream.flush().await?;
        // The deterministic generic-helper regression supplies the strict read
        // barrier. This socket fixture additionally exercises actual TLS state.
        tokio::time::sleep(Duration::from_millis(40)).await;
        if chunked {
            stream
                .write_all(format!("{:x};fixture=yes\r\n", body.len()).as_bytes())
                .await?;
        }
        for fragment in body.chunks(3) {
            stream.write_all(fragment).await?;
        }
        if chunked {
            stream.write_all(b"\r\n0\r\n\r\n").await?;
        }
        stream.shutdown().await?;
        Ok(())
    }
    for secure in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let certified =
            rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
        let root = certified.cert.der().clone();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()),
        );
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![root.clone()], key)?;
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await?;
            if secure {
                let stream = tokio_rustls::TlsAcceptor::from(Arc::new(tls))
                    .accept(tcp)
                    .await?;
                reject(stream, true).await
            } else {
                reject(tcp, false).await
            }
        });
        let fallback = FakeNode::start().await?;
        let url = Url::parse(&format!(
            "{}://{address}",
            if secure { "wss" } else { "ws" }
        ))?;
        let mut cfg = SessionConfig::new(url);
        cfg.pool.urls.push(Url::parse(fallback.url())?);
        cfg.retry_forever = true;
        cfg.handshake_timeout = Duration::from_secs(1);
        if secure {
            cfg.extra_roots.push(root);
        }
        let error = timeout(Duration::from_secs(2), Session::connect(cfg))
            .await?
            .err()
            .ok_or("HTTP refusal unexpectedly connected")?;
        let RpcError::Policy(refusal) = error else {
            return Err(format!("unexpected refusal: {error}").into());
        };
        assert_eq!(refusal.status_code, Some(403));
        assert_eq!(refusal.policy.as_deref(), Some("first"));
        assert_eq!(refusal.retry_after.as_deref(), Some("12"));
        assert_eq!(
            refusal.message,
            "RPC endpoint rejected this source under its access policy (HTTP 403): deployment blocked (policy=first)"
        );
        assert_eq!(fallback.connect_attempts(), 0);
        server.await??;
    }
    Ok(())
}
