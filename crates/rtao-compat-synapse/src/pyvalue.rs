// Derived from opentensor/bittensor bittensor/core/synapse.py @ v10.5.0 (MIT); modified by rtao.
// Derived from opentensor/bittensor bittensor/utils/__init__.py @ v10.5.0 (MIT); modified by rtao.
//! Python values for `str()`/`repr()` body hashing.

use std::{collections::HashMap, fmt, hash::Hash};

use num_bigint::BigInt;
use rtao_primitives::pyfmt::{float_repr, string_repr};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{MapAccess, Visitor},
    ser::{self, SerializeMap},
};
use sha3::{Digest, Sha3_256};

/// A Python value as pydantic's Python-mode `model_dump()` produces it.
///
/// Only the types whose `str()` the v10 body hash can reproduce exactly are
/// represented; bytes, sets, enums and datetimes are unsupported.
#[derive(Clone, Debug, PartialEq)]
pub enum PyValue {
    /// `None`.
    None,
    /// `bool`.
    Bool(bool),
    /// Arbitrary-precision `int`.
    Int(BigInt),
    /// Binary64 `float`.
    Float(f64),
    /// `str`.
    Str(String),
    /// `list`.
    List(Vec<PyValue>),
    /// `tuple`.
    Tuple(Vec<PyValue>),
    /// `dict`, in insertion order.
    Dict(Vec<(PyValue, PyValue)>),
}

impl PyValue {
    /// Look up a string key in a dict value.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        match self {
            Self::Dict(entries) => entries.iter().find_map(|(name, value)| match name {
                Self::Str(name) if name == key => Some(value),
                _ => None,
            }),
            _ => None,
        }
    }

    /// Whether this value, or any value it contains, is a non-finite float.
    pub fn has_non_finite(&self) -> bool {
        match self {
            Self::Float(value) => !value.is_finite(),
            Self::List(items) | Self::Tuple(items) => items.iter().any(Self::has_non_finite),
            Self::Dict(entries) => entries
                .iter()
                .any(|(key, value)| key.has_non_finite() || value.has_non_finite()),
            _ => false,
        }
    }
}

/// Python's top-level `str(value)`: strings verbatim, everything else `repr`.
pub fn py_str(value: &PyValue) -> String {
    match value {
        PyValue::Str(text) => text.clone(),
        other => py_repr(other),
    }
}

/// Python's `repr(value)`.
pub fn py_repr(value: &PyValue) -> String {
    let mut out = String::new();
    write_repr(value, &mut out);
    out
}

fn write_repr(value: &PyValue, out: &mut String) {
    match value {
        PyValue::None => out.push_str("None"),
        PyValue::Bool(true) => out.push_str("True"),
        PyValue::Bool(false) => out.push_str("False"),
        PyValue::Int(value) => out.push_str(&value.to_string()),
        PyValue::Float(value) => out.push_str(&float_repr(*value)),
        PyValue::Str(text) => out.push_str(&string_repr(text)),
        PyValue::List(items) => {
            out.push('[');
            write_items(items, out);
            out.push(']');
        }
        PyValue::Tuple(items) => {
            out.push('(');
            write_items(items, out);
            if items.len() == 1 {
                out.push(',');
            }
            out.push(')');
        }
        PyValue::Dict(entries) => {
            out.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_repr(key, out);
                out.push_str(": ");
                write_repr(value, out);
            }
            out.push('}');
        }
    }
}

fn write_items(items: &[PyValue], out: &mut String) {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        write_repr(item, out);
    }
}

/// Python's `bittensor.utils.get_hash`: lowercase hex SHA3-256 of UTF-8 text.
pub fn get_hash(text: &str) -> String {
    hex::encode(Sha3_256::digest(text.as_bytes()))
}

/// A value has no exact Python `str()` equivalent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyValueError(pub String);

impl fmt::Display for PyValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for PyValueError {}
impl ser::Error for PyValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

/// Convert a serializable value into the Python value pydantic would hold.
///
/// Integers stay `int`, floats stay `float` (an `f32` becomes the `float`
/// Python parses from its shortest JSON text), sequences become `list`,
/// tuples `tuple`, maps and structs `dict` in serialization order, and unit
/// enum variants `str`. Byte strings are refused.
pub fn to_py_value<T: Serialize + ?Sized>(value: &T) -> Result<PyValue, PyValueError> {
    value.serialize(PySerializer)
}

// serde_json's arbitrary_precision feature (enabled workspace-wide) serializes
// a `Number` as a one-field struct with this name, carrying its decimal text.
const JSON_NUMBER_TOKEN: &str = "$serde_json::private::Number";

struct PySerializer;

fn unsupported(kind: &str) -> PyValueError {
    PyValueError(format!(
        "{kind} values have no exact Python str() equivalent"
    ))
}

impl Serializer for PySerializer {
    type Ok = PyValue;
    type Error = PyValueError;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = VariantBuilder<SeqBuilder>;
    type SerializeMap = MapBuilder;
    type SerializeStruct = StructBuilder;
    type SerializeStructVariant = VariantBuilder<StructBuilder>;

    fn serialize_bool(self, value: bool) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_i16(self, value: i16) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_i32(self, value: i32) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_i64(self, value: i64) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_i128(self, value: i128) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_u8(self, value: u8) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_u16(self, value: u16) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_u32(self, value: u32) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_u64(self, value: u64) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_u128(self, value: u128) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Int(value.into()))
    }
    fn serialize_f32(self, value: f32) -> Result<PyValue, PyValueError> {
        if !value.is_finite() {
            return Ok(PyValue::Float(f64::from(value)));
        }
        // JSON carries the shortest f32 text; Python parses it as binary64.
        value
            .to_string()
            .parse()
            .map(PyValue::Float)
            .map_err(|_| unsupported("f32"))
    }
    fn serialize_f64(self, value: f64) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Float(value))
    }
    fn serialize_char(self, value: char) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Str(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Str(value.to_owned()))
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<PyValue, PyValueError> {
        Err(unsupported("bytes"))
    }
    fn serialize_none(self) -> Result<PyValue, PyValueError> {
        Ok(PyValue::None)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<PyValue, PyValueError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<PyValue, PyValueError> {
        Ok(PyValue::None)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<PyValue, PyValueError> {
        Ok(PyValue::None)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Str(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<PyValue, PyValueError> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Dict(vec![(
            PyValue::Str(variant.to_owned()),
            value.serialize(self)?,
        )]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder, PyValueError> {
        Ok(SeqBuilder::new(len, false))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder, PyValueError> {
        Ok(SeqBuilder::new(Some(len), true))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SeqBuilder, PyValueError> {
        Ok(SeqBuilder::new(Some(len), true))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantBuilder<SeqBuilder>, PyValueError> {
        Ok(VariantBuilder {
            variant,
            inner: SeqBuilder::new(Some(len), true),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapBuilder, PyValueError> {
        Ok(MapBuilder {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            key: None,
        })
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<StructBuilder, PyValueError> {
        Ok(StructBuilder {
            entries: Vec::with_capacity(len),
            number: name == JSON_NUMBER_TOKEN,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantBuilder<StructBuilder>, PyValueError> {
        Ok(VariantBuilder {
            variant,
            inner: StructBuilder {
                entries: Vec::with_capacity(len),
                number: false,
            },
        })
    }
}

#[doc(hidden)]
pub struct SeqBuilder {
    items: Vec<PyValue>,
    tuple: bool,
}

impl SeqBuilder {
    fn new(len: Option<usize>, tuple: bool) -> Self {
        Self {
            items: Vec::with_capacity(len.unwrap_or(0).min(4096)),
            tuple,
        }
    }
    fn finish(self) -> PyValue {
        if self.tuple {
            PyValue::Tuple(self.items)
        } else {
            PyValue::List(self.items)
        }
    }
}

impl ser::SerializeSeq for SeqBuilder {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PyValueError> {
        self.items.push(value.serialize(PySerializer)?);
        Ok(())
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        Ok(self.finish())
    }
}
impl ser::SerializeTuple for SeqBuilder {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PyValueError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        Ok(self.finish())
    }
}
impl ser::SerializeTupleStruct for SeqBuilder {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PyValueError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        Ok(self.finish())
    }
}

#[doc(hidden)]
pub struct MapBuilder {
    entries: Vec<(PyValue, PyValue)>,
    key: Option<PyValue>,
}

impl ser::SerializeMap for MapBuilder {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), PyValueError> {
        self.key = Some(key.serialize(PySerializer)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PyValueError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| PyValueError("map value without a key".into()))?;
        self.entries.push((key, value.serialize(PySerializer)?));
        Ok(())
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Dict(self.entries))
    }
}

#[doc(hidden)]
pub struct StructBuilder {
    entries: Vec<(PyValue, PyValue)>,
    number: bool,
}

impl ser::SerializeStruct for StructBuilder {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), PyValueError> {
        self.entries
            .push((PyValue::Str(key.to_owned()), value.serialize(PySerializer)?));
        Ok(())
    }
    fn skip_field(&mut self, _key: &'static str) -> Result<(), PyValueError> {
        Ok(())
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        if self.number {
            let Some((_, PyValue::Str(text))) = self.entries.into_iter().next() else {
                return Err(PyValueError("malformed serde_json number".into()));
            };
            return json_number(&text);
        }
        Ok(PyValue::Dict(self.entries))
    }
}

/// Python's `json.loads` reading of a JSON number token.
fn json_number(text: &str) -> Result<PyValue, PyValueError> {
    if text.contains(['.', 'e', 'E']) {
        text.parse()
            .map(PyValue::Float)
            .map_err(|_| PyValueError(format!("invalid JSON number {text}")))
    } else {
        text.parse()
            .map(PyValue::Int)
            .map_err(|_| PyValueError(format!("invalid JSON number {text}")))
    }
}

#[doc(hidden)]
pub struct VariantBuilder<B> {
    variant: &'static str,
    inner: B,
}

impl ser::SerializeTupleVariant for VariantBuilder<SeqBuilder> {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PyValueError> {
        ser::SerializeSeq::serialize_element(&mut self.inner, value)
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        Ok(PyValue::Dict(vec![(
            PyValue::Str(self.variant.to_owned()),
            self.inner.finish(),
        )]))
    }
}
impl ser::SerializeStructVariant for VariantBuilder<StructBuilder> {
    type Ok = PyValue;
    type Error = PyValueError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), PyValueError> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }
    fn end(self) -> Result<PyValue, PyValueError> {
        let inner = ser::SerializeStruct::end(self.inner)?;
        Ok(PyValue::Dict(vec![(
            PyValue::Str(self.variant.to_owned()),
            inner,
        )]))
    }
}

/// An insertion-ordered map that serializes as a JSON object.
///
/// Python dicts keep JSON key order, and the body hash renders them in that
/// order. Deserializing into this type keeps the received order, so a hashed
/// `dict` field round-trips exactly. A duplicate key keeps the last value at
/// the first key's position, as Python's dict does.
#[derive(Clone, Debug, PartialEq)]
pub struct PyDict<K, V>(pub Vec<(K, V)>);

impl<K, V> Default for PyDict<K, V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<K: Serialize, V: Serialize> Serialize for PyDict<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de, K, V> Deserialize<'de> for PyDict<K, V>
where
    K: Deserialize<'de> + Eq + Hash + Clone,
    V: Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DictVisitor<K, V>(std::marker::PhantomData<(K, V)>);
        impl<'de, K, V> Visitor<'de> for DictVisitor<K, V>
        where
            K: Deserialize<'de> + Eq + Hash + Clone,
            V: Deserialize<'de>,
        {
            type Value = PyDict<K, V>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(K, V)> = Vec::new();
                // Index by key so that many distinct keys stay linear.
                let mut index: HashMap<K, usize> = HashMap::new();
                while let Some((key, value)) = access.next_entry::<K, V>()? {
                    // Python's dict keeps the first position and the last value.
                    if let Some(&slot) = index.get(&key) {
                        entries[slot].1 = value;
                    } else {
                        index.insert(key.clone(), entries.len());
                        entries.push((key, value));
                    }
                }
                Ok(PyDict(entries))
            }
        }
        deserializer.deserialize_map(DictVisitor(std::marker::PhantomData))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn golden_hashes() {
        assert_eq!(get_hash(""), crate::EMPTY_BODY_HASH);
        assert_eq!(
            get_hash("1"),
            "67b176705b46206614219f47a05aee7ae6a3edbe850bbbe214c536b989aea4d2"
        );
        assert_eq!(
            get_hash("None"),
            "61ae1749811579a5799852da2541dd16a7ffb1744c6248718a123570fffd0a63"
        );
        let list = to_py_value(&vec!["foobar"]).unwrap();
        assert_eq!(py_str(&list), "['foobar']");
        assert_eq!(
            get_hash(&py_str(&list)),
            "182e45de5f003a5f02ce181fd7f2a2a45790550feaf90cf8ffdad76f5c450fc1"
        );
    }

    #[test]
    fn reprs_follow_python() {
        let value =
            to_py_value(&(1_u8, "a'b", vec![0.1_f64, 1e16, 1e-5], Option::<i32>::None)).unwrap();
        assert_eq!(py_repr(&value), "(1, \"a'b\", [0.1, 1e+16, 1e-05], None)");
        assert_eq!(py_repr(&to_py_value(&(true,)).unwrap()), "(True,)");
        let dict = PyDict(vec![("z".to_owned(), 1.5_f32), ("a".to_owned(), 0.1_f32)]);
        assert_eq!(py_str(&to_py_value(&dict).unwrap()), "{'z': 1.5, 'a': 0.1}");
        assert_eq!(py_str(&PyValue::Str("x'".into())), "x'");
        assert!(to_py_value(&serde_bytes_like()).is_err());
    }

    fn serde_bytes_like() -> impl Serialize {
        struct Bytes;
        impl Serialize for Bytes {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(b"x")
            }
        }
        Bytes
    }

    #[test]
    fn json_numbers_keep_python_types() {
        let value: serde_json::Value =
            serde_json::from_str("[1, 1.0, 100000000000000000000]").unwrap();
        assert_eq!(
            py_repr(&to_py_value(&value).unwrap()),
            "[1, 1.0, 100000000000000000000]"
        );
    }

    #[test]
    fn py_dict_keeps_order_and_last_value() {
        let dict: PyDict<String, i32> = serde_json::from_str(r#"{"b":1,"a":2,"b":3}"#).unwrap();
        assert_eq!(dict.0, vec![("b".into(), 3), ("a".into(), 2)]);
        assert_eq!(serde_json::to_string(&dict).unwrap(), r#"{"b":3,"a":2}"#);
    }
}
