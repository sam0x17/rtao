//! Protocol hot paths; no socket, node, or secret files.
#![allow(clippy::expect_used)]
#![allow(missing_docs)] // Criterion generates a public benchmark entry point.

use bittensor_core::{
    codec::{extrinsic::TxParams, value::Value},
    keys::Keypair,
    runtime::Runtime,
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rtao_core_ext::ops::storage::decode_storage_bytes;
use std::hint::black_box;

fn protocol(c: &mut Criterion) {
    let runtime = Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .expect("metadata");
    let weights = runtime
        .storage_entry("SubtensorModule", "Weights")
        .expect("Weights");
    let value = Value::List(
        (0..256)
            .map(|uid| Value::Tuple(vec![Value::Uint(uid), Value::Uint(257 - uid)]))
            .collect(),
    );
    let mut encoded = Vec::new();
    runtime
        .encode_id(weights.value_type, &value, &mut encoded)
        .expect("weights fixture");
    let keys: Vec<_> = (0..256)
        .map(|uid| {
            runtime
                .storage_key(weights, &[Value::Uint(1), Value::Uint(uid)])
                .expect("map key")
        })
        .collect();
    let mut group = c.benchmark_group("storage");
    group.throughput(Throughput::Elements(256));
    group.bench_function("map_page_decode_256", |b| {
        b.iter(|| {
            black_box(
                keys.iter()
                    .map(|key| {
                        let free = runtime
                            .decode_storage_key_params(weights, black_box(key), 1)
                            .expect("free map key");
                        let value =
                            decode_storage_bytes(&runtime, weights, Some(black_box(&encoded)))
                                .expect("map row");
                        (free, value)
                    })
                    .collect::<Vec<_>>(),
            )
        })
    });
    group.finish();
    let pair = Keypair::from_uri("//Alice", 1).expect("public development key");
    let call = runtime
        .compose_call(
            "System",
            "remark",
            &Value::record(vec![(
                "remark".into(),
                Value::Bytes(b"rtao benchmark".to_vec()),
            )]),
        )
        .expect("remark call");
    let params = TxParams {
        era: Value::Str("00".into()),
        nonce: 0,
        tip: 0,
        tip_asset_id: None,
        genesis_hash: [1; 32],
        era_block_hash: [1; 32],
        metadata_hash: None,
    };
    c.bench_function("extrinsic/sign_and_assemble", |b| {
        b.iter(|| {
            let payload = runtime
                .signature_payload(black_box(&call), &params)
                .expect("payload");
            let signature = pair.sign(&payload).expect("signature");
            black_box(
                runtime
                    .encode_signed_extrinsic(&call, pair.public_key_bytes(), &signature, 1, &params)
                    .expect("extrinsic"),
            )
        })
    });
}

criterion_group!(benches, protocol);
criterion_main!(benches);
