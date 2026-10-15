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

#[test]
fn regex_custom_code_boundaries_preserve_precedence_and_stop_failed_normalization() {
    for digits in [4_300, 4_301] {
        // Leading zeroes count toward Python's limit, including Unicode digits.
        for capture in [
            format!("{}8", "0".repeat(digits - 1)),
            format!("{}٨", "٠".repeat(digits - 1)),
        ] {
            let data_text = format!("{{'Custom': {capture}}}");
            let data = json!(data_text);
            let message = "Custom error: 3; Extrinsic dropped; transaction is outdated";
            let actual = PoolRejection::from_rpc(1014, message, Some(&data));
            let expected = if digits == 4_300 {
                PoolRejection::Custom(8)
            } else {
                PoolRejection::InvalidCustomCode(format!("{message}: {data_text}"))
            };
            assert_eq!(actual, expected);
            assert_eq!(
                actual.name(),
                (digits == 4_300).then_some("SlippageTooHigh")
            );
            if digits == 4_301 {
                assert_eq!(actual.to_string(), format!("{message}: {data_text}"));
            }

            let message = format!(
                "Custom error: {capture}; Custom error: 3; Extrinsic dropped; transaction is outdated"
            );
            let actual = PoolRejection::from_rpc(1014, &message, None);
            let expected = if digits == 4_300 {
                PoolRejection::Custom(8)
            } else {
                PoolRejection::InvalidCustomCode(message.clone())
            };
            assert_eq!(actual, expected);
            assert_eq!(
                actual.name(),
                (digits == 4_300).then_some("SlippageTooHigh")
            );
            if digits == 4_301 {
                assert_eq!(actual.to_string(), message);
            }
        }
    }
}

#[test]
fn authoritative_structured_codes_do_not_parse_a_later_oversized_message_capture() {
    let message = format!("Custom error: {}; Extrinsic dropped", "9".repeat(4_301));
    assert_eq!(
        PoolRejection::from_rpc(1014, &message, Some(&json!({"Custom":8}))),
        PoolRejection::Custom(8)
    );
    let unmapped = json!({"Custom":256});
    assert_eq!(
        PoolRejection::from_rpc(1014, &message, Some(&unmapped)),
        PoolRejection::Watch(format!("{message}: {{'Custom': 256}}"))
    );
    // A caught structured conversion failure still reaches the message regex.
    assert_eq!(
        PoolRejection::from_rpc(1014, &message, Some(&json!({"Custom":"9".repeat(4_301)}))),
        PoolRejection::InvalidCustomCode(format!(
            "{message}: {{'Custom': '{}'}}",
            "9".repeat(4_301)
        ))
    );
}

#[tokio::test]
async fn invalid_custom_code_normalization_preserves_author_ack_fate_and_known_codes()
-> Result<(), Box<dyn Error>> {
    use blake2::{Blake2b, Digest, digest::consts::U32};
    use rtao_client::{
        Client, ClientConfig, ClientError, Network, SubmitFailure, SubmitOptions, Wait,
    };
    use rtao_primitives::{CryptoType, H256};
    use rtao_signer::{KeypairSigner, Signer};
    use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};

    let capture = "9".repeat(4_301);
    for (code, definite_refusal) in [(-32603, false), (1010, true), (1014, true)] {
        for in_data in [true, false] {
            let (message, data) = if in_data {
                (
                    "Custom error: 8; Extrinsic dropped; transaction is outdated".to_owned(),
                    Some(json!(format!("Custom error: {capture}"))),
                )
            } else {
                (
                    format!("Custom error: {capture}; Extrinsic dropped; transaction is outdated"),
                    None,
                )
            };
            let diagnostic = match &data {
                Some(Value::String(text)) => format!("{message}: {text}"),
                _ => message.clone(),
            };
            let fake = FakeChain::from_state(FakeState::try_default()?);
            let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
            let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519)?;
            let call = rtao_runtime::calls::system::remark(vec![])
                .compose(&client.refresh_runtime().await?)?;
            let response_message = message.clone();
            let response_data = data.clone();
            fake.mutate_state(|state| {
                state.set_handler("author_submitExtrinsic", move |_, _| {
                    Reply::error(code, response_message.clone(), response_data.clone())
                });
            });
            let failure = client
                .submit(
                    call,
                    &signer,
                    &SubmitOptions {
                        wait: Wait::Pool,
                        ..Default::default()
                    },
                )
                .await
                .err()
                .ok_or("author error was accepted")?;
            let requests = fake.requests();
            let submissions = requests
                .iter()
                .filter(|request| request.method == "author_submitExtrinsic")
                .collect::<Vec<_>>();
            assert_eq!(submissions.len(), 1);
            let encoded = submissions[0].params[0]
                .as_str()
                .and_then(|text| text.strip_prefix("0x"))
                .ok_or("submission bytes absent")?;
            let hash = H256(Blake2b::<U32>::digest(hex::decode(encoded)?).into());
            assert_eq!(
                failure.unknown_fate_hash(),
                (!definite_refusal).then_some(hash)
            );
            if definite_refusal {
                assert!(matches!(
                    &failure,
                    SubmitFailure::Pool(PoolRejection::InvalidCustomCode(text))
                        if text == &diagnostic
                ));
            } else {
                assert!(matches!(
                    &failure,
                    SubmitFailure::Unresolved { cause, reconciliation: None, .. }
                        if matches!(cause.as_ref(), SubmitFailure::Client(ClientError::Rpc(
                            rtao_rpc::RpcError::JsonRpc {
                                code: actual_code,
                                message: actual_message,
                                data: actual_data,
                                ..
                            }
                        )) if *actual_code == code && actual_message == &message && actual_data == &data)
                ));
            }
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                None
            );
            client.close().await;
        }
    }
    Ok(())
}
