# M2 client submission

`rtao-client` now prepares, signs, assembles and submits spec-stamped calls through the portable core. Ordinary and externally signed submissions use the client's shared `NonceManager`. A lease settles only after pool acceptance or a resolved inclusion, including dispatch failure. Every unsettled exit clears synchronously; reconciliation starts after the lease has dropped and never reserves or repins the nonce.

Preparation uses a fresh pool-aware nonce without changing cached allocations. The default mortal era is the extracted 128 blocks, anchored at the finalized height and its core-computed birth block. Immortal transactions use genesis. Finality lag produces the configured warning and optional metric. Metadata-verifying signers receive the cached RFC-0078 digest, the exact signed extension parts and runtime context needed to generate a proof. The runtime's actual `specName` supplies the digest identity. Software signers leave metadata-hash mode disabled; tip remains zero.

`UnsignedExtrinsic` exposes read-only getters and borrowed signing views. Its call, nonce, era, payload, extension parts, JSON and context cannot be modified independently. Signature attachment checks the live spec and transaction versions, genesis identity and negotiated MultiSignature variant before the core encoder assembles the extrinsic. The oversized-payload test verifies the signature over the core's single normalized hash. The metadata signer test generates an actual core proof from the same call, extension parts and context, including a renamed runtime.

`Wait::Pool` preserves Python's `success = true` and `Submitted (not waiting).`, with no fabricated inclusion fields. Watched submissions resolve their complete encoded extrinsic, parent-runtime events, checked native fee and staged `receipt::Outcome`. Dispatch diagnostics use the staged Python result-message renderer, including non-string shield reasons. RPC pool codes, custom-code regex, terminal-watch regex and watch names come from generated authenticated catalogs; custom codes take precedence, then exact node codes, then text classification. Policy metadata remains a typed client error.

Retraction keeps the watch open for later inclusion. A retracted watch that ends or subsequently drops forces finalized-history reconciliation. A dropped-only watch uses that scan when requested and otherwise retains Python's exact failure. `finalityTimeout` resolves its block; `Wait::Inclusion` returns that inclusion, while `Wait::Finalization` confirms canonical finality. A mortal scan stops with typed `Expired` after finalized history passes the validity window. Immortal reconciliation has no expiry; its caller can supply a deadline.

An absolute operation deadline covers nonce acquisition, preparation, signing, encoding, watch and reconciliation. Standalone preparation honors the same option. The client checks both deadline and shutdown immediately before broadcast, so a synchronous signer that runs past its deadline cannot submit late. Closing an injected transport client also prevents pool and watch broadcasts and cancels pending signing. Dropping an active watch unsubscribes and clears its nonce lease. The earlier raw signed-bytes helper now uses the same synchronous invalidation rule without allocating or changing a nonce; cancellation and shutdown during its request clear the existing cache entry.

`estimate_weight` uses only public account and algorithm identity. It builds the staged immortal probe with a zero signature, zero tip and disabled metadata hash, calls `TransactionPaymentApi_query_info`, and decodes the live metadata output. Its fresh nonce does not change the cache and it never calls a signing method or the deprecated `payment_queryInfo` RPC.

Review also exposed transaction-version-only cache upgrades. Runtime and in-flight entries now include `(genesis, spec, transactionVersion)`, and historical block mappings retain that identity. Same-spec metadata is reused and reparsed off the reactor under the live stamp. Python-compatible disk filenames and bytes remain unchanged; the queried transaction version is authoritative over a stale saved header. Old handles remain immutable. Tests show both refusal of an old prepared signature and successful preparation/submission of a fresh call after this upgrade.

Validation uses the production client over `FakeChain`, actual pinned metadata and real signatures. The earlier combined validation ran 141 client tests plus one compile-fail check; all features and no default features passed. That client and signer run passed 156 checks on current Rust and Rust 1.89 with all features. Strict all-target Clippy passed on both toolchains. Its 28 submission tests covered the behaviors above and separate admission deadlines; three structured pool-error tests covered all custom bytes and original diagnostic retention. The 24 cache tests covered live-stamp recovery, historical isolation and concurrent in-flight versions. The facade and transaction consumers compiled with all features. The parent-owned instrumented nightly `event_splitting` target passed its 15-second run, using production `Events::decode` and a valid nonempty seed; all seven targets passed in that run. Combined validation logs and TX evidence are recorded in [m2-tx.md](m2-tx.md). Later offline cache support is recorded in [m1-runtime-cache.md](m1-runtime-cache.md).

Commands used with the recorded temporary root and `--locked`:

```text
cargo test --locked -p rtao-client --all-features
cargo test --locked -p rtao-client --no-default-features
cargo +1.89.0 test --locked -p rtao-client -p rtao-signer --all-features
cargo clippy --locked -p rtao-client -p rtao-signer --all-features --all-targets -- -D warnings
```

Logs are bounded beneath `target/execute-plan.6Ro0PC/tmp/m2-client-{tests,nodefault,msrv,clippy}.log`; the focused submission log is `m2-submission-tests.log`. Instrumented fuzz results remain in the parent-owned `fuzz/results.json` beneath the recorded root. Raw preparation and signer JSON views are implemented here; Python prepared-extrinsic import/export and interop lane 6 remain the M5 work.

No local Python, user wallets or Keychain state were accessed. Test-created cache/config directories are removed by their owners. The shared build and fuzz caches remain live for the continuing execution task. No commits, pushes, PLAN checkboxes or milestone exit claims were made by this child.

## Full-deadline hash retention

`submit`, `submit_leased` and `submit_signature` retain the attempted extrinsic
hash outside the future cancelled by the complete operation deadline. A timeout
after an author call is attempted returns `FateUnknown { hash }` and invalidates
the nonce cache, including timeouts during watch, report resolution or
reconciliation. Timeouts before the author attempt remain ordinary hash-free
RPC errors. The final budget check also clears the cache if it races with lease
settlement. No timeout path rebroadcasts the transaction.

The complete budget still covers reconciliation. Before starting a scan the
client checks the original full deadline; exhausted budgets return the known
hash without issuing a scan. An admission or watch transport timeout can scan
finalized history when time remains, after the lease has dropped. The admission
limit remains released after acknowledgment.

The focused regressions use real signed and encoded transactions. They exercise
all three entry points with acknowledged watches and reconciliation on/off,
equal full/admission acknowledgment limits for both pool and watch submission,
stalled report resolution, stalled reconciliation and zero-duration pre-send
budgets. A watch transport-timeout fixture verifies successful finalized-history
recovery within the remaining full budget. Exact unknown-fate hashes are checked
against Blake2b-256 of the accepted SCALE bytes; each postbroadcast case records
one accepted submission, and exhausted-budget watches start no reconciliation
scan.

The M3 certificate read adapter adds `models::NeuronCertificate` as an ordinary
`ReadValue` leaf. Its test decodes `NEURON_CERTIFICATES` using pinned Finney V15
metadata and a 64-byte public key, and checks missing, truncated and trailing
storage bytes.

Validation on 2026-10-14 used `--locked`: all 33 submission tests and the
certificate regression passed on Rust 1.99 and Rust 1.89. Strict client Clippy
with all features and all targets passed on both toolchains. Owned Rust
formatting passed with edition 2024. Final logs are
`target/execute-plan.6Ro0PC/tmp/m2-submission-full-deadline-{current,msrv,clippy,clippy-msrv,format}.log`
and `m3-client-certificate-{current,msrv}.log`. No disposable directories were
created; the shared build cache and bounded logs remain live for parent and
peer validation.

This follow-up validates the client's three submission APIs. Security review
separately identified executor wrappers in `rtao-tx/src/executor.rs` that can
cancel or discard a known-hash client result under `TimeBudget::run`. Executor
ownership and public executor regressions remain with the parent before S04
can be closed for ordinary intent execution.
