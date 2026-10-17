//! Rust dendrite against the Rust axon over loopback TCP.

#![cfg(all(feature = "axon", feature = "dendrite", feature = "tensor"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "../examples/interop/synapses.rs"]
mod synapses;

use std::{sync::Arc, time::Duration};

use futures::StreamExt;
use rtao_compat_synapse::{BaseSynapse, CallOptions, Dendrite, PyDict, UuidMode, V10AxonConfig};
use rtao_primitives::{CryptoType, Uid, axon::AxonTarget};
use rtao_signer::{KeypairSigner, Signer};
use synapses::*;

async fn serve() -> (AxonTarget, tokio::sync::oneshot::Sender<()>) {
    let bob = Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519).unwrap());
    let hotkey = bob.account();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(
        build_axon(bob, V10AxonConfig::default()).serve(listener, async move {
            let _ = stopped.await;
        }),
    );
    let target = AxonTarget {
        uid: Uid(1),
        hotkey,
        base_url: format!("http://127.0.0.1:{port}").parse().unwrap(),
    };
    (target, stop)
}

fn dendrite() -> Dendrite {
    let alice = Arc::new(KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap());
    Dendrite::new(alice).unwrap()
}

fn status<S>(envelope: &rtao_compat_synapse::Envelope<S>) -> (Option<i64>, Option<String>) {
    let dendrite = envelope.dendrite.clone().unwrap_or_default();
    (dendrite.status_code, dendrite.status_message)
}

#[tokio::test]
async fn round_trips_and_merges() {
    let (target, _stop) = serve().await;
    let dendrite = dendrite();
    let options = CallOptions::default();

    let mixed = Mixed {
        i: 3,
        f: 0.1,
        s: "x'y\u{e9}".into(),
        l: vec!["q".into()],
        d: PyDict(vec![("z".into(), vec![1.0]), ("a".into(), vec![])]),
        n: Some(1),
        out: None,
    };
    let reply = dendrite.call(&target, mixed.clone(), &options).await;
    assert_eq!(status(&reply), (Some(200), Some("Success".into())));
    assert_eq!(reply.body.out.as_deref(), Some("3|x'y\u{e9}|1"));
    assert_eq!(reply.body.d, mixed.d, "dict order survives both directions");
    let axon = reply.axon.unwrap();
    assert_eq!(axon.status_code, Some(200));
    assert!(axon.process_time.is_some());
    assert_eq!(
        reply.computed_body_hash.as_deref(),
        Some(""),
        "frozen field kept"
    );

    let reply = dendrite.call(&target, Status404 {}, &options).await;
    assert_eq!(status(&reply).0, Some(404));
    let reply = dendrite.call(&target, Deny {}, &options).await;
    assert_eq!(
        status(&reply),
        (
            Some(403),
            Some("Forbidden. Key is blacklisted: denied by test.".into())
        )
    );
    let reply = dendrite.call(&target, NanResp::default(), &options).await;
    assert_eq!((status(&reply).0, reply.body.v), (Some(200), None));

    let short = CallOptions {
        timeout: Duration::from_millis(300),
        ..CallOptions::default()
    };
    let reply = dendrite.call(&target, Slow { delay: 1.0 }, &short).await;
    assert_eq!(
        status(&reply),
        (Some(408), Some("Request timeout after 0.3 seconds".into()))
    );

    let (chunks, done) = dendrite.call_stream(&target, Echo::default(), &options);
    let chunks: Vec<_> = chunks.collect().await;
    assert_eq!(chunks.len(), 3);
    assert_eq!(status(&done.await.unwrap()).0, Some(200));

    let targets = vec![target.clone(); 4];
    let per_request = CallOptions {
        uuid_mode: UuidMode::PerRequest,
        ..CallOptions::default()
    };
    let replies = dendrite
        .forward(&targets, BaseSynapse {}, &per_request, 4)
        .await;
    assert!(replies.iter().all(|reply| status(reply).0 == Some(200)));

    let nan = Mixed {
        f: f64::NAN,
        ..Mixed::default()
    };
    assert!(
        dendrite
            .prepare(&target, nan.clone(), &options)
            .await
            .is_err()
    );
    let reply = dendrite.call(&target, nan, &options).await;
    assert_eq!(status(&reply).0, Some(422));
}

#[tokio::test]
async fn unreachable_axon_is_503() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let target = AxonTarget {
        uid: Uid(2),
        hotkey: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
            .parse()
            .unwrap(),
        base_url: format!("http://127.0.0.1:{port}").parse().unwrap(),
    };
    let reply = dendrite()
        .call(&target, BaseSynapse {}, &CallOptions::default())
        .await;
    assert_eq!(
        status(&reply),
        (
            Some(503),
            Some(format!("Service unavailable at 127.0.0.1:{port}/Synapse"))
        )
    );
}

#[tokio::test]
async fn response_size_is_capped() {
    let (target, _stop) = serve().await;
    let reply = dendrite()
        .with_max_response_bytes(16)
        .call(&target, BaseSynapse {}, &CallOptions::default())
        .await;
    assert_eq!(
        status(&reply),
        (
            Some(422),
            Some("Failed to parse response: response exceeds 16 bytes".into())
        )
    );
}

/// A raw HTTP peer that answers every request with `status` and a body of
/// `length` spaces.
async fn raw_peer(status: &'static str, length: usize) -> AxonTarget {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                // Read the head and the declared body before answering.
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {length}\r\n\r\n"
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let chunk = vec![b' '; 64 * 1024];
                let mut sent = 0;
                while sent < length {
                    let take = chunk.len().min(length - sent);
                    if socket.write_all(&chunk[..take]).await.is_err() {
                        return;
                    }
                    sent += take;
                }
            });
        }
    });
    AxonTarget {
        uid: Uid(3),
        hotkey: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
            .parse()
            .unwrap(),
        base_url: format!("http://127.0.0.1:{port}").parse().unwrap(),
    }
}

#[tokio::test]
async fn streaming_error_bodies_are_capped() {
    // 64 MiB of error body; the dendrite keeps at most 4 KiB.
    let target = raw_peer("503 Service Unavailable", 64 * 1024 * 1024).await;
    let (chunks, done) = dendrite().with_max_response_bytes(4096).call_stream(
        &target,
        Echo::default(),
        &CallOptions::default(),
    );
    let chunks: Vec<_> = chunks.collect().await;
    assert!(chunks.is_empty());
    let envelope = done.await.unwrap();
    assert_eq!(
        status(&envelope),
        (
            Some(422),
            Some("Failed to parse response: response exceeds 4096 bytes".into())
        )
    );

    // A small error body is still read and decoded (here: blank, as aiohttp
    // would fail to decode it).
    let target = raw_peer("503 Service Unavailable", 2).await;
    let (chunks, done) = dendrite().call_stream(&target, Echo::default(), &CallOptions::default());
    assert!(chunks.collect::<Vec<_>>().await.is_empty());
    let envelope = done.await.unwrap();
    assert_eq!(
        status(&envelope),
        (
            Some(422),
            Some("Failed to parse response: empty response body".into())
        )
    );
}
