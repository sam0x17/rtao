// Derived from RaoFoundation/subtensor sdk/python/bittensor/hyperparams.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hyperparameter semantics, native codec conversion and normalized proportions.

#[path = "hyperparam_table.rs"]
mod table;
pub use table::{Hyperparam, HyperparamError, HyperparamStorage, HyperparamTable, RawInteger};

use std::{borrow::Cow, fmt};

use num_bigint::BigInt;
use num_traits::ToPrimitive;

/// A raw integer, human fraction or textual proportion input.
///
/// Integers retain their raw meaning; floats and text containing `.` or `e`
/// carry a human fraction. Boolean inputs produce Python's explicit error.
#[derive(Clone, Debug, PartialEq)]
pub enum ProportionInput<'a> {
    /// Raw chain integer, retaining oversized input for exact diagnostics.
    Raw(BigInt),
    /// Human fraction from zero through one.
    Fraction(f64),
    /// Text interpreted using Python's integer-or-float convention.
    Text(Cow<'a, str>),
    /// A boolean, rejected instead of being treated as zero or one.
    Boolean(bool),
}

macro_rules! raw_input {
    ($($type:ty),+ $(,)?) => {$ (
        impl From<$type> for ProportionInput<'_> {
            fn from(value: $type) -> Self { Self::Raw(BigInt::from(value)) }
        }
    )+};
}
raw_input!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, BigInt
);

impl From<f64> for ProportionInput<'_> {
    fn from(value: f64) -> Self {
        Self::Fraction(value)
    }
}

impl From<bool> for ProportionInput<'_> {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl<'a> From<&'a str> for ProportionInput<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(Cow::Borrowed(value))
    }
}

impl From<String> for ProportionInput<'_> {
    fn from(value: String) -> Self {
        Self::Text(Cow::Owned(value))
    }
}

/// Invalid proportion input, carrying Python's error message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProportionError(String);

impl ProportionError {
    /// Python-compatible diagnostic text.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProportionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProportionError {}

// py: sdk/python/bittensor/hyperparams.py:581-621 (proportion_to_raw)
/// Convert a fraction or raw input using the default `"proportion"` label.
///
/// Float multiplication and rounding follow Python, including half to even.
/// The result uses `u128`: `1.0 * u64::MAX` rounds to `2**64` in Python. Chain
/// callers must check the final field width when encoding the result.
pub fn proportion_to_raw<'a>(
    value: impl Into<ProportionInput<'a>>,
    denominator: u64,
) -> Result<u128, ProportionError> {
    proportion_to_raw_with_label(value, denominator, "proportion")
}

/// Convert a proportion with a field-specific label in error messages.
pub fn proportion_to_raw_with_label<'a>(
    value: impl Into<ProportionInput<'a>>,
    denominator: u64,
    label: &str,
) -> Result<u128, ProportionError> {
    let input = match value.into() {
        ProportionInput::Text(value) => {
            let lowered = value.trim_matches(crate::python::whitespace).to_lowercase();
            let text = lowered.strip_prefix('+').unwrap_or(&lowered);
            let parsed = if text.contains(['.', 'e']) {
                crate::python::float(text).map(ProportionInput::Fraction)
            } else {
                crate::python::integer(text).map(ProportionInput::Raw)
            };
            parsed.ok_or_else(|| {
                ProportionError(format!(
                    "{label} takes a 0..1 fraction (e.g. 0.18) or the raw integer (0..{denominator}); got {}",
                    crate::python::string_repr(&value)
                ))
            })?
        }
        input => input,
    };
    match input {
        ProportionInput::Boolean(_) => Err(ProportionError(format!(
            "{label} takes a 0..1 fraction or a raw integer, not a boolean"
        ))),
        ProportionInput::Raw(value) => {
            if value < BigInt::from(0u8) || value > BigInt::from(denominator) {
                return Err(ProportionError(format!(
                    "a raw {label} must be within 0..{denominator} (= 1.0); got {value}. Pass a value with a decimal point for the 0..1 human form."
                )));
            }
            value
                .to_u128()
                .ok_or_else(|| ProportionError(format!("{label} value is out of range")))
        }
        ProportionInput::Fraction(value) => {
            if !(0.0..=1.0).contains(&value) {
                return Err(ProportionError(format!(
                    "a fractional {label} must be within 0..1 (e.g. 0.18); got {}. Pass a plain integer for the raw 0..{denominator} form.",
                    crate::pyfmt::float_repr(value)
                )));
            }
            (value * denominator as f64)
                .round_ties_even()
                .to_u128()
                .ok_or_else(|| ProportionError(format!("{label} value is out of range")))
        }
        ProportionInput::Text(_) => unreachable!("text input normalized above"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_raw_and_fractions_round_half_even() -> Result<(), ProportionError> {
        assert_eq!(proportion_to_raw(1, 65535)?, 1);
        assert_eq!(proportion_to_raw("1", 65535)?, 1);
        assert_eq!(proportion_to_raw(0.5, 65535)?, 32768);
        assert_eq!(proportion_to_raw("0.5", 65535)?, 32768);
        assert_eq!(proportion_to_raw(0.5, 5)?, 2);
        assert_eq!(proportion_to_raw(0.5, 7)?, 4);
        assert_eq!(proportion_to_raw("1e0", 65535)?, 65535);
        assert_eq!(proportion_to_raw("0.18", 65535)?, 11796);
        assert_eq!(proportion_to_raw(0.9, 1_000_000)?, 900_000);
        Ok(())
    }

    #[test]
    fn full_native_denominator_preserves_python_rounding() -> Result<(), ProportionError> {
        assert_eq!(proportion_to_raw(u64::MAX, u64::MAX)?, u128::from(u64::MAX));
        assert_eq!(proportion_to_raw(0.5, u64::MAX)?, 1u128 << 63);
        assert_eq!(proportion_to_raw(1.0, u64::MAX)?, 1u128 << 64);
        assert_eq!(proportion_to_raw("1.0", u64::MAX)?, 1u128 << 64);
        assert!(u64::try_from(proportion_to_raw(1.0, u64::MAX)?).is_err());
        Ok(())
    }

    #[test]
    fn strings_keep_python_numeric_conventions() -> Result<(), ProportionError> {
        assert_eq!(proportion_to_raw(" +１_２ ", 65535)?, 12);
        assert_eq!(proportion_to_raw("++1", 65535)?, 1);
        assert_eq!(proportion_to_raw(".٥", 65535)?, 32768);
        assert_eq!(proportion_to_raw("1e-10000", 65535)?, 0);
        for text in ["1__2", "nan", "infinity", "not'a number", "1_.0"] {
            let error = proportion_to_raw(text, 65535).expect_err("malformed numeric input");
            assert!(error.message().ends_with(&crate::python::string_repr(text)));
        }
        Ok(())
    }

    #[test]
    fn errors_keep_exact_original_values() {
        assert_eq!(
            proportion_to_raw_with_label(true, 100, "take")
                .unwrap_err()
                .message(),
            "take takes a 0..1 fraction or a raw integer, not a boolean"
        );
        assert_eq!(
            proportion_to_raw_with_label(-1, 100, "take")
                .unwrap_err()
                .message(),
            "a raw take must be within 0..100 (= 1.0); got -1. Pass a value with a decimal point for the 0..1 human form."
        );
        assert_eq!(
            proportion_to_raw_with_label(1.5, 100, "take")
                .unwrap_err()
                .message(),
            "a fractional take must be within 0..1 (e.g. 0.18); got 1.5. Pass a plain integer for the raw 0..100 form."
        );
        let oversized = "170141183460469231731687303715884105728";
        assert!(
            proportion_to_raw(oversized, 100)
                .unwrap_err()
                .message()
                .contains(oversized)
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                proportion_to_raw(value, 100)
                    .unwrap_err()
                    .message()
                    .contains(&crate::pyfmt::float_repr(value))
            );
        }
    }
}
