//! Metadata parse and live-metadata storage key construction.
#![allow(clippy::expect_used)]
#![allow(missing_docs)] // Criterion generates a public benchmark entry point.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rtao_core::{codec::Value, runtime::Runtime};
use std::hint::black_box;

fn runtime(c: &mut Criterion) {
    let bytes = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
    c.bench_function("runtime/v15_parse", |b| {
        b.iter(|| black_box(Runtime::parse(black_box(bytes), 478, 1, 42).expect("metadata")));
    });
    let runtime = Runtime::parse(bytes, 478, 1, 42).expect("metadata");
    let info = runtime
        .storage_entry("System", "Account")
        .expect("System.Account");
    let keys: Vec<_> = (0u32..10_000)
        .map(|n| {
            let mut account = [0; 32];
            account[..4].copy_from_slice(&n.to_le_bytes());
            vec![Value::Bytes(account.to_vec())]
        })
        .collect();
    let mut group = c.benchmark_group("storage");
    group.throughput(Throughput::Elements(keys.len() as u64));
    group.bench_function("keys_10000", |b| {
        b.iter(|| {
            black_box(
                keys.iter()
                    .map(|key| {
                        runtime
                            .storage_key(info, black_box(key))
                            .expect("storage key")
                    })
                    .collect::<Vec<_>>(),
            )
        })
    });
    group.finish();
}

criterion_group!(benches, runtime);
criterion_main!(benches);
