// Derived from RaoFoundation/subtensor sdk/python/bittensor/hyperparams.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Data-driven hyperparameter units and native codec conversion.

use std::{cmp::Ordering, fmt};

use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive};

use super::ProportionInput;

/// A checked native protocol integer, including unsigned fixed128 settings.
///
/// TAO codecs remain unsigned 64-bit. This carrier also represents signed
/// settings and unsigned 128-bit fixed-point bits without widening balances.
#[derive(Clone, Copy, Debug)]
pub enum RawInteger {
    /// Unsigned protocol value.
    Unsigned(u128),
    /// Signed protocol value.
    Signed(i128),
}

impl RawInteger {
    /// Return the value when it fits the unsigned 64-bit wire range.
    pub fn as_u64(self) -> Option<u64> {
        self.as_u128().and_then(|value| value.try_into().ok())
    }

    /// Return the value when it fits the unsigned 128-bit wire range.
    pub fn as_u128(self) -> Option<u128> {
        match self {
            Self::Unsigned(value) => Some(value),
            Self::Signed(value) => value.try_into().ok(),
        }
    }

    /// Return the value when it fits the signed 128-bit wire range.
    pub fn as_i128(self) -> Option<i128> {
        match self {
            Self::Unsigned(value) => value.try_into().ok(),
            Self::Signed(value) => Some(value),
        }
    }
}

impl fmt::Display for RawInteger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsigned(value) => value.fmt(formatter),
            Self::Signed(value) => value.fmt(formatter),
        }
    }
}

impl Ord for RawInteger {
    fn cmp(&self, other: &Self) -> Ordering {
        match (*self, *other) {
            (Self::Signed(a), Self::Signed(b)) => a.cmp(&b),
            (Self::Unsigned(a), Self::Unsigned(b)) => a.cmp(&b),
            (Self::Signed(a), Self::Unsigned(b)) => {
                if a < 0 {
                    Ordering::Less
                } else {
                    a.unsigned_abs().cmp(&b)
                }
            }
            (Self::Unsigned(a), Self::Signed(b)) => {
                if b < 0 {
                    Ordering::Greater
                } else {
                    a.cmp(&b.unsigned_abs())
                }
            }
        }
    }
}

impl PartialOrd for RawInteger {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for RawInteger {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for RawInteger {}

macro_rules! native_integer {
    ($variant:ident; $($type:ty),+ $(,)?) => {$(
        impl From<$type> for RawInteger {
            fn from(value: $type) -> Self { Self::$variant(value.into()) }
        }
    )+};
}
native_integer!(Unsigned; u8, u16, u32, u64, u128);
native_integer!(Signed; i8, i16, i32, i64, i128);

impl From<RawInteger> for BigInt {
    fn from(value: RawInteger) -> Self {
        match value {
            RawInteger::Unsigned(value) => value.into(),
            RawInteger::Signed(value) => value.into(),
        }
    }
}
impl From<RawInteger> for ProportionInput<'_> {
    fn from(value: RawInteger) -> Self {
        Self::Raw(value.into())
    }
}

/// Hand-authored semantics extracted from one Python `Hyperparam` constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hyperparam {
    /// Unit kind when metadata supplies no unit-bearing identity.
    pub kind: &'static str,
    /// Full upstream description.
    pub doc: &'static str,
    /// Listing description, bounded in Unicode codepoints by the units gate.
    pub short: &'static str,
    /// Signedness override, used only when `bits` is present.
    pub signed: Option<bool>,
    /// Explicit codec width override.
    pub bits: Option<u8>,
    /// Optional inclusive semantic lower bound.
    pub minimum: Option<RawInteger>,
    /// Optional inclusive semantic upper bound.
    pub maximum: Option<RawInteger>,
}

/// Authenticated source and pinned-metadata identities for one storage value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HyperparamStorage {
    /// Hyperparameter name.
    pub parameter: &'static str,
    /// Runtime pallet/container name.
    pub container: &'static str,
    /// Runtime storage item name.
    pub name: &'static str,
    /// Identity written in Python's generated or explicit descriptor.
    pub source_ident: &'static str,
    /// Value identity authenticated against the pinned runtime metadata.
    pub metadata_ident: &'static str,
}

/// Generated semantics, identities, codecs and conversion constants.
#[derive(Clone, Copy, Debug)]
pub struct HyperparamTable {
    /// All hand-authored hyperparameter descriptions and semantic bounds.
    pub entries: &'static [(&'static str, Hyperparam)],
    /// Dedicated storage values with source and metadata identities.
    pub storage: &'static [HyperparamStorage],
    /// Unit-bearing metadata identities and their unit kinds.
    pub ident_kinds: &'static [(&'static str, &'static str)],
    /// Identity codec widths as `(signed, bits)`.
    pub ident_codecs: &'static [(&'static str, (bool, u8))],
    /// Fallback unit codec widths as `(signed, bits)`.
    pub kind_codecs: &'static [(&'static str, (bool, u8))],
    /// Fraction-kind raw denominators.
    pub fraction_denominators: &'static [(&'static str, u64)],
    /// Rough seconds per block, used with Python's float truncation.
    pub blocktime: f64,
    /// Native rao per TAO.
    pub rao_per_tao: u64,
    /// TAO display symbol.
    pub tao_symbol: &'static str,
    /// Fixed128 raw value encoding one.
    pub fixed128_one: u128,
    /// Listing description ceiling in Unicode codepoints.
    pub short_max: usize,
}

/// Invalid hyperparameter input with Python-compatible diagnostic text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HyperparamError(String);
impl HyperparamError {
    /// Python-compatible diagnostic text.
    pub fn message(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for HyperparamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for HyperparamError {}

impl HyperparamTable {
    fn meta(&self, name: &str) -> Option<&Hyperparam> {
        self.entries
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, meta)| meta)
    }
    fn item(&self, name: &str) -> Option<&HyperparamStorage> {
        self.storage.iter().find(|item| item.parameter == name)
    }
    fn denominator(&self, name: &str) -> Option<u64> {
        let kind = self.kind_of(name);
        self.fraction_denominators
            .iter()
            .find(|(key, _)| *key == kind)
            .map(|(_, d)| *d)
    }

    // py: sdk/python/bittensor/hyperparams.py:474-485 (metadata_kind)
    /// Return the unit kind dictated by the pinned storage value identity.
    pub fn metadata_kind(&self, name: &str) -> Option<&'static str> {
        let item = self.item(name)?;
        self.ident_kinds
            .iter()
            .find(|(ident, _)| *ident == item.metadata_ident)
            .map(|(_, kind)| *kind)
    }

    // py: sdk/python/bittensor/hyperparams.py:488-500 (kind_of)
    /// Resolve metadata first, then hand semantics, then the `int` fallback.
    pub fn kind_of(&self, name: &str) -> &'static str {
        self.metadata_kind(name)
            .or_else(|| self.meta(name).map(|meta| meta.kind))
            .unwrap_or("int")
    }

    // py: sdk/python/bittensor/hyperparams.py:503-505 (doc_of)
    /// Return the full description, or `None` for an unknown name.
    pub fn doc_of(&self, name: &str) -> Option<&'static str> {
        self.meta(name).map(|meta| meta.doc)
    }

    // py: sdk/python/bittensor/hyperparams.py:508-511 (short_of)
    /// Return a nonempty listing description, or `None`.
    pub fn short_of(&self, name: &str) -> Option<&'static str> {
        self.meta(name)
            .map(|meta| meta.short)
            .filter(|short| !short.is_empty())
    }

    // py: sdk/python/bittensor/hyperparams.py:514-519 (normalized)
    /// Normalize a numeric raw fraction without clamping.
    ///
    /// Integers divide exactly before rounding once. Boolean and float inputs
    /// retain Python's numeric division behavior. Text and integer division
    /// overflow yield `None`; native signed/unsigned raw integers fit.
    pub fn normalized<'a>(&self, name: &str, raw: impl Into<ProportionInput<'a>>) -> Option<f64> {
        let denominator = self.denominator(name)?;
        match raw.into() {
            ProportionInput::Raw(raw) => {
                crate::ratio::integer_division_big(&raw, denominator.into())
            }
            ProportionInput::Boolean(raw) => {
                crate::ratio::integer_division_big(&BigInt::from(u8::from(raw)), denominator.into())
            }
            ProportionInput::Fraction(raw) => Some(raw / denominator as f64),
            ProportionInput::Text(_) => None,
        }
    }

    fn duration(&self, raw: &BigInt) -> Option<String> {
        let mut seconds = BigInt::from_f64((raw.to_f64()? * self.blocktime).trunc())?;
        if seconds < BigInt::from(60) {
            return None;
        }
        let mut parts = Vec::new();
        for (suffix, size) in [("d", 86_400), ("h", 3_600), ("m", 60)] {
            if seconds >= BigInt::from(size) {
                parts.push(format!("{}{suffix}", &seconds / size));
                seconds %= size;
            } else if !parts.is_empty() {
                break;
            }
        }
        parts.truncate(2);
        Some(parts.join(" "))
    }

    // py: sdk/python/bittensor/hyperparams.py:540-557 (annotate)
    /// Render a Python-style human reading for raw integer input.
    ///
    /// Rejects booleans and nonintegers. Raw values are never clamped.
    pub fn annotate<'a>(&self, name: &str, raw: impl Into<ProportionInput<'a>>) -> Option<String> {
        let ProportionInput::Raw(raw) = raw.into() else {
            return None;
        };
        let kind = self.kind_of(name);
        if let Some(denominator) = self.denominator(name) {
            return Some(format!(
                "≈ {}",
                crate::pyfmt::fmt_g4(crate::ratio::integer_division_big(
                    &raw,
                    denominator.into()
                )?)
            ));
        }
        match kind {
            "rao" => Some(format!(
                "= {}{}",
                self.tao_symbol,
                crate::pyfmt::fmt_g(crate::ratio::integer_division_big(
                    &raw,
                    self.rao_per_tao.into()
                )?)
            )),
            "blocks" => self.duration(&raw).map(|duration| format!("≈ {duration}")),
            "difficulty" if raw == BigInt::from(u64::MAX) => Some("= u64::MAX".into()),
            "fixed128" => Some(format!(
                "≈ {}",
                crate::pyfmt::fmt_g4(crate::ratio::integer_division_big(&raw, self.fixed128_one)?)
            )),
            _ => None,
        }
    }

    // py: sdk/python/bittensor/hyperparams.py:561-578 (value_forms)
    /// Describe the accepted raw and human input forms.
    pub fn value_forms(&self, name: &str) -> String {
        if let Some(denominator) = self.denominator(name) {
            return format!("a 0..1 fraction (e.g. 0.5) or the raw integer (0..{denominator})");
        }
        let (lo, hi) = self.raw_bounds(name);
        match self.kind_of(name) {
            "rao" => {
                "a non-negative TAO amount with a decimal point (e.g. 0.7) or the raw rao integer"
                    .into()
            }
            "fixed128"
                if lo == self.fixed128_one.into()
                    && self
                        .fixed128_one
                        .checked_mul(3)
                        .is_some_and(|three| hi == three.into()) =>
            {
                "a multiplier with a decimal point in 1..3 (e.g. 1.5) or the raw U64F64 bits".into()
            }
            "fixed128" => {
                "a non-negative multiplier with a decimal point (e.g. 1.5) or the raw U64F64 bits"
                    .into()
            }
            "bool" => "true/false (or 1/0)".into(),
            "blocks" => format!("a block count in {lo}..{hi} (12s blocks)"),
            "epochs" => "an epoch (tempo) count".into(),
            _ => format!("an integer in {lo}..{hi}"),
        }
    }

    // py: sdk/python/bittensor/hyperparams.py:663-680 (codec_of)
    /// Resolve explicit width/signedness, then metadata, then the kind codec.
    ///
    /// `signed` alone is not an override; explicit width defaults to unsigned.
    pub fn codec_of(&self, name: &str) -> (bool, u8) {
        if let Some(meta) = self.meta(name)
            && let Some(bits) = meta.bits
        {
            return (meta.signed.unwrap_or(false), bits);
        }
        if let Some(item) = self.item(name)
            && let Some((_, codec)) = self
                .ident_codecs
                .iter()
                .find(|(ident, _)| *ident == item.metadata_ident)
        {
            return *codec;
        }
        self.kind_codecs
            .iter()
            .find(|(kind, _)| *kind == self.kind_of(name))
            .map(|(_, codec)| *codec)
            .unwrap_or((false, 64))
    }

    // py: sdk/python/bittensor/hyperparams.py:683-700 (raw_bounds)
    /// Intersect native codec, semantic and fraction bounds without clamping input.
    ///
    /// Generated widths must be at most 128 bits. Widths zero and one use 0..1.
    pub fn raw_bounds(&self, name: &str) -> (RawInteger, RawInteger) {
        let (signed, bits) = self.codec_of(name);
        let (mut lo, mut hi) = if bits <= 1 {
            (RawInteger::Unsigned(0), RawInteger::Unsigned(1))
        } else if signed {
            let high = (u128::MAX >> (129 - u32::from(bits))) as i128;
            (RawInteger::Signed(-high - 1), RawInteger::Signed(high))
        } else {
            (
                RawInteger::Unsigned(0),
                RawInteger::Unsigned(u128::MAX >> (128 - u32::from(bits))),
            )
        };
        if let Some(meta) = self.meta(name) {
            if let Some(minimum) = meta.minimum {
                lo = lo.max(minimum)
            }
            if let Some(maximum) = meta.maximum {
                hi = hi.min(maximum)
            }
        }
        if let Some(denominator) = self.denominator(name) {
            lo = lo.max(0u8.into());
            hi = hi.min(denominator.into());
        }
        (lo, hi)
    }

    fn check_raw(&self, name: &str, raw: BigInt) -> Result<RawInteger, HyperparamError> {
        let (lo, hi) = self.raw_bounds(name);
        if raw < BigInt::from(lo) || raw > BigInt::from(hi) {
            return Err(HyperparamError(format!(
                "{name} must be within {lo}..{hi} (raw on-chain range); got {raw}"
            )));
        }
        if self.codec_of(name).0 {
            raw.to_i128().map(RawInteger::Signed)
        } else {
            raw.to_u128().map(RawInteger::Unsigned)
        }
        .ok_or_else(|| HyperparamError(format!("{name} value is out of range")))
    }

    // py: sdk/python/bittensor/hyperparams.py:715-785 (to_raw)
    /// Convert raw integers or binary64 human inputs to checked native values.
    ///
    /// Plain integer text is raw; text containing `.` or `e` uses the human
    /// form. Fractions, TAO amounts and multipliers round half to even after
    /// binary64 multiplication. Boolean parameters accept only bool, integer
    /// zero/one, or text true/false/0/1. This keeps TAO conversion within u64.
    pub fn to_raw<'a>(
        &self,
        name: &str,
        value: impl Into<ProportionInput<'a>>,
    ) -> Result<RawInteger, HyperparamError> {
        let kind = self.kind_of(name);
        let invalid = |repr: &str| {
            HyperparamError(format!(
                "{name} takes {}; got {repr}",
                self.value_forms(name)
            ))
        };
        let value = match value.into() {
            ProportionInput::Boolean(value) => {
                return if kind == "bool" {
                    Ok(RawInteger::Unsigned(u128::from(value)))
                } else {
                    Err(invalid("a boolean"))
                };
            }
            ProportionInput::Text(original) => {
                let lowered = original
                    .trim_matches(crate::python::whitespace)
                    .to_lowercase();
                let text = lowered.strip_prefix('+').unwrap_or(&lowered);
                if kind == "bool" {
                    return match text {
                        "true" | "1" => Ok(RawInteger::Unsigned(1)),
                        "false" | "0" => Ok(RawInteger::Unsigned(0)),
                        _ => Err(invalid(&crate::pyfmt::string_repr(&original))),
                    };
                }
                let parsed = if text.contains(['.', 'e']) {
                    crate::python::float(text).map(ProportionInput::Fraction)
                } else {
                    crate::python::integer(text).map(ProportionInput::Raw)
                };
                parsed.ok_or_else(|| invalid(&crate::pyfmt::string_repr(&original)))?
            }
            input => input,
        };
        if kind == "bool" {
            return match value {
                ProportionInput::Raw(value)
                    if value == BigInt::from(0) || value == BigInt::from(1) =>
                {
                    Ok(RawInteger::Unsigned(u128::from(value == BigInt::from(1))))
                }
                ProportionInput::Raw(value) => Err(invalid(&value.to_string())),
                ProportionInput::Fraction(value) => Err(invalid(&crate::pyfmt::float_repr(value))),
                _ => unreachable!("bool and text normalized above"),
            };
        }
        let ProportionInput::Fraction(value) = value else {
            let ProportionInput::Raw(raw) = value else {
                unreachable!("bool and text normalized above")
            };
            return self.check_raw(name, raw);
        };
        let scaled = if let Some(denominator) = self.denominator(name) {
            if !(0.0..=1.0).contains(&value) {
                return Err(HyperparamError(format!(
                    "{name} is a normalized fraction: pass 0..1 (e.g. 0.5), or the raw integer 0..{denominator} without a decimal point"
                )));
            }
            value * denominator as f64
        } else if kind == "rao" || kind == "fixed128" {
            if value < 0.0 {
                return Err(HyperparamError(format!("{name} cannot be negative")));
            }
            value
                * if kind == "rao" {
                    self.rao_per_tao as f64
                } else {
                    self.fixed128_one as f64
                }
        } else if value.is_finite() && value.fract() == 0.0 {
            value
        } else {
            return Err(invalid(&crate::pyfmt::float_repr(value)));
        };
        if scaled.is_nan() {
            return Err(HyperparamError(
                "cannot convert float NaN to integer".into(),
            ));
        }
        let raw = BigInt::from_f64(scaled.round_ties_even())
            .ok_or_else(|| HyperparamError(format!("{name} value is out of range")))?;
        self.check_raw(name, raw)
    }
}
