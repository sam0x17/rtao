//! Python-compatible conformance at validator-sized and large-subnet rows.
#![allow(clippy::expect_used, missing_docs)] // Criterion's generated entry point.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rtao_intents::weights::{ConformState, conform};
use rtao_primitives::{NetUid, Uid};
use std::hint::black_box;

fn weights(c: &mut Criterion) {
    let mut group = c.benchmark_group("weights");
    for size in [256, 2500] {
        let uids: Vec<_> = (0..size).map(Uid).collect();
        let weights: Vec<_> = (0..size).map(|uid| f64::from(uid + 1)).collect();
        let state = ConformState {
            uid: Uid(0),
            min_allowed_weights: 16,
            max_weight_limit: 300,
        };
        group.throughput(Throughput::Elements(u64::from(size)));
        group.bench_function(format!("conform_{size}"), |b| {
            b.iter(|| {
                black_box(
                    conform(
                        black_box(&uids),
                        black_box(&weights),
                        state,
                        NetUid(1),
                        false,
                    )
                    .expect("conform"),
                )
            })
        });
    }
    group.finish();
}
criterion_group!(benches, weights);
criterion_main!(benches);
