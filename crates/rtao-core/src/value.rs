use std::fmt;

use crate::codec::{Value, value::to_corpus_json};

/// A dynamic decoded SCALE value with corpus-compatible JSON rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct DynValue(Value);

impl DynValue {
    /// Wrap a decoded core value without changing its shape.
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// Borrow the core carrier for encoding and typed model conversion.
    pub fn as_core(&self) -> &Value {
        &self.0
    }

    /// Recover the core carrier without cloning it.
    pub fn into_core(self) -> Value {
        self.0
    }

    /// Render bytes, tuples, dictionary keys and full-width integers according
    /// to the upstream shape corpus.
    pub fn to_json(&self) -> serde_json::Value {
        to_corpus_json(&self.0)
    }

    /// Borrow a string value.
    pub fn as_str(&self) -> Option<&str> {
        match &self.0 {
            Value::Str(value) => Some(value),
            _ => None,
        }
    }

    /// Read a boolean value without coercion.
    pub fn as_bool(&self) -> Option<bool> {
        match self.0 {
            Value::Bool(value) => Some(value),
            _ => None,
        }
    }

    /// Read an unsigned integer, rejecting negative or wider values.
    pub fn as_u128(&self) -> Option<u128> {
        match self.0 {
            Value::Int(value) => u128::try_from(value).ok(),
            Value::Uint(value) => Some(value),
            _ => None,
        }
    }

    /// Read a signed integer, rejecting unsigned values outside its range.
    pub fn as_i128(&self) -> Option<i128> {
        match self.0 {
            Value::Int(value) => Some(value),
            Value::Uint(value) => i128::try_from(value).ok(),
            _ => None,
        }
    }
}

impl From<Value> for DynValue {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

impl From<DynValue> for Value {
    fn from(value: DynValue) -> Self {
        value.into_core()
    }
}

impl AsRef<Value> for DynValue {
    fn as_ref(&self) -> &Value {
        self.as_core()
    }
}

impl fmt::Display for DynValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_json().fmt(formatter)
    }
}
