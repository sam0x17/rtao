# Changelog

## Unreleased

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
