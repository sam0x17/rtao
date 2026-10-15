//! Independent submitted-HEAD audit: old binding projection and type graph guards.
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use bittensor_core::runtime::Runtime;
use codec::{Decode, Encode};
use frame_metadata::{v15, v16, RuntimeMetadata, RuntimeMetadataPrefixed};
use scale_info::{form::PortableForm, TypeDef};
use serde_json::{json, Value};

fn runtime() -> Runtime {
    let fixture: Value =
        serde_json::from_str(include_str!("../../python/tests/fixtures/golden.json")).unwrap();
    let encoded = hex::decode(
        fixture["metadata"]["v15_hex"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap();
    let bytes = Option::<Vec<u8>>::decode(&mut encoded.as_slice())
        .unwrap()
        .unwrap();
    Runtime::parse(&bytes, 461, 1, 42).unwrap()
}

fn docs(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

// Port of the original Python binding's extraction, not of the new IR code.
fn old_binding(runtime: &Runtime) -> Value {
    let pallets: Vec<Value> = runtime.pallets.iter().map(|pallet| {
        let variants = |id: Option<u32>| {
            id.map(|id| match &runtime.resolve(id).unwrap().type_def {
                TypeDef::Variant(def) => def.variants.clone(),
                _ => vec![],
            }).unwrap_or_default()
        };
        json!({
            "name": pallet.name,
            "index": pallet.index,
            "calls": variants(pallet.calls_type).iter().map(|call| json!({
                "name": call.name,
                "args": call.fields.iter().map(|field| json!({"name": field.name.clone().unwrap_or_default(), "type_ident": runtime.type_ident(field.ty.id)})).collect::<Vec<_>>(),
                "docs": docs(&call.docs),
            })).collect::<Vec<_>>(),
            "errors": variants(pallet.errors_type).iter().map(|error| json!({"name": error.name, "index": error.index, "docs": docs(&error.docs)})).collect::<Vec<_>>(),
            "storage": pallet.storage.iter().filter(|entry| !entry.name.contains(':')).map(|entry| json!({"name": entry.name, "value_type_ident": runtime.type_ident(entry.value_type)})).collect::<Vec<_>>(),
            "constants": pallet.constants.iter().map(|constant| constant.name.clone()).collect::<Vec<_>>(),
        })
    }).collect();
    json!({"spec_version": runtime.spec_version, "pallets": pallets, "runtime_apis": runtime.apis.iter().map(|api| json!({"name": api.name, "methods": api.methods.iter().map(|method| method.name.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>()})
}

fn legacy_projection(ir: &bittensor_core::ir::MetadataIr) -> Value {
    let mut value = serde_json::to_value(ir).unwrap();
    value.as_object_mut().unwrap().remove("extrinsic");
    for pallet in value["pallets"].as_array_mut().unwrap() {
        pallet.as_object_mut().unwrap().remove("constant_details");
        pallet.as_object_mut().unwrap().remove("events");
        for call in pallet["calls"].as_array_mut().unwrap() {
            call.as_object_mut().unwrap().remove("index");
            for field in call["args"].as_array_mut().unwrap() {
                field.as_object_mut().unwrap().remove("type_id");
            }
        }
        for entry in pallet["storage"].as_array_mut().unwrap() {
            entry
                .as_object_mut()
                .unwrap()
                .retain(|key, _| ["name", "value_type_ident"].contains(&key.as_str()));
        }
    }
    for api in value["runtime_apis"].as_array_mut().unwrap() {
        api.as_object_mut().unwrap().remove("version");
        api.as_object_mut().unwrap().remove("method_signatures");
    }
    value
}

#[test]
fn every_old_binding_field_matches_on_entire_pinned_golden() {
    let runtime = runtime();
    let ir = runtime.metadata_ir().unwrap();
    assert_eq!(legacy_projection(&ir), old_binding(&runtime));
    for (pallet, source) in ir.pallets.iter().zip(&runtime.pallets) {
        for (entry, source) in pallet.storage.iter().zip(
            source
                .storage
                .iter()
                .filter(|entry| !entry.name.contains(':')),
        ) {
            assert_eq!(entry.hashers, source.hashers);
            assert_eq!(entry.key_types, source.key_types);
            assert_eq!(entry.value_type, source.value_type);
            assert_eq!(entry.default_bytes, source.default_bytes);
            assert_eq!(entry.prefix, source.prefix);
            assert_eq!(entry.modifier, source.modifier);
        }
    }
    assert_eq!(
        ir.extrinsic
            .signed_extensions
            .iter()
            .map(|extension| &extension.identifier)
            .collect::<Vec<_>>(),
        runtime
            .extrinsic
            .signed_extensions
            .iter()
            .map(|extension| &extension.identifier)
            .collect::<Vec<_>>()
    );
}

fn bad_field() -> scale_info::Field<PortableForm> {
    scale_info::Field {
        name: None,
        ty: u32::MAX.into(),
        type_name: None,
        docs: vec![],
    }
}

#[test]
fn every_reachable_reference_kind_rejects_dangling_id() {
    use scale_info::*;
    let definitions: Vec<TypeDef<PortableForm>> = vec![
        TypeDefComposite {
            fields: vec![bad_field()],
        }
        .into(),
        TypeDefVariant {
            variants: vec![Variant {
                name: "Bad".into(),
                fields: vec![bad_field()],
                index: 0,
                docs: vec![],
            }],
        }
        .into(),
        TypeDefSequence {
            type_param: u32::MAX.into(),
        }
        .into(),
        TypeDefArray {
            type_param: u32::MAX.into(),
            len: 4,
        }
        .into(),
        TypeDefTuple {
            fields: vec![u32::MAX.into()],
        }
        .into(),
        TypeDefCompact {
            type_param: u32::MAX.into(),
        }
        .into(),
        TypeDefBitSequence {
            bit_store_type: u32::MAX.into(),
            bit_order_type: 0.into(),
        }
        .into(),
        TypeDefBitSequence {
            bit_store_type: 0.into(),
            bit_order_type: u32::MAX.into(),
        }
        .into(),
    ];
    for definition in definitions {
        let mut runtime = runtime();
        let output = runtime.apis[0].methods[0].output;
        runtime
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == output)
            .unwrap()
            .ty
            .type_def = definition;
        assert!(runtime.metadata_ir().is_err());
    }
    let mut runtime = runtime();
    let output = runtime.apis[0].methods[0].output;
    runtime
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == output)
        .unwrap()
        .ty
        .type_params
        .push(TypeParameter {
            name: "Bad".into(),
            ty: Some(u32::MAX.into()),
        });
    assert!(runtime.metadata_ir().is_err());
}

#[test]
fn cycles_are_safe_but_do_not_hide_other_bad_edges() {
    let mut runtime = runtime();
    let output = runtime.apis[0].methods[0].output;
    let ty = &mut runtime
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == output)
        .unwrap()
        .ty;
    ty.type_params.clear();
    ty.path.segments.clear();
    ty.type_def = scale_info::TypeDefTuple {
        fields: vec![output.into()],
    }
    .into();
    assert!(runtime.metadata_ir().is_ok());
    let ty = &mut runtime
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == output)
        .unwrap()
        .ty;
    ty.type_def = scale_info::TypeDefTuple {
        fields: vec![output.into(), u32::MAX.into()],
    }
    .into();
    assert!(runtime.metadata_ir().is_err());
}

fn v16(runtime: &Runtime) -> v16::RuntimeMetadataV16 {
    let prefixed = RuntimeMetadataPrefixed::decode(&mut runtime.metadata_bytes.as_slice()).unwrap();
    let RuntimeMetadata::V15(metadata) = prefixed.1 else {
        panic!("fixture must be V15")
    };
    v16::RuntimeMetadataV16 {
        types: metadata.types,
        pallets: vec![],
        extrinsic: v16::ExtrinsicMetadata {
            versions: vec![4],
            address_ty: metadata.extrinsic.address_ty,
            call_ty: metadata.extrinsic.call_ty,
            signature_ty: metadata.extrinsic.signature_ty,
            transaction_extensions_by_version: BTreeMap::new(),
            transaction_extensions: vec![],
        },
        apis: metadata
            .apis
            .into_iter()
            .map(|api| v16::RuntimeApiMetadata {
                name: api.name,
                docs: api.docs,
                version: 2.into(),
                deprecation_info: v16::ItemDeprecationInfo::NotDeprecated,
                methods: api
                    .methods
                    .into_iter()
                    .map(|method| v16::RuntimeApiMethodMetadata {
                        name: method.name,
                        docs: method.docs,
                        output: method.output,
                        inputs: method
                            .inputs
                            .into_iter()
                            .map(|input| v16::FunctionParamMetadata {
                                name: input.name,
                                ty: input.ty,
                            })
                            .collect(),
                        deprecation_info: v16::ItemDeprecationInfo::NotDeprecated,
                    })
                    .collect(),
            })
            .collect(),
        outer_enums: metadata.outer_enums,
        custom: metadata.custom,
    }
}

#[test]
fn v16_all_api_guards_are_atomic_and_allow_only_method_reordering() {
    let runtime = runtime();
    let mut metadata = v16(&runtime);
    for api in &mut metadata.apis {
        api.methods.reverse();
    }
    let mut ir = runtime.metadata_ir().unwrap();
    ir.set_v16_api_versions(&metadata).unwrap();
    let unchanged = ir.clone();
    let mut duplicate = metadata.clone();
    duplicate.apis.push(metadata.apis[0].clone());
    assert!(ir.set_v16_api_versions(&duplicate).is_err());
    assert_eq!(ir, unchanged);
    let mut missing = metadata.clone();
    missing.apis.pop();
    assert!(ir.set_v16_api_versions(&missing).is_err());
    assert_eq!(ir, unchanged);
    let mut methods = metadata.clone();
    let duplicate_method = methods.apis[0].methods[0].clone();
    methods.apis[0].methods.push(duplicate_method);
    assert!(ir.set_v16_api_versions(&methods).is_err());
    assert_eq!(ir, unchanged);
    let mut conflict = metadata;
    conflict.apis[0].version = 3.into();
    assert!(ir.set_v16_api_versions(&conflict).is_err());
    assert_eq!(ir, unchanged);
}

#[test]
fn single_tuple_key_stays_whole_and_plain_storage_has_no_key_components() {
    let runtime = runtime();
    let prefixed = RuntimeMetadataPrefixed::decode(&mut runtime.metadata_bytes.as_slice()).unwrap();
    let RuntimeMetadata::V15(mut metadata) = prefixed.1 else {
        panic!("fixture must be V15")
    };
    let tuple = metadata
        .types
        .types
        .iter()
        .find(|ty| matches!(&ty.ty.type_def, TypeDef::Tuple(def) if def.fields.len() == 2))
        .unwrap()
        .id;
    let value = runtime.type_id_of("u32").unwrap();
    let entries = &mut metadata
        .pallets
        .iter_mut()
        .find_map(|pallet| pallet.storage.as_mut())
        .unwrap()
        .entries;
    entries.clear();
    entries.push(v15::StorageEntryMetadata {
        name: "TupleKey".into(),
        modifier: v15::StorageEntryModifier::Optional,
        ty: v15::StorageEntryType::Map {
            hashers: vec![v15::StorageHasher::Blake2_128Concat],
            key: tuple.into(),
            value: value.into(),
        },
        default: vec![],
        docs: vec![],
    });
    entries.push(v15::StorageEntryMetadata {
        name: "Plain".into(),
        modifier: v15::StorageEntryModifier::Default,
        ty: v15::StorageEntryType::Plain(value.into()),
        default: 17u32.encode(),
        docs: vec![],
    });
    let modified = Runtime::parse(
        &RuntimeMetadataPrefixed::from(metadata).encode(),
        461,
        1,
        42,
    )
    .unwrap();
    let ir = modified.metadata_ir().unwrap();
    let entries = ir
        .pallets
        .iter()
        .find(|pallet| pallet.storage.iter().any(|entry| entry.name == "TupleKey"))
        .unwrap();
    assert_eq!(entries.storage[0].key_types, [tuple]);
    assert_eq!(
        entries.storage[0].key_type_idents,
        [modified.type_ident(tuple)]
    );
    assert_eq!(entries.storage[0].hashers, ["Blake2_128Concat"]);
    assert!(entries.storage[1].hashers.is_empty());
    assert!(entries.storage[1].key_types.is_empty());
    assert_eq!(entries.storage[1].default_bytes, 17u32.encode());
}
