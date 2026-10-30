//! A moved call index with an unchanged argument contract must still compose.
//!
//! Composition resolves the call by name and encodes the live pallet and call
//! indices, so a newer runtime that only renumbers a call stays usable. A real
//! argument change must still be refused.
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_core::runtime::Runtime;
use rtao_primitives::{MechId, NetUid};
use rtao_runtime::{
    ComposeError, RuntimeHandle, SurfaceClass, SurfaceDescriptor, SurfaceKind, calls,
    verify_surface_with,
};
use scale_info::TypeDef;

const DESCRIPTOR: &str = "call:SubtensorModule.set_mechanism_weights";

fn pinned() -> Runtime {
    Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/486.v15.scale"),
        486,
        1,
        42,
    )
    .unwrap()
}

fn set_weights() -> rtao_runtime::Call {
    calls::subtensor_module::set_mechanism_weights(
        NetUid(7),
        MechId(0),
        vec![0, 3],
        vec![u16::MAX, 1],
        9,
    )
}

/// Edit the live `SubtensorModule.set_mechanism_weights` variant in place.
fn with_variant(
    edit: impl FnOnce(&mut Vec<scale_info::Variant<scale_info::form::PortableForm>>),
) -> RuntimeHandle {
    let mut core = pinned();
    let calls_type = core.pallet("SubtensorModule").unwrap().calls_type.unwrap();
    let ty = core
        .types
        .types
        .iter_mut()
        .find(|ty| ty.id == calls_type)
        .unwrap();
    let TypeDef::Variant(variants) = &mut ty.ty.type_def else {
        panic!("SubtensorModule call enum required")
    };
    edit(&mut variants.variants);
    RuntimeHandle::new(core).unwrap()
}

#[test]
fn moved_call_index_with_unchanged_shape_composes_with_the_live_index() {
    let baseline = RuntimeHandle::new(pinned()).unwrap();
    let compiled = &calls::subtensor_module::SET_MECHANISM_WEIGHTS;
    assert_eq!(
        baseline.call_index("SubtensorModule", "set_mechanism_weights"),
        Some(compiled.call_index)
    );
    let mut moved_to = None;
    let changed = with_variant(|variants| {
        let free = (0..=255u8)
            .rev()
            .find(|index| variants.iter().all(|variant| variant.index != *index))
            .unwrap();
        let variant = variants
            .iter_mut()
            .find(|variant| variant.name == "set_mechanism_weights")
            .unwrap();
        variant.index = free;
        moved_to = Some(free);
    });
    let moved_to = moved_to.unwrap();
    assert_ne!(moved_to, compiled.call_index);
    assert_eq!(
        changed.call_index("SubtensorModule", "set_mechanism_weights"),
        Some(moved_to)
    );

    let encoded = set_weights().compose(&changed).unwrap();
    assert_eq!(
        encoded.as_bytes()[..2],
        [compiled.pallet_index, moved_to],
        "composition must encode the live call index"
    );
    let original = set_weights().compose(&baseline).unwrap();
    assert_eq!(encoded.as_bytes()[2..], original.as_bytes()[2..]);

    let report = changed.surface_report();
    assert!(report.shape_matches(DESCRIPTOR));
    let moved = report
        .changes
        .iter()
        .filter(|change| change.descriptor == DESCRIPTOR)
        .collect::<Vec<_>>();
    assert_eq!(moved.len(), 1, "{moved:#?}");
    assert_eq!(moved[0].kind, SurfaceKind::Call);
    assert_eq!(moved[0].class, SurfaceClass::Behavioural);
    assert!(
        moved[0].detail.contains("index changed"),
        "{}",
        moved[0].detail
    );
    assert_eq!(report.breaking_items(), 0, "{:#?}", report.changes);

    let scoped = verify_surface_with(&changed, &[SurfaceDescriptor::Call(compiled)]);
    assert_eq!(scoped.breaking_items(), 0);
    assert_eq!(scoped.behavioural_items(), 1);
    assert!(scoped.shape_matches(DESCRIPTOR));
}

#[test]
fn changed_argument_shape_is_still_refused() {
    let changed = with_variant(|variants| {
        let variant = variants
            .iter_mut()
            .find(|variant| variant.name == "set_mechanism_weights")
            .unwrap();
        let netuid = variant.fields[0].ty;
        let version_key = variant
            .fields
            .iter_mut()
            .find(|field| field.name.as_deref() == Some("version_key"))
            .unwrap();
        assert_ne!(version_key.ty, netuid);
        version_key.ty = netuid;
    });
    assert!(matches!(
        set_weights().compose(&changed),
        Err(ComposeError::SurfaceChanged { .. })
    ));
    let report = changed.surface_report();
    assert!(!report.shape_matches(DESCRIPTOR));
    assert!(report.changes.iter().any(|change| change.descriptor == DESCRIPTOR
        && change.class == SurfaceClass::Breaking));
}
