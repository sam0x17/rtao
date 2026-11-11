// Derived from RaoFoundation/subtensor sdk/python/bittensor/account_mode.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Account derivation modes and hashed destination descriptors.
//!
//! Upstream added a hashed (post-quantum) destination descriptor to transfers
//! and coldkey swaps, and a `safe` account derivation to multisig and pure
//! proxy intents. Both safe derivations are ported: the chain derives a safe
//! pure proxy, and the core's `safe_multisig_account_id` a safe multisig
//! ([`rtao_primitives::multisig_account_id`]). A hashed destination
//! descriptor is registered or checked by the executor's guard
//! (`rtao_tx::hashed`, D-148).

use rtao_tx::IntentError;

#[cfg_attr(
    not(any(feature = "proxy", feature = "multisig")),
    allow(unused_imports)
)]
pub(crate) use rtao_primitives::AccountMode;

/// Python's `validate_account_mode`: exactly `legacy` or `safe`.
#[doc(alias = "bittensor.account_mode.validate_account_mode")]
#[cfg_attr(not(any(feature = "proxy", feature = "multisig")), allow(dead_code))]
pub(crate) fn validate_account_mode(mode: &str) -> Result<AccountMode, IntentError> {
    mode.parse()
        .map_err(|error: rtao_primitives::AccountModeError| {
            IntentError::validation(error.to_string())
        })
}

/// Check a hashed destination descriptor (hex) with the core's strict
/// decoder; the executor's registration guard uses it (D-148).
#[cfg_attr(not(any(feature = "transfer", feature = "coldkey")), allow(dead_code))]
pub(crate) fn check_hashed_descriptor(descriptor: Option<&String>) -> Result<(), IntentError> {
    if let Some(descriptor) = descriptor {
        let bytes = rtao_primitives::pyfmt::bytes_fromhex(
            descriptor.strip_prefix("0x").unwrap_or(descriptor),
        )
        .map_err(IntentError::validation)?;
        rtao_primitives::receiving::descriptor_account(&bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn hashed_descriptors_are_decoded_strictly() {
        assert!(check_hashed_descriptor(None).is_ok());
        assert!(check_hashed_descriptor(Some(&"0x02".to_owned())).is_err());
        assert!(check_hashed_descriptor(Some(&"zz".to_owned())).is_err());
    }

    #[test]
    fn only_the_two_python_modes_validate() {
        assert_eq!(
            validate_account_mode("legacy").ok(),
            Some(AccountMode::Legacy)
        );
        assert_eq!(validate_account_mode("safe").ok(), Some(AccountMode::Safe));
        for mode in ["", "Safe", "LEGACY", "safe "] {
            assert_eq!(
                validate_account_mode(mode).unwrap_err().to_string(),
                "account_mode must be 'legacy' or 'safe'"
            );
        }
    }
}
