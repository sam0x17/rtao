//! rtao client against rtao server over loopback QUIC.

#![cfg(all(feature = "client", feature = "server"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::{ConnectionError, Endpoint};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use rtao_transport_quic::QuicError;
use rtao_transport_quic::client::{QuicClient, QuicClientConfig, QuicTarget, authenticate};
use rtao_transport_quic::server::{
    ChunkSender, HandlerError, QuicRouter, QuicServer, QuicServerConfig, ServerStats, source_key,
};
use rtao_transport_quic::tls::{ServerIdentity, client_config, provider, transport};
use rtao_transport_quic::wire::{
    self, DataMap, MessageType, Value, decode, encode, read_frame, write_frame,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

fn signer(seed: u8) -> Arc<dyn Signer> {
    Arc::new(KeypairSigner::from_seed(&[seed; 32], CryptoType::Sr25519).unwrap())
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Add {
    a: i64,
    b: i64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Sum {
    sum: i64,
}

fn router() -> QuicRouter {
    QuicRouter::new()
        .unary_fn("echo", |_, data| async move { Ok(data) })
        .unary_fn("fail", |_, _| async move { Err(HandlerError::new("boom")) })
        .unary_fn("panic", |_, _| async move { panic!("handler panic") })
        .unary_fn("slow", |_, data| async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(data)
        })
        .unary_fn("whoami", |caller, _| async move {
            Ok(DataMap::from([(
                "hotkey".to_owned(),
                Value::Binary(caller.hotkey.0.to_vec()),
            )]))
        })
        .typed("add", |_, r: Add| async move { Ok(Sum { sum: r.a + r.b }) })
        .streaming_fn("stream", |_, _, tx: ChunkSender| async move {
            for i in 0..3u8 {
                tx.send(vec![i]).await.map_err(HandlerError::new)?;
            }
            Ok(())
        })
        .streaming_fn("stream_fail", |_, _, tx: ChunkSender| async move {
            tx.send(vec![1]).await.map_err(HandlerError::new)?;
            Err(HandlerError::new("boom"))
        })
        .streaming_fn("stream_slow", |_, _, tx: ChunkSender| async move {
            tx.send(vec![1]).await.map_err(HandlerError::new)?;
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        })
}

struct Running {
    addr: SocketAddr,
    miner: AccountId,
    fingerprint: [u8; 32],
    stats: ServerStats,
    stop: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn start(config: QuicServerConfig, deny: bool) -> Running {
    let mut builder = QuicServer::builder(signer(1), router()).config(config);
    if deny {
        builder = builder.policy(|_: &AccountId| false);
    }
    let server = builder.bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    let addr = server.local_addr().unwrap();
    let fingerprint = server.fingerprint();
    let stats = server.stats();
    let stop = CancellationToken::new();
    tokio::spawn(server.serve(stop.clone()));
    Running {
        addr,
        miner: signer(1).account(),
        fingerprint,
        stats,
        stop,
    }
}

fn fast() -> QuicServerConfig {
    QuicServerConfig {
        handler_timeout: Duration::from_millis(400),
        handshake_timeout: Duration::from_millis(500),
        ..Default::default()
    }
}

#[tokio::test]
async fn unary_streaming_and_errors() {
    let server = start(fast(), false);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let target = QuicTarget::new(server.miner, server.addr);

    let data = DataMap::from([("msg".to_owned(), Value::from("hi"))]);
    let reply = client
        .query(&target, "echo", data.clone(), None)
        .await
        .unwrap();
    assert!(reply.success);
    assert_eq!(reply.data, data);

    let reply = client
        .query(&target, "whoami", DataMap::new(), None)
        .await
        .unwrap();
    assert_eq!(
        reply.data["hotkey"],
        Value::Binary(signer(2).account().0.to_vec())
    );

    let sum: Sum = client
        .query_typed(&target, "add", &Add { a: 2, b: 3 }, None)
        .await
        .unwrap();
    assert_eq!(sum, Sum { sum: 5 });

    for (synapse, want) in [
        ("nope", "unrecognized synapse type"),
        ("fail", "request processing failed"),
        ("panic", "request processing failed"),
        ("slow", "handler timed out"),
    ] {
        let reply = client
            .query(&target, synapse, DataMap::new(), None)
            .await
            .unwrap();
        assert!(!reply.success);
        assert_eq!(reply.error.as_deref(), Some(want), "{synapse}");
        assert!(matches!(reply.into_result(), Err(QuicError::Handler(_))));
    }

    let chunks = client
        .query_stream(&target, "stream", DataMap::new())
        .await
        .unwrap()
        .collect_all()
        .await
        .unwrap();
    assert_eq!(chunks, vec![vec![0], vec![1], vec![2]]);

    for (synapse, want) in [
        ("stream_fail", "stream processing failed"),
        ("stream_slow", "handler timed out"),
    ] {
        let mut stream = client
            .query_stream(&target, synapse, DataMap::new())
            .await
            .unwrap();
        assert_eq!(stream.next_chunk().await.unwrap(), Some(vec![1]));
        let err = stream.next_chunk().await.unwrap_err();
        assert_eq!(err.to_string(), format!("stream error: {want}"));
    }

    let err = client
        .query_stream(&target, "nope", DataMap::new())
        .await
        .unwrap()
        .collect_all()
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("on streaming path: unrecognized synapse type")
    );

    let err = client
        .query_stream(&target, "echo", DataMap::new())
        .await
        .unwrap()
        .collect_all()
        .await;
    assert!(err.is_err());
    client.close().await;
}

#[tokio::test]
async fn handshake_identity_and_policy() {
    let server = start(fast(), false);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let wrong = QuicTarget::new(AccountId([9; 32]), server.addr);
    let err = client.connect(&wrong).await.unwrap_err();
    assert!(err.to_string().contains("miner hotkey mismatch"), "{err}");
    // The failed attempt starts a backoff for that address.
    assert!(matches!(
        client.connect(&wrong).await,
        Err(QuicError::Backoff(_))
    ));

    let pinned = QuicTarget {
        fingerprint: Some([0; 32]),
        ..QuicTarget::new(server.miner, server.addr)
    };
    let other = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    assert!(other.connect(&pinned).await.is_err());

    let denied = start(fast(), true);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let err = client
        .connect(&QuicTarget::new(denied.miner, denied.addr))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("rejected"), "{err}");
}

fn raw_endpoint() -> Endpoint {
    let mut endpoint = Endpoint::client(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    endpoint.set_default_client_config(
        client_config(
            None,
            transport(Duration::from_secs(150), Duration::from_secs(30)).unwrap(),
        )
        .unwrap(),
    );
    endpoint
}

async fn send_packet(connection: &quinn::Connection, synapse: &str) -> wire::SynapseResponse {
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    let packet = wire::SynapsePacket {
        synapse_type: synapse.into(),
        data: DataMap::new(),
        timestamp: 0,
    };
    write_frame(
        &mut send,
        MessageType::SynapsePacket,
        &encode(&packet).unwrap(),
    )
    .await
    .unwrap();
    send.finish().unwrap();
    let (kind, payload) = read_frame(&mut recv, 1 << 20).await.unwrap();
    assert_eq!(kind, MessageType::SynapseResponse);
    decode(&payload).unwrap()
}

#[tokio::test]
async fn authorization_is_per_connection() {
    let server = start(fast(), false);
    let endpoint = raw_endpoint();
    let a = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let b = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    authenticate(&a, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    assert!(send_packet(&a, "echo").await.success);
    // Same UDP source address, different connection: not authorized (A4).
    let r = send_packet(&b, "echo").await;
    assert_eq!(r.error.as_deref(), Some("authentication failed"));

    // A second, different validator identity on an authenticated connection is refused.
    let err = authenticate(&a, signer(3).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("rejected"), "{err}");
    // The same identity may handshake again.
    authenticate(&a, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();

    // A new connection for the same validator replaces the old one.
    let c = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    authenticate(&c, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(5), a.closed())
        .await
        .unwrap()
    {
        ConnectionError::ApplicationClosed(close) => assert_eq!(&close.reason[..], b"replaced"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn unauthenticated_connections_are_closed() {
    let server = start(fast(), false);
    let endpoint = raw_endpoint();
    let conn = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(5), conn.closed())
        .await
        .unwrap()
    {
        ConnectionError::ApplicationClosed(close) => {
            assert_eq!(&close.reason[..], b"handshake_timeout");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn preauth_cap_refuses() {
    let config = QuicServerConfig {
        max_preauth_per_ip: 1,
        ..fast()
    };
    let server = start(config, false);
    let endpoint = raw_endpoint();
    let first = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let second = endpoint.connect(server.addr, "localhost").unwrap().await;
    assert!(
        second.is_err(),
        "second pre-auth connection should be refused"
    );
    // Authenticating frees the slot.
    authenticate(&first, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    let third = endpoint.connect(server.addr, "localhost").unwrap().await;
    assert!(third.is_ok());
}

#[test]
fn config_validation() {
    let ok = QuicServerConfig::default();
    ok.validate().unwrap();
    for bad in [
        QuicServerConfig {
            max_signature_age_secs: 0,
            ..Default::default()
        },
        QuicServerConfig {
            max_signature_age_secs: 3601,
            ..Default::default()
        },
        QuicServerConfig {
            max_frame_payload: 1024 * 1024 - 1,
            ..Default::default()
        },
        QuicServerConfig {
            keep_alive: Duration::from_secs(150),
            ..Default::default()
        },
        QuicServerConfig {
            handler_timeout: Duration::ZERO,
            ..Default::default()
        },
        QuicServerConfig {
            handshake_timeout: Duration::from_secs(150),
            ..Default::default()
        },
        QuicServerConfig {
            max_connections: 0,
            ..Default::default()
        },
        QuicServerConfig {
            max_preauth_frame_payload: 1023,
            ..Default::default()
        },
        QuicServerConfig {
            max_preauth_frame_payload: 2 * 1024 * 1024,
            max_frame_payload: 1024 * 1024,
            ..Default::default()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    QuicClientConfig::default().validate().unwrap();
    for bad in [
        QuicClientConfig {
            max_frame_payload: 1024,
            ..Default::default()
        },
        QuicClientConfig {
            max_stream_payload: 1024,
            ..Default::default()
        },
        QuicClientConfig {
            keep_alive: Duration::from_secs(200),
            ..Default::default()
        },
        QuicClientConfig {
            connect_timeout: Duration::ZERO,
            ..Default::default()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}

#[test]
fn backoff_schedule() {
    let b = rtao_transport_quic::client::Backoff::default();
    let secs: Vec<u64> = (0..8).map(|n| b.delay(n).as_secs()).collect();
    assert_eq!(secs, vec![0, 1, 2, 4, 8, 16, 60, 60]);
}

#[test]
fn ed25519_signer_is_refused() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let ed: Arc<dyn Signer> =
            Arc::new(KeypairSigner::from_seed(&[1; 32], CryptoType::Ed25519).unwrap());
        let r = QuicServer::bind(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            ed,
            router(),
            QuicServerConfig::default(),
        );
        assert!(matches!(r, Err(QuicError::Config(_))));
    });
}

#[test]
fn axon_targets() {
    let hk = AccountId([1; 32]);
    let t = QuicTarget::from_axon(hk, 0x7f00_0001, 4, 8091).unwrap();
    assert_eq!(t.addr, "127.0.0.1:8091".parse().unwrap());
    assert!(QuicTarget::from_axon(hk, 0, 4, 1).is_none());
    assert!(QuicTarget::from_axon(hk, 1, 4, 0).is_none());
    assert!(QuicTarget::from_axon(hk, u128::from(u32::MAX) + 1, 4, 1).is_none());
    assert!(QuicTarget::from_axon(hk, 1, 6, 1).unwrap().addr.is_ipv6());
}

async fn dial(endpoint: &Endpoint, addr: SocketAddr) -> quinn::Connection {
    endpoint.connect(addr, "localhost").unwrap().await.unwrap()
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn close_reason(connection: &quinn::Connection) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .unwrap()
    {
        ConnectionError::ApplicationClosed(close) => close.reason.to_vec(),
        other => panic!("{other:?}"),
    }
}

/// Review finding 2: concurrent handshakes for two validators on one
/// connection bind only one of them, and closing the connection removes
/// every validator entry it registered.
#[tokio::test]
async fn concurrent_handshakes_bind_one_identity() {
    let server = start(fast(), false);
    let endpoint = raw_endpoint();
    for _ in 0..5 {
        let a = endpoint
            .connect(server.addr, "localhost")
            .unwrap()
            .await
            .unwrap();
        let (two, three) = (signer(2), signer(3));
        let (r2, r3) = tokio::join!(
            authenticate(&a, two.as_ref(), &server.miner, 1 << 20),
            authenticate(&a, three.as_ref(), &server.miner, 1 << 20),
        );
        assert!(
            r2.is_ok() != r3.is_ok(),
            "exactly one identity binds: {r2:?} {r3:?}"
        );
        let winner = if r2.is_ok() { two } else { three };
        assert_eq!(server.stats.authenticated_validators(), 1);
        // Requests are attributed to the identity that won.
        let (mut send, mut recv) = a.open_bi().await.unwrap();
        let packet = wire::SynapsePacket {
            synapse_type: "whoami".into(),
            data: DataMap::new(),
            timestamp: 0,
        };
        write_frame(
            &mut send,
            MessageType::SynapsePacket,
            &encode(&packet).unwrap(),
        )
        .await
        .unwrap();
        send.finish().unwrap();
        let (_, payload) = read_frame(&mut recv, 1 << 20).await.unwrap();
        let reply: wire::SynapseResponse = decode(&payload).unwrap();
        assert_eq!(
            reply.data["hotkey"],
            Value::Binary(winner.account().0.to_vec())
        );
        a.close(0u32.into(), b"bye");
        wait_for("validator entry removal", || {
            server.stats.authenticated_validators() == 0
        })
        .await;
    }
}

/// Review finding 3: a full pre-authentication table closes the oldest
/// unauthenticated connection of the busiest source instead of refusing.
#[tokio::test]
async fn preauth_cap_evicts_oldest() {
    let config = QuicServerConfig {
        max_preauth_connections: 2,
        handshake_timeout: Duration::from_secs(5),
        ..fast()
    };
    let server = start(config, false);
    let endpoint = raw_endpoint();
    let first = dial(&endpoint, server.addr).await;
    let second = dial(&endpoint, server.addr).await;
    wait_for("two pre-auth connections", || {
        server.stats.preauth_connections() == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let third = dial(&endpoint, server.addr).await;
    assert_eq!(close_reason(&first).await, b"preauth_evicted");
    assert!(second.close_reason().is_none());
    assert_eq!(server.stats.preauth_evictions(), 1);
    assert_eq!(server.stats.preauth_refusals(), 0);
    // The newcomer can authenticate and use the server.
    authenticate(&third, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    assert!(send_packet(&third, "echo").await.success);
}

#[test]
fn source_keys_group_ipv6_by_prefix() {
    let key = |s: &str| source_key(s.parse().unwrap());
    assert_eq!(key("192.0.2.7"), key("192.0.2.7"));
    assert_ne!(key("192.0.2.7"), key("192.0.2.8"));
    assert_eq!(key("2001:db8:1:2:aaaa::1"), key("2001:db8:1:2:ffff::9"));
    assert_eq!(
        key("2001:db8:1:2:aaaa::1"),
        "2001:db8:1:2::".parse::<std::net::IpAddr>().unwrap()
    );
    assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    assert_eq!(key("::ffff:192.0.2.7"), key("192.0.2.7"));
}

/// Review finding 1: a large first frame on an unauthenticated connection
/// is answered without waiting for (or buffering) its payload.
#[tokio::test]
async fn oversized_preauth_frames_are_answered_unread() {
    let server = start(fast(), false);
    let endpoint = raw_endpoint();
    let conn = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    for kind in [MessageType::SynapsePacket, MessageType::HandshakeRequest] {
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let mut header = vec![kind as u8];
        header.extend_from_slice(&(1024u32 * 1024).to_be_bytes());
        tokio::io::AsyncWriteExt::write_all(&mut send, &header)
            .await
            .unwrap();
        let (got, payload) =
            tokio::time::timeout(Duration::from_millis(300), read_frame(&mut recv, 1 << 20))
                .await
                .expect("answered before the payload arrives")
                .unwrap();
        match got {
            MessageType::SynapseResponse => {
                let r: wire::SynapseResponse = decode(&payload).unwrap();
                assert_eq!(r.error.as_deref(), Some("authentication failed"));
            }
            MessageType::HandshakeResponse => {
                let r: wire::HandshakeResponse = decode(&payload).unwrap();
                assert!(!r.accepted);
                assert_eq!(r.miner_hotkey, "");
            }
            other => panic!("{other:?}"),
        }
    }
    // After authenticating, the same frame size is read normally.
    authenticate(&conn, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    let big = DataMap::from([("b".to_owned(), Value::Binary(vec![7; 200_000]))]);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let reply = client
        .query(
            &QuicTarget::new(server.miner, server.addr),
            "echo",
            big.clone(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(reply.data, big);
}

/// Review finding 7: a reply the peer never grants flow control for is
/// reset after its write budget instead of holding the task forever.
#[tokio::test]
async fn stalled_reply_writes_are_reset() {
    let server = start(
        QuicServerConfig {
            handshake_timeout: Duration::from_secs(5),
            ..Default::default()
        },
        false,
    );
    let mut tight = transport(Duration::from_secs(150), Duration::from_secs(30)).unwrap();
    tight.stream_receive_window(quinn::VarInt::from_u32(4096));
    let mut endpoint = Endpoint::client(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    endpoint.set_default_client_config(client_config(None, tight).unwrap());
    let conn = endpoint
        .connect(server.addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    authenticate(&conn, signer(2).as_ref(), &server.miner, 1 << 20)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let packet = wire::SynapsePacket {
        synapse_type: "echo".into(),
        data: DataMap::from([("b".to_owned(), Value::Binary(vec![7; 200_000]))]),
        timestamp: 0,
    };
    write_frame(
        &mut send,
        MessageType::SynapsePacket,
        &encode(&packet).unwrap(),
    )
    .await
    .unwrap();
    send.finish().unwrap();
    // Read nothing (granting no credit) for longer than the 6 s budget.
    tokio::time::sleep(Duration::from_secs(8)).await;
    let err = read_frame(&mut recv, 1 << 20).await.unwrap_err();
    assert!(err.to_string().contains("reset"), "{err}");
}

/// A server that presents certificate `cert` but signs with `key`.
#[derive(Debug)]
struct FixedCert(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for FixedCert {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

fn tls_server(cert: &ServerIdentity, key: &ServerIdentity) -> Endpoint {
    let signing = provider()
        .key_provider
        .load_private_key(key.key.clone_key())
        .unwrap();
    let certified = rustls::sign::CertifiedKey::new(vec![cert.cert.clone()], signing);
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedCert(Arc::new(certified))));
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
    let config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let endpoint = Endpoint::server(config, SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    let acceptor = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = acceptor.accept().await {
            tokio::spawn(async move {
                if let Ok(connecting) = incoming.accept() {
                    let _ = connecting.await;
                }
            });
        }
    });
    endpoint
}

/// Review finding 10: the client checks the TLS 1.3 CertificateVerify
/// signature, so a relay presenting a miner's certificate without its key
/// cannot complete the TLS handshake.
#[tokio::test]
async fn certificate_without_its_key_is_rejected() {
    let a = ServerIdentity::generate().unwrap();
    let b = ServerIdentity::generate().unwrap();
    let client = raw_endpoint();

    let honest = tls_server(&a, &a);
    let ok = client
        .connect(honest.local_addr().unwrap(), "localhost")
        .unwrap()
        .await;
    assert!(ok.is_ok(), "control: {ok:?}");

    let relay = tls_server(&a, &b);
    let err = client
        .connect(relay.local_addr().unwrap(), "localhost")
        .unwrap()
        .await
        .unwrap_err();
    assert!(matches!(err, ConnectionError::TransportError(_)), "{err:?}");
    assert!(
        err.to_string().to_lowercase().contains("signature"),
        "{err}"
    );
}

/// Review finding 5: addresses whose dial failed do not use up the pool.
#[tokio::test]
async fn failed_dials_free_pool_slots() {
    let server = start(fast(), false);
    // Bound but silent UDP sockets: dials to them time out.
    let silent: Vec<std::net::UdpSocket> = (0..2)
        .map(|_| std::net::UdpSocket::bind("127.0.0.1:0").unwrap())
        .collect();
    let config = QuicClientConfig {
        max_connections: 2,
        connect_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let client = QuicClient::new(signer(2), config.clone()).unwrap();
    for socket in &silent {
        let dead = QuicTarget::new(server.miner, socket.local_addr().unwrap());
        assert!(matches!(
            client.connect(&dead).await,
            Err(QuicError::Timeout("connect"))
        ));
    }
    client
        .connect(&QuicTarget::new(server.miner, server.addr))
        .await
        .unwrap();

    // Live connections still count against the limit.
    let other = start(fast(), false);
    let client = QuicClient::new(
        signer(2),
        QuicClientConfig {
            max_connections: 1,
            ..config
        },
    )
    .unwrap();
    client
        .connect(&QuicTarget::new(server.miner, server.addr))
        .await
        .unwrap();
    let err = client
        .connect(&QuicTarget::new(other.miner, other.addr))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("connection limit 1 reached"),
        "{err}"
    );
}

/// Review finding 6: a pinned target never reuses a pooled connection whose
/// certificate does not match the pin.
#[tokio::test]
async fn pins_apply_to_pooled_connections() {
    let server = start(fast(), false);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let plain = QuicTarget::new(server.miner, server.addr);
    client.connect(&plain).await.unwrap();
    let wrong = QuicTarget {
        fingerprint: Some([0; 32]),
        ..plain.clone()
    };
    let err = client.connect(&wrong).await.unwrap_err();
    assert!(err.to_string().contains("pinned fingerprint"), "{err}");
    let right = QuicTarget {
        fingerprint: Some(server.fingerprint),
        ..plain
    };
    client.connect(&right).await.unwrap();
}

/// A miner hotkey that leaves an address is forgotten by `retain_targets`.
#[tokio::test]
async fn retain_targets_forgets_replaced_hotkeys() {
    let server = start(fast(), false);
    let client = QuicClient::new(signer(2), QuicClientConfig::default()).unwrap();
    let target = QuicTarget::new(server.miner, server.addr);
    let connection = client.connect(&target).await.unwrap();
    client.retain_targets(std::slice::from_ref(&target)).await;
    assert!(connection.close_reason().is_none());
    client
        .retain_targets(&[QuicTarget::new(AccountId([9; 32]), server.addr)])
        .await;
    assert!(connection.close_reason().is_some());
    // The listed hotkey is dialled afresh (and here fails its identity check).
    let err = client
        .connect(&QuicTarget::new(AccountId([9; 32]), server.addr))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("miner hotkey mismatch"), "{err}");
}
