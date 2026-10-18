// Derived from inference-labs-inc/btlightning crates/btlightning/src/types.rs @ 53c090b436238f78a4a0dd30cbbfcba1cbebe2e0 (MIT); modified by rtao.
//! Typed conversion between Rust values and request `data` maps.
//!
//! [`to_data`] is a port of btlightning's `NamedSerializer`: structs and maps
//! become string-keyed maps, unit enum variants become their index, and other
//! variants become `{index: content}`. Rust btlightning peers produce exactly
//! these shapes.
//!
//! [`from_data`] is deliberately more tolerant than btlightning's decoder,
//! which uses `rmpv::ext::from_value` and accepts enums only as `[index]` or
//! `[index, content]`, so it cannot decode the shapes its own encoder emits.
//! rtao accepts the encoder's `index` and `{index: content}` shapes, the
//! variant name in place of an index, and rmpv's array shapes.

use std::fmt;

use rmpv::{Integer, Utf8String, Value};
use serde::de::value::{MapDeserializer, SeqDeserializer};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, VariantAccess, Visitor,
};
use serde::{Serialize, ser};

use crate::wire::DataMap;

/// A value that cannot be converted to or from a `data` map.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DataError(pub String);

impl ser::Error for DataError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

impl de::Error for DataError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

/// Convert a serializable value, which must serialize as a map or struct,
/// into a `data` map with btlightning's typed-handler encoding.
///
/// Non-string top-level keys are rendered with `rmpv::Value`'s `Display`.
pub fn to_data<T: Serialize + ?Sized>(value: &T) -> Result<DataMap, DataError> {
    match value.serialize(NamedSerializer)? {
        Value::Map(entries) => entries
            .into_iter()
            .map(|(k, v)| {
                let key = match k {
                    Value::String(s) => s
                        .into_str()
                        .ok_or_else(|| DataError("non-UTF8 map key".into()))?,
                    other => other.to_string(),
                };
                Ok((key, v))
            })
            .collect(),
        _ => Err(DataError("expected map from serialized struct".into())),
    }
}

/// Convert a single value with btlightning's typed-handler encoding.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, DataError> {
    value.serialize(NamedSerializer)
}

/// Decode a typed value from a `data` map. See the module docs for the
/// accepted enum shapes.
pub fn from_data<T: DeserializeOwned>(data: DataMap) -> Result<T, DataError> {
    from_value(Value::Map(
        data.into_iter()
            .map(|(k, v)| (Value::String(k.into()), v))
            .collect(),
    ))
}

/// Decode a typed value from a single MessagePack value.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, DataError> {
    T::deserialize(ValueDe(value))
}

struct NamedSerializer;

fn int<N: Into<Integer>>(n: N) -> Value {
    Value::Integer(n.into())
}

impl ser::Serializer for NamedSerializer {
    type Ok = Value;
    type Error = DataError;
    type SerializeSeq = SerializeVec;
    type SerializeTuple = SerializeVec;
    type SerializeTupleStruct = SerializeVec;
    type SerializeTupleVariant = SerializeTupleVariant;
    type SerializeMap = SerializeMap;
    type SerializeStruct = SerializeMap;
    type SerializeStructVariant = SerializeStructVariant;

    fn serialize_bool(self, v: bool) -> Result<Value, DataError> {
        Ok(Value::Boolean(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, DataError> {
        Ok(int(i64::from(v)))
    }
    fn serialize_i16(self, v: i16) -> Result<Value, DataError> {
        Ok(int(i64::from(v)))
    }
    fn serialize_i32(self, v: i32) -> Result<Value, DataError> {
        Ok(int(i64::from(v)))
    }
    fn serialize_i64(self, v: i64) -> Result<Value, DataError> {
        Ok(int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<Value, DataError> {
        Ok(int(u64::from(v)))
    }
    fn serialize_u16(self, v: u16) -> Result<Value, DataError> {
        Ok(int(u64::from(v)))
    }
    fn serialize_u32(self, v: u32) -> Result<Value, DataError> {
        Ok(int(u64::from(v)))
    }
    fn serialize_u64(self, v: u64) -> Result<Value, DataError> {
        Ok(int(v))
    }
    fn serialize_f32(self, v: f32) -> Result<Value, DataError> {
        Ok(Value::F32(v))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, DataError> {
        Ok(Value::F64(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, DataError> {
        Ok(Value::String(Utf8String::from(v.to_string())))
    }
    fn serialize_str(self, v: &str) -> Result<Value, DataError> {
        Ok(Value::String(Utf8String::from(v)))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, DataError> {
        Ok(Value::Binary(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Value, DataError> {
        Ok(Value::Nil)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Value, DataError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value, DataError> {
        Ok(Value::Nil)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, DataError> {
        Ok(Value::Nil)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        idx: u32,
        _variant: &'static str,
    ) -> Result<Value, DataError> {
        Ok(int(idx))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Value, DataError> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        idx: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Value, DataError> {
        Ok(Value::Map(vec![(
            int(idx),
            value.serialize(NamedSerializer)?,
        )]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SerializeVec, DataError> {
        Ok(SerializeVec(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<SerializeVec, DataError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SerializeVec, DataError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        idx: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<SerializeTupleVariant, DataError> {
        Ok(SerializeTupleVariant {
            idx,
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<SerializeMap, DataError> {
        Ok(SerializeMap {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<SerializeMap, DataError> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        idx: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<SerializeStructVariant, DataError> {
        Ok(SerializeStructVariant {
            idx,
            entries: Vec::with_capacity(len),
        })
    }
}

struct SerializeVec(Vec<Value>);

impl ser::SerializeSeq for SerializeVec {
    type Ok = Value;
    type Error = DataError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DataError> {
        self.0.push(value.serialize(NamedSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, DataError> {
        Ok(Value::Array(self.0))
    }
}

impl ser::SerializeTuple for SerializeVec {
    type Ok = Value;
    type Error = DataError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DataError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value, DataError> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SerializeVec {
    type Ok = Value;
    type Error = DataError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DataError> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value, DataError> {
        ser::SerializeSeq::end(self)
    }
}

struct SerializeTupleVariant {
    idx: u32,
    items: Vec<Value>,
}

impl ser::SerializeTupleVariant for SerializeTupleVariant {
    type Ok = Value;
    type Error = DataError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DataError> {
        self.items.push(value.serialize(NamedSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, DataError> {
        Ok(Value::Map(vec![(int(self.idx), Value::Array(self.items))]))
    }
}

struct SerializeMap {
    entries: Vec<(Value, Value)>,
    key: Option<Value>,
}

impl ser::SerializeMap for SerializeMap {
    type Ok = Value;
    type Error = DataError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), DataError> {
        self.key = Some(key.serialize(NamedSerializer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), DataError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| DataError("serialize_value called before serialize_key".into()))?;
        self.entries.push((key, value.serialize(NamedSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<Value, DataError> {
        Ok(Value::Map(self.entries))
    }
}

impl ser::SerializeStruct for SerializeMap {
    type Ok = Value;
    type Error = DataError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), DataError> {
        self.entries.push((
            Value::String(Utf8String::from(key)),
            value.serialize(NamedSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<Value, DataError> {
        Ok(Value::Map(self.entries))
    }
}

struct SerializeStructVariant {
    idx: u32,
    entries: Vec<(Value, Value)>,
}

impl ser::SerializeStructVariant for SerializeStructVariant {
    type Ok = Value;
    type Error = DataError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), DataError> {
        self.entries.push((
            Value::String(Utf8String::from(key)),
            value.serialize(NamedSerializer)?,
        ));
        Ok(())
    }
    fn end(self) -> Result<Value, DataError> {
        Ok(Value::Map(vec![(int(self.idx), Value::Map(self.entries))]))
    }
}

/// A deserializer over an owned value that accepts every enum shape listed
/// in the module docs.
struct ValueDe(Value);

impl<'de> IntoDeserializer<'de, DataError> for ValueDe {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

const EXT_STRUCT_NAME: &str = "_ExtStruct";

impl<'de> de::Deserializer<'de> for ValueDe {
    type Error = DataError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DataError> {
        match self.0 {
            Value::Nil => visitor.visit_unit(),
            Value::Boolean(b) => visitor.visit_bool(b),
            Value::Integer(i) => match (i.as_u64(), i.as_i64()) {
                (Some(u), _) => visitor.visit_u64(u),
                (None, Some(s)) => visitor.visit_i64(s),
                (None, None) => Err(DataError("integer out of range".into())),
            },
            Value::F32(f) => visitor.visit_f32(f),
            Value::F64(f) => visitor.visit_f64(f),
            Value::String(s) => match String::from_utf8(s.into_bytes()) {
                Ok(s) => visitor.visit_string(s),
                Err(e) => visitor.visit_byte_buf(e.into_bytes()),
            },
            Value::Binary(b) => visitor.visit_byte_buf(b),
            Value::Array(items) => {
                let mut seq = SeqDeserializer::new(items.into_iter().map(ValueDe));
                let out = visitor.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(out)
            }
            Value::Map(entries) => {
                let mut map = MapDeserializer::new(
                    entries.into_iter().map(|(k, v)| (ValueDe(k), ValueDe(v))),
                );
                let out = visitor.visit_map(&mut map)?;
                map.end()?;
                Ok(out)
            }
            Value::Ext(tag, data) => visitor.visit_newtype_struct(ValueDe(Value::Array(vec![
                int(i64::from(tag)),
                Value::Binary(data),
            ]))),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DataError> {
        match self.0 {
            Value::Nil => visitor.visit_none(),
            other => visitor.visit_some(ValueDe(other)),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DataError> {
        match self.0 {
            Value::Ext(..) if name == EXT_STRUCT_NAME => self.deserialize_any(visitor),
            other => visitor.visit_newtype_struct(ValueDe(other)),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DataError> {
        let (variant, content) = match self.0 {
            v @ (Value::Integer(_) | Value::String(_)) => (v, None),
            Value::Map(mut entries) if entries.len() == 1 => match entries.pop() {
                Some((k, v)) => (k, Some(v)),
                None => return Err(DataError("empty enum map".into())),
            },
            Value::Array(mut items) if items.len() == 1 || items.len() == 2 => {
                let content = if items.len() == 2 { items.pop() } else { None };
                match items.pop() {
                    Some(k) => (k, content),
                    None => return Err(DataError("empty enum array".into())),
                }
            }
            other => {
                return Err(DataError(format!(
                    "invalid enum encoding {other}; expected index, name, {{index: value}} or [index, value]"
                )));
            }
        };
        visitor.visit_enum(EnumDe { variant, content })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct EnumDe {
    variant: Value,
    content: Option<Value>,
}

impl<'de> EnumAccess<'de> for EnumDe {
    type Error = DataError;
    type Variant = VariantDe;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, VariantDe), DataError> {
        let tag = seed.deserialize(ValueDe(self.variant))?;
        Ok((tag, VariantDe(self.content)))
    }
}

struct VariantDe(Option<Value>);

impl<'de> VariantAccess<'de> for VariantDe {
    type Error = DataError;

    fn unit_variant(self) -> Result<(), DataError> {
        match self.0 {
            None | Some(Value::Nil) => Ok(()),
            Some(other) => Err(DataError(format!(
                "unit variant carries unexpected content {other}"
            ))),
        }
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, DataError> {
        seed.deserialize(ValueDe(self.0.unwrap_or(Value::Nil)))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DataError> {
        de::Deserializer::deserialize_any(ValueDe(self.0.unwrap_or(Value::Nil)), visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DataError> {
        de::Deserializer::deserialize_any(ValueDe(self.0.unwrap_or(Value::Nil)), visitor)
    }
}
