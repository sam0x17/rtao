use std::marker::PhantomData;

use scale_decode::{
    IntoVisitor, TypeResolver,
    visitor::{DecodeAsTypeResult, Visitor, decode_with_visitor},
};

/// Conversion performed after metadata-directed decoding of a wire primitive.
pub trait FromWire: Sized {
    /// Primitive accepted by the standard SCALE visitor.
    type Wire: IntoVisitor;
    /// Check and construct the domain primitive.
    fn from_wire(value: Self::Wire) -> Result<Self, scale_decode::Error>;
}

/// Visitor forwarding newtype and compact unwrapping to the standard visitor.
pub struct WireVisitor<T, R>(PhantomData<(T, R)>);

impl<T, R> WireVisitor<T, R> {
    /// Construct a stateless wire visitor.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T, R> Default for WireVisitor<T, R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: FromWire, R: TypeResolver> Visitor for WireVisitor<T, R> {
    type Value<'scale, 'resolver> = T;
    type Error = scale_decode::Error;
    type TypeResolver = R;

    fn unchecked_decode_as_type(
        self,
        input: &mut &[u8],
        type_id: R::TypeId,
        types: &R,
    ) -> DecodeAsTypeResult<Self, Result<T, Self::Error>> {
        DecodeAsTypeResult::Decoded(
            decode_with_visitor(input, type_id, types, T::Wire::into_visitor::<R>())
                .and_then(T::from_wire),
        )
    }
}
