//! Interop peer for btlightning compatibility checks.
//!
//! ```text
//! quic_peer serve   --seed-hex HEX [--port P] [--handler-timeout-ms N] [--deny-all]
//! quic_peer capture --seed-hex HEX --out FILE [--port P]
//! quic_peer suite   --seed-hex HEX --addr IP:PORT --hotkey SS58 [--rate-addr IP:PORT] [--capture FILE]
//! ```
//!
//! `serve` prints `LISTEN <port> <fingerprint> <hotkey>` and serves until
//! stdin closes. `capture` answers one handshake and records its raw bytes.
//! `suite` runs the client cases and the raw wire cases, printing one JSON
//! object per case and exiting non-zero if any case fails.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::{Connection, Endpoint};
use rtao_primitives::CryptoType;
use rtao_signer::{KeypairSigner, Signer};
use rtao_transport_quic::auth::{
    self, HandshakeVerifier, fingerprint_b64, parse_ss58, sign_handshake_response,
    sign_handshake_with_nonce,
};
use rtao_transport_quic::client::{QuicClient, QuicClientConfig, QuicTarget};
use rtao_transport_quic::server::{
    ChunkSender, HandlerError, QuicRouter, QuicServer, QuicServerConfig,
};
use rtao_transport_quic::tls::{
    ServerIdentity, client_config, peer_fingerprint, server_config, transport,
};
use rtao_transport_quic::wire::{
    self, DataMap, HandshakeRequest, HandshakeResponse, MessageType, Value, decode, encode,
    read_frame, unix_now, write_frame,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn args() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut it = std::env::args().skip(2);
    while let Some(k) = it.next() {
        let k = k.trim_start_matches("--").to_owned();
        match k.as_str() {
            "deny-all" => {
                out.insert(k, "1".into());
            }
            _ => {
                out.insert(k, it.next().unwrap_or_default());
            }
        }
    }
    out
}

fn signer(a: &HashMap<String, String>) -> Arc<dyn Signer> {
    let seed = hex::decode(&a["seed-hex"]).expect("seed hex");
    Arc::new(KeypairSigner::from_seed(&seed, CryptoType::Sr25519).expect("seed"))
}

#[derive(Deserialize)]
struct AddReq {
    a: i64,
    b: i64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct AddResp {
    sum: i64,
}

fn router() -> QuicRouter {
    QuicRouter::new()
        .unary_fn("echo", |_, data| async move { Ok(data) })
        .unary_fn("fail", |_, _| async move { Err(HandlerError::new("boom")) })
        .unary_fn("slow", |_, data: DataMap| async move {
            let ms = data.get("ms").and_then(Value::as_u64).unwrap_or(3000);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(data)
        })
        .unary_fn("whoami", |caller, _| async move {
            let mut out = DataMap::new();
            out.insert(
                "hotkey".into(),
                Value::from(caller.hotkey.to_ss58(42).as_str()),
            );
            Ok(out)
        })
        .typed("add", |_, r: AddReq| async move {
            Ok(AddResp { sum: r.a + r.b })
        })
        .streaming_fn("stream", |_, data: DataMap, tx: ChunkSender| async move {
            let n = data.get("n").and_then(Value::as_u64).unwrap_or(3);
            for i in 1..=n {
                tx.send(format!("chunk-{i}").into_bytes())
                    .await
                    .map_err(HandlerError::new)?;
            }
            Ok(())
        })
        .streaming_fn("stream_fail", |_, _, tx: ChunkSender| async move {
            for i in 1..=2 {
                tx.send(format!("chunk-{i}").into_bytes())
                    .await
                    .map_err(HandlerError::new)?;
            }
            Err(HandlerError::new("mid-stream failure"))
        })
}

async fn serve(a: HashMap<String, String>) -> Res<()> {
    let port: u16 = a.get("port").map(|p| p.parse()).transpose()?.unwrap_or(0);
    let mut config = QuicServerConfig::default();
    if let Some(ms) = a.get("handler-timeout-ms") {
        config.handler_timeout = Duration::from_millis(ms.parse()?);
    }
    let mut builder = QuicServer::builder(signer(&a), router()).config(config);
    if a.contains_key("deny-all") {
        builder = builder.policy(|_: &rtao_primitives::AccountId| false);
    }
    let server = builder.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    println!(
        "LISTEN {} {} {}",
        server.local_addr()?.port(),
        fingerprint_b64(&server.fingerprint()),
        server.miner_hotkey()
    );
    std::io::stdout().flush()?;
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(_)) = lines.next_line().await {}
        stop.cancel();
    });
    server.serve(shutdown).await?;
    Ok(())
}

/// Answer exactly one handshake with a minimal raw server and record the
/// request frame and certificate fingerprint as a fixture.
async fn capture(a: HashMap<String, String>) -> Res<()> {
    let port: u16 = a.get("port").map(|p| p.parse()).transpose()?.unwrap_or(0);
    let signer = signer(&a);
    let identity = ServerIdentity::generate()?;
    let fp_b64 = fingerprint_b64(&identity.fingerprint);
    let config = server_config(
        &identity,
        transport(Duration::from_secs(150), Duration::from_secs(30))?,
        128,
    )?;
    let endpoint = Endpoint::server(config, SocketAddr::from(([127, 0, 0, 1], port)))?;
    println!(
        "LISTEN {} {} {}",
        endpoint.local_addr()?.port(),
        fp_b64,
        auth::signer_ss58(signer.as_ref())
    );
    std::io::stdout().flush()?;
    let connection = loop {
        let incoming = endpoint.accept().await.ok_or("endpoint closed")?;
        if !incoming.remote_address_validated() && incoming.may_retry() {
            incoming.retry()?;
            continue;
        }
        break incoming.accept()?.await?;
    };
    let (mut send, mut recv) = connection.accept_bi().await?;
    let (kind, payload) = read_frame(&mut recv, wire::DEFAULT_MAX_FRAME_PAYLOAD).await?;
    assert_eq!(kind, MessageType::HandshakeRequest);
    let now = unix_now();
    let request: HandshakeRequest = decode(&payload)?;
    let verifier = HandshakeVerifier::new(
        300,
        Arc::new(rtao_btauth::InMemoryNonceStore::with_config(
            rtao_btauth::NonceStoreConfig {
                retention: HandshakeVerifier::required_retention(300),
                ..Default::default()
            },
        )?),
    )?;
    let verified = verifier.verify(&request, &fp_b64, now, |_| true).await;
    // Accept only a request rtao verifies, so the Python client's view of
    // the handshake reflects rtao's verdict.
    let response = match &verified {
        Ok(_) => {
            sign_handshake_response(
                signer.as_ref(),
                &request,
                &fp_b64,
                now,
                format!("conn_{}_{}", request.validator_hotkey, now * 1000),
            )
            .await?
        }
        Err(_) => HandshakeResponse::rejected(&auth::signer_ss58(signer.as_ref()), now),
    };
    write_frame(
        &mut send,
        MessageType::HandshakeResponse,
        &encode(&response)?,
    )
    .await?;
    send.finish()?;
    let fixture = json!({
        "source": "btlightning-py client (53c090b) HandshakeRequest",
        "server_fingerprint_b64": fp_b64,
        "received_at": now,
        "request_frame_hex": hex::encode(wire::encode_frame(kind, &payload)?),
        "rtao_verified": verified.is_ok(),
        "rtao_reject_reason": verified.err().map(|e| e.to_string()),
    });
    std::fs::write(&a["out"], serde_json::to_string_pretty(&fixture)?)?;
    // Let the client finish reading before closing.
    tokio::time::sleep(Duration::from_millis(500)).await;
    connection.close(0u32.into(), b"done");
    Ok(())
}

struct Suite {
    failures: usize,
}

impl Suite {
    fn report(&mut self, case: &str, result: Result<String, String>) {
        let (ok, detail) = match result {
            Ok(d) => (true, d),
            Err(d) => {
                self.failures += 1;
                (false, d)
            }
        };
        println!("{}", json!({"case": case, "ok": ok, "detail": detail}));
        let _ = std::io::stdout().flush();
    }
}

fn check(cond: bool, detail: impl Into<String>) -> Result<String, String> {
    if cond {
        Ok("ok".into())
    } else {
        Err(detail.into())
    }
}

fn value_matrix() -> Vec<(&'static str, Value)> {
    vec![
        ("none", Value::Nil),
        ("true", Value::Boolean(true)),
        ("false", Value::Boolean(false)),
        ("zero", Value::from(0)),
        ("int", Value::from(42)),
        ("neg", Value::from(-7)),
        ("float", Value::F64(2.5)),
        ("f32", Value::F32(1.5)),
        ("str", Value::from("hello")),
        ("empty", Value::from("")),
        ("bytes", Value::Binary(vec![0xde, 0xad, 0xbe, 0xef])),
        (
            "list",
            Value::Array(vec![Value::from(1), Value::from(2), Value::from(3)]),
        ),
        (
            "nested",
            Value::Map(vec![(
                Value::from("nested"),
                Value::Map(vec![(Value::from("key"), Value::from("val"))]),
            )]),
        ),
        ("u64max", Value::from(u64::MAX)),
        ("i64min", Value::from(i64::MIN)),
    ]
}

/// Python echo turns F32 into a float, which returns as F64.
fn same_value(sent: &Value, got: &Value) -> bool {
    match (sent, got) {
        (Value::F32(a), Value::F64(b)) => f64::from(*a) == *b,
        _ => sent == got,
    }
}

async fn raw_endpoint() -> Res<Endpoint> {
    let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(client_config(
        None,
        transport(Duration::from_secs(150), Duration::from_secs(30))?,
    )?);
    Ok(endpoint)
}

async fn raw_connect(endpoint: &Endpoint, addr: SocketAddr) -> Res<(Connection, String)> {
    let connection = endpoint.connect(addr, &addr.ip().to_string())?.await?;
    let fp = peer_fingerprint(&connection).ok_or("no certificate")?;
    Ok((connection, fingerprint_b64(&fp)))
}

/// Send one frame on a fresh stream and read one reply frame (or report EOF).
async fn exchange(
    connection: &Connection,
    kind: u8,
    payload: &[u8],
) -> Res<Option<(MessageType, Vec<u8>)>> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let mut frame = vec![kind];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    let _ = send.write_all(&frame).await;
    let _ = send.finish();
    match tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut recv, wire::DEFAULT_MAX_FRAME_PAYLOAD),
    )
    .await
    {
        Ok(Ok(frame)) => Ok(Some(frame)),
        Ok(Err(_)) => Ok(None),
        Err(_) => Err("no reply within 5 s".into()),
    }
}

async fn handshake_raw(
    connection: &Connection,
    request_payload: &[u8],
) -> Res<(Vec<u8>, wire::HandshakeResponse)> {
    let (kind, payload) = exchange(connection, 0x01, request_payload)
        .await?
        .ok_or("EOF instead of HandshakeResponse")?;
    if kind != MessageType::HandshakeResponse {
        return Err(format!("unexpected {kind:?}").into());
    }
    let response = decode(&payload)?;
    Ok((payload, response))
}

fn array_shape(payload: &[u8]) -> Res<Vec<Value>> {
    match rmpv::decode::read_value(&mut &payload[..])? {
        Value::Array(items) => Ok(items),
        other => Err(format!("not an array: {other}").into()),
    }
}

async fn suite(a: HashMap<String, String>) -> Res<()> {
    let signer = signer(&a);
    let addr: SocketAddr = a["addr"].parse()?;
    let miner = parse_ss58(&a["hotkey"])?;
    let target = QuicTarget::new(miner, addr);
    let mut s = Suite { failures: 0 };
    let client = QuicClient::new(signer.clone(), QuicClientConfig::default())?;

    s.report(
        "connect",
        client
            .connect(&target)
            .await
            .map(|_| "ok".into())
            .map_err(|e| e.to_string()),
    );

    let mut data = DataMap::new();
    for (k, v) in value_matrix() {
        data.insert(k.into(), v);
    }
    let r = client
        .query(&target, "echo", data.clone(), Some(Duration::from_secs(10)))
        .await;
    s.report(
        "echo_matrix",
        match r {
            Ok(reply) if reply.success => {
                let bad: Vec<String> = value_matrix()
                    .into_iter()
                    .filter(|(k, v)| !reply.data.get(*k).is_some_and(|g| same_value(v, g)))
                    .map(|(k, _)| format!("{k}: got {:?}", reply.data.get(k)))
                    .collect();
                check(bad.is_empty(), bad.join("; "))
            }
            other => Err(format!("{other:?}")),
        },
    );

    let r = client
        .query_typed::<_, AddResp>(&target, "add", &json_add(), Some(Duration::from_secs(10)))
        .await;
    s.report(
        "typed_add",
        check(matches!(r, Ok(AddResp { sum: 5 })), format!("{r:?}")),
    );

    for (case, synapse, want) in [
        ("unrecognized", "nope", "unrecognized synapse type"),
        ("handler_error", "fail", "request processing failed"),
    ] {
        let r = client.query(&target, synapse, DataMap::new(), None).await;
        s.report(
            case,
            check(
                matches!(&r, Ok(reply) if !reply.success && reply.error.as_deref() == Some(want)),
                format!("{r:?}"),
            ),
        );
    }

    let r = match client.query_stream(&target, "stream", DataMap::new()).await {
        Ok(stream) => stream.collect_all().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    s.report(
        "stream",
        check(
            r.as_ref().is_ok_and(|c| {
                *c == [
                    b"chunk-1".to_vec(),
                    b"chunk-2".to_vec(),
                    b"chunk-3".to_vec(),
                ]
            }),
            format!("{r:?}"),
        ),
    );

    let mut got = Vec::new();
    let mut end = None;
    match client
        .query_stream(&target, "stream_fail", DataMap::new())
        .await
    {
        Ok(mut stream) => loop {
            match stream.next_chunk().await {
                Ok(Some(c)) => got.push(c),
                Ok(None) => break,
                Err(e) => {
                    end = Some(e.to_string());
                    break;
                }
            }
        },
        Err(e) => end = Some(e.to_string()),
    }
    s.report(
        "stream_fail",
        check(
            got.len() == 2 && end.as_deref() == Some("stream error: stream processing failed"),
            format!("chunks={} end={end:?}", got.len()),
        ),
    );

    let r = match client.query_stream(&target, "nope", DataMap::new()).await {
        Ok(stream) => stream
            .collect_all()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    s.report(
        "stream_unrecognized",
        check(
            r.as_ref().is_err_and(|e| {
                e.contains("SynapseResponse error on streaming path: unrecognized synapse type")
            }),
            format!("{r:?}"),
        ),
    );

    let other = QuicClient::new(signer.clone(), QuicClientConfig::default())?;
    let wrong = QuicTarget::new(rtao_primitives::AccountId([7; 32]), addr);
    let r = other.connect(&wrong).await;
    s.report(
        "wrong_hotkey",
        check(
            r.as_ref()
                .is_err_and(|e| e.to_string().contains("miner hotkey mismatch")),
            format!("{:?}", r.map(|_| ())),
        ),
    );
    other.close().await;

    raw_cases(&mut s, signer.as_ref(), addr, &a).await?;

    if let Some(rate) = a.get("rate-addr") {
        let addr: SocketAddr = rate.parse()?;
        let endpoint = raw_endpoint().await?;
        let (connection, fp) = raw_connect(&endpoint, addr).await?;
        let mut outcomes = Vec::new();
        for _ in 0..31 {
            let req = auth::sign_handshake(signer.as_ref(), &fp, unix_now()).await?;
            let (_, resp) = handshake_raw(&connection, &encode(&req)?).await?;
            outcomes.push((resp.accepted, resp.miner_hotkey.is_empty()));
        }
        let first_30_ok = outcomes[..30].iter().all(|(acc, _)| *acc);
        let last = outcomes[30];
        s.report(
            "rate_limit_31st_preauth_reject",
            check(
                first_30_ok && last == (false, true),
                format!("{outcomes:?}"),
            ),
        );
    }

    client.close().await;
    if s.failures > 0 {
        return Err(format!("{} case(s) failed", s.failures).into());
    }
    Ok(())
}

fn json_add() -> HashMap<&'static str, i64> {
    HashMap::from([("a", 2), ("b", 3)])
}

async fn raw_cases(
    s: &mut Suite,
    signer: &dyn Signer,
    addr: SocketAddr,
    a: &HashMap<String, String>,
) -> Res<()> {
    // Unauthenticated request on a fresh endpoint (new source port).
    let endpoint = raw_endpoint().await?;
    let (connection, fp) = raw_connect(&endpoint, addr).await?;
    let packet = wire::SynapsePacket {
        synapse_type: "echo".into(),
        data: DataMap::new(),
        timestamp: unix_now(),
    };
    let r = exchange(&connection, 0x03, &encode(&packet)?).await?;
    s.report(
        "unauthenticated_request",
        match r {
            Some((MessageType::SynapseResponse, p)) => {
                let resp: wire::SynapseResponse = decode(&p)?;
                check(
                    !resp.success
                        && resp.error.as_deref() == Some("authentication failed")
                        && array_shape(&p)?.len() == 4,
                    format!("{resp:?}"),
                )
            }
            other => Err(format!("{other:?}")),
        },
    );

    // Valid handshake: wire shape and capture.
    let request = auth::sign_handshake(signer, &fp, unix_now()).await?;
    let request_bytes = encode(&request)?;
    let (response_bytes, response) = handshake_raw(&connection, &request_bytes).await?;
    let shape = array_shape(&response_bytes)?;
    let shape_ok = shape.len() == 6
        && shape[0].is_str()
        && shape[1].is_u64()
        && shape[2].is_str()
        && shape[3] == Value::Boolean(true)
        && shape[4].is_str()
        && shape[5].as_str() == Some(fp.as_str())
        && response_bytes[0] == 0x96;
    s.report(
        "handshake_shape",
        check(shape_ok && response.accepted, format!("{shape:?}")),
    );
    let miner = parse_ss58(&a["hotkey"])?;
    s.report(
        "handshake_response_verifies",
        auth::verify_handshake_response(&response, &miner, &request, &fp)
            .map(|_| "ok".into())
            .map_err(|e| e.to_string()),
    );
    if let Some(path) = a.get("capture") {
        let fixture = json!({
            "source": "btlightning-py server (53c090b) HandshakeResponse",
            "server_fingerprint_b64": fp,
            "request_frame_hex": hex::encode(wire::encode_frame(MessageType::HandshakeRequest, &request_bytes)?),
            "response_frame_hex": hex::encode(wire::encode_frame(MessageType::HandshakeResponse, &response_bytes)?),
        });
        std::fs::write(path, serde_json::to_string_pretty(&fixture)?)?;
    }

    // Authenticated response shape.
    let mut data = DataMap::new();
    data.insert("msg".into(), Value::from("hi"));
    let packet = wire::SynapsePacket {
        synapse_type: "echo".into(),
        data,
        timestamp: unix_now(),
    };
    let r = exchange(&connection, 0x03, &encode(&packet)?).await?;
    s.report(
        "synapse_response_shape",
        match r {
            Some((MessageType::SynapseResponse, p)) => {
                let shape = array_shape(&p)?;
                check(
                    p[0] == 0x94
                        && shape.len() == 4
                        && shape[0] == Value::Boolean(true)
                        && shape[2].is_u64()
                        && shape[3].is_nil(),
                    format!("{shape:?}"),
                )
            }
            other => Err(format!("{other:?}")),
        },
    );

    // Replay of the accepted request.
    let (_, replay) = handshake_raw(&connection, &request_bytes).await?;
    s.report(
        "replay_rejected",
        check(
            !replay.accepted
                && !replay.miner_hotkey.is_empty()
                && replay.cert_fingerprint.is_none(),
            format!("{replay:?}"),
        ),
    );

    // Map-form request.
    let map_req = auth::sign_handshake(signer, &fp, unix_now()).await?;
    let (_, map_resp) = handshake_raw(&connection, &rmp_serde::to_vec_named(&map_req)?).await?;
    s.report(
        "map_form_accepted",
        check(map_resp.accepted, format!("{map_resp:?}")),
    );

    let now = unix_now();
    for (case, ts, nonce) in [
        ("stale_rejected", now - 300, auth::new_nonce()?),
        ("future_rejected", now + 61, auth::new_nonce()?),
        ("short_nonce_rejected", now, "0".repeat(31)),
    ] {
        let req: HandshakeRequest = sign_handshake_with_nonce(signer, &fp, ts, nonce).await?;
        let (_, resp) = handshake_raw(&connection, &encode(&req)?).await?;
        s.report(
            case,
            check(
                !resp.accepted && !resp.miner_hotkey.is_empty(),
                format!("{resp:?}"),
            ),
        );
    }

    let (bytes, resp) = handshake_raw(&connection, b"\xc1garbage").await?;
    let shape = array_shape(&bytes)?;
    s.report(
        "undecodable_preauth_shape",
        check(
            !resp.accepted
                && resp.miner_hotkey.is_empty()
                && resp.signature.is_empty()
                && resp.connection_id.is_empty()
                && shape.len() == 6
                && shape[5].is_nil()
                && bytes.len() == 11,
            format!("{} {shape:?}", hex::encode(&bytes)),
        ),
    );

    // Oversize header: no reply (EOF, reset or stop).
    let r = {
        let (mut send, mut recv) = connection.open_bi().await?;
        let mut header = vec![0x03];
        header.extend_from_slice(&(wire::DEFAULT_MAX_FRAME_PAYLOAD + 1).to_be_bytes());
        let _ = send.write_all(&header).await;
        let _ = send.finish();
        tokio::time::timeout(Duration::from_secs(5), read_frame(&mut recv, u32::MAX)).await
    };
    s.report(
        "oversize_no_reply",
        check(matches!(r, Ok(Err(_))), format!("{r:?}")),
    );

    // A server-to-client frame type first: FIN without reply.
    let r = exchange(
        &connection,
        0x04,
        &encode(&wire::SynapseResponse::error("x", 0))?,
    )
    .await?;
    s.report(
        "unexpected_type_no_reply",
        check(r.is_none(), format!("{r:?}")),
    );

    connection.close(0u32.into(), b"done");
    endpoint.wait_idle().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let a = args();
    let result = match mode.as_str() {
        "serve" => serve(a).await,
        "capture" => capture(a).await,
        "suite" => suite(a).await,
        _ => Err("usage: quic_peer serve|capture|suite ...".into()),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
