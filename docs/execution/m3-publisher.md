# M3 axon publisher preparation

`AxonConfig` and `AxonPublisher` are available with the neuron `miner` feature.
The publisher uses an explicit IP override before IPv4 discovery. Each discovery
source has a five-second deadline, redirects are disabled, and responses are
limited to 128 bytes. The HTTPS client uses an owned ring configuration and
native trust roots without a process-wide crypto default. IPv6 is accepted
through an explicit override. Non-global addresses require the local-network
opt-in; the resolved `local` neuron profile enables it.

Publication reads the axon and serving rate limit at one best-head snapshot.
When a certificate is configured, it also reads the stored neuron certificate
at that hash. It compares IP, port, IP type, protocol, version, algorithm and
public key. A certificate-only rotation uses `ServeAxonTls`; an old stored
certificate is ignored when no certificate is configured. Unchanged endpoints
return before private-key resolution or submission.

The certificate consists of an algorithm byte and at most 64 public-key bytes.
The publisher rejects longer values before HTTP or signing, with the D-017
explanation that the chain would silently discard them. The serving-intent
owner applies the same bound after Python-compatible IP and hex validation.

The first legal serving block follows the pallet's zero-last-serve exemption
and saturating rate-limit calculation. Pool codes 11 and 13 retain their typed
`InvalidIpAddress` and `InvalidPort` names. Code 12 schedules a retry after a
fresh best-head axon/rate-limit read, covering a rate-limit change after
preflight. Submission uses the shared policy-aware executor, era 128,
finalization, reconciliation and bounded deadlines. Unknown transmitted fate
blocks blind retries and exposes the extrinsic hash for operator reconciliation.

Nine production-module tests cover certificate-only rotation, unchanged
publication without unlock or submission, the ignored-certificate case,
scheduled rate limits, actual pool-rejection mapping and refreshed scheduling,
the 65-byte boundary, non-global rejection, IPv6 encoding and bounded discovery
fallback. Current Rust and Rust 1.89 pass all nine as part of the 61-test neuron
suite. Strict MSRV all-feature/all-target Clippy also passes. Logs are
`tmp/neuron-publisher-tests.log` under `target/execute-plan.6Ro0PC`.

These local checks supplement the required localnet scenario and interop lanes;
they do not claim a milestone exit. Discovery listeners are awaited and all
fake clients are closed. No publisher filesystem fixtures remain. Shared build
caches remain in use by the other execution lanes.
