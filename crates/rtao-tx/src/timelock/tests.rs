// Derived from RaoFoundation/subtensor sdk/python/bittensor/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Recorded core crypto and scripted client tests; no live beacon or chain.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::{
    collections::VecDeque,
    error::Error,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use rtao_core::timelock::constants::{GENESIS_TIME, QUICKNET_CHAIN_HASH};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use super::*;

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
const QUICKNET_ONE: &str = include_str!("../../../../vendor/recorded/drand/quicknet-1.json");
const PAST_VECTOR: &str = include_str!("../../../../vendor/recorded/drand/pr-u4-past-vector.json");

#[derive(serde::Deserialize)]
struct PastVector {
    reveal_round: u64,
    ciphertext_hex: String,
    envelope_hex: String,
    payload_hex: String,
}

fn recorded() -> Result<(Timelocked, PastVector, DrandRound)> {
    let vector: PastVector = rtao_primitives::json::from_str(PAST_VECTOR)?;
    let round = rtao_primitives::json::from_str(QUICKNET_ONE)?;
    Ok((
        Timelocked::parse(vector.envelope_hex.as_str())?,
        vector,
        round,
    ))
}

fn at_genesis(offset: Duration) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(GENESIS_TIME) + offset
}

struct FixedClock(SystemTime);
impl crate::Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

fn trailer(round: u64) -> Timelocked {
    let mut bytes = ROUND_SUFFIX.to_vec();
    bytes.extend_from_slice(&round.to_le_bytes());
    Timelocked::parse(bytes).expect("valid Python trailer")
}

#[test]
fn reveal_forms_share_python_round_arithmetic() -> Result {
    let now = at_genesis(Duration::from_millis(1500));
    for reveal in [
        Reveal::In(Duration::from_millis(2500)),
        Reveal::At(at_genesis(Duration::from_millis(4000))),
        Reveal::Round(3),
    ] {
        assert_eq!(target_round(reveal, DEFAULT_BLOCK_TIME, now)?, 3);
    }
    assert_eq!(
        Reveal::after("1h30m")?,
        Reveal::In(Duration::from_secs(5400))
    );
    assert_eq!(
        Reveal::after("30m1h")?,
        Reveal::In(Duration::from_secs(5400))
    );
    assert_eq!(
        Reveal::at("2023-08-23T15:09:27")?,
        Reveal::At(at_genesis(Duration::ZERO))
    );
    assert_eq!(
        Reveal::at("2023-08-23T17:09:27+02:00")?,
        Reveal::at("2023-08-23T15:09:27Z")?
    );
    Ok(())
}

#[test]
fn timelock_reveal_in_blocks() -> Result {
    let now = at_genesis(Duration::ZERO);
    assert_eq!(
        target_round(Reveal::InBlocks(2), DEFAULT_BLOCK_TIME, now)?,
        9
    );
    assert_eq!(
        target_round(Reveal::InBlocks(2), Duration::from_millis(500), now)?,
        2
    );
    assert_eq!(
        target_round(Reveal::InBlocks(2), Duration::from_secs(3), now)?,
        3
    );
    assert!(target_round(Reveal::InBlocks(0), DEFAULT_BLOCK_TIME, now).is_err());
    assert!(target_round(Reveal::InBlocks(1), Duration::ZERO, now).is_err());
    assert!(target_round(Reveal::InBlocks(u64::MAX), Duration::MAX, now).is_err());
    Ok(())
}

#[test]
fn seal_rejects_past_zero_and_nonfinite_inputs_before_crypto() -> Result {
    let now = at_genesis(Duration::ZERO);
    assert_eq!(
        target_round(Reveal::Round(1), DEFAULT_BLOCK_TIME, now)
            .unwrap_err()
            .to_string(),
        "reveal moment is not in the future (round 1, but the beacon is already at round 1)"
    );
    assert!(target_round(Reveal::In(Duration::ZERO), DEFAULT_BLOCK_TIME, now).is_err());
    assert!(target_round(Reveal::Round(u64::MAX), DEFAULT_BLOCK_TIME, now).is_err());
    for duration in ["0", "-1s", "NaN", "inf", "1h nope"] {
        assert!(Reveal::after(duration).is_err(), "{duration}");
    }
    assert!(Reveal::at(f64::NAN).is_err());
    Ok(())
}

#[test]
fn envelope_parse_accepts_python_bytes_hex_and_round_zero() -> Result {
    let (sealed, vector, _) = recorded()?;
    assert_eq!(sealed.reveal_round(), vector.reveal_round);
    assert_eq!(sealed.hex(), vector.envelope_hex);
    assert_eq!(Timelocked::parse(sealed.ciphertext())?, sealed);
    assert_eq!(Timelocked::parse(&sealed)?, sealed);
    let spaced = vector
        .envelope_hex
        .as_bytes()
        .chunks(2)
        .map(|pair| std::str::from_utf8(pair).expect("ASCII hex"))
        .collect::<Vec<_>>()
        .join(" \t\u{b}\u{c}\r\n");
    assert_eq!(Timelocked::parse(format!(" 0x{spaced} \n"))?, sealed);
    assert_eq!(
        Timelocked::parse(format!("\u{1c}0x{spaced}\u{1f}"))?,
        sealed
    );
    for input in ["0 x00", "A A", "0X00", "xy", "00"] {
        assert!(Timelocked::parse(input).is_err(), "{input}");
    }
    let zero = trailer(0);
    assert_eq!(zero.reveal_at()?.timestamp(), GENESIS_TIME as i64 - 3);
    assert!(zero.revealed_at(at_genesis(Duration::ZERO))?);
    Ok(())
}

#[test]
fn remaining_and_not_ready_diagnostic_use_injected_wall_time() -> Result {
    let sealed = trailer(3);
    assert_eq!(
        sealed.remaining_at(at_genesis(Duration::ZERO))?,
        Duration::from_secs(6)
    );
    assert_eq!(
        sealed.remaining_at(at_genesis(Duration::from_secs(10)))?,
        Duration::ZERO
    );
    assert!(!sealed.revealed_at(at_genesis(Duration::from_secs(5)))?);
    assert!(sealed.revealed_at(at_genesis(Duration::from_secs(6)))?);
    assert_eq!(
        TimelockError::NotReady {
            reveal_round: 3,
            reveal_at: sealed.reveal_at()?,
            remaining: Duration::from_millis(6500),
        }
        .to_string(),
        "not decryptable yet: reveals at 2023-08-23T15:09:33+00:00 (round 3, 6s from now)"
    );
    Ok(())
}

#[tokio::test]
async fn recorded_envelope_inner_and_signature_match_core_and_native_commitment() -> Result {
    let (sealed, vector, round) = recorded()?;
    let inner = sealed.encrypted().await?;
    assert_eq!(hex::encode(&inner), vector.ciphertext_hex);
    assert_ne!(inner, sealed.ciphertext());
    let bytes = sealed.ciphertext().to_vec();
    let signature = round.signature.clone();
    let plaintext =
        rtao_core::offload(move || crypto::decrypt_with_signature(&bytes, &signature)).await??;
    assert_eq!(hex::encode(&plaintext), vector.payload_hex);
    let field = sealed.commitment_field().await?;
    assert_eq!(
        field,
        rtao_runtime::models::Data::TimelockEncrypted {
            encrypted: inner,
            reveal_round: vector.reveal_round,
        }
    );
    let call = commitments::set_timelocked(rtao_primitives::NetUid::new(1)?, &[sealed]).await?;
    assert_eq!(call.descriptor.pallet, "Commitments");
    assert_eq!(call.descriptor.name, "set_commitment");
    use rtao_primitives::IntoCoreValue;
    assert_eq!(
        call.args,
        rtao_core::codec::Value::record(vec![
            ("netuid".into(), rtao_core::codec::Value::Int(1)),
            (
                "info".into(),
                rtao_runtime::models::CommitmentInfo {
                    fields: vec![field]
                }
                .into_core_value()
            ),
        ])
    );
    Ok(())
}

#[tokio::test]
async fn seal_offloads_future_round_and_core_opens_recorded_historical_pulse() -> Result {
    let (_, _, round) = recorded()?;
    let now = at_genesis(Duration::ZERO) - Duration::from_secs(4);
    // current_round clamps pre-genesis to one; use round two to check future
    // selection, then use the core directly for the public historical pulse.
    let sealed = seal_at("hello 🦀", Reveal::Round(2), now).await?;
    assert_eq!(sealed.reveal_round(), 2);
    assert_eq!(crypto::reveal_round(sealed.ciphertext())?, 2);
    let payload = b"a portable timelock commitment".to_vec();
    let expected = payload.clone();
    let (bytes, _) = rtao_core::offload(move || crypto::encrypt_at_round(&payload, 1)).await??;
    let sealed = Timelocked::parse(bytes)?;
    let bytes = sealed.into_bytes();
    let opened =
        rtao_core::offload(move || crypto::decrypt_with_signature(&bytes, &round.signature))
            .await??;
    assert_eq!(opened, expected);
    Ok(())
}

#[tokio::test]
async fn malformed_core_envelope_and_wrong_signatures_are_rejected() -> Result {
    assert!(trailer(1).encrypted().await.is_err());
    let (sealed, _, _) = recorded()?;
    let bytes = sealed.into_bytes();
    let error =
        rtao_core::offload(move || crypto::decrypt_with_signature(&bytes, &"00".repeat(48)))
            .await?;
    assert!(error.is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn nonwaiting_open_returns_not_ready_without_fetching() -> Result {
    let attempts = AtomicUsize::new(0);
    let error = open_with_attempt(
        &trailer(3),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            wait: false,
            timeout: None,
        },
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Ok(vec![]) }
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, TimelockError::NotReady { reveal_round: 3, remaining, .. } if remaining == Duration::from_secs(6))
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn short_total_timeout_refuses_future_reveal_without_wait() -> Result {
    let started = Instant::now();
    let error = open_with_attempt(
        &trailer(3),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            wait: true,
            timeout: Some(Duration::from_secs(5)),
        },
        || async { Ok(vec![1]) },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, TimelockError::NotReady { .. }));
    assert_eq!(started.elapsed(), Duration::ZERO);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_open_sleeps_until_exact_reveal_before_fetch() -> Result {
    let started = Instant::now();
    let result = open_with_attempt(
        &trailer(3),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions::default(),
        || async {
            assert_eq!(started.elapsed(), Duration::from_secs(6));
            Ok(b"opened".to_vec())
        },
    )
    .await?;
    assert_eq!(result, b"opened");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn beacon_and_crypto_failures_retry_once_per_second() -> Result {
    let attempts = AtomicUsize::new(0);
    let started = Instant::now();
    let result = open_with_attempt(
        &trailer(1),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions::default(),
        || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                match attempt {
                    0 => Err(TimelockError::Beacon("relay lag".into())),
                    1 => Err(TimelockError::Crypto("invalid signature".into())),
                    _ => Ok(b"opened".to_vec()),
                }
            }
        },
    )
    .await?;
    assert_eq!(result, b"opened");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deadline_caps_reveal_wait_and_fetch_together() -> Result {
    let started = Instant::now();
    let attempts = AtomicUsize::new(0);
    let error = open_with_attempt(
        &trailer(3),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            wait: true,
            timeout: Some(Duration::from_millis(6500)),
        },
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                sleep(Duration::from_secs(100)).await;
                Ok(vec![1])
            }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_millis(6500));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(matches!(error, TimelockError::Deadline { reveal_round: 3 }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deadline_truncates_the_retry_sleep_and_prevents_another_attempt() -> Result {
    let started = Instant::now();
    let attempts = AtomicUsize::new(0);
    let error = open_with_attempt(
        &trailer(1),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            timeout: Some(Duration::from_millis(1500)),
            ..OpenOptions::default()
        },
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(TimelockError::Beacon("relay lag".into())) }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_millis(1500));
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(matches!(error, TimelockError::Deadline { reveal_round: 1 }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn grace_is_bounded_even_without_caller_timeout() -> Result {
    let started = Instant::now();
    let attempts = AtomicUsize::new(0);
    let error = open_with_attempt(
        &trailer(1),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions::default(),
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(TimelockError::Beacon("beacon unreachable".into())) }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    assert_eq!(attempts.load(Ordering::SeqCst), 30);
    assert!(
        matches!(error, TimelockError::Unavailable { reveal_round: 1, ref detail } if detail == "beacon unreachable")
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn zero_timeout_does_not_start_a_beacon_request() -> Result {
    let attempts = AtomicUsize::new(0);
    let error = open_with_attempt(
        &trailer(1),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            timeout: Some(Duration::ZERO),
            ..OpenOptions::default()
        },
        || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Ok(vec![]) }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(matches!(error, TimelockError::Deadline { .. }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelling_open_stops_before_the_next_attempt() -> Result {
    let attempts = Arc::new(AtomicUsize::new(0));
    let called = attempts.clone();
    let task = tokio::spawn(async move {
        open_with_attempt(
            &trailer(1),
            &FixedClock(at_genesis(Duration::ZERO)),
            OpenOptions::default(),
            || {
                called.fetch_add(1, Ordering::SeqCst);
                async { Err(TimelockError::Beacon("lag".into())) }
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn deadline_drops_an_in_flight_attempt_future() -> Result {
    struct OnDrop(Arc<AtomicUsize>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let error = open_with_attempt(
        &trailer(1),
        &FixedClock(at_genesis(Duration::ZERO)),
        OpenOptions {
            timeout: Some(Duration::from_secs(1)),
            ..OpenOptions::default()
        },
        || {
            let guard = OnDrop(dropped.clone());
            async move {
                let _guard = guard;
                std::future::pending::<std::result::Result<Vec<u8>, TimelockError>>().await
            }
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, TimelockError::Deadline { .. }));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

fn tls(roots: rustls::RootCertStore) -> Result<rustls::ClientConfig> {
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

fn client(endpoints: Vec<String>) -> Result<DrandClient> {
    Ok(DrandClient::with_tls_config(
        DrandConfig {
            endpoints,
            request_timeout: Duration::from_secs(2),
            ..DrandConfig::default()
        },
        tls(rustls::RootCertStore::empty())?,
    )?)
}

#[derive(Clone)]
struct Response {
    status: u16,
    body: Vec<u8>,
    chunked: bool,
    extra: String,
    body_delay: Duration,
}
impl Response {
    fn json(body: &str) -> Self {
        Self {
            status: 200,
            body: body.as_bytes().to_vec(),
            chunked: false,
            extra: String::new(),
            body_delay: Duration::ZERO,
        }
    }
}

struct Server {
    url: String,
    root: Option<rustls_pki_types::CertificateDer<'static>>,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Server {
    async fn new(responses: Vec<Response>, https: bool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (acceptor, root) = if https {
            let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
            let root = certified.cert.der().clone();
            let private = rustls_pki_types::PrivateKeyDer::Pkcs8(
                rustls_pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()),
            );
            let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![root.clone()], private)?;
            cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
            (
                Some(tokio_rustls::TlsAcceptor::from(Arc::new(cfg))),
                Some(root),
            )
        } else {
            (None, None)
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let task = tokio::spawn(async move {
            let mut responses: VecDeque<_> = responses.into();
            while !responses.is_empty() {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let result = if let Some(acceptor) = &acceptor {
                    // An untrusted client may deliberately abort a handshake.
                    let Ok(socket) = acceptor.accept(socket).await else {
                        continue;
                    };
                    serve(socket, &observed, responses.pop_front().expect("response")).await
                } else {
                    serve(socket, &observed, responses.pop_front().expect("response")).await
                };
                // A bounded-body client deliberately closes before full write.
                if let Err(error) = result {
                    let _ = error;
                }
            }
        });
        Ok(Self {
            url: format!("{}://{addr}", if https { "https" } else { "http" }),
            root,
            requests,
            task,
        })
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("observations lock").clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut socket: S,
    requests: &Mutex<Vec<String>>,
    response: Response,
) -> Result {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            return Err(io::Error::other("request disconnected").into());
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > 16 * 1024 {
            return Err(io::Error::other("request too large").into());
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    requests
        .lock()
        .expect("observations lock")
        .push(String::from_utf8(request)?);
    let framing = if response.chunked {
        "Transfer-Encoding: chunked\r\n".into()
    } else {
        format!("Content-Length: {}\r\n", response.body.len())
    };
    socket.write_all(format!("HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\n{framing}{}\r\n", response.status, response.extra).as_bytes()).await?;
    sleep(response.body_delay).await;
    if response.chunked {
        for chunk in response.body.chunks(7) {
            socket
                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await?;
            socket.write_all(chunk).await?;
            socket.write_all(b"\r\n").await?;
        }
        socket.write_all(b"0\r\n\r\n").await?;
    } else {
        socket.write_all(&response.body).await?;
    }
    socket.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn production_open_uses_exact_quicknet_path_and_recorded_signature() -> Result {
    let server = Server::new(vec![Response::json(QUICKNET_ONE)], false).await?;
    let client = client(vec![server.url.clone()])?
        .with_clock(Arc::new(FixedClock(at_genesis(Duration::ZERO))));
    let (sealed, vector, _) = recorded()?;
    let plaintext = open(&sealed, &client, OpenOptions::default()).await?;
    assert_eq!(hex::encode(plaintext), vector.payload_hex);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with(&format!("GET /{QUICKNET_CHAIN_HASH}/public/1 HTTP/1.1\r\n")));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("accept: application/json\r\n")
    );
    Ok(())
}

#[tokio::test]
async fn relay_failover_rejects_wrong_round_status_and_signature_encoding() -> Result {
    let wrong_round = QUICKNET_ONE.replace("\"round\":1", "\"round\":2");
    let a = Server::new(vec![Response::json(&wrong_round)], false).await?;
    let b = Server::new(
        vec![Response {
            status: 404,
            ..Response::json(QUICKNET_ONE)
        }],
        false,
    )
    .await?;
    let c = Server::new(
        vec![Response::json("{\"round\":1,\"signature\":\"xx\"}")],
        false,
    )
    .await?;
    let d = Server::new(vec![Response::json(QUICKNET_ONE)], false).await?;
    let client = client(vec![
        a.url.clone(),
        b.url.clone(),
        c.url.clone(),
        d.url.clone(),
    ])?;
    let round = client.fetch_round(1).await?;
    assert_eq!(
        round,
        rtao_primitives::json::from_str::<DrandRound>(QUICKNET_ONE)?
    );
    for server in [&a, &b, &c, &d] {
        assert_eq!(server.requests().len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn stalled_first_relay_cannot_starve_a_healthy_fallback() -> Result {
    let stalled = Server::new(
        vec![Response {
            body_delay: Duration::from_secs(5),
            ..Response::json(QUICKNET_ONE)
        }],
        false,
    )
    .await?;
    let healthy = Server::new(vec![Response::json(QUICKNET_ONE)], false).await?;
    let client = DrandClient::with_tls_config(
        DrandConfig {
            endpoints: vec![stalled.url.clone(), healthy.url.clone()],
            request_timeout: Duration::from_millis(400),
            ..DrandConfig::default()
        },
        tls(rustls::RootCertStore::empty())?,
    )?;
    assert_eq!(client.fetch_round(1).await?.round, 1);
    assert_eq!(stalled.requests().len(), 1);
    assert_eq!(healthy.requests().len(), 1);
    Ok(())
}

#[tokio::test]
async fn response_bounds_cover_content_length_and_chunked_bodies() -> Result {
    for chunked in [false, true] {
        let server = Server::new(
            vec![Response {
                chunked,
                ..Response::json(QUICKNET_ONE)
            }],
            false,
        )
        .await?;
        let client = DrandClient::with_tls_config(
            DrandConfig {
                endpoints: vec![server.url.clone()],
                max_response_bytes: 32,
                ..DrandConfig::default()
            },
            tls(rustls::RootCertStore::empty())?,
        )?;
        let error = client.fetch_round(1).await.unwrap_err();
        assert!(error.to_string().contains("byte limit"), "{error}");
    }
    Ok(())
}

#[tokio::test]
async fn fetch_deadline_includes_body_and_all_relays() -> Result {
    let server = Server::new(
        vec![Response {
            body_delay: Duration::from_secs(5),
            ..Response::json(QUICKNET_ONE)
        }],
        false,
    )
    .await?;
    let client = DrandClient::with_tls_config(
        DrandConfig {
            endpoints: vec![server.url.clone()],
            request_timeout: Duration::from_millis(50),
            ..DrandConfig::default()
        },
        tls(rustls::RootCertStore::empty())?,
    )?;
    let started = Instant::now();
    assert!(client.fetch_round(1).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn beacon_redirects_are_not_followed() -> Result {
    let destination = Server::new(vec![Response::json(QUICKNET_ONE)], false).await?;
    let redirect = Server::new(
        vec![Response {
            status: 302,
            extra: format!("Location: {}\r\n", destination.url),
            ..Response::json("")
        }],
        false,
    )
    .await?;
    assert!(
        client(vec![redirect.url.clone()])?
            .fetch_round(1)
            .await
            .is_err()
    );
    assert_eq!(redirect.requests().len(), 1);
    assert!(destination.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn beacon_https_uses_explicit_ring_and_supplied_trust_roots() -> Result {
    let server = Server::new(vec![Response::json(QUICKNET_ONE)], true).await?;
    assert!(
        client(vec![server.url.clone()])?
            .fetch_round(1)
            .await
            .is_err()
    );
    let mut roots = rustls::RootCertStore::empty();
    roots.add(server.root.clone().expect("TLS root"))?;
    let client = DrandClient::with_tls_config(
        DrandConfig {
            endpoints: vec![server.url.clone()],
            ..DrandConfig::default()
        },
        tls(roots)?,
    )?;
    assert_eq!(client.fetch_round(1).await?.round, 1);
    assert_eq!(server.requests().len(), 1);
    Ok(())
}

#[test]
fn invalid_http_config_is_rejected_without_network() -> Result {
    for endpoints in [
        vec![],
        vec!["file:///tmp/beacon".into()],
        vec!["https://secret@example.com".into()],
        vec!["https://example.com?token=private".into()],
        vec!["https://example.com/#fragment".into()],
        vec!["https://example.com".into(); 33],
    ] {
        assert!(
            DrandClient::with_tls_config(
                DrandConfig {
                    endpoints,
                    ..DrandConfig::default()
                },
                tls(rustls::RootCertStore::empty())?
            )
            .is_err()
        );
    }
    for limit in [0, 65537] {
        assert!(
            DrandClient::with_tls_config(
                DrandConfig {
                    max_response_bytes: limit,
                    ..DrandConfig::default()
                },
                tls(rustls::RootCertStore::empty())?
            )
            .is_err()
        );
    }
    Ok(())
}
