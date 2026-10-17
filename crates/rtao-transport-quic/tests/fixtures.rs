//! G19: handshake frames captured from btlightning-py (53c090b), replayed
//! with an injected clock. Recapture with `ci/interop/quic/run.sh`.

#![cfg(feature = "server")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use rtao_btauth::{InMemoryNonceStore, NonceStoreConfig};
use rtao_transport_quic::auth::{
    HandshakeReject, HandshakeVerifier, parse_ss58, verify_handshake_response,
};
use rtao_transport_quic::wire::{
    HandshakeRequest, HandshakeResponse, MessageType, decode, parse_frame,
};
use serde_json::Value as Json;

const MINER: &str = "5CcyqxXnJucaCnQQvvUg5EPzj1uoNAxACZvzArHw5aVDvgNH";
const VALIDATOR: &str = "5CfCr47V5Dte6bwxNBE8K9oNnQd9fiay6aDEEkgYtFv7w4Fq";

fn fixture() -> Json {
    serde_json::from_str(include_str!("fixtures/btlightning_vectors.json")).unwrap()
}

fn frame<T: serde::de::DeserializeOwned>(hex_str: &Json, want: MessageType) -> (T, Vec<u8>) {
    let bytes = hex::decode(hex_str.as_str().unwrap()).unwrap();
    let (kind, payload) = parse_frame(&bytes, 1 << 20).unwrap();
    assert_eq!(kind, want);
    (decode(payload).unwrap(), payload.to_vec())
}

fn verifier() -> HandshakeVerifier {
    let store = InMemoryNonceStore::with_config(NonceStoreConfig {
        retention: HandshakeVerifier::required_retention(300),
        ..Default::default()
    })
    .unwrap();
    HandshakeVerifier::new(300, Arc::new(store)).unwrap()
}

#[tokio::test]
async fn python_client_request_verifies() {
    let f = &fixture()["python_client_request"];
    let fp = f["server_fingerprint_b64"].as_str().unwrap();
    let now = f["received_at"].as_u64().unwrap();
    let (req, payload): (HandshakeRequest, _) =
        frame(&f["request_frame_hex"], MessageType::HandshakeRequest);
    // Positional array of four fields, 180 bytes.
    assert_eq!(payload[0], 0x94);
    assert_eq!(payload.len(), 180);
    assert_eq!(req.validator_hotkey, VALIDATOR);
    assert_eq!(req.nonce, req.nonce.to_lowercase());
    let v = verifier();
    let account = v.verify(&req, fp, now, |_| true).await.unwrap();
    assert_eq!(account, parse_ss58(VALIDATOR).unwrap());
    assert_eq!(
        v.verify(&req, fp, now, |_| true).await,
        Err(HandshakeReject::Replay)
    );
    // Bound to the certificate fingerprint and to time.
    assert_eq!(
        verifier()
            .verify(
                &req,
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                now,
                |_| true
            )
            .await,
        Err(HandshakeReject::BadSignature)
    );
    assert_eq!(
        verifier()
            .verify(&req, fp, req.timestamp + 300, |_| true)
            .await,
        Err(HandshakeReject::Stale)
    );
}

#[test]
fn python_server_response_verifies() {
    let f = &fixture()["python_server_response"];
    let fp = f["server_fingerprint_b64"].as_str().unwrap();
    let (req, _): (HandshakeRequest, _) =
        frame(&f["rtao_request_frame_hex"], MessageType::HandshakeRequest);
    let (resp, payload): (HandshakeResponse, _) =
        frame(&f["response_frame_hex"], MessageType::HandshakeResponse);
    assert_eq!(payload[0], 0x96);
    assert_eq!(resp.miner_hotkey, MINER);
    assert!(resp.accepted);
    assert!(
        resp.connection_id
            .starts_with(&format!("conn_{VALIDATOR}_"))
    );
    assert_eq!(resp.cert_fingerprint.as_deref(), Some(fp));
    verify_handshake_response(&resp, &parse_ss58(MINER).unwrap(), &req, fp).unwrap();
    let mut tampered = req.clone();
    tampered.nonce = "0".repeat(32);
    assert!(verify_handshake_response(&resp, &parse_ss58(MINER).unwrap(), &tampered, fp).is_err());
}
