//! The generated public catalog preserves every authenticated upstream record.

use std::collections::{BTreeMap, BTreeSet};

use rtao_catalog::{CHAIN_ERRORS, INTENTS, READS, SEMANTIC_ERRORS, specs};
use serde_json::{Value, json};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[test]
fn every_intent_and_read_retains_its_complete_catalog_record() -> Result {
    let intents: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/catalog/intents.json"
    ))?;
    let reads: Value =
        serde_json::from_str(include_str!("../../../vendor/upstream/catalog/reads.json"))?;
    let intent_rows = intents.as_array().ok_or("intent catalog is not an array")?;
    let read_rows = reads.as_array().ok_or("read catalog is not an array")?;
    assert_eq!(INTENTS.len(), 84);
    assert_eq!(READS.len(), 102);
    assert_eq!(INTENTS.len(), intent_rows.len());
    assert_eq!(READS.len(), read_rows.len());
    let mut intent_names = BTreeSet::new();
    for row in intent_rows {
        let name = row["name"].as_str().ok_or("intent name missing")?;
        let spec = specs::intent(name).ok_or("intent absent from public catalog")?;
        assert!(intent_names.insert(spec.name));
        assert_eq!(serde_json::from_str::<Value>(spec.catalog_json)?, *row);
        assert_eq!(json!(spec.summary), row["summary"]);
        assert_eq!(json!(spec.description), row["description"]);
        assert_eq!(json!(spec.python_class), row["python_class"]);
        assert_eq!(json!(spec.signer), row["signer"]);
        assert_eq!(json!(spec.origin), row["origin"]);
        assert_eq!(json!(spec.verify), row["verify"]);
        assert_eq!(json!(spec.wraps), row["wraps"]);
        let properties = row["input_schema"]["properties"]
            .as_object()
            .ok_or("intent properties missing")?;
        assert_eq!(
            spec.fields
                .iter()
                .map(|field| field.name)
                .collect::<BTreeSet<_>>(),
            properties
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        );
    }
    let mut read_names = BTreeSet::new();
    for row in read_rows {
        let name = row["name"].as_str().ok_or("read name missing")?;
        let spec = specs::read(name).ok_or("read absent from public catalog")?;
        assert!(read_names.insert(spec.name));
        assert_eq!(serde_json::from_str::<Value>(spec.catalog_json)?, *row);
        assert_eq!(json!(spec.summary), row["summary"]);
        assert_eq!(json!(spec.description), row["description"]);
        assert_eq!(json!(spec.category), row["category"]);
        let params = row["params"].as_object().ok_or("read params missing")?;
        assert_eq!(
            spec.params
                .iter()
                .map(|param| (param.name, param.json_type))
                .collect::<BTreeMap<_, _>>(),
            params
                .iter()
                .map(|(name, kind)| (name.as_str(), kind.as_str()))
                .collect::<BTreeMap<_, _>>()
        );
        if let Some(expression) = row["python"].as_str() {
            assert_eq!(expression, format!("sub.{}.{name}(...)", spec.namespace));
        } else {
            assert_eq!(name, "metagraph");
            assert_eq!(spec.namespace, "subnets");
        }
    }
    assert!(specs::intent("nonexistent").is_none());
    assert!(specs::read("nonexistent").is_none());
    Ok(())
}

#[test]
fn all_semantic_and_chain_error_records_preserve_documentation_and_sources() -> Result {
    let catalog: Value =
        serde_json::from_str(include_str!("../../../vendor/upstream/catalog/errors.json"))?;
    let semantic = catalog["codes"]
        .as_object()
        .ok_or("semantic codes missing")?;
    let chain = catalog["chain_errors"]
        .as_object()
        .ok_or("chain errors missing")?;
    assert_eq!(SEMANTIC_ERRORS.len(), 18);
    assert_eq!(CHAIN_ERRORS.len(), 392);
    let mut semantic_names = BTreeSet::new();
    for spec in &SEMANTIC_ERRORS {
        assert!(semantic_names.insert(spec.code.as_str()));
        let row = semantic
            .get(spec.code.as_str())
            .ok_or("semantic row absent")?;
        assert_eq!(json!(spec.remediation), row["remediation"]);
        assert_eq!(json!(spec.docs_url), row["docs_url"]);
        assert_eq!(json!(spec.markdown_url), row["markdown_url"]);
        assert!(!spec.explanation.is_empty());
    }
    assert_eq!(semantic_names.len(), semantic.len());
    let mut chain_names = BTreeSet::new();
    for spec in &CHAIN_ERRORS {
        assert!(chain_names.insert(spec.name));
        let row = chain.get(spec.name).ok_or("chain row absent")?;
        assert_eq!(serde_json::from_str::<Value>(spec.catalog_json)?, *row);
        assert_eq!(json!(spec.code), row["code"]);
        assert_eq!(json!(spec.description), row["description"]);
        assert_eq!(json!(spec.docs_url), row["docs_url"]);
        assert_eq!(json!(spec.markdown_url), row["markdown_url"]);
        assert_eq!(json!(spec.pallets), row["pallets"]);
        match (spec.sources_json, row.get("sources")) {
            (Some(actual), Some(expected)) => {
                assert_eq!(serde_json::from_str::<Value>(actual)?, *expected)
            }
            (None, None) => {}
            _ => return Err("source-reference absence changed".into()),
        }
    }
    assert_eq!(chain_names.len(), chain.len());
    Ok(())
}

#[test]
fn defaults_distinguish_omission_none_factories_and_own_key_fallback() -> Result {
    let field = |name, parameter| -> Result<&'static specs::ParamSpec> {
        specs::intent(name)
            .ok_or("intent absent")?
            .fields
            .iter()
            .find(|field| field.name == parameter)
            .ok_or_else(|| "field absent".into())
    };
    let required = field("add_stake", "hotkey_ss58")?;
    assert!(required.required);
    assert_eq!(required.default_json, None);
    let optional_none = field("burned_register", "hotkey_ss58")?;
    assert!(!optional_none.required);
    assert_eq!(optional_none.default_json, Some("null"));
    let factory = field("claim_root", "subnets")?;
    assert!(factory.default_factory);
    assert_eq!(factory.default_json, Some("[0]"));
    assert!(
        specs::intent("burned_register")
            .ok_or("intent absent")?
            .mev_shield_required
    );
    assert!(
        !specs::intent("transfer")
            .ok_or("intent absent")?
            .mev_shield_required
    );
    let own_key = specs::read("balance")
        .ok_or("read absent")?
        .params
        .iter()
        .find(|param| param.name == "coldkey_ss58")
        .ok_or("balance coldkey absent")?;
    assert!(own_key.own_key);
    assert!(!own_key.required);
    let provenance: Value = serde_json::from_str(factory.provenance_json)?;
    assert!(
        provenance["blob"]
            .as_str()
            .is_some_and(|blob| blob.len() == 40)
    );
    assert!(provenance["start_byte"].as_u64() < provenance["end_byte"].as_u64());
    Ok(())
}

#[test]
fn shared_python_declaration_index_round_trips_every_shared_value_and_span() -> Result {
    let tables: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/named_tables.json"
    ))?;
    let tables = tables["tables"].as_object().ok_or("named tables absent")?;
    let mut count = 0;
    for (path, table) in tables {
        if path == "sdk/python/bittensor/basket_index_data.py" {
            // Frozen price history belongs to the CLI, so neuron consumers of
            // rtao-catalog do not compile it. Extraction retains its evidence.
            for name in table["values"]
                .as_object()
                .ok_or("basket values absent")?
                .keys()
            {
                assert!(rtao_catalog::py::table(path, name).is_none());
            }
            continue;
        }
        for (name, row) in table["values"].as_object().ok_or("values absent")? {
            let spec = rtao_catalog::py::table(path, name).ok_or("Python table absent")?;
            assert_eq!(
                serde_json::from_str::<Value>(spec.value_json)?,
                row["value"]
            );
            assert_eq!(json!(spec.blob), table["provenance"]["blob"]);
            assert_eq!(json!(spec.line), row["line"]);
            assert_eq!(json!(spec.end_line), row["end_line"]);
            count += 1;
        }
    }
    assert_eq!(count, rtao_catalog::py::PYTHON_TABLES.len());
    Ok(())
}
