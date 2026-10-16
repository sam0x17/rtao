// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_prepare_attach.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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

// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_offline_roundtrip_assembles_nested_calls
// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_payload_json_declares_signed_extensions
// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_payload_json_pins_polkadot_js_number_shape
#[tokio::test]
async fn utility_batch_survives_unsigned_json_roundtrip_and_signature_attachment() {
    let (client, fake, signer) = fixture(CryptoType::Sr25519).await;
    client.nonces().pin(&signer.account(), 80);
    let runtime = client.refresh_runtime().await.unwrap();
    let transfer = rtao_runtime::calls::balances::transfer_keep_alive(
        rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![9; 32])),
        rtao_primitives::TaoBalance(1),
    )
    .compose(&runtime)
    .unwrap();
    let batch = rtao_runtime::calls::utility::batch(vec![transfer.clone(), transfer])
        .compose(&runtime)
        .unwrap();
    let call_data = batch.as_bytes().to_vec();
    let unsigned = client
        .prepare_public(
            batch,
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                tip: 3,
                submit: SubmitOptions {
                    mortality: Mortality::Immortal,
                    nonce: NoncePolicy::Explicit(7),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let expected = unsigned.to_json().unwrap();
    let encoded = serde_json::to_vec(&expected).unwrap();
    drop(unsigned);
    let imported = client
        .import_unsigned_json(&encoded, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(imported.to_json().unwrap(), expected);
    assert_eq!(imported.call().as_bytes(), call_data);
    let payload = imported.payload_json();
    assert!(
        payload["signedExtensions"]
            .as_array()
            .unwrap()
            .contains(&json!("CheckMetadataHash"))
    );
    assert_eq!(payload["mode"], 0);
    assert_eq!(payload["metadataHash"], Value::Null);
    assert_eq!(payload["nonce"], "0x00000007");
    assert_eq!(payload["tip"], "0x00000000000000000000000000000003");
    assert_eq!(
        payload["specVersion"],
        format!("0x{}", hex::encode(runtime.spec_version().to_be_bytes()))
    );
    assert_eq!(
        payload["transactionVersion"],
        format!(
            "0x{}",
            hex::encode(runtime.transaction_version().to_be_bytes())
        )
    );
    assert_eq!(payload["era"], "0x00");
    assert_eq!(payload["method"], format!("0x{}", hex::encode(&call_data)));
    assert_eq!(payload["version"], runtime.core().extrinsic.version);
    let signature = signer
        .sign(&imported.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (signed, _) = client.attach_signature(&imported, signature).await.unwrap();
    assert!(signed.ends_with(&call_data));
    let decoded =
        rtao_core::DynValue::new(runtime.core().decode_extrinsic(&signed, true).unwrap()).to_json();
    assert_eq!(decoded["call"]["call_module"], "Utility");
    assert_eq!(decoded["call"]["call_function"], "batch");
    let nested = decoded["call"]["call_args"]
        .as_array()
        .unwrap()
        .iter()
        .find(|arg| arg["name"] == "calls")
        .unwrap()["value"]
        .as_array()
        .unwrap();
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0], nested[1]);
    assert_eq!(nested[0]["call_module"], "Balances");
    assert_eq!(nested[0]["call_function"], "transfer_keep_alive");
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(80)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    client.close().await;
}

// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_signature_normalization_forms_are_equivalent
#[tokio::test]
async fn valid_raw_tagged_and_hex_signatures_attach_identical_extrinsics() {
    let (client, fake, signer) = fixture(CryptoType::Sr25519).await;
    client.nonces().pin(&signer.account(), 80);
    let runtime = client.refresh_runtime().await.unwrap();
    let call = rtao_runtime::calls::balances::transfer_keep_alive(
        rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![9; 32])),
        rtao_primitives::TaoBalance(1),
    )
    .compose(&runtime)
    .unwrap();
    let call_data = call.as_bytes().to_vec();
    let unsigned = client
        .prepare_public(
            call,
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                tip: 3,
                submit: SubmitOptions {
                    mortality: Mortality::Immortal,
                    nonce: NoncePolicy::Explicit(7),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let encoded = serde_json::to_vec(&unsigned.to_json().unwrap()).unwrap();
    drop(unsigned);
    let imported = client
        .import_unsigned_json(&encoded, &SubmitOptions::default())
        .await
        .unwrap();
    // Sr25519 signatures need not repeat across signing calls. Normalize the
    // same valid signature rather than signing separately for each form.
    let signature = signer
        .sign(&imported.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let raw = MultiSignature {
        crypto: signer.crypto_type(),
        bytes: signature.bytes,
    };
    let tagged =
        MultiSignature::from_bytes_for(&signature.to_bytes(), signer.crypto_type()).unwrap();
    // Client attachment accepts a typed signature. Hex is caller-side wire
    // text here; this does not exercise the extension response protocol.
    let text = format!("0x{}", hex::encode(signature.bytes));
    let from_hex = MultiSignature {
        crypto: signer.crypto_type(),
        bytes: hex::decode(text.strip_prefix("0x").unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    };
    let expected = client.attach_signature(&imported, signature).await.unwrap();
    for (name, normalized) in [("raw64", raw), ("tagged65", tagged), ("0x-hex", from_hex)] {
        assert_eq!(normalized, signature, "{name}");
        let (signed, hash) = client
            .attach_signature(&imported, normalized)
            .await
            .unwrap();
        assert_eq!(signed, expected.0, "{name}");
        assert_eq!(hash, expected.1, "{name}");
        assert!(signed.ends_with(&call_data), "{name}");
    }
    let decoded =
        rtao_core::DynValue::new(runtime.core().decode_extrinsic(&expected.0, true).unwrap())
            .to_json();
    assert_eq!(decoded["call"]["call_module"], "Balances");
    assert_eq!(decoded["call"]["call_function"], "transfer_keep_alive");
    assert_eq!(decoded["nonce"], 7);
    assert_eq!(decoded["tip"], 3);
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(80)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
}

// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_offline_roundtrip_assembles_nested_calls
#[tokio::test]
async fn transfer_proxy_and_sudo_survive_unsigned_json_roundtrip_and_attachment() {
    let (client, fake, signer) = fixture(CryptoType::Sr25519).await;
    client.nonces().pin(&signer.account(), 80);
    let runtime = client.refresh_runtime().await.unwrap();
    let transfer = rtao_runtime::calls::balances::transfer_keep_alive(
        rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(vec![9; 32])),
        rtao_primitives::TaoBalance(1),
    )
    .compose(&runtime)
    .unwrap();
    let proxy = rtao_runtime::calls::proxy::proxy(
        rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(signer.account().0.to_vec())),
        Some(rtao_runtime::models::ProxyType::Transfer),
        transfer.clone(),
    )
    .compose(&runtime)
    .unwrap();
    let sudo = rtao_runtime::calls::sudo::sudo(transfer.clone())
        .compose(&runtime)
        .unwrap();
    for (module, function, call) in [
        ("Balances", "transfer_keep_alive", transfer),
        ("Proxy", "proxy", proxy),
        ("Sudo", "sudo", sudo),
    ] {
        let call_data = call.as_bytes().to_vec();
        let unsigned = client
            .prepare_public(
                call,
                signer.account(),
                &PublicPrepareOptions {
                    crypto_type: signer.crypto_type(),
                    tip: 3,
                    submit: SubmitOptions {
                        mortality: Mortality::Immortal,
                        nonce: NoncePolicy::Explicit(7),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let expected = unsigned.to_json().unwrap();
        let encoded = serde_json::to_vec(&expected).unwrap();
        drop(unsigned);
        let imported = client
            .import_unsigned_json(&encoded, &SubmitOptions::default())
            .await
            .unwrap();
        assert_eq!(imported.to_json().unwrap(), expected, "{module}.{function}");
        assert_eq!(imported.call().as_bytes(), call_data, "{module}.{function}");
        assert_eq!(imported.address(), signer.account());
        let signature = signer
            .sign(&imported.signing_request(SignPurpose::Extrinsic))
            .await
            .unwrap();
        let (signed, _) = client.attach_signature(&imported, signature).await.unwrap();
        assert!(signed.ends_with(&call_data), "{module}.{function}");
        let decoded =
            rtao_core::DynValue::new(runtime.core().decode_extrinsic(&signed, true).unwrap())
                .to_json();
        let outer = &decoded["call"];
        assert_eq!(outer["call_module"], module);
        assert_eq!(outer["call_function"], function);
        let transfer = if module == "Balances" {
            outer
        } else {
            &outer["call_args"]
                .as_array()
                .unwrap()
                .iter()
                .find(|arg| arg["name"] == "call")
                .unwrap()["value"]
        };
        assert_eq!(transfer["call_module"], "Balances");
        assert_eq!(transfer["call_function"], "transfer_keep_alive");
        let args = transfer["call_args"].as_array().unwrap();
        assert_eq!(
            args.iter().find(|arg| arg["name"] == "value").unwrap()["value"],
            1
        );
        assert_eq!(
            args.iter().find(|arg| arg["name"] == "dest").unwrap()["value"],
            AccountId([9; 32]).to_ss58(runtime.core().ss58_format)
        );
        if module == "Proxy" {
            let args = outer["call_args"].as_array().unwrap();
            assert_eq!(
                args.iter().find(|arg| arg["name"] == "real").unwrap()["value"],
                signer.account().to_ss58(runtime.core().ss58_format)
            );
            assert_eq!(
                args.iter()
                    .find(|arg| arg["name"] == "force_proxy_type")
                    .unwrap()["value"],
                "Transfer"
            );
        }
        assert_eq!(
            client.nonces().cached(&signer.account().to_string()).await,
            Some(80)
        );
        assert!(fake.state().lock().unwrap().submissions.is_empty());
    }
    client.close().await;
}

#[tokio::test]
async fn fuzz_unsigned_seed_reconstructs_signs_and_attaches_under_authenticated_context() {
    let (client, fake, _) = fixture(CryptoType::Sr25519).await;
    let signer = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap();
    client.nonces().pin(&signer.account(), 80);
    // This is the same hand-authored public candidate used by the standalone
    // target. Its acceptance is established only when this fixture is executed.
    let seed = include_bytes!("../../../fuzz/seeds/unsigned_extrinsic_json/immortal-remark");
    let expected: Value = rtao_primitives::json::from_slice(seed).unwrap();
    let unsigned = client
        .import_unsigned_json(seed, &SubmitOptions::default())
        .await
        .unwrap();
    assert_eq!(unsigned.to_json().unwrap(), expected);
    assert_eq!(unsigned.address(), signer.account());
    assert_eq!(unsigned.genesis_hash(), client.genesis());
    assert_eq!(unsigned.call().as_bytes(), &[0, 0, 0]);
    assert_eq!(unsigned.payload().len(), 80);
    assert_eq!(unsigned.expiry(), None);
    let signature = signer
        .sign(&unsigned.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    unsigned.verify_signature(signature).unwrap();
    let (signed, _) = client.attach_signature(&unsigned, signature).await.unwrap();
    assert!(signed.ends_with(&[0, 0, 0]));
    let runtime = client.refresh_runtime().await.unwrap();
    let decoded =
        rtao_core::DynValue::new(runtime.core().decode_extrinsic(&signed, true).unwrap()).to_json();
    assert_eq!(decoded["call"]["call_module"], "System");
    assert_eq!(decoded["call"]["call_function"], "remark");
    assert_eq!(decoded["nonce"], 0);
    assert_eq!(decoded["tip"], 0);
    assert_eq!(decoded["mode"]["mode"], "Disabled");
    for (field, value) in [
        ("payload", json!("0x00")),
        ("genesis_hash", json!(format!("0x{}", "00".repeat(32)))),
    ] {
        let mut changed = expected.clone();
        changed[field] = value;
        assert!(import(&client, &changed).await.is_err(), "{field}");
    }
    assert_eq!(
        client.nonces().cached(&signer.account().to_string()).await,
        Some(80)
    );
    assert!(fake.state().lock().unwrap().submissions.is_empty());
    assert!(
        !fake
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
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

// pyt: sdk/python/tests/unit/test_prepare_attach.py::test_metadata_hash_mode_survives_roundtrip
// Native digests are authenticated; cross-payload signature reuse stays refused.
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
    let submit = SubmitOptions {
        nonce: NoncePolicy::Explicit(7),
        ..Default::default()
    };
    let unsigned = client
        .prepare_public(
            call.clone(),
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                metadata_hash: Some(digest),
                tip: u64::MAX,
                submit: submit.clone(),
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
    let imported = import(&client, &value).await.unwrap();
    assert_eq!(imported.to_json().unwrap(), value);
    assert_eq!(imported.metadata_hash(), Some(digest));
    assert_eq!(imported.payload_json()["mode"], 1);
    assert_eq!(
        imported.payload_json()["metadataHash"],
        format!("0x{}", hex::encode(digest))
    );
    let signature = signer
        .sign(&imported.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (enabled_bytes, _) = client.attach_signature(&imported, signature).await.unwrap();
    let enabled = rtao_core::DynValue::new(
        runtime
            .core()
            .decode_extrinsic(&enabled_bytes, true)
            .unwrap(),
    )
    .to_json();
    assert_eq!(enabled["mode"]["mode"], "Enabled");
    assert_eq!(enabled["call"]["call_module"], "System");
    assert_eq!(enabled["call"]["call_function"], "remark");
    assert_eq!(enabled["nonce"], 7);
    assert_eq!(enabled["tip"], u64::MAX);
    assert!(enabled_bytes.ends_with(imported.call().as_bytes()));

    let disabled = client
        .prepare_public(
            call.clone(),
            signer.account(),
            &PublicPrepareOptions {
                crypto_type: signer.crypto_type(),
                metadata_hash: None,
                tip: u64::MAX,
                submit,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(disabled.metadata_hash(), None);
    assert_eq!(disabled.payload_json()["mode"], 0);
    assert_eq!(disabled.payload_json()["metadataHash"], Value::Null);
    assert_eq!(disabled.call().as_bytes(), imported.call().as_bytes());
    assert_eq!(disabled.nonce(), imported.nonce());
    assert_eq!(disabled.tip(), imported.tip());
    assert_eq!(disabled.to_json().unwrap()["era"], value["era"]);
    assert_eq!(disabled.era_block_hash(), imported.era_block_hash());
    let disabled_signature = signer
        .sign(&disabled.signing_request(SignPurpose::Extrinsic))
        .await
        .unwrap();
    let (disabled_bytes, _) = client
        .attach_signature(&disabled, disabled_signature)
        .await
        .unwrap();
    let disabled_decoded = rtao_core::DynValue::new(
        runtime
            .core()
            .decode_extrinsic(&disabled_bytes, true)
            .unwrap(),
    )
    .to_json();
    assert_eq!(disabled_decoded["mode"]["mode"], "Disabled");
    for field in ["call", "nonce", "tip", "era"] {
        assert_ne!(enabled[field], Value::Null, "{field}");
        assert_eq!(enabled[field], disabled_decoded[field], "{field}");
    }
    // Compare unsigned extras: each signed extrinsic has its own valid
    // signature, while only the transmitted metadata-mode byte should differ.
    let enabled_extra = imported.included_in_extrinsic();
    let disabled_extra = disabled.included_in_extrinsic();
    assert_eq!(enabled_extra.len(), disabled_extra.len());
    let differences = enabled_extra
        .iter()
        .zip(disabled_extra)
        .filter(|(enabled, disabled)| enabled != disabled)
        .map(|(enabled, disabled)| (*enabled, *disabled))
        .collect::<Vec<_>>();
    assert_eq!(differences, vec![(1, 0)]);
    assert!(client.attach_signature(&disabled, signature).await.is_err());
    assert!(
        client
            .attach_signature(&imported, disabled_signature)
            .await
            .is_err()
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
    let expiry = imported.expiry().unwrap();
    fake.mutate_state(|state| {
        state.advance(expiry.saturating_sub(state.block));
        state.finalized = expiry;
    });
    assert!(import(&client, &value).await.is_err());
    client.nonces().pin(&signer.account(), 80);
    assert!(
        client
            .submit_signature(
                &imported,
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
