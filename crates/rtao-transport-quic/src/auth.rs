//! Handshake signing and verification.
//!
//! Both signed strings are ASCII and carry no trailing newline:
//!
//! ```text
//! handshake:<validator>:<timestamp>:<nonce>:<fingerprint>
//! handshake_response:<validator>:<miner>:<timestamp>:<nonce>:<fingerprint>
//! ```
//!
//! Signatures are raw sr25519 (context `substrate`, no `<Bytes>` wrapper),
//! sent as standard padded base64. Addresses are signed and verified as the
//! exact strings carried on the wire.

use base64::{Engine, prelude::BASE64_STANDARD};
use blake2::{Blake2b, Digest, digest::consts::U32};
use rtao_core::keys::{CRYPTO_SR25519, DEFAULT_SS58_FORMAT, public_key_from_ss58, verify_strict};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{Signer, SignerError};

use crate::wire::{HandshakeRequest, HandshakeResponse};

/// A handshake that cannot be signed or whose reply does not verify.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The signer is not sr25519.
    #[error("QUIC handshakes require an sr25519 signer, not {0:?}")]
    UnsupportedCrypto(CryptoType),
    /// The signer refused or failed to sign raw bytes.
    #[error("raw signing failed: {0}")]
    Signer(#[from] SignerError),
    /// The operating system random source failed.
    #[error("random source failed: {0}")]
    Random(String),
    /// The miner rejected the handshake.
    #[error("handshake rejected by miner")]
    Rejected,
    /// The reply names a different miner.
    #[error("miner hotkey mismatch: expected {expected}, got {got}")]
    HotkeyMismatch {
        /// Expected SS58 address.
        expected: String,
        /// Received address string.
        got: String,
    },
    /// The reply carries no certificate fingerprint.
    #[error("miner returned no certificate fingerprint")]
    MissingFingerprint,
    /// The reply's fingerprint differs from the TLS certificate's.
    #[error("certificate fingerprint mismatch: expected {expected}, got {got}")]
    FingerprintMismatch {
        /// Fingerprint of the presented certificate.
        expected: String,
        /// Fingerprint in the reply.
        got: String,
    },
    /// The reply signature is empty.
    #[error("miner returned empty signature")]
    EmptySignature,
    /// An address is not valid SS58.
    #[error("invalid SS58 address: {0}")]
    InvalidAddress(String),
    /// A signature is not valid base64.
    #[error("failed to decode signature: {0}")]
    InvalidBase64(String),
    /// A signature is not 64 bytes.
    #[error("invalid signature length: {0}")]
    InvalidSignatureLength(usize),
    /// A signature does not verify.
    #[error("signature verification failed")]
    BadSignature,
}

/// The validator's signed handshake string.
pub fn handshake_request_message(validator: &str, ts: u64, nonce: &str, fp_b64: &str) -> String {
    format!("handshake:{validator}:{ts}:{nonce}:{fp_b64}")
}

/// The miner's signed handshake reply string.
pub fn handshake_response_message(
    validator: &str,
    miner: &str,
    ts: u64,
    nonce: &str,
    fp_b64: &str,
) -> String {
    format!("handshake_response:{validator}:{miner}:{ts}:{nonce}:{fp_b64}")
}

/// BLAKE2b-256 of a certificate's DER bytes.
pub fn cert_fingerprint(cert_der: &[u8]) -> [u8; 32] {
    Blake2b::<U32>::digest(cert_der).into()
}

/// Standard padded base64 of a fingerprint (44 characters).
pub fn fingerprint_b64(fp: &[u8; 32]) -> String {
    BASE64_STANDARD.encode(fp)
}

/// 16 random bytes as 32 lowercase hexadecimal digits.
pub fn new_nonce() -> Result<String, AuthError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| AuthError::Random(e.to_string()))?;
    Ok(format!("{:032x}", u128::from_be_bytes(bytes)))
}

/// Whether a nonce is exactly 32 ASCII hexadecimal digits, in either case.
pub fn is_valid_nonce(nonce: &str) -> bool {
    nonce.len() == 32 && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Decode an SS58 address of any registered prefix.
pub fn parse_ss58(address: &str) -> Result<AccountId, AuthError> {
    public_key_from_ss58(address)
        .map(AccountId)
        .map_err(|e| AuthError::InvalidAddress(format!("{address}: {e}")))
}

/// The SS58 string (prefix 42) a signer's account is sent as.
pub fn signer_ss58(signer: &dyn Signer) -> String {
    signer.account().to_ss58(DEFAULT_SS58_FORMAT)
}

async fn sign_b64(signer: &dyn Signer, message: &str) -> Result<String, AuthError> {
    let crypto = signer.crypto_type();
    if crypto != CryptoType::Sr25519 {
        return Err(AuthError::UnsupportedCrypto(crypto));
    }
    let signature = signer.sign_raw(message.as_bytes()).await?;
    Ok(BASE64_STANDARD.encode(signature))
}

/// Build and sign a validator handshake with a fresh nonce.
pub async fn sign_handshake(
    signer: &dyn Signer,
    fp_b64: &str,
    now: u64,
) -> Result<HandshakeRequest, AuthError> {
    sign_handshake_with_nonce(signer, fp_b64, now, new_nonce()?).await
}

/// Build and sign a validator handshake with a caller-chosen nonce.
pub async fn sign_handshake_with_nonce(
    signer: &dyn Signer,
    fp_b64: &str,
    now: u64,
    nonce: String,
) -> Result<HandshakeRequest, AuthError> {
    let validator_hotkey = signer_ss58(signer);
    let message = handshake_request_message(&validator_hotkey, now, &nonce, fp_b64);
    let signature = sign_b64(signer, &message).await?;
    Ok(HandshakeRequest {
        validator_hotkey,
        timestamp: now,
        nonce,
        signature,
    })
}

/// Build and sign an accepted miner reply to a verified request.
pub async fn sign_handshake_response(
    signer: &dyn Signer,
    request: &HandshakeRequest,
    fp_b64: &str,
    now: u64,
    connection_id: String,
) -> Result<HandshakeResponse, AuthError> {
    let miner_hotkey = signer_ss58(signer);
    let message = handshake_response_message(
        &request.validator_hotkey,
        &miner_hotkey,
        now,
        &request.nonce,
        fp_b64,
    );
    let signature = sign_b64(signer, &message).await?;
    Ok(HandshakeResponse {
        miner_hotkey,
        timestamp: now,
        signature,
        accepted: true,
        connection_id,
        cert_fingerprint: Some(fp_b64.to_owned()),
    })
}

/// Verify a base64 raw sr25519 signature over `message` by `ss58`.
///
/// Malformed inputs are errors; a well-formed signature that does not
/// verify returns `Ok(false)`.
pub fn verify_b64(ss58: &str, sig_b64: &str, message: &str) -> Result<bool, AuthError> {
    parse_ss58(ss58)?;
    let signature = BASE64_STANDARD
        .decode(sig_b64)
        .map_err(|e| AuthError::InvalidBase64(e.to_string()))?;
    if signature.len() != 64 {
        return Err(AuthError::InvalidSignatureLength(signature.len()));
    }
    verify_strict(message.as_bytes(), &signature, ss58, CRYPTO_SR25519)
        .map_err(|e| AuthError::InvalidAddress(e.to_string()))
}

/// Check a miner's reply, in btlightning's order: acceptance, miner
/// identity, certificate fingerprint, then signature.
///
/// The miner address is compared as a decoded account, so any SS58 prefix
/// naming `expected` is accepted; the signature covers the exact string.
pub fn verify_handshake_response(
    response: &HandshakeResponse,
    expected: &AccountId,
    request: &HandshakeRequest,
    fp_b64: &str,
) -> Result<(), AuthError> {
    if !response.accepted {
        return Err(AuthError::Rejected);
    }
    let mismatch = || AuthError::HotkeyMismatch {
        expected: expected.to_ss58(DEFAULT_SS58_FORMAT),
        got: response.miner_hotkey.clone(),
    };
    match parse_ss58(&response.miner_hotkey) {
        Ok(account) if account == *expected => {}
        _ => return Err(mismatch()),
    }
    match response.cert_fingerprint.as_deref() {
        None => return Err(AuthError::MissingFingerprint),
        Some(got) if got != fp_b64 => {
            return Err(AuthError::FingerprintMismatch {
                expected: fp_b64.to_owned(),
                got: got.to_owned(),
            });
        }
        Some(_) => {}
    }
    if response.signature.is_empty() {
        return Err(AuthError::EmptySignature);
    }
    let message = handshake_response_message(
        &request.validator_hotkey,
        &response.miner_hotkey,
        response.timestamp,
        &request.nonce,
        fp_b64,
    );
    if verify_b64(&response.miner_hotkey, &response.signature, &message)? {
        Ok(())
    } else {
        Err(AuthError::BadSignature)
    }
}

#[cfg(feature = "server")]
pub use verifier::{CLOCK_STEP_MARGIN_SECS, HandshakeReject, HandshakeVerifier};

#[cfg(feature = "server")]
mod verifier {
    use std::sync::Arc;
    use std::time::Duration;

    use num_bigint::BigInt;
    use rtao_btauth::{NonceStore, StoreError};
    use rtao_primitives::AccountId;

    use super::{handshake_request_message, is_valid_nonce, parse_ss58, verify_b64};
    use crate::wire::{HandshakeRequest, MAX_FUTURE_SKEW_SECS};

    /// Why a validator handshake was refused. Peers only see `accepted = false`.
    #[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
    pub enum HandshakeReject {
        /// `now - timestamp >= max_age`.
        #[error("handshake timestamp is stale")]
        Stale,
        /// `timestamp > now + 60`.
        #[error("handshake timestamp is too far in the future")]
        Future,
        /// The nonce is not 32 hexadecimal digits.
        #[error("invalid nonce format")]
        BadNonce,
        /// The validator address is not valid SS58.
        #[error("invalid validator address")]
        BadHotkey,
        /// The signature is malformed or does not verify.
        #[error("invalid handshake signature")]
        BadSignature,
        /// The hotkey policy refused the validator.
        #[error("validator not permitted")]
        NotPermitted,
        /// The nonce was already accepted for this validator.
        #[error("nonce replayed")]
        Replay,
        /// The nonce store cannot remember another nonce.
        #[error("nonce store refused: {0}")]
        Store(String),
    }

    /// Backward wall-clock step, in seconds, that
    /// [`HandshakeVerifier::recommended_retention`] tolerates.
    pub const CLOCK_STEP_MARGIN_SECS: u64 = 60;

    /// Validator handshake checks with bounded, fail-closed replay protection.
    ///
    /// Freshness is judged by the wall clock, while nonce stores expire
    /// entries by a monotonic clock. If the wall clock steps back by `d`
    /// seconds after a nonce is stored, its request stays fresh `d` seconds
    /// longer than [`Self::required_retention`] keeps the nonce. The server's
    /// default store therefore uses [`Self::recommended_retention`], which
    /// closes that gap for steps of up to [`CLOCK_STEP_MARGIN_SECS`]. A
    /// larger backward step (for example a clock corrected by minutes) can
    /// reopen a replay window, as in btlightning; keep the clock synchronised.
    pub struct HandshakeVerifier {
        max_age: u64,
        store: Arc<dyn NonceStore>,
    }

    impl HandshakeVerifier {
        /// The minimum nonce retention a store needs for `max_age`: the
        /// signature age plus the accepted future skew.
        pub fn required_retention(max_age: u64) -> Duration {
            Duration::from_secs(max_age.saturating_add(MAX_FUTURE_SKEW_SECS))
        }

        /// [`Self::required_retention`] plus [`CLOCK_STEP_MARGIN_SECS`]: the
        /// retention of the server's default store.
        pub fn recommended_retention(max_age: u64) -> Duration {
            Self::required_retention(max_age)
                .saturating_add(Duration::from_secs(CLOCK_STEP_MARGIN_SECS))
        }

        /// A verifier over `store`, which must retain nonces for at least
        /// [`Self::required_retention`].
        pub fn new(max_age: u64, store: Arc<dyn NonceStore>) -> Result<Self, String> {
            if max_age == 0 {
                return Err("max signature age must be non-zero".into());
            }
            let need = Self::required_retention(max_age);
            if store.retention() < need {
                return Err(format!(
                    "nonce store retention {:?} is shorter than the required {:?}",
                    store.retention(),
                    need
                ));
            }
            Ok(Self { max_age, store })
        }

        /// Verify a request against the server fingerprint at `now`.
        ///
        /// Checks run in btlightning's order (age, future skew, nonce format,
        /// signature); `admit` runs after the signature and before the nonce
        /// is stored, so refused validators consume no replay capacity.
        pub async fn verify(
            &self,
            request: &HandshakeRequest,
            fp_b64: &str,
            now: u64,
            admit: impl FnOnce(&AccountId) -> bool,
        ) -> Result<AccountId, HandshakeReject> {
            let ts = request.timestamp;
            if now > ts && now - ts >= self.max_age {
                return Err(HandshakeReject::Stale);
            }
            if ts > now.saturating_add(MAX_FUTURE_SKEW_SECS) {
                return Err(HandshakeReject::Future);
            }
            if !is_valid_nonce(&request.nonce) {
                return Err(HandshakeReject::BadNonce);
            }
            let account =
                parse_ss58(&request.validator_hotkey).map_err(|_| HandshakeReject::BadHotkey)?;
            let message =
                handshake_request_message(&request.validator_hotkey, ts, &request.nonce, fp_b64);
            match verify_b64(&request.validator_hotkey, &request.signature, &message) {
                Ok(true) => {}
                _ => return Err(HandshakeReject::BadSignature),
            }
            if !admit(&account) {
                return Err(HandshakeReject::NotPermitted);
            }
            let nonce =
                u128::from_str_radix(&request.nonce, 16).map_err(|_| HandshakeReject::BadNonce)?;
            match self
                .store
                .check_and_store(&account, &BigInt::from(nonce))
                .await
            {
                Ok(true) => Ok(account),
                Ok(false) => Err(HandshakeReject::Replay),
                Err(e @ (StoreError::Full | StoreError::Unavailable(_))) => {
                    Err(HandshakeReject::Store(e.to_string()))
                }
            }
        }
    }
}
