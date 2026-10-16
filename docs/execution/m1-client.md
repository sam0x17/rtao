# M1 client read path

## Current Q30 development exit

The applicable M1 development exit is satisfied at
`d01830cdf6a7e1301ca4a2629b35663617f97dab`. M1-01 through M1-21 are complete;
M1-22 and M1-24 remain optional targeted hosted evidence, and M1-23 is deferred.

[Hosted run 38873429391](https://github.com/sam0x17/rtao/actions/runs/38873429391)
passed all six blocking jobs: check, features, MSRV, no-host, supply and semver.
The main job passed 3,051 tests with zero failures and four skipped tests, plus
nine doctests across 19 crate suites, strict Clippy, documentation and the
blocking source, pin and dependency checks. The bounded
`m0-m1-d01830c-hosted-results.json` record retains 45 exact M1 fixture passes.
The separate one-test RPC two-provider result and authenticated graph are
recorded in [m1-transport-policy.md](m1-transport-policy.md).

This exit covers client development under Q30. Source-row dispositions remain
separate, and the compatibility validation range remains empty. Python/oracle,
HTTPS two-provider, live-network, platform/device and release evidence retain
their separate statuses. Substantial validation stays in free `ubuntu-latest`
CI; the retired local execution root remains absent.

## Client behavior and historical checks

`rtao-client` now initializes a real `Session` or a supplied `RpcClient`, checks the connected genesis and ss58 format, requires a transaction version, resolves V15 metadata at the finalized head, and computes the runtime extension and surface checks. A public-network genesis mismatch keeps reads available and refuses signing with both hashes in the error. The compiled compatibility guard checks the spec range and exact raw metadata hash, algorithm, network and genesis; the current empty validation range warns without inventing evidence.

The client owns the head tracker, nonce cache and runtime cache. Background workers hold the inner state without the user-facing lifetime guard. Dropping the last client clone cancels head/runtime subscriptions and closes the socket; a real loopback node test observes closure within one second. Startup uses a 30-second deadline. Runtime updates are checked through the runtime subscription, finalized heads and the 12-second poll fallback. Block-number lookups cache only finalized history, with a 12-second finality probe and a bounded 512-entry map.

Snapshots stamp one block hash, height and parent-selected runtime. Typed and dynamic storage, runtime APIs, constants, map iteration and events use that snapshot. `fetch` coalesces mixed storage queries, preserves defaults and optional misses, deduplicates keys and chunks at 256 keys. Map pages use the live metadata hashers, retain the snapshot hash and decode reversible free key components. Constants come from the snapshot runtime. Archive runtime calls use the frozen Python legacy table, including double encoding; other V14 calls fail before sending an RPC. Connection token symbols come from a pinned storage-map scan and the shared seven-day display cache.

The six higher client-boundary cases from pinned `test_rpc.py` run against these public production wrappers: connection policy metadata, blank timeout wording, strict-stream policy metadata, extrinsic lookup policy metadata, submission policy metadata with nonce clearing, and submission retry exhaustion with nonce clearing. No test is ignored and no alternate test wrapper stands in for the client.

Historical verification at this checkpoint:

- Full all-feature product suite: 83 unit tests, 7 connection integration tests and 6 source-parity integration tests passed.
- Full no-default product suite passed with the same 96 tests.
- Strict all-feature, all-target Clippy passed after the checked-u64 legacy-stake correction.
- Rust 1.89 all-feature, all-target check passed after that correction.
- Focused view tests reproduce all checked-in dynamic storage/runtime-call/map goldens, snapshot constants, live-hasher changes, 300-key chunking, API-version refusal and V14 legacy selection.

Configuration evidence is in `m1-config.md`; metadata caching evidence is in `m1-runtime-cache.md`. The 22 runtime-cache tests use actual pinned V15 metadata and derived V14 metadata, with exact Python `STMD` bytes and fetch-path flags, four-runtime/512-block LRU bounds, shared-future cancellation, corrupt cache recovery and eight-file pruning. The 13 head tests include real independently retrieved header hashes, reconnect backfill, best-chain forks, strict subscription loss, slow-consumer recovery and cancellation. Test fixtures use injected task-root paths and disable user disk caches.

Historical build outputs and verification logs used the former execution root. That root is retired and absent; no current build/log dependency remains there. Current substantial validation uses free `ubuntu-latest` CI under Q30, with no local build or Python execution. Shared global Cargo/toolchain caches remain preserved.

`FakeState` now includes its explicit arbitrary hash in emitted headers, so the production tracker can resolve fake ancestry consistently. The client integration test follows advances from height 100 through 102 with matching finalized identities. The combined client/testkit suites and strict Clippy passed after this correction.

The connection-wide borrowed metagraph opt-out is `ClientConfig::metagraph_view`, enabled by default. `View::metagraph_view_enabled()` and its object-safe `DynView` twin carry this setting through client clones, snapshots, dynamic views and repinning. The client depends only on the boolean read policy, preserving the dependency direction toward the metagraph crate. A FakeChain integration test verifies false across every view path; strict all-target Clippy and Rust 1.89 checks pass after this addition.
