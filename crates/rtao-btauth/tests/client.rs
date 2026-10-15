//! Signed adapter behavior against actual HTTP/1.1 request bytes.

#![cfg(feature = "client")]

#[path = "client/fixture.rs"]
mod fixture;

use std::{
    error::Error,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use fixture::{Observed, Response, Server};
use futures::StreamExt;
use http::{Method, StatusCode};
use rtao_btauth::{
    AuthError, Clock, ClockError, HEADER_CRYPTO, HEADER_HOTKEY, HEADER_NONCE, HEADER_RECEIVER,
    HEADER_SIGNATURE, HEADER_VERSION, InMemoryNonceStore, Verifier, VerifyConfig, WrappedPolicy,
    client::{ClientError, FanOut, FanOutError, SignedClient},
};
use rtao_primitives::{AccountId, CryptoType, Uid, axon::AxonTarget};
use rtao_signer::{BoxFuture, KeypairSigner, MultiSignature, Signer, SignerError, SigningRequest};
use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const NOW: u64 = 1_752_076_800_000_000_000;

struct SequentialClock(AtomicU64);

impl SequentialClock {
    fn new() -> Self {
        Self(AtomicU64::new(NOW))
    }
}

impl Clock for SequentialClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0.fetch_add(1, Ordering::SeqCst))
    }
}

fn signer(crypto: CryptoType) -> TestResult<Arc<KeypairSigner>> {
    Ok(Arc::new(KeypairSigner::from_uri("//Alice", crypto)?))
}

fn receiver() -> TestResult<AccountId> {
    Ok(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?.account())
}

fn header<'a>(request: &'a Observed, name: &str) -> TestResult<&'a str> {
    Ok(request
        .headers
        .get(name)
        .ok_or_else(|| io::Error::other(format!("missing wire header {name}")))?
        .to_str()?)
}

fn verifier(receiver: AccountId) -> TestResult<Verifier<InMemoryNonceStore>> {
    Ok(Verifier::new(
        receiver,
        VerifyConfig {
            wrapped: WrappedPolicy::Strict,
            ..VerifyConfig::default()
        },
        Arc::new(InMemoryNonceStore::new()),
    )?)
}

async fn verify_requests(
    requests: &[Observed],
    receiver: AccountId,
    sender: AccountId,
) -> TestResult {
    let verifier = verifier(receiver)?;
    for request in requests {
        let caller = verifier
            .verify(
                &request.headers,
                &request.method,
                &request.target,
                &request.body,
                NOW + 1_000_000,
            )
            .await?;
        assert_eq!(caller.hotkey, sender);
    }
    Ok(())
}

fn targets(server: &Server, receiver: AccountId, count: u16) -> Vec<AxonTarget> {
    (0..count)
        .map(|uid| AxonTarget {
            uid: Uid(uid),
            hotkey: receiver,
            base_url: server.base_url.clone(),
        })
        .collect()
}

fn query(target: &AxonTarget) -> (Method, String, Value) {
    (
        Method::POST,
        format!("/query/{}", target.uid),
        json!({"uid": target.uid.get()}),
    )
}

fn signed_client(signer: Arc<dyn Signer>) -> TestResult<SignedClient> {
    signed_client_with_clock(signer, Arc::new(SequentialClock::new()))
}

fn signed_client_with_clock(
    signer: Arc<dyn Signer>,
    clock: Arc<dyn Clock>,
) -> TestResult<SignedClient> {
    // HTTP fixtures need no native certificate store. Explicit empty roots also
    // make the negative HTTPS trust test independent of the host's trust store.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(rustls::RootCertStore::empty())
    .with_no_client_auth();
    Ok(SignedClient::with_tls_config(signer, clock, config)?)
}

fn cfg(concurrency: usize) -> FanOut {
    FanOut {
        concurrency,
        connect_timeout: Duration::from_secs(1),
        timeout: Duration::from_secs(2),
        deadline: Some(Duration::from_secs(3)),
        ..FanOut::default()
    }
}

async fn one_fanout_reply(
    client: &SignedClient,
    server: &Server,
    streaming: bool,
    max_response_bytes: usize,
) -> TestResult<(Uid, Result<Value, FanOutError>)> {
    let config = FanOut {
        max_response_bytes,
        retries: 2,
        timeout: Duration::from_millis(500),
        ..cfg(1)
    };
    let targets = targets(server, receiver()?, 1);
    let mut replies = if streaming {
        client
            .fan_out_stream::<_, Value>(targets, query, config)
            .collect::<Vec<_>>()
            .await
    } else {
        client.fan_out::<_, Value>(targets, query, config).await
    };
    assert_eq!(replies.len(), 1);
    Ok(replies
        .pop()
        .ok_or_else(|| io::Error::other("missing fan-out reply"))?)
}

fn chunked_response(chunks: &[&[u8]], terminate: bool) -> Response {
    let mut wire = Vec::new();
    for chunk in chunks {
        wire.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        wire.extend_from_slice(chunk);
        wire.extend_from_slice(b"\r\n");
    }
    if terminate {
        wire.extend_from_slice(b"0\r\n\r\n");
    }
    let mut response = Response::json(wire);
    response
        .headers
        .push(("Transfer-Encoding".into(), "chunked".into()));
    response
}

struct BehaviorSigner {
    key: KeypairSigner,
    delay: Duration,
    refuse: bool,
    started: AtomicUsize,
    completed: AtomicUsize,
}

impl BehaviorSigner {
    fn new(delay: Duration, refuse: bool) -> TestResult<Arc<Self>> {
        Ok(Arc::new(Self {
            key: KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?,
            delay,
            refuse,
            started: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
        }))
    }
}

impl Signer for BehaviorSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        self.key.sign(request)
    }
    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        Box::pin(async move {
            self.started.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if self.refuse {
                return Err(SignerError::RawSigningUnsupported);
            }
            let signed = self.key.sign_raw(message).await?;
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(signed)
        })
    }
}

struct CountedJson {
    calls: Arc<AtomicUsize>,
}

impl serde::Serialize for CountedJson {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        json!({"exact": "π", "float": 0.25}).serialize(serializer)
    }
}

struct FailingJson;

impl serde::Serialize for FailingJson {
    fn serialize<S>(&self, _: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom(
            "deliberate serialization failure",
        ))
    }
}

#[tokio::test]
async fn actual_wire_bytes_bind_encoded_targets_empty_query_and_materialized_bodies() -> TestResult
{
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let receiver = receiver()?;
    let raw = b"\x00raw body\r\n\xff";
    let payload = json!({"text": "π/ +", "float": 0.125, "nested": [true, null]});
    let json_bytes = serde_json::to_vec(&payload)?;
    for crypto in [CryptoType::Sr25519, CryptoType::Ed25519] {
        let signer = signer(crypto)?;
        let client = signed_client(signer.clone())?;
        for (path, body) in [
            ("/a%2Fb/%CF%80?x=%2B&blank=&x=one", raw.as_slice()),
            ("/empty?", b"".as_slice()),
        ] {
            let response = client
                .request(Method::POST, server.base_url.join(path)?, &receiver)
                .header("X-Application", "present")
                .body(body.to_vec())
                .send()
                .await?;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.bytes().await?.as_ref(), b"{}");
        }
        client
            .request(
                Method::PUT,
                server.base_url.join("/json?value=%25")?,
                &receiver,
            )
            .json(&payload)
            .send()
            .await?
            .bytes()
            .await?;
        let all = server.observations().await?;
        let start = all.len() - 3;
        let requests = &all[start..];
        assert_eq!(requests[0].target, "/a%2Fb/%CF%80?x=%2B&blank=&x=one");
        assert_eq!(requests[0].body, raw);
        assert_eq!(header(&requests[0], "x-application")?, "present");
        assert_eq!(requests[1].target, "/empty?");
        assert!(requests[1].body.is_empty());
        assert_eq!(requests[2].target, "/json?value=%25");
        assert_eq!(requests[2].method, Method::PUT);
        assert_eq!(requests[2].body, json_bytes);
        assert_eq!(header(&requests[2], "content-type")?, "application/json");
        for request in requests {
            assert_eq!(
                header(request, HEADER_HOTKEY)?,
                signer.account().to_string()
            );
            assert_eq!(header(request, HEADER_RECEIVER)?, receiver.to_string());
            assert_eq!(request.headers.get_all(HEADER_SIGNATURE).iter().count(), 1);
            match crypto {
                CryptoType::Sr25519 => assert!(!request.headers.contains_key(HEADER_CRYPTO)),
                CryptoType::Ed25519 => assert_eq!(header(request, HEADER_CRYPTO)?, "ed25519"),
            }
        }
        verify_requests(requests, receiver, signer.account()).await?;
        let verifier = verifier(receiver)?;
        assert!(matches!(
            verifier.authenticate(
                &requests[0].headers,
                &requests[0].method,
                "/a/b/π?x=+",
                &requests[0].body,
                NOW
            ),
            Err(AuthError::BadSignature(_))
        ));
        assert!(matches!(
            verifier.authenticate(
                &requests[0].headers,
                &requests[0].method,
                &requests[0].target,
                b"changed",
                NOW
            ),
            Err(AuthError::BadSignature(_))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn application_headers_cannot_override_or_inject_authentication() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let receiver = receiver()?;
    for name in [
        HEADER_VERSION,
        HEADER_HOTKEY,
        HEADER_NONCE,
        HEADER_RECEIVER,
        HEADER_SIGNATURE,
        HEADER_CRYPTO,
        "x-BitTenSor-unrecognized",
        "bad header name",
        "Host",
        "content-length",
        "Transfer-Encoding",
        "Connection",
    ] {
        let result = client
            .request(Method::POST, server.base_url.clone(), &receiver)
            .header(name, "override")
            .body(b"later body does not erase errors".to_vec())
            .json(&json!({"later": true}))
            .send()
            .await;
        assert!(
            matches!(result, Err(ClientError::InvalidHeader(_))),
            "{name}"
        );
    }
    let result = client
        .request(Method::GET, server.base_url.clone(), &receiver)
        .header("x-app", "line\r\ninjected: value")
        .send()
        .await;
    assert!(matches!(result, Err(ClientError::InvalidHeader(_))));
    let result = client
        .request(Method::GET, server.base_url.clone(), &receiver)
        .header("Connection", HEADER_SIGNATURE)
        .send()
        .await;
    assert!(matches!(result, Err(ClientError::InvalidHeader(_))));
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn https_uses_explicit_ring_and_requires_the_fixture_root() -> TestResult {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let server = Server::https(|_, _| Response::json(b"{\"secure\":true}".to_vec())).await?;
    let signer = signer(CryptoType::Ed25519)?;
    let receiver = receiver()?;
    let untrusted = signed_client(signer.clone())?;
    let error = untrusted
        .request(
            Method::POST,
            server.base_url.join("/secure?encoded=%2F")?,
            &receiver,
        )
        .body(b"https body".to_vec())
        .send()
        .await;
    assert!(matches!(error, Err(ClientError::Http(_))));
    assert!(server.observations().await?.is_empty());
    let root = server
        .root
        .clone()
        .ok_or_else(|| io::Error::other("TLS fixture has no root"))?;
    let trusted = SignedClient::with_extra_roots(
        signer.clone(),
        Arc::new(SequentialClock::new()),
        vec![root],
    )?;
    let response = trusted
        .request(
            Method::POST,
            server.base_url.join("/secure?encoded=%2F")?,
            &receiver,
        )
        .body(b"https body".to_vec())
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await?.as_ref(), b"{\"secure\":true}");
    let observed = server.wait_for(1).await?;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].target, "/secure?encoded=%2F");
    verify_requests(&observed, receiver, signer.account()).await?;
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    Ok(())
}

#[tokio::test]
async fn redirects_never_replay_auth_to_the_same_or_another_origin() -> TestResult {
    let sink = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let destination = sink.base_url.join("/destination")?.to_string();
    let source = Server::http(move |index, request| {
        let status = [301, 302, 303, 307, 308][index % 5];
        let location = if request.target.starts_with("/same") {
            "/redirect-target".into()
        } else {
            destination.clone()
        };
        Response {
            status,
            headers: vec![("Location".into(), location)],
            ..Response::json(b"{}".to_vec())
        }
    })
    .await?;
    let signer = signer(CryptoType::Sr25519)?;
    let client = signed_client(signer.clone())?;
    let receiver = receiver()?;
    for prefix in ["same", "other"] {
        for status in [301, 302, 303, 307, 308] {
            let response = client
                .request(
                    Method::POST,
                    source.base_url.join(&format!("/{prefix}/{status}"))?,
                    &receiver,
                )
                .body(b"sensitive".to_vec())
                .send()
                .await?;
            assert_eq!(response.status().as_u16(), status);
            response.bytes().await?;
        }
    }
    let requests = source.observations().await?;
    assert_eq!(requests.len(), 10);
    assert!(
        requests
            .iter()
            .all(|request| request.target != "/redirect-target")
    );
    assert!(sink.observations().await?.is_empty());
    verify_requests(&requests, receiver, signer.account()).await?;
    Ok(())
}

#[tokio::test]
async fn direct_transport_failure_is_not_automatically_replayed() -> TestResult {
    let server = Server::http(|_, _| Response::disconnect()).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let receiver = receiver()?;
    let error = client
        .request(Method::GET, server.base_url.clone(), &receiver)
        .send()
        .await;
    assert!(matches!(error, Err(ClientError::Http(_))));
    assert_eq!(server.wait_for(1).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn retries_resign_fresh_nonces_and_reuse_once_serialized_json() -> TestResult {
    for failure in [408, 429, 503] {
        let server = Server::http(move |index, _| Response {
            status: if index < 2 { failure } else { 200 },
            ..Response::json(b"{\"ok\":true}".to_vec())
        })
        .await?;
        let signer = signer(CryptoType::Ed25519)?;
        let client = signed_client(signer.clone())?;
        let receiver = receiver()?;
        let calls = Arc::new(AtomicUsize::new(0));
        let serializations = calls.clone();
        let results = client
            .fan_out::<_, Value>(
                targets(&server, receiver, 1),
                move |_| {
                    (
                        Method::POST,
                        "/retry?x=%2B&blank=".into(),
                        CountedJson {
                            calls: calls.clone(),
                        },
                    )
                },
                FanOut {
                    retries: 2,
                    ..cfg(1)
                },
            )
            .await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, Uid(0));
        assert_eq!(results[0].1.as_ref().ok(), Some(&json!({"ok": true})));
        assert_eq!(serializations.load(Ordering::SeqCst), 1);
        let requests = server.wait_for(3).await?;
        assert_eq!(requests.len(), 3);
        let bytes = serde_json::to_vec(&json!({"exact": "π", "float": 0.25}))?;
        let mut previous = None;
        for request in &requests {
            assert_eq!(request.target, "/retry?x=%2B&blank=");
            assert_eq!(request.body, bytes);
            let nonce: u64 = header(request, HEADER_NONCE)?.parse()?;
            if let Some(previous) = previous {
                assert!(nonce > previous);
            }
            previous = Some(nonce);
        }
        verify_requests(&requests, receiver, signer.account()).await?;
    }
    Ok(())
}

#[tokio::test]
async fn retry_limit_retains_the_final_http_status_and_all_attempts_verify() -> TestResult {
    let server = Server::http(|_, _| Response {
        status: 503,
        ..Response::json(b"{}".to_vec())
    })
    .await?;
    let signer = signer(CryptoType::Sr25519)?;
    let client = signed_client(signer.clone())?;
    let receiver = receiver()?;
    let results = client
        .fan_out::<_, Value>(
            targets(&server, receiver, 1),
            query,
            FanOut {
                retries: 3,
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(results.len(), 1);
    assert!(matches!(
        results[0],
        (
            Uid(0),
            Err(FanOutError::Status(StatusCode::SERVICE_UNAVAILABLE))
        )
    ));
    let requests = server.wait_for(4).await?;
    assert_eq!(requests.len(), 4);
    verify_requests(&requests, receiver, signer.account()).await?;
    Ok(())
}

#[tokio::test]
async fn connection_failures_retry_above_signing_and_preserve_the_transport_source() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    let signer = BehaviorSigner::new(Duration::ZERO, false)?;
    let client = signed_client(signer.clone())?;
    let target = AxonTarget {
        uid: Uid(29),
        hotkey: receiver()?,
        base_url: url::Url::parse(&format!("http://{address}/"))?,
    };
    let results = client
        .fan_out::<_, Value>(
            vec![target],
            query,
            FanOut {
                retries: 2,
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Uid(29));
    match &results[0].1 {
        Err(FanOutError::Request(ClientError::Http(error))) => assert!(error.is_connect()),
        other => {
            return Err(
                io::Error::other(format!("expected connection error, got {other:?}")).into(),
            );
        }
    }
    assert_eq!(signer.started.load(Ordering::SeqCst), 3);
    assert_eq!(signer.completed.load(Ordering::SeqCst), 3);
    assert!(
        results[0]
            .1
            .as_ref()
            .err()
            .and_then(Error::source)
            .and_then(Error::source)
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn clock_failure_preserves_authentication_error_without_invoking_signer() -> TestResult {
    struct FailedClock;
    impl Clock for FailedClock {
        fn now_ns(&self) -> Result<u64, ClockError> {
            Err(ClockError::NonceOverflow)
        }
    }
    let signer = BehaviorSigner::new(Duration::ZERO, false)?;
    let client = signed_client_with_clock(signer.clone(), Arc::new(FailedClock))?;
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let error = client
        .request(Method::GET, server.base_url.clone(), &receiver()?)
        .send()
        .await;
    assert!(matches!(
        error,
        Err(ClientError::Auth(AuthError::Clock(
            ClockError::NonceOverflow
        )))
    ));
    assert_eq!(signer.started.load(Ordering::SeqCst), 0);
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn fan_out_respects_concurrency_and_retains_each_uid() -> TestResult {
    let server = Server::http(|_, request| {
        Response::delayed(request.body.clone(), Duration::from_millis(60))
    })
    .await?;
    let signer = signer(CryptoType::Sr25519)?;
    let client = signed_client(signer.clone())?;
    let receiver = receiver()?;
    let mut results = client
        .fan_out::<_, Value>(targets(&server, receiver, 7), query, cfg(2))
        .await;
    results.sort_by_key(|(uid, _)| *uid);
    assert_eq!(results.len(), 7);
    for (number, (uid, result)) in results.into_iter().enumerate() {
        assert_eq!(usize::from(uid.get()), number);
        assert_eq!(result?, json!({"uid": number}));
    }
    assert_eq!(server.peak(), 2);
    let requests = server.wait_for(7).await?;
    assert_eq!(requests.len(), 7);
    verify_requests(&requests, receiver, signer.account()).await?;
    Ok(())
}

#[tokio::test]
async fn stream_yields_early_replies_in_completion_order() -> TestResult {
    let server = Server::http(|_, request| {
        let delay = match request.target.as_str() {
            "/query/0" => 250,
            "/query/1" => 10,
            _ => 120,
        };
        Response::delayed(request.body.clone(), Duration::from_millis(delay))
    })
    .await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let mut stream =
        client.fan_out_stream::<_, Value>(targets(&server, receiver()?, 3), query, cfg(3));
    let first = stream
        .next()
        .await
        .ok_or_else(|| io::Error::other("no first reply"))?;
    assert_eq!(first.0, Uid(1));
    assert_eq!(first.1?, json!({"uid": 1}));
    let rest: Vec<_> = stream.collect().await;
    assert_eq!(
        rest.iter().map(|(uid, _)| *uid).collect::<Vec<_>>(),
        vec![Uid(2), Uid(0)]
    );
    assert!(rest.iter().all(|(_, result)| result.is_ok()));
    assert_eq!(server.peak(), 3);
    Ok(())
}

#[tokio::test]
async fn call_deadline_bounds_active_retries_and_queued_targets() -> TestResult {
    let server =
        Server::http(|_, _| Response::delayed(b"{}".to_vec(), Duration::from_secs(1))).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let calls = Arc::new(AtomicUsize::new(0));
    let materialized = calls.clone();
    let started = tokio::time::Instant::now();
    let mut results = client
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 20),
            move |target| {
                calls.fetch_add(1, Ordering::SeqCst);
                query(target)
            },
            FanOut {
                deadline: Some(Duration::from_millis(150)),
                retries: 4,
                ..cfg(2)
            },
        )
        .await;
    assert!(started.elapsed() < Duration::from_millis(850));
    results.sort_by_key(|(uid, _)| *uid);
    assert_eq!(results.len(), 20);
    for (number, (uid, result)) in results.into_iter().enumerate() {
        assert_eq!(usize::from(uid.get()), number);
        assert!(matches!(result, Err(FanOutError::Deadline)));
    }
    assert_eq!(materialized.load(Ordering::SeqCst), 2);
    assert_eq!(server.observations().await?.len(), 2);
    assert_eq!(server.peak(), 2);
    Ok(())
}

#[tokio::test]
async fn stream_deadline_starts_at_creation_even_when_unpolled() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let calls = Arc::new(AtomicUsize::new(0));
    let materialized = calls.clone();
    let stream = client.fan_out_stream::<_, Value>(
        targets(&server, receiver()?, 4),
        move |target| {
            calls.fetch_add(1, Ordering::SeqCst);
            query(target)
        },
        FanOut {
            deadline: Some(Duration::from_millis(30)),
            ..cfg(2)
        },
    );
    tokio::time::sleep(Duration::from_millis(70)).await;
    let results: Vec<_> = stream.collect().await;
    assert_eq!(results.len(), 4);
    assert!(
        results
            .iter()
            .all(|(_, result)| matches!(result, Err(FanOutError::Deadline)))
    );
    assert_eq!(materialized.load(Ordering::SeqCst), 0);
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn deadline_crossed_by_factory_or_serializer_prevents_signing_and_transmission() -> TestResult
{
    struct SlowJson {
        delay: Duration,
    }
    impl serde::Serialize for SlowJson {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            std::thread::sleep(self.delay);
            json!({"slow": true}).serialize(serializer)
        }
    }
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    for slow_serializer in [false, true] {
        let signer = BehaviorSigner::new(Duration::ZERO, false)?;
        let client = signed_client(signer.clone())?;
        let calls = Arc::new(AtomicUsize::new(0));
        let materialized = calls.clone();
        let results = client
            .fan_out::<_, Value>(
                targets(&server, receiver()?, 3),
                move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if !slow_serializer {
                        std::thread::sleep(Duration::from_millis(80));
                    }
                    (
                        Method::POST,
                        "/slow".into(),
                        SlowJson {
                            delay: if slow_serializer {
                                Duration::from_millis(80)
                            } else {
                                Duration::ZERO
                            },
                        },
                    )
                },
                FanOut {
                    deadline: Some(Duration::from_millis(20)),
                    ..cfg(2)
                },
            )
            .await;
        assert_eq!(results.len(), 3);
        assert!(
            results
                .iter()
                .all(|(_, result)| matches!(result, Err(FanOutError::Deadline)))
        );
        assert_eq!(materialized.load(Ordering::SeqCst), 1);
        assert_eq!(signer.started.load(Ordering::SeqCst), 0);
        assert!(server.observations().await?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn dropping_the_stream_cancels_pending_work_and_leaves_queued_targets_unsent() -> TestResult {
    let server =
        Server::http(|_, _| Response::delayed(b"{}".to_vec(), Duration::from_secs(1))).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let mut stream =
        client.fan_out_stream::<_, Value>(targets(&server, receiver()?, 9), query, cfg(2));
    assert!(
        tokio::time::timeout(Duration::from_millis(120), stream.next())
            .await
            .is_err()
    );
    assert_eq!(server.wait_for(2).await?.len(), 2);
    drop(stream);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn attempt_timeout_covers_signing_and_retries_are_fresh_attempts() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let signer = BehaviorSigner::new(Duration::from_secs(1), false)?;
    let client = signed_client(signer.clone())?;
    let receiver = receiver()?;
    let error = client
        .request(Method::POST, server.base_url.clone(), &receiver)
        .timeout(Duration::from_millis(25))
        .send()
        .await;
    assert!(matches!(error, Err(ClientError::Timeout)));
    let results = client
        .fan_out::<_, Value>(
            targets(&server, receiver, 1),
            query,
            FanOut {
                timeout: Duration::from_millis(25),
                retries: 2,
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].1, Err(FanOutError::Timeout)));
    assert_eq!(signer.started.load(Ordering::SeqCst), 4);
    assert_eq!(signer.completed.load(Ordering::SeqCst), 0);
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn direct_timeout_retains_the_remaining_budget_while_consuming_response_body() -> TestResult {
    let server = Server::http(|_, _| Response {
        body_delay: Duration::from_millis(150),
        ..Response::json(b"{\"late\":true}".to_vec())
    })
    .await?;
    let signer = BehaviorSigner::new(Duration::from_millis(80), false)?;
    let client = signed_client(signer)?;
    let response = client
        .request(Method::POST, server.base_url.clone(), &receiver()?)
        .timeout(Duration::from_millis(200))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let error = response
        .bytes()
        .await
        .err()
        .ok_or_else(|| io::Error::other("response exceeded the remaining request budget"))?;
    assert!(error.is_timeout());
    assert_eq!(server.observations().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn fanout_timeout_covers_body_consumption_after_successful_headers() -> TestResult {
    let server = Server::http(|_, _| Response {
        body_delay: Duration::from_secs(1),
        ..Response::json(b"{\"late\":true}".to_vec())
    })
    .await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let result = client
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 1),
            query,
            FanOut {
                timeout: Duration::from_millis(100),
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(result.len(), 1);
    // Either the encompassing Tokio timer or reqwest's body timer may win.
    match &result[0].1 {
        Err(FanOutError::Timeout) => {}
        Err(FanOutError::Request(ClientError::Http(error))) => assert!(error.is_timeout()),
        other => {
            return Err(io::Error::other(format!("expected body timeout, got {other:?}")).into());
        }
    }
    assert_eq!(server.observations().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn both_fanout_apis_reject_oversized_declared_lengths_before_body_read_or_retry() -> TestResult
{
    let server = Server::http(|_, _| {
        let mut response = Response {
            body_delay: Duration::from_secs(30),
            ..Response::json(b"not needed".to_vec())
        };
        response
            .headers
            .push(("Content-Length".into(), "1024".into()));
        response
    })
    .await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for streaming in [false, true] {
        let (uid, reply) = one_fanout_reply(&client, &server, streaming, 11).await?;
        assert_eq!(uid, Uid(0));
        assert!(matches!(
            reply,
            Err(FanOutError::ResponseTooLarge { limit: 11 })
        ));
    }
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn both_fanout_apis_accept_fixed_length_json_at_the_exact_response_budget() -> TestResult {
    let body = b"{\"ok\":true}";
    let server = Server::http(move |_, _| Response::json(body.to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for streaming in [false, true] {
        let (uid, reply) = one_fanout_reply(&client, &server, streaming, body.len()).await?;
        assert_eq!(uid, Uid(0));
        assert_eq!(reply?, json!({"ok": true}));
    }
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn both_fanout_apis_stop_chunked_overflow_before_eof_decoding_or_retry() -> TestResult {
    // The first chunk fills the budget exactly. The next byte exceeds it;
    // deliberately omit the terminal chunk, so collecting to EOF would fail
    // as a transport error rather than the response-size error required here.
    let server = Server::http(|_, _| chunked_response(&[b"{\"ok\":true}", b" "], false)).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for streaming in [false, true] {
        let (uid, reply) = one_fanout_reply(&client, &server, streaming, 11).await?;
        assert_eq!(uid, Uid(0));
        assert!(matches!(
            reply,
            Err(FanOutError::ResponseTooLarge { limit: 11 })
        ));
    }
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn both_fanout_apis_accept_chunked_json_at_the_exact_response_budget() -> TestResult {
    let server = Server::http(|_, _| chunked_response(&[b"{\"ok\":", b"true}"], true)).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for streaming in [false, true] {
        let (uid, reply) = one_fanout_reply(&client, &server, streaming, 11).await?;
        assert_eq!(uid, Uid(0));
        assert_eq!(reply?, json!({"ok": true}));
    }
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn status_decode_and_signing_failures_keep_uid_and_error_identity() -> TestResult {
    let server = Server::http(|_, request| match request.target.as_str() {
        "/query/0" => Response {
            status: 401,
            ..Response::json(b"{}".to_vec())
        },
        "/query/1" => Response::json(b"not json".to_vec()),
        _ => Response::json(request.body.clone()),
    })
    .await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let mut results = client
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 3),
            query,
            FanOut {
                retries: 3,
                ..cfg(3)
            },
        )
        .await;
    results.sort_by_key(|(uid, _)| *uid);
    assert_eq!(results.len(), 3);
    assert!(matches!(
        results[0],
        (Uid(0), Err(FanOutError::Status(StatusCode::UNAUTHORIZED)))
    ));
    assert!(matches!(results[1], (Uid(1), Err(FanOutError::Decode(_)))));
    assert_eq!(results[2].1.as_ref().ok(), Some(&json!({"uid": 2})));
    assert_eq!(server.observations().await?.len(), 3);
    assert!(
        results[1]
            .1
            .as_ref()
            .err()
            .and_then(Error::source)
            .is_some()
    );
    let signer = BehaviorSigner::new(Duration::ZERO, true)?;
    let refusing = signed_client(signer.clone())?;
    let errors = refusing
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 1),
            query,
            FanOut {
                retries: 3,
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0],
        (
            Uid(0),
            Err(FanOutError::Request(ClientError::Auth(AuthError::Signing(
                SignerError::RawSigningUnsupported
            ))))
        )
    ));
    assert_eq!(signer.started.load(Ordering::SeqCst), 1);
    assert!(
        errors[0]
            .1
            .as_ref()
            .err()
            .and_then(Error::source)
            .and_then(Error::source)
            .and_then(Error::source)
            .is_some()
    );
    assert_eq!(server.observations().await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn value_first_response_json_handles_untagged_floats() -> TestResult {
    #[derive(Debug, serde::Deserialize, PartialEq)]
    #[serde(untagged)]
    enum FloatReply {
        Number(f64),
        Text(String),
    }
    let server = Server::http(|_, _| Response::json(b"0.125".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let results = client
        .fan_out::<_, FloatReply>(targets(&server, receiver()?, 1), query, cfg(1))
        .await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.as_ref().ok(), Some(&FloatReply::Number(0.125)));
    Ok(())
}

#[tokio::test]
async fn serialization_error_stays_sticky_and_is_not_retried_or_transmitted() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let error = client
        .request(Method::POST, server.base_url.clone(), &receiver()?)
        .json(&FailingJson)
        .body(b"replacement".to_vec())
        .send()
        .await;
    assert!(matches!(error, Err(ClientError::Json(_))));
    let errors = client
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 2),
            |_| (Method::POST, "/".into(), FailingJson),
            FanOut {
                retries: 3,
                ..cfg(2)
            },
        )
        .await;
    assert_eq!(errors.len(), 2);
    for (_, error) in errors {
        let error = error
            .err()
            .ok_or_else(|| io::Error::other("expected serialization error"))?;
        assert!(matches!(error, FanOutError::Request(ClientError::Json(_))));
        assert_eq!(error.to_string(), "deliberate serialization failure");
        assert!(error.source().and_then(Error::source).is_some());
    }
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn unsafe_urls_and_authority_changing_fanout_paths_are_rejected() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for text in [
        "ftp://127.0.0.1/query",
        "http://user@127.0.0.1/query",
        "http://user:password@127.0.0.1/query",
        "http://127.0.0.1/query#fragment",
    ] {
        let error = client
            .request(Method::GET, url::Url::parse(text)?, &receiver()?)
            .send()
            .await;
        assert!(matches!(error, Err(ClientError::InvalidUrl(_))), "{text}");
    }
    for path in [
        "//elsewhere.test/query",
        "http://elsewhere.test/query",
        "/query#fragment",
        "/\\elsewhere.test/query",
        "relative/query",
    ] {
        let errors = client
            .fan_out::<_, Value>(
                targets(&server, receiver()?, 1),
                move |_| (Method::POST, path.into(), json!({})),
                cfg(1),
            )
            .await;
        assert_eq!(errors.len(), 1);
        assert!(
            matches!(
                errors[0].1,
                Err(FanOutError::Request(ClientError::InvalidUrl(_)))
            ),
            "{path}"
        );
    }
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn connect_is_rejected_before_signing_or_transmitting_an_authority_form_target() -> TestResult
{
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let signer = BehaviorSigner::new(Duration::ZERO, false)?;
    let client = signed_client(signer.clone())?;
    let receiver = receiver()?;
    let result = client
        .request(
            Method::CONNECT,
            server.base_url.join("/wrong-target?x=%2F")?,
            &receiver,
        )
        .body(b"connect body".to_vec())
        .send()
        .await;
    assert!(matches!(result, Err(ClientError::InvalidMethod(_))));
    let mut results = client
        .fan_out::<_, Value>(
            targets(&server, receiver, 3),
            |_| (Method::CONNECT, "/wrong-target?x=%2F".into(), json!({})),
            FanOut {
                retries: 3,
                ..cfg(3)
            },
        )
        .await;
    results.sort_by_key(|(uid, _)| *uid);
    assert_eq!(results.len(), 3);
    for (index, (uid, result)) in results.into_iter().enumerate() {
        assert_eq!(usize::from(uid.get()), index);
        assert!(matches!(
            result,
            Err(FanOutError::Request(ClientError::InvalidMethod(_)))
        ));
    }
    assert_eq!(signer.started.load(Ordering::SeqCst), 0);
    assert_eq!(signer.completed.load(Ordering::SeqCst), 0);
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn synchronous_response_decoding_cannot_return_success_after_attempt_or_call_deadline()
-> TestResult {
    static DECODE_CALLS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct SlowReply;
    impl<'de> serde::Deserialize<'de> for SlowReply {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let millis = <u64 as serde::Deserialize>::deserialize(deserializer)?;
            DECODE_CALLS.fetch_add(1, Ordering::SeqCst);
            // Unlike an async sleep, this completes inside one future poll.
            std::thread::sleep(Duration::from_millis(millis));
            Ok(Self)
        }
    }
    let server = Server::http(|_, _| Response::json(b"350".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for call_deadline in [false, true] {
        let before = DECODE_CALLS.load(Ordering::SeqCst);
        let results = client
            .fan_out::<_, SlowReply>(
                targets(&server, receiver()?, 1),
                query,
                if call_deadline {
                    FanOut {
                        deadline: Some(Duration::from_millis(200)),
                        ..cfg(1)
                    }
                } else {
                    FanOut {
                        timeout: Duration::from_millis(200),
                        ..cfg(1)
                    }
                },
            )
            .await;
        assert_eq!(DECODE_CALLS.load(Ordering::SeqCst), before + 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, Uid(0));
        if call_deadline {
            assert!(matches!(results[0].1, Err(FanOutError::Deadline)));
        } else {
            assert!(matches!(results[0].1, Err(FanOutError::Timeout)));
        }
    }
    assert_eq!(server.observations().await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn synchronous_signing_that_crosses_the_call_deadline_never_transmits() -> TestResult {
    struct BlockingSigner {
        key: KeypairSigner,
        started: AtomicUsize,
    }
    impl Signer for BlockingSigner {
        fn account(&self) -> AccountId {
            self.key.account()
        }
        fn crypto_type(&self) -> CryptoType {
            self.key.crypto_type()
        }
        fn sign<'a>(
            &'a self,
            request: &'a SigningRequest<'a>,
        ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
            self.key.sign(request)
        }
        fn sign_raw<'a>(
            &'a self,
            message: &'a [u8],
        ) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
            Box::pin(async move {
                self.started.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                self.key.sign_raw(message).await
            })
        }
    }
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let signer = Arc::new(BlockingSigner {
        key: KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?,
        started: AtomicUsize::new(0),
    });
    let client = signed_client(signer.clone())?;
    let results = client
        .fan_out::<_, Value>(
            targets(&server, receiver()?, 3),
            query,
            FanOut {
                deadline: Some(Duration::from_millis(50)),
                retries: 3,
                ..cfg(1)
            },
        )
        .await;
    assert_eq!(results.len(), 3);
    assert!(
        results
            .iter()
            .all(|(_, result)| matches!(result, Err(FanOutError::Deadline)))
    );
    assert_eq!(signer.started.load(Ordering::SeqCst), 1);
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_fanout_configuration_returns_an_error_for_every_uid() -> TestResult {
    let server = Server::http(|_, _| Response::json(b"{}".to_vec())).await?;
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    for config in [
        FanOut {
            concurrency: 0,
            ..cfg(1)
        },
        FanOut {
            timeout: Duration::ZERO,
            ..cfg(1)
        },
        FanOut {
            connect_timeout: Duration::ZERO,
            ..cfg(1)
        },
        FanOut {
            deadline: Some(Duration::MAX),
            ..cfg(1)
        },
        FanOut {
            max_response_bytes: 0,
            ..cfg(1)
        },
    ] {
        let results = client
            .fan_out::<_, Value>(targets(&server, receiver()?, 4), query, config)
            .await;
        assert_eq!(results.len(), 4);
        for (number, (uid, error)) in results.into_iter().enumerate() {
            assert_eq!(usize::from(uid.get()), number);
            assert!(matches!(
                error,
                Err(FanOutError::Request(ClientError::Configuration(_)))
            ));
        }
    }
    assert!(
        client
            .fan_out::<_, Value>(Vec::<AxonTarget>::new(), query, cfg(1))
            .await
            .is_empty()
    );
    assert!(server.observations().await?.is_empty());
    Ok(())
}

#[test]
fn fanout_defaults_match_validator_timing_budget() {
    let config = FanOut::default();
    assert_eq!(config.concurrency, 256);
    assert_eq!(config.connect_timeout, Duration::from_secs(3));
    assert_eq!(config.timeout, Duration::from_secs(12));
    assert_eq!(config.deadline, Some(Duration::from_secs(13)));
    assert_eq!(config.retries, 0);
    assert_eq!(config.max_response_bytes, 1024 * 1024);
}

#[tokio::test]
async fn fanout_connect_timeout_bounds_a_stalled_tls_handshake() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        // Hold the accepted TCP connection without returning TLS handshake
        // bytes. This distinguishes the connection budget from attempt timing.
        std::future::pending::<()>().await;
        drop(socket);
        Ok::<(), std::io::Error>(())
    });
    let client = signed_client(signer(CryptoType::Sr25519)?)?;
    let started = tokio::time::Instant::now();
    let results = client
        .fan_out::<_, Value>(
            [AxonTarget {
                uid: Uid(42),
                hotkey: receiver()?,
                base_url: url::Url::parse(&format!("https://{address}"))?,
            }],
            query,
            FanOut {
                connect_timeout: Duration::from_millis(40),
                timeout: Duration::from_secs(2),
                deadline: None,
                ..cfg(1)
            },
        )
        .await;
    peer.abort();
    let _ = peer.await;
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Uid(42));
    match &results[0].1 {
        Err(FanOutError::Request(ClientError::Http(error))) => {
            assert!(error.is_connect());
            assert!(error.is_timeout());
        }
        other => {
            return Err(
                io::Error::other(format!("expected connect timeout, got {other:?}")).into(),
            );
        }
    }
    Ok(())
}
