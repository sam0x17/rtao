//! Exact JSON input and comparisons for Python oracle values.
//!
//! The workspace enables `serde_json/arbitrary_precision` for full-width chain
//! integers. Parsing a [`serde_json::Value`] before deserializing and adapting Serde's
//! content buffering also support untagged and flattened floats, including
//! non-canonical exponent spellings. Ordinary number input remains digit-exact.

use std::{error::Error, fmt};

use serde::{
    Deserializer,
    de::{
        DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
    },
};
use serde_json::{Number, Value};

/// Deserialize JSON text through an intermediate [`Value`].
///
/// Full-width number tokens remain exact, and Serde's untagged and flattened
/// float fields see ordinary numeric values rather than private number maps.
/// Exact number-or-string input types retain the original number text.
/// Serde's buffered untagged variants and flattened fields necessarily store
/// non-integers as `f64`; keep exact decimal domain values in ordinary fields.
/// Trailing non-whitespace input is rejected by the initial parse.
pub fn from_str<T: DeserializeOwned>(input: &str) -> serde_json::Result<T> {
    T::deserialize(ExactValueDeserializer(serde_json::from_str::<Value>(
        input,
    )?))
}

/// Deserialize JSON bytes through an intermediate [`Value`].
///
/// This has the same numeric and untagged/flattened behavior as [`from_str`].
pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> serde_json::Result<T> {
    T::deserialize(ExactValueDeserializer(serde_json::from_slice::<Value>(
        input,
    )?))
}

// Serde buffers untagged and flattened input with its ContentVisitor. Under
// arbitrary_precision, Number::deserialize_any supplies a private map for
// non-canonical float spellings (including 1e-05). ContentVisitor interprets
// that map as an object, so a simple serde_json::from_value still fails.
//
// Adapt only Serde's content-buffering hook. Ordinary deserialize_any retains
// serde_json's exact Number representation, which Money/Amount/Ratio obtain
// through Value::deserialize and Number::as_str. Typed float requests use the
// original deserializer's numeric conversion. Owned accessors recursively
// apply the hook to nested arrays, objects, options and enum payloads. Serde
// content has no arbitrary-precision decimal scalar, so this buffering boundary
// cannot preserve exact decimals for custom types within untagged/flatten data.
// serde_json's Value deserializer can convert an integer wider than u128 into
// f64 when its decimal token happens to equal f64::to_string (e.g. 10^40).
// Keep owned values available all the way through tree traversal so the number
// token reaches Value/Number visitors without that lossy fallback.
struct ExactValueDeserializer(Value);
struct ExactSeqAccess(std::vec::IntoIter<Value>);
struct ExactMapAccess {
    entries: <serde_json::Map<String, Value> as IntoIterator>::IntoIter,
    value: Option<Value>,
}
struct ExactEnumAccess {
    variant: String,
    value: Option<Value>,
}
struct ExactVariantAccess(Option<Value>);
struct KeySeedVisitor<S>(S);

macro_rules! exact_delegate {
    ($($method:ident $(($($argument:ident: $argument_type:ty),*))?;)*) => {
        $(fn $method<V: Visitor<'de>>(self, $($($argument: $argument_type,)*)? visitor: V)
            -> Result<V::Value, Self::Error> {
            self.0.$method($($($argument,)*)? visitor)
        })*
    };
}

impl<'de> Deserializer<'de> for ExactValueDeserializer {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Array(values) => visit_exact_array(values, visitor),
            Value::Object(values) => visit_exact_map(values, visitor),
            Value::Number(number) => {
                if is_integer(&number) && number.as_str() != "-0" {
                    if let Some(value) = number.as_u64() {
                        return visitor.visit_u64(value);
                    }
                    if let Some(value) = number.as_i64() {
                        return visitor.visit_i64(value);
                    }
                } else if let Some(value) = number.as_f64()
                    && let Some(canonical) = Number::from_f64(value)
                    && canonical.as_str() == number.as_str()
                {
                    return visitor.visit_f64(value);
                }
                exact_number(number, visitor)
            }
            value => value.deserialize_any(visitor),
        }
    }

    fn __deserialize_content_v1<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        use serde::de::Error;
        if let Value::Number(number) = &self.0 {
            if !is_integer(number) {
                return match number.as_f64() {
                    Some(number) => visitor.visit_f64(number),
                    None => Err(Self::Error::custom(
                        "non-integer token is outside finite f64 range",
                    )),
                };
            }
            if let Some(number) = number.as_u64() {
                return visitor.visit_u64(number);
            }
            if let Some(number) = number.as_i64() {
                return visitor.visit_i64(number);
            }
        }
        self.deserialize_any(visitor)
    }

    exact_delegate! {
        deserialize_bool;
        deserialize_i8; deserialize_i16; deserialize_i32; deserialize_i64; deserialize_i128;
        deserialize_u8; deserialize_u16; deserialize_u32; deserialize_u64; deserialize_u128;
        deserialize_char; deserialize_str; deserialize_string;
        deserialize_unit; deserialize_unit_struct(name: &'static str);
        deserialize_identifier; deserialize_ignored_any;
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        use serde::de::Error;
        if let Value::Number(number) = &self.0
            && !number.as_str().parse::<f32>().is_ok_and(f32::is_finite)
        {
            return Err(Self::Error::custom("number is outside finite f32 range"));
        }
        self.0.deserialize_f32(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        use serde::de::Error;
        if let Value::Number(number) = &self.0
            && number.as_f64().is_none()
        {
            return Err(Self::Error::custom("number is outside finite f64 range"));
        }
        self.0.deserialize_f64(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Array(values) => visit_exact_array(values, visitor),
            value => value.deserialize_seq(visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }
    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_byte_buf(visitor)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Array(values) => visit_exact_array(values, visitor),
            value => value.deserialize_byte_buf(visitor),
        }
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Object(values) => visit_exact_map(values, visitor),
            value => value.deserialize_map(visitor),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Array(values) => visit_exact_array(values, visitor),
            Value::Object(values) => visit_exact_map(values, visitor),
            value => value.deserialize_struct(name, fields, visitor),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::String(variant) => visitor.visit_enum(ExactEnumAccess {
                variant,
                value: None,
            }),
            Value::Object(values) if values.len() == 1 => {
                let mut values = values.into_iter();
                match values.next() {
                    Some((variant, value)) => visitor.visit_enum(ExactEnumAccess {
                        variant,
                        value: Some(value),
                    }),
                    None => Err(serde::de::Error::custom("enum variant is missing")),
                }
            }
            value => value.deserialize_enum(name, variants, visitor),
        }
    }
}

fn visit_exact_array<'de, V: Visitor<'de>>(
    values: Vec<Value>,
    visitor: V,
) -> Result<V::Value, serde_json::Error> {
    use serde::de::Error;
    let len = values.len();
    let mut access = ExactSeqAccess(values.into_iter());
    let result = visitor.visit_seq(&mut access)?;
    if access.0.len() != 0 {
        return Err(serde_json::Error::invalid_length(
            len,
            &"fewer elements in array",
        ));
    }
    Ok(result)
}

fn visit_exact_map<'de, V: Visitor<'de>>(
    values: serde_json::Map<String, Value>,
    visitor: V,
) -> Result<V::Value, serde_json::Error> {
    use serde::de::Error;
    let len = values.len();
    let mut access = ExactMapAccess {
        entries: values.into_iter(),
        value: None,
    };
    let result = visitor.visit_map(&mut access)?;
    if access.entries.len() != 0 {
        return Err(serde_json::Error::invalid_length(
            len,
            &"fewer elements in map",
        ));
    }
    Ok(result)
}

fn exact_number<'de, V: Visitor<'de>>(
    number: Number,
    visitor: V,
) -> Result<V::Value, serde_json::Error> {
    let number = std::iter::once((
        "$serde_json::private::Number".to_owned(),
        number.as_str().to_owned(),
    ));
    visitor.visit_map(serde::de::value::MapDeserializer::new(number))
}

impl<'de> SeqAccess<'de> for ExactSeqAccess {
    type Error = serde_json::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.0
            .next()
            .map(|value| seed.deserialize(ExactValueDeserializer(value)))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

impl<'de> MapAccess<'de> for ExactMapAccess {
    type Error = serde_json::Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        match self.entries.next() {
            Some((key, value)) => {
                self.value = Some(value);
                // Reuse serde_json's map-key deserializer, which accepts integer
                // and enum keys as well as strings. Only the null placeholder is
                // traversed here; the owned field value stays exact below.
                let mut key_map = serde_json::Map::new();
                key_map.insert(key, Value::Null);
                key_map.deserialize_any(KeySeedVisitor(seed)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        use serde::de::Error;
        match self.value.take() {
            Some(value) => seed.deserialize(ExactValueDeserializer(value)),
            None => Err(Self::Error::custom("map value is missing")),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

impl<'de, S: DeserializeSeed<'de>> Visitor<'de> for KeySeedVisitor<S> {
    type Value = S::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a map key")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        use serde::de::Error;
        let key = access
            .next_key_seed(self.0)?
            .ok_or_else(|| A::Error::custom("map key is missing"))?;
        let _: serde::de::IgnoredAny = access.next_value()?;
        Ok(key)
    }
}

impl<'de> EnumAccess<'de> for ExactEnumAccess {
    type Error = serde_json::Error;
    type Variant = ExactVariantAccess;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(ExactValueDeserializer(Value::String(self.variant)))?;
        Ok((variant, ExactVariantAccess(self.value)))
    }
}

impl<'de> VariantAccess<'de> for ExactVariantAccess {
    type Error = serde_json::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.0 {
            Some(value) => serde::Deserialize::deserialize(ExactValueDeserializer(value)),
            None => Ok(()),
        }
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        use serde::de::Error;
        let value = self
            .0
            .ok_or_else(|| Self::Error::custom("newtype variant value is missing"))?;
        seed.deserialize(ExactValueDeserializer(value))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        use serde::de::Error;
        let value = self
            .0
            .ok_or_else(|| Self::Error::custom("tuple variant value is missing"))?;
        ExactValueDeserializer(value).deserialize_tuple(len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        use serde::de::Error;
        let value = self
            .0
            .ok_or_else(|| Self::Error::custom("struct variant value is missing"))?;
        ExactValueDeserializer(value).deserialize_struct("", fields, visitor)
    }
}

/// Optional tolerances for comparisons between two non-integer JSON tokens.
///
/// Exact comparisons use zero tolerances. Both values must be finite and
/// nonnegative; [`compare_with_tolerance`] rejects invalid configurations.
/// Integer tokens always compare exactly, including values beyond `u128`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FloatTolerance {
    /// Maximum absolute difference between two floats.
    pub absolute: f64,
    /// Maximum difference relative to the larger float's absolute magnitude.
    pub relative: f64,
}

/// The first difference found while walking two JSON trees.
#[derive(Debug, Clone)]
pub struct JsonMismatch {
    /// RFC 6901 JSON pointer to the differing value; empty for the root.
    pub path: String,
    /// Expected value, or `None` when the key or array element is absent.
    pub expected: Option<Box<Value>>,
    /// Actual value, or `None` when the key or array element is absent.
    pub actual: Option<Box<Value>>,
    /// Explanation of the difference.
    pub reason: &'static str,
}

impl fmt::Display for JsonMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = if self.path.is_empty() {
            "<root>"
        } else {
            &self.path
        };
        write!(formatter, "JSON mismatch at {path}: {}", self.reason)
    }
}

impl Error for JsonMismatch {}

/// Compare oracle JSON values using exact token-kind numeric rules.
///
/// Integer tokens (no `.`, `e` or `E`) compare as `i128`/`u128`, with canonical
/// decimal comparison beyond those ranges. Non-integer tokens compare as finite
/// `f64`, so spellings such as `1e-05` and `1e-5` agree. An integer and a float
/// always differ. Object order is ignored; array order and string bytes matter.
/// No number comparison uses [`Value`]'s spelling-sensitive equality.
pub fn compare(expected: &Value, actual: &Value) -> Result<(), JsonMismatch> {
    compare_with_tolerance(expected, actual, FloatTolerance::default())
}

/// Compare JSON trees, optionally allowing differences between two floats.
///
/// Float pairs agree when they are equal, their absolute difference is at most
/// `tolerance.absolute`, or their difference divided by the larger absolute
/// magnitude is at most `tolerance.relative`. Integers remain exact and never
/// compare equal to floats, even with large tolerances. A float outside finite
/// `f64` range is a mismatch. Negative or non-finite tolerances are rejected.
pub fn compare_with_tolerance(
    expected: &Value,
    actual: &Value,
    tolerance: FloatTolerance,
) -> Result<(), JsonMismatch> {
    if !tolerance.absolute.is_finite()
        || tolerance.absolute < 0.0
        || !tolerance.relative.is_finite()
        || tolerance.relative < 0.0
    {
        return Err(mismatch(
            "",
            Some(expected),
            Some(actual),
            "float tolerances must be finite and nonnegative",
        ));
    }
    compare_at(expected, actual, tolerance, "")
}

fn compare_at(
    expected: &Value,
    actual: &Value,
    tolerance: FloatTolerance,
    path: &str,
) -> Result<(), JsonMismatch> {
    match (expected, actual) {
        (Value::Null, Value::Null) => Ok(()),
        (Value::Bool(left), Value::Bool(right)) if left == right => Ok(()),
        (Value::String(left), Value::String(right)) if left == right => Ok(()),
        (Value::Number(left), Value::Number(right)) => {
            let reason = match (is_integer(left), is_integer(right)) {
                (true, true) => {
                    if integers_equal(left, right) {
                        return Ok(());
                    }
                    "integer values differ"
                }
                (false, false) => match (left.as_f64(), right.as_f64()) {
                    (Some(left), Some(right)) => {
                        if floats_equal(left, right, tolerance) {
                            return Ok(());
                        }
                        "float values differ"
                    }
                    _ => "non-integer token is outside finite f64 range",
                },
                _ => "integer and non-integer token kinds differ",
            };
            Err(mismatch(path, Some(expected), Some(actual), reason))
        }
        (Value::Array(left), Value::Array(right)) => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                compare_at(
                    left,
                    right,
                    tolerance,
                    &child_path(path, &index.to_string()),
                )?;
            }
            if left.len() != right.len() {
                let index = left.len().min(right.len());
                return Err(mismatch(
                    &child_path(path, &index.to_string()),
                    left.get(index),
                    right.get(index),
                    "array lengths differ",
                ));
            }
            Ok(())
        }
        (Value::Object(left), Value::Object(right)) => {
            for (key, left) in left {
                let path = child_path(path, key);
                match right.get(key) {
                    Some(right) => compare_at(left, right, tolerance, &path)?,
                    None => {
                        return Err(mismatch(&path, Some(left), None, "object key is absent"));
                    }
                }
            }
            for (key, right) in right {
                if !left.contains_key(key) {
                    return Err(mismatch(
                        &child_path(path, key),
                        None,
                        Some(right),
                        "unexpected object key",
                    ));
                }
            }
            Ok(())
        }
        _ => Err(mismatch(
            path,
            Some(expected),
            Some(actual),
            "JSON types or scalar values differ",
        )),
    }
}

fn is_integer(number: &Number) -> bool {
    !number.as_str().contains(['.', 'e', 'E'])
}

fn integers_equal(left: &Number, right: &Number) -> bool {
    if let (Some(left), Some(right)) = (left.as_i128(), right.as_i128()) {
        return left == right;
    }
    if let (Some(left), Some(right)) = (left.as_u128(), right.as_u128()) {
        return left == right;
    }
    canonical_integer(left.as_str()) == canonical_integer(right.as_str())
}

fn canonical_integer(token: &str) -> (bool, &str) {
    let (negative, digits) = match token.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, token),
    };
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        (false, "0")
    } else {
        (negative, digits)
    }
}

fn floats_equal(left: f64, right: f64, tolerance: FloatTolerance) -> bool {
    if left == right {
        return true;
    }
    if tolerance.absolute == 0.0 && tolerance.relative == 0.0 {
        return false;
    }
    let difference = (left - right).abs();
    if difference <= tolerance.absolute {
        return true;
    }
    let scale = left.abs().max(right.abs());
    let relative_difference = if difference.is_finite() {
        difference / scale
    } else {
        // Dividing first avoids overflow for opposite values near f64::MAX.
        (left / scale - right / scale).abs()
    };
    relative_difference <= tolerance.relative
}

fn child_path(parent: &str, component: &str) -> String {
    format!(
        "{parent}/{}",
        component.replace('~', "~0").replace('/', "~1")
    )
}

fn mismatch(
    path: &str,
    expected: Option<&Value>,
    actual: Option<&Value>,
    reason: &'static str,
) -> JsonMismatch {
    JsonMismatch {
        path: path.to_owned(),
        expected: expected.map(|value| Box::new(value.clone())),
        actual: actual.map(|value| Box::new(value.clone())),
        reason,
    }
}
