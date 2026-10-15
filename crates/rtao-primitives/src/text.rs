use parity_scale_codec::{Compact, Decode, Encode, Error, Input, Output};
use scale_decode::{IntoVisitor, TypeResolver};
use scale_info::{Type, TypeInfo};

use crate::{
    IntoCoreValue,
    decoding::{FromWire, WireVisitor},
};

/// Bytes encoded as `Vec<Compact<u8>>`, used for subnet names and symbols.
///
/// Byte values at least 64 occupy two SCALE bytes each. The stored bytes remain
/// exact even when they are not valid UTF-8.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct CompactBytesText(#[doc = "Exact compact-byte payload."] pub Vec<u8>);

impl CompactBytesText {
    /// Wrap exact bytes, without requiring UTF-8.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// Borrow the exact bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Render valid UTF-8, rejecting malformed text without losing bytes.
    pub fn as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.0)
    }
}
impl From<&str> for CompactBytesText {
    fn from(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }
}
impl Encode for CompactBytesText {
    fn size_hint(&self) -> usize {
        self.0.len().saturating_mul(2).saturating_add(5)
    }
    fn encode_to<T: Output + ?Sized>(&self, dest: &mut T) {
        self.0
            .iter()
            .copied()
            .map(Compact)
            .collect::<Vec<_>>()
            .encode_to(dest);
    }
}
impl Decode for CompactBytesText {
    fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
        Ok(Self(
            Vec::<Compact<u8>>::decode(input)?
                .into_iter()
                .map(|value| value.0)
                .collect(),
        ))
    }
}
impl TypeInfo for CompactBytesText {
    type Identity = Vec<Compact<u8>>;
    fn type_info() -> Type {
        <Self::Identity as TypeInfo>::type_info()
    }
}
impl FromWire for CompactBytesText {
    type Wire = Vec<u8>;
    fn from_wire(value: Self::Wire) -> Result<Self, scale_decode::Error> {
        Ok(Self(value))
    }
}
impl IntoVisitor for CompactBytesText {
    type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
    fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
        WireVisitor::new()
    }
}
impl IntoCoreValue for CompactBytesText {
    fn into_core_value(self) -> rtao_core::codec::Value {
        self.0.into_core_value()
    }
}
