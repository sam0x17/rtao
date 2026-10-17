//! Golden wire vectors (spec §12) and handshake verification rules.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use rtao_btauth::{BoxFuture, InMemoryNonceStore, NonceStore, NonceStoreConfig, StoreError};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, Signer};
use rtao_transport_quic::auth::{
    self, AuthError, HandshakeReject, HandshakeVerifier, cert_fingerprint, fingerprint_b64,
    handshake_request_message, handshake_response_message, is_valid_nonce, sign_handshake,
    sign_handshake_response, sign_handshake_with_nonce, verify_b64, verify_handshake_response,
};
use rtao_transport_quic::data::{from_data, from_value, to_data, to_value};
use rtao_transport_quic::wire::{
    DataMap, HandshakeRequest, HandshakeResponse, MessageType, StreamChunk, StreamEnd,
    SynapsePacket, SynapseResponse, Value, WireError, decode, encode, encode_frame, parse_frame,
    read_frame, write_budget,
};
use serde::{Deserialize, Serialize};

const TS: u64 = 1_700_000_000;
const MINER: &str = "5CcyqxXnJucaCnQQvvUg5EPzj1uoNAxACZvzArHw5aVDvgNH";
const VALIDATOR: &str = "5CfCr47V5Dte6bwxNBE8K9oNnQd9fiay6aDEEkgYtFv7w4Fq";
const ZERO_FP: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn h(s: &str) -> Vec<u8> {
    hex::decode(s.replace(' ', "")).unwrap()
}

fn signer(seed: u8) -> KeypairSigner {
    KeypairSigner::from_seed(&[seed; 32], CryptoType::Sr25519).unwrap()
}

fn one(key: &str, value: Value) -> DataMap {
    DataMap::from([(key.to_owned(), value)])
}

fn verifier(max_age: u64) -> HandshakeVerifier {
    let store = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: HandshakeVerifier::required_retention(max_age),
        ..Default::default()
    })
    .unwrap();
    HandshakeVerifier::new(max_age, Arc::new(store)).unwrap()
}

#[test]
fn g0_seed_addresses() {
    assert_eq!(auth::signer_ss58(&signer(1)), MINER);
    assert_eq!(auth::signer_ss58(&signer(2)), VALIDATOR);
}

#[test]
fn g1_frame_parse() {
    let frame = h("01 00000005 68656c6c6f");
    let (kind, payload) = parse_frame(&frame, 1 << 26).unwrap();
    assert_eq!(
        (kind, payload),
        (MessageType::HandshakeRequest, &b"hello"[..])
    );
    assert!(matches!(
        parse_frame(&h("01 00"), 1 << 26),
        Err(WireError::Truncated { .. })
    ));
    assert!(matches!(
        parse_frame(&h("01 0000000a 73686f7274"), 1 << 26),
        Err(WireError::Truncated { got: 5, want: 10 })
    ));
    let mut big = vec![0x01];
    big.extend_from_slice(&(64u32 * 1024 * 1024 + 1).to_be_bytes());
    assert!(matches!(
        parse_frame(&big, 64 * 1024 * 1024),
        Err(WireError::Oversize { .. })
    ));
    // The type byte is checked before the length.
    let mut bad = vec![0xff];
    bad.extend_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        parse_frame(&bad, 1 << 20),
        Err(WireError::UnknownType(0xff))
    );
    assert_eq!(
        WireError::UnknownType(0xff).to_string(),
        "unknown message type: 0xff"
    );
}

#[tokio::test]
async fn g1_stream_reader() {
    let mut src: &[u8] = &h("05 00000005 91c4026869 06 00000003 92c3c0");
    let (k, p) = read_frame(&mut src, 1 << 20).await.unwrap();
    assert_eq!(k, MessageType::StreamChunk);
    assert_eq!(decode::<StreamChunk>(&p).unwrap().data, b"hi");
    let (k, p) = read_frame(&mut src, 1 << 20).await.unwrap();
    assert_eq!(k, MessageType::StreamEnd);
    assert_eq!(
        decode::<StreamEnd>(&p).unwrap(),
        StreamEnd {
            success: true,
            error: None
        }
    );
    assert_eq!(
        read_frame(&mut src, 1 << 20).await,
        Err(WireError::Truncated { got: 0, want: 5 })
    );
    // A large declared length with few bytes sent fails without a large allocation.
    let mut frame = vec![0x03];
    frame.extend_from_slice(&(32u32 * 1024 * 1024).to_be_bytes());
    frame.extend_from_slice(&[0u8; 100]);
    let mut src: &[u8] = &frame;
    assert_eq!(
        read_frame(&mut src, 64 * 1024 * 1024).await,
        Err(WireError::Truncated {
            got: 100,
            want: 32 * 1024 * 1024
        })
    );
    let mut src: &[u8] = &h("03 00000000");
    assert_eq!(
        read_frame(&mut src, 1 << 20).await.unwrap().1,
        Vec::<u8>::new()
    );
}

/// Yields `data`, then fails every later read like a reset QUIC stream.
struct FailingReader {
    data: Vec<u8>,
    pos: usize,
    reads_after_error: usize,
}

impl tokio::io::AsyncRead for FailingReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let rest = &self.data[self.pos..];
        if rest.is_empty() {
            self.reads_after_error += 1;
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "stream reset by peer",
            )));
        }
        let n = rest.len().min(buf.remaining());
        buf.put_slice(&rest[..n]);
        self.pos += n;
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn g1_stream_errors_end_the_read() {
    // Small (single allocation) and large (incremental) payload paths, and
    // an error inside the header. Each read stops at the first error rather
    // than filling the declared length with zeros.
    for (declared, sent) in [
        (10u32, 3usize),
        (64 * 1024 * 1024, 100),
        (64 * 1024 * 1024, 0),
    ] {
        let mut data = vec![0x03];
        data.extend_from_slice(&declared.to_be_bytes());
        data.extend(std::iter::repeat_n(7u8, sent));
        let mut reader = FailingReader {
            data,
            pos: 0,
            reads_after_error: 0,
        };
        let result = read_frame(&mut reader, 64 * 1024 * 1024).await;
        assert!(
            matches!(&result, Err(WireError::Io(e)) if e.contains("reset")),
            "declared {declared}: {result:?}"
        );
        assert_eq!(reader.reads_after_error, 1);
    }
    let mut reader = FailingReader {
        data: vec![0x03, 0x00],
        pos: 0,
        reads_after_error: 0,
    };
    assert!(matches!(
        read_frame(&mut reader, 1 << 20).await,
        Err(WireError::Io(_))
    ));
}

#[test]
fn g2_signed_messages() {
    assert_eq!(
        handshake_request_message("5GrwvaEF", 1234567890, "abc123", "fp_b64"),
        "handshake:5GrwvaEF:1234567890:abc123:fp_b64"
    );
    assert_eq!(
        handshake_response_message("5GrwvaEF", "5FHneW46", 1234567890, "abc123", "fp_b64"),
        "handshake_response:5GrwvaEF:5FHneW46:1234567890:abc123:fp_b64"
    );
}

#[test]
fn g3_fingerprint() {
    let fp = cert_fingerprint(b"");
    assert_eq!(
        hex::encode(fp),
        "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"
    );
    assert_eq!(
        fingerprint_b64(&fp),
        "DldRwCblQ7Loqy6wYJnaodHl30d3j3eH+qtFzfEv46g="
    );
    assert_eq!(fingerprint_b64(&[0; 32]), ZERO_FP);
}

#[tokio::test]
async fn g4_request_signature() {
    let nonce = "00000000000000000000000000000001".to_owned();
    let req = sign_handshake_with_nonce(&signer(2), ZERO_FP, TS, nonce.clone())
        .await
        .unwrap();
    let msg = handshake_request_message(VALIDATOR, TS, &nonce, ZERO_FP);
    assert_eq!(
        msg,
        "handshake:5CfCr47V5Dte6bwxNBE8K9oNnQd9fiay6aDEEkgYtFv7w4Fq:1700000000:00000000000000000000000000000001:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    );
    assert_eq!(msg.len(), 147);
    assert_eq!(req.signature.len(), 88);
    assert!(verify_b64(VALIDATOR, &req.signature, &msg).unwrap());
    assert!(!verify_b64(MINER, &req.signature, &msg).unwrap());
}

#[test]
fn g5_to_g9_envelopes() {
    let packet = SynapsePacket {
        synapse_type: "echo".into(),
        data: one("msg", Value::from("hi")),
        timestamp: TS,
    };
    let bytes = encode(&packet).unwrap();
    assert_eq!(bytes, h("93 a4 6563686f 81 a3 6d7367 a2 6869 ce 6553f100"));
    assert_eq!(
        encode_frame(MessageType::SynapsePacket, &bytes).unwrap()[..5],
        h("03 00000013")
    );

    let ok = SynapseResponse {
        success: true,
        data: one("msg", Value::from("hi")),
        timestamp: TS,
        error: None,
    };
    assert_eq!(
        encode(&ok).unwrap(),
        h("94 c3 81 a3 6d7367 a2 6869 ce 6553f100 c0")
    );

    assert_eq!(
        encode(&SynapseResponse::error("unrecognized synapse type", TS)).unwrap(),
        h("94 c2 80 ce6553f100 b9 756e7265636f676e697a65642073796e617073652074797065")
    );
    assert_eq!(
        encode(&SynapseResponse::error("authentication failed", TS)).unwrap(),
        h("94 c2 80 ce6553f100 b5 61757468656e7469636174696f6e206661696c6564")
    );

    let chunk = encode(&StreamChunk {
        data: b"hi".to_vec(),
    })
    .unwrap();
    assert_eq!(
        encode_frame(MessageType::StreamChunk, &chunk).unwrap(),
        h("05 00000005 91c4026869")
    );
    let end = encode(&StreamEnd {
        success: true,
        error: None,
    })
    .unwrap();
    assert_eq!(
        encode_frame(MessageType::StreamEnd, &end).unwrap(),
        h("06 00000003 92c3c0")
    );
    assert_eq!(
        encode(&StreamEnd::error("handler timed out")).unwrap(),
        h("92 c2 b1 68616e646c65722074696d6564206f7574")
    );

    let pre = encode(&HandshakeResponse::rejected_preauth(TS)).unwrap();
    assert_eq!(pre, h("96 a0 ce6553f100 a0 c2 a0 c0"));
    assert_eq!(
        encode_frame(MessageType::HandshakeResponse, &pre).unwrap()[..5],
        h("02 0000000b")
    );
}

#[tokio::test]
async fn g10_handshake_layout() {
    let req = sign_handshake(&signer(2), ZERO_FP, TS).await.unwrap();
    let bytes = encode(&req).unwrap();
    assert_eq!(bytes.len(), 180);
    assert_eq!(bytes[..3], h("94 d9 30"));
    assert_eq!(bytes[51..56], h("ce 6553f100"));
    assert_eq!(bytes[56..58], h("d9 20"));
    assert_eq!(bytes[90..92], h("d9 58"));
    assert!(
        req.nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );

    let resp = sign_handshake_response(
        &signer(1),
        &req,
        ZERO_FP,
        TS,
        format!("conn_{VALIDATOR}_1700000000123"),
    )
    .await
    .unwrap();
    let bytes = encode(&resp).unwrap();
    assert_eq!(bytes[..3], h("96 d9 30"));
    let tail = 3 + 48 + 5;
    assert_eq!(bytes[tail..tail + 2], h("d9 58"));
    let after_sig = tail + 2 + 88;
    assert_eq!(bytes[after_sig..after_sig + 3], h("c3 d9 43"));
    let fp_at = after_sig + 3 + 67;
    assert_eq!(bytes[fp_at..fp_at + 2], h("d9 2c"));
    assert_eq!(bytes.len(), fp_at + 2 + 44);
    let pk = AccountId(rtao_core::keys::public_key_from_ss58(MINER).unwrap());
    verify_handshake_response(&resp, &pk, &req, ZERO_FP).unwrap();
}

#[test]
fn g11_map_form_and_lengths() {
    #[derive(Serialize)]
    struct Named<'a> {
        validator_hotkey: &'a str,
        timestamp: u64,
        nonce: &'a str,
        signature: &'a str,
    }
    let named = rmp_serde::to_vec_named(&Named {
        validator_hotkey: VALIDATOR,
        timestamp: TS,
        nonce: "ab",
        signature: "sig",
    })
    .unwrap();
    assert_eq!(named[0], 0x84);
    let req: HandshakeRequest = decode(&named).unwrap();
    assert_eq!(req.validator_hotkey, VALIDATOR);
    assert_eq!(req.timestamp, TS);

    // Five-element HandshakeRequest arrays are a length mismatch.
    let five = h("95 a1 61 01 a1 62 a1 63 c0");
    assert!(decode::<HandshakeRequest>(&five).is_err());
    // A five-element HandshakeResponse defaults the fingerprint to None.
    let resp: HandshakeResponse = decode(&h("95 a0 ce6553f100 a0 c2 a0")).unwrap();
    assert_eq!(resp.cert_fingerprint, None);
}

#[test]
fn g12_nonce_validity() {
    assert!(is_valid_nonce(&"0".repeat(32)));
    assert!(is_valid_nonce("ABCDEFabcdef0123456789ABCDEFabcd"));
    assert!(!is_valid_nonce(&"0".repeat(31)));
    assert!(!is_valid_nonce(&"0".repeat(33)));
    assert!(!is_valid_nonce(&format!("g{}", "0".repeat(31))));
    assert!(!is_valid_nonce(&format!("+{}", "0".repeat(31))));
    let n = auth::new_nonce().unwrap();
    assert!(is_valid_nonce(&n));
    assert_eq!(n, n.to_lowercase());
}

async fn signed(ts: u64) -> HandshakeRequest {
    sign_handshake(&signer(2), ZERO_FP, ts).await.unwrap()
}

#[tokio::test]
async fn g13_freshness_and_replay() {
    let now = TS;
    let v = verifier(300);
    let admit = |_: &AccountId| true;
    assert_eq!(
        v.verify(&signed(now - 300).await, ZERO_FP, now, admit)
            .await,
        Err(HandshakeReject::Stale)
    );
    assert!(
        v.verify(&signed(now - 299).await, ZERO_FP, now, admit)
            .await
            .is_ok()
    );
    let future = signed(now + 60).await;
    assert!(v.verify(&future, ZERO_FP, now, admit).await.is_ok());
    assert_eq!(
        v.verify(&signed(now + 61).await, ZERO_FP, now, admit).await,
        Err(HandshakeReject::Future)
    );
    // Still fresh at now+299: rejected as a replay. The store's own clock
    // does not move here; g13_retention_covers_freshness checks retention.
    assert_eq!(
        v.verify(&future, ZERO_FP, now + 299, admit).await,
        Err(HandshakeReject::Replay)
    );
    // Signature over a different fingerprint fails.
    assert_eq!(
        v.verify(
            &signed(now).await,
            "DldRwCblQ7Loqy6wYJnaodHl30d3j3eH+qtFzfEv46g=",
            now,
            admit
        )
        .await,
        Err(HandshakeReject::BadSignature)
    );
}

#[tokio::test]
async fn g13_policy_runs_before_nonce_store() {
    let v = verifier(300);
    let req = signed(TS).await;
    assert_eq!(
        v.verify(&req, ZERO_FP, TS, |_| false).await,
        Err(HandshakeReject::NotPermitted)
    );
    // The refused attempt did not consume the nonce.
    assert!(v.verify(&req, ZERO_FP, TS, |_| true).await.is_ok());
}

#[tokio::test]
async fn g13_full_store_fails_closed() {
    let store = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: HandshakeVerifier::required_retention(300),
        max_per_hotkey: 1,
        max_entries: 1,
    })
    .unwrap();
    let v = HandshakeVerifier::new(300, Arc::new(store)).unwrap();
    let first = signed(TS).await;
    assert!(v.verify(&first, ZERO_FP, TS, |_| true).await.is_ok());
    assert!(matches!(
        v.verify(&signed(TS).await, ZERO_FP, TS, |_| true).await,
        Err(HandshakeReject::Store(_))
    ));
    assert_eq!(
        v.verify(&first, ZERO_FP, TS, |_| true).await,
        Err(HandshakeReject::Replay)
    );
}

#[test]
fn verifier_requires_retention() {
    let short = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: Duration::from_secs(300),
        ..Default::default()
    })
    .unwrap();
    assert!(HandshakeVerifier::new(300, Arc::new(short)).is_err());
}

/// A nonce store whose monotonic clock the test moves by hand.
struct ManualStore {
    retention: Duration,
    keep: u64,
    elapsed: std::sync::atomic::AtomicU64,
    seen: std::sync::Mutex<std::collections::HashMap<(AccountId, num_bigint::BigInt), u64>>,
}

impl ManualStore {
    /// A store that reports `retention` but forgets nonces after `keep` seconds.
    fn new(retention: Duration, keep: u64) -> Arc<Self> {
        Arc::new(Self {
            retention,
            keep,
            elapsed: Default::default(),
            seen: Default::default(),
        })
    }

    fn advance_to(&self, secs: u64) {
        self.elapsed
            .store(secs, std::sync::atomic::Ordering::SeqCst);
    }
}

impl NonceStore for ManualStore {
    fn retention(&self) -> Duration {
        self.retention
    }

    fn check_and_store<'a>(
        &'a self,
        hotkey: &'a AccountId,
        nonce: &'a num_bigint::BigInt,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(async move {
            let now = self.elapsed.load(std::sync::atomic::Ordering::SeqCst);
            let keep = self.keep;
            let mut seen = self.seen.lock().unwrap();
            seen.retain(|_, at| now - *at < keep);
            let key = (*hotkey, nonce.clone());
            if seen.contains_key(&key) {
                return Ok(false);
            }
            seen.insert(key, now);
            Ok(true)
        })
    }
}

/// A store that keeps nonces exactly as long as a verifier requires still
/// remembers every request for as long as it is fresh, even when the wall
/// clock steps back by up to the documented margin.
#[tokio::test]
async fn g13_retention_covers_freshness() {
    let max_age = 300;
    assert_eq!(
        HandshakeVerifier::required_retention(max_age),
        Duration::from_secs(360)
    );
    assert_eq!(
        HandshakeVerifier::recommended_retention(max_age),
        Duration::from_secs(360 + auth::CLOCK_STEP_MARGIN_SECS)
    );
    let admit = |_: &AccountId| true;
    for (retention, step_back) in [
        (HandshakeVerifier::required_retention(max_age), 0),
        (
            HandshakeVerifier::recommended_retention(max_age),
            auth::CLOCK_STEP_MARGIN_SECS,
        ),
    ] {
        // The newest request a verifier accepts: signed 60 s ahead of `TS`.
        let request = signed(TS + 60).await;
        let mut fresh_until = 0;
        for elapsed in [0u64, 1, 60, 299, 300, 359, 360, 400, 419, 420, 480] {
            let store = ManualStore::new(retention, retention.as_secs());
            let v = HandshakeVerifier::new(max_age, store.clone()).unwrap();
            assert!(v.verify(&request, ZERO_FP, TS, admit).await.is_ok());
            store.advance_to(elapsed);
            // The wall clock advanced by `elapsed` but stepped back by `step_back`.
            let now = TS + elapsed - step_back.min(elapsed);
            match v.verify(&request, ZERO_FP, now, admit).await {
                Err(HandshakeReject::Replay) => fresh_until = elapsed,
                Err(HandshakeReject::Stale) => {}
                other => panic!("replay at +{elapsed}s (step back {step_back}): {other:?}"),
            }
        }
        assert!(fresh_until >= 359, "{fresh_until}");
    }
    // The check above detects a store that forgets too early.
    let store = ManualStore::new(HandshakeVerifier::required_retention(max_age), 300);
    let request = signed(TS + 60).await;
    let short = HandshakeVerifier::new(max_age, store.clone()).unwrap();
    assert!(short.verify(&request, ZERO_FP, TS, admit).await.is_ok());
    store.advance_to(300);
    assert!(
        short
            .verify(&request, ZERO_FP, TS + 300, admit)
            .await
            .is_ok()
    );
}

#[test]
fn g14_write_budget() {
    assert_eq!(write_budget(0), Duration::from_secs(5));
    assert_eq!(write_budget(1024 * 1024), Duration::from_secs(6));
    assert_eq!(write_budget(10 * 1024 * 1024), Duration::from_secs(15));
    assert_eq!(write_budget(usize::MAX), Duration::from_secs(60));
}

#[tokio::test]
async fn g16_response_negatives() {
    let req = signed(TS).await;
    let good = sign_handshake_response(&signer(1), &req, ZERO_FP, TS, "c".into())
        .await
        .unwrap();
    let miner = signer(1).account();
    verify_handshake_response(&good, &miner, &req, ZERO_FP).unwrap();

    let mut r = good.clone();
    r.accepted = false;
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::Rejected)
    ));
    let mut r = good.clone();
    r.signature.clear();
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::EmptySignature)
    ));
    let mut r = good.clone();
    r.signature = "not base64!".into();
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::InvalidBase64(_))
    ));
    let mut r = good.clone();
    r.signature = fingerprint_b64(&[1; 32]);
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::InvalidSignatureLength(32))
    ));
    let mut r = good.clone();
    r.miner_hotkey = "not-ss58".into();
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::HotkeyMismatch { .. })
    ));
    // Signed by the wrong key.
    let wrong = sign_handshake_response(&signer(3), &req, ZERO_FP, TS, "c".into())
        .await
        .unwrap();
    assert!(verify_handshake_response(&wrong, &miner, &req, ZERO_FP).is_err());
    let mut forged = wrong.clone();
    forged.miner_hotkey = MINER.into();
    assert!(matches!(
        verify_handshake_response(&forged, &miner, &req, ZERO_FP),
        Err(AuthError::BadSignature)
    ));
    // Tampered nonce.
    let mut other = req.clone();
    other.nonce = "0".repeat(32);
    assert!(matches!(
        verify_handshake_response(&good, &miner, &other, ZERO_FP),
        Err(AuthError::BadSignature)
    ));
    // Fingerprint missing or different.
    let mut r = good.clone();
    r.cert_fingerprint = None;
    assert!(matches!(
        verify_handshake_response(&r, &miner, &req, ZERO_FP),
        Err(AuthError::MissingFingerprint)
    ));
    assert!(matches!(
        verify_handshake_response(
            &good,
            &miner,
            &req,
            "DldRwCblQ7Loqy6wYJnaodHl30d3j3eH+qtFzfEv46g="
        ),
        Err(AuthError::FingerprintMismatch { .. })
    ));
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
enum E {
    A,
    B(u8),
    C { x: u8 },
    D(u8, u8),
}

#[test]
fn g18_typed_enums() {
    assert_eq!(to_value(&E::A).unwrap(), Value::from(0));
    assert_eq!(
        to_value(&E::B(5)).unwrap(),
        Value::Map(vec![(Value::from(1), Value::from(5))])
    );
    assert_eq!(
        to_value(&E::C { x: 1 }).unwrap(),
        Value::Map(vec![(
            Value::from(2),
            Value::Map(vec![(Value::from("x"), Value::from(1))])
        )])
    );
    assert_eq!(
        to_value(&E::D(1, 2)).unwrap(),
        Value::Map(vec![(
            Value::from(3),
            Value::Array(vec![Value::from(1), Value::from(2)])
        )])
    );
    // rtao's policy (A19): accept NamedSerializer shapes, rmpv arrays and names.
    for e in [E::A, E::B(5), E::C { x: 1 }, E::D(1, 2)] {
        assert_eq!(from_value::<E>(to_value(&e).unwrap()).unwrap(), e);
    }
    assert_eq!(
        from_value::<E>(Value::Array(vec![Value::from(0)])).unwrap(),
        E::A
    );
    assert_eq!(
        from_value::<E>(Value::Array(vec![Value::from(1), Value::from(5)])).unwrap(),
        E::B(5)
    );
    assert_eq!(from_value::<E>(Value::from("A")).unwrap(), E::A);
    assert_eq!(
        from_value::<E>(Value::Map(vec![(Value::from("B"), Value::from(9))])).unwrap(),
        E::B(9)
    );
    assert!(from_value::<E>(Value::from(9)).is_err());
}

#[test]
fn typed_struct_round_trip() {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Req {
        name: String,
        count: u32,
        tags: Vec<String>,
        opt: Option<i64>,
        bytes: serde_bytes::ByteBuf,
        kind: E,
        value: Value,
    }
    let req = Req {
        name: "n".into(),
        count: 3,
        tags: vec!["a".into()],
        opt: None,
        bytes: serde_bytes::ByteBuf::from(vec![1, 2]),
        kind: E::C { x: 4 },
        value: Value::from("v"),
    };
    let data = to_data(&req).unwrap();
    assert_eq!(data["name"], Value::from("n"));
    assert_eq!(data["opt"], Value::Nil);
    assert_eq!(data["bytes"], Value::Binary(vec![1, 2]));
    assert_eq!(from_data::<Req>(data).unwrap(), req);
    assert!(to_data(&5u8).is_err());
    // Like btlightning's NamedSerializer, newtype structs (including rmpv's
    // Ext wrapper) are flattened on encode; a received Ext decodes intact.
    assert_eq!(
        to_value(&Value::Ext(5, vec![9])).unwrap(),
        Value::Array(vec![Value::from(5), Value::Binary(vec![9])])
    );
    assert_eq!(
        from_value::<Value>(Value::Ext(5, vec![9])).unwrap(),
        Value::Ext(5, vec![9])
    );
    let keyed = to_data(&std::collections::BTreeMap::from([(1u8, true)])).unwrap();
    assert_eq!(keyed["1"], Value::Boolean(true));
}

#[test]
fn g20_lenient_decoding() {
    let mut g5 = h("93 a4 6563686f 81 a3 6d7367 a2 6869 ce 6553f100");
    g5.extend_from_slice(&[0xc0, 0xc0]);
    let p: SynapsePacket = decode(&g5).unwrap();
    assert_eq!(p.synapse_type, "echo");
    let by_index: SynapsePacket = decode(&h("83 00 a4 6563686f 01 80 02 ce6553f100")).unwrap();
    assert_eq!(by_index.timestamp, TS);
    let end: StreamEnd = decode(&h("81 a7 73756363657373 c3")).unwrap();
    assert_eq!(
        end,
        StreamEnd {
            success: true,
            error: None
        }
    );
    assert!(decode::<StreamEnd>(&h("91 c3")).is_err());
    assert!(decode::<SynapseResponse>(&h("93 c3 80 00")).is_err());
    // A UTF-8 bin is accepted for a String field.
    let bin: SynapsePacket = decode(&h("93 c4 04 6563686f 80 00")).unwrap();
    assert_eq!(bin.synapse_type, "echo");
}
