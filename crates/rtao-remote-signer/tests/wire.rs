//! W1-W4 and V3: wire schema strictness and the btauth payload of a `/sign` request.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use http::Method;
use rtao_btauth::server::Json;
use rtao_primitives::CryptoType;
use rtao_remote_signer::wire::{
    ErrorCode, Hex32, HexBytes, PROTOCOL, SignRequest, SignResponse, Ss58, WireCrypto, from_hex,
};
use rtao_signer::{Signer, SignerError};
use serde_json::{Value as Json_, json};

fn fixture() -> Json_ {
    serde_json::from_str(include_str!("fixtures/v1_sign_request.json")).unwrap()
}

fn parse(value: &Json_) -> Result<SignRequest, String> {
    Json::<SignRequest>::from_bytes(&serde_json::to_vec(value).unwrap())
        .map(|json| json.0)
        .map_err(|error| error.to_string())
}

#[test]
fn w1_v1_request_matches_the_pinned_value_and_round_trips() {
    let body = v1().body();
    let value = serde_json::to_value(&body).unwrap();
    // Compared as parsed values: payload_json key order depends on feature unification.
    assert_eq!(value, fixture());
    assert_eq!(parse(&value).unwrap(), body);
    assert_eq!(body.version, PROTOCOL);
    assert_eq!(body.payload.0.len(), 115);
}

#[test]
fn w2_strict_hex_rejects_uppercase_missing_prefix_odd_length_and_wrong_width() {
    assert_eq!(from_hex("0x").unwrap(), Vec::<u8>::new());
    for bad in ["0xAB", "ab", "0xabc", "0X00", "0x0g"] {
        assert!(from_hex(bad).is_err(), "{bad}");
        let mut value = fixture();
        value["call"] = json!(bad);
        assert!(parse(&value).is_err(), "{bad}");
    }
    for width in [31, 33] {
        let mut value = fixture();
        value["chain"]["genesis_hash"] = json!(format!("0x{}", "00".repeat(width)));
        assert!(parse(&value).is_err(), "{width}");
    }
    let empty: HexBytes = serde_json::from_value(json!("0x")).unwrap();
    assert!(empty.0.is_empty());
    let hash: Hex32 = serde_json::from_value(json!(format!("0x{}", "ab".repeat(32)))).unwrap();
    assert_eq!(hash.0, [0xab; 32]);
}

#[test]
fn w3_unknown_fields_versions_and_spellings_are_rejected_but_duplicates_are_last_wins() {
    let mut value = fixture();
    value["extra"] = json!(1);
    assert!(parse(&value).is_err());
    let mut value = fixture();
    value["chain"]["extra"] = json!(1);
    assert!(parse(&value).is_err());
    let mut value = fixture();
    value["purpose"] = json!("Extrinsic");
    assert!(parse(&value).is_err());
    let mut value = fixture();
    value["crypto"] = json!("SR25519");
    assert!(parse(&value).is_err());
    // The schema accepts any version string; the server refuses it (S-tests).
    let mut value = fixture();
    value["version"] = json!("rtao-remote-signer/2");
    assert_eq!(parse(&value).unwrap().version, "rtao-remote-signer/2");
    // A duplicate top-level key is accepted, last wins (parsed through a Value).
    let text = serde_json::to_string(&fixture()).unwrap();
    let duplicated = text.replacen("{", "{\"purpose\":\"shield_inner\",", 1);
    let parsed = Json::<SignRequest>::from_bytes(duplicated.as_bytes())
        .unwrap()
        .0;
    let last = serde_json::from_str::<Json_>(&text).unwrap()["purpose"].clone();
    assert_eq!(serde_json::to_value(parsed.purpose).unwrap(), last);
    // Accounts at another SS58 prefix decode to the same bytes.
    let mut value = fixture();
    value["account"] = json!(alice().account().to_ss58(0));
    assert_eq!(parse(&value).unwrap().account, Ss58(alice().account()));
    assert_eq!(
        serde_json::to_value(WireCrypto::Ed25519).unwrap(),
        json!("ed25519")
    );
}

#[test]
fn w4_sign_response_rejects_short_signatures_and_unknown_variants() {
    let response = |signature: Vec<u8>| SignResponse {
        version: PROTOCOL.into(),
        account: Ss58(alice().account()),
        signature: signature.into(),
    };
    assert!(matches!(
        response(vec![1; 64]).signature(CryptoType::Sr25519),
        Err(SignerError::InvalidSignature(_))
    ));
    let mut unknown = vec![2];
    unknown.extend([0; 64]);
    assert!(response(unknown).signature(CryptoType::Sr25519).is_err());
    let mut ed = vec![0];
    ed.extend([0; 64]);
    assert!(response(ed).signature(CryptoType::Sr25519).is_err());
    let mut sr = vec![1];
    sr.extend([7; 64]);
    assert_eq!(
        response(sr).signature(CryptoType::Sr25519).unwrap().bytes,
        [7; 64]
    );
    let mut value = serde_json::to_value(response(vec![0; 65])).unwrap();
    value["extra"] = json!(true);
    assert!(Json::<SignResponse>::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn error_codes_round_trip_with_their_statuses() {
    for (code, status) in [
        (ErrorCode::MalformedRequest, 400),
        (ErrorCode::UndecodableCall, 400),
        (ErrorCode::AccountNotAuthorized, 403),
        (ErrorCode::PurposeNotAllowed, 403),
        (ErrorCode::RuntimeMismatch, 409),
        (ErrorCode::TamperedRequest, 422),
        (ErrorCode::EraRejected, 422),
        (ErrorCode::PolicyDenied, 403),
        (ErrorCode::ChainUnavailable, 503),
        (ErrorCode::SigningFailed, 500),
    ] {
        assert_eq!(code.status(), status);
        assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
    }
}

#[tokio::test]
async fn v3_btauth_payload_text_for_a_sign_request_and_sign_verify() {
    let body = v1().body_bytes();
    let charlie = charlie();
    let nonce = NOW;
    let payload = rtao_btauth::build_payload(
        CryptoType::Sr25519,
        &Method::POST,
        "/sign",
        &body,
        nonce,
        &charlie.account(),
        Some(&alice().account()),
    );
    let expected = format!(
        "btauth/1\nsr25519\nPOST\n/sign\n{}\n1752076800000000000\n5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y\n5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&body))
    );
    assert_eq!(String::from_utf8(payload.clone()).unwrap(), expected);
    let headers = rtao_btauth::sign(
        &charlie,
        &Method::POST,
        "/sign",
        &body,
        Some(&alice().account()),
        &FixedClock(nonce),
    )
    .await
    .unwrap();
    let signature = headers["X-Bittensor-Signature"].to_str().unwrap();
    assert!(verify(
        &payload,
        &from_hex(signature).unwrap(),
        charlie.account()
    ));
    assert!(headers.get("X-Bittensor-Crypto").is_none());
    assert_eq!(
        headers["X-Bittensor-Receiver"].to_str().unwrap(),
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    );
}
