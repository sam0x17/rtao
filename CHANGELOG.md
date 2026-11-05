# Changelog

## Unreleased

### Automatic runtime adoption (repository maintenance)

- New hourly `runtime-adopt` workflow (`ci/runtime-adopt.sh`, decision checks in `ci/runtime-adopt-test.sh`). When Finney finalizes a newer spec it pins it (the final `vN` release, else upstream main's release-N merge), imports, extracts, regenerates, reconciles and checks it in a disposable candidate, and if every stage passes pushes the commit to `main` with a changelog entry, an upstream-log entry and a `snapshot-<date>` tag. A testnet runtime missing from its index is recorded with `drift --network testnet --apply`.
- A red stage or a case only a maintainer can decide leaves `main` unchanged, opens or refreshes one issue and, when the `RUNTIME_ALERT_URL` and `RUNTIME_ALERT_TOKEN` repository secrets are set, posts an alert. Upstream branches that declare a spec newer than any network runs keep one informational issue.
- The job that compiles and runs code has a read-only token; the validated commit leaves it as a git bundle, and a separate job that builds nothing pushes main and the tag atomically. A prerelease of the new spec is reported as blocked (pin accepts neither the prerelease nor an unreleased merge while it exists). A run that cannot observe a network does not close an open adoption issue.
- Adoption approves itself on evidence (D-116). A new runtime is acted on only when a second public endpoint reports the same runtime at the same finalized block. After the pin, at least two endpoints (the default list spans two operators; operator diversity is not required, so one operator's outage does not stop adoption) must serve the pinned block with the pinned spec, `:code` hash, BLAKE2b-256 of the `:code` bytes (computed by the workflow) and V15/V16 metadata digests, and a published release wasm must hash to the same `:code`. The built CLI then reads a constant, `Timestamp.Now` and subnet hyperparameters from Finney.
- `cargo xtask parity report --adopt`: unchanged sources carry their reconciliation to the new tag, cosmetic changes auto-bless (the absent Q30 oracle is reported unverified, and extraction is compared without source tags, commits and blobs), generated, extracted and vendored sources are re-derived and checked in the same run, and other changed hand ports are listed in the new `parity/port_due.toml` instead of failing. Ordinary parity accepts exactly those listed changes; strict parity names them as due; `parity bless` removes an entry. One informational issue lists them, without an alert.
- A failed adoption backs off (1 h doubling per failed attempt, measured from when the attempt failed, at most 24 h; the open issue records the state, which another network's report, an unreachable network or a failed decide step does not reset) instead of rerunning hourly, retries at once when main changes the adoption inputs, and alerts once per failure. `workflow_dispatch` gains `force`.
- Witnesses and evidence endpoints must hold the block on their own finalized chain, testnet records are witnessed by testnet's second public name, and an adoption commit that changes the tooling that validated it (scripts, xtask, workflows, manifests, the lock, the toolchain) is refused before it is bundled and again before it is pushed.
- Fixed: GitHub refused to parse the workflow, so neither the hourly schedule nor a dispatch ever ran it. The adopt job's job-level `env` read the `runner` context, which is not available there; those paths are now set by the job's first step.
- No library change: the SDK's run-time compatibility checks and pin provenance are unchanged.

### Owner-row guard and weight refusals at the dispatch block (D-098)

#### rtao-neuron

- Fixed: the self/owner weight-row guard took the owner hotkey's UID from the last-good metagraph. If the owner re-registered at another UID after that graph was taken (for example, by an application that keeps one snapshot and stops the refresh worker), a queued row for the miner now at the old UID was held on every head, and its operation stayed pending with no reason. Under `Hold`, a row for the owner's new UID was signed as an ordinary row. The guard now reads the owner's `Uids` entry at the block the attempt is prepared at, alongside this validator's UID and `SubnetOwnerHotkey`. The read happens only when exactly one other UID carries positive weight, so ordinary rows cost no extra request.
- Queued (journaled) work that the guard holds now reports `WeightAttemptOutcome::Refused` named `OwnerRowHeld` (`policy_violation`) instead of `HoldPrevious`. The refusal names the operation, the block and the held destinations. The work stays pending and is decided again at every head. A held callback row still reports `HoldPrevious`.
- New: `WeightScheduler::retire_pending(mechanism, id)` retires a pending operation that never reached the chain: it has no attempts, or every attempt is `ExpiredUnexecuted`. It is marked `scheduler_retired` (and handed off, so earlier releases skip it too). Its ID stays idempotent, so a replacement needs a new ID.
- `IncorrectWeightVersionKey` (`invalid_argument`, unchanged) now reports the observed `WeightsVersionKey`, its block and the version the attempt carries. For queued work it also says the version is part of the immutable effects and points to `retire_pending`. The check still runs before signing on every path.
- No journal or checkpoint schema change.

### rtao-neuron

#### Added

- `weights::OwnerRowPolicy` (`Hold`, the default, and `Submit`; `#[non_exhaustive]`) with the public predicate `OwnerRowPolicy::holds` (D-093).
- `WeightScheduler::on_finalized_with`: the callback returns a proposal and its owner-row policy.
- `WeightScheduler::enqueue_with_owner_rows`: queues durable work with a policy.
- `Validator::owner_row_policy`: a provided method that returns `Hold` unless overridden.
- Under `Submit`, an owner burn row goes through the normal path: a direct `set_weights` row, or a timelocked commit with its automatic reveal. In an owner burn row, every positive entry goes to the subnet owner hotkey's UID.
- These rows still hold under `Submit`:
  - a row with positive weight on the validator's own UID when that UID is not the owner's;
  - a row with no positive entry.

#### Journal

- Work queued under `Submit` records `"owner_rows":"submit"` in its effects. Default work keeps its exact bytes, and effects without the key load as `Hold`.
- Restart, recovery and the re-preparation after a rejected commitment use the recorded policy. Changing the policy for an existing operation ID is refused, the same as changed input.
- An earlier release cannot read effects that carry the key, and it refuses every mechanism of that scope. Let `Submit` work finish before rolling back.

#### Fixed

- A durable callback proposal that the self/owner guard holds is no longer journaled. Before, it was proposed again from the journal without the callback, and the mechanism never signed again.

#### Metrics

- New: `rtao_weights_owner_row_total`.
- The re-preparation after a rejected commitment now counts `rtao_weights_hold_total` when it holds.

#### Unchanged

- With the default policy, every existing caller holds exactly as before.

### Validators on a paced endpoint (D-094 to D-097)

An application that drives `WeightScheduler::on_finalized` from its own loop,
without `NeuronRuntime`, against an endpoint paced to a few requests a minute.

#### rtao-neuron

- The scheduler anchors its own timelock clock (D-094). Before each timelocked commit it reads `Timestamp.Now` at the block the commit's preflight is pinned to and observes it on its clock and `ClockGuard`. A `ChainClock::unanchored()` clock no longer needs the runtime's clock worker, and a `SystemClock` gets its skew sample. The clock only moves forward. A failed read warns and counts `rtao_clock_sync_errors_total`; an unanchored clock still refuses the commit. Cost: one request per timelocked commit, none on other heads.
- New: `Clock::observe_timestamp_at` (provided; defaults to `observe_timestamp`), `ChainClock::observe_at`, `ChainClock::is_anchored`.
- A weight dispatch that provably signed nothing, such as an admission timeout, gives its cadence slot back (D-095). The same epoch may commit again after 1 block, then 2, doubling to 32 while such failures repeat. A failure after the hotkey was asked to sign, or with a new signed journal attempt, keeps the slot. Such an attempt reports `WeightAttemptOutcome::Failed`, not `Unresolved`. A journaled release stays uncertain until recovery reports the work rebuildable.
- The checkpoint's per-mechanism slot may carry `unsigned_retry`. Earlier releases ignore it.
- `WeightScheduler::with_submit_deadline` and the provided `Validator::weight_submit_deadline` configure each weight transaction's admission window; the default is `DEFAULT_WEIGHT_SUBMIT_DEADLINE` (30 s) (D-097). The overall bound is the window plus a 128-block era. New metric: `rtao_weights_commit_epoch_slip_total`, for a commit that landed in the epoch after the one it was prepared for.
- `RevealWatcher` reads in full only the blocks that can change a pending reveal outcome (epoch-due blocks, a window's first block, and the first changed block found by bisection) and skips spans whose end state matches its prediction (D-096).
- The runtime's scheduler and coalescing loops follow the newest finalized head (`Backfill::Latest`); the reveal watcher and recovery follow history from their own cursors.

#### rtao-client and rtao

- `Backfill` (`Complete`, the default; `Bounded(n)`; `Latest`; `#[non_exhaustive]`) and `HeadTracker::blocks_with`, `Client::blocks_with` and the blocking facade's `blocks_with` (D-096). `blocks` is unchanged and equals `Complete`. `Latest` never makes a request for a skipped height.
- The shared tracker recovers finalized gaps up to `SHARED_FINALIZED_BACKFILL` (16) once for every follower. After a longer gap it publishes the newest head promptly, and each `Complete` follower recovers the gap itself.
- Preparation and attachment read each value once: the runtime handle and the signing stamp come from one `state_getRuntimeVersion` read (D-097). From the nonce lease to the author call an ordinary submission makes 10 requests instead of 15 or 16.

#### rtao-metagraph

- `MetagraphSync` decides from the finalized height and its cached epoch schedule before making any request, so heads between refreshes cost nothing (D-096). An unpredicted epoch is refreshed at the predicted block or the fallback, whichever comes first.

#### rtao-rpc

- An HTTP 403/429 refusal of a reconnect handshake, after the session has connected, no longer ends the session (D-097). The session stays `Reconnecting`, waits the longer of the refusal's `Retry-After` (at most 10 minutes) and a backoff of 1 s doubling to 60 s per consecutive refusal, and dials again. Without `retry_forever` it becomes terminal after `max_retries` further refusals. A refused initial connect, a JSON-RPC policy error and a policy close (1008) stay terminal. New metric: `rtao_rpc_reconnect_refusals_total`.

### Portable commit-reveal tags, bounded `on_finalized` and commitments (D-110 to D-115)

#### rtao-core

- New module `commit_reveal` (D-110): `NATIVE_PAYLOAD_VERSION`, the reviewed-runtime list (`REVIEWED_RUNTIME_CODE`, `REVIEWED_RUNTIMES`, `reviewed_runtime`), the tag decision `select_tag` (`TagDecision`, `TagBasis`, `TagPause`, `RevealEvidence`, `Unreviewed`) and `TagEvidence<N>`, a fixed-array evidence store. No I/O and no allocation; it builds for `wasm32-unknown-unknown`.

#### rtao-neuron

- The scheduler's commit-reveal tag comes from `rtao_core::commit_reveal::select_tag` (D-110). `NATIVE_CR_PAYLOAD_VERSION`, `REVIEWED_CR_RUNTIME_CODE` and `reviewed_cr_runtime` are now the core's values.
- Changed: a changed `CommitRevealWeightsVersion` under runtime code that is neither reviewed nor proven by a reveal is now followed, not paused. The scheduler warns once and sets `rtao_weights_cr_version_unreviewed{mech}` to 1. An unexplained miss of a commitment that carried the tag under that code still pauses timelocked commits. `WeightScheduler::with_unreviewed_cr_tags(Unreviewed::Pause)` restores the earlier pause. New: `WeightScheduler::commit_reveal_decision`, `RevealWatcher::reveal_evidence`.
- The chain-clock reads of `Timestamp.Now`, the scheduler's anchor read and `NeuronRuntime`'s clock worker, fall back to a by-name read when the compiled shape no longer matches (D-111).
- `RevealWatcher` verifies each pending commitment's queue across skipped spans before its window opens too, and reads its inclusion block in full (D-112). A reveal period that drops (to 0, say) and returns between two read blocks, and reveals or expires a commitment early, is now seen at the block where it happened. A period change while a commitment is pending counts `rtao_weights_reveal_period_changed_total`, and that commitment's miss never refutes a followed tag. The queue read shares the schedule's storage request; on the request-budget fixture the live path makes 31 requests instead of 30 and a restart replay 16 instead of 12.
- Bounded `on_finalized` (D-113): `WeightScheduler::with_bounded_dispatch(wait_blocks)` makes a journaled dispatch wait at most the admission window plus `wait_blocks` blocks; an attempt still unresolved stays journaled and uncertain, and the recovery at the start of later calls resolves it, instead of one call scanning inline until the era ends. `WeightScheduler::with_recovery_budget(transaction_blocks, reveal_blocks)` sets the per-call bounds of that recovery (defaults `DEFAULT_RECOVERY_BLOCKS` = 128 and `REVEAL_BLOCK_READS` = 256); both scans resume from their cursors. New: `RevealWatcher::set_read_budget`, `RevealWatcher::read_budget`.

#### Checkpoints

- A pending commitment whose reveal period changed records `"period_changed":true`. An earlier release cannot read a checkpoint that carries it; let such commitments settle before rolling back. Other checkpoints keep their bytes.

#### rtao-client

- `Fetch` is implemented for `Vec<F>`: a homogeneous list of typed queries coalesced with the rest of a plan.

#### rtao-intents and rtao

- New feature `commitments` with `commitments::SetCommitment`, a typed `Commitments.set_commitment` signed by the hotkey (D-114). It checks raw field lengths, refuses a failed-reveal field and blocks more fields than `Commitments.MaxFields`. It is an rtao addition, outside the generated Python catalog and `AnyIntent`. `all` and the facade's `validator` and `miner` features enable it.

#### Documentation

- The README and the book list rtao's workspace `[patch]` tables and when an application copies each: `w3f-bls` is required to build, and the `bittensor-core` fork patch with its companion table is recommended because it carries the Ed25519 erasure repair (D-115).

### Also in this release, since the last widely pinned revision

- Validator storage reads fall back to name resolution against live metadata after a runtime change (D-089). Ratio and fixed-point types are refused rather than guessed.
- Weight calls can be composed by name or through an application encoder.
- A fresh nonce waits for a pending shielded carrier from the same account instead of taking its inner nonce (D-084), and the shield era anchor stays fresh across that wait.
- Journaled MEV-shield dispatch is supported from per-runtime evidence.
- Commit-reveal follows a changed version under known runtime code, and the evidence is bound to the stored tag.
- `stake add` stakes to a named wallet hotkey without the target picker.
- `rtao-arena` 0.2.0: a fixed-capacity allocator whose heaps take their region lazily on any target (D-090 to D-092).

### Migrating from `hold_previous`

- Nothing changes unless you opt in.
- To commit an owner burn row, pick one:
  - pass `OwnerRowPolicy::Submit` with the proposal through `on_finalized_with`;
  - queue durable work with `enqueue_with_owner_rows(.., OwnerRowPolicy::Submit)` and keep driving `on_finalized`;
  - override `Validator::owner_row_policy`.
- A burn row from a validator that is not the owner needs an effective `MinAllowedWeights` of 1.
- Under a stored `MaxWeightsLimit` below 65535, list only the owner UID. Zero entries would otherwise be spread over by the clipping.
- An application that mirrors the old guard can call `OwnerRowPolicy::holds` with the same policy, so its check agrees with the scheduler's.
