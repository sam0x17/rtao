//! Public fixture signing and adversarial unsigned-JSON imports.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rtao_client::{
    Client, ClientConfig, Mortality, Network, NoncePolicy, PublicPrepareOptions, SubmitOptions,
    UntrustedUnsignedExtrinsic, Wait,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::{KeypairSigner, MultiSignature, SignPurpose, Signer};
use rtao_testkit::{FakeChain, FakeState};
use serde_json::{Value, json};
use subxt_rpcs::RpcClient;

async fn fixture(crypto: CryptoType) -> (Client, FakeChain, KeypairSigner) {
    let fake = FakeChain::from_state(FakeState::default());
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    cfg.track_heads = false;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    let client = Client::from_rpc(RpcClient::new(fake.clone()), cfg)
        .await
        .unwrap();
    // Public deterministic fixture seed; no wallet material is accessed.
    let signer = KeypairSigner::from_seed(&[7; 32], crypto).unwrap();
    (client, fake, signer)
}
async fn prepared(
    client: &Client,
    signer: &KeypairSigner,
    large: bool,
) -> rtao_client::UnsignedExtrinsic {
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![1; if large { 300 } else { 3 }])
        .compose(&runtime)
        .unwrap();
    client
        .prepare_public(
            call,
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                tip: 17,
                tip_asset_id: Some(65),
                submit: SubmitOptions {
                    mortality: Mortality::Immortal,
                    nonce: NoncePolicy::Explicit(3),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap()
}
async fn import(
    client: &Client,
    value: &Value,
) -> Result<rtao_client::UnsignedExtrinsic, rtao_client::ClientError> {
    client
        .import_unsigned_json(
            &serde_json::to_vec(value).unwrap(),
            &SubmitOptions::default(),
        )
        .await
}

#[tokio::test]
async fn python_schema_and_big_endian_views_roundtrip_both_schemes_and_payload_sizes() {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        for large in [false, true] {
            let (client, fake, signer) = fixture(crypto).await;
            client.nonces().pin(&signer.account(), 80);
            let prepared = prepared(&client, &signer, large).await;
            let value = prepared.to_json().unwrap();
            let expected = [
                "call_data",
                "address",
                "public_key",
                "crypto_type",
                "era",
                "nonce",
                "tip",
                "tip_asset_id",
                "genesis_hash",
                "era_block_hash",
                "spec_version",
                "transaction_version",
                "metadata_hash",
                "payload",
                "payload_json",
                "included_in_extrinsic",
                "included_in_signed_data",
            ];
            assert_eq!(
                value
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(value["era"], "00");
            assert_eq!(value["payload_json"]["nonce"], "0x00000003");
            assert_eq!(value["payload_json"]["specVersion"], "0x000001de");
            assert_eq!(value["payload_json"]["transactionVersion"], "0x00000001");
            assert_eq!(
                value["payload_json"]["tip"],
                "0x00000000000000000000000000000011"
            );
            assert_eq!(value["payload_json"]["assetId"], "0x0501");
            if large {
                assert_eq!(prepared.payload().len(), 32);
            }
            let imported = import(&client, &value).await.unwrap();
            assert_eq!(imported.to_json().unwrap(), value);
            assert_eq!(imported.tip(), 17);
            assert_eq!(imported.tip_asset_id(), Some(65));
            let signature = signer
                .sign(&imported.signing_request(SignPurpose::Extrinsic))
                .await
                .unwrap();
            imported.verify_signature(signature).unwrap();
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                Some(80)
            );
            let report = client
                .submit_signature(
                    &imported,
                    signature,
                    &SubmitOptions {
                        wait: Wait::Pool,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            assert!(report.success);
            let state = fake.state();
            let state = state.lock().unwrap();
            assert_eq!(state.submissions.len(), 1);
            let decoded = imported.context();
            assert_eq!(decoded.transaction_version, 1);
        }
    }
}

#[tokio::test]
async fn every_required_field_missing_unknown_duplicate_and_numeric_overflow_refuse() {
    let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
    let value = prepared(&client, &signer, false).await.to_json().unwrap();
    for key in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(import(&client, &missing).await.is_err(), "missing {key}");
    }
    for key in ["policy", "context", "allow_raw_calls"] {
        let mut extra = value.clone();
        extra[key] = json!(true);
        assert!(import(&client, &extra).await.is_err());
    }
    let encoded = serde_json::to_string(&value).unwrap();
    let positional = value
        .as_object()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        UntrustedUnsignedExtrinsic::from_json(&serde_json::to_vec(&positional).unwrap()).is_err()
    );
    let duplicate = encoded.replacen("{", "{\"nonce\":3,", 1);
    assert!(UntrustedUnsignedExtrinsic::from_json(duplicate.as_bytes()).is_err());
    for (needle, replacement) in [
        (
            "\"payload_json\":{",
            "\"payload_json\":{\"method\":\"0xattacker\",",
        ),
        (
            "\"payload_json\":{",
            "\"payload_json\":{\"signedExtensions\":[\"attacker\"],",
        ),
    ] {
        let duplicate = encoded.replacen(needle, replacement, 1);
        assert_ne!(duplicate, encoded);
        assert!(UntrustedUnsignedExtrinsic::from_json(duplicate.as_bytes()).is_err());
    }
    let mortal = client
        .prepare_public(
            prepared(&client, &signer, false).await.call().clone(),
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mortal = serde_json::to_string(&mortal).unwrap();
    let duplicate = mortal.replacen("\"era\":{", "\"era\":{\"period\":1,", 1);
    assert_ne!(duplicate, mortal);
    assert!(UntrustedUnsignedExtrinsic::from_json(duplicate.as_bytes()).is_err());
    for key in ["nonce", "tip", "tip_asset_id"] {
        for number in ["-1", "18446744073709551616", "3.0", "\"3\""] {
            let mut overflow = value.clone();
            overflow[key] = serde_json::from_str(number).unwrap();
            assert!(import(&client, &overflow).await.is_err(), "{key}: {number}");
        }
    }
    let mut wide_nonce = value.clone();
    wide_nonce["nonce"] = json!(u64::from(u32::MAX) + 1);
    assert!(import(&client, &wide_nonce).await.is_err());
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn altered_call_identity_era_payload_views_parts_and_hashes_refuse() {
    let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
    let value = prepared(&client, &signer, false).await.to_json().unwrap();
    for key in [
        "call_data",
        "public_key",
        "genesis_hash",
        "era_block_hash",
        "payload",
        "included_in_extrinsic",
        "included_in_signed_data",
    ] {
        let mut altered = value.clone();
        let original = altered[key].as_str().unwrap();
        let mut bytes = hex::decode(&original[2..]).unwrap();
        bytes[0] ^= 1;
        altered[key] = json!(format!("0x{}", hex::encode(bytes)));
        assert!(import(&client, &altered).await.is_err(), "{key}");
    }
    for (key, field) in [
        ("nonce", json!(4)),
        ("tip", json!(18)),
        ("tip_asset_id", json!(66)),
        ("crypto_type", json!(2)),
        ("spec_version", json!(479)),
        ("transaction_version", json!(2)),
        ("era", json!({"period":128,"current":0})),
        ("address", json!(AccountId([9; 32]).to_ss58(42))),
        ("metadata_hash", json!(format!("0x{}", "00".repeat(32)))),
    ] {
        let mut altered = value.clone();
        altered[key] = field;
        assert!(import(&client, &altered).await.is_err(), "{key}");
    }
    for key in value["payload_json"].as_object().unwrap().keys() {
        let mut altered = value.clone();
        altered["payload_json"][key] = json!("altered");
        assert!(import(&client, &altered).await.is_err(), "view {key}");
    }
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn serde_private_carriers_cannot_inject_unscanned_unsigned_json() {
    let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
    let unsigned = prepared(&client, &signer, false).await;
    let value = unsigned.to_json().unwrap();
    let encoded = serde_json::to_string(&value).unwrap();
    let duplicate = encoded.replacen('{', "{\"nonce\":3,", 1);
    for payload in [encoded, duplicate] {
        let wrapped = format!(
            "{{\"$serde_json::private::RawValue\":{}}}",
            serde_json::to_string(&payload).unwrap()
        );
        let error = UntrustedUnsignedExtrinsic::from_json(wrapped.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("private JSON carrier"));
        assert!(
            client
                .import_unsigned_json(wrapped.as_bytes(), &SubmitOptions::default())
                .await
                .is_err()
        );
    }
    for field in ["payload_json", "era", "nonce"] {
        for carrier in [
            "$serde_json::private::RawValue",
            "$serde_json::private::Number",
        ] {
            let mut altered = value.clone();
            let mut wrapper = serde_json::Map::new();
            wrapper.insert(
                carrier.into(),
                json!(serde_json::to_string(&value[field]).unwrap()),
            );
            altered[field] = Value::Object(wrapper);
            let bytes = serde_json::to_vec(&altered).unwrap();
            let error = UntrustedUnsignedExtrinsic::from_json(&bytes).unwrap_err();
            assert!(error.to_string().contains("private JSON carrier"));
        }
    }
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

#[tokio::test]
async fn bad_external_signature_refuses_before_nonce_mutation_and_author_rpc() {
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let (client, fake, signer) = fixture(crypto).await;
        let unsigned = prepared(&client, &signer, true).await;
        let valid = signer
            .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
            .await
            .unwrap();
        let wrong_signer = KeypairSigner::from_seed(&[8; 32], crypto).unwrap();
        let wrong = wrong_signer
            .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
            .await
            .unwrap();
        let mut altered = valid;
        altered.bytes[0] ^= 1;
        let variant = MultiSignature {
            crypto: if crypto == CryptoType::Ed25519 {
                CryptoType::Sr25519
            } else {
                CryptoType::Ed25519
            },
            bytes: valid.bytes,
        };
        client.nonces().pin(&signer.account(), 80);
        for signature in [wrong, altered, variant] {
            assert!(client.attach_signature(&unsigned, signature).await.is_err());
            assert!(
                client
                    .submit_signature(
                        &unsigned,
                        signature,
                        &SubmitOptions {
                            wait: Wait::Pool,
                            ..Default::default()
                        }
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                client.nonces().cached(&signer.account().to_string()).await,
                Some(80)
            );
        }
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn live_spec_and_transaction_upgrade_refuse_import_and_external_submit() {
    for (spec, tx) in [(479, 1), (478, 2)] {
        let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
        let unsigned = prepared(&client, &signer, false).await;
        let signature = signer
            .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
            .await
            .unwrap();
        fake.mutate_state(|s| s.set_result("state_getRuntimeVersion", json!({"specName":"node-subtensor","specVersion":spec,"transactionVersion":tx,"apis":[]})));
        assert!(import(&client, &unsigned.to_json().unwrap()).await.is_err());
        client.nonces().pin(&signer.account(), 80);
        assert!(
            client
                .submit_signature(
                    &unsigned,
                    signature,
                    &SubmitOptions {
                        wait: Wait::Pool,
                        ..Default::default()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(80)
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
}

#[tokio::test]
async fn metadata_digest_mortal_anchor_expiry_and_native_fee_boundaries() {
    let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![])
        .compose(&runtime)
        .unwrap();
    let first = client
        .prepare_public(
            call.clone(),
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let digest = runtime
        .metadata_digest(&first.context().chain_info)
        .unwrap();
    let unsigned = client
        .prepare_public(
            call.clone(),
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                metadata_hash: Some(digest),
                tip: u64::MAX,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(unsigned.payload_json()["mode"], 1);
    assert_eq!(
        unsigned.payload_json()["tip"],
        "0x0000000000000000ffffffffffffffff"
    );
    let value = unsigned.to_json().unwrap();
    assert_eq!(
        import(&client, &value).await.unwrap().to_json().unwrap(),
        value
    );
    assert!(
        client
            .prepare_public(
                call,
                signer.account(),
                &PublicPrepareOptions {
                    metadata_hash: Some([0; 32]),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    fake.mutate_state(|s| s.finalized = unsigned.expiry().unwrap());
    assert!(import(&client, &value).await.is_err());
    client.nonces().pin(&signer.account(), 80);
    assert!(
        client
            .submit_signature(
                &unsigned,
                signature,
                &SubmitOptions {
                    wait: Wait::Pool,
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(80)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn foreign_client_genesis_refuses_prepared_file_and_signature() {
    let (client, _, signer) = fixture(CryptoType::Ed25519).await;
    let unsigned = prepared(&client, &signer, false).await;
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let mut state = FakeState::default();
    state.genesis_hash = format!("0x{}", "09".repeat(32));
    let other = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let foreign = Client::from_rpc(RpcClient::new(other.clone()), config)
        .await
        .unwrap();
    assert!(
        import(&foreign, &unsigned.to_json().unwrap())
            .await
            .is_err()
    );
    foreign.nonces().pin(&signer.account(), 80);
    assert!(
        foreign
            .submit_signature(
                &unsigned,
                signature,
                &SubmitOptions {
                    wait: Wait::Pool,
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        foreign.nonces().cached(&signer.account().to_string()).await,
        Some(80)
    );
    assert!(other.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn import_deadline_precedes_parsing_and_fresh_public_preparation_keeps_cache() {
    let (client, fake, signer) = fixture(CryptoType::Ed25519).await;
    assert!(matches!(
        client
            .import_unsigned_json(
                b"invalid JSON",
                &SubmitOptions {
                    deadline: Some(std::time::Duration::ZERO),
                    ..Default::default()
                }
            )
            .await,
        Err(rtao_client::ClientError::Rpc(
            rtao_rpc::RpcError::Timeout { .. }
        ))
    ));
    client.nonces().pin(&signer.account(), 90);
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![])
        .compose(&runtime)
        .unwrap();
    let unsigned = client
        .prepare_public(
            call,
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(unsigned.nonce(), 0);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(90)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
}

#[tokio::test]
async fn python_requested_era_periods_and_phase_roundtrip_at_normalized_core_width() {
    let (client, _, signer) = fixture(CryptoType::Ed25519).await;
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::system::remark(vec![])
        .compose(&runtime)
        .unwrap();
    for requested in [0, 7, 129, 65537] {
        let native = client
            .prepare_public(
                call.clone(),
                signer.account(),
                &PublicPrepareOptions {
                    crypto_type: signer.crypto_type(),
                    submit: SubmitOptions {
                        mortality: Mortality::Mortal(rtao_client::Period::new(requested)),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut value = native.to_json().unwrap();
        value["era"]["period"] = json!(requested);
        let imported = import(&client, &value).await.unwrap();
        assert_eq!(imported.to_json().unwrap(), value);
        assert_eq!(native.payload(), imported.payload());
        assert_eq!(native.expiry(), imported.expiry());
        let current = value["era"]["current"].as_u64().unwrap();
        let normalized = rtao_client::Period::new(requested).get();
        let quantize = (normalized >> 12).max(1);
        value["era"]["phase"] = json!((current % normalized) / quantize * quantize);
        assert_eq!(
            import(&client, &value).await.unwrap().to_json().unwrap(),
            value
        );
        value["era"]["phase"] = json!(normalized);
        assert!(import(&client, &value).await.is_err());
        value["era"]["period"] = json!(u64::MAX);
        assert!(import(&client, &value).await.is_err());
    }
}

#[tokio::test]
async fn literal_hex_byte_spans_and_some_u256_calls_import_before_fixture_signing() {
    let (client, _, signer) = fixture(CryptoType::Ed25519).await;
    let runtime = client.refresh_runtime().await.unwrap();
    let mut calls = Vec::new();
    for bytes in [
        b"0x41".to_vec(),
        b"0x".to_vec(),
        b"0x1".to_vec(),
        b"0x0xff".to_vec(),
        vec![0xff, 0x80],
    ] {
        let inner = rtao_runtime::calls::system::remark(bytes)
            .compose(&runtime)
            .unwrap();
        calls.push(inner.clone());
        calls.push(
            rtao_runtime::calls::sudo::sudo(inner)
                .compose(&runtime)
                .unwrap(),
        );
    }
    let limbs = || {
        rtao_core::DynValue::new(rtao_core::codec::Value::List(vec![
            rtao_core::codec::Value::Uint(u64::MAX.into()),
            rtao_core::codec::Value::Uint(7),
            rtao_core::codec::Value::Uint(9),
            rtao_core::codec::Value::Uint(11),
        ]))
    };
    calls.push(
        rtao_runtime::calls::evm::call(
            rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![7; 20])),
            rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![8; 20])),
            b"0x41".to_vec(),
            limbs(),
            21000,
            limbs(),
            Some(limbs()),
            Some(limbs()),
            Vec::new(),
            Vec::new(),
        )
        .compose(&runtime)
        .unwrap(),
    );
    for call in calls {
        let unsigned = client
            .prepare_public(
                call.clone(),
                signer.account(),
                &PublicPrepareOptions {
                    crypto_type: signer.crypto_type(),
                    submit: SubmitOptions {
                        mortality: Mortality::Immortal,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let value = unsigned.to_json().unwrap();
        let imported = import(&client, &value).await.unwrap();
        assert_eq!(imported.call(), &call);
        assert_eq!(imported.to_json().unwrap(), value);
        let signature = signer
            .sign(&imported.signing_request(SignPurpose::Extrinsic))
            .await
            .unwrap();
        imported.verify_signature(signature).unwrap();
    }
}
