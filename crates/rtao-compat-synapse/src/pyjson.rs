//! Python `json.loads` input that serde_json refuses: bare `NaN`, `Infinity`
//! and `-Infinity`.
//!
//! aiohttp serializes request bodies with `json.dumps(allow_nan=True)`, so a
//! Python dendrite sends non-finite floats as those tokens, and Python axons
//! (`json.loads` plus pydantic) accept them. serde_json cannot represent them,
//! so a body that serde_json refuses and that contains such a token is parsed
//! here. Every subtree without a non-finite token is kept as raw JSON text and
//! later deserialized by serde_json itself, so only the path to a token uses
//! this module's deserializer.

use std::{collections::HashMap, fmt};

use serde::{
    Deserializer,
    de::{self, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor},
    forward_to_deserialize_any,
};

type Error = serde_json::Error;

/// Nesting limit, as serde_json's.
const MAX_DEPTH: usize = 128;

/// A parsed JSON value whose subtrees without non-finite tokens stay raw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Node<'a> {
    /// A complete JSON value without non-finite tokens.
    Raw(&'a str),
    /// `NaN`, `Infinity` or `-Infinity`.
    NonFinite(f64),
    /// An array containing a non-finite token.
    Array(Vec<Node<'a>>),
    /// An object containing a non-finite token, with Python's duplicate-key
    /// rule (first position, last value).
    Object(Vec<(String, Node<'a>)>),
}

impl Node<'_> {
    pub(crate) fn is_raw(&self) -> bool {
        matches!(self, Self::Raw(_))
    }
}

fn error(message: impl fmt::Display) -> Error {
    de::Error::custom(message)
}

/// Whether `bytes` may contain a Python-only constant.
pub(crate) fn may_have_constants(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|window| window == b"NaN")
        || bytes.windows(8).any(|window| window == b"Infinity")
}

/// Parse one JSON document with Python's grammar.
pub(crate) fn parse(text: &str) -> Result<Node<'_>, Error> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        at: 0,
    };
    let node = parser.value(0)?;
    parser.whitespace();
    if parser.at != parser.bytes.len() {
        return Err(error(format!("Extra data at byte {}", parser.at)));
    }
    Ok(node)
}

/// Parse `bytes` as serde_json would, falling back to Python's grammar when
/// serde_json refuses a document that contains a Python-only constant.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), Error> {
    match serde_json::from_slice::<de::IgnoredAny>(bytes) {
        Ok(_) => Ok(()),
        Err(original) => {
            if may_have_constants(bytes)
                && let Ok(text) = std::str::from_utf8(bytes)
                && parse(text).is_ok()
            {
                return Ok(());
            }
            Err(original)
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), Error> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(error(format!("Expecting value at byte {}", self.at)))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node<'a>, Error> {
        if depth > MAX_DEPTH {
            return Err(error("recursion limit exceeded"));
        }
        self.whitespace();
        let start = self.at;
        match self.peek() {
            Some(b'{') => self.object(start, depth),
            Some(b'[') => self.array(start, depth),
            Some(b'"') => {
                self.string()?;
                Ok(Node::Raw(&self.text[start..self.at]))
            }
            Some(b'N') => {
                self.literal("NaN")?;
                Ok(Node::NonFinite(f64::NAN))
            }
            Some(b'I') => {
                self.literal("Infinity")?;
                Ok(Node::NonFinite(f64::INFINITY))
            }
            Some(b'-') if self.bytes[self.at + 1..].starts_with(b"Infinity") => {
                self.literal("-Infinity")?;
                Ok(Node::NonFinite(f64::NEG_INFINITY))
            }
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Ok(Node::Raw(&self.text[start..self.at]))
            }
            Some(b't') => self.literal("true").map(|()| Node::Raw("true")),
            Some(b'f') => self.literal("false").map(|()| Node::Raw("false")),
            Some(b'n') => self.literal("null").map(|()| Node::Raw("null")),
            _ => Err(error(format!("Expecting value at byte {}", self.at))),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    // -?(0|[1-9]\d*)(\.\d+)?([eE][-+]?\d+)?
    fn number(&mut self) -> Result<(), Error> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(error(format!("Expecting value at byte {}", self.at))),
        }
        if self.peek() == Some(b'.') && matches!(self.bytes.get(self.at + 1), Some(b'0'..=b'9')) {
            self.at += 1;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mark = self.at;
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                // Python's regex stops before an incomplete exponent.
                self.at = mark;
            }
        }
        Ok(())
    }

    // strict=True: no raw control characters; escapes as in JSON.
    fn string(&mut self) -> Result<(), Error> {
        self.at += 1;
        loop {
            match self.peek() {
                None => return Err(error("Unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(());
                }
                Some(b'\\') => {
                    self.at += 1;
                    match self.peek() {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {
                            self.at += 1;
                        }
                        Some(b'u') => {
                            let hex = self.bytes.get(self.at + 1..self.at + 5);
                            if !hex.is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)) {
                                return Err(error("Invalid \\uXXXX escape"));
                            }
                            self.at += 5;
                        }
                        _ => return Err(error("Invalid \\escape")),
                    }
                }
                Some(0x00..=0x1f) => return Err(error("Invalid control character")),
                Some(_) => self.at += 1,
            }
        }
    }

    fn array(&mut self, start: usize, depth: usize) -> Result<Node<'a>, Error> {
        self.at += 1;
        let mut items = Vec::new();
        self.whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Node::Raw(&self.text[start..self.at]));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    break;
                }
                _ => return Err(error("Expecting ',' delimiter")),
            }
        }
        if items.iter().all(Node::is_raw) {
            Ok(Node::Raw(&self.text[start..self.at]))
        } else {
            Ok(Node::Array(items))
        }
    }

    fn object(&mut self, start: usize, depth: usize) -> Result<Node<'a>, Error> {
        self.at += 1;
        let mut entries: Vec<(String, Node<'a>)> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        // Raw text is usable only if no value, even an overwritten duplicate,
        // held a non-finite token.
        let mut clean = true;
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Node::Raw(&self.text[start..self.at]));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(error("Expecting property name enclosed in double quotes"));
            }
            let key_start = self.at;
            self.string()?;
            let key: String = serde_json::from_str(&self.text[key_start..self.at])?;
            self.whitespace();
            if self.peek() != Some(b':') {
                return Err(error("Expecting ':' delimiter"));
            }
            self.at += 1;
            let value = self.value(depth + 1)?;
            clean &= value.is_raw();
            match index.get(&key) {
                Some(&slot) => entries[slot].1 = value,
                None => {
                    index.insert(key.clone(), entries.len());
                    entries.push((key, value));
                }
            }
            self.whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    break;
                }
                _ => return Err(error("Expecting ',' delimiter")),
            }
        }
        if clean {
            Ok(Node::Raw(&self.text[start..self.at]))
        } else {
            Ok(Node::Object(entries))
        }
    }
}

macro_rules! forward_raw {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self {
                Node::Raw(text) => serde_json::Deserializer::from_str(text).$method(visitor),
                other => other.deserialize_any(visitor),
            }
        }
    )*};
}

impl<'de> Deserializer<'de> for Node<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_any(visitor),
            Node::NonFinite(value) => visitor.visit_f64(value),
            Node::Array(items) => {
                let length = items.len();
                let mut access = Seq(items.into_iter());
                let value = visitor.visit_seq(&mut access)?;
                if access.0.len() == 0 {
                    Ok(value)
                } else {
                    Err(de::Error::invalid_length(
                        length,
                        &"fewer elements in array",
                    ))
                }
            }
            Node::Object(entries) => visitor.visit_map(Map {
                entries: entries.into_iter(),
                value: None,
            }),
        }
    }

    forward_raw!(
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64
        deserialize_i128 deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64
        deserialize_u128 deserialize_f32 deserialize_f64 deserialize_char deserialize_str
        deserialize_string deserialize_bytes deserialize_byte_buf deserialize_unit
        deserialize_seq deserialize_map deserialize_identifier deserialize_ignored_any
    );

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_option(visitor),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => {
                serde_json::Deserializer::from_str(text).deserialize_unit_struct(name, visitor)
            }
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => {
                serde_json::Deserializer::from_str(text).deserialize_newtype_struct(name, visitor)
            }
            other => visitor.visit_newtype_struct(other),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => {
                serde_json::Deserializer::from_str(text).deserialize_tuple(length, visitor)
            }
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        length: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => serde_json::Deserializer::from_str(text)
                .deserialize_tuple_struct(name, length, visitor),
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => {
                serde_json::Deserializer::from_str(text).deserialize_struct(name, fields, visitor)
            }
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Node::Raw(text) => {
                serde_json::Deserializer::from_str(text).deserialize_enum(name, variants, visitor)
            }
            _ => Err(error("non-finite floats inside an enum are not supported")),
        }
    }
}

struct Seq<'de>(std::vec::IntoIter<Node<'de>>);

impl<'de> SeqAccess<'de> for Seq<'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        self.0.next().map(|node| seed.deserialize(node)).transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

struct Map<'de> {
    entries: std::vec::IntoIter<(String, Node<'de>)>,
    value: Option<Node<'de>>,
}

impl<'de> MapAccess<'de> for Map<'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        match self.entries.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(Key(key)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let value = self
            .value
            .take()
            .ok_or_else(|| error("map value without a key"))?;
        seed.deserialize(value)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

/// An object key: a string, or a number spelled as one (as serde_json allows).
struct Key(String);

macro_rules! numeric_key {
    ($($method:ident $kind:ty => $visit:ident;)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.0.parse::<$kind>() {
                Ok(value) => visitor.$visit(value),
                Err(_) => visitor.visit_string(self.0),
            }
        }
    )*};
}

impl<'de> Deserializer<'de> for Key {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_string(self.0)
    }

    numeric_key!(
        deserialize_i8 i8 => visit_i8;
        deserialize_i16 i16 => visit_i16;
        deserialize_i32 i32 => visit_i32;
        deserialize_i64 i64 => visit_i64;
        deserialize_u8 u8 => visit_u8;
        deserialize_u16 u16 => visit_u16;
        deserialize_u32 u32 => visit_u32;
        deserialize_u64 u64 => visit_u64;
        deserialize_f32 f32 => visit_f32;
        deserialize_f64 f64 => visit_f64;
    );

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_enum(IntoDeserializer::<Error>::into_deserializer(self.0))
    }

    forward_to_deserialize_any! {
        bool i128 u128 char str string bytes byte_buf unit unit_struct seq tuple
        tuple_struct map struct identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use serde::Deserialize;

    use super::*;
    use crate::PyDict;

    #[test]
    fn keeps_clean_subtrees_raw() {
        let text =
            r#" {"a": [1, {"b": "NaN"}], "c": [NaN, 2.5e3], "d": {"e": -Infinity, "f": null}} "#;
        let Node::Object(entries) = parse(text).unwrap() else {
            panic!("expected an object");
        };
        assert_eq!(entries[0], ("a".into(), Node::Raw(r#"[1, {"b": "NaN"}]"#)));
        assert!(matches!(&entries[1].1, Node::Array(items)
            if matches!(items[0], Node::NonFinite(value) if value.is_nan())
                && items[1] == Node::Raw("2.5e3")));
        assert!(matches!(&entries[2].1, Node::Object(inner)
            if inner[0].1 == Node::NonFinite(f64::NEG_INFINITY) && inner[1].1 == Node::Raw("null")));
        assert_eq!(
            parse(r#"{"x": "Infinity"}"#).unwrap(),
            Node::Raw(r#"{"x": "Infinity"}"#)
        );
        // An overwritten non-finite duplicate must not leave raw text behind.
        assert_eq!(
            parse(r#"{"x": NaN, "x": 1}"#).unwrap(),
            Node::Object(vec![("x".into(), Node::Raw("1"))])
        );
    }

    #[test]
    fn follows_python_grammar() {
        for bad in [
            "-NaN",
            "+Infinity",
            "nan",
            "inf",
            "Infinityx",
            "[NaN,]",
            "{\"a\" NaN}",
            "01",
            "[NaN] x",
            "\"\u{1}\"",
            "{'a': NaN}",
            "[1.]",
            "\"\\x\"",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("[NaN, -Infinity, Infinity, -0, 1e5, 1E+2, \"\\u00e9\"]").is_ok());
        let nested = format!("{}NaN{}", "[".repeat(200), "]".repeat(200));
        assert!(parse(&nested).is_err());
        assert!(validate(b"[NaN]").is_ok());
        assert!(validate(b"[NaN").is_err());
        assert!(validate(b"{\"a\": 1}").is_ok());
    }

    #[test]
    fn deserializes_python_values() {
        #[derive(Debug, Deserialize)]
        struct Body {
            v: f64,
            o: Option<f64>,
            l: Vec<f64>,
            d: PyDict<String, f64>,
            t: (f64, i64),
            n: serde_json::Value,
            s: String,
        }
        let text = r#"{"v": NaN, "o": Infinity, "l": [1, -Infinity], "d": {"z": NaN, "a": 1.5, "z": 2.0},
            "t": [NaN, 7], "n": {"k": [1, 2]}, "s": "x\u00e9", "v": -Infinity}"#;
        let body = Body::deserialize(parse(text).unwrap()).unwrap();
        assert_eq!(body.v, f64::NEG_INFINITY);
        assert_eq!(body.o, Some(f64::INFINITY));
        assert_eq!(body.l, vec![1.0, f64::NEG_INFINITY]);
        assert_eq!(body.d.0, vec![("z".into(), 2.0), ("a".into(), 1.5)]);
        assert!(body.t.0.is_nan() && body.t.1 == 7);
        assert_eq!(body.n, serde_json::json!({"k": [1, 2]}));
        assert_eq!(body.s, "x\u{e9}");
        let keyed = PyDict::<i64, f64>::deserialize(parse(r#"{"3": NaN}"#).unwrap()).unwrap();
        assert_eq!(keyed.0[0].0, 3);
    }
}
