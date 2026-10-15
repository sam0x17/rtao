// Derived from RaoFoundation/subtensor sdk/python/scripts/record_shape_corpus.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Registry-directed model differential generation, including compact boundaries.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use parity_scale_codec::Decode;
use rtao_core::{codec::decode::Cursor, runtime::Runtime};
use scale_decode::DecodeAsType;
use scale_info::{PortableRegistry, TypeDef, TypeDefPrimitive};

use super::*;

struct ByteGen<'a> {
    types: &'a PortableRegistry,
    seed: u64,
    force_variant: Option<(u32, usize)>,
}
impl ByteGen<'_> {
    fn next(&mut self) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }
    fn generate(&mut self, id: u32, depth: u8) -> Vec<u8> {
        assert!(
            depth <= 8,
            "registry recursion exceeds corpus safety ceiling"
        );
        let ty = self.types.resolve(id).unwrap();
        let def = ty.type_def.clone();
        if matches!(
            ty.path.segments.last().map(String::as_str),
            Some("NetUid" | "MechId")
        ) {
            let max = if ty.path.segments.last().is_some_and(|name| name == "NetUid") {
                4095
            } else {
                15
            };
            let number = self.next() % (max + 1);
            return if max == 4095 {
                (number as u16).to_le_bytes().to_vec()
            } else {
                vec![number as u8]
            };
        }
        match def {
            TypeDef::Composite(composite) => composite
                .fields
                .iter()
                .flat_map(|field| self.generate(field.ty.id, depth + 1))
                .collect(),
            TypeDef::Variant(variants) => {
                assert!(
                    !variants.variants.is_empty(),
                    "uninhabited registry variant"
                );
                let index = if self.force_variant.is_some_and(|(forced, _)| forced == id) {
                    self.force_variant.take().unwrap().1
                } else {
                    self.next() as usize % variants.variants.len()
                };
                let selected = if depth >= 8 {
                    variants
                        .variants
                        .iter()
                        .find(|variant| variant.fields.is_empty())
                        .unwrap_or(&variants.variants[index])
                } else {
                    &variants.variants[index]
                };
                let mut bytes = vec![selected.index];
                for field in &selected.fields {
                    bytes.extend(self.generate(field.ty.id, depth + 1));
                }
                bytes
            }
            TypeDef::Sequence(sequence) => {
                let len = if depth >= 8 {
                    0
                } else {
                    self.next() as usize % 4
                };
                let mut bytes = compact(len as u128);
                for _ in 0..len {
                    bytes.extend(self.generate(sequence.type_param.id, depth + 1));
                }
                bytes
            }
            TypeDef::Array(array) => {
                assert!(array.len <= 4096, "array exceeds corpus bound");
                (0..array.len)
                    .flat_map(|_| self.generate(array.type_param.id, depth + 1))
                    .collect()
            }
            TypeDef::Tuple(tuple) => tuple
                .fields
                .iter()
                .flat_map(|field| self.generate(field.id, depth + 1))
                .collect(),
            TypeDef::Compact(inner) => {
                let max = max_number(self.types, inner.type_param.id);
                let boundaries = [
                    0,
                    63,
                    64,
                    16_383,
                    16_384,
                    (1 << 30) - 1,
                    1 << 30,
                    u128::from(u32::MAX),
                    u128::from(u64::MAX),
                    u128::MAX,
                ];
                let sample = boundaries[self.next() as usize % boundaries.len()];
                compact(sample.min(max))
            }
            TypeDef::Primitive(primitive) => {
                let width = match primitive {
                    TypeDefPrimitive::Bool => return vec![(self.next() % 2) as u8],
                    TypeDefPrimitive::Char => {
                        return (u32::from(b' ') + (self.next() % 95) as u32)
                            .to_le_bytes()
                            .to_vec();
                    }
                    TypeDefPrimitive::Str => {
                        let texts = ["", "abc", "τ subnet"];
                        let text = texts[self.next() as usize % texts.len()];
                        let mut bytes = compact(text.len() as u128);
                        bytes.extend_from_slice(text.as_bytes());
                        return bytes;
                    }
                    TypeDefPrimitive::U8 | TypeDefPrimitive::I8 => 1,
                    TypeDefPrimitive::U16 | TypeDefPrimitive::I16 => 2,
                    TypeDefPrimitive::U32 | TypeDefPrimitive::I32 => 4,
                    TypeDefPrimitive::U64 | TypeDefPrimitive::I64 => 8,
                    TypeDefPrimitive::U128 | TypeDefPrimitive::I128 => 16,
                    TypeDefPrimitive::U256 | TypeDefPrimitive::I256 => 32,
                };
                (0..width).map(|_| self.next() as u8).collect()
            }
            TypeDef::BitSequence(_) => {
                panic!("listed hand model contains unsupported bit sequence")
            }
        }
    }
}
fn compact(value: u128) -> Vec<u8> {
    let mut output = Vec::new();
    rtao_core::codec::encode::compact(value, &mut output).unwrap();
    output
}
fn max_number(types: &PortableRegistry, id: u32) -> u128 {
    let ty = types.resolve(id).unwrap();
    if ty.path.segments.last().is_some_and(|name| name == "NetUid") {
        return 4095;
    }
    match &ty.type_def {
        TypeDef::Primitive(TypeDefPrimitive::U8) => u128::from(u8::MAX),
        TypeDef::Primitive(TypeDefPrimitive::U16) => u128::from(u16::MAX),
        TypeDef::Primitive(TypeDefPrimitive::U32) => u128::from(u32::MAX),
        TypeDef::Primitive(TypeDefPrimitive::U64) => u128::from(u64::MAX),
        TypeDef::Primitive(TypeDefPrimitive::U128) => u128::MAX,
        TypeDef::Composite(composite) if composite.fields.len() == 1 => {
            max_number(types, composite.fields[0].ty.id)
        }
        _ => panic!("unsupported compact target {}", ty.path.segments.join("::")),
    }
}
fn compare<T: DecodeAsType + ToDynamic>(runtime: &Runtime, name: &str, seed: u64) {
    let id = runtime
        .types
        .types
        .iter()
        .find(|ty| ty.ty.path.segments.last().is_some_and(|last| last == name))
        .unwrap()
        .id;
    let bytes = ByteGen {
        types: &runtime.types,
        seed: seed | 1,
        force_variant: None,
    }
    .generate(id, 0);
    compare_bytes::<T>(runtime, id, name, seed, &bytes);
}

fn compare_bytes<T: DecodeAsType + ToDynamic>(
    runtime: &Runtime,
    id: u32,
    name: &str,
    seed: u64,
    bytes: &[u8],
) {
    let mut cursor = Cursor::new(bytes);
    let dynamic = runtime.decode_id(id, &mut cursor);
    let mut input = bytes;
    let typed = T::decode_as_type(&mut input, id, &runtime.types);
    assert_eq!(
        typed.is_ok(),
        dynamic.is_ok(),
        "{name}, seed {seed}, typed {:?}, dynamic {:?}",
        typed.as_ref().err(),
        dynamic.as_ref().err()
    );
    let typed = typed.unwrap().to_dynamic();
    let dynamic = dynamic.unwrap();
    assert_eq!(typed, dynamic, "{name}, seed {seed}");
    assert!(input.is_empty(), "{name} typed decoder left bytes");
    assert_eq!(cursor.remaining(), 0, "{name} dynamic decoder left bytes");
}

fn every_variant<T: DecodeAsType + ToDynamic>(runtime: &Runtime, id: u32) {
    let ty = runtime.types.resolve(id).unwrap();
    let TypeDef::Variant(variants) = &ty.type_def else {
        panic!("enum required");
    };
    for index in 0..variants.variants.len() {
        let bytes = ByteGen {
            types: &runtime.types,
            seed: 47,
            force_variant: Some((id, index)),
        }
        .generate(id, 0);
        compare_bytes::<T>(
            runtime,
            id,
            &ty.path.segments.join("::"),
            index as u64,
            &bytes,
        );
    }
}

#[test]
fn every_commitment_hyperparameter_and_proxy_variant_matches_core() {
    let runtime = Runtime::parse(
        include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap();
    let data = runtime
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Data")
                && ty
                    .ty
                    .path
                    .segments
                    .iter()
                    .any(|part| part.contains("commitment"))
        })
        .unwrap()
        .id;
    every_variant::<Data>(&runtime, data);
    for (name, test) in [
        (
            "HyperparamValue",
            every_variant::<HyperparamValue> as fn(&Runtime, u32),
        ),
        ("ProxyType", every_variant::<ProxyType> as fn(&Runtime, u32)),
    ] {
        let id = runtime
            .types
            .types
            .iter()
            .find(|ty| ty.ty.path.segments.last().is_some_and(|last| last == name))
            .unwrap()
            .id;
        test(&runtime, id);
    }
    let info = runtime
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "CrowdloanInfo")
        })
        .unwrap();
    let TypeDef::Composite(fields) = &info.ty.type_def else {
        panic!("record required");
    };
    let option_id = fields
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some("call"))
        .unwrap()
        .ty
        .id;
    let TypeDef::Variant(option) = &runtime.types.resolve(option_id).unwrap().type_def else {
        panic!("Option required");
    };
    let bounded_id = option
        .variants
        .iter()
        .find(|variant| variant.name == "Some")
        .unwrap()
        .fields[0]
        .ty
        .id;
    every_variant::<CrowdloanCall>(&runtime, bounded_id);
}

fn historical_runtime() -> Runtime {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../../../vendor/upstream/golden.json")).unwrap();
    let bytes = hex::decode(
        golden["metadata"]["v15_hex"]
            .as_str()
            .unwrap()
            .strip_prefix("0x")
            .unwrap(),
    )
    .unwrap();
    let bytes = Option::<Vec<u8>>::decode(&mut bytes.as_slice())
        .unwrap()
        .unwrap();
    Runtime::parse(&bytes, 461, 1, 42).unwrap()
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 256, failure_persistence: None, ..Default::default() })]
    #[test]
    fn all_typed_wire_models_equal_core_registry(seed in proptest::num::u64::ANY) {
        let runtime = Runtime::parse(include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"), 478, 1, 42).unwrap();
        compare::<MetagraphWire>(&runtime, "Metagraph", seed);
        compare::<SelectiveMetagraphWire>(&runtime, "SelectiveMetagraph", seed);
        compare::<AxonInfo>(&runtime, "AxonInfo", seed);
        compare::<ChainIdentityV2>(&runtime, "ChainIdentityV2", seed);
        compare::<SubnetIdentityV3>(&runtime, "SubnetIdentityV3", seed);
        compare::<NeuronInfoLite>(&runtime, "NeuronInfoLite", seed);
        compare::<NeuronInfo>(&runtime, "NeuronInfo", seed);
        compare::<DynamicInfo>(&runtime, "DynamicInfo", seed);
        compare::<SubnetInfoV2>(&runtime, "SubnetInfov2", seed);
        compare::<SubnetState>(&runtime, "SubnetState", seed);
        compare::<SimSwapResult>(&runtime, "SimSwapResult", seed);
        compare::<SubnetPrice>(&runtime, "SubnetPrice", seed);
        compare::<HyperparamEntry>(&runtime, "HyperparamEntry", seed);
        compare::<HyperparamValue>(&runtime, "HyperparamValue", seed);
        compare::<StakeInfo>(&runtime, "StakeInfo", seed);
        compare::<StakeAvailability>(&runtime, "StakeAvailability", seed);
        compare::<DelegateInfo>(&runtime, "DelegateInfo", seed);
        compare::<PrometheusInfo>(&runtime, "PrometheusInfo", seed);
        compare::<NeuronCertificate>(&runtime, "NeuronCertificate", seed);
        compare::<LockState>(&runtime, "LockState", seed);
        compare::<MinerCollateralState>(&runtime, "MinerCollateralState", seed);
        compare::<CommitmentRegistration>(&runtime, "Registration", seed);
        compare::<SubnetLease>(&runtime, "SubnetLease", seed);
        compare::<CrowdloanInfo>(&runtime, "CrowdloanInfo", seed);
        compare::<ProxyDefinition>(&runtime, "ProxyDefinition", seed);
        compare::<MultisigInfo>(&runtime, "Multisig", seed);
        compare::<BasketSummary>(&runtime, "BasketSummary", seed);
        compare::<BasketPosition>(&runtime, "BasketPosition", seed);
        compare::<BetaPricing>(&runtime, "BetaPricing", seed);
        let historical = historical_runtime();
        compare::<MetagraphWire>(&historical, "Metagraph", seed);
        compare::<SelectiveMetagraphWire>(&historical, "SelectiveMetagraph", seed);
        compare::<AxonInfo>(&historical, "AxonInfo", seed);
        compare::<ChainIdentityV2>(&historical, "ChainIdentityV2", seed);
        compare::<SubnetIdentityV3>(&historical, "SubnetIdentityV3", seed);
    }
}

#[test]
fn selective_compact_commitment_bytes_keep_list_shape() {
    let runtime = Runtime::parse(
        include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap();
    compare::<SelectiveMetagraphWire>(&runtime, "SelectiveMetagraph", 7_239_308_216_738_785_734);
}

#[test]
fn every_compact_width_is_generated_at_its_boundary() {
    for value in [
        0,
        63,
        64,
        16_383,
        16_384,
        (1 << 30) - 1,
        1 << 30,
        u128::from(u64::MAX),
        u128::MAX,
    ] {
        let encoded = compact(value);
        let mut cursor = Cursor::new(&encoded);
        assert_eq!(
            rtao_core::codec::decode::compact_u128(&mut cursor).unwrap(),
            value
        );
        assert_eq!(cursor.remaining(), 0);
    }
}
