# TLS root-loading repairs

The canonical full gate run in `root-m4-01-gates-weight-fixed.log` completed
1,184 tests: 1,179 passed, five failed and four separate tests were skipped.
The failures were the miner runtime integration test and four RPC tests:
retry-forever connection cycling, handshake policy refusal, handshake timeout
diagnostics, and cancellation during archive initialization.

The miner fixture used native TLS roots for two clients that send only loopback
HTTP. The preserved run failed with no usable native roots. Subsequent untouched
current/MSRV retries passed, so this failure was intermittent. The fixture now
supplies an owned ring TLS configuration with an empty trust store. The shared
initializer still checks registration, unlocks the actual hotkey and supplies
that signer to client construction; production still uses `SignedClient::new`
and its fail-closed native-root behavior. The test retains authenticated 3 MiB
requests, state recovery before routing and shutdown persistence.

RPC initialization synchronously loaded native roots even when every endpoint
used plaintext WebSockets. That host lookup exceeded the original test deadlines.
The private connection path now skips native enumeration only when all primary,
fallback and archive endpoints use `ws`. Any `wss` endpoint retains native trust
loading. Explicit extra roots are still validated for every configuration; ring,
ALPN and certificate verification remain. The public `tls_config` function keeps
its native-plus-extra-root contract.

Both independently isolated toolchains (Rust 1.99 and 1.89) pass all 80 affected
RPC unit/socket/lifecycle/metrics tests and the miner integration test. Strict
Clippy passes for the affected targets on both toolchains. Regression coverage
includes secure fallback, lazy secure archives, rejection of untrusted WSS
certificates, invalid explicit roots and native-loader selection. The original
deadlines remain. No test was ignored, no TLS verifier bypassed and no host
settings changed. These affected-suite results do not establish full gates.

Root reviewed and adopted both exact patches with scope and SHA-256 checks:

- RPC: `7fecb30fd570e2ee1a9012c1f9861a498aeee16b9ff8fff64cd739ad8f49b321`.
- Miner: `b5fb8489b9d0ee08c38d88abe31a955dcb9bf0ae1ed8175a7dca1e2289f95f11`.

Proof is retained under the recorded execution root in `rpc-native-roots/proof`
and `miner-tls.aBWNbI/evidence`, with before/after source, exact logs and cleanup
audits. Owned snapshots, test fixtures and isolated build outputs were removed
after checking live dependencies; shared canonical caches remain active. Root's
adoption manifest is `tmp/root-tls-repair-adoption.json`. The new complete gate
run is `tmp/root-tls-repaired-gates-final.log` and must finish before publication.
That complete rerun passed all preceding checks and 1,190 of 1,191 workspace
tests, with four separate skipped tests. All four RPC deadline/cancellation
failures are resolved. The miner fixture still failed its routing deadline:
`AxonPublisher::new` performed another synchronous native-root lookup before
starting the router, even with an explicit IP. The independently verified
publisher repair is now adopted. Configuration validates immediately; a
per-publisher `OnceCell` initializes the same native-root/ring discovery client
only at the first automatic IP lookup, then reuses it across configuration
changes. TLS/HTTP construction errors retain their typed variants and now
surface when discovery first runs. HTTPS trust, ALPN, redirect refusal and
five-second per-source deadlines remain.

The counted-loader regression fails on both compilers against the retained
eager-construction source, then passes after repair. All ten publisher tests
and the unchanged miner test pass on current/MSRV; the miner test completes
in 0.66 seconds on both. Strict Clippy passes both. Patch SHA-256:
`1099b99dce3f009992d70a6e18b9ff24ea90914766b692f0fcdeb6b6981058b3`.
The scoped patch and cleanup evidence remain in `publisher-lazy.tMA9DH`.

The standalone no-host gate passed before this final publisher adoption.
The complete gate rerun after all three repairs is active in
`tmp/root-publisher-repaired-gates.log`. Its workspace nextest suite passes
all 1,192 tests in 43.321 seconds, with four separate skipped tests. Three
skips are environment-isolated child cases executed by their passing parent
tests; the hosted oracle replay remains pending its immutable CI vectors.
Doctests, no-host and MSRV also pass. The features job then failed in the
temporary no-dev manifest copy: forwarding edges still named removed dev
dependencies. That harness repair and remaining gates must pass before
claiming full gates. Root's exact adoption manifest is
`tmp/root-publisher-lazy-adoption.json`.

An earlier rerun launched before patch adoption and was stopped; it is retained
as `tmp/root-tls-repaired-gates.log` and is not repair-validation evidence.
