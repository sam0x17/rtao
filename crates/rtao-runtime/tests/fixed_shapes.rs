//! Numeric-scale changes must refuse typed model and API decoding contracts.
#![allow(clippy::unwrap_used, clippy::panic)]

use rtao_core::runtime::Runtime;
use rtao_runtime::{
    ModelSurface, Projection, ProjectionField, ProjectionFields, ProjectionVariant, RuntimeHandle,
    SurfaceDescriptor, runtime_apis, shape_parts, verify_surface_with,
};
use scale_info::TypeDef;

fn pinned_runtime() -> Runtime {
    Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap()
}

fn moving_price_projection() -> Projection {
    Projection::Composite(vec![ProjectionField::exact("moving_price")])
}

fn moving_price_api_projection() -> Projection {
    Projection::Variant(vec![
        ProjectionVariant {
            name: "None".to_owned(),
            fields: ProjectionFields::Exact,
        },
        ProjectionVariant {
            name: "Some".to_owned(),
            fields: ProjectionFields::Unnamed(vec![moving_price_projection()]),
        },
    ])
}

#[test]
fn fixed_fraction_change_refuses_exact_and_projected_model_and_api_gates() {
    let core = pinned_runtime();
    let metagraph = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let TypeDef::Composite(fields) = &metagraph.ty.type_def else {
        panic!("pinned Metagraph composite");
    };
    let fixed = fields
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some("moving_price"))
        .unwrap()
        .ty
        .id;
    let metagraph = metagraph.id;
    let fraction32 = core.types.resolve(fixed).unwrap().type_params[0]
        .ty
        .unwrap()
        .id;
    let fraction64 = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty.path.segments == ["substrate_fixed", "FixedI128"]
                && ty.ty.type_params[0].ty.unwrap().id != fraction32
        })
        .unwrap()
        .ty
        .type_params[0]
        .ty
        .unwrap()
        .id;
    let baseline = RuntimeHandle::new(core).unwrap();
    let mut changed = pinned_runtime();
    changed.types.types[fixed as usize].ty.type_params[0].ty = Some(fraction64.into());
    assert_eq!(
        baseline.core().types.resolve(fixed).unwrap().type_def,
        changed.types.resolve(fixed).unwrap().type_def
    );
    let changed = RuntimeHandle::new(changed).unwrap();
    for projection in [Projection::Exact, moving_price_projection()] {
        let expected = ModelSurface {
            type_path: "pallet_subtensor::rpc_info::metagraph::Metagraph",
            shape: baseline.projection_shape(metagraph, &projection).unwrap(),
            projection,
        };
        let descriptor = SurfaceDescriptor::Model(expected);
        assert!(
            verify_surface_with(&baseline, std::slice::from_ref(&descriptor))
                .shape_matches("model:pallet_subtensor::rpc_info::metagraph::Metagraph")
        );
        assert!(
            !verify_surface_with(&changed, &[descriptor])
                .shape_matches("model:pallet_subtensor::rpc_info::metagraph::Metagraph")
        );
    }
    for projection in [
        None,
        Some(moving_price_api_projection as fn() -> Projection),
    ] {
        let mut expected = runtime_apis::subnet_info_runtime_api::GET_METAGRAPH.surface();
        let method = baseline.runtime_api(expected.api, expected.name).unwrap();
        let mut parts = method
            .inputs
            .iter()
            .map(|input| (input.name.as_str(), baseline.exact_shape(input.ty).unwrap()))
            .collect::<Vec<_>>();
        let output = match projection {
            Some(make) => baseline.projection_shape(method.output, &make()).unwrap(),
            None => baseline.exact_shape(method.output).unwrap(),
        };
        parts.push(("$output", output));
        expected.shape = shape_parts(&parts);
        expected.output_projection = projection;
        let descriptor = SurfaceDescriptor::RuntimeApi(expected);
        assert!(
            verify_surface_with(&baseline, std::slice::from_ref(&descriptor))
                .shape_matches("api:SubnetInfoRuntimeApi.get_metagraph")
        );
        assert!(
            !verify_surface_with(&changed, &[descriptor])
                .shape_matches("api:SubnetInfoRuntimeApi.get_metagraph")
        );
    }
}
