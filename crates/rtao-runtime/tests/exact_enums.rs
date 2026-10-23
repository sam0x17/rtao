//! Typed enum additions must invalidate the enum and containing model contracts.
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_core::runtime::Runtime;
use rtao_runtime::{Projection, RuntimeHandle, SurfaceDescriptor, shapes, verify_surface_with};
use scale_info::TypeDef;

#[test]
fn future_proxy_variant_refuses_exact_enum_and_containing_model() {
    let pinned = || {
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/482.v15.scale"),
            482,
            1,
            42,
        )
        .unwrap()
    };
    let mut core = pinned();
    let baseline = RuntimeHandle::new(pinned()).unwrap();
    let descriptors = shapes::models()
        .into_iter()
        .filter(|descriptor| match descriptor {
            SurfaceDescriptor::Model(model) => {
                model.type_path.ends_with("::ProxyType")
                    || model.type_path.ends_with("::ProxyDefinition")
            }
            _ => false,
        })
        .collect::<Vec<_>>();
    assert_eq!(descriptors.len(), 2);
    assert!(descriptors.iter().any(|descriptor| matches!(descriptor,
        SurfaceDescriptor::Model(model) if model.type_path.ends_with("::ProxyType") && model.projection == Projection::Exact)));
    let before = verify_surface_with(&baseline, &descriptors);
    let proxy = core
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "ProxyType")
        })
        .unwrap();
    let TypeDef::Variant(variants) = &mut proxy.ty.type_def else {
        panic!("ProxyType enum required")
    };
    let mut future = variants.variants[0].clone();
    future.name = "FuturePermission".into();
    future.index = 200;
    variants.variants.push(future);
    let changed = RuntimeHandle::new(core).unwrap();
    let after = verify_surface_with(&changed, &descriptors);
    for descriptor in &descriptors {
        let SurfaceDescriptor::Model(model) = descriptor else {
            unreachable!()
        };
        let identity = format!("model:{}", model.type_path);
        assert!(before.shape_matches(&identity));
        assert!(!after.shape_matches(&identity));
    }
}
