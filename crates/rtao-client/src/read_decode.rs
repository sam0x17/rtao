//! Typed read decoding with dynamic leaves inside ordinary Rust containers.

use bytes::Bytes;
use rtao_core::{CoreError, DynValue, codec::Value, runtime::TypeSpec};
use rtao_runtime::RuntimeHandle;
use scale_decode::{
    DecodeAsType,
    visitor::{
        DecodeError, Visitor, decode_with_visitor,
        types::{Array, Composite, Sequence, Tuple, Variant},
    },
};
use scale_info::{PortableRegistry, TypeDef};

use crate::ClientError;

/// Decode a read value using the runtime that describes its SCALE bytes.
///
/// Implementations for containers recursively preserve dynamic leaves. A
/// `Vec<DynValue>` therefore retains each element's Python shape while a
/// `Vec<NetUid>` checks each element's typed conversion.
///
/// A hand-written model that derives [`DecodeAsType`] can implement this trait
/// by forwarding to [`decode_typed`]. Models containing [`DynValue`] can decode
/// selected fields through [`ReadValue::decode_nested`] instead.
pub trait ReadValue: Sized + Send + Sync + 'static {
    /// Decode one complete value, rejecting malformed or trailing bytes.
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError>;

    /// Decode one complete container child, preserving its nested value shape.
    ///
    /// The default forwards to [`ReadValue::decode`] so existing custom models
    /// keep their conversions. Dynamic leaves override this method because
    /// the core intentionally renders empty composites differently at the top
    /// level. Container implementations call it for each child.
    fn decode_nested(
        runtime: &RuntimeHandle,
        type_id: u32,
        bytes: &[u8],
    ) -> Result<Self, ClientError> {
        Self::decode(runtime, type_id, bytes)
    }
}

/// Decode an ordinary metadata-directed SCALE type from complete input.
///
/// The core first checks canonical SCALE encoding, recursion depth, and the
/// collection element budget. The typed decoder then performs the requested
/// conversions and must consume the complete input.
pub fn decode_typed<T: DecodeAsType>(
    runtime: &RuntimeHandle,
    type_id: u32,
    bytes: &[u8],
) -> Result<T, ClientError> {
    validate(runtime, type_id, bytes)?;
    let mut input = bytes;
    let value = T::decode_as_type(&mut input, type_id, &runtime.core().types)
        .map_err(|error| decode_error(type_id, error.to_string()))?;
    if !input.is_empty() {
        return Err(decode_error(
            type_id,
            format!("{} trailing bytes remain after typed decoding", input.len()),
        ));
    }
    Ok(value)
}

fn validate(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<(), ClientError> {
    core_value(runtime, type_id, bytes).map(|_| ())
}

fn core_value(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Value, ClientError> {
    core_spec_value(runtime, type_id, &TypeSpec::Id(type_id), bytes)
}

fn core_nested_value(
    runtime: &RuntimeHandle,
    type_id: u32,
    bytes: &[u8],
) -> Result<Value, ClientError> {
    // A core tuple adds nesting without changing the child's SCALE bytes.
    // Core decoding owns its shape, canonical encoding and resource limits.
    let value = core_spec_value(
        runtime,
        type_id,
        &TypeSpec::Tuple(vec![TypeSpec::Id(type_id)]),
        bytes,
    )?;
    if let Value::Tuple(mut fields) = value
        && fields.len() == 1
    {
        return fields
            .pop()
            .ok_or_else(|| decode_error(type_id, "nested decoding returned no tuple field"));
    }
    Err(decode_error(
        type_id,
        "nested decoding did not return a one-field tuple",
    ))
}

fn core_spec_value(
    runtime: &RuntimeHandle,
    type_id: u32,
    spec: &TypeSpec,
    bytes: &[u8],
) -> Result<Value, ClientError> {
    runtime
        .core()
        .decode_spec(spec, bytes, true)
        .map_err(|source| ClientError::Decode {
            what: format!("SCALE type {type_id}").into(),
            source: source.into(),
        })
}

fn decode_error(type_id: u32, message: impl Into<String>) -> ClientError {
    ClientError::Decode {
        what: format!("SCALE type {type_id}").into(),
        source: CoreError::Codec(message.into()),
    }
}

impl ReadValue for DynValue {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        core_value(runtime, type_id, bytes).map(Self::new)
    }

    fn decode_nested(
        runtime: &RuntimeHandle,
        type_id: u32,
        bytes: &[u8],
    ) -> Result<Self, ClientError> {
        core_nested_value(runtime, type_id, bytes).map(Self::new)
    }
}

impl ReadValue for Value {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        core_value(runtime, type_id, bytes)
    }

    fn decode_nested(
        runtime: &RuntimeHandle,
        type_id: u32,
        bytes: &[u8],
    ) -> Result<Self, ClientError> {
        core_nested_value(runtime, type_id, bytes)
    }
}

macro_rules! typed_leaves {
    ($($ty:ty),* $(,)?) => {$(
        impl ReadValue for $ty {
            fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
                decode_typed(runtime, type_id, bytes)
            }
        }
    )*};
}

typed_leaves!(
    (),
    bool,
    char,
    String,
    u8,
    u16,
    u32,
    u64,
    u128,
    i8,
    i16,
    i32,
    i64,
    i128,
    rtao_primitives::AccountId,
    rtao_primitives::DefaultAccount,
    rtao_primitives::BlockNumber,
    rtao_primitives::H256,
    rtao_primitives::MechId,
    rtao_primitives::NetUid,
    rtao_primitives::NetUidStorageIndex,
    rtao_primitives::Uid,
    rtao_primitives::TaoBalance,
    rtao_primitives::AlphaBalance,
    rtao_primitives::PriceRao,
    rtao_primitives::StakeWeight,
    rtao_primitives::CompactBytesText,
    rtao_primitives::PerU16,
    rtao_primitives::PerMillion,
    rtao_primitives::Percent,
    rtao_primitives::Perbill,
    rtao_primitives::Permill,
    rtao_primitives::U64Normalized,
    rtao_primitives::U64F64,
    rtao_primitives::U96F32,
    rtao_primitives::I32F32,
    rtao_primitives::I64F64,
    rtao_primitives::I96F32,
    rtao_runtime::models::AxonInfo,
    rtao_runtime::models::ChainIdentityV2,
    rtao_runtime::models::NeuronCertificate,
    rtao_runtime::models::DelegateInfo,
    rtao_runtime::models::StakeInfo,
    rtao_runtime::models::SimSwapResult,
    rtao_runtime::models::MinerCollateralState,
    rtao_runtime::models::LockState,
    rtao_runtime::models::SubnetPrice,
    rtao_runtime::models::MultisigInfo,
    rtao_runtime::models::ProxyDefinition,
    rtao_runtime::models::MultisigTimepoint,
    rtao_runtime::models::SubnetIdentityV3,
    rtao_runtime::models::MetagraphWire,
    rtao_runtime::models::SelectiveMetagraphWire,
);

impl ReadValue for Bytes {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        Vec::<u8>::decode(runtime, type_id, bytes).map(Self::from)
    }
}

impl<T: ReadValue> ReadValue for Vec<T> {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        let children = match children(runtime, type_id, bytes)? {
            Children::Sequence(children) | Children::Array(children) => children,
            _ => return Err(decode_error(type_id, "expected a sequence or array")),
        };
        children
            .into_iter()
            .map(|child| T::decode_nested(runtime, child.type_id, child.bytes))
            .collect()
    }
}

impl<T: ReadValue, const N: usize> ReadValue for [T; N] {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        let children = match children(runtime, type_id, bytes)? {
            Children::Sequence(children) | Children::Array(children) => children,
            _ => return Err(decode_error(type_id, "expected a sequence or array")),
        };
        if children.len() != N {
            return Err(decode_error(
                type_id,
                format!(
                    "expected an array with {N} elements, found {}",
                    children.len()
                ),
            ));
        }
        children
            .into_iter()
            .map(|child| T::decode_nested(runtime, child.type_id, child.bytes))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| decode_error(type_id, "array length changed while decoding"))
    }
}

impl<T: ReadValue> ReadValue for Option<T> {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        match children(runtime, type_id, bytes)? {
            Children::Variant {
                name: "None",
                fields,
            } if fields.is_empty() => Ok(None),
            Children::Variant {
                name: "Some",
                fields,
            } if fields.len() == 1 => {
                let child = fields
                    .into_iter()
                    .next()
                    .ok_or_else(|| decode_error(type_id, "Some has no payload"))?;
                T::decode_nested(runtime, child.type_id, child.bytes).map(Some)
            }
            _ => Err(decode_error(
                type_id,
                "expected an Option variant with its declared payload",
            )),
        }
    }
}

impl<T: ReadValue> ReadValue for Box<T> {
    fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
        T::decode(runtime, type_id, bytes).map(Self::new)
    }

    fn decode_nested(
        runtime: &RuntimeHandle,
        type_id: u32,
        bytes: &[u8],
    ) -> Result<Self, ClientError> {
        T::decode_nested(runtime, type_id, bytes).map(Self::new)
    }
}

macro_rules! tuple {
    ($length:expr; $($name:ident),+ $(,)?) => {
        impl<$($name: ReadValue),+> ReadValue for ($($name,)+) {
            fn decode(runtime: &RuntimeHandle, type_id: u32, bytes: &[u8]) -> Result<Self, ClientError> {
                let fields = match children(runtime, type_id, bytes)? {
                    Children::Tuple(fields) => fields,
                    _ => return Err(decode_error(type_id, "expected a tuple or unnamed composite")),
                };
                if fields.len() != $length {
                    return Err(decode_error(type_id, format!(
                        "expected a {}-element tuple, found {}", $length, fields.len()
                    )));
                }
                let mut fields = fields.into_iter();
                Ok(($({
                    let child = fields.next().ok_or_else(|| decode_error(type_id, "tuple field is absent"))?;
                    $name::decode_nested(runtime, child.type_id, child.bytes)?
                },)+))
            }
        }
    };
}

tuple!(1; A);
tuple!(2; A, B);
tuple!(3; A, B, C);
tuple!(4; A, B, C, D);
tuple!(5; A, B, C, D, E);
tuple!(6; A, B, C, D, E, F);

struct Child<'scale> {
    type_id: u32,
    bytes: &'scale [u8],
}

enum Children<'scale, 'resolver> {
    Sequence(Vec<Child<'scale>>),
    Array(Vec<Child<'scale>>),
    Tuple(Vec<Child<'scale>>),
    Variant {
        name: &'resolver str,
        fields: Vec<Child<'scale>>,
    },
}

fn children<'scale, 'resolver>(
    runtime: &'resolver RuntimeHandle,
    type_id: u32,
    bytes: &'scale [u8],
) -> Result<Children<'scale, 'resolver>, ClientError> {
    validate(runtime, type_id, bytes)?;
    let mut inner_id = type_id;
    // Newtype containers use their inner metadata node. Their wire bytes are
    // unchanged; named structs retain their own field semantics.
    for _ in 0..256 {
        let ty = runtime.core().resolve(inner_id).map_err(CoreError::from)?;
        if let TypeDef::Composite(composite) = &ty.type_def
            && let [field] = composite.fields.as_slice()
            && field.name.is_none()
        {
            inner_id = field.ty.id;
            continue;
        }
        let mut input = bytes;
        let children =
            decode_with_visitor(&mut input, inner_id, &runtime.core().types, ChildrenVisitor)
                .map_err(|error| decode_error(type_id, error.to_string()))?;
        if !input.is_empty() {
            return Err(decode_error(
                type_id,
                "trailing bytes after container traversal",
            ));
        }
        return Ok(children);
    }
    Err(decode_error(
        type_id,
        "container newtype nesting exceeds 256 levels",
    ))
}

struct ChildrenVisitor;

impl Visitor for ChildrenVisitor {
    type Value<'scale, 'resolver> = Children<'scale, 'resolver>;
    type Error = DecodeError;
    type TypeResolver = PortableRegistry;

    fn visit_sequence<'scale, 'resolver>(
        self,
        sequence: &mut Sequence<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, DecodeError> {
        sequence
            .map(|item| {
                let item = item?;
                Ok(Child {
                    type_id: *item.type_id(),
                    bytes: item.bytes(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Children::Sequence)
    }

    fn visit_array<'scale, 'resolver>(
        self,
        array: &mut Array<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, DecodeError> {
        array
            .map(|item| {
                let item = item?;
                Ok(Child {
                    type_id: *item.type_id(),
                    bytes: item.bytes(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Children::Array)
    }

    fn visit_tuple<'scale, 'resolver>(
        self,
        tuple: &mut Tuple<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, DecodeError> {
        tuple
            .map(|item| {
                let item = item?;
                Ok(Child {
                    type_id: *item.type_id(),
                    bytes: item.bytes(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Children::Tuple)
    }

    fn visit_composite<'scale, 'resolver>(
        self,
        composite: &mut Composite<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, DecodeError> {
        if composite.fields().iter().any(|field| field.name.is_some()) {
            return Err(DecodeError::Unexpected(
                scale_decode::visitor::Unexpected::Composite,
            ));
        }
        composite
            .map(|item| {
                let item = item?;
                Ok(Child {
                    type_id: *item.type_id(),
                    bytes: item.bytes(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Children::Tuple)
    }

    fn visit_variant<'scale, 'resolver>(
        self,
        variant: &mut Variant<'scale, 'resolver, PortableRegistry>,
        _: u32,
    ) -> Result<Self::Value<'scale, 'resolver>, DecodeError> {
        let name = variant.name();
        let fields = variant
            .fields()
            .map(|item| {
                let item = item?;
                Ok(Child {
                    type_id: *item.type_id(),
                    bytes: item.bytes(),
                })
            })
            .collect::<Result<_, DecodeError>>()?;
        Ok(Children::Variant { name, fields })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
    use parity_scale_codec::Encode;
    use rtao_primitives::{NetUid, TaoBalance};
    use scale_info::{Registry, TypeInfo, meta_type};

    /// Extend real runtime metadata with test types while retaining its pallets
    /// and extension contracts. Every inserted reference is rebased, so these
    /// helpers exercise the same runtime resolution as generated descriptors.
    fn runtime<T: TypeInfo + 'static>() -> (RuntimeHandle, u32) {
        let mut metadata = <RuntimeMetadataPrefixed as parity_scale_codec::Decode>::decode(
            &mut include_bytes!("../../../vendor/metadata/finney/478.v15.scale").as_slice(),
        )
        .unwrap();
        let RuntimeMetadata::V15(v15) = &mut metadata.1 else {
            panic!("V15 fixture required");
        };
        let offset = u32::try_from(v15.types.types.len()).unwrap();
        let mut registry = Registry::new();
        let type_id = registry.register_type(&meta_type::<T>()).id + offset;
        let mut portable: PortableRegistry = registry.into();
        for portable in &mut portable.types {
            portable.id += offset;
            for parameter in &mut portable.ty.type_params {
                if let Some(ty) = &mut parameter.ty {
                    ty.id += offset;
                }
            }
            let shift_fields =
                |fields: &mut [scale_info::Field<scale_info::form::PortableForm>]| {
                    for field in fields {
                        field.ty.id += offset;
                    }
                };
            match &mut portable.ty.type_def {
                TypeDef::Composite(composite) => shift_fields(&mut composite.fields),
                TypeDef::Variant(variants) => {
                    for variant in &mut variants.variants {
                        shift_fields(&mut variant.fields);
                    }
                }
                TypeDef::Sequence(sequence) => sequence.type_param.id += offset,
                TypeDef::Array(array) => array.type_param.id += offset,
                TypeDef::Tuple(tuple) => {
                    for field in &mut tuple.fields {
                        field.id += offset;
                    }
                }
                TypeDef::Compact(compact) => compact.type_param.id += offset,
                TypeDef::BitSequence(bits) => {
                    bits.bit_store_type.id += offset;
                    bits.bit_order_type.id += offset;
                }
                TypeDef::Primitive(_) => {}
            }
        }
        v15.types.types.extend(portable.types);
        (
            RuntimeHandle::parse(&metadata.encode(), 478, 1, 42).unwrap(),
            type_id,
        )
    }

    #[test]
    fn dynamic_leaves_keep_python_byte_shapes_inside_typed_containers() {
        type Wire = Option<Vec<(u16, Vec<u8>)>>;
        type Read = Option<Vec<(NetUid, DynValue)>>;
        let (runtime, type_id) = runtime::<Wire>();
        let bytes = Some(vec![(1_u16, b"subnet".to_vec()), (2, vec![255])]).encode();
        let values = Read::decode(&runtime, type_id, &bytes).unwrap().unwrap();
        assert_eq!(values[0].0, NetUid(1));
        assert_eq!(values[0].1.as_str(), Some("subnet"));
        assert_eq!(values[1].0, NetUid(2));
        assert_eq!(values[1].1.as_str(), Some("0xff"));
        assert!(
            Read::decode(&runtime, type_id, &Wire::None.encode())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn typed_leaf_conversions_are_checked_without_losing_wide_values() {
        let (runtime, type_id) = runtime::<u64>();
        let amount = u64::MAX.encode();
        assert_eq!(
            TaoBalance::decode(&runtime, type_id, &amount).unwrap(),
            TaoBalance(u64::MAX)
        );
        assert_eq!(
            DynValue::decode(&runtime, type_id, &amount)
                .unwrap()
                .as_u128(),
            Some(u128::from(u64::MAX))
        );
        assert!(NetUid::decode(&runtime, type_id, &amount).is_err());
        assert!(u32::decode(&runtime, type_id, &amount).is_err());
    }

    #[test]
    fn neuron_certificate_storage_decodes_present_missing_and_malformed_values() {
        use crate::Fetch;
        use rtao_primitives::AccountId;
        use rtao_runtime::{
            models::NeuronCertificate, storage::subtensor_module::NEURON_CERTIFICATES,
        };

        let runtime = RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap();
        // The P-256 generator's coordinates are a 64-byte public key, which
        // exercises the two-byte compact sequence length in the storage layout.
        let expected = NeuronCertificate {
            public_key: hex::decode(concat!(
                "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
                "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
            ))
            .unwrap(),
            algorithm: 1,
        };
        // Storage encodes public_key before algorithm; serve_axon input uses
        // the opposite order before constructing the stored certificate.
        let bytes = (&expected.public_key, expected.algorithm).encode();
        let query = || NEURON_CERTIFICATES.at((NetUid(1), AccountId([7; 32])));
        assert_eq!(
            query()
                .decode(&runtime, &[Some(Bytes::from(bytes.clone()))])
                .unwrap(),
            Some(expected)
        );
        assert_eq!(query().decode(&runtime, &[None]).unwrap(), None);
        assert!(
            query()
                .decode(
                    &runtime,
                    &[Some(Bytes::copy_from_slice(&bytes[..bytes.len() - 1]))],
                )
                .is_err()
        );
        let mut trailing = bytes;
        trailing.push(0);
        assert!(
            query()
                .decode(&runtime, &[Some(Bytes::from(trailing))])
                .is_err()
        );
    }

    #[test]
    fn compact_scalars_and_container_elements_use_the_live_wire_layout() {
        type Wire = Vec<parity_scale_codec::Compact<u16>>;
        let (runtime, type_id) = runtime::<Wire>();
        let bytes = vec![
            parity_scale_codec::Compact(1_u16),
            parity_scale_codec::Compact(1000_u16),
        ]
        .encode();
        assert_eq!(
            Vec::<NetUid>::decode(&runtime, type_id, &bytes).unwrap(),
            [NetUid(1), NetUid(1000)]
        );
    }

    #[test]
    fn arrays_have_checked_lengths_and_keep_dynamic_element_shapes() {
        let (runtime, type_id) = runtime::<[Vec<u8>; 2]>();
        let bytes = [b"a".to_vec(), vec![255]].encode();
        let values = <[DynValue; 2]>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(values[0].as_str(), Some("a"));
        assert_eq!(values[1].as_str(), Some("0xff"));
        assert!(<[DynValue; 1]>::decode(&runtime, type_id, &bytes).is_err());
    }

    #[derive(TypeInfo, Encode)]
    struct Wrapped(Vec<(u16, Vec<u8>)>);

    #[derive(TypeInfo, Encode)]
    struct Named {
        values: Vec<u16>,
    }

    #[test]
    fn unnamed_container_wrappers_unwrap_without_discarding_named_struct_semantics() {
        let (runtime, type_id) = runtime::<Wrapped>();
        let value = Wrapped(vec![(7, b"alpha".to_vec())]);
        let values = Vec::<(NetUid, DynValue)>::decode(&runtime, type_id, &value.encode()).unwrap();
        assert_eq!(values[0].0, NetUid(7));
        assert_eq!(values[0].1.as_str(), Some("alpha"));
        let (runtime, type_id) = self::runtime::<Named>();
        assert!(
            Vec::<u16>::decode(&runtime, type_id, &Named { values: vec![7] }.encode()).is_err()
        );
    }

    #[test]
    fn every_supported_tuple_arity_decodes_dynamic_and_typed_fields() {
        macro_rules! check {
            ($wire:ty, $read:ty, $value:expr, $expected:expr) => {{
                let (runtime, type_id) = runtime::<$wire>();
                let decoded = <$read>::decode(&runtime, type_id, &$value.encode()).unwrap();
                assert_eq!(decoded, $expected);
            }};
        }
        check!((u16,), (NetUid,), (1_u16,), (NetUid(1),));
        check!(
            (u16, u64),
            (NetUid, TaoBalance),
            (1_u16, 2_u64),
            (NetUid(1), TaoBalance(2))
        );
        check!(
            (u16, u64, bool),
            (NetUid, TaoBalance, bool),
            (1_u16, 2_u64, true),
            (NetUid(1), TaoBalance(2), true)
        );
        check!(
            (u16, u64, bool, u8),
            (NetUid, TaoBalance, bool, u8),
            (1_u16, 2_u64, true, 3_u8),
            (NetUid(1), TaoBalance(2), true, 3)
        );
        check!(
            (u16, u64, bool, u8, i32),
            (NetUid, TaoBalance, bool, u8, i32),
            (1_u16, 2_u64, true, 3_u8, -4_i32),
            (NetUid(1), TaoBalance(2), true, 3, -4)
        );
        check!(
            (u16, u64, bool, u8, i32, String),
            (NetUid, TaoBalance, bool, u8, i32, String),
            (1_u16, 2_u64, true, 3_u8, -4_i32, "test".to_owned()),
            (NetUid(1), TaoBalance(2), true, 3, -4, "test".to_owned())
        );
    }

    #[test]
    fn zero_width_elements_and_empty_containers_decode_without_length_guesses() {
        let (runtime, type_id) = runtime::<Vec<()>>();
        assert_eq!(
            Vec::<()>::decode(&runtime, type_id, &vec![(); 3].encode()).unwrap(),
            [(); 3]
        );
        assert!(
            Vec::<()>::decode(&runtime, type_id, &Vec::<()>::new().encode())
                .unwrap()
                .is_empty()
        );
        let (runtime, type_id) = self::runtime::<[(); 0]>();
        assert_eq!(<[(); 0]>::decode(&runtime, type_id, &[]).unwrap(), []);
    }

    #[test]
    fn malformed_and_trailing_bytes_are_rejected_for_typed_and_dynamic_reads() {
        let (runtime, type_id) = runtime::<Vec<u64>>();
        let complete = vec![1_u64, 2].encode();
        for end in 0..complete.len() {
            assert!(Vec::<u64>::decode(&runtime, type_id, &complete[..end]).is_err());
            assert!(DynValue::decode(&runtime, type_id, &complete[..end]).is_err());
        }
        let mut trailing = complete;
        trailing.push(0);
        assert!(Vec::<u64>::decode(&runtime, type_id, &trailing).is_err());
        let (runtime, type_id) = self::runtime::<bool>();
        assert!(bool::decode(&runtime, type_id, &[2]).is_err());
        let (runtime, type_id) = self::runtime::<parity_scale_codec::Compact<u16>>();
        // 1 encoded using the two-byte compact form is noncanonical.
        assert!(u16::decode(&runtime, type_id, &[5, 0]).is_err());
    }

    #[test]
    fn recursive_zero_width_metadata_is_rejected_before_container_traversal() {
        let (runtime, type_id) = runtime::<Wrapped>();
        let mut metadata = <RuntimeMetadataPrefixed as parity_scale_codec::Decode>::decode(
            &mut runtime.core().metadata_bytes.as_slice(),
        )
        .unwrap();
        let RuntimeMetadata::V15(v15) = &mut metadata.1 else {
            panic!("V15 fixture required");
        };
        let portable = v15
            .types
            .types
            .iter_mut()
            .find(|portable| portable.id == type_id)
            .unwrap();
        let TypeDef::Composite(composite) = &mut portable.ty.type_def else {
            panic!("wrapper composite required");
        };
        composite.fields[0].ty.id = type_id;
        let runtime = RuntimeHandle::parse(&metadata.encode(), 478, 1, 42).unwrap();
        assert!(
            Vec::<u16>::decode(&runtime, type_id, &[])
                .unwrap_err()
                .to_string()
                .contains("decode recursion exceeds")
        );
        assert!(
            u16::decode(&runtime, type_id, &[])
                .unwrap_err()
                .to_string()
                .contains("decode recursion exceeds")
        );
        assert!(
            DynValue::decode_nested(&runtime, type_id, &[])
                .unwrap_err()
                .to_string()
                .contains("decode recursion exceeds")
        );
    }

    #[derive(TypeInfo, Encode)]
    struct Empty {}

    #[test]
    fn nested_empty_composites_keep_dictionary_shape_in_sequences_and_arrays() {
        let (runtime, type_id) = runtime::<Vec<Empty>>();
        let bytes = vec![Empty {}, Empty {}].encode();
        let whole = DynValue::decode(&runtime, type_id, &bytes)
            .unwrap()
            .to_json();
        assert_eq!(whole, serde_json::json!([{}, {}]));
        let values = Vec::<DynValue>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(
            values.iter().map(DynValue::to_json).collect::<Vec<_>>(),
            whole.as_array().unwrap().clone()
        );
        let values = Vec::<Value>::decode(&runtime, type_id, &bytes).unwrap();
        assert!(
            values
                .iter()
                .all(|value| matches!(value, Value::Dict(fields) if fields.is_empty()))
        );

        let (runtime, type_id) = self::runtime::<[Empty; 2]>();
        let values = <[DynValue; 2]>::decode(&runtime, type_id, &[]).unwrap();
        assert_eq!(
            values.map(|value| value.to_json()),
            [serde_json::json!({}), serde_json::json!({})]
        );
        assert!(<[DynValue; 1]>::decode(&runtime, type_id, &[]).is_err());
    }

    #[test]
    fn nested_empty_composites_keep_dictionary_shape_in_options_and_tuples() {
        let (runtime, type_id) = runtime::<Option<Empty>>();
        let bytes = Some(Empty {}).encode();
        assert_eq!(
            DynValue::decode(&runtime, type_id, &bytes)
                .unwrap()
                .to_json(),
            serde_json::json!({})
        );
        assert_eq!(
            Option::<DynValue>::decode(&runtime, type_id, &bytes)
                .unwrap()
                .unwrap()
                .to_json(),
            serde_json::json!({})
        );
        assert!(
            matches!(Option::<Value>::decode(&runtime, type_id, &bytes).unwrap(), Some(Value::Dict(fields)) if fields.is_empty())
        );
        assert!(
            Option::<DynValue>::decode(&runtime, type_id, &Option::<Empty>::None.encode())
                .unwrap()
                .is_none()
        );

        let (runtime, type_id) = self::runtime::<(Empty, u16)>();
        let bytes = (Empty {}, 7_u16).encode();
        let values = <(DynValue, NetUid)>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(values.0.to_json(), serde_json::json!({}));
        assert_eq!(values.1, NetUid(7));
        let (runtime, type_id) = self::runtime::<(Empty,)>();
        let values = <(Value,)>::decode(&runtime, type_id, &[]).unwrap();
        assert!(matches!(values.0, Value::Dict(fields) if fields.is_empty()));
    }

    #[test]
    fn nested_empty_composites_keep_dictionary_shape_through_box_combinations() {
        let (runtime, type_id) = runtime::<Vec<Empty>>();
        let bytes = vec![Empty {}].encode();
        let values = Box::<Vec<Box<Box<DynValue>>>>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(values[0].to_json(), serde_json::json!({}));

        let (runtime, type_id) = self::runtime::<Option<[Empty; 2]>>();
        let bytes = Some([Empty {}, Empty {}]).encode();
        let values =
            Box::<Option<Box<[Box<DynValue>; 2]>>>::decode(&runtime, type_id, &bytes).unwrap();
        let values = values.as_ref().as_ref().unwrap();
        assert_eq!(values[0].to_json(), serde_json::json!({}));
        assert_eq!(values[1].to_json(), serde_json::json!({}));

        let (runtime, type_id) = self::runtime::<(Option<Empty>, Vec<Empty>)>();
        let bytes = (Some(Empty {}), vec![Empty {}]).encode();
        let values =
            <(Box<Option<DynValue>>, Vec<Box<Value>>)>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(
            values.0.as_ref().as_ref().unwrap().to_json(),
            serde_json::json!({})
        );
        assert!(matches!(values.1[0].as_ref(), Value::Dict(fields) if fields.is_empty()));
    }

    #[derive(Debug, PartialEq)]
    struct CustomModel(u16);

    impl ReadValue for CustomModel {
        fn decode(
            runtime: &RuntimeHandle,
            type_id: u32,
            bytes: &[u8],
        ) -> Result<Self, ClientError> {
            decode_typed(runtime, type_id, bytes).map(Self)
        }
    }

    #[test]
    fn nested_context_preserves_top_level_empty_shape_and_existing_custom_models() {
        let (runtime, type_id) = runtime::<Empty>();
        assert_eq!(
            DynValue::decode(&runtime, type_id, &[]).unwrap().to_json(),
            serde_json::json!([])
        );
        assert!(
            matches!(Value::decode(&runtime, type_id, &[]).unwrap(), Value::Tuple(fields) if fields.is_empty())
        );
        assert_eq!(
            Box::<Box<DynValue>>::decode(&runtime, type_id, &[])
                .unwrap()
                .to_json(),
            serde_json::json!([])
        );
        assert!(DynValue::decode(&runtime, type_id, &[0]).is_err());

        let (runtime, type_id) = self::runtime::<Vec<u16>>();
        assert_eq!(
            Vec::<Box<CustomModel>>::decode(&runtime, type_id, &vec![1_u16, 7].encode()).unwrap(),
            [Box::new(CustomModel(1)), Box::new(CustomModel(7))]
        );
    }

    #[test]
    fn nested_dynamic_shapes_keep_nonempty_composites_and_unit_tuples_distinct() {
        let (runtime, type_id) = runtime::<Vec<Named>>();
        let bytes = vec![Named { values: vec![7] }].encode();
        let values = Vec::<DynValue>::decode(&runtime, type_id, &bytes).unwrap();
        assert_eq!(values[0].to_json(), serde_json::json!({"values": [7]}));
        assert_eq!(
            DynValue::decode(&runtime, type_id, &bytes)
                .unwrap()
                .to_json(),
            serde_json::json!([{"values": [7]}])
        );

        let (runtime, type_id) = self::runtime::<Vec<()>>();
        let values = Vec::<DynValue>::decode(&runtime, type_id, &vec![(); 2].encode()).unwrap();
        assert_eq!(
            values.iter().map(DynValue::to_json).collect::<Vec<_>>(),
            [serde_json::json!([]), serde_json::json!([])]
        );
    }

    #[test]
    fn nested_dynamic_containers_keep_strict_encoding_and_element_limits() {
        let (runtime, type_id) = runtime::<Vec<Empty>>();
        for bytes in [vec![], vec![4, 0], vec![5, 0]] {
            assert!(Vec::<DynValue>::decode(&runtime, type_id, &bytes).is_err());
            assert!(Vec::<Value>::decode(&runtime, type_id, &bytes).is_err());
        }
        // The pinned core caps zero-width elements at its per-input budget.
        let over_budget = parity_scale_codec::Compact((1_u32 << 20) + 1).encode();
        assert!(
            Vec::<DynValue>::decode(&runtime, type_id, &over_budget)
                .unwrap_err()
                .to_string()
                .contains("element budget")
        );
        let (runtime, type_id) = self::runtime::<Option<Empty>>();
        assert!(Option::<DynValue>::decode(&runtime, type_id, &[2]).is_err());
        assert!(Option::<DynValue>::decode(&runtime, type_id, &[1, 0]).is_err());
        let (runtime, type_id) = self::runtime::<Vec<bool>>();
        assert!(Vec::<DynValue>::decode(&runtime, type_id, &[4, 2]).is_err());
    }

    #[test]
    fn nested_dynamic_leaf_entry_points_keep_strict_validation() {
        let (runtime, type_id) = runtime::<Empty>();
        assert_eq!(
            DynValue::decode_nested(&runtime, type_id, &[])
                .unwrap()
                .to_json(),
            serde_json::json!({})
        );
        assert!(DynValue::decode_nested(&runtime, type_id, &[0]).is_err());
        assert!(Value::decode_nested(&runtime, type_id, &[0]).is_err());

        let (runtime, type_id) = self::runtime::<bool>();
        assert!(Value::decode_nested(&runtime, type_id, &[2]).is_err());
        assert!(DynValue::decode_nested(&runtime, type_id, &[1, 0]).is_err());
        let (runtime, type_id) = self::runtime::<parity_scale_codec::Compact<u16>>();
        assert!(DynValue::decode_nested(&runtime, type_id, &[5, 0]).is_err());
    }
}
