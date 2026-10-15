use std::{fmt, str::FromStr};

use parity_scale_codec::{CompactAs, Decode, Encode};
use scale_decode::{IntoVisitor, TypeResolver};
use scale_info::TypeInfo;

use crate::{
    IntoCoreValue, UnitError,
    decoding::{FromWire, WireVisitor},
};

macro_rules! fraction {
    ($name:ident, $wire:ty, $denominator:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Copy,
            Clone,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Encode,
            Decode,
            CompactAs,
            TypeInfo,
        )]
        #[cfg_attr(
            feature = "serde",
            derive(serde::Serialize, serde::Deserialize),
            serde(transparent)
        )]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        pub struct $name(#[doc = "The raw numerator."] pub $wire);
        impl FromWire for $name {
            type Wire = $wire;
            fn from_wire(value: $wire) -> Result<Self, scale_decode::Error> {
                Ok(Self(value))
            }
        }
        impl IntoVisitor for $name {
            type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
            fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
                WireVisitor::new()
            }
        }
        impl $name {
            /// Denominator of the wire fraction.
            pub const DENOMINATOR: $wire = $denominator;
            /// Python-compatible conversion for display and read math.
            pub fn to_f64(self) -> f64 {
                self.0 as f64 / Self::DENOMINATOR as f64
            }
        }
        impl IntoCoreValue for $name {
            fn into_core_value(self) -> rtao_core::codec::Value {
                self.0.into_core_value()
            }
        }
    };
}
fraction!(
    PerU16,
    u16,
    u16::MAX,
    "A fraction of 65535 used for subnet weights and proportions."
);
fraction!(
    Permill,
    u32,
    1_000_000,
    "A parts-per-million wire fraction."
);
fraction!(
    Perbill,
    u32,
    1_000_000_000,
    "A parts-per-billion wire fraction."
);
fraction!(Percent, u8, 100, "A parts-per-hundred wire fraction.");
fraction!(
    PerMillion,
    u64,
    1_000_000,
    "A u64 parts-per-million value used by moving averages."
);
fraction!(
    U64Normalized,
    u64,
    u64::MAX,
    "A fraction normalized by the maximum u64 value."
);

fn exact_decimal(bits: u128, fractional_bits: u32, negative: bool) -> String {
    use num_bigint::BigUint;
    let whole = bits >> fractional_bits;
    let mask = (1u128 << fractional_bits) - 1;
    let fraction = bits & mask;
    let sign = if negative && bits != 0 { "-" } else { "" };
    if fraction == 0 {
        return format!("{sign}{whole}");
    }
    let fraction_decimal =
        (BigUint::from(fraction) * BigUint::from(5u8).pow(fractional_bits)).to_string();
    let width = fractional_bits as usize;
    let zeros = "0".repeat(width.saturating_sub(fraction_decimal.len()));
    let fractional = format!("{zeros}{fraction_decimal}");
    format!("{sign}{whole}.{}", fractional.trim_end_matches('0'))
}

macro_rules! fixed {
    ($name:ident, $wire:ty, $frac:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Copy,
            Clone,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Encode,
            Decode,
            TypeInfo,
            scale_decode::DecodeAsType,
        )]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        pub struct $name {
            /// Exact runtime fixed-point bits.
            pub bits: $wire,
        }
        impl $name {
            /// Number of fractional bits in the wire representation.
            pub const FRACTIONAL_BITS: u32 = $frac;
            /// Construct an exact value from runtime bits.
            pub const fn from_bits(bits: $wire) -> Self {
                Self { bits }
            }
            /// Python's integer-to-float conversion followed by power-of-two division.
            pub fn to_f64(self) -> f64 {
                self.bits as f64 / (2.0f64).powi($frac)
            }
            /// Integer component rounded toward negative infinity.
            pub const fn int_part(self) -> $wire {
                self.bits >> $frac
            }
        }
        impl IntoCoreValue for $name {
            fn into_core_value(self) -> rtao_core::codec::Value {
                rtao_core::codec::Value::record(vec![("bits".into(), self.bits.into_core_value())])
            }
        }
    };
}

fixed!(
    U64F64,
    u128,
    64,
    "An unsigned fixed-point value with 64 fractional bits."
);
fixed!(
    U96F32,
    u128,
    32,
    "An unsigned 128-bit fixed-point value with 32 fractional bits."
);
fixed!(
    I96F32,
    i128,
    32,
    "A signed fixed-point value with 32 fractional bits."
);
fixed!(
    I64F64,
    i128,
    64,
    "A signed fixed-point value with 64 fractional bits."
);
fixed!(
    I32F32,
    i64,
    32,
    "A signed 64-bit fixed-point value with 32 fractional bits."
);

impl fmt::Display for U64F64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&exact_decimal(self.bits, 64, false))
    }
}

impl fmt::Display for U96F32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&exact_decimal(self.bits, 32, false))
    }
}

macro_rules! signed_display {
    ($($name:ident),+) => {$ (
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&exact_decimal(self.bits.unsigned_abs() as u128, Self::FRACTIONAL_BITS, self.bits < 0))
            }
        }
    )+};
}
signed_display!(I96F32, I64F64, I32F32);

/// A finite ratio retaining Python float semantics for intent calculations.
///
/// Text input accepts Python's `float(text)` syntax and rejects non-finite values.
#[derive(Copy, Clone, Debug, PartialEq, PartialOrd)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Ratio(f64);

impl Ratio {
    /// Reject non-finite ratios while retaining the supplied f64 exactly.
    pub fn new(value: f64) -> Result<Self, UnitError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(UnitError::InvalidDecimal(value.to_string()))
        }
    }
    /// Borrow the finite value for Python-compatible intent math.
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl FromStr for Ratio {
    type Err = UnitError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::pyfmt::parse_float(value)
            .ok_or_else(|| UnitError::InvalidDecimal(value.into()))
            .and_then(Self::new)
    }
}

impl fmt::Display for Ratio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::pyfmt::float_repr(self.0))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Ratio {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Ratio {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Number(number) => number.as_str().parse().map_err(D::Error::custom),
            serde_json::Value::String(text) => text.parse().map_err(D::Error::custom),
            _ => Err(D::Error::custom(
                "ratio must be a finite number or numeric string",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_text_preserves_values_and_requires_finite_numbers() {
        for (text, expected) in [
            (" 1.0 ", 1.0_f64),
            ("1_2.5", 12.5),
            ("\u{a0}+١_٢.٥e١\u{2003}", 125.0),
            ("-０.０", -0.0),
        ] {
            assert_eq!(
                text.parse::<Ratio>().map(|ratio| ratio.get().to_bits()),
                Ok(expected.to_bits()),
                "{text:?}"
            );
        }
        for text in [
            "nan",
            "-NaN",
            "+Infinity",
            "-inf",
            "1e9999",
            "1__2",
            "\u{1c}1",
        ] {
            assert!(text.parse::<Ratio>().is_err(), "{text:?}");
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn ratio_json_strings_use_python_float_coercion() -> Result<(), serde_json::Error> {
        for (text, expected) in [(" 1.0 ", 1.0_f64), ("1_2.5", 12.5), ("١٢.٥", 12.5)] {
            let ratio: Ratio = serde_json::from_value(serde_json::Value::String(text.into()))?;
            assert_eq!(ratio.get().to_bits(), expected.to_bits(), "{text:?}");
        }
        for text in ["nan", "infinity", "1__2", "\u{1f}1"] {
            assert!(
                serde_json::from_value::<Ratio>(serde_json::Value::String(text.into())).is_err(),
                "{text:?}"
            );
        }
        Ok(())
    }
}
