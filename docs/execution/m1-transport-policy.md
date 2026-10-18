# M1 transport policy and TLS provider checks

## Current source and hosted validation

[Hosted run 38873429391](https://github.com/sam0x17/rtao/actions/runs/38873429391)
passed all six blocking jobs at `965669d709bd8b8435f76ec7db5def9d471fe0d9`.
The main job passed the policy/socket and public-client fixtures, including the
refused-initialization socket observer and all three actual-resolver fixtures
for literal updates, authenticated spans and unsupported producer refusal.
The two source-extracted pytest parameter tables and seven exact `pyt:` origins
are integrated; hosted generation in run 38871417189 at `2143d2e` supplied the
authenticated outputs, and the final extraction check passed. These are Rust
fixture results. Complete source review now reconciles test_rpc.py and the
FakeNode harness, while Python execution remains unverified.

The maintained independent workspace at `ci/tls-two-providers` passed
`wss_with_two_compiled_providers_and_no_process_default` in the same run's
[features job](https://github.com/sam0x17/rtao/actions/runs/38873429391/job/116882706610):
one passed, zero failed, ignored or filtered. It used the real RPC connector and
TLS `FakeNode` with both ring and AWS-LC compiled for Rustls 0.23.46, and checked
that no process default existed before startup, after the request and after
shutdown. The authenticated phase-`check` graph artifact (ID 12035095089) binds
that exact source and the shared Rustls/provider graph; its `graph.json` SHA256 is
`ff917bef9f77d4d05b0625b055798e45b3136f6f02fddc572be66e1ba35acf2b`.
The independent 127,044-byte lock, prepared in run 38871434589, retains SHA256
`f193c79d7eab8fd4bbc06c7e0099a3bb3f8fdb46b39be8719c72c5fcd9730fe1`.

These results supply current RPC evidence for the applicable
[M1 Q30 development exit](m1-client.md#current-q30-development-exit).
The shipped workspace's AWS-LC bans, dependency inputs and locks remain
unchanged. HTTPS SignedClient, Python/oracle, live-network and additional
platform coverage remain separate obligations.

All substantial validation runs in bounded free `ubuntu-latest` jobs. Do not
create a local fixture workspace, install tools or resume a retired build cache.

## Historical evidence, 2026-10-14

`cargo test --locked -p rtao-rpc policy:: --lib` passed all 16 policy tests
locally with Rust/Cargo 1.99.0. The tests consume the extracted v478
constants, including the diagnostic limits, and cover the pinned Python
classifier, metadata truthiness, response body shapes, Unicode sanitization,
duplicate headers, reset fallback, close 1008, endpoint redaction, and raw retry
values with parsed seconds, milliseconds, and HTTP-dates. Six upstream test
functions have `pyt:` provenance in `policy.rs`; socket behavior is exercised by
the separate transport parity suite.

`cargo clippy --locked -p rtao-rpc --lib --all-features -- -D warnings` also
passed after integration of the transport error and supervisor changes.

The second-provider regression passed as an independent Cargo workspace under
the recorded task root. Its only test started the real TLS `FakeNode`, added its
self-signed certificate to `SessionConfig::extra_roots`, connected a `Session`,
and read `system_chain` over `wss://`. It checked that both compiled providers
were available and that the process-default provider was absent before startup,
after the request, and after shutdown. Exactly one connection was accepted.

The fixture used rustls 0.23.45, ring 0.17.14, aws-lc-rs 1.18.2, and
aws-lc-sys 0.46.0. Its independent lockfile's SHA-256 was
`a3b818dd941771e0804d19230d76183d7fc75183733091b72f51ca6fe542f428`.
The normal workspace lockfile contains neither aws-lc package; its dependency
bans remain in force. Both runs of the fixture test passed, most recently in
0.20 seconds, with no ignored tests.

The historical disposable workspace used path dependencies on
`rtao-rpc` and `rtao-testkit`, the root's pinned `w3f-bls` patch, tokio's `macros`
and `rt-multi-thread` features, `url`, and this rustls dependency:

```toml
rustls = { version = "0.23", default-features = false, features = ["ring", "aws-lc-rs", "std", "tls12"] }
```

The test body is:

```rust
#[tokio::test]
async fn wss_with_two_compiled_providers_and_no_process_default()
    -> Result<(), Box<dyn std::error::Error>>
{
    use std::time::Duration;
    use rtao_rpc::{ReadMethod, Session, SessionConfig};
    use rtao_testkit::FakeNode;

    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    assert!(!rustls::crypto::aws_lc_rs::default_provider().cipher_suites.is_empty());
    assert!(!rustls::crypto::ring::default_provider().cipher_suites.is_empty());
    let mut node = FakeNode::start_tls().await?;
    let mut config = SessionConfig::new(url::Url::parse(node.url())?);
    config.extra_roots = node.extra_root().into_iter().collect();
    config.handshake_timeout = Duration::from_secs(5);
    config.silence_timeout = Duration::from_secs(5);
    config.max_retries = 1;
    let session = Session::connect(config).await?;
    let chain: String = session.request(ReadMethod::SystemChain, Vec::<String>::new()).await?;
    assert_eq!(chain, "FakeChain");
    assert_eq!(node.connect_attempts(), 1);
    assert!(node.connections().iter().any(|connection| connection.accepted));
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    session.close().await;
    node.shutdown().await;
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    Ok(())
}
```

The disposable fixture manifest, source, lockfile and temporary logs were
removed after the liveness and dependency audit. The former shared task build
root is retired and absent. Current source and hosted checks above supersede
its reproduction instructions; no live-cache requirement remains.
