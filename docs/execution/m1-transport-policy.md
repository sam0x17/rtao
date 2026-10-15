# M1 transport policy and TLS provider checks

`cargo test --locked -p rtao-rpc policy:: --lib` passed all 16 policy tests on
the Mac Studio with Rust/Cargo 1.99.0. The tests consume the extracted v478
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

Reproduction requires a new independent workspace with path dependencies on
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

The disposable fixture manifest, source, lockfile, and temporary logs were
removed after the liveness and dependency audit. The shared task build cache
remains needed by the ongoing plan execution.
