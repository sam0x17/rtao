//! Python receipt semantics over the actual pinned portable runtime.

use bittensor_core::{codec::value::Value, runtime::Runtime};
use rtao_core_ext::{
    ops::{dispatch_error, extrinsic_lookup, fee, receipt, runtime_api},
    value_ext::ValueExt,
};
use serde_json::json;
use std::error::Error;

fn runtime() -> Result<Runtime, Box<dyn Error>> {
    Ok(Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?)
}
fn record(module: &str, event: &str, attributes: Value) -> Value {
    Value::record(vec![(
        "event".into(),
        Value::record(vec![
            ("module_id".into(), Value::str(module)),
            ("event_id".into(), Value::str(event)),
            ("attributes".into(), attributes),
        ]),
    )])
}
fn success() -> Value {
    record(
        "System",
        "ExtrinsicSuccess",
        Value::record(vec![(
            "dispatch_info".into(),
            Value::record(vec![(
                "weight".into(),
                Value::record(vec![
                    ("ref_time".into(), Value::Int(42)),
                    ("proof_size".into(), Value::Int(9)),
                ]),
            )]),
        )]),
    )
}

#[test]
fn every_nested_wrapper_overrides_outer_success() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    for (module, event, field, bare) in [
        ("Sudo", "Sudid", "sudo_result", false),
        ("Sudo", "SudoAsDone", "sudo_result", false),
        ("Proxy", "ProxyExecuted", "result", false),
        ("Multisig", "MultisigExecuted", "result", false),
        ("Utility", "BatchInterrupted", "error", true),
        ("Utility", "ItemFailed", "error", true),
    ] {
        let error = if bare {
            Value::str("BadOrigin")
        } else {
            Value::record(vec![("Err".into(), Value::str("BadOrigin"))])
        };
        let events = [
            record(module, event, Value::record(vec![(field.into(), error)])),
            success(),
        ];
        let outcome = receipt::resolve_outcome(&runtime, &events)?;
        assert!(!outcome.success, "missed {module}.{event}");
        assert_eq!(
            outcome.error.as_ref().map(|error| error.name.as_str()),
            Some("BadOrigin")
        );
    }
    let ok = record(
        "Sudo",
        "Sudid",
        Value::record(vec![(
            "sudo_result".into(),
            Value::record(vec![("Ok".into(), Value::Null)]),
        )]),
    );
    assert!(receipt::resolve_outcome(&runtime, &[ok, success()])?.success);
    assert!(!receipt::resolve_outcome(&runtime, &[])?.success);
    Ok(())
}

#[test]
fn fee_paid_last_wins_and_prevents_native_fallback_overflow() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    let deposit = record(
        "Balances",
        "Deposit",
        Value::record(vec![("amount".into(), Value::Int(i128::from(u64::MAX)))]),
    );
    let treasury = record(
        "Treasury",
        "Deposit",
        Value::record(vec![("value".into(), Value::Int(2))]),
    );
    assert!(receipt::resolve_outcome(&runtime, &[deposit.clone(), treasury.clone()]).is_err());
    let paid = |amount| {
        record(
            "TransactionPayment",
            "TransactionFeePaid",
            Value::record(vec![("actual_fee".into(), Value::Int(amount))]),
        )
    };
    assert_eq!(
        receipt::resolve_outcome(&runtime, &[deposit, paid(7), treasury, paid(9), success()])?
            .total_fee_amount,
        9
    );
    assert_eq!(
        receipt::resolve_outcome(
            &runtime,
            &[
                record(
                    "Balances",
                    "Deposit",
                    Value::record(vec![("amount".into(), Value::Int(3))])
                ),
                record(
                    "Treasury",
                    "Deposit",
                    Value::record(vec![("value".into(), Value::Int(5))])
                )
            ]
        )?
        .total_fee_amount,
        8
    );
    Ok(())
}

#[test]
fn all_shield_failure_names_weight_and_reason_shapes() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    for name in [
        "DecryptionFailed",
        "ExtrinsicDecodeFailed",
        "ExtrinsicExpired",
    ] {
        let outcome = receipt::resolve_outcome(
            &runtime,
            &[
                success(),
                record(
                    "MevShield",
                    name,
                    Value::record(vec![("reason".into(), Value::Bytes(vec![0, 255]))]),
                ),
            ],
        )?;
        assert!(!outcome.success);
        let error = outcome.error.ok_or("shield diagnostic missing")?;
        assert_eq!(error.kind, dispatch_error::DispatchErrorKind::MevShield);
        assert_eq!(error.docs, Value::Bytes(vec![0, 255]));
    }
    for name in ["DecryptedRejected", "ExtrinsicDispatchFailed"] {
        let attributes = if name == "DecryptedRejected" {
            Value::record(vec![(
                "reason".into(),
                Value::record(vec![
                    (
                        "post_info".into(),
                        Value::record(vec![("actual_weight".into(), Value::Int(99))]),
                    ),
                    ("error".into(), Value::str("BadOrigin")),
                ]),
            )])
        } else {
            Value::record(vec![("error".into(), Value::str("BadOrigin"))])
        };
        let outcome = receipt::resolve_outcome(
            &runtime,
            &[success(), record("MevShield", name, attributes)],
        )?;
        assert!(!outcome.success);
        assert_eq!(
            outcome.error.as_ref().map(|error| error.name.as_str()),
            Some("BadOrigin")
        );
        if name == "DecryptedRejected" {
            assert_eq!(outcome.weight, Value::Int(99));
        }
    }
    Ok(())
}

#[test]
fn system_error_wrappers_use_exact_extracted_docs_and_python_repr() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    let pallet = runtime.pallet("Balances").ok_or("Balances absent")?;
    let errors = runtime.resolve(pallet.errors_type.ok_or("Balances errors absent")?)?;
    let scale_info::TypeDef::Variant(errors) = &errors.type_def else {
        return Err("Balances error type not variant".into());
    };
    let variant = errors.variants.first().ok_or("Balances errors empty")?;
    let module = Value::record(vec![(
        "Module".into(),
        Value::record(vec![
            ("index".into(), Value::Int(i128::from(pallet.index))),
            ("error".into(), Value::Bytes(vec![variant.index, 0, 0, 0])),
        ]),
    )]);
    let module = dispatch_error::decode(&runtime, &module)?.ok_or("missing metadata error")?;
    assert_eq!(module.pallet.as_deref(), Some("Balances"));
    assert_eq!(module.name, variant.name);
    assert_eq!(
        module.docs,
        Value::List(variant.docs.iter().map(Value::str).collect())
    );
    let token = Value::record(vec![("Token".into(), Value::str("FundsUnavailable"))]);
    let error = dispatch_error::decode(&runtime, &token)?.ok_or("missing token error")?;
    assert_eq!(error.name, "FundsUnavailable");
    assert!(
        error
            .docs
            .as_str()
            .is_some_and(|docs| docs.contains("spendable balance"))
    );
    let direct = dispatch_error::decode(&runtime, &Value::str("BadOrigin"))?
        .ok_or("missing direct error")?;
    assert_eq!(direct.docs, Value::str("BadOrigin"));
    let arithmetic = Value::record(vec![(
        "Arithmetic".into(),
        Value::record(vec![("Overflow".into(), Value::Null)]),
    )]);
    assert_eq!(
        dispatch_error::decode(&runtime, &arithmetic)?
            .ok_or("missing arithmetic error")?
            .name,
        "Overflow"
    );
    let unrecognized = Value::record(vec![(
        "Unknown".into(),
        Value::Tuple(vec![
            Value::Null,
            Value::str("\u{200b}"),
            Value::Bytes(vec![0, 39]),
        ]),
    )]);
    assert_eq!(
        dispatch_error::decode(&runtime, &unrecognized)?
            .ok_or("missing unknown error")?
            .docs,
        Value::str("(None, '\\u200b', b\"\\x00'\")")
    );
    assert_eq!(
        dispatch_error::normalize_module_error(&Value::record(vec![(
            "Module".into(),
            Value::Tuple(vec![Value::Int(5), Value::Int(7)])
        )]))?,
        Some((5, 7))
    );
    assert_eq!(
        dispatch_error::normalize_module_error(&Value::record(vec![(
            "Module".into(),
            Value::record(vec![
                ("index".into(), Value::Int(5)),
                ("error".into(), Value::str("0x07010203"))
            ])
        )]))?,
        Some((5, 7))
    );
    assert!(
        dispatch_error::decode(
            &runtime,
            &Value::record(vec![(
                "Module".into(),
                Value::record(vec![
                    ("index".into(), Value::Int(255)),
                    ("error".into(), Value::str("0x00000000"))
                ])
            )])
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn result_message_preserves_python_truthiness_and_shield_reasons() -> Result<(), Box<dyn Error>> {
    let mut error = dispatch_error::DispatchErrorInfo {
        kind: dispatch_error::DispatchErrorKind::MevShield,
        pallet: None,
        name: "DecryptionFailed".into(),
        docs: Value::Bytes(vec![0, 255]),
    };
    assert_eq!(error.result_message()?, "b'\\x00\\xff'");
    error.docs = Value::record(vec![(
        "reason".into(),
        Value::Tuple(vec![Value::Bool(false), Value::str("\u{200b}")]),
    )]);
    assert_eq!(error.result_message()?, "{'reason': (False, '\\u200b')}");
    error.docs = Value::List(vec![Value::str("first"), Value::str("second")]);
    assert_eq!(error.result_message()?, "first second");
    for docs in [
        Value::Null,
        Value::Bool(false),
        Value::Int(0),
        Value::Uint(0),
        Value::U256([0; 32]),
        Value::str(""),
        Value::Bytes(vec![]),
        Value::List(vec![]),
        Value::Tuple(vec![]),
        Value::Dict(vec![]),
    ] {
        error.docs = docs;
        assert_eq!(error.result_message()?, "DecryptionFailed");
    }
    error.docs = Value::Bool(true);
    assert_eq!(error.result_message()?, "True");
    error.docs = Value::List(vec![Value::str("")]);
    assert_eq!(error.result_message()?, "");
    error.docs = Value::List(vec![Value::Int(1)]);
    assert!(error.result_message().is_err());
    error.name.clear();
    error.docs = Value::List(vec![]);
    assert_eq!(
        error.result_message()?,
        "{'type': 'MevShield', 'name': '', 'docs': []}"
    );
    Ok(())
}

#[test]
fn immortal_fee_probe_and_runtime_native_fee_width() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    let call = runtime.compose_call(
        "Balances",
        "transfer_keep_alive",
        &Value::record(vec![
            ("dest".into(), Value::Bytes(vec![7; 32])),
            ("value".into(), Value::Int(1)),
        ]),
    )?;
    let xt = fee::probe(&runtime, &call, [7; 32], 0, 77, [8; 32])?;
    let decoded = runtime.decode_extrinsic(&xt, true)?;
    assert_eq!(decoded.field("era").and_then(Value::as_str), Some("00"));
    assert_eq!(decoded.field("nonce").and_then(Value::as_u128), Some(77));
    assert_eq!(
        decoded
            .field("mode")
            .and_then(|mode| mode.field("mode"))
            .and_then(Value::as_str),
        Some("Disabled")
    );
    assert_eq!(
        decoded
            .field("signature")
            .and_then(|signature| signature.variant("Ed25519"))
            .and_then(Value::value_bytes),
        Some(vec![0; 64])
    );
    let request = fee::request(&xt, None)?;
    assert_eq!(request.method, "state_call");
    assert_eq!(request.params[0], "TransactionPaymentApi_query_info");
    let mut args = xt.clone();
    args.extend_from_slice(&u32::try_from(xt.len())?.to_le_bytes());
    assert_eq!(request.params[1], format!("0x{}", hex::encode(args)));
    assert!(request.params[2].is_null());
    let output =
        runtime_api::runtime_api_method(&runtime, "TransactionPaymentApi", "query_info")?.output;
    let value = Value::record(vec![
        ("partial_fee".into(), Value::Int(i128::from(u64::MAX))),
        ("class".into(), Value::str("Normal")),
        (
            "weight".into(),
            Value::record(vec![
                ("ref_time".into(), Value::Int(42)),
                ("proof_size".into(), Value::Int(9)),
            ]),
        ),
    ]);
    let mut encoded = Vec::new();
    runtime.encode_id(output, &value, &mut encoded)?;
    assert!(encoded.ends_with(&u64::MAX.to_le_bytes()));
    let estimate = fee::decode_bytes(&runtime, &encoded)?;
    assert_eq!(estimate.partial_fee, u64::MAX);
    assert_eq!(
        estimate.weight,
        value.field("weight").ok_or("weight missing")?.clone()
    );
    encoded.push(0);
    assert!(fee::decode_bytes(&runtime, &encoded).is_err());
    Ok(())
}

#[test]
fn lookup_hashes_bytes_and_never_invents_index_for_absent_hash() -> Result<(), Box<dyn Error>> {
    let runtime = runtime()?;
    let call = runtime.compose_call(
        "System",
        "remark",
        &Value::record(vec![("remark".into(), Value::Bytes(vec![0xAB, 0xCD]))]),
    )?;
    let xt = fee::probe(&runtime, &call, [7; 32], 0, 0, [8; 32])?;
    let decoded = runtime.decode_extrinsic(&xt, true)?;
    let hash: [u8; 32] = decoded
        .field("extrinsic_hash")
        .and_then(Value::value_bytes)
        .ok_or("hash missing")?
        .try_into()
        .map_err(|_| "hash width invalid")?;
    let block = json!({"block":{"header":{"number":"0x0c"},"extrinsics":["0x00",format!("0x{}",hex::encode_upper(&xt)),"malformed later entry"]}});
    assert_eq!(extrinsic_lookup::block_number(&block)?, 12);
    assert_eq!(extrinsic_lookup::find(&block, hash)?, Some(1));
    assert_eq!(
        extrinsic_lookup::find(&json!({"block":{"extrinsics":null}}), [0; 32])?,
        None
    );
    assert!(extrinsic_lookup::find(&block, [0; 32]).is_err());
    assert_eq!(
        extrinsic_lookup::missing_message("0x0123", "0x4567", 12),
        "extrinsic 0x0123 not found in block 0x4567 (#12); the serving node may not have imported it yet"
    );
    Ok(())
}
