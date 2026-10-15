// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_hyperparams_units.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The generated hyperparameter catalog must retain authenticated Python data.
// pyt: sdk/python/codegen/check.py:472-507, check_units

use rtao_catalog::hyperparams::TABLE;
use rtao_primitives::hyperparams::{HyperparamStorage, HyperparamTable, RawInteger};
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn extracted() -> Result<Value, Box<dyn std::error::Error>> {
    let named: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/named_tables.json"
    ))?;
    Ok(named["tables"]["sdk/python/bittensor/hyperparams.py"].clone())
}

fn dictionary<'a>(
    document: &'a Value,
    name: &str,
) -> Result<&'a [Value], Box<dyn std::error::Error>> {
    Ok(document["values"][name]["value"]["dict"]
        .as_array()
        .ok_or("extracted dictionary missing")?)
}

fn raw_bound(value: &Value) -> Result<Option<RawInteger>, Box<dyn std::error::Error>> {
    if value.is_null() {
        Ok(None)
    } else {
        let number = value.to_string();
        Ok(Some(if number.starts_with('-') {
            RawInteger::Signed(number.parse()?)
        } else {
            RawInteger::Unsigned(number.parse()?)
        }))
    }
}

// pyt: unit/test_hyperparams_units.py::test_committed_descriptors_agree_with_hand_table
#[test]
fn hyperparam_units_gate() -> TestResult {
    for (name, meta) in TABLE.entries {
        assert!(!meta.short.is_empty(), "{name}: no short description");
        assert!(
            meta.short.chars().count() <= TABLE.short_max,
            "{name}: short description exceeds {} Unicode codepoints",
            TABLE.short_max
        );
        if let Some(metadata_kind) = TABLE.metadata_kind(name) {
            assert_eq!(
                meta.kind, metadata_kind,
                "{name}: metadata/hand kind mismatch"
            );
        }
        assert_eq!(TABLE.kind_of(name), meta.kind, "{name}: resolved kind");
    }
    let document = extracted()?;
    assert_eq!(
        TABLE.short_max.to_string(),
        document["values"]["SHORT_MAX"]["value"].to_string()
    );
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_metadata_identity_decides_kind
// pyt: unit/test_hyperparams_units.py::test_metadata_identity_overrides_hand_table
#[test]
fn exact_source_metadata_identities_override_then_fall_back_to_the_hand_table() {
    const METADATA: &[HyperparamStorage] = &[
        HyperparamStorage {
            parameter: "kappa",
            container: "SubtensorModule",
            name: "Kappa",
            source_ident: "",
            metadata_ident: "PerU16",
        },
        HyperparamStorage {
            parameter: "min_burn",
            container: "SubtensorModule",
            name: "MinBurn",
            source_ident: "",
            metadata_ident: "TaoBalance",
        },
        HyperparamStorage {
            parameter: "tempo",
            container: "SubtensorModule",
            name: "Tempo",
            source_ident: "",
            metadata_ident: "PerU16",
        },
    ];
    const BARE_KAPPA: &[HyperparamStorage] = &[
        HyperparamStorage {
            metadata_ident: "u16",
            ..METADATA[0]
        },
        METADATA[1],
    ];
    let metadata = HyperparamTable {
        storage: METADATA,
        ..TABLE
    };
    assert_eq!(metadata.metadata_kind("kappa"), Some("u16"));
    assert_eq!(metadata.kind_of("kappa"), "u16");
    assert_eq!(metadata.metadata_kind("min_burn"), Some("rao"));
    assert_eq!(metadata.kind_of("min_burn"), "rao");
    assert_eq!(metadata.kind_of("tempo"), "u16");
    assert_eq!(metadata.normalized("tempo", 32767), Some(32767.0 / 65535.0));
    let bare = HyperparamTable {
        storage: BARE_KAPPA,
        ..TABLE
    };
    assert_eq!(bare.metadata_kind("kappa"), None);
    assert_eq!(bare.kind_of("kappa"), "u16");
}

#[test]
fn hyperparam_entries_match_authenticated_extraction() -> TestResult {
    let document = extracted()?;
    let entries = dictionary(&document, "HYPERPARAMS")?;
    assert_eq!(TABLE.entries.len(), entries.len());
    for ((name, meta), expected) in TABLE.entries.iter().zip(entries) {
        assert_eq!(Some(*name), expected[0].as_str());
        assert_eq!(Some("Hyperparam"), expected[1]["constructor"].as_str());
        let fields = &expected[1]["fields"];
        assert_eq!(Some(meta.kind), fields["kind"].as_str(), "{name}");
        assert_eq!(Some(meta.doc), fields["doc"].as_str(), "{name}");
        assert_eq!(Some(meta.short), fields["short"].as_str(), "{name}");
        assert_eq!(meta.signed, fields["signed"].as_bool(), "{name}");
        assert_eq!(meta.bits.map(u64::from), fields["bits"].as_u64(), "{name}");
        assert_eq!(meta.minimum, raw_bound(&fields["minimum"])?, "{name}");
        assert_eq!(meta.maximum, raw_bound(&fields["maximum"])?, "{name}");
    }
    Ok(())
}

#[test]
fn hyperparam_storage_keeps_source_and_metadata_identities() -> TestResult {
    let document = extracted()?;
    let storage = dictionary(&document, "STORAGE_ITEMS")?;
    assert_eq!(TABLE.storage.len(), storage.len());
    for (actual, expected) in TABLE.storage.iter().zip(storage) {
        assert_eq!(Some(actual.parameter), expected[0].as_str());
        assert_eq!(Some(actual.container), expected[1]["pallet"].as_str());
        assert_eq!(Some(actual.name), expected[1]["item"].as_str());
        assert_eq!(
            actual.source_ident,
            expected[1]["source_value_type_ident"]
                .as_str()
                .unwrap_or("")
        );
        assert_eq!(
            Some(actual.metadata_ident),
            expected[1]["value_type_ident"].as_str()
        );
        let metadata_kind = TABLE
            .ident_kinds
            .iter()
            .find_map(|(ident, kind)| (*ident == actual.metadata_ident).then_some(*kind));
        assert_eq!(TABLE.metadata_kind(actual.parameter), metadata_kind);
    }
    let collateral = TABLE
        .storage
        .iter()
        .find(|item| item.parameter == "collateral_drain_ratio")
        .ok_or("collateral drain storage missing")?;
    assert_eq!(collateral.source_ident, "U64F64");
    assert_eq!(collateral.metadata_ident, "FixedU128");
    assert_eq!(TABLE.kind_of(collateral.parameter), "fixed128");
    assert_eq!(TABLE.codec_of(collateral.parameter), (false, 128));
    Ok(())
}

#[test]
fn hyperparam_codecs_scales_and_settings_match_authenticated_extraction() -> TestResult {
    let document = extracted()?;
    assert_eq!(
        serde_json::to_value(TABLE.ident_kinds)?,
        Value::Array(dictionary(&document, "_IDENT_KINDS")?.to_vec())
    );
    assert_eq!(
        serde_json::to_value(TABLE.fraction_denominators)?,
        Value::Array(dictionary(&document, "_FRACTION_DENOMINATOR")?.to_vec())
    );
    for (table, source) in [
        (TABLE.ident_codecs, "_IDENT_CODEC"),
        (TABLE.kind_codecs, "_KIND_CODEC"),
    ] {
        let expected = dictionary(&document, source)?;
        assert_eq!(table.len(), expected.len());
        for ((name, (signed, bits)), row) in table.iter().zip(expected) {
            assert_eq!(Some(*name), row[0].as_str());
            assert_eq!(Some(*signed), row[1]["tuple"][0].as_bool());
            assert_eq!(Some(u64::from(*bits)), row[1]["tuple"][1].as_u64());
        }
    }
    assert_eq!(
        TABLE.fixed128_one.to_string(),
        document["values"]["FIXED128_ONE"]["value"].to_string()
    );
    let settings: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/settings.json"
    ))?;
    assert_eq!(
        Some(TABLE.blocktime),
        settings["values"]["BLOCKTIME"]["value"].as_f64()
    );
    assert_eq!(
        Some(TABLE.rao_per_tao),
        settings["values"]["RAO_PER_TAO"]["value"].as_u64()
    );
    assert_eq!(
        Some(TABLE.tao_symbol),
        settings["values"]["TAO_SYMBOL"]["value"].as_str()
    );
    Ok(())
}

#[test]
fn hyperparam_catalog_preserves_raw_u64_max_and_human_rounding() -> TestResult {
    assert_eq!(
        TABLE.to_raw("adjustment_alpha", u64::MAX)?,
        RawInteger::Unsigned(u128::from(u64::MAX))
    );
    assert!(TABLE.to_raw("adjustment_alpha", 1.0).is_err());
    assert_eq!(
        TABLE.to_raw("collateral_drain_ratio", "10.0")?,
        RawInteger::Unsigned(184467440737095516160)
    );
    assert_eq!(
        TABLE.to_raw("burn_increase_mult", "1.0")?,
        RawInteger::Unsigned(18446744073709551616)
    );
    Ok(())
}
