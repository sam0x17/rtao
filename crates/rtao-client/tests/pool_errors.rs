//! Pool normalization preserves Python's unknown custom-code diagnostics.

use std::{collections::BTreeMap, error::Error};

use rtao_client::PoolRejection;
use serde_json::{Value, json};

#[test]
fn every_custom_byte_resolves_or_retains_the_original_structured_diagnostic()
-> Result<(), Box<dyn Error>> {
    let source: Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/extracted/transaction_errors.json"
    ))?;
    let names: BTreeMap<_, _> = source["python_mirror"]["source"]["value"]["dict"]
        .as_array()
        .ok_or("custom-code extraction absent")?
        .iter()
        .map(|row| {
            Ok((
                row[0].as_u64().ok_or("custom code absent")?,
                row[1].as_str().ok_or("custom name absent")?,
            ))
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    assert_eq!(names.len(), 29);
    for custom in 0..=u8::MAX {
        let data = json!({"Custom": custom});
        let rejection = PoolRejection::from_rpc(1010, "Invalid Transaction", Some(&data));
        assert_eq!(
            rejection.name(),
            names.get(&u64::from(custom)).copied(),
            "{custom}"
        );
        if names.contains_key(&u64::from(custom)) {
            assert_eq!(rejection, PoolRejection::Custom(custom), "{custom}");
        } else {
            assert_eq!(
                rejection,
                PoolRejection::Unknown(format!("Invalid Transaction: {{'Custom': {custom}}}")),
                "{custom}"
            );
        }
    }
    Ok(())
}

#[test]
fn an_unmapped_payload_code_is_authoritative_before_watch_or_standard_fallback() {
    for custom in [
        json!(256),
        json!(-1),
        json!("999999999999999999999999999999999999999"),
    ] {
        let data = json!({"Custom": custom});
        let rejection = PoolRejection::from_rpc(1010, "Custom error: 8", Some(&data));
        assert!(matches!(rejection, PoolRejection::Unknown(_)), "{custom}");
        assert_eq!(rejection.name(), None, "{custom}");
    }
    let future = PoolRejection::from_rpc(
        1010,
        "will be valid in the future",
        Some(&json!({"Custom": 28})),
    );
    assert_eq!(future, PoolRejection::Future);
    let stale = PoolRejection::from_rpc(
        1010,
        "transaction is outdated",
        Some(&json!({"Custom": 256})),
    );
    assert_eq!(stale, PoolRejection::Stale);
    let watch =
        PoolRejection::from_rpc(1014, "Extrinsic DROPPED: {}", Some(&json!({"Custom": 28})));
    assert!(matches!(watch, PoolRejection::Watch(_)));
    assert_eq!(watch.name(), Some("Dropped"));
    assert_eq!(
        PoolRejection::from_rpc(1010, "Custom error: 8", Some(&json!("Custom error: 256"))),
        PoolRejection::Unknown("Custom error: 8: Custom error: 256".into())
    );
}

#[test]
fn custom_payload_integer_coercion_matches_python_and_keeps_malformed_data() {
    for (data, custom) in [
        (json!(true), 1),
        (json!(false), 0),
        (json!({"Custom": 1.9}), 1),
        (json!({"Custom": " ١_٢ "}), 12),
    ] {
        assert_eq!(
            PoolRejection::from_rpc(1010, "opaque", Some(&data)),
            PoolRejection::Custom(custom),
            "{data}"
        );
    }
    for (data, expected) in [
        (json!(1.0), "opaque: 1.0"),
        (json!({"Custom": {}}), "opaque: {'Custom': {}}"),
        (
            json!([1, "two", true, null]),
            "opaque: [1, 'two', True, None]",
        ),
        (
            json!({"detail": "x", "other": false}),
            "opaque: {'detail': 'x', 'other': False}",
        ),
        (json!(null), "opaque"),
    ] {
        assert_eq!(
            PoolRejection::from_rpc(1010, "opaque", Some(&data)),
            PoolRejection::Unknown(expected.into()),
            "{data}"
        );
    }
    assert_eq!(
        PoolRejection::from_rpc(1010, "", None),
        PoolRejection::Unknown("Request failed".into())
    );
    assert_eq!(
        PoolRejection::from_rpc(1010, "Custom error: 8", Some(&json!({"Custom": "bad"}))),
        PoolRejection::Custom(8)
    );
}

#[test]
fn oversized_custom_integer_strings_allow_message_fallback_without_discarding_valid_bigints()
-> Result<(), Box<dyn Error>> {
    let accepted = json!({"Custom": "9".repeat(4_300)});
    assert!(matches!(
        PoolRejection::from_rpc(1010, "Custom error: 8", Some(&accepted)),
        PoolRejection::Unknown(_)
    ));
    for spelling in [
        "9".repeat(4_301),
        format!(" +{} ", "٩_".repeat(4_300) + "٩"),
    ] {
        let data = json!({"Custom":spelling});
        assert_eq!(
            PoolRejection::from_rpc(1010, "Custom error: 8", Some(&data)),
            PoolRejection::Custom(8)
        );
    }
    // Structured numeric integers keep their existing arbitrary-size behavior.
    let integer: Value = serde_json::from_str(&"9".repeat(4_301))?;
    assert!(matches!(
        PoolRejection::from_rpc(1010, "Custom error: 8", Some(&json!({"Custom":integer}))),
        PoolRejection::Unknown(_)
    ));
    Ok(())
}
