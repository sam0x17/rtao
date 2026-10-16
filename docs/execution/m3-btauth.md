# M3 signed HTTP server

The local validation below describes historical preparation before Q30.
Current client acceptance requires applicable free `ubuntu-latest` checks and
security review. Live Redis, interop and instrumented fuzzing are optional
targeted hosted evidence; no local service or campaign is required.

`rtao-btauth/server` supplies `BtAuthLayer`, `VerifiedCaller`, caller policies,
a bounded per-hotkey token limiter, and the Value-first `Json<T>` extractor.
The layer caps the raw body before collection, preserves its original bytes,
and verifies the original percent-encoded target including the query and a
trailing `?`. Nested axum routers use `OriginalUri` so a stripped route prefix
cannot change the signed target. The default cap is 4 MiB; the miner runtime
must apply the same `DefaultBodyLimit` to its inner router.

The request order is authenticate -> policy -> commit -> limit. Invalid
signatures receive 401 without policy or store access. A policy denial receives
403 without consuming replay capacity. Replay commitment precedes limiting,
so replayed traffic cannot spend a validator's tokens. Replays receive 401,
body overflow 413, token exhaustion 429 with `Retry-After`, and full or
unavailable state stores 503. Backend error details are suppressed. Successful
requests carry `Caller` and `CallerPriority` extensions and the original body.

Async policy or backend delays require another freshness check before handler
admission. The layer checks after policy and after commitment. Its default
verification clock uses Unix time with a continuously advancing monotonic
floor: a backwards wall-clock change cannot make an expired nonce fresh again.
A supplied verification clock must preserve the same expiry relationship.

`MetagraphPolicy::validators` requires membership and a validator permit. An
optional `StakeWeight` floor compares exact alpha-equivalent weights and also
sets the request priority. Each admission reads one shared graph snapshot.
`PerHotkeyLimiter` defaults to a burst of 64, one token per second, and at most
4096 hotkeys with outstanding debt. Fully replenished buckets may be removed;
live debt cannot be evicted to admit a new key.

`redis` supplies `RedisNonceStore::connect`. It checks for a positive Redis
memory bound and `noeviction`, then uses `SET btauth:{hotkey}:{nonce} 1 NX PX`
with exact signed integer nonce identity. Duplicate requests do not extend
expiry. The TTL rounds upward and adds one millisecond to cover the inclusive
freshness boundary and Redis's expiry resolution. Memory exhaustion returns
`Full`; failures and five-second timeouts
close all clones to further operations. The deployment must preserve replay
state across restart and failover and protect live keys and configuration from
mutation. Construction fails if `CONFIG GET` is unavailable. RESP fixture
tests exercise the real Redis client's protocol; live Redis validation is
still pending.

`legacy-v10` ports the documented sr25519 dotted-message recipe. The application
supplies its schema-specific body hash and owns receiver, freshness and replay
checks for that temporary bridge.

The retained `btauth_server` fuzz target mutates headers,
body and target around genuine sr25519 and ed25519 signatures and exercises
policy denial, overflow, successful admission and replay rejection through the
production Tower service.

At the historical preparation checkpoint, current Rust and Rust 1.89 passed
all 120 tests: 34 library, 28 client, 44 core
conformance and 14 server tests. Strict all-feature/all-target MSRV Clippy
also passed. The server tests include a signed 3 MiB JSON body, nested
encoded targets and empty queries, untagged/flattened floats and full-width
integers, exact refusal statuses, replay-safe limiter order, store bounds,
and expiration during async policy and store work. A local reader reviewed
the implementation twice; the timing, clock and nested-URI findings are fixed.
The server-only feature passed 13 tests on Rust 1.89; separate `legacy-v10`
and `redis` compilation also passed with defaults disabled.

The instrumented nightly libFuzzer server target completed 137,729 runs in
31 seconds with address sanitizer, 3,634 coverage edges and no crash. The
result and corpus archive were recorded under the former task root. Its
disposable corpus and artifact directories were audited and removed, and Q30
cleanup later retired the shared builds, tools and raw logs. The retained fuzz
driver lists the target; its standalone lock resolved on Rust 1.89 at that
checkpoint. These historical results cover M3 preparation. Hosted interop and
live Redis execution remain unverified optional coverage. The security source
pass is recorded in `docs/security/m3.md`.

The reader also identified an existing core boundary: a store configured to
exactly `max_age + allowed_skew` expires an in-memory nonce at the inclusive
freshness boundary. Verifier construction now requires retention strictly
greater than that sum. D-038 records the correction; the conformance test
rejects equality and one nanosecond below, and accepts one nanosecond above.
Historical full runs on current Rust and Rust 1.89 each passed all 120 tests;
their former log names were `tmp/root-resume-btauth-{current,msrv}.log`.
The clock-error client test uses the explicit TLS fixture instead of depending on native certificate
discovery. Strict Clippy, ordinary fixtures and security checks remain required
on the final hosted checkpoint. M3 is not marked complete by this record; no
Python or Docker was executed during that preparation.
