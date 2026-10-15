# M2 validator framework preparation

The validator framework is implemented in `rtao-neuron`, with a facade example
at `crates/rtao/examples/validator.rs` and the guide at
`book/src/validator.md`. This record describes local preparation. Hosted image,
localnet and interoperability checks require their own evidence. Q29 waives
the twenty-four-hour testnet soak.

`NeuronConfig` resolves explicit parsed flags before environment variables,
the shared btcli file and defaults. It supports dotted aliases and
`BTCLI_CONFIG`. The binary owns argument parsing; embedded callers provide a
subnet with `load_for`. The architecture's `load()?` sample cannot select a
required subnet without reading argv, so that method returns a missing-netuid
error and the example uses `resolve_matches(...).await`.

`StateCell` uses poison-recovering `std::sync::RwLock` guards. Its compile-fail
doctest checks that a guard held across an await cannot satisfy the callback's
`Send` bound. `EmaScores` uses hotkey identities and drops scores for replaced
and deregistered registrations. `StateStore` retains three fsynced, atomically
installed JSON generations. It serializes off the reactor without a `Clone`
bound, refuses identity and schema mismatches, and keeps its write lock until
blocking atomic installation ends even if the async save future is cancelled.

The runtime recovers state before `on_start` and supervises finalized heads,
epoch state, graph diffs, scoring, best-head clock observations and periodic
state saves. User scoring errors, panics and timeouts count toward readiness
and retry with bounded exponential backoff. The default scoring cadence waits
for finalized blocks. Weight work has a separate sixty-second shutdown grace;
scoring cancellation begins immediately. Final state saves before telemetry
closes. Unix signals cancel on the first SIGTERM/SIGINT and exit 130 on the
second.
Application callback cancellation depends on yielding. Rust futures cannot
preempt a synchronous application loop; the guide states this limitation.

`Metrics` retains process gauges, counters and all readiness conditions. An
explicitly installed binary recorder also collects the RPC metric family,
including escaped labels and latency count/sum series. Its
bounded HTTP listener exposes `/metrics`, `/healthz` and `/readyz` only when
configured. Binaries can explicitly install compact, JSON or pretty tracing
to stderr. Libraries leave subscriber ownership with their caller.

`SystemClock` supplies the default timelock clock. `ClockGuard` retains twenty
best-head observations, uses their minimum timestamp offset, warns above two
seconds of absolute skew and refuses system-clock commits above twelve.
`ChainClock` advances a verified timestamp with monotonic elapsed time.
`EpochClock` retains coherent finalized epoch state for scoring and scheduling.

The `rtao-validator --check-connection` diagnostic resolves normal network,
endpoint and TLS configuration, checks chain identity and required metadata,
and closes before reading a keyfile or requiring registration. The diagnostic
accepts no subnet argument; that identity has no effect on its chain check.
For protocol tests, `--synthetic-weights` scores registered destinations other
than self and owner equally. It is an explicit testing option; the demonstration
binary's default skips weight proposals.

The weight scheduler and reveal watcher use the shared executor and intents.
The context constructs that executor through the generated intent registry,
then applies the validator's subnet allowlist, zero-spend limit and raw-call
refusal. Its regression sends a valid `reset_axon` tool invocation to a
different subnet through the production constructor. The parsed intent reaches
the policy refusal before private-key unlock or transaction submission; a
public-only key source and the FakeChain request record verify both boundaries.
The seventeen-field preflight uses one best-head RPC; mechanism, owner and
activity fields use another coherent auxiliary fetch at the same hash. Retained
attempt outcomes expose gate refusals, remediation and call-surface pauses.
Prepared rows use only calls 118 and 119, with checked conforming, the previous
row guard, an explicit clock, era 128, finalized inclusion and reconciliation.
An invalid reveal round gets one recomputation. Accepted commits survive a
temporary inclusion-queue read failure, follow their actual commit epoch and
recover reveal events emitted during reconciliation from finalized history.
Miss attribution uses the first reveal snapshot and retained epoch notices.
A fixed version key is configured explicitly through
`NeuronRuntime::with_weight_policy`; there is no automatic chain-key mode.

The final isolated production-module run passed 25 tests and the guard
compile-fail doctest. This includes an actual subprocess kill/restart state
test, corrupt-generation recovery, source precedence, poisoned state recovery,
callback panic/error/timeout supervision, skew refusal, two-stage Unix signals
and HTTP health checks. Current Rust and Rust 1.89 passed; strict MSRV Clippy
passed. The additional production recorder and HTTP tests passed current Rust
and MSRV, with strict all-target Clippy. The isolated harnesses and completed
fixtures were audited and removed; shared build caches remain live.
The integrated current-Rust library suite passed all 40 tests, including the
best/finalized lag-three RPC proof, inclusion-epoch slip, late drand, version and
deregistration attribution, per-mechanism cadence and exact Null raw rows.
Rust 1.89 passed the full 40-test suite, the validator binary build and the
guard compile-fail doctest (41 checks). Strict MSRV Clippy passed all features
and all targets, the no-default-feature base crate, and the facade's real
`validator,prometheus` example. The example performs bounded signed JSON echo
fan-out and binds scores to the queried hotkey across UID replacement.
Logs remain under the recorded task root as
`tmp/neuron-integrated-{tests,msrv,clippy}.log`,
`tmp/neuron-example-clippy.log` and `tmp/neuron-base-clippy.log`.
The no-host image and TLS proof are
owned by the root's CI integration lane.

The registry attachment regression passes on current Rust and Rust 1.89.
Current Rust compiled the all-feature library test executable for the focused
regression; running that executable directly passed all 41 library tests.
Rust 1.89 passed all 41 library tests, the validator binary harness and the
guard compile-fail doctest. Strict Rust 1.89 Clippy passed all features and all
targets. Logs are
`tmp/neuron-registry-focused.log`, `tmp/neuron-registry-current-suite.log` and
`tmp/neuron-registry-msrv.log`, plus `tmp/neuron-registry-clippy.log`, under the
recorded task root. The regression
uses an in-memory FakeChain and creates no wallet files or scratch directory.
