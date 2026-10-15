//! Compare raw decoders and complete projections on the same immutable payloads.
#![allow(clippy::expect_used)]
#![allow(missing_docs)] // Criterion generates a public benchmark entry point.

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rtao_core::codec::decode::Cursor;
use rtao_metagraph::{DecodePreference, Metagraph, MetagraphOptions, MetagraphView, SnapshotInfo};
use rtao_primitives::NetUid;
use rtao_runtime::{RuntimeHandle, models::MetagraphWire};
use scale_decode::DecodeAsType;
use std::hint::black_box;

fn metagraph(c: &mut Criterion) {
    let runtime = RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .expect("committed metadata");
    let output = runtime
        .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
        .expect("committed runtime API")
        .output;
    let fixtures: [(&str, u16, &[u8]); 4] = [
        (
            "finney-1",
            1,
            include_bytes!("../../../vendor/bench/finney-1.scale"),
        ),
        (
            "finney-3",
            3,
            include_bytes!("../../../vendor/bench/finney-3.scale"),
        ),
        (
            "finney-64",
            64,
            include_bytes!("../../../vendor/bench/finney-64.scale"),
        ),
        (
            "synthetic-2500",
            1,
            include_bytes!("../../../vendor/bench/synthetic-2500.scale"),
        ),
    ];
    let mut group = c.benchmark_group("metagraph_decode");
    for (name, _, bytes) in fixtures {
        group.throughput(Throughput::Bytes(bytes.len() as u64));
        group.bench_with_input(BenchmarkId::new("view", name), &bytes, |b, bytes| {
            b.iter(|| black_box(MetagraphView::parse(black_box(bytes), &runtime).expect("view")));
        });
        group.bench_with_input(BenchmarkId::new("typed", name), &bytes, |b, bytes| {
            b.iter(|| {
                let mut input = black_box(*bytes);
                black_box(
                    Option::<MetagraphWire>::decode_as_type(
                        &mut input,
                        output,
                        &runtime.core().types,
                    )
                    .expect("typed"),
                )
            });
        });
        group.bench_with_input(BenchmarkId::new("dynamic", name), &bytes, |b, bytes| {
            b.iter(|| {
                let mut cursor = Cursor::new(black_box(bytes));
                black_box(
                    runtime
                        .core()
                        .decode_id(output, &mut cursor)
                        .expect("dynamic"),
                )
            });
        });
    }
    group.finish();
    // The raw view intentionally omits projection/index construction. Measure
    // the public product path separately so the cheaper number cannot hide it.
    let mut group = c.benchmark_group("metagraph_projection");
    for (name, netuid, bytes) in fixtures {
        let raw = Bytes::copy_from_slice(bytes);
        for (path, decoder) in [
            ("view", DecodePreference::View),
            ("typed", DecodePreference::Typed),
            ("dynamic", DecodePreference::Dynamic),
        ] {
            group.bench_function(BenchmarkId::new(path, name), |b| {
                b.iter(|| {
                    black_box(
                        Metagraph::decode(
                            black_box(raw.clone()),
                            &runtime,
                            NetUid(netuid),
                            SnapshotInfo::default(),
                            MetagraphOptions {
                                decoder,
                                commitments: false,
                                view: true,
                            },
                        )
                        .expect("projection"),
                    )
                })
            });
        }
    }
    group.finish();
}

criterion_group!(benches, metagraph);
criterion_main!(benches);
