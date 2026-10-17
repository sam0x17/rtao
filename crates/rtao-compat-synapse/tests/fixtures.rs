//! Offline checks against vectors captured from bittensor==10.5.0
//! (`ci/interop/synapse/capture_fixtures.py`).

#![cfg(all(feature = "axon", feature = "dendrite", feature = "tensor"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "../examples/interop/synapses.rs"]
mod synapses;

use std::sync::Arc;

use num_bigint::BigInt;
use rtao_btauth::{Clock, ClockError};
use rtao_compat_synapse::{
    BaseSynapse, CallOptions, Dendrite, Envelope, PyDict, PyValue, Synapse, get_hash, py_str,
    tensor::Tensor, to_python_json,
};
use rtao_primitives::{AccountId, CryptoType, Uid, axon::AxonTarget};
use rtao_signer::KeypairSigner;
use serde_json::Value;
use synapses::*;

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/bittensor-10.5.0"
);
const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

fn load(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/{name}")).unwrap()).unwrap()
}

struct Fixed(u64);
impl Clock for Fixed {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(self.0)
    }
}

fn tensor() -> Tensor {
    let data: Vec<u8> = [1.5f32, 2.0, 3.0, 4.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    Tensor::from_raw("float32", &[2, 2], &data).unwrap()
}

/// Check one captured Python request against the Rust dendrite's rendering.
async fn compare<S: Synapse>(dendrite: &Dendrite, target: &AxonTarget, body: S, fixture: &Value) {
    let name = fixture["name"].as_str().unwrap();
    assert_eq!(name, S::NAME);
    let prepared = dendrite
        .prepare(target, body, &CallOptions::default())
        .await
        .unwrap();
    let python: Vec<(String, String)> = fixture["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_owned(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let names =
        |pairs: &[(String, String)]| pairs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(
        names(&prepared.headers),
        names(&python),
        "{name} header order"
    );
    for ((rust_name, rust_value), (_, python_value)) in prepared.headers.iter().zip(&python) {
        if matches!(
            rust_name.as_str(),
            "header_size" | "total_size" | "bt_header_dendrite_signature"
        ) {
            continue;
        }
        assert_eq!(rust_value, python_value, "{name} header {rust_name}");
    }
    assert_eq!(
        prepared.message,
        fixture["message"].as_str().unwrap(),
        "{name} message"
    );

    // Python's signature verifies under the Rust verifier (S2 for every synapse).
    let python_signature = &python
        .iter()
        .find(|(n, _)| n == "bt_header_dendrite_signature")
        .unwrap()
        .1;
    let bob: AccountId = BOB.parse().unwrap();
    let hash = fixture["body_hash"].as_str().unwrap();
    let uuid = prepared.envelope.dendrite.as_ref().unwrap().uuid.clone();
    rtao_btauth::legacy::verify_v10_for(
        ALICE,
        &BigInt::from(1_760_000_000_123_456_789_u64),
        uuid.as_deref(),
        Some(hash),
        python_signature,
        &bob,
    )
    .unwrap();

    // Byte-identical body once the CPython size and signature are aligned.
    let body_text = fixture["body"].as_str().unwrap();
    let mut envelope = prepared.envelope.clone();
    let python_body: Value = serde_json::from_str(body_text).unwrap();
    envelope.total_size = python_body["total_size"].as_i64();
    envelope.dendrite.as_mut().unwrap().signature = Some(python_signature.clone());
    assert_eq!(
        String::from_utf8(to_python_json(&envelope).unwrap()).unwrap(),
        body_text,
        "{name} body"
    );

    // The Python body parses back to the same hash and bytes.
    let parsed = Envelope::<S>::from_json_slice(body_text.as_bytes()).unwrap();
    assert_eq!(parsed.body_hash().unwrap(), hash, "{name} parsed hash");
    assert_eq!(
        String::from_utf8(to_python_json(&parsed).unwrap()).unwrap(),
        body_text
    );
}

#[tokio::test]
async fn python_requests_match_rust_rendering() {
    let alice = Arc::new(KeypairSigner::from_uri("//Alice", CryptoType::Sr25519).unwrap());
    let dendrite = Dendrite::new(alice)
        .unwrap()
        .with_uuid("5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a")
        .with_external_ip("203.0.113.7")
        .with_clock(Arc::new(Fixed(1_760_000_000_123_456_789)));
    let target = AxonTarget {
        uid: Uid(0),
        hotkey: BOB.parse().unwrap(),
        base_url: "http://198.51.100.9:8091".parse().unwrap(),
    };
    let fixtures = load("requests.json");
    let fixtures = fixtures.as_array().unwrap();
    assert_eq!(fixtures.len(), 7);
    compare(&dendrite, &target, BaseSynapse {}, &fixtures[0]).await;
    let mixed = Mixed {
        i: 7,
        f: 1.5,
        s: "it's \"q\" \u{e9} \u{1F600} \u{200b}\u{a0}\\".into(),
        l: vec!["a".into(), "b'c".into()],
        d: PyDict(vec![
            ("x".into(), vec![0.1, 1e-5, 1e16]),
            ("a".into(), vec![2.0]),
        ]),
        n: None,
        out: None,
    };
    compare(&dendrite, &target, mixed, &fixtures[1]).await;
    let req = Req {
        rs: "hello".into(),
        ri: 5,
        rl: vec![1, 2],
        rd: PyDict(vec![("k".into(), 1)]),
        rf: 0.25,
        rb: true,
        echo: None,
    };
    compare(&dendrite, &target, req, &fixtures[2]).await;
    let legacy = LegacyHashed {
        a: 1,
        b: 2,
        d: Some(vec!["foobar".into()]),
        ..LegacyHashed::default()
    };
    compare(&dendrite, &target, legacy, &fixtures[3]).await;
    compare(
        &dendrite,
        &target,
        Tens {
            t: Some(tensor()),
            total: None,
        },
        &fixtures[4],
    )
    .await;
    compare(&dendrite, &target, Tens::default(), &fixtures[5]).await;
    compare(
        &dendrite,
        &target,
        Echo {
            text: "hi".into(),
            completion: String::new(),
        },
        &fixtures[6],
    )
    .await;
}

fn py_value(value: &Value) -> PyValue {
    match value {
        Value::Null => PyValue::None,
        Value::Bool(flag) => PyValue::Bool(*flag),
        Value::String(text) => PyValue::Str(text.clone()),
        Value::Array(items) => PyValue::List(items.iter().map(py_value).collect()),
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("int") {
                PyValue::Int(text.parse().unwrap())
            } else if let Some(Value::String(text)) = map.get("float") {
                PyValue::Float(match text.as_str() {
                    "inf" => f64::INFINITY,
                    "-inf" => f64::NEG_INFINITY,
                    "nan" => f64::NAN,
                    other => other.parse().unwrap(),
                })
            } else if let Some(Value::Array(items)) = map.get("tuple") {
                PyValue::Tuple(items.iter().map(py_value).collect())
            } else if let Some(Value::Array(entries)) = map.get("dict") {
                PyValue::Dict(
                    entries
                        .iter()
                        .map(|pair| (py_value(&pair[0]), py_value(&pair[1])))
                        .collect(),
                )
            } else {
                panic!("unknown encoding {map:?}")
            }
        }
        Value::Number(_) => panic!("numbers are tagged"),
    }
}

#[test]
fn python_str_and_hash_vectors() {
    let vectors = load("hashes.json");
    let mut checked = 0;
    for vector in vectors.as_array().unwrap() {
        if let Some(value) = vector.get("value") {
            let text = py_str(&py_value(value));
            assert_eq!(text, vector["str"].as_str().unwrap(), "{value}");
            assert_eq!(get_hash(&text), vector["hash"].as_str().unwrap());
            checked += 1;
        }
    }
    assert!(checked >= 35, "{checked}");
}

#[test]
fn python_tensor_buffers() {
    for vector in load("tensors.json").as_array().unwrap() {
        let shape: Vec<u64> = vector["array_shape"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.as_u64().unwrap())
            .collect();
        let data = hex::decode(vector["data_hex"].as_str().unwrap()).unwrap();
        let dtype = vector["dtype"].as_str().unwrap();
        let tensor = Tensor::from_raw(dtype, &shape, &data).unwrap();
        assert_eq!(
            tensor.buffer.as_deref(),
            vector["buffer"].as_str(),
            "{dtype}"
        );
        let python_shape: Vec<i64> = vector["shape"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.as_i64().unwrap())
            .collect();
        assert_eq!(tensor.shape, python_shape);
        let raw = tensor.to_raw().unwrap();
        assert_eq!((raw.shape, raw.data), (shape, data));
    }
}

#[test]
fn python_signed_s1_verifies() {
    let provenance = load("provenance.json");
    let s1 = &provenance["s1"];
    let message = s1["message"].as_str().unwrap();
    assert_eq!(
        message,
        "1700000000000000000.5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY.5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty.5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a.a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
    );
    let bob: AccountId = BOB.parse().unwrap();
    rtao_btauth::legacy::verify_v10_for(
        ALICE,
        &BigInt::from(1_700_000_000_000_000_000_u64),
        Some("5ecbd69c-1cec-11ee-b0dc-e29ce36fec1a"),
        Some(rtao_compat_synapse::EMPTY_BODY_HASH),
        s1["python_signature"].as_str().unwrap(),
        &bob,
    )
    .unwrap();
}
