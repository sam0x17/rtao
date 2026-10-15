//! Signed HTTP throughput without network latency.
#![allow(clippy::expect_used)]
#![allow(missing_docs)] // Criterion generates a public benchmark entry point.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use futures::executor::block_on;
use http::Method;
use rtao_btauth::{Clock, ClockError, InMemoryNonceStore, Verifier, VerifyConfig, sign};
use rtao_primitives::CryptoType;
use rtao_signer::{KeypairSigner, Signer};
use std::{hint::black_box, sync::Arc};

struct FixedClock;
impl Clock for FixedClock {
    fn now_ns(&self) -> Result<u64, ClockError> {
        Ok(1_752_076_800_000_000_000)
    }
}

fn btauth(c: &mut Criterion) {
    let receiver = KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)
        .expect("Bob")
        .account();
    let body = vec![b'a'; 4096];
    let mut group = c.benchmark_group("btauth");
    group.throughput(Throughput::Elements(1));
    for crypto in [CryptoType::Sr25519, CryptoType::Ed25519] {
        let name = match crypto {
            CryptoType::Sr25519 => "sr25519",
            CryptoType::Ed25519 => "ed25519",
        };
        let signer = KeypairSigner::from_uri("//Alice", crypto).expect("Alice");
        let headers = block_on(sign(
            &signer,
            &Method::POST,
            "/generate?q=a%20b",
            &body,
            Some(&receiver),
            &FixedClock,
        ))
        .expect("signed headers");
        let verifier = Verifier::new(
            receiver,
            VerifyConfig::default(),
            Arc::new(InMemoryNonceStore::new()),
        )
        .expect("verifier");
        group.bench_function(format!("sign_{name}"), |b| {
            b.iter(|| {
                black_box(
                    block_on(sign(
                        &signer,
                        &Method::POST,
                        "/generate?q=a%20b",
                        black_box(&body),
                        Some(&receiver),
                        &FixedClock,
                    ))
                    .expect("sign"),
                )
            })
        });
        // Stateless checks are the repeatable verify hot path. A consumed
        // nonce would measure replay rejection after the first iteration.
        group.bench_function(format!("authenticate_{name}"), |b| {
            b.iter(|| {
                black_box(
                    verifier
                        .authenticate(
                            black_box(&headers),
                            &Method::POST,
                            "/generate?q=a%20b",
                            &body,
                            FixedClock.now_ns().expect("clock"),
                        )
                        .expect("authenticate"),
                )
            })
        });
        group.bench_function(format!("verify_and_commit_{name}"), |b| {
            b.iter_batched(
                || {
                    Verifier::new(
                        receiver,
                        VerifyConfig::default(),
                        Arc::new(InMemoryNonceStore::new()),
                    )
                    .expect("fresh verifier")
                },
                |verifier| {
                    black_box(
                        block_on(verifier.verify(
                            &headers,
                            &Method::POST,
                            "/generate?q=a%20b",
                            &body,
                            FixedClock.now_ns().expect("clock"),
                        ))
                        .expect("verify"),
                    )
                },
                criterion::BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, btauth);
criterion_main!(benches);
