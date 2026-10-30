# Running a validator

A validator owns a hotkey, keeps scores by hotkey identity, and proposes weights.
The framework pins chain reads, checks weight gates, supervises scoring failures,
persists state, and watches the commit's actual reveal epoch.

The source example is `crates/rtao/examples/validator.rs`, enabled by the
`validator` feature. It sends a signed JSON echo challenge to `/echo` and
scores exact replies; replace its `step` with your subnet's query and scoring
rules before operating a subnet. Substantial example builds belong in hosted
CI under the current development policy.

For an operator's existing build, the example accepts arguments such as
`--netuid 123 --network test --wallet validator --wallet-hotkey default
--metrics-addr 127.0.0.1:9090`. It connects to the selected endpoint; it does
not start a node. The hotkey must already be funded and registered in the
subnet. Keep wallet files read-only and persist the state directory across
restarts. Running a testnet validator is not a development prerequisite.

Configuration resolves flags, then environment variables, then the shared btcli
file, then defaults. `BTCLI_CONFIG` selects another shared file. Network and wallet
names use `BT_NETWORK`, `BT_CHAIN_ENDPOINT`, `BT_WALLET`, `BT_WALLET_HOTKEY` and
`BT_WALLET_PATH`. Framework settings use `RTAO_STATE_DIR` and `RTAO_METRICS_ADDR`.
`--submit-endpoint` or `BT_SUBMIT_ENDPOINT` sends weight commits, reveals and
their receipts through a separate full node while reads stay on the network
endpoint (see [Getting started](getting-started.md)).
Your binary parses arguments and passes its matches to
`NeuronConfig::resolve_matches`; the library never reads argv. Embedded callers
can use `NeuronConfig::load_for(NetUid(...))`.

In `step`, load `cx.metagraph` once and perform bounded signed miner queries with
`cx.http`. Acquire `state.write()` after the awaited queries complete. The state
guards cannot cross an await in a framework callback. `EmaScores` stores scores
by hotkey and removes replaced or deregistered identities when your
`on_metagraph` calls `state.apply(diff)`.

`FanOut::max_response_bytes` defaults to 1 MiB per miner response and must be
positive. Both query APIs reject an oversized declared length or stop collecting
when a chunk crosses the budget, before JSON decoding. `ResponseTooLarge` is
permanent and is never retried. Set an explicit larger budget if your protocol
needs it, accounting for concurrent responses and decoded JSON. The lower-level
signed request API returns a stream for caller-controlled consumption.

Return `WeightsProposal::Set` from the synchronous `weights` callback, or
`HoldPrevious` to keep the previous row and `Skip(reason)` when scores are not
ready. A row containing positive weights only for self or owner is held. Every
mechanism has a separate cadence and rate-limit slot. The scheduler reads its
preflight at one best head, while finalized heads trigger attempts and anchor
transaction mortality. Commit-reveal off uses direct mechanism weights;
commit-reveal on uses timelocked mechanism weights with the version 4 payload.
The commit is tagged with the chain's `CommitRevealWeightsVersion`. A tag
other than 4 is followed only under runtime code known to reveal that payload;
otherwise `last_attempt` reports an `IncorrectCommitRevealVersion` refusal,
`rtao_weights_cr_version_paused{mech="N"}` is 1 and commits wait while the
rest of the validator keeps running. An unexplained missed reveal of a commit
with a followed tag counts `rtao_weights_cr_version_refuted_total` and pauses
that tag again.
The weights version key defaults to zero. After upgrading the subnet binary,
set `--weights-version-key` to the version that release supports. The scheduler
refuses keys below the chain requirement and never follows that requirement
automatically.

Scoring errors, panics and configured timeouts count as failed steps. They retry
with a one-second delay doubling to sixty seconds, then reset after success.
The default cadence waits for each finalized block. Five consecutive failures
make `/readyz` return 503 while the process keeps running. `/healthz` reports that
the process is alive. These endpoints and `/metrics` are disabled unless a
metrics listener is configured.
Callback cancellation is cooperative. Async callbacks must yield; move CPU
work to the blocking pool. A synchronous loop that never yields cannot be
preempted by a step timeout.

Watch `rtao_step_errors_total`, `rtao_clock_skew_seconds`,
`rtao_weights_submitted_total`, `rtao_weights_commit_epoch_slip_total` and
`rtao_weights_reveal_missed_total{mech,reason}`. A reveal miss records the observed
epoch events and the first failing reveal condition. An owner-triggered epoch
can advance the reveal before a round becomes available; retain the metrics and
logs when investigating it.

A runtime change to a weight call pauses only the mechanisms that use that
call, and only when it cannot be composed any other way. A changed call whose
fields all still fit is composed by name against the live metadata, logged
once and counted in `rtao_weights_resolved_composition_total`. For a change
name resolution refuses, override `Validator::weight_call_encoder` (or call
`WeightScheduler::with_call_encoder`) to return a `WeightCallEncoder`. The
scheduler calls it only while the compiled call no longer matches, with the
final `WeightCall` values: the conformed row, or the encrypted commitment, its
reveal round and tag. It counts `rtao_weights_encoder_composition_total`. The
encoder returns complete SCALE call bytes, and rtao never signs them as
given. They must import strictly under the signing runtime (complete,
canonical, and re-encoded byte for byte by the core). They must also be a
`SubtensorModule` weight call of the requested kind with no nested call, and
must carry the scheduler's subnet and the prepared mechanism, row or
commitment, reveal round, version key and tag in every field the live call
has. Further call names returned by `additional_calls` are accepted only
while a semantic guard is installed. A refusal pauses that attempt with
`PausedSurface` and signs nothing. Preflight, executor policy, the semantic
guard, the journal, shielding and the bounded `IncorrectCommitRevealVersion`
retries apply as for compiled calls. Durable work queued while an encoder is
installed records its `contract()`. After a restart, that work stays
unresolved until an encoder with the same contract is installed again.
`rtao-validator --check-connection` reports a call composed by name as a
warning. It fails only when name resolution also refuses the call.

The default timelock clock is the system clock. The minimum timestamp offset
across twenty best heads filters propagation delays. The framework warns above
two seconds of absolute skew and refuses system-clock commits above twelve
seconds. Synchronize the host clock; embedded callers can use
`NeuronRuntime::with_clock(Arc::new(ChainClock::unanchored()))`. The runtime
refreshes this clock with verified best-head timestamps and waits for its first
anchor before preparing a commit.

State saves every five minutes and at shutdown. Each checkpoint records schema,
package version, subnet, hotkey, finalized block and save time. Writes use fsync
and atomic replacement, with three generations retained. Identity or schema
mismatches refuse recovery; a corrupt current generation can recover its
preceding valid checkpoint.
Each complete checkpoint is limited to 64 MiB. Load and rotation accept regular
files only and enforce the limit during acquisition. Failed rotation keeps the
current checkpoint intact.
Override `Validator::state_schema_version` when changing the application's saved
state format. The runtime refuses an incompatible checkpoint before invoking
`on_start`.

The first SIGINT or SIGTERM cancels scoring and new work. An in-flight weight
submission gets up to sixty seconds to reconcile, then the runtime saves state
and closes its listeners and connection. A second signal exits immediately with
status 130. A systemd unit should use `Restart=on-failure`, `KillSignal=SIGTERM`
and `TimeoutStopSec=90`.

The `rtao-validator` binary also accepts `--check-connection`. This checks the
configured endpoint's TLS connection, chain identity, signing support and the
runtime metadata the validator run loop uses (its weight calls and the
validator-read storage, constants and selective metagraph API), then closes
before loading a wallet. It fails only where the run loop would fail or report
itself not ready. Other drift, such as a changed metagraph projection that the
dynamic decode absorbs or a breaking change in an unrelated call, pallet or
module error, is logged as a warning. It supports
`--network finney --chain-endpoint wss://your-node.example` and the ordinary TLS
root configuration. It is useful for verifying container certificates before
providing the hotkey mount.

For protocol tests, `rtao-validator --synthetic-weights` assigns equal
scores to registered miners other than self and owner. It needs eligible miner
destinations. Without this flag, the demonstration binary holds weights until
an application supplies scores. Use your subnet's scoring implementation for
normal operation.

The hosted client checks exercise scheduling, signed requests, state recovery,
runtime refusals and transport behavior with committed fixtures and fake
transports. Use the exact commit's CI result when assessing those checks.
Live-chain commit/reveal timing, real endpoint reachability and host shutdown
behavior need separate operational evidence. Localnet and Python
interoperability checks are optional targeted hosted work; they are not
required local campaigns. No twenty-four-hour testnet soak is required, and no
passing soak or deployment result is claimed.
