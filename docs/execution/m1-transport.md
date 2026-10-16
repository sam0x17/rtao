# M1 transport validation

Current source review traces all 40 pinned `test_rpc.py` functions and 91
explicit assertions to native witnesses or documented API/input adaptations.
The actual refused-Client-initialization socket observer and the ten-result
batch cardinality assertion passed in
[hosted run 38873429391](https://github.com/sam0x17/rtao/actions/runs/38873429391)
at `d01830cdf6a7e1301ca4a2629b35663617f97dab`, together with all three guarded
parameter-producer fixtures and the existing socket/public-client witnesses.
All six blocking jobs passed; the main result was 3,051 passed, zero failed and
four skipped tests, plus nine doctests across 19 crate suites. The separate
two-provider RPC test passed once with zero ignored or filtered tests, as
recorded in [m1-transport-policy.md](m1-transport-policy.md).

This supplies current transport execution for the applicable
[M1 Q30 development exit](m1-client.md#current-q30-development-exit).
Generated parameter inputs are authenticated and adopted. Complete source review
and exact hosted results now reconcile test_rpc.py and the FakeNode harness;
hosted report run 38876950939 refreshed their generated matrix entries at
`82d82d7`. These Rust results add no
Python/oracle, HTTPS two-provider, live-network, platform or release claim.

`rtao-rpc` now owns a single WebSocket supervisor and implements the real
`subxt_rpcs::client::RpcClientT` interface. `FakeNode` exercises it over loopback
WebSockets; `FakeChain` implements that interface directly over the same encoded
`FakeState` used by the socket fake.

The supervisor correlates concurrent requests and batches, resends only allowed
reads, and returns a typed extrinsic hash when a transmitted submission's outcome
is unknown. Every send checks whether its caller remains alive. Cancellation also
covers a subscription response promoted into routing before its caller consumes
the reply. Subscription identity includes a connection epoch, so an old stream
cannot cancel a new stream that reuses the same server ID.

Socket writes process control commands while waiting and have a deadline. A quiet
idle connection starts a fresh response deadline when it first sends an unanswered
request. Replays, diagnostic discovery and detached unsubscribes consume rate
limit tokens. Discovery responses do not reset the unanswered user-request retry
budget, for either text or binary frames. Policy refusals remain terminal and
retain structured JSON-RPC errors and bounded HTTP diagnostics. HTTP capture
keeps at most 4096 decoded bytes, processes at most 64 KiB of body wire data,
limits chunk lines to 1024 bytes and uses the original handshake deadline.
Optional body failures preserve the known HTTP status and policy headers;
complete body acquisition and validation are not promised.

Historical reads use the archive pool lazily and retry once. This applies to
`Session::request`, the raw subxt adapter and individual failed batch members.
Explicit close releases both the primary and initialized archive connections and
cancels an archive handshake still being initialized. Metrics report aggregate
inflight requests across concurrent sessions and observe cancellation, shutdown,
policy failure and successful completion.

Both WebSocket size limits are 256 MiB by default. The transport accepts the
required 20 MiB response in one frame. TLS uses an explicit ring configuration
with native and extra roots and HTTP/1.1 ALPN. The library never installs or reads
the process-default crypto provider. Current two-provider source and hosted
validation status are recorded in
[m1-transport-policy.md](m1-transport-policy.md).

## Historical checks

The following commands record historical validation. Current substantial checks
run in free `ubuntu-latest` CI under Q30. The former execution root is retired
and absent; do not recreate it or run this suite locally.

```text
cargo test -p rtao-rpc --all-features --locked
cargo test -p rtao-testkit --features rpc-rate-limit,rpc-metrics \
  --test rpc --test rpc_lifecycle --test rpc_metrics --test fakes --locked
cargo clippy -p rtao-rpc --all-features --all-targets --locked -- -D warnings
cargo clippy -p rtao-testkit --features rpc-rate-limit,rpc-metrics \
  --test rpc --test rpc_lifecycle --test rpc_metrics --locked --no-deps -- -D warnings
cargo +1.89.0 check -p rtao-rpc --all-features --all-targets --locked
cargo check -p rtao-rpc --no-default-features --locked
```

The policy suite has 16 passing tests. The socket suite has 38 with the rate feature
enabled, including the source ports and cancellation, size, TLS and buffering
regressions. The lifecycle suite has nine tests with that feature. A recorder test
checks inflight, completion and policy metrics. The fakes have 16 integration tests
and 11 state unit tests. Strict Clippy and the Rust 1.89 checks pass.

## Public-boundary source ports

The transport source audit covers 34 of the 40 declarations in pinned
`sdk/python/tests/unit/test_rpc.py`. The six public-boundary declarations now
pass in `crates/rtao-client/tests/public_rpc.rs`, using the production client,
extrinsic lookup and submission helpers. Together the two layers cover all 40:

- `test_rpc_substrate_exposes_public_policy_error`
- `test_rpc_substrate_exposes_public_blank_timeout_error`
- `test_rpc_substrate_stream_preserves_public_policy_metadata`
- `test_rpc_substrate_write_helper_preserves_public_policy_metadata`
- `test_rpc_substrate_submit_preserves_policy_metadata_and_clears_nonce`
- `test_rpc_substrate_submit_timeout_is_a_public_connection_error`

Network names and endpoint presets remain sourced from generated catalog tables.
The client constructs `EndpointPool` through the transport builders and chooses
`RateLimit::PUBLIC` for the public presets. This preserves the transport crate's
third-party-only dependency layer. A workspace consumer must enable the
`rtao-rpc/rate-limit` feature explicitly because workspace path dependencies
disable default features.

Those historical narrow checks alone established no milestone exit or release
tag. Current hosted evidence is recorded above. Disposable transport probes
were removed; the former task build root is retired and creates no current
cache dependency. Shared global Cargo/toolchain caches remain preserved.

The final repeat of the testkit target Clippy command reached a concurrent
primitive edit and stopped because the newly declared `python`, `drand`,
`hyperparams` and `ratio` modules had not yet been written. The earlier target
Clippy run passed. Any integrated repeat belongs in hosted CI under Q30.
The final standalone RPC Clippy, no-default-feature check and Rust 1.89 check
passed; its active normal/build graph contains no internal rtao crate,
`bittensor-core`, jsonrpsee, reqwest, native-tls or aws-lc dependency.
