// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/balance.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/hyperparams.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Python-compatible text, numeric parsing and balance formatting.
//!
//! Balance conversion rounds the integer quotient once, before formatting the
//! resulting binary float. Decimal formatting rounds that exact binary value,
//! including halfway cases, without multiplying or rounding in floating point.
//! Balance helpers accept the chain's native non-negative u64 rao range.

use num_bigint::BigUint;
use num_traits::Zero;

use crate::NetUid;

/// Render a string with Python's quote selection and Unicode escaping.
///
/// Shared by Python-compatible diagnostics, including wallet name validation.
pub fn string_repr(value: &str) -> String {
    crate::python::string_repr(value)
}

// py: sdk/python/bittensor/metagraph.py:459-472 (_text)
// py: sdk/python/bittensor/metagraph.py:489-510 (_decode_fields)
/// Decode Python's `bytes.fromhex(text)` syntax, without a `0x` prefix.
///
/// The six ASCII whitespace characters are accepted between complete byte
/// pairs. Errors retain Python's message and position; the first non-ASCII
/// character is rejected before checking earlier malformed ASCII pairs.
pub fn bytes_fromhex(text: &str) -> Result<Vec<u8>, String> {
    let chars: Vec<_> = text.chars().collect();
    let mut index = 0;
    let mut bytes = Vec::new();
    let error =
        |position| format!("non-hexadecimal number found in fromhex() arg at position {position}");
    // CPython rejects the first non-ASCII character before inspecting any
    // hexadecimal pairs, even when an earlier ASCII character is malformed.
    if let Some(position) = chars.iter().position(|character| !character.is_ascii()) {
        return Err(error(position));
    }
    while index < chars.len() {
        if matches!(chars[index], '\t'..='\r' | ' ') {
            index += 1;
            continue;
        }
        let upper = chars[index]
            .to_digit(16)
            .filter(|_| chars[index].is_ascii())
            .ok_or_else(|| error(index))?;
        index += 1;
        let lower = chars
            .get(index)
            .and_then(|digit| digit.to_digit(16).filter(|_| digit.is_ascii()))
            .ok_or_else(|| error(index))?;
        bytes.push((upper * 16 + lower) as u8);
        index += 1;
    }
    Ok(bytes)
}

/// Parse Python's base-ten `int(text)` syntax without losing integer precision.
///
/// Signs, digit separators and Unicode decimal digits are accepted. Callers
/// enforce their native field width after parsing. ASCII file/group/record/unit
/// separators are accepted by Python's `str.strip`, but rejected by `int`.
pub fn parse_integer(value: &str) -> Option<num_bigint::BigInt> {
    if value
        .chars()
        .any(|character| matches!(character, '\u{1c}'..='\u{1f}'))
    {
        return None;
    }
    crate::python::integer(value)
}

/// Parse Python's `float(text)` syntax, including infinity and NaN.
///
/// Signs, digit separators and Unicode decimal digits are accepted. Callers
/// enforce any finite-value requirement after parsing. ASCII file/group/record/
/// unit separators are accepted by Python's `str.strip`, but rejected by `float`.
pub fn parse_float(value: &str) -> Option<f64> {
    if value
        .chars()
        .any(|character| matches!(character, '\u{1c}'..='\u{1f}'))
    {
        return None;
    }
    crate::python::float(value)
}

// py: sdk/python/bittensor/balance.py:92-116 (tao, alpha, amount)
// py: sdk/python/bittensor/settings.py:16 (RAO_PER_TAO)
/// Return Python's correctly rounded `rao / 10**9` as a binary64 float.
///
/// The integer numerator is never converted to a float before division. This
/// avoids double rounding for amounts greater than `2**53` rao.
pub fn rao_to_f64(rao: u64) -> f64 {
    if rao == 0 {
        return 0.0;
    }

    const DENOMINATOR: u128 = 1_000_000_000;
    let numerator = u128::from(rao);
    // First locate the binary exponent of the exact rational quotient.
    let mut exponent = (64 - rao.leading_zeros()) as i32 - 30;
    let below_power = if exponent < 0 {
        (numerator << (-exponent as u32)) < DENOMINATOR
    } else {
        numerator < (DENOMINATOR << (exponent as u32))
    };
    if below_power {
        exponent -= 1;
    }

    // The scaled numerator is at most 83 bits, even at either u64 boundary.
    let scaled = numerator << ((52 - exponent) as u32);
    let mut significand = scaled / DENOMINATOR;
    let remainder = scaled % DENOMINATOR;
    if remainder * 2 > DENOMINATOR || (remainder * 2 == DENOMINATOR && significand & 1 != 0) {
        significand += 1;
    }
    if significand == 1 << 53 {
        significand >>= 1;
        exponent += 1;
    }
    let bits = (((exponent + 1023) as u64) << 52) | (significand as u64 - (1 << 52));
    f64::from_bits(bits)
}

// py: sdk/python/bittensor/balance.py:147-150 (Balance.__repr__)
/// Format as Python's `format(x, ",.9f")`, including grouping and signed zero.
///
/// Rounding is half to even from the exact binary value. Every finite binary64
/// value is supported, including subnormals and values beyond balance ranges.
/// Non-finite values render as `inf`, `-inf`, or `nan`.
pub fn fmt_comma_9f(x: f64) -> String {
    if let Some(special) = special_value(x) {
        return special.to_owned();
    }
    let (numerator, denominator) = magnitude_fraction(x);
    let scaled = rounded_decimal(&numerator, &denominator, 9);
    let digits = scaled.to_str_radix(10);
    let padded = if digits.len() < 10 {
        format!("{}{}", "0".repeat(10 - digits.len()), digits)
    } else {
        digits
    };
    let (integer, fractional) = padded.split_at(padded.len() - 9);
    let mut out = String::with_capacity(padded.len() + integer.len() / 3 + 2);
    if x.is_sign_negative() {
        out.push('-');
    }
    for (index, digit) in integer.bytes().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(char::from(digit));
    }
    out.push('.');
    out.push_str(fractional);
    out
}

// py: sdk/python/bittensor/balance.py:127-152 (unit, __repr__, __str__)
/// Render Python's `str(Balance.from_rao(rao, netuid, symbol))` for native u64 rao.
///
/// TAO uses a `τ` prefix regardless of the supplied symbol. Alpha uses a
/// nonempty supplied symbol as its suffix, or `α` and the subscript netuid.
pub fn balance_str(rao: u64, netuid: NetUid, symbol: Option<&str>) -> String {
    let amount = fmt_comma_9f(rao_to_f64(rao));
    if netuid.0 == 0 {
        return format!("τ{amount}");
    }
    if let Some(unit) = symbol.filter(|unit| !unit.is_empty()) {
        return format!("{amount}{unit}");
    }
    const SUBSCRIPT_DIGITS: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    let mut out = format!("{amount}α");
    for digit in netuid.0.to_string().bytes() {
        out.push(SUBSCRIPT_DIGITS[usize::from(digit - b'0')]);
    }
    out
}

// py: sdk/python/bittensor/balance.py:90 (Decimal(str(amount)))
/// Format Python's `repr(float)`: shortest round trip with Python's notation.
///
/// Decimal exponents from -4 through 15 use fixed notation. Other exponents
/// use `e`, an explicit sign, and at least two digits. Integral fixed values
/// have a `.0` suffix. Signed zero and non-finite values follow Python too.
pub fn float_repr(x: f64) -> String {
    if let Some(special) = special_value(x) {
        return special.to_owned();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_owned();
    }
    let (digits, exponent) = shortest_parts(x.abs());
    let body = if (-4..16).contains(&exponent) {
        fixed_digits(&digits, exponent, true)
    } else {
        scientific_digits(&digits, exponent)
    };
    with_sign(body, x)
}

// py: sdk/python/bittensor/http_auth.py:311 ("{max_age:g}s" diagnostics)
/// Format Python's `format(x, "g")` with the default six significant digits.
///
/// Rounding is half to even from the exact binary value. Insignificant zeros
/// are removed; exponents below -4 or at least 6 use scientific notation. The
/// notation decision uses the exponent after rounding, as Python does.
pub fn fmt_g(x: f64) -> String {
    fmt_g_precision(x, 6)
}

// py: sdk/python/bittensor/hyperparams.py:537-556 (annotate)
/// Format Python's `format(x, ".4g")` with four significant digits.
///
/// Rounding is half to even from the exact binary value. Insignificant zeros
/// are removed; exponents below -4 or at least 4 use scientific notation. The
/// notation decision uses the exponent after rounding.
pub fn fmt_g4(x: f64) -> String {
    fmt_g_precision(x, 4)
}

fn fmt_g_precision(x: f64, precision: i32) -> String {
    if let Some(special) = special_value(x) {
        return special.to_owned();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }

    let (numerator, denominator) = magnitude_fraction(x);
    let (_, mut exponent) = shortest_parts(x.abs());
    // A shortest decimal can cross an exact power-of-ten boundary. Locate
    // the exponent of the binary value before choosing the rounding scale.
    while compare_power(&numerator, &denominator, exponent).is_lt() {
        exponent -= 1;
    }
    while !compare_power(&numerator, &denominator, exponent + 1).is_lt() {
        exponent += 1;
    }

    let rounded = rounded_decimal(&numerator, &denominator, precision - 1 - exponent);
    let mut digits = rounded.to_str_radix(10);
    if digits.len() == precision as usize + 1 {
        // A carry into the next decimal power raises the exponent by one.
        digits.pop();
        exponent += 1;
    }
    trim_zeros(&mut digits);
    let body = if (-4..precision).contains(&exponent) {
        fixed_digits(&digits, exponent, false)
    } else {
        scientific_digits(&digits, exponent)
    };
    with_sign(body, x)
}

fn special_value(x: f64) -> Option<&'static str> {
    if x.is_nan() {
        Some("nan")
    } else if x == f64::INFINITY {
        Some("inf")
    } else if x == f64::NEG_INFINITY {
        Some("-inf")
    } else {
        None
    }
}

fn magnitude_fraction(x: f64) -> (BigUint, BigUint) {
    let bits = x.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (significand, exponent) = if raw_exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), raw_exponent - 1023 - 52)
    };
    let numerator = BigUint::from(significand);
    let one = BigUint::from(1_u8);
    if exponent >= 0 {
        (numerator << (exponent as usize), one)
    } else {
        (numerator, one << (-exponent as usize))
    }
}

fn rounded_decimal(numerator: &BigUint, denominator: &BigUint, places: i32) -> BigUint {
    let mut numerator = numerator.clone();
    let mut denominator = denominator.clone();
    let scale = BigUint::from(10_u8).pow(places.unsigned_abs());
    if places >= 0 {
        numerator *= scale;
    } else {
        denominator *= scale;
    }
    let mut quotient = &numerator / &denominator;
    let twice_remainder = (numerator % &denominator) << 1_usize;
    if twice_remainder > denominator
        || (twice_remainder == denominator && !(&quotient & BigUint::from(1_u8)).is_zero())
    {
        quotient += 1_u8;
    }
    quotient
}

fn compare_power(numerator: &BigUint, denominator: &BigUint, exponent: i32) -> std::cmp::Ordering {
    let scale = BigUint::from(10_u8).pow(exponent.unsigned_abs());
    if exponent >= 0 {
        numerator.cmp(&(denominator * scale))
    } else {
        (numerator * scale).cmp(denominator)
    }
}

fn shortest_parts(x: f64) -> (String, i32) {
    let mut buffer = ryu::Buffer::new();
    let shortest = buffer.format_finite(x);
    let (mantissa, exponent_text) = shortest.split_once('e').unwrap_or((shortest, "0"));
    // Ryu's finite representation is ASCII with a bounded decimal exponent.
    let exponent_negative = exponent_text.starts_with('-');
    let exponent = exponent_text
        .bytes()
        .filter(u8::is_ascii_digit)
        .fold(0_i32, |n, b| n * 10 + i32::from(b - b'0'))
        * if exponent_negative { -1 } else { 1 };
    let decimal_point = mantissa.find('.').unwrap_or(mantissa.len());
    let mut digits: String = mantissa
        .bytes()
        .filter(|&b| b != b'.')
        .map(char::from)
        .collect();
    let leading_zeros = digits.bytes().take_while(|&b| b == b'0').count();
    digits.drain(..leading_zeros);
    trim_zeros(&mut digits);
    (
        digits,
        exponent + decimal_point as i32 - leading_zeros as i32 - 1,
    )
}

fn trim_zeros(digits: &mut String) {
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
}

fn fixed_digits(digits: &str, exponent: i32, integer_dot: bool) -> String {
    let point = exponent + 1;
    if point <= 0 {
        format!("0.{}{digits}", "0".repeat(-point as usize))
    } else if point as usize >= digits.len() {
        let mut out = format!("{digits}{}", "0".repeat(point as usize - digits.len()));
        if integer_dot {
            out.push_str(".0");
        }
        out
    } else {
        let (whole, fraction) = digits.split_at(point as usize);
        format!("{whole}.{fraction}")
    }
}

fn scientific_digits(digits: &str, exponent: i32) -> String {
    let (leading, rest) = digits.split_at(1);
    let mantissa = if rest.is_empty() {
        leading.to_owned()
    } else {
        format!("{leading}.{rest}")
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs())
}

fn with_sign(body: String, x: f64) -> String {
    if x.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn float_text_accepts_python_numeric_syntax() {
        for (text, expected) in [
            (" 1.0 ", 1.0_f64),
            ("\t+1_234.5_0e-1\r\n", 123.45),
            ("\u{a0}+١_٢.٥e١\u{2003}", 125.0),
            ("１２３.４_５", 123.45),
            ("-０.０", -0.0),
            ("1e-9999", 0.0),
        ] {
            assert_eq!(
                parse_float(text).map(f64::to_bits),
                Some(expected.to_bits()),
                "{text:?}"
            );
        }
        assert_eq!(parse_float(" +InFiNiTy "), Some(f64::INFINITY));
        assert_eq!(parse_float("-inf"), Some(f64::NEG_INFINITY));
        assert!(parse_float(" NaN ").is_some_and(f64::is_nan));
        assert_eq!(parse_float("1e9999"), Some(f64::INFINITY));
    }

    #[test]
    fn float_text_rejects_invalid_separators_and_non_decimal_digits() {
        for text in [
            "",
            " ",
            "_1",
            "1_",
            "1__2",
            "1_.0",
            "1._0",
            "1e_2",
            "1_e2",
            "1 2",
            "1,2",
            "0x1",
            "¹",
            "＋１",
            "１．０",
            "\u{1c}1",
            "1\u{1d}",
            "\u{1e}1",
            "1\u{1f}",
        ] {
            assert_eq!(parse_float(text), None, "{text:?}");
        }
    }

    #[test]
    fn unicode_data_matches_the_pinned_python_312_oracle() {
        // Nag Mundari was added in Unicode 15. Garay, Sunuwar and the new
        // Myanmar digit sets are Unicode 16 and remain unassigned in Python 3.12.
        assert_eq!(parse_integer("\u{1e4f1}"), Some(1.into()));
        assert_eq!(parse_float("\u{1e4f1}"), Some(1.0));
        for text in ["\u{10d41}", "\u{11bf1}", "\u{116d1}"] {
            assert_eq!(parse_integer(text), None);
            assert_eq!(parse_float(text), None);
        }
        assert_eq!(string_repr("\u{1c89}"), "'\\u1c89'");
        assert_eq!(string_repr("\u{10d40}"), "'\\U00010d40'");
    }

    #[test]
    fn integer_division_rounds_once() {
        assert_eq!(rao_to_f64(0).to_bits(), 0);
        assert_eq!(rao_to_f64(1).to_bits(), 1e-9_f64.to_bits());
        assert_eq!(rao_to_f64(1_000_000_000), 1.0);
        assert_eq!(rao_to_f64(21_000_000_000_000_000), 21_000_000.0);
        // Exact rational regression around the double-rounding boundary.
        // These are mathematical expectations, not recorded Python vectors.
        let rao = 9_007_199_254_740_995;
        assert_eq!(rao_to_f64(rao).to_bits(), 0x4161_2e0b_e826_d696);
        assert_eq!((rao as f64 / 1e9).to_bits(), 0x4161_2e0b_e826_d697);
        assert_eq!(rao_to_f64(u64::MAX).to_bits(), 0x4211_2e0b_e826_d695);
    }

    #[test]
    fn fixed_decimal_rounds_exact_binary_ties_to_even() {
        // Architecture §5.3 names 2^-10 multiples as halfway regressions.
        assert_eq!(fmt_comma_9f(1.0 / 1024.0), "0.000976562");
        assert_eq!(fmt_comma_9f(3.0 / 1024.0), "0.002929688");
        assert_eq!(fmt_comma_9f(-1.0 / 1024.0), "-0.000976562");
        assert_eq!(fmt_comma_9f(-0.0), "-0.000000000");
        assert_eq!(fmt_comma_9f(-f64::from_bits(1)), "-0.000000000");
        assert_eq!(fmt_comma_9f(1_234_567.5), "1,234,567.500000000");
        assert_eq!(fmt_comma_9f(f64::INFINITY), "inf");
        assert_eq!(fmt_comma_9f(f64::NEG_INFINITY), "-inf");
        assert_eq!(fmt_comma_9f(-f64::NAN), "nan");
    }

    #[test]
    fn balances_use_chain_symbols_and_python_fallback() {
        // py: sdk/python/bittensor/balance.py:17-21,127-152
        assert_eq!(
            balance_str(1_000_000_000, NetUid(0), Some("β")),
            "τ1.000000000"
        );
        assert_eq!(
            balance_str(1_500_000_000, NetUid(2), Some("β")),
            "1.500000000β"
        );
        assert_eq!(
            balance_str(1_500_000_000, NetUid(42), None),
            "1.500000000α₄₂"
        );
        assert_eq!(balance_str(0, NetUid(65535), Some("")), "0.000000000α₆₅₅₃₅");
        assert_eq!(balance_str(0, NetUid(1), Some(" ")), "0.000000000 ");
        assert_eq!(
            balance_str(21_000_000_000_000_000, NetUid(0), None),
            "τ21,000,000.000000000"
        );
        assert_eq!(
            balance_str(u64::MAX, NetUid(0), None),
            "τ18,446,744,073.709552765"
        );
    }

    #[test]
    fn repr_matches_python_notation_boundaries() {
        for (value, expected) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1e-9, "1e-09"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (123.456, "123.456"),
            (-1e20, "-1e+20"),
            (f64::from_bits(1), "5e-324"),
            (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
            (f64::MAX, "1.7976931348623157e+308"),
            // Both .2 and .3 round trip; Python chooses the even final digit.
            (f64::from_bits(0x4311_2e0b_e826_d695), "1208925819614629.2"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (-f64::NAN, "nan"),
        ] {
            assert_eq!(float_repr(value), expected);
        }
    }

    #[test]
    fn general_format_uses_six_digits_and_rounded_exponent() {
        for (value, expected) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (1.25, "1.25"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1e5, "100000"),
            (1e6, "1e+06"),
            (1_234_567.0, "1.23457e+06"),
            (123_456.5, "123456"),
            (123_457.5, "123458"),
            (999_999.5, "1e+06"),
            (-999_999.5, "-1e+06"),
            (0.00009999994, "9.99999e-05"),
            (0.00009999996, "0.0001"),
            (-0.00009999996, "-0.0001"),
            (f64::from_bits(1), "4.94066e-324"),
            (f64::MIN_POSITIVE, "2.22507e-308"),
            (f64::MAX, "1.79769e+308"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (-f64::NAN, "nan"),
        ] {
            assert_eq!(fmt_g(value), expected);
        }
    }

    #[test]
    fn four_digit_format_rounds_ties_carries_and_notation_boundaries() {
        for (value, expected) in [
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (1.25, "1.25"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1e3, "1000"),
            (1e4, "1e+04"),
            (12_345.0, "1.234e+04"),
            (12_355.0, "1.236e+04"),
            (1234.5, "1234"),
            (1235.5, "1236"),
            (-1234.5, "-1234"),
            (-1235.5, "-1236"),
            (9998.5, "9998"),
            (9999.5, "1e+04"),
            (-9999.5, "-1e+04"),
            (0.000099994, "9.999e-05"),
            (0.000099996, "0.0001"),
            (-0.000099996, "-0.0001"),
            (f64::from_bits(1), "4.941e-324"),
            (-f64::from_bits(1), "-4.941e-324"),
            (f64::MIN_POSITIVE, "2.225e-308"),
            (f64::MAX, "1.798e+308"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (-f64::NAN, "nan"),
        ] {
            assert_eq!(fmt_g4(value), expected, "{value:?}");
        }
    }

    fn quotient_distance(rao: &BigUint, value: f64) -> (BigUint, BigUint) {
        let (numerator, denominator) = magnitude_fraction(value);
        let left = numerator * BigUint::from(1_000_000_000_u64);
        let right = rao * &denominator;
        let distance = if left >= right {
            left - right
        } else {
            right - left
        };
        (distance, denominator)
    }

    proptest! {
        #[test]
        fn quotient_is_nearest_float_to_the_exact_rational(rao in 1_u64..=u64::MAX) {
            let value = rao_to_f64(rao);
            let bits = value.to_bits();
            let integer = BigUint::from(rao);
            let (distance, denominator) = quotient_distance(&integer, value);
            for neighbor in [bits - 1, bits + 1] {
                let (other_distance, other_denominator) = quotient_distance(&integer, f64::from_bits(neighbor));
                let own = &distance * other_denominator;
                let other = other_distance * &denominator;
                prop_assert!(own <= other);
                if own == other {
                    prop_assert_eq!(bits & 1, 0);
                }
            }
        }

        #[test]
        fn repr_roundtrips_every_finite_binary_float(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            if value.is_finite() {
                prop_assert_eq!(float_repr(value).parse::<f64>().map(f64::to_bits), Ok(bits));
            }
        }

        #[test]
        fn fixed_decimal_agrees_with_independent_rust_formatter(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            if value.is_finite() {
                prop_assert_eq!(fmt_comma_9f(value).replace(',', ""), format!("{value:.9}"));
            }
        }

        #[test]
        fn general_format_rounding_agrees_with_independent_six_digit_formatter(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            if value.is_finite() {
                // Scientific .5e also retains six significant digits, while
                // using Rust's formatter rather than our decimal arithmetic.
                // The example tests separately pin Python's notation rules.
                prop_assert_eq!(
                    fmt_g(value).parse::<f64>().map(f64::to_bits),
                    format!("{value:.5e}").parse::<f64>().map(f64::to_bits)
                );
            }
        }

        #[test]
        fn four_digit_rounding_agrees_with_independent_rust_formatter(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            if value.is_finite() {
                // Rust's scientific formatter supplies an independent decimal
                // rounding implementation. Examples pin the notation policy.
                prop_assert_eq!(
                    fmt_g4(value).parse::<f64>().map(f64::to_bits),
                    format!("{value:.3e}").parse::<f64>().map(f64::to_bits)
                );
            }
        }
    }
}
