// Derived from RaoFoundation/subtensor sdk/python/bittensor/account_mode.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Account derivation modes and runtime 487/488 account inputs not ported yet.
//!
//! Upstream added a hashed (post-quantum) destination descriptor to transfers
//! and coldkey swaps, and a `safe` account derivation to multisig and pure
//! proxy intents. A pure proxy's safe mode is ported (the chain derives the
//! account). The multisig intents still derive only the legacy account, which
//! needs the core's safe derivation (the core bump), so they refuse any mode
//! but `legacy` instead of silently deriving a different account; hashed
//! destinations refuse any descriptor.

use rtao_tx::IntentError;

/// Explicit account derivation mode; existing account identities stay legacy.
#[cfg_attr(not(feature = "proxy"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountMode {
    /// The original derivation, unchanged for existing accounts.
    Legacy,
    /// The opt-in derivation that cannot collide with a raw Ed25519/Sr25519 key.
    Safe,
}

/// Python's `validate_account_mode`: exactly `legacy` or `safe`.
#[doc(alias = "bittensor.account_mode.validate_account_mode")]
#[cfg_attr(not(feature = "proxy"), allow(dead_code))]
pub(crate) fn validate_account_mode(mode: &str) -> Result<AccountMode, IntentError> {
    match mode {
        "legacy" => Ok(AccountMode::Legacy),
        "safe" => Ok(AccountMode::Safe),
        _ => Err(IntentError::validation(
            "account_mode must be 'legacy' or 'safe'",
        )),
    }
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

/// Refuse any account derivation mode but `legacy`.
#[cfg_attr(not(feature = "multisig"), allow(dead_code))]
pub(crate) fn legacy_account_mode(op: &'static str, mode: Option<&str>) -> Result<(), IntentError> {
    match mode {
        None | Some("legacy") => Ok(()),
        Some(mode) => Err(IntentError::InvalidArgument(format!(
            "{op}: account_mode {mode:?} is not ported yet; only \"legacy\" derivation is supported"
        ))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn classical_defaults_pass_and_new_account_inputs_refuse() {
        assert!(no_hashed_destination("transfer", None).is_ok());
        assert!(no_hashed_destination("transfer", Some(&"0x02".to_owned())).is_err());
        assert!(legacy_account_mode("multisig_approve", None).is_ok());
        assert!(legacy_account_mode("multisig_approve", Some("legacy")).is_ok());
        assert!(legacy_account_mode("multisig_approve", Some("safe")).is_err());
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
