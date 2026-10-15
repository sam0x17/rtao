// Derived from RaoFoundation/subtensor docs/guides/signed-requests.mdx @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
