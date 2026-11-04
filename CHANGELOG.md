# Changelog

## Unreleased

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
