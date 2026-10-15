use std::fmt;

use crate::NetUid;

/// Failure to represent an identifier, amount, or currency safely.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnitError {
    /// The value exceeds its checked identifier range.
    InvalidIdentifier {
        /// Identifier name.
        kind: &'static str,
        /// Rejected value.
        value: u64,
        /// Exclusive upper bound.
        limit: u64,
    },
    /// An amount carries a different currency than the operation expects.
    UnitMismatch {
        /// Required currency, with zero denoting TAO.
        expected: NetUid,
        /// Currency supplied by the caller.
        actual: NetUid,
    },
    /// An input is not a finite non-negative decimal amount.
    InvalidDecimal(String),
    /// A domain amount cannot represent a negative number of base units.
    Negative,
    /// A floating-point amount is NaN or infinite.
    NonFinite,
    /// A decimal input contains more than nine fractional digits.
    Precision,
    /// An amount or computation exceeds its requested numeric representation.
    Overflow,
    /// Subtraction would produce a negative amount.
    Underflow,
    /// The operation does not support draining all available funds.
    AllNotAllowed,
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { kind, value, limit } => {
                write!(f, "{kind} {value} must be less than {limit}")
            }
            Self::UnitMismatch { expected, actual } => {
                write!(f, "expected netuid {expected}, got netuid {actual}")
            }
            Self::InvalidDecimal(value) => {
                write!(f, "invalid non-negative decimal amount: {value:?}")
            }
            Self::Negative => f.write_str("amount must be non-negative"),
            Self::NonFinite => f.write_str("amount must be finite"),
            Self::Precision => f.write_str("amount requires more than nine fractional digits"),
            Self::Overflow => f.write_str("amount exceeds the requested numeric range"),
            Self::Underflow => f.write_str("amount would be negative"),
            Self::AllNotAllowed => f.write_str("amount does not accept 'all'"),
        }
    }
}

impl std::error::Error for UnitError {}
