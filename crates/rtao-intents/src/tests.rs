#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::WeightsArg;
use rtao_primitives::Uid;

#[test]
fn weights_decode_arbitrary_precision_numbers_through_values() {
    let weights: WeightsArg = serde_json::from_str("{\"10\":0.1,\"2\":0.125}").unwrap();
    let (uids, weights) = weights.as_pairs(None).unwrap();
    assert_eq!(uids, [Uid(2), Uid(10)]);
    assert_eq!(weights, [0.125, 0.1]);
    let weights: WeightsArg = serde_json::from_str("[0.1,0.2]").unwrap();
    assert_eq!(
        weights.as_pairs(Some(&[Uid(0), Uid(1)])).unwrap().1,
        [0.1, 0.2]
    );
}

#[test]
fn weights_reject_mixed_shape_and_duplicate_numeric_uid_aliases() {
    let weights: WeightsArg = serde_json::from_str("{\"01\":1,\"1\":2}").unwrap();
    assert_eq!(
        weights.as_pairs(None).unwrap_err().to_string(),
        "Duplicate UIDs in weights; each UID may appear once."
    );
    assert_eq!(
        weights.as_pairs(Some(&[Uid(1)])).unwrap_err().to_string(),
        "Pass weights as a {uid: weight} mapping OR as parallel uids/weights lists, not both."
    );
    let weights: WeightsArg = serde_json::from_str("[1]").unwrap();
    assert_eq!(
        weights.as_pairs(None).unwrap_err().to_string(),
        "Provide weights as a {uid: weight} mapping, or uids and weights as parallel lists."
    );
    assert_eq!(
        weights
            .as_pairs(Some(&[Uid(1), Uid(1)]))
            .unwrap_err()
            .to_string(),
        "Duplicate UIDs in weights; each UID may appear once."
    );
}

#[test]
fn weight_uid_mapping_uses_python_integer_syntax_before_native_bounds() {
    let weights: WeightsArg =
        serde_json::from_str("{\" -0 \":1,\"١_٢\":2,\"+６５５３５\":3}").unwrap();
    assert_eq!(
        weights.as_pairs(None).unwrap(),
        (vec![Uid(0), Uid(12), Uid(65535)], vec![1.0, 2.0, 3.0])
    );
    let duplicate: WeightsArg = serde_json::from_str("{\"０\":1,\"-0\":2}").unwrap();
    assert_eq!(
        duplicate.as_pairs(None).unwrap_err().to_string(),
        "Duplicate UIDs in weights; each UID may appear once."
    );
    for spelling in ["-1", "65536", "999999999999999999999999999999999999999"] {
        let weights: WeightsArg = serde_json::from_value(serde_json::json!({spelling:1})).unwrap();
        assert_eq!(
            weights.as_pairs(None).unwrap_err().to_string(),
            "UIDs must be in 0..65535."
        );
    }
    for spelling in ["1__2", "1.0", "1e0", "0x1", "¹", "\u{1c}1"] {
        let weights: WeightsArg = serde_json::from_value(serde_json::json!({spelling:1})).unwrap();
        assert_eq!(
            weights.as_pairs(None).unwrap_err().to_string(),
            format!(
                "invalid literal for int() with base 10: {}",
                rtao_primitives::pyfmt::string_repr(spelling)
            )
        );
    }
}

#[test]
fn weights_accept_python_float_text_before_canonical_numeric_serialization() {
    let mapping: WeightsArg = serde_json::from_value(serde_json::json!({
        "0": " 1.0 ", "1": "１_２.٥e-١", "2": "\u{a0}.5\u{2003}",
        "3": true, "4": false
    }))
    .unwrap();
    assert_eq!(
        mapping.as_pairs(None).unwrap(),
        (
            vec![Uid(0), Uid(1), Uid(2), Uid(3), Uid(4)],
            vec![1.0, 1.25, 0.5, 1.0, 0.0]
        )
    );
    let parallel: WeightsArg =
        serde_json::from_value(serde_json::json!([" +1. ", "1_2.5e-1", "٥e-١"])).unwrap();
    assert_eq!(
        parallel.as_pairs(Some(&[Uid(0), Uid(1), Uid(2)])).unwrap(),
        (vec![Uid(0), Uid(1), Uid(2)], vec![1.0, 1.25, 0.5])
    );
    let booleans: WeightsArg = serde_json::from_value(serde_json::json!([true, false])).unwrap();
    assert_eq!(
        booleans.as_pairs(Some(&[Uid(0), Uid(1)])).unwrap().1,
        [1.0, 0.0]
    );
    for spelling in ["1__0", "¹", "\u{1c}1", "NaN", "Infinity"] {
        assert!(
            serde_json::from_value::<WeightsArg>(serde_json::json!({"0": spelling})).is_err(),
            "{spelling:?} must not produce a finite weight"
        );
    }
}

#[cfg(feature = "serving")]
#[test]
fn tool_arguments_require_an_object_and_keep_unknown_op_precedence() {
    use serde_json::json;
    for args in [json!([1]), json!(null), json!(1), json!("1"), json!(true)] {
        assert_eq!(
            crate::AnyIntent::from_tool("reset_axon", args.clone())
                .unwrap_err()
                .to_string(),
            "Intent arguments must be a JSON object."
        );
        assert_eq!(
            crate::AnyIntent::from_tool("unknown", args)
                .unwrap_err()
                .to_string(),
            "Unknown or disabled intent: unknown"
        );
    }
    let intent =
        crate::AnyIntent::from_tool("reset_axon", json!({"op":"ignored", "netuid":1})).unwrap();
    assert_eq!(intent.to_json(), json!({"op":"reset_axon", "netuid":1}));
}

#[cfg(feature = "weights")]
#[test]
fn weight_tool_construction_canonicalizes_the_python_mapping_shape() {
    let intent = crate::AnyIntent::from_tool(
        "set_weights",
        serde_json::json!({"op":"ignored","netuid":1,"weights":{"10":0.1,"2":0.2}}),
    )
    .unwrap();
    let json = intent.to_json();
    assert_eq!(json["uids"], serde_json::json!([2, 10]));
    assert_eq!(json["weights"], serde_json::json!([0.2, 0.1]));
    assert_eq!(json["op"], "set_weights");
    assert_eq!(
        crate::AnyIntent::from_tool("set_weights", json).unwrap(),
        intent
    );
    for op in ["set_weights", "commit_weights"] {
        let intent = crate::AnyIntent::from_tool(
            op,
            serde_json::json!({"netuid":1,"weights":{"0":" 1.0 ","1":"１_２.٥e-١","2":true,"3":false}}),
        )
        .unwrap();
        assert_eq!(intent.to_json()["uids"], serde_json::json!([0, 1, 2, 3]));
        assert_eq!(
            intent.to_json()["weights"],
            serde_json::json!([1.0, 1.25, 1.0, 0.0])
        );
    }
}

#[cfg(feature = "all")]
#[test]
fn all_83_interfaces_preserve_defaults_specs_field_sets_and_tool_roundtrip() {
    use crate::AnyIntent;
    use serde_json::{Value, json};
    let address = rtao_primitives::AccountId([1; 32]).to_ss58(110);
    let mut tested = std::collections::BTreeSet::new();
    for spec in &rtao_catalog::INTENTS {
        let catalog: Value = serde_json::from_str(spec.catalog_json).unwrap();
        let mut args = serde_json::Map::new();
        for field in spec.fields.iter().filter(|field| field.required) {
            let annotation = field.annotation.unwrap_or("");
            let value = if field.name.ends_with("_ss58") {
                json!(address)
            } else if annotation == "Money" {
                json!("1.000000001")
            } else if annotation.starts_with("list") {
                json!([])
            } else if annotation == "dict" {
                json!({})
            } else if annotation == "bool" {
                json!(false)
            } else if annotation == "str" {
                json!("sample")
            } else {
                json!(1)
            };
            args.insert(field.name.into(), value);
        }
        if matches!(spec.name, "set_weights" | "commit_weights") {
            args.insert("uids".into(), json!([0]));
            args.insert("weights".into(), json!([1.0]));
        }
        if spec.name == "pow_register" {
            args.insert("work_hex".into(), json!("00".repeat(32)));
        }
        if spec.name == "set_hyperparameter" {
            args.insert("name".into(), json!("min_burn"));
        }
        if spec.name == "create_crowdloan" {
            // Optional individually, but Python requires one funding target.
            args.insert("target_ss58".into(), json!(address));
        }
        if spec.name == "set_subnet_emission_enabled" {
            args.insert("netuids".into(), json!([1]));
        }
        if spec.name == "execute_proxy_announced" {
            args.insert("inner_op".into(), json!("finalize_crowdloan"));
            args.insert("inner_args".into(), json!({"crowdloan_id":1}));
        }
        if spec.name == "fund_evm_key" {
            args.insert(
                "evm_address".into(),
                json!(format!("0x{}", "11".repeat(20))),
            );
        }
        if spec.name == "batch" {
            args.insert(
                "intents".into(),
                json!([{"op":"transfer_all","dest_ss58":address}]),
            );
        }
        if matches!(spec.name, "swap_stake" | "move_swap_stake") {
            args.insert("dest_netuid".into(), json!(2));
        }
        if spec.name == "move_swap_stake" {
            args.insert(
                "dest_hotkey_ss58".into(),
                json!(rtao_primitives::AccountId([2; 32]).to_ss58(110)),
            );
        }
        if matches!(spec.name, "swap_basket" | "swap_basket_many") {
            if spec.name == "swap_basket" {
                args.insert("dest_netuid".into(), json!(2));
            } else {
                args.insert(
                    "legs".into(),
                    json!([{"origin_netuid":1,"dest_netuid":2,"amount":"1"}]),
                );
            }
        }
        let supplied: std::collections::BTreeSet<_> = args.keys().cloned().collect();
        // Some constructors require distinct accounts (MoveSwapStake above).
        // Retain each supplied spelling rather than assuming all address
        // fields use the common fixture account.
        let address_spellings: std::collections::BTreeMap<_, _> = spec
            .fields
            .iter()
            .filter(|field| field.required && field.name.ends_with("_ss58"))
            .map(|field| (field.name, args[field.name].clone()))
            .collect();
        let constructed = AnyIntent::from_tool(spec.name, Value::Object(args));
        assert!(
            constructed.is_ok(),
            "{} catalog sample constructor: {constructed:?}",
            spec.name
        );
        let intent = constructed.unwrap();
        assert!(
            tested.insert(spec.name),
            "{} duplicate catalog entry",
            spec.name
        );
        assert_eq!(intent.spec(), spec);
        let json = intent.to_json();
        assert_eq!(json["op"], spec.name);
        for (field, spelling) in address_spellings {
            assert_eq!(
                json[field], spelling,
                "{}.{field} address spelling",
                spec.name
            );
        }
        let actual: std::collections::BTreeSet<_> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|key| *key != "op")
            .collect();
        let expected: std::collections::BTreeSet<_> = catalog["input_schema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, expected, "{} fields", spec.name);
        assert_eq!(
            AnyIntent::from_tool(spec.name, json.clone()).unwrap(),
            intent,
            "{} roundtrip",
            spec.name
        );
        for field in spec
            .fields
            .iter()
            .filter(|field| !field.required && !supplied.contains(field.name))
        {
            let expected: Value = serde_json::from_str(field.default_json.unwrap()).unwrap();
            if field.annotation == Some("Money") {
                continue;
            }
            assert_eq!(
                json[field.name], expected,
                "{}.{} default",
                spec.name, field.name
            );
        }
        if spec.name == "create_crowdloan" {
            // Exercise the other valid constructor so both optional defaults
            // are still checked despite their mutually exclusive constraint.
            assert_eq!(json["call"], Value::Null);
            let mut nested = json.clone();
            nested.as_object_mut().unwrap().remove("target_ss58");
            nested["call"] = json!({"op":"finalize_crowdloan","crowdloan_id":1});
            let nested_intent = AnyIntent::from_tool(spec.name, nested).unwrap();
            let nested_json = nested_intent.to_json();
            assert_eq!(nested_json["target_ss58"], Value::Null);
            assert_eq!(
                nested_json["call"],
                json!({"op":"finalize_crowdloan","crowdloan_id":1})
            );
            assert_eq!(
                AnyIntent::from_tool(spec.name, nested_json).unwrap(),
                nested_intent
            );
        }
        for field in spec
            .fields
            .iter()
            .filter(|field| !field.required && field.name.ends_with("_ss58"))
        {
            let mut explicit = json.clone();
            explicit[field.name] = json!(address);
            let intent = AnyIntent::from_tool(spec.name, explicit).unwrap();
            assert_eq!(
                intent.to_json()[field.name],
                address,
                "{}.{} explicit address spelling",
                spec.name,
                field.name
            );
        }
        let mut unknown = json.clone();
        unknown["unexpected_field"] = json!(true);
        assert!(
            AnyIntent::from_tool(spec.name, unknown).is_err(),
            "{} unknown field",
            spec.name
        );
        for field in spec.fields.iter().filter(|field| field.required) {
            let mut missing = json.clone();
            missing.as_object_mut().unwrap().remove(field.name);
            assert!(
                AnyIntent::from_tool(spec.name, missing).is_err(),
                "{}.{} required",
                spec.name,
                field.name
            );
        }
    }
    assert_eq!(rtao_catalog::INTENTS.len(), 83);
    assert_eq!(tested.len(), 83);
}

#[cfg(feature = "serving")]
#[test]
fn serving_interfaces_preserve_hotkey_role_origin_defaults_and_netuid() {
    use crate::{ResetAxon, ServeAxon, ServeAxonTls, ServePrometheus};
    use rtao_signer::Role;
    use rtao_tx::{Intent, IntentInterface, OriginMarker};
    fn check<I: Intent>(intent: &I) {
        assert_eq!(I::Role::KIND, rtao_signer::SignerRole::Hotkey);
        assert_eq!(I::Origin::NAME, "signed");
        assert_eq!(intent.spend(), rtao_primitives::Spend::None);
        assert_eq!(
            intent.touches_netuids().as_slice(),
            &[rtao_primitives::NetUid(1)]
        );
        assert!(!intent.affects_all_subnets());
        assert!(intent.validate().is_ok());
        assert_eq!(intent.semantic_intent().spec(), I::SPEC);
    }
    let axon: ServeAxon =
        serde_json::from_str("{\"netuid\":1,\"ip\":\"invalid-at-build\",\"port\":1}").unwrap();
    check(&axon);
    assert_eq!(axon.protocol, 4);
    assert_eq!(axon.version, 1);
    check(&ServeAxonTls {
        netuid: axon.netuid,
        ip: axon.ip.clone(),
        port: 1,
        certificate: "invalid-at-build".into(),
        protocol: 4,
        version: 1,
    });
    check(&ServePrometheus {
        netuid: axon.netuid,
        ip: axon.ip.clone(),
        port: 1,
        version: 1,
    });
    check(&ResetAxon {
        netuid: axon.netuid,
    });
    assert!(!<ServeAxon as IntentInterface>::SPEC.mev_shield_required);
}
