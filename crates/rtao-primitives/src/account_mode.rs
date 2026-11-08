// Derived from RaoFoundation/subtensor sdk/python/bittensor/account_mode.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/codec.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Explicit composite-account derivation modes (Python `bittensor.account_mode`).
//!
//! Existing multisig and pure-proxy accounts keep the `legacy` derivation. The
//! opt-in `safe` derivation (runtime 488) yields an account that cannot be a
//! raw Ed25519 or Sr25519 public key; the chain's `*_safe` calls dispatch from
//! it. The two modes derive different accounts for the same members, so a
//! group's mode is part of its identity and must match every approval.

use crate::AccountId;
use std::{fmt, str::FromStr};

/// How a composite (multisig or pure proxy) account is derived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AccountMode {
    /// The original derivation, unchanged for existing accounts.
    #[default]
    Legacy,
    /// The opt-in derivation that cannot collide with a raw classical key.
    Safe,
}

impl AccountMode {
    /// Python's spelling: `legacy` or `safe`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Safe => "safe",
        }
    }

    /// The suffix the runtime gives this mode's Multisig and Proxy calls.
    pub const fn call_suffix(self) -> &'static str {
        match self {
            Self::Legacy => "",
            Self::Safe => "_safe",
        }
    }
}

/// An `account_mode` other than `legacy` or `safe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountModeError;

impl fmt::Display for AccountModeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("account_mode must be 'legacy' or 'safe'")
    }
}

impl std::error::Error for AccountModeError {}

impl FromStr for AccountMode {
    type Err = AccountModeError;
    /// Python's `validate_account_mode`: exactly `legacy` or `safe`.
    #[doc(alias = "bittensor.account_mode.validate_account_mode")]
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "safe" => Ok(Self::Safe),
            _ => Err(AccountModeError),
        }
    }
}

impl fmt::Display for AccountMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl serde::Serialize for AccountMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<'de> serde::Deserialize<'de> for AccountMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <String as serde::Deserialize>::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Derive a multisig account and its canonical (sorted) signatories.
///
/// `Legacy` is the core's original `blake2_256("modlpy/utilisuba" ...)`
/// derivation; `Safe` is the core's `safe_multisig_account_id`, which also
/// requires 2 to the safe maximum of distinct members and a threshold within
/// the set. Neither checks the runtime's `MaxSignatories`; callers that compose
/// calls do that against live metadata.
#[doc(alias = "bittensor._transport.codec.multisig_account")]
pub fn multisig_account_id(
    signatories: &[AccountId],
    threshold: u16,
    mode: AccountMode,
) -> Result<(AccountId, Vec<AccountId>), rtao_core::CoreError> {
    let keys = signatories
        .iter()
        .map(|account| account.0)
        .collect::<Vec<_>>();
    let (account, sorted) = match mode {
        AccountMode::Legacy => rtao_core::extrinsic::multisig_account_id(&keys, threshold),
        AccountMode::Safe => rtao_core::extrinsic::safe_multisig_account_id(&keys, threshold),
    }
    .map_err(rtao_core::CoreError::from)?;
    Ok((
        AccountId(account),
        sorted.into_iter().map(AccountId).collect(),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn only_the_two_python_modes_parse() {
        assert_eq!("legacy".parse(), Ok(AccountMode::Legacy));
        assert_eq!("safe".parse(), Ok(AccountMode::Safe));
        for mode in ["", "Safe", "LEGACY", "safe "] {
            assert_eq!(
                mode.parse::<AccountMode>().unwrap_err().to_string(),
                "account_mode must be 'legacy' or 'safe'"
            );
        }
        assert_eq!(AccountMode::default(), AccountMode::Legacy);
        assert_eq!(AccountMode::Safe.call_suffix(), "_safe");
    }

    #[test]
    fn safe_derivation_is_order_independent_and_differs_from_legacy() {
        let members = [AccountId([2; 32]), AccountId([1; 32]), AccountId([3; 32])];
        let (safe, sorted) = multisig_account_id(&members, 2, AccountMode::Safe).unwrap();
        assert_eq!(
            sorted,
            vec![AccountId([1; 32]), AccountId([2; 32]), AccountId([3; 32])]
        );
        assert_eq!(
            multisig_account_id(&sorted, 2, AccountMode::Safe)
                .unwrap()
                .0,
            safe
        );
        let (legacy, _) = multisig_account_id(&members, 2, AccountMode::Legacy).unwrap();
        assert_ne!(safe, legacy);
        // The legacy account is the original derivation, unchanged.
        let keys = members.map(|account| account.0);
        assert_eq!(
            legacy.0,
            rtao_core::extrinsic::multisig_account_id(&keys, 2)
                .unwrap()
                .0
        );
        // Safe derivation refuses what the chain refuses.
        assert!(multisig_account_id(&members[..1], 1, AccountMode::Safe).is_err());
        assert!(multisig_account_id(&members, 0, AccountMode::Safe).is_err());
        assert!(multisig_account_id(&members, 4, AccountMode::Safe).is_err());
        assert!(multisig_account_id(&[members[0], members[0]], 1, AccountMode::Safe).is_err());
    }
}
