// Derived from RaoFoundation/subtensor docs/guides/signed-requests.mdx @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from opentensor/btwallet src/python_bindings.rs and src/keypair.rs @ v4.1.0 (7347c85b7164296f1a05e2134aa1cc936d9dfdc7) (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/core/axon.py @ v10.5.0 (MIT); modified by rtao.
//! Verify v10 signatures when the application supplies its schema body hash.
//!
//! This bridge follows the documented Python recipe. The application must
//! enforce receiver identity, nonce freshness and replay protection itself.

use http::HeaderMap;
use rtao_primitives::{AccountId, CryptoType};

use crate::{
    AuthError, MAX_SENDER_SS58_BYTES, check_auth_header_budget, decode_signature, text_header,
};

/// Verify the sr25519 v10 dotted message with the schema's computed body hash.
///
/// Unlike btauth/1, this signature does not bind the HTTP method or target.
/// Authentication fields are limited to 4,096 bytes each and 8,192 bytes in
/// total, including the supplied body hash. Oversized fields are rejected
/// before parsing, payload allocation or native address decoding.
pub fn verify_v10(headers: &HeaderMap, body_hash: &str) -> Result<AccountId, AuthError> {
    fn required<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, AuthError> {
        text_header(headers, name)?
            .ok_or_else(|| AuthError::MalformedAuth(format!("missing {name}")))
    }
    check_auth_header_budget(
        headers,
        &[
            "bt_header_dendrite_hotkey",
            "bt_header_dendrite_nonce",
            "bt_header_axon_hotkey",
            "bt_header_dendrite_uuid",
            "bt_header_dendrite_signature",
        ],
        body_hash.len(),
    )?;
    let sender = required(headers, "bt_header_dendrite_hotkey")?;
    let nonce = required(headers, "bt_header_dendrite_nonce")?;
    let receiver = required(headers, "bt_header_axon_hotkey")?;
    let uuid = required(headers, "bt_header_dendrite_uuid")?;
    let signature = decode_signature(required(headers, "bt_header_dendrite_signature")?)
        .ok_or_else(|| {
            AuthError::MalformedAuth("bt_header_dendrite_signature is not hex".into())
        })?;
    if sender.len() > MAX_SENDER_SS58_BYTES {
        return Err(AuthError::MalformedAuth(
            "invalid hotkey address or signature encoding".into(),
        ));
    }
    let message = format!("{nonce}.{sender}.{receiver}.{uuid}.{body_hash}");
    let valid = rtao_core::keys::verify(
        message.as_bytes(),
        &signature,
        sender,
        u8::from(CryptoType::Sr25519),
    )
    .map_err(|_| AuthError::MalformedAuth("invalid hotkey address or signature encoding".into()))?;
    if !valid {
        return Err(AuthError::BadSignature("bad v10 signature".into()));
    }
    sender.parse().map_err(|_| {
        AuthError::MalformedAuth("invalid hotkey address or signature encoding".into())
    })
}

/// Longest accepted v10 signature, uuid or body-hash text, in bytes.
const MAX_V10_FIELD_BYTES: usize = 4_096;

/// Decode a v10 signature with bittensor-wallet 4.1's exact text rules.
///
/// The text must start with `0x`; every leading `0x` is then removed, the rest
/// is strict hexadecimal in either case, and the result must be 64 bytes.
/// Errors carry Python's exception text, for example
/// `Invalid signature format. Expected hex string.`.
pub fn decode_v10_signature(text: &str) -> Result<[u8; 64], String> {
    if !text.starts_with("0x") {
        return Err("Invalid signature format. Expected hex string.".into());
    }
    if text.len() > MAX_V10_FIELD_BYTES {
        return Err("Invalid SR25519 signature.".into());
    }
    let bytes = hex::decode(text.trim_start_matches("0x"))
        .map_err(|error| format!("Invalid hex string: {error:?}"))?;
    bytes
        .try_into()
        .map_err(|_| "Invalid SR25519 signature.".into())
}

/// Verify a v10 dendrite signature exactly as Python's `default_verify` does.
///
/// The signed message is
/// `"{nonce}.{sender}.{me}.{uuid}.{body_hash}"`, where `nonce` is the parsed
/// integer in canonical decimal, `me` is this axon's own hotkey with SS58
/// prefix 42 (not the request's `bt_header_axon_hotkey`), and an absent uuid
/// or body hash renders as `None`. Nonce freshness and replay protection are
/// the caller's responsibility.
///
/// Errors use Python's messages: a malformed signature yields
/// [`AuthError::MalformedAuth`] with bittensor-wallet's text, and a valid
/// signature over a different message yields [`AuthError::BadSignature`] with
/// `Signature mismatch with {message} and {signature}`. The sender is decoded
/// with the pinned core, which, unlike bittensor-wallet, refuses SS58
/// prefixes outside the registry.
pub fn verify_v10_for(
    sender: &str,
    nonce: &num_bigint::BigInt,
    uuid: Option<&str>,
    body_hash: Option<&str>,
    signature: &str,
    me: &AccountId,
) -> Result<AccountId, AuthError> {
    if sender.len() > MAX_SENDER_SS58_BYTES {
        return Err(AuthError::MalformedAuth(
            "Invalid SS58 address: Length is bad".into(),
        ));
    }
    if uuid.is_some_and(|uuid| uuid.len() > MAX_V10_FIELD_BYTES)
        || body_hash.is_some_and(|hash| hash.len() > MAX_V10_FIELD_BYTES)
    {
        return Err(AuthError::MalformedAuth(
            "v10 uuid or body hash exceeds 4096 bytes".into(),
        ));
    }
    let account: AccountId = sender.parse().map_err(|error: rtao_core::CoreError| {
        let text = error.to_string();
        let detail = text
            .split_once("invalid ss58 address: ")
            .map_or(text.as_str(), |(_, detail)| detail);
        AuthError::MalformedAuth(format!("Invalid SS58 address: {detail}"))
    })?;
    let decoded = decode_v10_signature(signature).map_err(AuthError::MalformedAuth)?;
    let message = format!(
        "{nonce}.{sender}.{me}.{}.{}",
        uuid.unwrap_or("None"),
        body_hash.unwrap_or("None"),
    );
    let valid = rtao_core::keys::verify(
        message.as_bytes(),
        &decoded,
        sender,
        u8::from(CryptoType::Sr25519),
    )
    .map_err(|_| AuthError::MalformedAuth("Invalid SR25519 signature.".into()))?;
    if !valid {
        return Err(AuthError::BadSignature(format!(
            "Signature mismatch with {message} and {signature}"
        )));
    }
    Ok(account)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use num_bigint::BigInt;
    use rtao_signer::{KeypairSigner, Signer};

    use super::*;

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    const UUID: &str = "5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a";
    const H1: &str = "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a";

    fn signed(message: &str) -> String {
        let alice = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap();
        let signature = futures::executor::block_on(alice.sign_raw(message.as_bytes())).unwrap();
        format!("0x{}", hex::encode(signature))
    }

    #[test]
    fn signature_text_follows_bittensor_wallet() {
        let zeros = "00".repeat(64);
        assert_eq!(
            decode_v10_signature(&zeros).unwrap_err(),
            "Invalid signature format. Expected hex string."
        );
        assert_eq!(
            decode_v10_signature(&format!("0x{}", "00".repeat(63))).unwrap_err(),
            "Invalid SR25519 signature."
        );
        assert_eq!(
            decode_v10_signature("0xzz").unwrap_err(),
            "Invalid hex string: InvalidHexCharacter { c: 'z', index: 0 }"
        );
        assert_eq!(
            decode_v10_signature("0x0").unwrap_err(),
            "Invalid hex string: OddLength"
        );
        assert_eq!(
            decode_v10_signature(&format!("0x0x{zeros}")).unwrap(),
            [0; 64]
        );
        assert_eq!(
            decode_v10_signature(&format!("0x{}", "AB".repeat(64))).unwrap(),
            [0xab; 64]
        );
    }

    #[test]
    fn verifies_python_message_with_own_hotkey() {
        let me: AccountId = BOB.parse().unwrap();
        let nonce = BigInt::from(1_700_000_000_000_000_000_u64);
        let message = format!("1700000000000000000.{ALICE}.{BOB}.{UUID}.{H1}");
        let signature = signed(&message);
        let caller = verify_v10_for(ALICE, &nonce, Some(UUID), Some(H1), &signature, &me).unwrap();
        assert_eq!(caller.to_string(), ALICE);
        let repeated = format!("0x{signature}");
        verify_v10_for(ALICE, &nonce, Some(UUID), Some(H1), &repeated, &me).unwrap();

        let other: AccountId = ALICE.parse().unwrap();
        let error =
            verify_v10_for(ALICE, &nonce, Some(UUID), Some(H1), &signature, &other).unwrap_err();
        assert_eq!(
            error,
            AuthError::BadSignature(format!(
                "Signature mismatch with 1700000000000000000.{ALICE}.{ALICE}.{UUID}.{H1} and {signature}"
            ))
        );
    }

    #[test]
    fn absent_uuid_and_hash_render_as_none() {
        let me: AccountId = BOB.parse().unwrap();
        let nonce = BigInt::from(7);
        let signature = signed(&format!("7.{ALICE}.{BOB}.None.None"));
        verify_v10_for(ALICE, &nonce, None, None, &signature, &me).unwrap();
        let error = verify_v10_for("5Grw", &nonce, None, None, &signature, &me).unwrap_err();
        assert!(
            matches!(error, AuthError::MalformedAuth(text) if text.starts_with("Invalid SS58 address: "))
        );
    }
}
