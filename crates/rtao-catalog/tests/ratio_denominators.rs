//! The primitive ratio table must agree with the extracted pinned Python data.

use rtao_primitives::ratio::RATIO_TYPE_DENOMINATORS;

#[test]
fn ratio_denominators_match_extracted_python_table() -> Result<(), Box<dyn std::error::Error>> {
    let extracted: serde_json::Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/named_tables.json"
    ))?;
    let table = &extracted["tables"]["sdk/python/bittensor/hyperparams.py"]["values"]["RATIO_TYPE_DENOMINATORS"]
        ["value"]["dict"];
    let expected: serde_json::Value = serde_json::to_value(RATIO_TYPE_DENOMINATORS)?;
    assert_eq!(table, &expected);
    Ok(())
}
