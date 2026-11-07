//! Runtime 487/488 account inputs whose behavior is not ported yet (D-128).
//!
//! Upstream added a hashed (post-quantum) destination descriptor to transfers
//! and coldkey swaps, and a `safe` account derivation to multisig and pure
//! proxy intents. The generated inputs carry the fields; these behaviors still
//! build the classical calls, so they refuse any value but the classical
//! default instead of silently deriving a different account.

use rtao_tx::IntentError;

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
    use super::*;

    #[test]
    fn classical_defaults_pass_and_new_account_inputs_refuse() {
        assert!(no_hashed_destination("transfer", None).is_ok());
        assert!(no_hashed_destination("transfer", Some(&"0x02".to_owned())).is_err());
        assert!(legacy_account_mode("multisig_approve", None).is_ok());
        assert!(legacy_account_mode("multisig_approve", Some("legacy")).is_ok());
        assert!(legacy_account_mode("multisig_approve", Some("safe")).is_err());
    }
}
