# M2-11 weight math and epoch preparation

The pure weight helpers now port `normalize`, `clip_to_max_weight` and
`_conform` from pinned Python v478. Quantization uses ties-to-even rounding,
retains Python's division/multiplication order and drops zero quantized values.
Raw-u16 mode preserves integer ratios, checks the share limit with widened
integer arithmetic and retains the single self-weight exception. Minimum
weight count is checked after quantization with Python's message and semantic
code. Typed UIDs enforce the chain's u16 range.

Float sums use CPython 3.12+'s Neumaier compensation. Cumulative sums in the
clipping calculation remain a left fold, matching `itertools.accumulate`.
The CI oracle must record its exact Python version and compare these helpers
bit for bit; those vectors are still pending. No local Python was run.

The staged `epoch_schedule` uses the pallet's `counter > tempo` predicate
and `tempo + 1` counter cap. Its tests cover all four upstream v478 schedule
vectors plus the scheduled skip, owner-triggered skip, lowered tempo and
overflow cases. `timelock_commit` takes Unix seconds explicitly and uses the
core's `WeightsTlockPayload`, SCALE encoding and encryption. Its tests pin the
security offset, round calculation, payload fields and invalid clock inputs.
The asynchronous `TimelockCommitter` and ciphertext-size checks are integrated;
their separate evidence is in `m2-weights-preparation.md`.

There is a further phase detail within D-006. At head 105, a subnet can have
`LastEpochBlock = 90`, tempo 20 and a step counter of 20 after an earlier
owner-triggered skipped slot. At inclusion 106, reveal processing sees the
old counter; coinbase then increments it to 21 and consumes an epoch before
the commit extrinsic runs. The commit must therefore use the post-coinbase
epoch. With a reveal period of one and subsequent successful epochs, its
first reveal block is 126. The pinned core returns 110. Changing only its
predicate leaves a second error, predicting 107. The staged predictor derives
the commit epoch from the post-coinbase state and returns 126.

This model assumes successful future epochs. It cannot predict future input
inconsistency or per-block-cap deferrals. The validator's reveal watcher still
needs to follow the actual included commit's assigned epoch.

Validation under `target/execute-plan.6Ro0PC`:

- Six weight math tests pass on current Rust and Rust 1.89. Strict all-target,
  all-feature intent Clippy passed before the separately owned committer joined.
- Nine schedule/payload tests pass on current Rust and Rust 1.89.
- Strict core-extension all-target Clippy passes after receipt integration.
  All 59 core-extension tests passed together; earlier compile/lint failures
  remain historical logs.
- An isolated no-host upstream vector probe ran both committed safety tests
  against v478. Both fail as expected, including `Ok(110)` versus `Ok(126)`.
  This establishes that the deletion probe detects the current core defects.

Logs are `tmp/root-weights-{current,msrv}.log`,
`tmp/root-weights-clippy.log`, `tmp/root-epochs-{current,msrv}.log`,
`tmp/root-epochs-clippy.log` and `tmp/root-epoch-upstream-proof.log` under
the recorded task root. No M2 checkbox or milestone exit is claimed.

The bounded upstream failure transcript is retained in
`docs/execution/m2-epoch-upstream-proof.txt`. The isolated proof directory
and downloaded reference/body files were checked for running processes, open
files, launchd/configuration references, symlinks and virtual environments,
then removed by exact paths. Shared build, source and fuzz caches remain live
for the ongoing goal. The open-file audit warned about an unrelated backup
SMB mount; the proof directory is on the local workspace filesystem and had
no open files. No backup path was changed.

The set, commit and legacy reveal intent bodies now use these helpers. Six
FakeChain behavior tests pass on current Rust and Rust 1.89: mechanism-index
storage and coalesced pinned reads, Null-permit refusal before crypto,
raw-u16/Yuma gating, registration/rate diagnostics, adaptive/forced commit
rounds and extras, and legacy normalization without unrelated preflight reads.
Two generated registry tests also pass. Strict all-target, all-feature intent
Clippy passes. Logs are `tmp/root-m2-intents-{current,msrv,clippy}.log`.

The staged predictor also checks simulation budget and search-end arithmetic
before iterating. Its overflow regression and six other unit tests pass on
current Rust and Rust 1.89 (`tmp/root-epoch-budget-tests.log` and
`tmp/root-epoch-budget-msrv.log`).
Upstream U5 is filed as [subtensor#3248](https://github.com/RaoFoundation/subtensor/pull/3248)
with the same overflow guard. Hosted oracle equality and actual accepted
commits/reveals remain required.


Final standalone-intent parity verification adds the exact Null storage-read
set regression. Both current Rust and Rust 1.89 pass all sixteen weight tests
(seven behavior tests, six math tests and three checked-committer tests), and
strict all-target, all-feature Clippy passes. Standalone intents share the
injected-clock encryption implementation and retain Python build-time reads;
the validator scheduler uses `TimelockCommitter` for its explicit consensus
ciphertext bounds. No oracle touch exemption was introduced. Final logs are
`tmp/root-m2-weight-read-set-{current,msrv,clippy}.log`.
