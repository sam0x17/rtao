//! Typed calls compose from live metadata when their compiled shape differs
//! (D-123).
//!
//! Runtime 487 adds `Lock` (21) and `GammaSpend` (22) to `ProxyType`, which
//! changes the exact shape of every call carrying one. Encoding by variant
//! name against live metadata still works, so such calls compose from live
//! metadata with the same integrity checks instead of being refused. Only a
//! call the live metadata cannot encode, such as one using a removed variant
//! or missing an added field, refuses.
#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use rtao_core::codec::Value;
use rtao_core::runtime::Runtime;
use rtao_primitives::{AccountId, IntoCoreValue};
use rtao_runtime::{
    CallBytes, ComposeError, RuntimeHandle, calls, compose_by_name, live_compositions,
    models::ProxyType,
};
use scale_info::{Field, TypeDef, TypeDefPrimitive, Variant, form::PortableForm};

const PROXY: &str = "call:Proxy.proxy";

fn pinned() -> Runtime {
    Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/488.v15.scale"),
        488,
        1,
        42,
    )
    .unwrap()
}

fn proxy_type_variants(core: &mut Runtime) -> &mut Vec<Variant<PortableForm>> {
    let ty = core
        .types
        .types
        .iter_mut()
        .find(|ty| ty.ty.path.segments.last().map(String::as_str) == Some("ProxyType"))
        .expect("ProxyType in pinned metadata");
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("ProxyType is an enum")
    };
    &mut variants.variants
}

fn unit_variant(name: &str, index: u8) -> Variant<PortableForm> {
    Variant {
        name: name.into(),
        fields: vec![],
        index,
        docs: vec![],
    }
}

/// Pinned metadata with one more `ProxyType` variant, as runtime 487 added
/// `Lock` (21) and `GammaSpend` (22) to 486 (both are pinned since 488).
fn with_added_proxy_type() -> RuntimeHandle {
    let mut core = pinned();
    let variants = proxy_type_variants(&mut core);
    let next = variants.iter().map(|variant| variant.index).max().unwrap() + 1;
    variants.push(unit_variant("FutureProxyType", next));
    RuntimeHandle::new(core).unwrap()
}

fn proxy_call_variant<'a>(core: &'a mut Runtime, name: &str) -> &'a mut Variant<PortableForm> {
    let calls_type = core.pallet("Proxy").unwrap().calls_type.unwrap();
    let ty = core
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls_type)
        .unwrap();
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("Proxy call enum required")
    };
    variants
        .variants
        .iter_mut()
        .find(|variant| variant.name == name)
        .unwrap()
}

fn primitive(core: &Runtime, wanted: TypeDefPrimitive) -> u32 {
    core.types
        .types
        .iter()
        .find(|ty| matches!(&ty.ty.type_def, TypeDef::Primitive(p) if *p == wanted))
        .map(|ty| ty.id)
        .expect("primitive in pinned registry")
}

fn remark(runtime: &RuntimeHandle) -> CallBytes {
    calls::system::remark(b"lock-in".to_vec())
        .compose(runtime)
        .unwrap()
}

fn proxied(runtime: &RuntimeHandle, proxy_type: Option<ProxyType>) -> rtao_runtime::Call {
    calls::proxy::proxy(
        rtao_core::DynValue::new(AccountId([3; 32]).into_core_value()),
        proxy_type,
        remark(runtime),
    )
}

fn surface_refusal(result: Result<CallBytes, ComposeError>) -> String {
    match result {
        Err(ComposeError::SurfaceChanged { detail, .. }) => detail,
        other => panic!("expected a per-call refusal: {other:?}"),
    }
}

#[test]
fn an_added_enum_variant_composes_from_live_metadata() {
    let pinned = RuntimeHandle::new(pinned()).unwrap();
    let changed = with_added_proxy_type();
    assert!(pinned.surface_report().shape_matches(PROXY));
    assert!(
        !changed.surface_report().shape_matches(PROXY),
        "an added ProxyType variant changes the exact shape"
    );
    for proxy_type in [None, Some(ProxyType::Staking), Some(ProxyType::Any)] {
        let expected = proxied(&pinned, proxy_type).compose(&pinned).unwrap();
        let before = live_compositions();
        let live = proxied(&changed, proxy_type).compose(&changed).unwrap();
        assert!(live_compositions() > before, "the composition is counted");
        // Variant indices are unchanged, so the bytes are identical.
        assert_eq!(live.as_bytes(), expected.as_bytes());
        assert_eq!(live.stamp(), changed.stamp());
        // The bytes import strictly under the live metadata.
        assert_eq!(CallBytes::decode(live.as_bytes(), &changed).unwrap(), live);
    }
}

#[test]
fn a_moved_variant_encodes_with_its_live_index() {
    let mut core = pinned();
    let variants = proxy_type_variants(&mut core);
    let staking = variants.iter_mut().find(|v| v.name == "Staking").unwrap();
    let old = staking.index;
    staking.index = 200;
    let changed = RuntimeHandle::new(core).unwrap();
    let pinned = RuntimeHandle::new(pinned()).unwrap();
    let expected = proxied(&pinned, Some(ProxyType::Staking))
        .compose(&pinned)
        .unwrap();
    let live = proxied(&changed, Some(ProxyType::Staking))
        .compose(&changed)
        .unwrap();
    assert_ne!(live.as_bytes(), expected.as_bytes());
    let position = expected
        .as_bytes()
        .iter()
        .zip(live.as_bytes())
        .position(|(a, b)| a != b)
        .unwrap();
    assert_eq!(expected.as_bytes()[position], old);
    assert_eq!(live.as_bytes()[position], 200);
}

#[test]
fn a_removed_variant_the_call_uses_refuses_per_call() {
    let mut core = pinned();
    proxy_type_variants(&mut core).retain(|variant| variant.name != "Staking");
    let changed = RuntimeHandle::new(core).unwrap();
    let detail = surface_refusal(proxied(&changed, Some(ProxyType::Staking)).compose(&changed));
    assert!(
        detail.contains("composing from live metadata failed"),
        "{detail}"
    );
    assert!(detail.contains("no variant Staking"), "{detail}");
    // The same call with a variant that still exists composes.
    proxied(&changed, Some(ProxyType::Any))
        .compose(&changed)
        .unwrap();
}

#[test]
fn an_added_field_refuses_naming_it() {
    let mut core = pinned();
    let u64 = primitive(&core, TypeDefPrimitive::U64);
    proxy_call_variant(&mut core, "proxy").fields.push(Field {
        name: Some("max_fee".into()),
        ty: u64.into(),
        type_name: None,
        docs: vec![],
    });
    let changed = RuntimeHandle::new(core).unwrap();
    let detail = surface_refusal(proxied(&changed, None).compose(&changed));
    assert!(detail.contains("max_fee"), "{detail}");
}

#[test]
fn a_changed_field_type_the_value_does_not_fit_refuses() {
    let mut core = pinned();
    let boolean = primitive(&core, TypeDefPrimitive::Bool);
    let field = proxy_call_variant(&mut core, "proxy")
        .fields
        .iter_mut()
        .find(|field| field.name.as_deref() == Some("force_proxy_type"))
        .unwrap();
    field.ty = boolean.into();
    let changed = RuntimeHandle::new(core).unwrap();
    let detail = surface_refusal(proxied(&changed, Some(ProxyType::Any)).compose(&changed));
    assert!(detail.contains("force_proxy_type"), "{detail}");
}

#[test]
fn calls_without_a_compiled_descriptor_compose_by_name() {
    // A runtime adds a call this build has no descriptor for.
    let mut core = pinned();
    let mut added = proxy_call_variant(&mut core, "proxy").clone();
    added.name = "proxy_protected".into();
    added.index = 250;
    let calls_type = core.pallet("Proxy").unwrap().calls_type.unwrap();
    let ty = core
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls_type)
        .unwrap();
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("Proxy call enum required")
    };
    variants.variants.push(added);
    let runtime = RuntimeHandle::new(core).unwrap();
    let args = |extra: bool| {
        let mut fields = vec![
            ("real".to_owned(), AccountId([3; 32]).into_core_value()),
            ("force_proxy_type".to_owned(), Value::Str("Staking".into())),
            (
                "call".to_owned(),
                Value::Bytes(remark(&runtime).as_bytes().to_vec()),
            ),
        ];
        if extra {
            fields.push(("ignored".to_owned(), Value::Int(1)));
        }
        Value::record(fields)
    };
    let bytes = compose_by_name(&runtime, "Proxy", "proxy_protected", &args(false)).unwrap();
    assert_eq!(
        &bytes.as_bytes()[..2],
        &[runtime.pallet_index("Proxy").unwrap(), 250]
    );
    // The lenient core encoder would drop an unknown key; this path refuses.
    let ComposeError::SurfaceChanged { detail, .. } =
        compose_by_name(&runtime, "Proxy", "proxy_protected", &args(true)).unwrap_err()
    else {
        panic!("expected a refusal")
    };
    assert!(detail.contains("ignored"), "{detail}");
    assert!(matches!(
        compose_by_name(&runtime, "Proxy", "absent_call", &args(false)),
        Err(ComposeError::SurfaceChanged { .. })
    ));
}
