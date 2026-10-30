//! The pinned spec-486 metadata keeps the emission weights call where the
//! generated descriptor and `rtao subnets emission` expect it.
#![allow(clippy::unwrap_used)]

use rtao_core::runtime::Runtime;
use rtao_primitives::NetUid;
use rtao_runtime::{RuntimeHandle, calls::admin_utils};

fn pinned() -> Runtime {
    Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/486.v15.scale"),
        486,
        1,
        42,
    )
    .unwrap()
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn sudo_set_subnet_emission_weights_has_pinned_indices_and_argument_types() -> TestResult {
    let runtime = pinned();
    let pallet = runtime.pallet("AdminUtils").unwrap();
    assert_eq!(pallet.index, 19);
    let scale_info::TypeDef::Variant(calls) = &runtime
        .resolve(pallet.calls_type.unwrap())
        .unwrap()
        .type_def
    else {
        return Err("AdminUtils calls are not an enum".into());
    };
    let call = calls
        .variants
        .iter()
        .find(|call| call.name == "sudo_set_subnet_emission_weights")
        .unwrap();
    assert_eq!(call.index, 114);
    let fields = call
        .fields
        .iter()
        .map(|field| {
            (
                field.name.as_deref().unwrap(),
                runtime.type_ident(field.ty.id),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        [
            ("netuid", "NetUid".to_owned()),
            ("uids", "Vec<u16>".to_owned()),
            ("values", "Vec<u64>".to_owned()),
        ]
    );
    // NetUid is a transparent u16 newtype on the wire.
    let scale_info::TypeDef::Composite(netuid) =
        &runtime.resolve(call.fields[0].ty.id).unwrap().type_def
    else {
        return Err("NetUid is not a composite".into());
    };
    assert_eq!(netuid.fields.len(), 1);
    assert_eq!(runtime.type_ident(netuid.fields[0].ty.id), "u16");

    let descriptor = &admin_utils::SUDO_SET_SUBNET_EMISSION_WEIGHTS;
    assert_eq!((descriptor.pallet_index, descriptor.call_index), (19, 114));
    assert_eq!(
        descriptor
            .fields
            .iter()
            .map(|field| (field.name, field.type_ident))
            .collect::<Vec<_>>(),
        [
            ("netuid", "NetUid"),
            ("uids", "Vec<u16>"),
            ("values", "Vec<u64>"),
        ]
    );
    Ok(())
}

#[test]
fn sudo_set_subnet_emission_weights_composes_exact_wire_bytes() {
    let runtime = RuntimeHandle::new(pinned()).unwrap();
    let values = [0, 0, 0x0102_0304_0506_0708, u64::MAX - 1, 1];
    let call = admin_utils::sudo_set_subnet_emission_weights(
        NetUid(0x0107),
        vec![0, 1, 2, 3, 4],
        values.to_vec(),
    );
    // Expected bytes are written out by hand: pallet 19, call 114, the u16
    // netuid, then compact-length-prefixed u16 slots and u64 parts.
    let mut expected = vec![19, 114, 0x07, 0x01, 5 << 2];
    for slot in 0u16..5 {
        expected.extend_from_slice(&slot.to_le_bytes());
    }
    expected.push(5 << 2);
    for value in values {
        expected.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(call.compose(&runtime).unwrap().as_bytes(), expected);
}
