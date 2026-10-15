//! Strict call import uses core encoding without losing nested carrier shapes.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use parity_scale_codec::{Decode, Encode};
use scale_info::{PortableRegistry, Registry, TypeDef, TypeInfo};

const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");

#[derive(TypeInfo, Encode)]
struct NestedSequences(Vec<Vec<u16>>);
#[derive(TypeInfo, Encode)]
struct NestedArrays(Vec<[u16; 2]>);
#[derive(TypeInfo, Encode)]
struct U256([u64; 4]);
#[derive(TypeInfo, Encode)]
struct ZeroWidthLimbs {
    padding: [u8; 31],
    elements: Vec<()>,
}
#[derive(TypeInfo, Encode)]
struct ZeroWidthU256(ZeroWidthLimbs);

#[derive(TypeInfo, Encode)]
enum FixtureCalls {
    NestedOption(Option<Option<u16>>),
    Sequences(NestedSequences),
    Arrays(NestedArrays),
    OptionalSequences(Option<NestedSequences>),
    FullWidth(U256),
    RepeatedZeroWidth(Vec<ZeroWidthU256>),
}
#[derive(TypeInfo, Encode)]
enum FixtureOuter {
    System(FixtureCalls),
}

fn fixture(edit: impl FnOnce(&mut PortableRegistry)) -> RuntimeHandle {
    let mut registry = Registry::new();
    let calls = registry
        .register_type(&scale_info::meta_type::<FixtureCalls>())
        .id;
    let outer = registry
        .register_type(&scale_info::meta_type::<FixtureOuter>())
        .id;
    let unit = registry.register_type(&scale_info::meta_type::<()>()).id;
    let mut types: PortableRegistry = registry.into();
    edit(&mut types);
    let RuntimeMetadata::V15(mut metadata) = RuntimeMetadataPrefixed::decode(&mut &METADATA[..])
        .unwrap()
        .1
    else {
        panic!("V15 fixture required")
    };
    // Build a complete metadata fixture rather than using an encoder with a
    // detached registry. All values below are independently SCALE-derived.
    metadata.types = types;
    metadata.pallets.retain(|pallet| pallet.name == "System");
    let system = metadata.pallets.first_mut().unwrap();
    assert_eq!(system.index, 0);
    system.calls.as_mut().unwrap().ty = calls.into();
    system.storage = None;
    system.event = None;
    system.error = None;
    system.constants.clear();
    metadata.extrinsic.call_ty = outer.into();
    metadata.extrinsic.address_ty = unit.into();
    metadata.extrinsic.signature_ty = unit.into();
    metadata.extrinsic.extra_ty = unit.into();
    metadata.extrinsic.signed_extensions.clear();
    metadata.apis.clear();
    metadata.ty = outer.into();
    metadata.outer_enums.call_enum_ty = outer.into();
    metadata.outer_enums.event_enum_ty = outer.into();
    metadata.outer_enums.error_enum_ty = outer.into();
    metadata.custom.map.clear();
    let encoded = RuntimeMetadataPrefixed(
        frame_metadata::META_RESERVED,
        RuntimeMetadata::V15(metadata),
    )
    .encode();
    RuntimeHandle::parse(&encoded, 478, 1, 42).unwrap()
}

#[test]
fn nested_options_and_sequence_newtypes_preserve_complete_call_bytes() {
    let runtime = fixture(|_| {});
    let calls = [
        FixtureCalls::NestedOption(None),
        FixtureCalls::NestedOption(Some(None)),
        FixtureCalls::NestedOption(Some(Some(7))),
        FixtureCalls::Sequences(NestedSequences(vec![])),
        FixtureCalls::Sequences(NestedSequences(vec![vec![]])),
        FixtureCalls::Sequences(NestedSequences(vec![vec![1, 2]])),
        FixtureCalls::Sequences(NestedSequences(vec![vec![1], vec![2, 3]])),
        FixtureCalls::Arrays(NestedArrays(vec![[1, 2]])),
        FixtureCalls::OptionalSequences(Some(NestedSequences(vec![vec![1, 2]]))),
    ];
    let original_metadata = runtime.core().metadata_bytes.clone();
    let original_types = runtime.core().types.clone();
    for call in calls {
        let bytes = FixtureOuter::System(call).encode();
        let imported = CallBytes::decode(&bytes, &runtime).unwrap();
        assert_eq!(imported.as_bytes(), bytes);
        assert_eq!(imported.stamp(), runtime.stamp());
        imported.ensure_runtime(&runtime).unwrap();
    }
    assert_eq!(runtime.core().metadata_bytes, original_metadata);
    assert_eq!(runtime.core().types, original_types);
}

#[test]
fn private_encoder_changes_only_option_presentation_paths() {
    let runtime = fixture(|_| {});
    let encoder = call_import_encoder(runtime.core()).unwrap();
    let mut changed = 0;
    for (original, projected) in runtime.core().types.types.iter().zip(&encoder.types.types) {
        let mut expected = original.clone();
        if expected.ty.path.segments.as_slice() == ["Option"] {
            expected
                .ty
                .path
                .segments
                .insert(0, "rtao_call_import".into());
            changed += 1;
        }
        assert_eq!(&expected, projected);
    }
    assert!(changed >= 2);
    assert_eq!(encoder.metadata_bytes, runtime.core().metadata_bytes);
    assert_eq!(encoder.spec_version, runtime.spec_version());
    assert_eq!(encoder.transaction_version, runtime.transaction_version());
}

#[test]
fn self_referential_u256_metadata_is_stopped_by_the_bridge_budget() {
    let runtime = fixture(|types| {
        let ty = types
            .types
            .iter_mut()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "U256")
            })
            .unwrap();
        ty.ty.path.segments = vec!["primitive_types".into(), "U256".into()];
        let TypeDef::Composite(composite) = &mut ty.ty.type_def else {
            panic!("U256 composite required")
        };
        composite.fields[0].ty = ty.id.into();
    });
    let bytes = FixtureOuter::System(FixtureCalls::FullWidth(U256([7; 4]))).encode();
    // The original strict decoder deliberately treats U256 as a fixed scalar.
    // Its success cannot stand in for the bridge's independent recursion bound.
    runtime
        .core()
        .decode_spec(&rtao_core::runtime::TypeSpec::Call, &bytes, true)
        .unwrap();
    let error = CallBytes::decode(&bytes, &runtime).unwrap_err();
    assert!(error.to_string().contains("call import recursion exceeds"));
}

#[test]
fn element_budget_is_shared_across_fresh_u256_limb_cursors() {
    let runtime = fixture(|types| {
        let ty = types
            .types
            .iter_mut()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "ZeroWidthU256")
            })
            .unwrap();
        ty.ty.path.segments = vec!["primitive_types".into(), "U256".into()];
    });
    let bytes = FixtureOuter::System(FixtureCalls::RepeatedZeroWidth(
        (0..3)
            .map(|_| {
                ZeroWidthU256(ZeroWidthLimbs {
                    padding: [0; 31],
                    elements: vec![(); 3],
                })
            })
            .collect(),
    ))
    .encode();
    let core = runtime.core();
    core.decode_spec(&rtao_core::runtime::TypeSpec::Call, &bytes, true)
        .unwrap();
    // Three outer elements and three zero-width elements per fresh cursor.
    // A small injected bound tests cumulative charging without large fixtures.
    for (remaining, succeeds) in [(11, false), (12, true)] {
        let mut budget = CallBridgeBudget {
            depth: 0,
            elements_remaining: remaining,
        };
        let mut cursor = rtao_core::codec::decode::Cursor::new(&bytes);
        cursor.strict = true;
        let result = canonical_scale_value(
            core,
            core.extrinsic.call_type.unwrap(),
            &mut cursor,
            &mut budget,
        );
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(budget.depth, 0);
        if succeeds {
            let value = result.unwrap();
            assert_eq!(cursor.remaining(), 0);
            assert_eq!(budget.elements_remaining, 0);
            assert_eq!(
                call_import_encoder(core)
                    .unwrap()
                    .encode_spec(&rtao_core::runtime::TypeSpec::Call, &value)
                    .unwrap(),
                bytes
            );
        } else {
            assert!(result.unwrap_err().to_string().contains("element budget"));
        }
    }
    assert_eq!(
        CallBytes::decode(&bytes, &runtime).unwrap().as_bytes(),
        bytes
    );
}
