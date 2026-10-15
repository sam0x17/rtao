//! Independent exact-head public API audit; task-local, never submitted.
#![cfg(feature = "ledger")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
use bittensor_core::{
    signers::{ledger::LedgerDevice, ApduTransport},
    CoreError,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<Vec<u8>>>>);
impl ApduTransport for Capture {
    fn exchange(&self, request: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.0.lock().unwrap().push(request.to_vec());
        let mut result = if request[2] == 2 {
            vec![0x55; 65]
        } else {
            Vec::new()
        };
        result.extend_from_slice(&[0x90, 0x00]);
        Ok(result)
    }
}

#[test]
fn public_sign_chunk_boundaries_and_payload_limit() {
    for size in [1, 249, 250, 251, 499, 500, 501, 65535] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let device = LedgerDevice::with_transport(Capture(seen.clone()));
        let payload = vec![0xab; size];
        let proof = [0xcd; 7];
        assert_eq!(device.sign(2, 7, &payload, &proof).unwrap(), vec![0x55; 65]);
        let requests = seen.lock().unwrap();
        assert_eq!(&requests[0][..5], &[0xf9, 2, 0, 0, 22]);
        assert_eq!(&requests[0][25..], &(size as u16).to_le_bytes());
        let mut bytes = Vec::new();
        for (position, request) in requests.iter().enumerate().skip(1) {
            assert_eq!(request[0], 0xf9);
            assert_eq!(request[1], 2);
            assert_eq!(request[3], 0);
            assert_eq!(request[4] as usize, request.len() - 5);
            assert!(request[4] <= 250);
            assert_eq!(
                request[2],
                if position + 1 == requests.len() { 2 } else { 1 }
            );
            if position + 1 != requests.len() {
                assert_eq!(request[4], 250);
            }
            bytes.extend_from_slice(&request[5..]);
        }
        assert_eq!(&bytes[..size], &payload);
        assert_eq!(&bytes[size..], &proof);
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let device = LedgerDevice::with_transport(Capture(seen.clone()));
    assert_eq!(
        device
            .sign(0, 0, &vec![0; 65536], &[])
            .unwrap_err()
            .to_string(),
        "device error: signature payload exceeds the device's 64 KiB limit"
    );
    assert!(seen.lock().unwrap().is_empty());
}

struct Fixed(Vec<u8>);
impl ApduTransport for Fixed {
    fn exchange(&self, _: &[u8]) -> Result<Vec<u8>, CoreError> {
        Ok(self.0.clone())
    }
}
#[test]
fn public_short_responses_status_words_and_invalid_address_are_bounded() {
    for response in [vec![], vec![0x90]] {
        let device = LedgerDevice::with_transport(Fixed(response));
        assert_eq!(
            device.app_version().unwrap_err().to_string(),
            "device error: APDU response was too short"
        );
    }
    for count in 0..7 {
        let mut response = vec![0; count];
        response.extend([0x90, 0]);
        let device = LedgerDevice::with_transport(Fixed(response));
        assert_eq!(
            device.app_version().unwrap_err().to_string(),
            "device error: unexpected GET_VERSION response length"
        );
    }
    let device = LedgerDevice::with_transport(Fixed(vec![0, 0, 100, 0, 0, 0, 29, 0x90, 0]));
    assert_eq!(device.app_version().unwrap(), (100, 0, 29));
    let device = LedgerDevice::with_transport(Fixed(vec![0x69, 0x86]));
    assert_eq!(
        device.sign(0, 0, &[1], &[]).unwrap_err().to_string(),
        "device error: the request was rejected on the device"
    );
    let device = LedgerDevice::with_transport(Fixed(vec![b'x', 0x67, 0]));
    assert_eq!(
        device.app_version().unwrap_err().to_string(),
        "device error: device returned 0x6700 (wrong buffer length): x"
    );
    let mut response = vec![0; 31];
    response.extend([0x90, 0]);
    let device = LedgerDevice::with_transport(Fixed(response));
    assert_eq!(
        device.address(0, 0, 42, false).unwrap_err().to_string(),
        "device error: unexpected GET_ADDR response length"
    );
    let mut response = vec![0; 32];
    response.extend([0xff, 0x90, 0]);
    let device = LedgerDevice::with_transport(Fixed(response));
    assert_eq!(
        device.address(0, 0, 42, false).unwrap_err().to_string(),
        "device error: device returned a non-UTF-8 address"
    );
}

#[test]
fn ledger_quality_generate_ratio_vectors() {
    use bittensor_core::{
        codec::{extrinsic::TxParams, value::Value},
        digest::{generate_extrinsic_proof, metadata_digest, ChainInfo},
        runtime::Runtime,
    };
    let Some(output) = std::env::var_os("LEDGER_QUALITY_VECTOR_PATH") else {
        return;
    };
    let metadata = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../pr-3246/sdk/bittensor-core/tests/fixtures/finney-v478.v15.scale"
    ))
    .unwrap();
    let runtime = Runtime::parse(&metadata, 478, 1, 42).unwrap();
    let info = ChainInfo {
        spec_version: 478,
        spec_name: "node-subtensor".into(),
        base58_prefix: 42,
        decimals: 9,
        token_symbol: "TAO".into(),
    };
    let digest = metadata_digest(&metadata, &info).unwrap();
    let params = TxParams {
        era: Value::str("00"),
        nonce: 7,
        tip: 0,
        tip_asset_id: None,
        genesis_hash: [0x22; 32],
        era_block_hash: [0x22; 32],
        metadata_hash: Some(digest),
    };
    let (extra, additional) = runtime.signature_payload_parts(&params).unwrap();
    let account = Value::hex(&[0x11; 32]);
    let swap = runtime
        .compose_call(
            "SubtensorModule",
            "swap_stake_limit",
            &Value::record(vec![
                ("hotkey".into(), account.clone()),
                ("origin_netuid".into(), Value::Int(1)),
                ("destination_netuid".into(), Value::Int(2)),
                ("alpha_amount".into(), Value::Int(2_000_000_000)),
                ("limit_price".into(), Value::Int(950_000_000)),
                ("allow_partial".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
    let moved = runtime
        .compose_call(
            "SubtensorModule",
            "move_stake_limit",
            &Value::record(vec![
                ("origin_hotkey".into(), account.clone()),
                ("destination_hotkey".into(), account.clone()),
                ("origin_netuid".into(), Value::Int(1)),
                ("destination_netuid".into(), Value::Int(2)),
                ("alpha_amount".into(), Value::Int(2_000_000_000)),
                ("limit_price".into(), Value::Int(950_000_000)),
                ("allow_partial".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
    let batch = runtime
        .compose_call(
            "Utility",
            "batch",
            &Value::record(vec![(
                "calls".into(),
                Value::List(vec![
                    Value::Bytes(swap.clone()),
                    Value::Bytes(moved.clone()),
                ]),
            )]),
        )
        .unwrap();
    let mut vectors = Vec::new();
    for (name, call) in [
        ("swap_stake_limit", swap),
        ("move_stake_limit", moved),
        ("batch_swap_move", batch),
    ] {
        let proof = generate_extrinsic_proof(&call, &extra, &additional, &metadata, &info).unwrap();
        let payload = [call.as_slice(), extra.as_slice(), additional.as_slice()].concat();
        vectors.push(serde_json::json!({"name":name, "blob":hex::encode(payload), "metadata":hex::encode(proof), "digest":hex::encode(digest)}));
    }
    std::fs::write(output, serde_json::to_vec_pretty(&vectors).unwrap()).unwrap();
}
