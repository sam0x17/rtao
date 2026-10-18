//! Validate semantic generation rules against the actual pinned chain metadata.
#![allow(clippy::unwrap_used)]
use rtao_core::runtime::Runtime;
use rtao_primitives::{AccountId, CompactBytesText, DefaultAccount, StakeWeight, U96F32};
use rtao_runtime::{ModelFieldDescriptor, StorageDescriptor, shapes, storage};
use scale_decode::DecodeAsType;

#[test]
fn zero_account_default_decodes_to_zero_bytes() {
    let runtime = Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/479.v15.scale"),
        479,
        1,
        42,
    )
    .unwrap();
    let entries = runtime
        .pallets
        .iter()
        .flat_map(|pallet| {
            pallet
                .storage
                .iter()
                .map(move |entry| (pallet.name.as_str(), entry))
        })
        .filter(|(_, entry)| {
            entry.modifier == "Default"
                && runtime.type_ident(entry.value_type) == "AccountId32"
                && entry.default_bytes == [0; 32]
        })
        .collect::<Vec<_>>();
    assert_eq!(
        entries
            .iter()
            .map(|(pallet, entry)| format!("{pallet}.{}", entry.name))
            .collect::<Vec<_>>(),
        vec![
            "SubtensorModule.Owner",
            "SubtensorModule.SubnetOwner",
            "SubtensorModule.SubnetOwnerHotkey",
            "SubtensorModule.Keys",
        ]
    );
    for (_, entry) in entries {
        let decoded = rtao_core::ops::storage::decode_storage_bytes(&runtime, entry, None).unwrap();
        assert_eq!(
            rtao_core::value_ext::as_str(&decoded),
            Some(AccountId::ZERO.to_ss58(42).as_str())
        );
        let typed = AccountId::decode_as_type(
            &mut &entry.default_bytes[..],
            entry.value_type,
            &runtime.types,
        )
        .unwrap();
        assert_eq!(typed.as_bytes(), &[0; 32]);
        assert!(DefaultAccount::new(AccountId::ZERO).get().is_none());
    }
    fn account<K>(_: &StorageDescriptor<K, DefaultAccount>) {}
    account(&storage::subtensor_module::OWNER);
    account(&storage::subtensor_module::SUBNET_OWNER);
    account(&storage::subtensor_module::SUBNET_OWNER_HOTKEY);
    account(&storage::subtensor_module::KEYS);
}

#[test]
fn semantic_model_fields_and_native_fixed_fraction_are_typed() {
    fn field<T>(_: &ModelFieldDescriptor<T>) {}
    field::<CompactBytesText>(&shapes::metagraph::NAME);
    field::<CompactBytesText>(&shapes::metagraph::SYMBOL);
    field::<Vec<StakeWeight>>(&shapes::metagraph::TOTAL_STAKE);
    field::<Option<CompactBytesText>>(&shapes::selective_metagraph::NAME);
    field::<Option<CompactBytesText>>(&shapes::selective_metagraph::SYMBOL);
    field::<Option<Vec<StakeWeight>>>(&shapes::selective_metagraph::TOTAL_STAKE);
    fn fixed<K>(_: &StorageDescriptor<K, U96F32>) {}
    fixed(&storage::subtensor_module::ROOT_PROP);
    fixed(&storage::subtensor_module::MINER_BURNED);
    assert_eq!(
        U96F32::from_bits((1u128 << 32) + 1).to_string(),
        "1.00000000023283064365386962890625"
    );
    assert_eq!(U96F32::from_bits(u128::MAX).int_part(), (1u128 << 96) - 1);
}

#[test]
fn path_only_leaf_aliases_preserve_structural_surface_and_composed_bytes() {
    use rtao_primitives::NetUid;
    use rtao_runtime::{RuntimeHandle, calls};
    let parse = || {
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/479.v15.scale"),
            479,
            1,
            42,
        )
        .unwrap()
    };
    let before = RuntimeHandle::new(parse()).unwrap();
    let mut aliased = parse();
    let account = aliased
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "AccountId32")
        })
        .unwrap();
    *account.ty.path.segments.last_mut().unwrap() = "RenamedAccountAlias".to_owned();
    account.ty.docs.push("Unrelated documentation".to_owned());
    let after = RuntimeHandle::new(aliased).unwrap();
    let builder = || calls::subtensor_module::burned_register(NetUid(1), AccountId([17; 32]));
    let original = builder().compose(&before).unwrap();
    let renamed = builder().compose(&after).unwrap();
    assert_eq!(original.as_bytes(), renamed.as_bytes());
    assert!(after.surface_report().shapes_match());
}

fn projected_metagraph_output() -> rtao_runtime::Projection {
    use rtao_runtime::{Projection, ProjectionField, ProjectionFields, ProjectionVariant};
    Projection::Variant(vec![
        ProjectionVariant {
            name: "None".to_owned(),
            fields: ProjectionFields::Exact,
        },
        ProjectionVariant {
            name: "Some".to_owned(),
            fields: ProjectionFields::Unnamed(vec![Projection::Composite(vec![
                ProjectionField::exact("netuid"),
            ])]),
        },
    ])
}
#[test]
fn api_by_name_output_projection_tolerates_only_undeclared_fields() {
    use rtao_runtime::{
        RuntimeHandle, SurfaceDescriptor, runtime_apis, shape_parts, verify_surface_with,
    };
    let parse = || {
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/479.v15.scale"),
            479,
            1,
            42,
        )
        .unwrap()
    };
    let baseline = RuntimeHandle::new(parse()).unwrap();
    let mut expected = runtime_apis::subnet_info_runtime_api::GET_METAGRAPH.surface();
    let method = baseline.runtime_api(expected.api, expected.name).unwrap();
    let mut parts = method
        .inputs
        .iter()
        .map(|input| (input.name.as_str(), baseline.exact_shape(input.ty).unwrap()))
        .collect::<Vec<_>>();
    parts.push((
        "$output",
        baseline
            .projection_shape(method.output, &projected_metagraph_output())
            .unwrap(),
    ));
    expected.shape = shape_parts(&parts);
    expected.output_projection = Some(projected_metagraph_output);
    assert!(
        verify_surface_with(&baseline, &[SurfaceDescriptor::RuntimeApi(expected)]).shapes_match()
    );
    let mut appended = parse();
    let ty = appended
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(composite) = &mut ty.ty.type_def else {
        unreachable!()
    };
    let mut extra = composite.fields[0].clone();
    extra.name = Some("future_unused_field".to_owned());
    composite.fields.push(extra);
    let appended = RuntimeHandle::new(appended).unwrap();
    assert!(
        verify_surface_with(&appended, &[SurfaceDescriptor::RuntimeApi(expected)]).shapes_match()
    );
    let mut renamed = parse();
    let ty = renamed
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(composite) = &mut ty.ty.type_def else {
        unreachable!()
    };
    composite
        .fields
        .iter_mut()
        .find(|field| field.name.as_deref() == Some("netuid"))
        .unwrap()
        .name = Some("renamed_netuid".to_owned());
    let renamed = RuntimeHandle::new(renamed).unwrap();
    assert!(
        !verify_surface_with(&renamed, &[SurfaceDescriptor::RuntimeApi(expected)])
            .shape_matches("api:SubnetInfoRuntimeApi.get_metagraph")
    );
}

#[test]
fn metagraph_netuid_contract_preserves_the_packed_mechanism_index() {
    use rtao_primitives::{MechId, NetUid, NetUidStorageIndex};
    fn index(_: &ModelFieldDescriptor<NetUidStorageIndex>) {}
    index(&shapes::metagraph::NETUID);
    index(&shapes::selective_metagraph::NETUID);
    let encoded = NetUidStorageIndex::new(NetUid(1), MechId(3)).unwrap();
    assert_eq!(encoded.0, 12289);
    assert_eq!(encoded.split(), (NetUid(1), MechId(3)));
}
