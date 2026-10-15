//! Name-based chain descriptors. Encoded indices and storage hashers stay live.

use crate::{RuntimeStamp, ShapeHash};
use rtao_core::codec::Value;
use std::marker::PhantomData;

/// A named argument and its metadata identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldDescriptor {
    /// Metadata field name; empty for positional fields.
    pub name: &'static str,
    /// Metadata type identity, independent of registry IDs.
    pub type_ident: &'static str,
}

/// One callable variant resolved by name before encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallDescriptor {
    /// Pallet name.
    pub pallet: &'static str,
    /// Call name.
    pub name: &'static str,
    /// Arguments in SCALE order.
    pub fields: &'static [FieldDescriptor],
    /// Exact argument shape.
    pub shape: ShapeHash,
    /// Recorded pallet index, used for drift reports.
    pub pallet_index: u8,
    /// Recorded call index, used for drift reports.
    pub call_index: u8,
}

/// A typed builder's arguments, encoded only by the core.
#[derive(Clone, Debug)]
pub struct Call {
    /// Descriptor checked against the selected runtime.
    pub descriptor: &'static CallDescriptor,
    /// Dynamic argument carrier accepted by the core encoder.
    pub args: Value,
    /// Stamps of nested composed calls.
    pub nested_stamps: Vec<RuntimeStamp>,
}
impl Call {
    /// Record arguments without encoding or resolving indices.
    pub fn new(
        descriptor: &'static CallDescriptor,
        args: Value,
        nested_stamps: Vec<RuntimeStamp>,
    ) -> Self {
        Self {
            descriptor,
            args,
            nested_stamps,
        }
    }
}

/// A storage entry's name, key/value shape, and missing-value semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageSurface {
    /// Pallet name.
    pub pallet: &'static str,
    /// Entry name.
    pub name: &'static str,
    /// Metadata storage prefix.
    pub prefix: &'static str,
    /// Unhashed key identities, in encoding order.
    pub key_type_idents: &'static [&'static str],
    /// Value identity.
    pub value_type_ident: &'static str,
    /// Recorded hashers for detecting behaviour drift, never key construction.
    pub hashers: &'static [&'static str],
    /// Default or Optional.
    pub modifier: &'static str,
    /// Encoded default.
    pub default_bytes: &'static [u8],
    /// Exact key/value shape.
    pub shape: ShapeHash,
}
/// A name-based typed storage descriptor.
#[derive(Clone, Copy, Debug)]
pub struct StorageDescriptor<K, V> {
    /// Portable, erased descriptor for runtime verification.
    pub surface: StorageSurface,
    marker: PhantomData<fn(K) -> V>,
}
impl<K, V> StorageDescriptor<K, V> {
    /// Wrap a complete storage surface without storing any encoded key.
    pub const fn new(surface: StorageSurface) -> Self {
        Self {
            surface,
            marker: PhantomData,
        }
    }
    /// Return the portable descriptor.
    pub const fn surface(&self) -> StorageSurface {
        self.surface
    }
    /// Bind typed keys without encoding a storage key or consulting a node.
    pub fn at(&self, keys: K) -> StorageQuery<K, V> {
        StorageQuery {
            descriptor: Self::new(self.surface),
            keys,
        }
    }
}
/// A missing-value marker for storage whose live modifier is Optional.
///
/// The client decodes present values as `V` and returns `Option<V>`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OptionalStorage<V>(PhantomData<fn() -> V>);
/// A storage descriptor bound to typed, unhashed keys.
#[derive(Clone, Debug)]
pub struct StorageQuery<K, V> {
    /// Live metadata lookup and structural contract.
    pub descriptor: StorageDescriptor<K, V>,
    /// Unhashed key values, in metadata order.
    pub keys: K,
}

/// One runtime constant and its pinned encoded value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstantSurface {
    /// Pallet name.
    pub pallet: &'static str,
    /// Constant name.
    pub name: &'static str,
    /// Metadata type identity.
    pub type_ident: &'static str,
    /// SCALE value for drift detection.
    pub value: &'static [u8],
    /// Exact type shape.
    pub shape: ShapeHash,
}
/// A typed runtime constant descriptor.
#[derive(Clone, Copy, Debug)]
pub struct ConstantDescriptor<V> {
    /// Portable, erased descriptor.
    pub surface: ConstantSurface,
    marker: PhantomData<fn() -> V>,
}
impl<V> ConstantDescriptor<V> {
    /// Record the constant's name, type, and value.
    pub const fn new(surface: ConstantSurface) -> Self {
        Self {
            surface,
            marker: PhantomData,
        }
    }
    /// Return the portable descriptor.
    pub const fn surface(&self) -> ConstantSurface {
        self.surface
    }
}

/// One runtime API method and its advertised API version.
#[derive(Clone, Copy, Debug)]
pub struct ApiSurface {
    /// API trait name.
    pub api: &'static str,
    /// Method name.
    pub name: &'static str,
    /// Inputs in encoding order.
    pub inputs: &'static [FieldDescriptor],
    /// Output identity.
    pub output_type_ident: &'static str,
    /// Advertised runtime API version from V16.
    pub version: u32,
    /// Exact input/output shape.
    pub shape: ShapeHash,
    /// Declared by-name output projection, when a hand model is configured.
    pub output_projection: Option<ProjectionFactory>,
}
/// Construct a configured by-name metadata projection without I/O.
pub type ProjectionFactory = fn() -> crate::Projection;

impl PartialEq for ApiSurface {
    fn eq(&self, other: &Self) -> bool {
        (
            self.api,
            self.name,
            self.inputs,
            self.output_type_ident,
            self.version,
            self.shape,
        ) == (
            other.api,
            other.name,
            other.inputs,
            other.output_type_ident,
            other.version,
            other.shape,
        ) && self.output_projection.map(|make| make()) == other.output_projection.map(|make| make())
    }
}
impl Eq for ApiSurface {}
/// A typed runtime API descriptor.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeApiDescriptor<I, O> {
    /// Portable, erased descriptor.
    pub surface: ApiSurface,
    marker: PhantomData<fn(I) -> O>,
}
impl<I, O> RuntimeApiDescriptor<I, O> {
    /// Record an API method by name.
    pub const fn new(surface: ApiSurface) -> Self {
        Self {
            surface,
            marker: PhantomData,
        }
    }
    /// Return the portable descriptor.
    pub const fn surface(&self) -> ApiSurface {
        self.surface
    }
}

/// A metadata event variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventDescriptor {
    /// Pallet name.
    pub pallet: &'static str,
    /// Event name.
    pub name: &'static str,
    /// Fields in declaration order.
    pub fields: &'static [FieldDescriptor],
    /// Exact event field shape.
    pub shape: ShapeHash,
    /// Recorded outer pallet index.
    pub pallet_index: u8,
    /// Recorded event variant index.
    pub event_index: u8,
}
/// An expected signed extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    /// Extension identifier.
    pub identifier: &'static str,
    /// Explicit payload identity.
    pub type_ident: &'static str,
    /// Implied payload identity.
    pub additional_signed_type_ident: &'static str,
    /// Explicit payload shape.
    pub shape: ShapeHash,
    /// Implied payload shape.
    pub additional_signed_shape: ShapeHash,
}
/// A complete module-error table row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleError {
    /// Pallet name.
    pub pallet: &'static str,
    /// Pallet index.
    pub pallet_index: u8,
    /// Error variant index.
    pub error_index: u8,
    /// Error name.
    pub name: &'static str,
    /// Metadata documentation.
    pub docs: &'static str,
}
/// A generated pallet name table row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PalletDescriptor {
    /// Pallet name.
    pub name: &'static str,
    /// Pinned outer enum index, used for drift.
    pub index: u8,
}
/// The structural contract of a hand-written by-name model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSurface {
    /// Full metadata type path.
    pub type_path: &'static str,
    /// Fields recursively projected by name.
    pub projection: crate::Projection,
    /// Pinned projection hash.
    pub shape: ShapeHash,
}
/// One semantically typed field of a runtime model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFieldSurface {
    /// Full metadata type path of the containing model.
    pub type_path: &'static str,
    /// Metadata field name.
    pub name: &'static str,
    /// Wire identity before applying the semantic override.
    pub type_ident: &'static str,
    /// Exact field shape.
    pub shape: ShapeHash,
}
/// A field whose wire identity needs a semantic type override.
#[derive(Clone, Copy, Debug)]
pub struct ModelFieldDescriptor<V> {
    /// Exact metadata field contract.
    pub surface: ModelFieldSurface,
    marker: PhantomData<fn() -> V>,
}
impl<V> ModelFieldDescriptor<V> {
    /// Associate a semantic Rust type with a checked metadata field.
    pub const fn new(surface: ModelFieldSurface) -> Self {
        Self {
            surface,
            marker: PhantomData,
        }
    }
}
/// A descriptor included in generated surface verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceDescriptor {
    /// Callable argument contract.
    Call(&'static CallDescriptor),
    /// Storage keys, value, and default behaviour.
    Storage(StorageSurface),
    /// Constant shape and value.
    Constant(ConstantSurface),
    /// Runtime API input/output contract.
    RuntimeApi(ApiSurface),
    /// Event field contract.
    Event(&'static EventDescriptor),
    /// By-name model projection.
    Model(ModelSurface),
}

/// Hash named structural parts in declaration order, used by runtime API signatures.
pub fn shape_parts(parts: &[(&str, ShapeHash)]) -> ShapeHash {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    use parity_scale_codec::Encode;
    ShapeHash(
        Blake2b::<U32>::digest(
            parts
                .iter()
                .map(|(name, shape)| (*name, shape.0))
                .collect::<Vec<_>>()
                .encode(),
        )
        .into(),
    )
}

/// Collect runtime stamps from arguments containing nested composed calls.
#[doc(hidden)]
pub trait NestedCalls {
    /// Append every contained call stamp.
    fn append_stamps(&self, output: &mut Vec<RuntimeStamp>);
}
impl NestedCalls for crate::CallBytes {
    fn append_stamps(&self, output: &mut Vec<RuntimeStamp>) {
        output.push(self.stamp());
    }
}
impl<T: NestedCalls> NestedCalls for Vec<T> {
    fn append_stamps(&self, output: &mut Vec<RuntimeStamp>) {
        for value in self {
            value.append_stamps(output);
        }
    }
}
impl<T: NestedCalls> NestedCalls for Option<T> {
    fn append_stamps(&self, output: &mut Vec<RuntimeStamp>) {
        if let Some(value) = self {
            value.append_stamps(output);
        }
    }
}
impl<T: NestedCalls, const N: usize> NestedCalls for [T; N] {
    fn append_stamps(&self, output: &mut Vec<RuntimeStamp>) {
        for value in self {
            value.append_stamps(output);
        }
    }
}
/// Return stamps from a typed nested-call argument before it is converted to `Value`.
#[doc(hidden)]
pub fn nested_call_stamps<T: NestedCalls>(value: &T) -> Vec<RuntimeStamp> {
    let mut output = Vec::new();
    value.append_stamps(&mut output);
    output
}
