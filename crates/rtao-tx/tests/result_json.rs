//! Python result dictionaries retain their field contract as native fate grows.

use rtao_client::{Events, Inclusion};
use rtao_primitives::{Balance, H256, Tao};
use rtao_tx::{ChainError, ExtrinsicResult, IntentError, WeightsSubmitted};
use serde_json::{Map, json};

#[test]
fn included_results_keep_complete_python_json_without_native_tracking_fields()
-> Result<(), IntentError> {
    // Independent expected fields follow result.py::ExtrinsicResult.to_dict:
    // all truthy optional identity fields, a float-TAO fee, and unchanged data.
    let expected = json!({
        "success": true,
        "message": "Success",
        "block_hash": "0x1111111111111111111111111111111111111111111111111111111111111111",
        "extrinsic_id": "123-4",
        "explorer_url": "https://example.test/extrinsic/123-4",
        "fee_tao": 0.125,
        "data": {"reveal_round": 7654, "unknown_python_extra": [1, "kept"]}
    });
    let runtime = rtao_runtime::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?;
    // One Initialization/Balances.Issued event (17 rao, no topics). Python's
    // to_dict omits events even when its in-memory outcome contains them.
    let events = Events::decode(
        runtime,
        vec![4, 2, 5, 15, 17, 0, 0, 0, 0, 0, 0, 0, 0].into(),
    )?;
    assert_eq!(events.records().len(), 1);
    for inclusion in [Inclusion::InBlock, Inclusion::Finalized] {
        let result = ExtrinsicResult {
            success: true,
            message: "Success".into(),
            extrinsic_hash: Some(H256([0x33; 32])),
            block_hash: Some(H256([0x11; 32])),
            extrinsic_id: Some("123-4".into()),
            explorer_url: Some("https://example.test/extrinsic/123-4".into()),
            fee: Some(Balance::<Tao>::from_rao(125_000_000)),
            events: events.records().to_vec(),
            error: None,
            data: Map::from_iter([
                ("reveal_round".into(), json!(7654)),
                ("unknown_python_extra".into(), json!([1, "kept"])),
            ]),
            output: (),
            inclusion,
            fate_unknown: None,
        };
        let result = result.with_output::<WeightsSubmitted>()?;
        assert_eq!(result.output.reveal_round, Some(7654));
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.inclusion, inclusion);
        assert_eq!(result.fate_unknown, None);
        assert_eq!(result.extrinsic_hash, Some(H256([0x33; 32])));
        assert_eq!(result.to_json(), expected);
    }
    Ok(())
}

#[test]
fn fate_unknown_results_keep_complete_python_failure_json_and_native_recovery_hash()
-> Result<(), IntentError> {
    let result = ExtrinsicResult {
        success: false,
        message: "transmitted outcome sentinel".into(),
        extrinsic_hash: Some(H256([0x22; 32])),
        block_hash: None,
        extrinsic_id: None,
        explorer_url: None,
        fee: None,
        events: Vec::new(),
        error: Some(ChainError::with_code(
            "transmitted outcome sentinel",
            None,
            rtao_catalog::ErrorCode::Unknown,
        )),
        data: Map::new(),
        output: (),
        inclusion: Inclusion::Unobserved,
        fate_unknown: Some(H256([0x22; 32])),
    }
    .with_output::<WeightsSubmitted>()?;
    assert_eq!(result.inclusion, Inclusion::Unobserved);
    assert_eq!(result.fate_unknown, Some(H256([0x22; 32])));
    assert_eq!(result.extrinsic_hash, Some(H256([0x22; 32])));
    assert_eq!(
        result.to_json(),
        json!({
            "success": false,
            "message": "transmitted outcome sentinel",
            "error": {
                "message": "transmitted outcome sentinel",
                "name": null,
                "code": "unknown",
                "remediation": "inspect the message for details"
            }
        })
    );
    Ok(())
}
