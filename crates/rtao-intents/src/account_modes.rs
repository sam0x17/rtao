// Derived from RaoFoundation/subtensor sdk/python/bittensor/account_mode.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Account derivation modes and the runtime 487/488 account inputs not ported yet.
//!
//! Upstream added a hashed (post-quantum) destination descriptor to transfers
//! and coldkey swaps, and a `safe` account derivation to multisig and pure
//! proxy intents. Both safe derivations are ported: the chain derives a safe
//! pure proxy, and the core's `safe_multisig_account_id` a safe multisig
//! ([`rtao_primitives::multisig_account_id`]). Hashed destinations still
//! refuse any descriptor.

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

/// Refuse a hashed destination descriptor.
pub(crate) fn no_hashed_destination(
    op: &'static str,
    descriptor: Option<&String>,
) -> Result<(), IntentError> {
    match descriptor {
        None => Ok(()),
        Some(_) => Err(IntentError::InvalidArgument(format!(
            "{op}: hashed_descriptor (hashed account destinations, runtime 487) is not ported yet"
        ))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn classical_defaults_pass_and_hashed_destinations_refuse() {
        assert!(no_hashed_destination("transfer", None).is_ok());
        assert!(no_hashed_destination("transfer", Some(&"0x02".to_owned())).is_err());
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
