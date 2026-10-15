// Derived from RaoFoundation/subtensor sdk/python/bittensor/hyperparams.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Ratio normalization by the exact metadata type identity.

use crate::{PerU16, Perbill, Percent, Permill};

/// Runtime ratio identities and their typed primitive denominators.
///
/// A catalog test compares this table with the extracted Python source table.
pub const RATIO_TYPE_DENOMINATORS: &[(&str, u64)] = &[
    ("PerU16", PerU16::DENOMINATOR as u64),
    ("Percent", Percent::DENOMINATOR as u64),
    ("Permill", Permill::DENOMINATOR as u64),
    ("Perbill", Perbill::DENOMINATOR as u64),
];

/// Correctly round an integer quotient to binary64 once, as Python does.
///
/// A zero denominator yields `None`. The integer numerator remains exact until
/// rounding, including native chain reserve amounts above `2^53`.
pub fn integer_division(numerator: i128, denominator: u64) -> Option<f64> {
    (denominator != 0).then(|| crate::python::int_div_to_f64(numerator, denominator))
}

/// Correctly round an arbitrary signed integer quotient to binary64 once.
///
/// The native u128 denominator supports protocol fixed-point settings without
/// restricting observations to a wire integer range. Values are not clamped.
/// A zero denominator or a quotient that rounds beyond binary64's finite range
/// returns `None`, matching Python's division and overflow errors.
pub fn integer_division_big(numerator: &num_bigint::BigInt, denominator: u128) -> Option<f64> {
    crate::python::int_div_big_to_f64(numerator, denominator)
}

// py: sdk/python/bittensor/hyperparams.py:40-53 (ratio_fraction)
/// Normalize a raw integer when its metadata identity denotes a ratio.
///
/// Unknown, empty and absent identities return `None`. Identity matching is
/// case sensitive; values are neither clamped nor checked against a range.
/// Integer division is correctly rounded once, as in Python's `int / int`.
pub fn ratio_fraction<'a>(
    type_ident: impl Into<Option<&'a str>>,
    raw: impl Into<i128>,
) -> Option<f64> {
    let ident = type_ident.into()?;
    RATIO_TYPE_DENOMINATORS
        .iter()
        .find(|(known, _)| *known == ident)
        .map(|(_, denominator)| crate::python::int_div_to_f64(raw.into(), *denominator))
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::{BigInt, BigUint};
    use num_traits::Zero;
    use proptest::prelude::*;

    // pyt: unit/test_hyperparams_units.py::test_ratio_fraction_known_idents
    #[test]
    fn known_idents() {
        assert_eq!(ratio_fraction("PerU16", 65535), Some(1.0));
        assert_eq!(ratio_fraction("PerU16", 32767), Some(32767.0 / 65535.0));
        assert_eq!(ratio_fraction("Percent", 50), Some(0.5));
        assert_eq!(ratio_fraction("Permill", 900_000), Some(0.9));
        assert_eq!(ratio_fraction("Perbill", 250_000_000), Some(0.25));
        assert_eq!(ratio_fraction("PerU16", 0), Some(0.0));
    }

    // pyt: unit/test_hyperparams_units.py::test_ratio_fraction_non_ratio_idents_return_none
    #[test]
    fn unknown_identities_do_not_normalize() {
        for ident in [
            "u16",
            "u64",
            "TaoBalance",
            "AlphaBalance",
            "Vec<u16>",
            "",
            "percent",
            "sp_arithmetic::Percent",
            "FixedU128",
        ] {
            assert_eq!(ratio_fraction(ident, 123), None);
        }
        assert_eq!(ratio_fraction(None, 123), None);
    }

    #[test]
    fn unbounded_values_and_exact_integer_division() {
        assert_eq!(ratio_fraction("Percent", -50), Some(-0.5));
        assert_eq!(ratio_fraction("Percent", 150), Some(1.5));
        let values = [
            (
                "PerU16",
                9_007_199_254_740_993i128,
                137_441_050_656.000_52f64,
            ),
            ("Percent", 9_007_199_254_740_993, 90_071_992_547_409.94),
            ("Permill", 9_007_199_254_740_993, 9_007_199_254.740_993),
            ("Perbill", 9_007_199_254_740_995, 9_007_199.254_740_994),
        ];
        for (ident, raw, expected) in values {
            assert_eq!(
                ratio_fraction(ident, raw).map(f64::to_bits),
                Some(expected.to_bits())
            );
        }
        assert_eq!(
            ratio_fraction("Percent", i128::MIN).map(f64::to_bits),
            Some((-2.0f64.powi(127) / 100.0).to_bits())
        );
    }

    #[test]
    fn big_integer_division_preserves_signed_native_u128_and_rounds_once() {
        assert_eq!(integer_division_big(&BigInt::from(0), 1), Some(0.0));
        assert_eq!(integer_division_big(&BigInt::from(0), 0), None);
        assert_eq!(integer_division_big(&BigInt::from(1), 0), None);
        let maximum = BigInt::from(u128::MAX);
        assert_eq!(integer_division_big(&maximum, u128::MAX), Some(1.0));
        assert_eq!(integer_division_big(&(-&maximum), u128::MAX), Some(-1.0));
        assert_eq!(
            integer_division_big(&maximum, 1).map(f64::to_bits),
            Some(2.0_f64.powi(128).to_bits())
        );
        assert_eq!(
            integer_division_big(&BigInt::from(1), u128::MAX).map(f64::to_bits),
            Some(2.0_f64.powi(-128).to_bits())
        );
        let raw = BigInt::from(9_007_199_254_740_995_u64);
        assert_eq!(
            integer_division_big(&raw, 1_000_000_000).map(f64::to_bits),
            Some(0x4161_2e0b_e826_d696)
        );
        assert_eq!(
            integer_division_big(&(-raw), 1_000_000_000).map(f64::to_bits),
            Some(0xc161_2e0b_e826_d696)
        );
        // Exactly halfway between consecutive floats at 2^53.
        for (raw, expected) in [
            (9_007_199_254_740_993_u64, 9_007_199_254_740_992_f64),
            (9_007_199_254_740_995, 9_007_199_254_740_996_f64),
        ] {
            assert_eq!(integer_division_big(&BigInt::from(raw), 1), Some(expected));
            assert_eq!(
                integer_division_big(&(-BigInt::from(raw)), 1),
                Some(-expected)
            );
        }
    }

    #[test]
    fn big_integer_division_detects_overflow_after_rounding() {
        let largest = ((BigInt::from(1_u8) << 53_usize) - 1_u8) << 971_usize;
        let threshold = (BigInt::from(1_u8) << 1024_usize) - (BigInt::from(1_u8) << 970_usize);
        for denominator in [1, u128::MAX] {
            let scale = BigInt::from(denominator);
            let below = &threshold * &scale - 1_u8;
            assert_eq!(
                integer_division_big(&(&largest * &scale), denominator),
                Some(f64::MAX)
            );
            assert_eq!(integer_division_big(&below, denominator), Some(f64::MAX));
            assert_eq!(
                integer_division_big(&(-&below), denominator),
                Some(-f64::MAX)
            );
            assert_eq!(
                integer_division_big(&(&threshold * &scale), denominator),
                None
            );
            assert_eq!(
                integer_division_big(&(-&threshold * &scale), denominator),
                None
            );
            assert_eq!(
                integer_division_big(&(&threshold * &scale + 1_u8), denominator),
                None
            );
        }
        assert_eq!(
            integer_division_big(&(BigInt::from(1_u8) << 4096_usize), 1),
            None
        );
    }

    // This reference reconstructs a candidate float as an exact rational and
    // compares distances to its neighbors. It does not use the quotient's
    // exponent selection, scaling or remainder-based rounding implementation.
    fn exact_distance(numerator: &BigUint, denominator: u128, value: f64) -> (BigUint, BigUint) {
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let fraction = bits & ((1_u64 << 52) - 1);
        let (mantissa, shift) = if exponent == 0 {
            (fraction, -1074)
        } else {
            (fraction | (1_u64 << 52), exponent - 1075)
        };
        let mut float_numerator = BigUint::from(mantissa);
        let mut float_denominator = BigUint::from(1_u8);
        if shift < 0 {
            float_denominator <<= (-shift) as usize;
        } else {
            float_numerator <<= shift as usize;
        }
        let left = numerator * &float_denominator;
        let right = float_numerator * BigUint::from(denominator);
        let distance = if left >= right {
            left - right
        } else {
            right - left
        };
        (distance, float_denominator)
    }

    fn assert_nearest(numerator: &BigInt, denominator: u128, result: Option<f64>) {
        let threshold = ((BigUint::from(1_u8) << 1024_usize) - (BigUint::from(1_u8) << 970_usize))
            * BigUint::from(denominator);
        let magnitude = numerator.magnitude();
        if magnitude >= &threshold {
            assert!(result.is_none());
            return;
        }
        let result = result.unwrap_or(f64::NAN);
        assert!(
            result.is_finite(),
            "the exact rational lies below the overflow threshold"
        );
        if numerator.is_zero() {
            assert_eq!(result.to_bits(), 0);
            return;
        }
        assert_eq!(
            result.is_sign_negative(),
            numerator.sign() == num_bigint::Sign::Minus
        );
        let value = result.abs();
        let bits = value.to_bits();
        assert!(bits > 0);
        let (distance, scale) = exact_distance(magnitude, denominator, value);
        for neighbor in [bits - 1, bits + 1] {
            let adjacent = f64::from_bits(neighbor);
            if adjacent.is_finite() {
                let (other_distance, other_scale) =
                    exact_distance(magnitude, denominator, adjacent);
                let own = &distance * other_scale;
                let other = other_distance * &scale;
                assert!(own <= other);
                if own == other {
                    assert_eq!(bits & 1, 0);
                }
            }
        }
    }

    proptest! {
        #[test]
        fn native_u128_division_matches_exact_rational_reference(
            magnitude in any::<u128>(), denominator in 1_u128..=u128::MAX, negative in any::<bool>()
        ) {
            let numerator = BigInt::from(magnitude) * if negative { -1_i8 } else { 1_i8 };
            assert_nearest(&numerator, denominator, integer_division_big(&numerator, denominator));
        }

        #[test]
        fn unbounded_integer_division_matches_exact_rational_reference(
            magnitude in any::<u128>(), denominator in 1_u128..=u128::MAX,
            shift in 0_usize..1200, negative in any::<bool>()
        ) {
            let numerator = (BigInt::from(magnitude) << shift) * if negative { -1_i8 } else { 1_i8 };
            assert_nearest(&numerator, denominator, integer_division_big(&numerator, denominator));
        }

        #[test]
        fn existing_native_division_matches_exact_rational_reference(
            numerator in any::<i128>(), denominator in 1_u64..=u64::MAX
        ) {
            assert_nearest(&BigInt::from(numerator), u128::from(denominator), integer_division(numerator, denominator));
        }
    }
}
