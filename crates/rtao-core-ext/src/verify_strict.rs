// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/keys/mod.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/polkadot-sdk substrate/primitives/core/src/sr25519.rs and ed25519.rs @ cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a (Apache-2.0); modified by rtao.
//! UPSTREAM-TARGET: keys::verify_strict (PR-U10)
//! UPSTREAM-PROBE: symbol keys::verify_strict
//! UPSTREAM-FEATURES: none
//!
//! Verify exactly the supplied bytes using Substrate's signing rules.
//!
//! Adapted from `sdk/bittensor-core/src/keys/mod.rs` at subtensor v478
//! (`850f2755bb05a3851f948489f0ab27b0645cebce`), Apache-2.0, and the
//! `Pair::verify` implementations in the RaoFoundation/polkadot-sdk fork at
//! `cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a`, Apache-2.0. This extracts
//! the core's private verifier without its public `<Bytes>` retry.

use bittensor_core::{CoreError, keys};

/// Verify a raw signature, with no `<Bytes>` wrapper fallback.
///
/// Invalid SS58 addresses, unsupported schemes and wrong signature lengths
/// are errors. Well-sized signatures that do not verify return `Ok(false)`.
/// Sr25519 uses the same `b"substrate"` context as `sp-core`; ed25519 follows
/// the fork's ZIP-215 verifier, including its public-key acceptance rules.
pub fn verify_strict(
    message: &[u8],
    signature: &[u8],
    ss58_address: &str,
    crypto_type: u8,
) -> Result<bool, CoreError> {
    let public_key = keys::public_key_from_ss58(ss58_address)?;
    match crypto_type {
        keys::CRYPTO_SR25519 => {
            let _: &[u8; 64] = signature
                .try_into()
                .map_err(|_| CoreError::Crypto("invalid sr25519 signature length".into()))?;
            let Ok(signature) = schnorrkel::Signature::from_bytes(signature) else {
                return Ok(false);
            };
            let Ok(public_key) = schnorrkel::PublicKey::from_bytes(&public_key) else {
                return Ok(false);
            };
            Ok(public_key
                .verify_simple(b"substrate", message, &signature)
                .is_ok())
        }
        keys::CRYPTO_ED25519 => {
            let _: &[u8; 64] = signature
                .try_into()
                .map_err(|_| CoreError::Crypto("invalid ed25519 signature length".into()))?;
            let Ok(public_key) = ed25519_zebra::VerificationKey::try_from(public_key) else {
                return Ok(false);
            };
            let Ok(signature) = ed25519_zebra::Signature::try_from(signature) else {
                return Ok(false);
            };
            Ok(public_key.verify(&signature, message).is_ok())
        }
        other => Err(CoreError::Crypto(format!("unknown crypto type {other}"))),
    }
}
