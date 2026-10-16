# M2-16 signed HTTP client

## Current source and scope

The current SignedClient source keeps explicit ring TLS and controlled trust
roots. The loopback `https_uses_explicit_ring_and_requires_the_fixture_root`
fixture belongs to ordinary hosted client validation. The retained dual-provider
HTTPS record below used Rustls 0.23.45 and its own source snapshot; it is not a
fresh run against the current graph. Current M1 RPC two-provider evidence in
[m1-transport-policy.md](m1-transport-policy.md) exercises WSS, not SignedClient
HTTPS. No fresh dual-provider HTTPS result or additional M2-16 requirement is
inferred from that RPC proof.

Substantial validation belongs in bounded free `ubuntu-latest` CI under Q30.
Do not restore the historical workspace, build it locally or install tools.
M2-S02 remains open and unaccepted; the M2 milestone exit remains open.

## Historical preparation and validation

The `rtao-btauth/client` feature implements `SignedClient`,
`SignedRequestBuilder`, `FanOut` and per-UID `FanOutError` results. Its public
target vocabulary comes from `rtao_primitives::axon`; it has no dependency on
the RPC, client, runtime, metagraph or transaction crates.

Requests materialize JSON or supplied body bytes before signing. The payload
uses the URL's percent-encoded path and exact query, including an empty
trailing `?`, and binds the receiver's hotkey. JSON is serialized once across
explicit retries. Every attempt calls the signer and shared nonce clock again.
Applications can add headers, but cannot replace authentication headers,
authority or body-framing headers, or nominate authentication fields through
`Connection`. `CONNECT` is rejected before signing because HTTP sends its
target in authority-form rather than the protocol's signed path/query form.
Serialization/build errors remain sticky
on the builder and surface before transmission.

`SignedClient::new` uses native trust roots, a shared monotonic `SystemClock`
and an explicit ring `rustls::ClientConfig`. `with_clock`, `with_extra_roots`
and `with_tls_config` support controlled clocks and trust stores. The reqwest
0.13.5 builder receives an owned TLS configuration with `h2` and `http/1.1`
ALPN. No library code installs or consults a process-default TLS provider.
Redirects and reqwest automatic retries are disabled: they could replay an
already signed request or change its destination without signing again.

The request builder supports `.json`, `.body`, `.header`, `.timeout` and
`.send`. It returns a streaming reqwest response, retaining HTTP status and
response headers for the caller. The timeout covers signing; only the
remaining duration is passed to the HTTP request so streamed body consumption
also shares that budget.

`fan_out_stream` yields responses in completion order and `fan_out` collects
the same stream. Every result retains the target UID. Defaults are 256
concurrent attempts, a three-second connection timeout, twelve seconds per
attempt, a thirteen-second call deadline and zero retries. The call deadline
starts when the stream is created and includes queueing, signing, response
bodies and explicit retries. A check after synchronous JSON decoding rejects
results that finish after an attempt or call deadline. Expired queued targets return `Deadline` without
running the query factory or transmitting. Dropping the stream cancels its
pending futures and leaves queued targets unsent.

If retries are explicitly enabled, only connect/transport timeout/body errors
and HTTP 408, 429 or 5xx statuses are retried, within the same call deadline.
Signing, configuration, serialization, response-decoding and other HTTP
status failures retain their individual error identity and are not retried.
`deadline: None` disables the call deadline. Changing the attempt timeout
does not modify a separately stored deadline; configure both when changing
the budget. Successful JSON responses pass through
`rtao_primitives::json::from_slice`, preserving untagged/flattened float
behavior under the workspace's arbitrary-precision JSON feature.

Query factories and Serde callbacks run synchronously; they should complete
promptly to avoid blocking Tokio. The client checks the deadline after body
materialization and decoding, before returning success or transmitting a
request. It cannot preempt a blocking application callback.

`cargo test --locked -p rtao-btauth --all-features` passes 91 tests:
19 core/store units, 28 HTTP/HTTPS adapter regressions and 44 conformance
tests, with no failures or ignored tests. The final 28 adapter tests also pass
on Rust 1.89.0. Strict Clippy passes for
`rtao-btauth --all-features --all-targets` including its dependencies, and
formatting passes. The [raw results](btauth-client/verification.json) link to
bounded logs in the same directory. The [normal client dependency
tree](btauth-client/client-tree.txt) contains reqwest 0.13.5, with no chain
adapter crates, `aws-lc-*` or `native-tls` reached by this feature.

The raw loopback fixture observes actual HTTP request targets, headers and
body bytes. A strict verifier authenticates both signing algorithms and
commits successive retry nonces in one replay store. Tests cover percent
encoding, trailing `?`, once-only JSON serialization, reserved header
injection, every standard redirect status, no hidden transport retry,
configured retry limits, source error identity, completion order, peak
concurrency, cancellation, queued deadlines, signing and response-body
timeouts, stalled TLS connection timing, and synchronous callbacks finishing
past their deadlines. HTTPS rejects the untrusted self-signed certificate,
then succeeds through the explicitly configured trust root; the global TLS
provider remains absent before and after the test.

A historical independent Cargo workspace compiled ring and aws-lc simultaneously.
All 28 adapter tests pass there on current Rust and Rust 1.89.0, including
the same positive/negative HTTPS test. This exercises third-party provider
feature unification without adding aws-lc to the production lock. The
retained [manifest](btauth-client/two-provider-fixture.toml) and
[lock](btauth-client/two-provider.lock) preserve that historical fixture
provenance, including its old checkout paths. They do not authorize recreating
a local workspace. Its lock SHA-256 is
`d601d6b1278e7d07c0f8d4bb59dcbb858603e08b40f93ee0144ee1ea28e0b160`.
It uses rustls 0.23.45, ring 0.17.14 and aws-lc-rs 1.18.2.

Historical validation and the isolated TLS-provider fixture used the temporary root
recorded in `state.md`; no Python interpreter, container or chain write was
used. The disposable provider fixture, its exact test/fingerprint/incremental
artifacts and temporary logs were removed after checks for live processes,
open files, launchd/configuration references, symlinks and virtual
environments. The [cleanup record](btauth-client/cleanup.json) lists the
removed directories. That former task root is retired and absent; its build
and scratch caches are not current live dependencies. Useful source, lock,
results and manifests remain here; shared global Cargo/toolchain caches remain
preserved.
