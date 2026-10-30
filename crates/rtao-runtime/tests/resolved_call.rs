//! Name-resolved call composition validated by structural shape (D-088).
//!
//! Benign argument changes (reordered fields, a widened integer, a
//! single-field newtype, `BoundedVec` versus `Vec`, a compact switch) still
//! compose and decode back to the supplied values. Real incompatibilities
//! refuse per field.
#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use rtao_core::runtime::Runtime;
use rtao_primitives::{MechId, NetUid};
use rtao_runtime::{
    ArgClass, ArgValue, CallBytes, ComposeError, ResolvedArg, RuntimeHandle, calls, check_resolved,
    compose_resolved, inspect_call,
};
use scale_info::{
    Field, Path, PortableType, Type, TypeDef, TypeDefArray, TypeDefCompact, TypeDefComposite,
    TypeDefPrimitive, TypeDefVariant, Variant, form::PortableForm,
};

const PALLET: &str = "SubtensorModule";
const SET: &str = "set_mechanism_weights";
const COMMIT: &str = "commit_timelocked_mechanism_weights";

fn pinned() -> Runtime {
    Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/486.v15.scale"),
        486,
        1,
        42,
    )
    .unwrap()
}

fn set_args() -> Vec<ResolvedArg<'static>> {
    vec![
        ResolvedArg {
            name: "netuid",
            class: ArgClass::Uint(16),
            value: ArgValue::Uint(7),
        },
        ResolvedArg {
            name: "mecid",
            class: ArgClass::Uint(8),
            value: ArgValue::Uint(1),
        },
        ResolvedArg {
            name: "dests",
            class: ArgClass::Seq(Box::new(ArgClass::Uint(16))),
            value: ArgValue::uints(&[0u16, 3]),
        },
        ResolvedArg {
            name: "weights",
            class: ArgClass::Seq(Box::new(ArgClass::Uint(16))),
            value: ArgValue::uints(&[u16::MAX, 1]),
        },
        ResolvedArg {
            name: "version_key",
            class: ArgClass::Uint(64),
            value: ArgValue::Uint(9),
        },
    ]
}

fn commit_args() -> Vec<ResolvedArg<'static>> {
    vec![
        ResolvedArg {
            name: "netuid",
            class: ArgClass::Uint(16),
            value: ArgValue::Uint(7),
        },
        ResolvedArg {
            name: "mecid",
            class: ArgClass::Uint(8),
            value: ArgValue::Uint(1),
        },
        ResolvedArg {
            name: "commit",
            class: ArgClass::Bytes,
            value: ArgValue::Bytes(vec![1, 2, 3, 0x30, 0x78]),
        },
        ResolvedArg {
            name: "reveal_round",
            class: ArgClass::Uint(64),
            value: ArgValue::Uint(123_456),
        },
        ResolvedArg {
            name: "commit_reveal_version",
            class: ArgClass::Uint(16),
            value: ArgValue::Uint(4),
        },
    ]
}

fn classes(args: &[ResolvedArg<'static>]) -> Vec<(&'static str, ArgClass)> {
    args.iter()
        .map(|arg| (arg.name, arg.class.clone()))
        .collect()
}

fn add_type(core: &mut Runtime, type_def: TypeDef<PortableForm>, path: &[&str]) -> u32 {
    let id = u32::try_from(core.types.types.len()).unwrap();
    core.types.types.push(PortableType {
        id,
        ty: Type {
            path: Path {
                segments: path.iter().map(|s| (*s).to_owned()).collect(),
            },
            type_params: vec![],
            type_def,
            docs: vec![],
        },
    });
    id
}

fn primitive(core: &Runtime, wanted: TypeDefPrimitive) -> u32 {
    core.types
        .types
        .iter()
        .find(|ty| matches!(&ty.ty.type_def, TypeDef::Primitive(p) if *p == wanted))
        .map(|ty| ty.id)
        .expect("primitive in pinned registry")
}

fn field(name: Option<&str>, ty: u32) -> Field<PortableForm> {
    Field {
        name: name.map(Into::into),
        ty: ty.into(),
        type_name: None,
        docs: vec![],
    }
}

fn variant<'a>(core: &'a mut Runtime, call: &str) -> &'a mut Variant<PortableForm> {
    let calls_type = core.pallet(PALLET).unwrap().calls_type.unwrap();
    let ty = core
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls_type)
        .unwrap();
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("SubtensorModule call enum required")
    };
    variants
        .variants
        .iter_mut()
        .find(|variant| variant.name == call)
        .unwrap()
}

fn field_type(core: &mut Runtime, call: &str, name: &str) -> u32 {
    variant(core, call)
        .fields
        .iter()
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap()
        .ty
        .id
}

fn retype(core: &mut Runtime, call: &str, name: &str, ty: u32) {
    let target = variant(core, call)
        .fields
        .iter_mut()
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap();
    target.ty = ty.into();
}

fn changed(call: &str, edit: impl FnOnce(&mut Runtime)) -> RuntimeHandle {
    let mut core = pinned();
    edit(&mut core);
    let runtime = RuntimeHandle::new(core).unwrap();
    let descriptor = format!("call:{PALLET}.{call}");
    assert!(
        !runtime.surface_report().shape_matches(&descriptor),
        "the edit must change the compiled shape"
    );
    runtime
}

fn decoded(runtime: &RuntimeHandle, bytes: &CallBytes, args: &[ResolvedArg<'static>]) {
    let inspected = inspect_call(runtime, bytes, &classes(args)).unwrap();
    assert_eq!(inspected.pallet, PALLET);
    for arg in args {
        let field = inspected.field(arg.name).unwrap();
        assert_eq!(field.value.as_ref(), Some(&arg.value), "{}", arg.name);
        assert!(!field.nests_call);
    }
}

fn refused(result: Result<CallBytes, ComposeError>, field: &str) -> String {
    match result {
        Err(ComposeError::ResolvedIncompatible {
            field: Some(name),
            detail,
            ..
        }) if name == field => detail,
        other => panic!("expected a refusal naming {field}: {other:?}"),
    }
}

#[test]
fn unchanged_calls_compose_exactly_as_the_compiled_builders() {
    let runtime = RuntimeHandle::new(pinned()).unwrap();
    let set = compose_resolved(&runtime, PALLET, SET, &set_args()).unwrap();
    let typed = calls::subtensor_module::set_mechanism_weights(
        NetUid(7),
        MechId(1),
        vec![0, 3],
        vec![u16::MAX, 1],
        9,
    )
    .compose(&runtime)
    .unwrap();
    assert_eq!(set, typed);
    let commit = compose_resolved(&runtime, PALLET, COMMIT, &commit_args()).unwrap();
    let typed = calls::subtensor_module::commit_timelocked_mechanism_weights(
        NetUid(7),
        MechId(1),
        vec![1, 2, 3, 0x30, 0x78],
        123_456,
        4,
    )
    .compose(&runtime)
    .unwrap();
    assert_eq!(commit, typed);
    check_resolved(&runtime, PALLET, SET, &classes(&set_args())).unwrap();
    check_resolved(&runtime, PALLET, COMMIT, &classes(&commit_args())).unwrap();
    decoded(&runtime, &set, &set_args());
    decoded(&runtime, &commit, &commit_args());
}

#[test]
fn reordered_fields_compose_in_live_order() {
    let runtime = changed(SET, |core| {
        variant(core, SET).fields.swap(2, 4);
    });
    assert!(matches!(
        calls::subtensor_module::set_mechanism_weights(NetUid(7), MechId(1), vec![], vec![], 9)
            .compose(&runtime),
        Err(ComposeError::SurfaceChanged { .. })
    ));
    let bytes = compose_resolved(&runtime, PALLET, SET, &set_args()).unwrap();
    decoded(&runtime, &bytes, &set_args());
}

#[test]
fn widened_integer_composes_and_decodes_back() {
    let runtime = changed(SET, |core| {
        let u128 = primitive(core, TypeDefPrimitive::U128);
        retype(core, SET, "version_key", u128);
    });
    let bytes = compose_resolved(&runtime, PALLET, SET, &set_args()).unwrap();
    decoded(&runtime, &bytes, &set_args());
    let original = compose_resolved(
        &RuntimeHandle::new(pinned()).unwrap(),
        PALLET,
        SET,
        &set_args(),
    )
    .unwrap();
    assert_eq!(bytes.as_bytes().len(), original.as_bytes().len() + 8);
}

#[test]
fn single_field_newtypes_compose_through_named_and_unnamed_fields() {
    for named in [true, false] {
        let runtime = changed(SET, |core| {
            let netuid = field_type(core, SET, "netuid");
            let wrapper = add_type(
                core,
                TypeDef::Composite(TypeDefComposite {
                    fields: vec![field(named.then_some("inner"), netuid)],
                }),
                &["fixture", "Wrapped"],
            );
            retype(core, SET, "netuid", wrapper);
            // A sequence inside an unnamed newtype, like `BoundedVec<u16, _>`.
            let dests = field_type(core, SET, "dests");
            let bounded = add_type(
                core,
                TypeDef::Composite(TypeDefComposite {
                    fields: vec![field(None, dests)],
                }),
                &["bounded_collections", "BoundedVec"],
            );
            retype(core, SET, "dests", bounded);
        });
        // One element exercises the core's legacy newtype list level.
        let mut args = set_args();
        args[2].value = ArgValue::uints(&[5u16]);
        args[3].value = ArgValue::uints(&[1u16]);
        let bytes = compose_resolved(&runtime, PALLET, SET, &args).unwrap();
        decoded(&runtime, &bytes, &args);
        let original =
            compose_resolved(&RuntimeHandle::new(pinned()).unwrap(), PALLET, SET, &args).unwrap();
        assert_eq!(bytes.as_bytes(), original.as_bytes());
    }
}

#[test]
fn bounded_versus_plain_byte_sequences_compose_identically() {
    let runtime = changed(COMMIT, |core| {
        let bounded = field_type(core, COMMIT, "commit");
        let TypeDef::Composite(composite) = &core.resolve(bounded).unwrap().type_def else {
            panic!("commit is a BoundedVec newtype")
        };
        let inner = composite.fields[0].ty.id;
        retype(core, COMMIT, "commit", inner);
    });
    let bytes = compose_resolved(&runtime, PALLET, COMMIT, &commit_args()).unwrap();
    decoded(&runtime, &bytes, &commit_args());
    let original = compose_resolved(
        &RuntimeHandle::new(pinned()).unwrap(),
        PALLET,
        COMMIT,
        &commit_args(),
    )
    .unwrap();
    assert_eq!(bytes.as_bytes(), original.as_bytes());
}

#[test]
fn compact_switch_composes_and_decodes_back() {
    let runtime = changed(COMMIT, |core| {
        let u64 = primitive(core, TypeDefPrimitive::U64);
        let compact = add_type(
            core,
            TypeDef::Compact(TypeDefCompact {
                type_param: u64.into(),
            }),
            &[],
        );
        retype(core, COMMIT, "reveal_round", compact);
    });
    let bytes = compose_resolved(&runtime, PALLET, COMMIT, &commit_args()).unwrap();
    decoded(&runtime, &bytes, &commit_args());
}

#[test]
fn added_removed_and_renamed_fields_refuse_per_field() {
    let runtime = changed(SET, |core| {
        let u64 = primitive(core, TypeDefPrimitive::U64);
        variant(core, SET).fields.push(field(Some("extra"), u64));
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "extra",
    );
    assert!(detail.contains("added or renamed"), "{detail}");
    assert!(check_resolved(&runtime, PALLET, SET, &classes(&set_args())).is_err());

    let runtime = changed(SET, |core| {
        variant(core, SET).fields.pop();
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "version_key",
    );
    assert!(detail.contains("removed or renamed"), "{detail}");

    let runtime = changed(SET, |core| {
        let last = variant(core, SET).fields.last_mut().unwrap();
        last.name = Some("version".into());
    });
    refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "version",
    );
}

#[test]
fn changed_value_types_refuse_per_field() {
    // u16 -> Option<u16>
    let runtime = changed(COMMIT, |core| {
        let u16 = primitive(core, TypeDefPrimitive::U16);
        let option = add_type(
            core,
            TypeDef::Variant(TypeDefVariant {
                variants: vec![
                    Variant {
                        name: "None".into(),
                        fields: vec![],
                        index: 0,
                        docs: vec![],
                    },
                    Variant {
                        name: "Some".into(),
                        fields: vec![field(None, u16)],
                        index: 1,
                        docs: vec![],
                    },
                ],
            }),
            &["Option"],
        );
        retype(core, COMMIT, "commit_reveal_version", option);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, COMMIT, &commit_args()),
        "commit_reveal_version",
    );
    assert!(detail.contains("Option"), "{detail}");

    // u16 -> an enum
    let runtime = changed(COMMIT, |core| {
        let mode = add_type(
            core,
            TypeDef::Variant(TypeDefVariant {
                variants: vec![Variant {
                    name: "V4".into(),
                    fields: vec![],
                    index: 0,
                    docs: vec![],
                }],
            }),
            &["fixture", "Mode"],
        );
        retype(core, COMMIT, "commit_reveal_version", mode);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, COMMIT, &commit_args()),
        "commit_reveal_version",
    );
    assert!(detail.contains("enum"), "{detail}");

    // NetUid(u16) -> u8
    let runtime = changed(SET, |core| {
        let u8 = primitive(core, TypeDefPrimitive::U8);
        retype(core, SET, "netuid", u8);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "netuid",
    );
    assert!(detail.contains("narrowed to u8"), "{detail}");

    // u64 -> i64
    let runtime = changed(SET, |core| {
        let i64 = primitive(core, TypeDefPrimitive::I64);
        retype(core, SET, "version_key", i64);
    });
    refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "version_key",
    );

    // Vec<u16> -> [u16; 2]
    let runtime = changed(SET, |core| {
        let u16 = primitive(core, TypeDefPrimitive::U16);
        let array = add_type(
            core,
            TypeDef::Array(TypeDefArray {
                len: 2,
                type_param: u16.into(),
            }),
            &[],
        );
        retype(core, SET, "dests", array);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "dests",
    );
    assert!(detail.contains("fixed array"), "{detail}");

    // a nested runtime call
    let runtime = changed(SET, |core| {
        let call = core.extrinsic.call_type.unwrap();
        retype(core, SET, "version_key", call);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        "version_key",
    );
    assert!(detail.contains("nested runtime call"), "{detail}");
}

#[test]
fn supplied_arguments_must_match_the_live_call_and_their_classes() {
    let runtime = RuntimeHandle::new(pinned()).unwrap();
    // The core encoder ignores extra dictionary keys; resolution does not.
    let mut extra = set_args();
    extra.push(ResolvedArg {
        name: "hotkey",
        class: ArgClass::Uint(8),
        value: ArgValue::Uint(1),
    });
    let detail = refused(compose_resolved(&runtime, PALLET, SET, &extra), "hotkey");
    assert!(detail.contains("absent from the live call"), "{detail}");

    let mut duplicate = set_args();
    duplicate.push(duplicate[0].clone());
    refused(
        compose_resolved(&runtime, PALLET, SET, &duplicate),
        "netuid",
    );

    let mut wide = set_args();
    wide[0].value = ArgValue::Uint(70_000);
    let detail = refused(compose_resolved(&runtime, PALLET, SET, &wide), "netuid");
    assert!(detail.contains("supplied value"), "{detail}");

    let mut wrong = set_args();
    wrong[2].value = ArgValue::Bytes(vec![1]);
    refused(compose_resolved(&runtime, PALLET, SET, &wrong), "dests");

    assert!(matches!(
        compose_resolved(&runtime, PALLET, "no_such_call", &set_args()),
        Err(ComposeError::ResolvedIncompatible { field: None, .. })
    ));
}

#[test]
fn disabled_signing_refuses_as_typed_composition_does() {
    let mut core = pinned();
    core.extrinsic.version = 5;
    let runtime = RuntimeHandle::new(core).unwrap();
    assert!(matches!(
        compose_resolved(&runtime, PALLET, SET, &set_args()),
        Err(ComposeError::SigningDisabled(_))
    ));
    assert!(matches!(
        check_resolved(&runtime, PALLET, SET, &classes(&set_args())),
        Err(ComposeError::SigningDisabled(_))
    ));
}

#[test]
fn inspection_reports_nested_calls_and_refuses_untyped_fields() {
    let runtime = RuntimeHandle::new(pinned()).unwrap();
    let remark = calls::system::remark(vec![1, 2]).compose(&runtime).unwrap();
    let batch = calls::utility::batch(vec![remark])
        .compose(&runtime)
        .unwrap();
    let inspected = inspect_call(&runtime, &batch, &[]).unwrap();
    assert_eq!(inspected.pallet, "Utility");
    assert_eq!(inspected.call, "batch");
    assert!(inspected.fields.iter().any(|field| field.nests_call));

    // A field named in the classes must fit its class.
    let set = compose_resolved(&runtime, PALLET, SET, &set_args()).unwrap();
    let error = inspect_call(&runtime, &set, &[("dests", ArgClass::Bytes)]).unwrap_err();
    assert!(
        matches!(&error, ComposeError::ResolvedIncompatible { field: Some(f), .. } if f == "dests"),
        "{error:?}"
    );
    // Unnamed classes are skipped by the core decoder.
    let only = inspect_call(&runtime, &set, &[("netuid", ArgClass::Uint(16))]).unwrap();
    assert_eq!(only.field("netuid").unwrap().value, Some(ArgValue::Uint(7)));
    assert_eq!(only.field("dests").unwrap().value, None);
}

#[test]
fn integers_that_became_ratios_or_fixed_point_refuse_instead_of_encoding_raw_parts() {
    // A single-field newtype keeps its integer's meaning, but a ratio or a
    // fixed-point number scales it: encoding the old integer as its raw parts
    // would sign a different value.
    let cases: [(&[&str], Option<&str>, &str); 3] = [
        (
            &["sp_arithmetic", "per_things", "Permill"],
            None,
            "per-thing ratio (sp_arithmetic::per_things::Permill)",
        ),
        (
            &["sp_arithmetic", "fixed_point", "FixedU128"],
            None,
            "fixed-point number (sp_arithmetic::fixed_point::FixedU128)",
        ),
        (
            &["substrate_fixed", "FixedU128"],
            Some("bits"),
            "fixed-point number (substrate_fixed::FixedU128)",
        ),
    ];
    for (path, name, expected) in cases {
        let runtime = changed(SET, |core| {
            let u64 = primitive(core, TypeDefPrimitive::U64);
            let scaled = add_type(
                core,
                TypeDef::Composite(TypeDefComposite {
                    fields: vec![field(name, u64)],
                }),
                path,
            );
            retype(core, SET, "version_key", scaled);
        });
        let detail = refused(
            compose_resolved(&runtime, PALLET, SET, &set_args()),
            "version_key",
        );
        assert!(detail.contains(expected), "{detail}");
    }
    let runtime = changed(COMMIT, |core| {
        let u64 = primitive(core, TypeDefPrimitive::U64);
        let ratio = add_type(
            core,
            TypeDef::Composite(TypeDefComposite {
                fields: vec![field(None, u64)],
            }),
            &["sp_arithmetic", "per_things", "Perbill"],
        );
        let compact = add_type(
            core,
            TypeDef::Compact(TypeDefCompact {
                type_param: ratio.into(),
            }),
            &[],
        );
        retype(core, COMMIT, "reveal_round", compact);
    });
    let detail = refused(
        compose_resolved(&runtime, PALLET, COMMIT, &commit_args()),
        "reveal_round",
    );
    assert!(
        detail.contains("compact per-thing ratio (sp_arithmetic::per_things::Perbill)"),
        "{detail}"
    );
}
