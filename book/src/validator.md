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
ready. A row containing positive weights only for self or owner is held,
unless the proposal opts in with `OwnerRowPolicy::Submit`
(`WeightScheduler::on_finalized_with`, `enqueue_with_owner_rows` or
`Validator::owner_row_policy`). That submits a row whose positive weight goes
only to the subnet owner hotkey's UID (a burn row), including through a
timelocked commit and its automatic reveal. A row with positive weight on the
validator's own UID still holds, unless the validator is the owner hotkey.
Durable work journals the policy, so a restarted or recovered operation decides
the same way. A callback row the guard holds is not journaled; check
`OwnerRowPolicy::holds` before queuing a row directly, because a held queued
row stays pending. The row still goes through Python-parity conformance first:
a non-owner validator's burn row needs an effective `MinAllowedWeights` of 1
(zeros do not count), and with a stored `MaxWeightsLimit` below 65535 it should
list only the owner UID, since clipping a row that lists zeros spreads it over
every listed UID. Every
mechanism has a separate cadence and rate-limit slot. The scheduler reads its
preflight at one best head, while finalized heads trigger attempts and anchor
transaction mortality. Commit-reveal off uses direct mechanism weights;
commit-reveal on uses timelocked mechanism weights with the version 4 payload.
The commit is always tagged with the chain's `CommitRevealWeightsVersion`: a
newer runtime never stops commits by itself. What is known about the live code
is reported, not required. `rtao_weights_cr_version_reviewed{mech="N"}` carries
the spec version when the code is on this build's reviewed list (0 otherwise),
and `rtao_weights_cr_version_unreviewed{mech="N"}` is 1 while the code is
neither listed nor proven by one of this validator's reveals, or its identity
could not be read; each change logs once. An unexplained missed reveal of a
commit with a non-native tag counts `rtao_weights_cr_version_refuted_total`,
warns and sets `rtao_weights_cr_version_refuted{mech="N"}` to 1; commits
continue with the chain's tag at most once per subnet epoch until a reveal
proves it again. `with_unreviewed_cr_tags(Unreviewed::Pause)` opts into the
behavior of earlier releases (pause under unreviewed, refuted or unidentified
code), which ties commits to this build's list. The decision itself is
`rtao_core::commit_reveal::select_tag`, which also builds for wasm32.
A journaled scheduler built with `with_bounded_dispatch(n)` returns from a
dispatch after the admission window plus `n` blocks and leaves an unresolved
transaction to the recovery of later heads, whose per-call cost
`with_recovery_budget` bounds.
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

On a paced public endpoint, keep the reads per finalized block small. An
application that drives `WeightScheduler::on_finalized` from its own loop
should follow `client.blocks_with(Finality::Finalized, Backfill::Latest)`:
after a long call it receives the newest finalized head at once, instead of
after two requests for every height it missed. The reveal watcher follows
finalized history from its own cursor and reads only the blocks that can change
a pending commitment's outcome: blocks where an epoch is due and, while a
reveal window is open, the block where the commitment leaves its queue, plus
one state read per head. `MetagraphSync` with an epoch or block-count cadence
makes no request until a refresh is due.

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
on an attempt the installed semantic guard checks: durable work queued while
that guard's contract was installed. Without a journal, in the bounded
retries, and for work queued without that contract, they refuse. A refusal
pauses that attempt with `PausedSurface` and signs nothing. When the runtime
the executor signs with differs from the best-head runtime the call was
composed for, the encoder is called again for the signing runtime and its
bytes are checked again. Preflight, executor policy, the semantic guard, the
journal, shielding and the bounded `IncorrectCommitRevealVersion` retries
apply as for compiled calls. Each durable attempt the encoder composes
records its `contract()` with that attempt, not with the operation, so
installing an encoder never changes how an operation ID is redelivered. After
a restart, work with a signed attempt the encoder composed stays unresolved
until an encoder with the same contract is installed again; other work
continues with the composers installed now.
`rtao-validator --check-connection` reports a changed call as a warning
naming the composer the scheduler will use, the encoder first when one is
installed. It fails only when no composer can produce the call.

Storage the validator reads behaves the same way after a runtime change. An
entry whose shape changed but whose values are still recoverable (a widened
integer, a newtype, `BoundedVec`, a tuple that became a struct, reordered or
added fields) is read by name against the live metadata into the same values,
and the preflight is still one storage request. An entry whose meaning
changed fails only the reads that need it, with an error naming the entry and
field. `--check-connection` reports the first kind as a warning and the
second as a failure.

Each weight transaction has an admission window: signer resolution, the
nonce lease, preparation, signing and the pool's acknowledgment, which come
after planning. It is 30 seconds by default and about ten requests long. An
endpoint paced to a few requests a minute needs more; set it with
`WeightScheduler::with_submit_deadline` or `Validator::weight_submit_deadline`.
An attempt that runs out of the window fails before anything is signed or
broadcast. The wait for inclusion is a whole 128-block era after the window,
so a longer window does not shorten it. A long window also lets a commit
prepared in an epoch's last blocks land in the next epoch; the reveal watcher
follows the epoch it landed in and counts
`rtao_weights_commit_epoch_slip_total`.

The default timelock clock is the system clock. The minimum timestamp offset
across twenty best heads filters propagation delays. The framework warns above
two seconds of absolute skew and refuses system-clock commits above twelve
seconds. Synchronize the host clock; embedded callers can use
`NeuronRuntime::with_clock(Arc::new(ChainClock::unanchored()))`. The runtime
refreshes this clock with verified best-head timestamps and waits for its first
anchor before preparing a commit. An application that drives
`WeightScheduler::on_finalized` from its own loop, without the runtime's clock
worker, needs no anchor of its own: before each timelocked commit the
scheduler reads `Timestamp.Now` at the best block the commit's preflight is
pinned to, anchors an unanchored or lagging `ChainClock` to it as of the
moment it pinned that block, and records the same reading in the clock guard
that a system clock needs. It never moves a clock back, so the worker's
fresher anchor is kept.

A failed weight dispatch that provably signed nothing, such as an admission
timeout while reads queue on a paced endpoint, gives its cadence slot back:
the same epoch can still commit. The next attempt waits one block after the
first such failure, doubling to 32 blocks while they repeat. A failure after
the hotkey was asked to sign keeps the slot, and journaled work reconciles
through its journal before dispatching again.

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
dynamic decode absorbs, a storage entry or constant the validator reads by
name after its shape changed, or a breaking change in an unrelated call,
pallet or module error, is logged as a warning. It supports
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
