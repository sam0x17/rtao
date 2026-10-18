# Weights and timelock preparation

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

The pure weights helper uses the pinned Python cutoff search, ties-to-even
quantization, raw integer ratio checks, minimum nonzero count and single
self-weight exception. The CPython 3.12 oracle uses compensated `sum(float)`;
the sorted cutoff scan's `itertools.accumulate` remains ordinary ordered
addition. Six math tests pass on current Rust and Rust 1.89. Oracle vector
comparison remains unverified; a future optional refresh belongs in a bounded
hosted job before claiming independent Python equality.

`TimelockCommitter` accepts a named `TimelockCommitRequest` containing an
explicit Unix timestamp, public hotkey, conformed row, version key, coherent
best-head schedule, reveal period and chain slot duration. It runs schedule
prediction and upstream encryption together on the blocking pool. The wrapper
applies Yuma's 5,000-byte ciphertext limit or Null's floored 32 KiB allowance per
mechanism, rejects zero mechanisms, and reports oversized ciphertexts before
submission. Three wrapper tests and both Criterion smoke cases pass on current
Rust and Rust 1.89; strict all-target intent Clippy passes.

The staged schedule applies the pallet's tempo safety net and counter cap. Its
post-coinbase inclusion calculation covers a further boundary: an age equal to
tempo can fire after the increment in the inclusion block, before the commit
extrinsic. Unit vectors pin that case and the ordinary schedule. Eight
integration tests include every private upstream vector directly from the
immutable vendored Rust source, ordinary schedules over multiple tempos,
scheduled and owner-triggered skips, lowered tempo, deferral and overflow.
Four round/payload tests cover explicit clocks, fractional slot durations,
floor boundaries, invalid time inputs, upstream SCALE framing and randomized
ciphertexts. These twelve integration tests pass on current Rust and Rust 1.89;
strict Clippy passes for the staged library and these targets.

The five-harness Criterion suite adds `weights/conform_256` and
`weights/conform_2500`, for 36 measurements. Its
[development baseline](../../vendor/bench/baselines/m2-aarch64-apple-darwin.json)
records the actual compiler, CPU, source snapshot, features and sample settings.
The original 34-path M1 baseline remains a historical record. A nightly reference
revision must contain all current harnesses; missing targets fail explicitly.
The measured conform means were 1.17 microseconds for 256 entries and 6.78
microseconds for 2,500. A same-source repeat changed them by +0.67% and +3.11%.
That repeat failed the actual 30% gate on two ed25519 measurements while other
builds ran on the shared Mac. A fresh comparison is optional targeted hosted
work; that historical failure does not require a local replacement campaign.

The scheduler still supplies ClockGuard, fresh schedule reads and the one allowed
`InvalidRevealRound` recomputation. This helper performs no submission or retry.
Independent oracle vectors, localnet acceptance and observed reveals remain
unverified optional hosted evidence under Q30. These historical local checks
do not complete M2; its unaccepted S02 security disposition remains open.
