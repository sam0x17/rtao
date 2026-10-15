//! Numeric input and diagnostics shared by the pure Python helper ports.

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::Zero;
use unicode_general_category::{GeneralCategory, get_general_category};

pub(crate) fn whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}

pub(crate) fn decimal_digit(character: char) -> Option<u8> {
    if character.is_ascii_digit() {
        return Some(character as u8 - b'0');
    }
    if get_general_category(character) != GeneralCategory::DecimalNumber {
        return None;
    }
    let mut start = u32::from(character);
    while start > 0 {
        let previous = char::from_u32(start - 1)?;
        if get_general_category(previous) != GeneralCategory::DecimalNumber {
            break;
        }
        start -= 1;
    }
    Some(((u32::from(character) - start) % 10) as u8)
}

fn numeric_text(text: &str) -> Option<String> {
    let mut normalized = String::with_capacity(text.len());
    let chars: Vec<_> = text.trim_matches(whitespace).chars().collect();
    for (index, character) in chars.iter().copied().enumerate() {
        if character == '_' {
            if index == 0
                || index + 1 == chars.len()
                || decimal_digit(chars[index - 1]).is_none()
                || decimal_digit(chars[index + 1]).is_none()
            {
                return None;
            }
        } else if let Some(digit) = decimal_digit(character) {
            normalized.push(char::from(b'0' + digit));
        } else if character.is_ascii() {
            normalized.push(character);
        } else {
            return None;
        }
    }
    Some(normalized)
}

pub(crate) fn float(text: &str) -> Option<f64> {
    let normalized = numeric_text(text)?.to_ascii_lowercase();
    match normalized.as_str() {
        "nan" | "+nan" => Some(f64::NAN),
        "-nan" => Some(-f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
        "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
        _ => normalized.parse().ok(),
    }
}

pub(crate) fn integer(text: &str) -> Option<BigInt> {
    let normalized = numeric_text(text)?;
    let digits = normalized.strip_prefix(['+', '-']).unwrap_or(&normalized);
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    normalized.parse().ok()
}

pub(crate) fn string_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::new();
    result.push(quote);
    for character in value.chars() {
        match character {
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character == quote => {
                result.push('\\');
                result.push(character);
            }
            character if !printable(character) => {
                let ordinal = u32::from(character);
                if ordinal <= 0xff {
                    result.push_str(&format!("\\x{ordinal:02x}"));
                } else if ordinal <= 0xffff {
                    result.push_str(&format!("\\u{ordinal:04x}"));
                } else {
                    result.push_str(&format!("\\U{ordinal:08x}"));
                }
            }
            character => result.push(character),
        }
    }
    result.push(quote);
    result
}

fn printable(character: char) -> bool {
    character == ' '
        || !matches!(
            get_general_category(character),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::PrivateUse
                | GeneralCategory::Surrogate
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
                | GeneralCategory::SpaceSeparator
        )
}

// Python int/int division rounds the exact rational once, unlike casting each
// input to f64 and dividing. The callers' nonzero native i128/u64 denominator
// keeps the quotient finite, so the fallible helper cannot fail for them.
pub(crate) fn int_div_to_f64(numerator: i128, denominator: u64) -> f64 {
    int_div_big_to_f64(&BigInt::from(numerator), u128::from(denominator)).unwrap_or(f64::NAN)
}

pub(crate) fn int_div_big_to_f64(numerator: &BigInt, denominator: u128) -> Option<f64> {
    if denominator == 0 {
        return None;
    }
    if numerator.is_zero() {
        return Some(0.0);
    }
    let magnitude = numerator.magnitude();
    let denominator = BigUint::from(denominator);
    let mut exponent = i32::try_from(magnitude.bits()).ok()? - denominator.bits() as i32;
    if exponent > 1024 {
        return None;
    }
    let below = if exponent < 0 {
        (magnitude << (-exponent as usize)) < denominator
    } else {
        magnitude.cmp(&(&denominator << exponent as usize)).is_lt()
    };
    if below {
        exponent -= 1;
    }
    // A u128 denominator keeps every nonzero magnitude above 2^-128, so
    // subnormal rounding cannot occur. Only the upper binary64 limit can fail.
    if exponent > 1023 {
        return None;
    }
    let shift = 52 - exponent;
    let (scaled, divisor) = if shift >= 0 {
        (magnitude << shift as usize, denominator)
    } else {
        (magnitude.clone(), denominator << (-shift as usize))
    };
    let mut significand = &scaled / &divisor;
    let remainder = &scaled % &divisor;
    let twice = remainder << 1usize;
    if twice > divisor || (twice == divisor && !(&significand & BigUint::from(1u8)).is_zero()) {
        significand += 1u8;
    }
    // The scaled exact quotient has 53 bits, so its only limb is sufficient.
    let mut significand = significand.to_u64_digits().first().copied().unwrap_or(0);
    if significand == 1 << 53 {
        significand >>= 1;
        exponent += 1;
    }
    if exponent > 1023 {
        return None;
    }
    let sign = u64::from(numerator.sign() == Sign::Minus) << 63;
    Some(f64::from_bits(
        sign | (((exponent + 1023) as u64) << 52) | (significand - (1 << 52)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_numeric_syntax() {
        assert_eq!(float(" +١_٢.٥e1 "), Some(125.0));
        assert_eq!(integer(" -１２_３ "), Some(BigInt::from(-123)));
        for bad in ["1__2", "_1", "1_", "1_.0", "1e_2", "¹"] {
            assert!(float(bad).is_none(), "{bad}");
            assert!(integer(bad).is_none(), "{bad}");
        }
        assert!(float("NAN").is_some_and(f64::is_nan));
        assert_eq!(float("-Infinity"), Some(f64::NEG_INFINITY));
        assert!(integer("1.0").is_none());
    }

    #[test]
    fn python_string_diagnostics() {
        assert_eq!(string_repr("don't"), "\"don't\"");
        assert_eq!(string_repr("\n\u{a0}\u{200b}"), "'\\n\\xa0\\u200b'");
    }
}
