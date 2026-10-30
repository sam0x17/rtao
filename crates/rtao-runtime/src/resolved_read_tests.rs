//! Name-resolved reads accept changes whose meaning is recoverable and refuse
//! every other change per item, key or field (D-089).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use parity_scale_codec::{Compact, Encode};
use scale_info::{
    Path, PortableType, Type, TypeDefArray, TypeDefCompact, TypeDefComposite, TypeDefSequence,
    TypeDefTuple, TypeDefVariant, Variant,
};

const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/486.v15.scale");
const PALLET: &str = "SubtensorModule";
const PROBE: &str = "ResolvedReadProbe";
const WHO: [u8; 32] = [7; 32];

const TWC_FIELDS: &[ReadField] = &[
    ReadField {
        names: &["who", "account", "hotkey"],
        class: ReadClass::Account,
    },
    ReadField {
        names: &["commit_block"],
        class: ReadClass::Uint(64),
    },
    ReadField {
        names: &["commit", "serialized_compressed_commit"],
        class: ReadClass::Bytes,
    },
    ReadField {
        names: &["reveal_round", "round_number"],
        class: ReadClass::Uint(64),
    },
];
const TWC: ReadClass = ReadClass::Seq(&ReadClass::Record(TWC_FIELDS));

struct Lab {
    core: Runtime,
}

fn field(name: Option<&str>, ty: u32) -> Field<PortableForm> {
    Field {
        name: name.map(Into::into),
        ty: ty.into(),
        type_name: None,
        docs: vec![],
    }
}

impl Lab {
    fn new() -> Self {
        Self {
            core: Runtime::parse(METADATA, 486, 1, 42).unwrap(),
        }
    }

    fn add_at(&mut self, path: &[&str], type_def: TypeDef<PortableForm>) -> u32 {
        let id = u32::try_from(self.core.types.types.len()).unwrap();
        self.core.types.types.push(PortableType {
            id,
            ty: Type {
                path: Path {
                    segments: path.iter().map(|segment| (*segment).to_owned()).collect(),
                },
                type_params: vec![],
                type_def,
                docs: vec![],
            },
        });
        id
    }

    fn add(&mut self, type_def: TypeDef<PortableForm>) -> u32 {
        self.add_at(&[], type_def)
    }

    fn prim(&mut self, primitive: TypeDefPrimitive) -> u32 {
        self.add(TypeDef::Primitive(primitive))
    }

    fn compact(&mut self, inner: u32) -> u32 {
        self.add(TypeDef::Compact(TypeDefCompact {
            type_param: inner.into(),
        }))
    }

    fn seq(&mut self, inner: u32) -> u32 {
        self.add(TypeDef::Sequence(TypeDefSequence {
            type_param: inner.into(),
        }))
    }

    fn composite(&mut self, path: &[&str], fields: Vec<Field<PortableForm>>) -> u32 {
        self.add_at(path, TypeDef::Composite(TypeDefComposite { fields }))
    }

    fn tuple(&mut self, fields: &[u32]) -> u32 {
        self.add(TypeDef::Tuple(TypeDefTuple {
            fields: fields.iter().map(|id| (*id).into()).collect(),
        }))
    }

    fn account(&mut self) -> u32 {
        let byte = self.prim(TypeDefPrimitive::U8);
        let array = self.add(TypeDef::Array(TypeDefArray {
            len: 32,
            type_param: byte.into(),
        }));
        self.composite(
            &["sp_core", "crypto", "AccountId32"],
            vec![field(None, array)],
        )
    }

    fn bounded_bytes(&mut self) -> u32 {
        let byte = self.prim(TypeDefPrimitive::U8);
        let bytes = self.seq(byte);
        self.composite(&["BoundedVec"], vec![field(None, bytes)])
    }

    fn variants(&mut self, path: &[&str], variants: &[(&str, u8, Vec<u32>)]) -> u32 {
        self.add_at(
            path,
            TypeDef::Variant(TypeDefVariant {
                variants: variants
                    .iter()
                    .map(|(name, index, fields)| Variant {
                        name: (*name).to_owned(),
                        fields: fields.iter().map(|ty| field(None, *ty)).collect(),
                        index: *index,
                        docs: vec![],
                    })
                    .collect(),
            }),
        )
    }

    /// Add the probe entry with these key and value types.
    fn probe(&mut self, keys: &[u32], value: u32, modifier: &str, default: Vec<u8>) {
        let pallet = self
            .core
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == PALLET)
            .unwrap();
        pallet.storage.push(StorageInfo {
            name: PROBE.into(),
            prefix: PALLET.into(),
            modifier: modifier.into(),
            hashers: keys.iter().map(|_| "Twox64Concat".to_owned()).collect(),
            key_types: keys.to_vec(),
            value_type: value,
            default_bytes: default,
        });
    }

    fn entry(&mut self, name: &str) -> &mut StorageInfo {
        self.core
            .pallets
            .iter_mut()
            .find(|pallet| pallet.name == PALLET)
            .unwrap()
            .storage
            .iter_mut()
            .find(|entry| entry.name == name)
            .unwrap()
    }

    fn handle(self) -> RuntimeHandle {
        RuntimeHandle::new(self.core).unwrap()
    }
}

fn probe(keys: &'static [ReadClass], value: ReadClass) -> ResolvedStorage {
    ResolvedStorage {
        pallet: PALLET,
        name: PROBE,
        keys,
        value,
        presence: ReadPresence::Default,
    }
}

fn decode(storage: ResolvedStorage, runtime: &RuntimeHandle, bytes: &[u8]) -> ReadDatum {
    storage
        .plan(runtime)
        .unwrap()
        .decode(Some(bytes))
        .unwrap()
        .unwrap()
}

fn refused(storage: ResolvedStorage, runtime: &RuntimeHandle) -> ReadResolveError {
    storage.plan(runtime).expect_err("the read must refuse")
}

fn twc_datum(block: u64, commit: &[u8], round: u64) -> ReadDatum {
    ReadDatum::Seq(vec![ReadDatum::Record(vec![
        ReadDatum::Account(WHO),
        ReadDatum::Uint(block.into()),
        ReadDatum::Bytes(commit.to_vec()),
        ReadDatum::Uint(round.into()),
    ])])
}

#[test]
fn pinned_entries_decode_through_their_classes() {
    let runtime = Lab::new().handle();
    let twc = ResolvedStorage {
        pallet: PALLET,
        name: "TimelockedWeightCommits",
        keys: &[ReadClass::Uint(16), ReadClass::Uint(64)],
        value: TWC,
        presence: ReadPresence::Default,
    };
    let plan = twc.plan(&runtime).unwrap();
    let bytes = vec![(WHO, 110u64, vec![4u8, 5, 6], 500u64)].encode();
    assert_eq!(
        plan.decode(Some(&bytes)).unwrap(),
        Some(twc_datum(110, &[4, 5, 6], 500))
    );
    // A missing ValueQuery reads its live default, an empty queue.
    assert_eq!(plan.decode(None).unwrap(), Some(ReadDatum::Seq(vec![])));
    // The key is built from the live prefix, key types and hashers.
    let info = runtime
        .storage_entry(PALLET, "TimelockedWeightCommits")
        .unwrap();
    let expected = runtime
        .core()
        .storage_key(
            info,
            &[
                rtao_core::codec::Value::Int(1),
                rtao_core::codec::Value::Int(6),
            ],
        )
        .unwrap();
    assert_eq!(
        plan.key(&[ReadDatum::Uint(1), ReadDatum::Uint(6)]).unwrap(),
        expected
    );

    let uids = ResolvedStorage {
        pallet: PALLET,
        name: "Uids",
        keys: &[ReadClass::Uint(16), ReadClass::Account],
        value: ReadClass::Uint(16),
        presence: ReadPresence::Optional,
    };
    let plan = uids.plan(&runtime).unwrap();
    assert_eq!(plan.decode(None).unwrap(), None);
    assert_eq!(
        plan.decode(Some(&3u16.encode())).unwrap(),
        Some(ReadDatum::Uint(3))
    );
    let consensus = ResolvedStorage {
        pallet: PALLET,
        name: "SubnetEpochConsensus",
        keys: &[ReadClass::Uint(16)],
        value: ReadClass::Variants(&["Yuma", "Null"]),
        presence: ReadPresence::Default,
    };
    let plan = consensus.plan(&runtime).unwrap();
    assert_eq!(
        plan.decode(None).unwrap(),
        Some(ReadDatum::Variant("Yuma".into()))
    );
    assert_eq!(
        plan.decode(Some(&[1])).unwrap(),
        Some(ReadDatum::Variant("Null".into()))
    );
    assert_eq!(
        read_constant(&runtime, "Aura", "SlotDuration", &ReadClass::Uint(64)).unwrap(),
        ReadDatum::Uint(12_000)
    );
}

/// One probe value type, built in a fresh lab.
type Shape = fn(&mut Lab) -> u32;

fn wide(lab: &mut Lab) -> u32 {
    lab.prim(TypeDefPrimitive::U32)
}
fn compact_u64(lab: &mut Lab) -> u32 {
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    lab.compact(u64_)
}

#[test]
fn widened_compact_and_newtype_integers_decode_with_range_checks() {
    let cases: [(Shape, bool); 4] = [
        (wide, false),
        (compact_u64, true),
        (
            |lab| {
                let inner = wide(lab);
                lab.composite(&["Tempo"], vec![field(Some("blocks"), inner)])
            },
            false,
        ),
        (
            |lab| {
                let inner = compact_u64(lab);
                lab.composite(&["Tempo"], vec![field(None, inner)])
            },
            true,
        ),
    ];
    for (shape, compact) in cases {
        let encode = |value: u32| {
            if compact {
                Compact(u64::from(value)).encode()
            } else {
                value.encode()
            }
        };
        let mut lab = Lab::new();
        let key = lab.prim(TypeDefPrimitive::U32);
        let value = shape(&mut lab);
        lab.probe(&[key], value, "Default", encode(0));
        let runtime = lab.handle();
        let storage = probe(&[ReadClass::Uint(16)], ReadClass::Uint(16));
        assert_eq!(
            decode(storage, &runtime, &encode(360)),
            ReadDatum::Uint(360)
        );
        // A key widened to u32 is encoded with its live width.
        let plan = storage.plan(&runtime).unwrap();
        let built = plan.key(&[ReadDatum::Uint(1)]).unwrap();
        assert!(built.ends_with(&1u32.encode()), "{built:?}");
        // A value beyond the width this read uses refuses that value only.
        let error = plan.decode(Some(&encode(70_000))).unwrap_err();
        assert_eq!(error.part, "value");
        assert!(error.detail.contains("does not fit the u16"), "{error}");
        // A key beyond u16 cannot be supplied for a u16 key class.
        assert!(plan.key(&[ReadDatum::Uint(70_000)]).is_err());
    }
}

#[test]
fn sequences_bounded_vecs_and_arrays_decode_element_wise() {
    let seq_u64: Shape = |lab| {
        let u64_ = lab.prim(TypeDefPrimitive::U64);
        lab.seq(u64_)
    };
    let bools: Shape = |lab| {
        let bool_ = lab.prim(TypeDefPrimitive::Bool);
        lab.seq(bool_)
    };
    // (value type, class, default bytes, stored bytes, decoded)
    type Case = (Shape, ReadClass, Vec<u8>, Vec<u8>, ReadDatum);
    let cases: [Case; 4] = [
        (
            |lab| {
                let u64_ = lab.prim(TypeDefPrimitive::U64);
                let plain = lab.seq(u64_);
                lab.composite(&["BoundedVec"], vec![field(None, plain)])
            },
            ReadClass::Seq(&ReadClass::Uint(64)),
            vec![0],
            vec![80u64, 0].encode(),
            ReadDatum::Seq(vec![ReadDatum::Uint(80), ReadDatum::Uint(0)]),
        ),
        (
            seq_u64,
            ReadClass::Seq(&ReadClass::Uint(64)),
            vec![0],
            vec![1u64].encode(),
            ReadDatum::Seq(vec![ReadDatum::Uint(1)]),
        ),
        (
            bools,
            ReadClass::Seq(&ReadClass::Bool),
            vec![0],
            vec![true, false].encode(),
            ReadDatum::Seq(vec![ReadDatum::Bool(true), ReadDatum::Bool(false)]),
        ),
        (
            |lab| {
                let bool_ = lab.prim(TypeDefPrimitive::Bool);
                lab.add(TypeDef::Array(TypeDefArray {
                    len: 2,
                    type_param: bool_.into(),
                }))
            },
            ReadClass::Seq(&ReadClass::Bool),
            vec![0, 0],
            [false, true].encode(),
            ReadDatum::Seq(vec![ReadDatum::Bool(false), ReadDatum::Bool(true)]),
        ),
    ];
    for (shape, class, default, bytes, expected) in cases {
        let mut lab = Lab::new();
        let key = lab.prim(TypeDefPrimitive::U16);
        let value = shape(&mut lab);
        lab.probe(&[key], value, "Default", default);
        let runtime = lab.handle();
        assert_eq!(
            decode(probe(&[ReadClass::Uint(16)], class), &runtime, &bytes),
            expected
        );
    }
    // A bool byte other than 0 or 1 refuses.
    let mut lab = Lab::new();
    let key = lab.prim(TypeDefPrimitive::U16);
    let value = bools(&mut lab);
    lab.probe(&[key], value, "Default", vec![0]);
    let runtime = lab.handle();
    let plan = probe(&[ReadClass::Uint(16)], ReadClass::Seq(&ReadClass::Bool))
        .plan(&runtime)
        .unwrap();
    assert!(plan.decode(Some(&[4, 1, 2])).is_err());
}

#[test]
fn tuples_become_structs_with_reordered_aliased_and_added_fields() {
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u32_ = lab.prim(TypeDefPrimitive::U32);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    let account = lab.account();
    let bytes = lab.bounded_bytes();
    // Reordered, one field renamed to a declared alias, one added field.
    let entry = lab.composite(
        &["TimelockedCommit"],
        vec![
            field(Some("commit_block"), u64_),
            field(Some("hotkey"), account),
            field(Some("round_number"), u64_),
            field(Some("weight_count"), u32_),
            field(Some("commit"), bytes),
        ],
    );
    let queue = lab.seq(entry);
    let queue = lab.composite(&["VecDeque"], vec![field(None, queue)]);
    lab.probe(&[u16_], queue, "Default", vec![0]);
    let runtime = lab.handle();
    let mut encoded = Compact(1u32).encode();
    encoded.extend(110u64.encode());
    encoded.extend(WHO);
    encoded.extend(500u64.encode());
    encoded.extend(9u32.encode());
    encoded.extend(vec![4u8, 5, 6].encode());
    assert_eq!(
        decode(probe(&[ReadClass::Uint(16)], TWC), &runtime, &encoded),
        twc_datum(110, &[4, 5, 6], 500)
    );
    // Trailing bytes after the complete value refuse.
    let mut trailing = encoded.clone();
    trailing.push(0);
    let error = probe(&[ReadClass::Uint(16)], TWC)
        .plan(&runtime)
        .unwrap()
        .decode(Some(&trailing))
        .unwrap_err();
    assert!(error.detail.contains("trailing"), "{error}");

    // A tuple of the same arity still decodes positionally.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    let account = lab.account();
    let bytes = lab.bounded_bytes();
    let tuple = lab.tuple(&[account, u64_, bytes, u64_]);
    let queue = lab.seq(tuple);
    lab.probe(&[u16_], queue, "Default", vec![0]);
    let runtime = lab.handle();
    assert_eq!(
        decode(
            probe(&[ReadClass::Uint(16)], TWC),
            &runtime,
            &vec![(WHO, 1u64, vec![9u8], 2u64)].encode()
        ),
        twc_datum(1, &[9], 2)
    );
}

#[test]
fn unrecoverable_record_changes_refuse_naming_the_field() {
    let build = |fields: &dyn Fn(&mut Lab, u32, u32, u32) -> u32| {
        let mut lab = Lab::new();
        let u16_ = lab.prim(TypeDefPrimitive::U16);
        let u64_ = lab.prim(TypeDefPrimitive::U64);
        let account = lab.account();
        let bytes = lab.bounded_bytes();
        let entry = fields(&mut lab, u64_, account, bytes);
        let queue = lab.seq(entry);
        lab.probe(&[u16_], queue, "Default", vec![0]);
        let runtime = lab.handle();
        refused(probe(&[ReadClass::Uint(16)], TWC), &runtime)
    };
    // A rename without a declared alias.
    let error = build(&|lab, u64_, account, bytes| {
        lab.composite(
            &["TimelockedCommit"],
            vec![
                field(Some("who"), account),
                field(Some("included_at"), u64_),
                field(Some("commit"), bytes),
                field(Some("reveal_round"), u64_),
            ],
        )
    });
    assert_eq!(error.part, "value");
    assert!(error.detail.contains("`commit_block` is absent"), "{error}");
    // A removed tuple field.
    let error = build(&|lab, u64_, account, bytes| lab.tuple(&[account, u64_, bytes]));
    assert!(
        error.detail.contains("tuple of 3 fields, expected 4"),
        "{error}"
    );
    // A field whose type changed meaning.
    let error = build(&|lab, u64_, account, _| {
        let i64_ = lab.prim(TypeDefPrimitive::I64);
        lab.composite(
            &["TimelockedCommit"],
            vec![
                field(Some("who"), account),
                field(Some("commit_block"), u64_),
                field(Some("commit"), i64_),
                field(Some("reveal_round"), u64_),
            ],
        )
    });
    assert!(error.detail.contains("field `commit` is i64"), "{error}");
    // Two live fields both matching one declared field.
    let error = build(&|lab, u64_, account, bytes| {
        lab.composite(
            &["TimelockedCommit"],
            vec![
                field(Some("who"), account),
                field(Some("hotkey"), account),
                field(Some("commit_block"), u64_),
                field(Some("commit"), bytes),
                field(Some("reveal_round"), u64_),
            ],
        )
    });
    assert!(error.detail.contains("both match `who`"), "{error}");
}

#[test]
fn scalars_that_changed_meaning_refuse() {
    let cases: [(&str, Shape); 5] = [
        ("i32", |lab| lab.prim(TypeDefPrimitive::I32)),
        ("an Option", |lab| {
            let u16_ = lab.prim(TypeDefPrimitive::U16);
            lab.variants(&["Option"], &[("None", 0, vec![]), ("Some", 1, vec![u16_])])
        }),
        ("a struct of 2 fields", |lab| {
            let u16_ = lab.prim(TypeDefPrimitive::U16);
            lab.composite(
                &["Tempo"],
                vec![field(Some("blocks"), u16_), field(Some("phase"), u16_)],
            )
        }),
        ("bool", |lab| lab.prim(TypeDefPrimitive::Bool)),
        ("a sequence", |lab| {
            let u16_ = lab.prim(TypeDefPrimitive::U16);
            lab.seq(u16_)
        }),
    ];
    for (described, value) in cases {
        let mut lab = Lab::new();
        let key = lab.prim(TypeDefPrimitive::U16);
        let value = value(&mut lab);
        lab.probe(&[key], value, "Optional", vec![]);
        let runtime = lab.handle();
        let error = refused(probe(&[ReadClass::Uint(16)], ReadClass::Uint(16)), &runtime);
        assert_eq!(error.part, "value");
        assert!(
            error.detail.contains(&format!("is {described}")),
            "{described}: {error}"
        );
        assert_eq!(error.item, PROBE);
    }
}

#[test]
fn ratios_and_fixed_point_numbers_refuse_instead_of_unwrapping() {
    // (read class, live value type, described path)
    type Case = (ReadClass, Shape, &'static str);
    let cases: [Case; 6] = [
        // A per-mille u32 that became Permill(u32) would read millionths.
        (
            ReadClass::Uint(32),
            |lab| {
                let u32_ = lab.prim(TypeDefPrimitive::U32);
                lab.composite(
                    &["sp_arithmetic", "per_things", "Permill"],
                    vec![field(None, u32_)],
                )
            },
            "a per-thing ratio (sp_arithmetic::per_things::Permill)",
        ),
        // A u64 that became U64F64 { bits: u128 } would read raw bits.
        (
            ReadClass::Uint(64),
            |lab| {
                let u128_ = lab.prim(TypeDefPrimitive::U128);
                lab.composite(
                    &["substrate_fixed", "FixedU128"],
                    vec![field(Some("bits"), u128_)],
                )
            },
            "a fixed-point number (substrate_fixed::FixedU128)",
        ),
        (
            ReadClass::Uint(128),
            |lab| {
                let u128_ = lab.prim(TypeDefPrimitive::U128);
                lab.composite(
                    &["sp_arithmetic", "fixed_point", "FixedU128"],
                    vec![field(None, u128_)],
                )
            },
            "a fixed-point number (sp_arithmetic::fixed_point::FixedU128)",
        ),
        // A single `bits` field is fixed point whatever its path.
        (
            ReadClass::Uint(64),
            |lab| {
                let u64_ = lab.prim(TypeDefPrimitive::U64);
                lab.composite(&["Ratio"], vec![field(Some("bits"), u64_)])
            },
            "a fixed-point number (Ratio)",
        ),
        // A plain newtype around a ratio still refuses at the ratio.
        (
            ReadClass::Uint(32),
            |lab| {
                let u32_ = lab.prim(TypeDefPrimitive::U32);
                let perbill = lab.composite(&["Perbill"], vec![field(None, u32_)]);
                lab.composite(&["Cutoff"], vec![field(None, perbill)])
            },
            "a per-thing ratio (Perbill)",
        ),
        // Compact<Perbill> is a compact ratio, not a compact integer.
        (
            ReadClass::Uint(32),
            |lab| {
                let u32_ = lab.prim(TypeDefPrimitive::U32);
                let perbill = lab.composite(
                    &["sp_arithmetic", "per_things", "Perbill"],
                    vec![field(None, u32_)],
                );
                lab.compact(perbill)
            },
            "is a compact per-thing ratio (sp_arithmetic::per_things::Perbill)",
        ),
    ];
    for (class, value, described) in cases {
        let mut lab = Lab::new();
        let key = lab.prim(TypeDefPrimitive::U16);
        let value = value(&mut lab);
        // Every byte string decodes under the raw integer, so only the
        // layout can catch the changed meaning.
        lab.probe(&[key], value, "Default", vec![0; 16]);
        let runtime = lab.handle();
        let error = refused(probe(&[ReadClass::Uint(16)], class), &runtime);
        assert_eq!((error.item.as_str(), error.part.as_str()), (PROBE, "value"));
        assert!(error.detail.contains(described), "{described}: {error}");
        assert!(error.detail.contains("carries a scale"), "{error}");
    }

    // The same refusal names the record field whose integer became a ratio.
    let mut lab = Lab::new();
    let key = lab.prim(TypeDefPrimitive::U16);
    let u32_ = lab.prim(TypeDefPrimitive::U32);
    let permill = lab.composite(
        &["sp_arithmetic", "per_things", "Permill"],
        vec![field(None, u32_)],
    );
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    let entry = lab.composite(
        &["Entry"],
        vec![field(Some("block"), u64_), field(Some("cutoff"), permill)],
    );
    lab.probe(&[key], entry, "Optional", vec![]);
    let runtime = lab.handle();
    const FIELDS: &[ReadField] = &[
        ReadField {
            names: &["block"],
            class: ReadClass::Uint(64),
        },
        ReadField {
            names: &["cutoff"],
            class: ReadClass::Uint(32),
        },
    ];
    let error = refused(
        probe(&[ReadClass::Uint(16)], ReadClass::Record(FIELDS)),
        &runtime,
    );
    assert!(
        error.detail.contains("field `cutoff` is a per-thing ratio"),
        "{error}"
    );

    // A plain newtype around the same integer still unwraps.
    let mut lab = Lab::new();
    let key = lab.prim(TypeDefPrimitive::U16);
    let u32_ = lab.prim(TypeDefPrimitive::U32);
    let plain = lab.composite(&["Milli"], vec![field(None, u32_)]);
    lab.probe(&[key], plain, "Default", vec![0; 4]);
    let runtime = lab.handle();
    let storage = probe(&[ReadClass::Uint(16)], ReadClass::Uint(32));
    assert_eq!(
        decode(storage, &runtime, &500u32.encode()),
        ReadDatum::Uint(500)
    );
}

#[test]
fn key_changes_refuse_by_position() {
    // A key added to the map.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    lab.probe(&[u16_, u16_], u64_, "Default", vec![0; 8]);
    let error = refused(
        probe(&[ReadClass::Uint(16)], ReadClass::Uint(64)),
        &lab.handle(),
    );
    assert_eq!(error.part, "keys");
    assert!(error.detail.contains("2 keys, expected 1"), "{error}");
    // A key turned into a two-field struct.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    let pair = lab.composite(
        &["NetUidStorageIndex"],
        vec![field(Some("netuid"), u16_), field(Some("mechanism"), u16_)],
    );
    lab.probe(&[u16_, pair], u64_, "Default", vec![0; 8]);
    let error = refused(
        probe(
            &[ReadClass::Uint(16), ReadClass::Uint(16)],
            ReadClass::Uint(64),
        ),
        &lab.handle(),
    );
    assert_eq!(error.part, "key 1");
    // A key in a single-field newtype is still a scalar key.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    let newtype = lab.composite(&["NetUid"], vec![field(Some("id"), u16_)]);
    lab.probe(&[newtype], u64_, "Default", vec![0; 8]);
    let runtime = lab.handle();
    let plan = probe(&[ReadClass::Uint(16)], ReadClass::Uint(64))
        .plan(&runtime)
        .unwrap();
    assert!(
        plan.key(&[ReadDatum::Uint(1)])
            .unwrap()
            .ends_with(&1u16.encode())
    );
    assert_eq!(plan.decode(None).unwrap(), Some(ReadDatum::Uint(0)));
}

#[test]
fn enums_accept_added_variants_and_refuse_removed_or_unknown_ones() {
    let build = |added: Option<&str>, second: &str| {
        let mut lab = Lab::new();
        let u16_ = lab.prim(TypeDefPrimitive::U16);
        let byte = lab.prim(TypeDefPrimitive::U8);
        let mut variants = vec![("Yuma", 0, vec![]), (second, 1, vec![])];
        if let Some(added) = added {
            variants.push((added, 2, vec![byte]));
        }
        let value = lab.variants(&["EpochConsensus"], &variants);
        lab.probe(&[u16_], value, "Default", vec![0]);
        lab.handle()
    };
    let class = probe(
        &[ReadClass::Uint(16)],
        ReadClass::Variants(&["Yuma", "Null"]),
    );
    let runtime = build(Some("Weighted"), "Null");
    let plan = class.plan(&runtime).unwrap();
    assert_eq!(
        plan.decode(Some(&[1])).unwrap(),
        Some(ReadDatum::Variant("Null".into()))
    );
    // An added variant decodes by name, its payload consumed by the core.
    let weighted = plan.decode(Some(&[2, 0])).unwrap().unwrap();
    assert_eq!(weighted.as_variant(), Some("Weighted"));
    let error = plan.decode(Some(&[3])).unwrap_err();
    assert!(error.detail.contains("unknown variant index 3"), "{error}");

    let runtime = build(None, "Off");
    let error = refused(class, &runtime);
    assert!(error.detail.contains("variant `Null` is absent"), "{error}");
}

#[test]
fn optional_presence_refuses_a_default_modifier_and_defaults_are_checked() {
    let mut lab = Lab::new();
    lab.entry("Uids").modifier = "Default".into();
    let runtime = lab.handle();
    let uids = ResolvedStorage {
        pallet: PALLET,
        name: "Uids",
        keys: &[ReadClass::Uint(16), ReadClass::Account],
        value: ReadClass::Uint(16),
        presence: ReadPresence::Optional,
    };
    for error in [
        uids.check_presence(&runtime).unwrap_err(),
        refused(uids, &runtime),
    ] {
        assert_eq!(error.part, "modifier");
        assert!(error.detail.contains("no longer Optional"), "{error}");
    }

    // A Default entry whose live default no longer decodes refuses.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    lab.probe(&[u16_], u64_, "Default", vec![0; 2]);
    let error = refused(
        probe(&[ReadClass::Uint(16)], ReadClass::Uint(64)),
        &lab.handle(),
    );
    assert_eq!(error.part, "default");

    // A Default read of an entry that became Optional refuses only a miss.
    let mut lab = Lab::new();
    let u16_ = lab.prim(TypeDefPrimitive::U16);
    let u64_ = lab.prim(TypeDefPrimitive::U64);
    lab.probe(&[u16_], u64_, "Optional", vec![]);
    let runtime = lab.handle();
    let plan = probe(&[ReadClass::Uint(16)], ReadClass::Uint(64))
        .plan(&runtime)
        .unwrap();
    assert_eq!(
        plan.decode(Some(&5u64.encode())).unwrap(),
        Some(ReadDatum::Uint(5))
    );
    assert!(plan.decode(None).is_err());
}

#[test]
fn absent_entries_and_constants_refuse() {
    let runtime = Lab::new().handle();
    let error = refused(probe(&[], ReadClass::Uint(64)), &runtime);
    assert_eq!(error.part, "entry");
    assert!(read_constant(&runtime, "Aura", "Missing", &ReadClass::Uint(64)).is_err());
    let error = read_constant(&runtime, "Aura", "SlotDuration", &ReadClass::Bool).unwrap_err();
    assert_eq!(error.part, "value");
    assert_eq!(
        error.to_string(),
        "Aura.SlotDuration value: live type is u64, expected a bool"
    );
}
