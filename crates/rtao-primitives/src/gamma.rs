// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/_money.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Gamma amounts: exact decimal strings and the chain's units.
//!
//! Gamma is not a [`Balance`] unit. Python keeps a gamma amount as an exact
//! decimal string (`"1.5"`) and converts it to the chain's units,
//! [`GAMMA_UNITS`](crate::gamma::GAMMA_UNITS) per gamma, only when composing
//! a call.

use std::fmt;

use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};

/// Chain units per gamma (`settings.GAMMA_UNITS`).
pub const GAMMA_UNITS: u128 = 1_000_000_000;

/// Sentinel accepted by opt-in gamma fields: "use everything available".
pub const ALL: &str = "all";

/// A gamma amount that is not a finite, non-negative 9-decimal number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GammaAmountError(String);

impl GammaAmountError {
    /// Python's message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GammaAmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GammaAmountError {}

/// An exact decimal `coefficient * 10^exponent`, parsed as Python's `Decimal`.
struct Exact {
    coefficient: BigInt,
    exponent: i64,
}

// Python's `Decimal(str)`: surrounding whitespace, a sign, digits with an
// optional point (and `_` separators between digits) and an optional exponent.
// Infinity and NaN parse in Python but are refused as non-finite below.
fn parse_decimal(text: &str) -> Option<Exact> {
    let text = text.trim();
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(index) => (&body[..index], Some(&body[index + 1..])),
        None => (body, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (mantissa, ""),
    };
    let digits = |part: &str| {
        !part.starts_with('_')
            && !part.ends_with('_')
            && !part.contains("__")
            && part.chars().all(|c| c.is_ascii_digit() || c == '_')
    };
    if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    let whole: String = whole.chars().filter(char::is_ascii_digit).collect();
    let fraction: String = fraction.chars().filter(char::is_ascii_digit).collect();
    let mut exponent = match exponent {
        Some(text) => {
            let (sign, digits_text) = match text.as_bytes().first() {
                Some(b'-') => (-1, &text[1..]),
                Some(b'+') => (1, &text[1..]),
                _ => (1, text),
            };
            if digits_text.is_empty() || !digits(digits_text) {
                return None;
            }
            let digits_text: String = digits_text.chars().filter(char::is_ascii_digit).collect();
            sign * digits_text.parse::<i64>().ok()?
        }
        None => 0,
    };
    exponent = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    let mut coefficient: BigInt = format!("{whole}{fraction}").parse().ok()?;
    if negative {
        coefficient = -coefficient;
    }
    Some(Exact {
        coefficient,
        exponent,
    })
}

fn ten_pow(exponent: u64) -> Option<BigInt> {
    Some(BigInt::from(10u8).pow(u32::try_from(exponent).ok()?))
}

// `format(Decimal(...).normalize(), "f")` for a non-negative exact value:
// trailing zeros dropped, no exponent, `"0"` for zero.
fn plain(units: &BigInt, scale: u32) -> String {
    let divisor = BigInt::from(10u8).pow(scale);
    let whole = units / &divisor;
    let fraction = units % &divisor;
    if fraction.is_zero() {
        return whole.to_string();
    }
    let fraction = format!("{:0>width$}", fraction, width = scale as usize);
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

// py: sdk/python/bittensor/intents/_money.py:89-110
/// Normalize a gamma amount (a number's text or a decimal string) to an exact
/// decimal string; `"all"` only when `allow_all`.
pub fn gamma_amount(value: &str, allow_all: bool) -> Result<String, GammaAmountError> {
    if value.trim().eq_ignore_ascii_case(ALL) {
        if !allow_all {
            return Err(GammaAmountError(
                "gamma amount does not accept 'all'.".into(),
            ));
        }
        return Ok(ALL.into());
    }
    let shown = crate::pyfmt::string_repr(value);
    let finite = || {
        GammaAmountError(format!(
            "gamma amount must be a finite number, got {shown}."
        ))
    };
    let lower = value.trim().to_ascii_lowercase();
    let lower = lower.trim_start_matches(['+', '-']);
    let special = matches!(lower, "inf" | "infinity" | "nan" | "snan")
        || lower.starts_with("nan")
        || lower.starts_with("snan");
    let range = || {
        GammaAmountError(format!(
            "gamma amount must be non-negative with at most 9 decimals, got {shown}."
        ))
    };
    if special {
        // Decimal accepts these; they are not finite units.
        return Err(range());
    }
    let exact = parse_decimal(value).ok_or_else(finite)?;
    if exact.coefficient.sign() == Sign::Minus {
        return Err(range());
    }
    // units = amount * 10^9 must be integral.
    let shifted = exact.exponent + 9;
    let units = if shifted >= 0 {
        exact.coefficient * ten_pow(shifted.unsigned_abs()).ok_or_else(range)?
    } else {
        let divisor = ten_pow(shifted.unsigned_abs()).ok_or_else(range)?;
        if !(&exact.coefficient % &divisor).is_zero() {
            return Err(range());
        }
        exact.coefficient / divisor
    };
    Ok(plain(&units, 9))
}

/// A gamma amount for a JSON number or string argument, as Python's
/// `Decimal(str(value))`.
pub fn gamma_amount_json(
    value: &serde_json::Value,
    allow_all: bool,
) -> Result<String, GammaAmountError> {
    match value {
        serde_json::Value::String(text) => gamma_amount(text, allow_all),
        serde_json::Value::Number(number) => {
            let text = number
                .as_f64()
                .filter(|_| number.is_f64())
                .map_or_else(|| number.to_string(), crate::pyfmt::float_repr);
            gamma_amount(&text, allow_all)
        }
        other => Err(GammaAmountError(format!(
            "gamma amount must be a finite number, got {}.",
            match other {
                serde_json::Value::Bool(true) => "True".into(),
                serde_json::Value::Bool(false) => "False".into(),
                serde_json::Value::Null => "None".into(),
                other => other.to_string(),
            }
        ))),
    }
}

// py: sdk/python/bittensor/intents/_money.py:113-115
/// A normalized gamma amount from [`gamma_amount`] in the chain's units.
pub fn gamma_units(amount: &str) -> Result<u128, GammaAmountError> {
    let normalized = gamma_amount(amount, false)?;
    let exact = parse_decimal(&normalized)
        .ok_or_else(|| GammaAmountError("gamma amount is not normalized".into()))?;
    let shifted = u64::try_from(exact.exponent + 9)
        .map_err(|_| GammaAmountError("gamma amount is not normalized".into()))?;
    let units = exact.coefficient
        * ten_pow(shifted).ok_or_else(|| GammaAmountError("gamma amount overflows".into()))?;
    units.to_u128().ok_or_else(|| {
        GammaAmountError(format!(
            "gamma amount {normalized} exceeds the chain's u128 units"
        ))
    })
}

// py: sdk/python/bittensor/intents/_money.py:118-120
/// The chain's gamma units as an exact decimal string, the inverse of
/// [`gamma_units`].
pub fn gamma_from_units(units: u128) -> String {
    plain(&BigInt::from(units), 9)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Fixtures name exact expected results.
    use super::*;

    #[test]
    fn amounts_normalize_like_python_decimal() {
        for (input, expected) in [
            ("1", "1"),
            ("1.50", "1.5"),
            (" 0.000000001 ", "0.000000001"),
            ("1e3", "1000"),
            ("1.5E-3", "0.0015"),
            ("0", "0"),
            ("0.0", "0"),
            ("1_000", "1000"),
            ("ALL", "all"),
        ] {
            assert_eq!(gamma_amount(input, true).unwrap(), expected, "{input}");
        }
        for input in ["-1", "0.0000000001", "nan", "inf", "-Infinity"] {
            assert!(
                gamma_amount(input, false)
                    .unwrap_err()
                    .message()
                    .contains("non-negative with at most 9 decimals"),
                "{input}"
            );
        }
        for input in ["", "abc", "1..2", "_1", "1e"] {
            assert!(
                gamma_amount(input, false)
                    .unwrap_err()
                    .message()
                    .contains("finite number"),
                "{input}"
            );
        }
        assert_eq!(
            gamma_amount("all", false).unwrap_err().message(),
            "gamma amount does not accept 'all'."
        );
    }

    #[test]
    fn units_round_trip() {
        assert_eq!(gamma_units("1.5").unwrap(), 1_500_000_000);
        assert_eq!(gamma_units("0.000000001").unwrap(), 1);
        assert_eq!(gamma_from_units(1_500_000_000), "1.5");
        assert_eq!(gamma_from_units(2_000_000_000), "2");
        assert_eq!(gamma_from_units(123), "0.000000123");
        assert_eq!(gamma_from_units(0), "0");
        assert_eq!(
            gamma_amount_json(&serde_json::json!(0.1), false).unwrap(),
            "0.1"
        );
        assert_eq!(
            gamma_amount_json(&serde_json::json!(3), false).unwrap(),
            "3"
        );
    }
}
