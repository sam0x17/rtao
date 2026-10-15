# Independent audit of subtensor PR #3248

The initial dynamic runs below are provisional: another audit lane exposed
shared-target artifact reuse across equivalent local package identities.
Root recreated this exact checkout and assigned a unique no-op workspace
compiler wrapper. The final isolated checks now pass on both compilers, with
fresh compilation and exact source/artifact identities verified. The initial
unisolated runs are retained as history, rather than final proof.

The submitted epoch fix is supported by an independent source review and
failing-before/passing-after Rust probes. The real v478 predictor returns block
110 for the reported inclusion-counter case; the submitted predictor and an
independent trace of the pallet's storage transitions return block 126. Changing
only the counter predicate returns 107, confirming that the inclusion-phase
change is necessary. No production defect or inaccurate submitted claim has
been found in this audit.

The audited PR is [RaoFoundation/subtensor#3248](https://github.com/RaoFoundation/subtensor/pull/3248),
at `9b4d8c51aa7d9fb11be6f0e4cef500e02e0fad36`, based on v478 commit
`850f2755bb05a3851f948489f0ab27b0645cebce`. This is an audit of that exact
submitted source, rather than a claim that current upstream main or hosted CI
has passed. The disposable checkout is recorded under
`target/execute-plan.6Ro0PC/upstream-quality/pr-3248`. Its original tracked
source and lockfile remained unchanged during verification; audit-only files
were untracked.
Those audit files and the checkout were removed after their source, fixtures
and results were retained below; no branch or published PR was changed.
Root then recreated the exact checkout for the isolated rerun. After root
accepted its final identities and results, the recreated checkout and its
unique wrapper were audited and removed. The shared bare repository, build
cache and other owners' sources/wrappers remain live.

## Source checks

The pallet's `block_step` calls `reveal_crv3_commits` before `run_coinbase`.
The hook precedes commit extrinsic dispatch. `drain_pending` increments the
stored step counter, caps it at `tempo + 1`, then tests whether an epoch is
due. Its safety net compares the counter with the actual subnet tempo.
`do_commit_timelocked_weights` assigns the queue key with
`current_epoch_with_lookahead` after those hook updates. A successful epoch
resets the counter; an inconsistent input consumes the epoch slot without
resetting it. The per-block cap can defer a due slot without consuming it.
These distinctions agree with the PR body and justify its stated limitations.
The exact sources are the [initialization hook](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/macros/hooks.rs#L18),
[reveal-before-coinbase order](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/coinbase/block_step.rs#L17),
[bounded counter and consumed slot](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/coinbase/run_coinbase.rs#L404),
and [commit queue epoch assignment](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/subnets/weights.rs#L426).

The reported head is reachable: a successful slot at 85 followed by an
inconsistent owner-scheduled slot at block 90 leaves counter 5 and increments
the epoch index. Owner triggering requires commit-reveal disabled; it can be
enabled again before the commit, and its enable setter preserves the epoch
fields. Fifteen subsequent blocks
leave counter 20 at head 105. Reveal processing at inclusion block 106 sees
that counter before its increment. Coinbase then increments it to 21, runs
the epoch and resets the counter before assigning the new commit. Period one
therefore targets the following epoch's first reveal-processing block, 126.
The audit uses epoch index one at the head to retain the already-consumed
skipped slot; changing that starting index does not change the reveal block.
The replay probe seeds a synthetic pending-trigger snapshot directly, rather
than dispatching the owner trigger. It exercises that pending transition even
with independently chosen initial anchor/counter fields. The reachable owner
setup above is established from the pallet source; no owner-dispatch runtime
test is claimed.
The [owner trigger guard](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/coinbase/tempo_control.rs#L87)
and [commit-reveal enable setter](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/utils/misc.rs#L611)
establish that qualification.

`apply_tempo_with_cycle_reset` changes tempo and `LastEpochBlock` while
retaining the counter, so reducing tempo below a retained counter is also
reachable. The chain permits reveal periods 1 through 100. Period zero is
outside that native range. Root can set a tempo above the owner maximum;
the SDK retains its existing owner-tempo-based simulation budget and may
return `BoundExceeded` for such a large native tempo. The PR does not promise
to remove that existing budget limitation.

## Independent proof

The audit compiles byte-for-byte base scheduling/vector files alongside the
submitted core. It separately implements the pallet storage transition and
hook order, including the skipped-input and deferral branches, rather than
using the submitted predictor to generate expected results.

Across 20,568 combinations of tempo 1–12, scheduler age, retained counter,
pending triggers and reveal periods 1–3, the submitted predictor agrees with
the successful-epoch pallet trace in every case. The original predictor
disagrees in 1,507 cases. The original three prediction vectors and original
commit-epoch vector also pass without changing their expected outputs.

The explicit base regression is run separately and must fail with exit 101:

```text
failing_before_regression ... FAILED
assertion `left == right` failed
  left: Ok(110)
 right: Ok(126)
```

For the actual `generate_commit_v2_at` API, the audit injects timestamps that
produce rounds 245 and 246. It decrypts both returned ciphertexts with those
rounds' real Quicknet signatures and compares every plaintext byte with the
canonical core SCALE payload. It then decodes and re-encodes all four payload
fields. Invalid signature bytes fail. This extends the submitted tests'
parseable-ciphertext check to an actual cryptographic round trip. The
encryption implementation itself is unchanged by the PR.

These are audit-only checks, distinct from the seven submitted epoch tests and
four submitted clock/payload/API tests. They exercise core Rust and a
source-backed pallet model. They do not execute a full live runtime, simulate
all subnets sharing the scheduler cap, or operate a validator against a chain.
Future tempo updates, inconsistent future epochs, repeated scheduler deferrals,
delayed inclusion and drand ingestion delays remain outside the predictor's
guarantee. The submitted body correctly tells callers to follow the actual
epoch assigned to the included commit.

## Reproduction identity

All Cargo commands explicitly set `PYO3_NO_PYTHON=1`, use the recorded shared
build directory and task temporary directory, and pass `--locked --offline`.
No Python process or container was invoked. Rust 1.89.0 targeted checks passed;
the checkout's unqualified Cargo also resolves to Rust 1.89.0, so current-Rust
validation uses explicit `+stable`, Rust 1.99.0.

| Artifact | SHA-256 |
| --- | --- |
| Cargo.lock, before audit | `c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a` |
| Submitted epoch_schedule.rs | `3dc094449433717d36228b727dbe79bcfccf09cf3dd711148ebda796d8128e74` |
| Submitted timelock/mod.rs | `5dce06b114efd42c3ba550840ec47b0afd1358e928de18dc5e5f58ba042c8b9a` |
| Submitted epoch_schedule_vectors.rs | `ecc035b52291ca54f3b63c7480fa171cba1e9e6b028aa35b6c06b8afc2e6e54d` |
| Exact base epoch_schedule.rs | `cf34e79dbe3574b868a475999c9d77fcdf3e0f0521a9909e5fb56902aff277f7` |
| Exact base epoch_schedule_vectors.rs | `7aa89dd42ecd825758a880af651dfe52125f7ffd24eaa501c3b5709a692a2715` |
| Audit-only replay source | `76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50` |
| Quicknet round 245 response | `dc22720c801389472aa4e223d1b21981d758f514f2f78d354aa4136bbb6e7692` |
| Quicknet round 246 response | `a6d4a190a09bac5be6ff816c9f6d48a44c0009d25fecf45c9ba6953f04e1c808` |

The command environment and replay source below are retained to make the
independent test results reviewable after the disposable checkout is removed.
The base fixtures are extracted with `git show` from the exact base commit.
The deliberate counter-only fixture is that base source with only its counter
comparison changed; its original `cfg(test)` modules are omitted to avoid
cross-module type collisions. The two JSON fixtures are public responses from
the Quicknet chain hash
`52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971`.

```sh
export RTAO_TEMP_ROOT=~/workspace/rtao/target/execute-plan.6Ro0PC
export CARGO_TARGET_DIR="$RTAO_TEMP_ROOT/build"
export TMPDIR="$RTAO_TEMP_ROOT/tmp"
export PYO3_NO_PYTHON=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features \
  --test quality_epoch_audit --test epoch_safety --test explicit_commit_clock -- --nocapture
cargo +stable test --locked --offline -p bittensor-core --no-default-features \
  --test quality_epoch_audit --test epoch_safety --test explicit_commit_clock -- --nocapture
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features \
  --test quality_epoch_audit failing_before_regression -- --ignored --exact --nocapture
# The last command intentionally exits 101, and must show 110 versus 126.
```

Initial provisional results on 2026-10-14:

| Check | Observed result |
| --- | --- |
| Rust 1.89.0 submitted tests | 7 epoch tests + 4 clock/payload/API tests pass |
| Rust 1.89.0 independent audit | 11 pass; the deliberate base-failure test is separately selected |
| Rust 1.99.0 submitted and independent tests | The same 7 + 4 + 11 pass |
| Explicit original-base regression | Fails with exit 101 and exactly `Ok(110)` versus `Ok(126)` |
| Rust 1.89.0 all-feature core library | 71 pass; one physical OpenCL GPU test remains ignored |
| Original golden schedule suite | Original three prediction and one commit-epoch vectors pass; submitted added inclusion vector also passes |
| Lock and submitted source after all runs | Unchanged; same recorded lock SHA-256 |

This audit does not rerun the earlier core/binding Clippy and wasm checks;
the submitted-head pre-push evidence remains historical in `pr-u5-pre-push.log`.
Those checks are not included in the final isolated proof below.
No full upstream monorepo preflight or hosted workflow result is inferred from
the scoped Rust checks. The recorded command sessions are 74245 (initial
MSRV), 31542 (current Rust) and 88928 (final MSRV and all-feature library).
Bounded command outputs are retained in the recorded root's `tmp/` directory
as `quality-epoch-msrv-final.log`, `quality-epoch-stable.log`,
`quality-epoch-failing-before.log` and
`quality-epoch-all-feature-lib-msrv.log`.

Cleanup used `git worktree remove --force` on the exact audited checkout after
checking its expected HEAD, unchanged tracked files and lock, and the known
audit-only untracked files. Final process and open-file counts were zero.
Both GUI and system launchd checks found zero references. Nineteen symlinks
and 57 configuration/environment/Git dependency files were inspected, with
no external dependency on the checkout and no scan error. Removal covered
3,087 regular files (148,807,200 bytes), 528 directories and the internal
`.claude/skills` symlink. The unrelated backup SMB mount warning from
`lsof` remains recorded; the scoped local checkout query found no open file.
`tmp/quality-epoch-cleanup-final.json` retains the audit. Shared build/source/
tool caches, the common bare repository and other agents' worktrees remain
live and were preserved. No wallet, miner/validator state, private dataset,
model asset or recovery journal was present in the removed checkout.

## Final isolated verification

Session 49576 reran the exact submitted HEAD with
`RUSTC_WORKSPACE_WRAPPER=~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh`.
That executable contains only `#!/bin/sh` and `exec "$@"`; its SHA-256 is
`7f812349eb1017d2122230d2daf3592fc5952efee881ffc42585d876687a97ee`.
Its unique path separates workspace artifacts from the other PR checkouts
while retaining the shared registry/Git dependencies and build directory.
Every invocation also sets `PYO3_NO_PYTHON=1` and uses `--locked --offline`.

The final results are:

| Compiler | Submitted checks | Independent audit | All-feature library |
| --- | --- | --- | --- |
| Rust 1.99.0 (`b940084d7`, 2026-09-28) | 7 epoch + 4 clock/payload/API pass | 11 pass; 20,568 model cases; exact drand decryptions | 71 pass; one GPU test ignored |
| Rust 1.89.0 (`29483883e`, 2025-08-04) | 7 epoch + 4 clock/payload/API pass | 11 pass; 20,568 model cases; exact drand decryptions | 71 pass; one GPU test ignored |

The deliberate base regression fails with exit 101 and exactly `Ok(110)`
versus `Ok(126)`. It selects the already verified MSRV audit executable; this
selection correctly reports `fresh=true`, rather than rebuilding the same
isolated executable. Every first isolated executable in the table below
reports `fresh=false`. Verbose compiler commands show the assigned wrapper;
Cargo JSON records the exact recreated manifest and source paths. The
artifact checker rejects missing/extra targets, unexpected source paths,
uncompiled initial executables, missing wrapper compiler invocations, changed
source/lock content or an unexpected test result.

| Compiler/check | Test executable basename | Executable SHA-256 |
| --- | --- | --- |
| Current epoch | `epoch_safety-340815eb7e0bfa52` | `49fce400ad7ef6b3afe0f0bd6463cfd4faa731f7c469c4ef42e9d9acb5b7a851` |
| Current clock/API | `explicit_commit_clock-78bf1cd3b7ede866` | `2ee858fa2ac2f69a55b21c1d40a757f3a7a361dda59b66d974460695f6379e82` |
| Current audit | `quality_epoch_audit-f6aa9413316ebb6d` | `aa91d1141084746f436e638af81db2d29abc1091ff42e2c94830a4c5c1c9bf25` |
| MSRV epoch | `epoch_safety-980a6f76cb22ff6f` | `0b21ba84bb5f9a3194e87728ce33522c77c2d42d6c223ef9c75363860bf38043` |
| MSRV clock/API | `explicit_commit_clock-0229bcf0f56780e9` | `291e5aa54d2673b7bd73aaf1ea2f294ce172ae81be1f52b5e1812addfd0a59e6` |
| MSRV audit | `quality_epoch_audit-97dcced79f1308b1` | `1166174b932c5649f0787e1ae5a606bcd0bb9ee31f027ac13e8f893c77268889` |
| MSRV library | `bittensor_core-51d2bad3d78d8da3` | `1b5c9019f4cb391ae187ffc1f1086b60cf82ee17cc6f0208594ecf463fc72edd` |
| Current library | `bittensor_core-a89b1d3a2a08d81c` | `4399a4cb2fae150a90313c49e133f865cc067d0ed4fc29d2c3060a6acfc6eb3f` |

All binaries are under the recorded shared `build/debug/deps/` directory.
The exact selected manifest is the recreated checkout's
`sdk/bittensor-core/Cargo.toml`, SHA-256
`cb01dca2ae8a802bd80dbf4334a877e22fa949345264300169c23fa752e85a87`.
Its `src/lib.rs` SHA-256 is
`3d2cc1536bbcd632f850824895daea11c4a830c7e5354cbc35828ffa8bef28a1`.
Before/after maps confirm all recorded production and audit sources and
`Cargo.lock` remain unchanged through the isolated runs.

The final raw outputs are `tmp/quality-epoch-isolated-{stable,msrv,failing-before,lib-msrv,lib-stable}.{stdout,stderr}`
under the recorded task root. `tmp/quality-epoch-isolated-artifacts.json` retains
full compiler, wrapper, manifest, source, profile, package and binary identities,
including before/after hashes and raw-output hashes. The new first-artifact
names differ from the provisional runs. This isolated result supersedes the
initial dynamic evidence; it makes no claim of full monorepo preflight, a
hosted workflow, physical GPU execution or live pallet execution.
The identities report SHA-256 is
`ced598dabe47d295c64067241735923ce72c11e6fd5f6b2b19ac22bda3a7affb`.

<details>
<summary>Audit-only replay source and public drand fixtures</summary>

Save the Rust block as `sdk/bittensor-core/tests/quality_epoch_audit.rs`. Save the two JSON blocks in its sibling `quality_epoch_audit/` directory. Extract the base source files there from the stated base commit and prepare the counter-only variant as described above.

```rust
//! Independent audit-only tests; none of this file is part of the submitted PR.
#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use bittensor_core::timelock::epoch_schedule::{self as fixed, EpochScheduleState};

#[path = "quality_epoch_audit/base_epoch_schedule.rs"]
mod base;
#[path = "quality_epoch_audit/base_epoch_schedule_vectors.rs"]
mod base_vectors;
// Deliberately partial fix: only the counter predicate changes; extracted
// original cfg(test) blocks are omitted to avoid type collisions.
#[allow(unused_imports)]
#[path = "quality_epoch_audit/counter_only.rs"]
mod counter_only;
// The extracted original files are byte-for-byte v478 source, including their
// original crate-relative imports and tests.
mod timelock {
    pub mod constants {
        pub use bittensor_core::timelock::constants::*;
    }
    pub mod epoch_schedule {
        pub use crate::base::*;
    }
    pub mod epoch_schedule_vectors {
        pub use crate::base_vectors::*;
    }
}

fn inclusion_head() -> EpochScheduleState {
    EpochScheduleState {
        last_epoch_block: 90,
        pending_epoch_at: 0,
        subnet_epoch_index: 1,
        tempo: 20,
        blocks_since_last_step: 20,
        current_block: 105,
    }
}

fn as_base(s: &EpochScheduleState) -> base::EpochScheduleState {
    base::EpochScheduleState {
        last_epoch_block: s.last_epoch_block,
        pending_epoch_at: s.pending_epoch_at,
        subnet_epoch_index: s.subnet_epoch_index,
        tempo: s.tempo,
        blocks_since_last_step: s.blocks_since_last_step,
        current_block: s.current_block,
    }
}

// This represents pallet storage and hook order separately from the SDK
// implementation. `consistent` and `allowed` expose the branches the SDK
// explicitly excludes. The equations are sourced from v478 run_coinbase.rs
// drain_pending/should_run_epoch_with_tempo, and weights.rs lookahead.
#[derive(Clone)]
struct PalletTrace(EpochScheduleState);

impl PalletTrace {
    fn due(&self, block: u64) -> bool {
        let s = &self.0;
        s.tempo != 0
            && ((s.pending_epoch_at != 0 && block >= s.pending_epoch_at)
                || s.blocks_since_last_step > u64::from(s.tempo)
                || block.saturating_sub(s.last_epoch_block) >= u64::from(s.tempo))
    }

    fn reveal_queue_epoch(&self, block: u64) -> u64 {
        self.0.subnet_epoch_index + u64::from(self.due(block))
    }

    fn coinbase(&mut self, block: u64, consistent: bool, allowed: bool) {
        self.0.current_block = block;
        self.0.blocks_since_last_step = (self.0.blocks_since_last_step + 1)
            .min(u64::from(self.0.tempo) + 1);
        if self.due(block) {
            if !allowed {
                self.0.pending_epoch_at = block + 1;
                return;
            }
            if consistent {
                self.0.blocks_since_last_step = 0;
            }
            self.0.last_epoch_block = block;
            self.0.pending_epoch_at = 0;
            self.0.subnet_epoch_index += 1;
        }
    }

    fn assigned_commit_epoch(&self, block: u64) -> u64 {
        self.reveal_queue_epoch(block)
    }
}

fn model_prediction(head: &EpochScheduleState, period: u64) -> u64 {
    let inclusion = head.current_block + 1;
    let mut trace = PalletTrace(head.clone());
    trace.coinbase(inclusion, true, true);
    let assigned = trace.assigned_commit_epoch(inclusion);
    for block in inclusion + 1..=inclusion + (period + 2) * u64::from(head.tempo) + 2 {
        if trace.reveal_queue_epoch(block) == assigned + period {
            return block;
        }
        trace.coinbase(block, true, true);
    }
    panic!("audit model failed to reach reveal epoch")
}

#[test]
#[ignore = "Run explicitly: expected to fail against the extracted submitted base"]
fn failing_before_regression() {
    assert_eq!(base::predict_first_reveal_block(&as_base(&inclusion_head()), 1), Ok(126));
}

#[test]
fn passing_after_real_base_contrast() {
    let head = inclusion_head();
    assert_eq!(model_prediction(&head, 1), 126);
    assert_eq!(base::predict_first_reveal_block(&as_base(&head), 1), Ok(110));
    assert_eq!(fixed::predict_first_reveal_block(&head, 1), Ok(126));
    println!("real v478 predictor:110; independent pallet trace:126; submitted HEAD:126");
}

#[test]
fn counter_only_change_leaves_inclusion_phase_bug() {
    let head = inclusion_head();
    let partial = counter_only::EpochScheduleState {
        last_epoch_block: head.last_epoch_block,
        pending_epoch_at: head.pending_epoch_at,
        subnet_epoch_index: head.subnet_epoch_index,
        tempo: head.tempo,
        blocks_since_last_step: head.blocks_since_last_step,
        current_block: head.current_block,
    };
    assert_eq!(counter_only::predict_first_reveal_block(&partial, 1), Ok(107));
    println!("counter-predicate-only patch:107; correct submitted prediction:126");
}

#[test]
fn independent_pallet_trace_small_state_space() {
    let mut checked = 0;
    let mut base_mismatches = 0;
    for tempo in 1..=12_u16 {
        for elapsed in 0..=u64::from(tempo) * 2 {
            for counter in 0..=u64::from(tempo) + 1 {
                for pending_delta in [None, Some(0), Some(1), Some(3)] {
                    for period in [1, 2, 3] {
                        let head = EpochScheduleState {
                            last_epoch_block: 100,
                            pending_epoch_at: pending_delta.map_or(0, |d| 100 + elapsed + d),
                            subnet_epoch_index: 7,
                            tempo,
                            blocks_since_last_step: counter,
                            current_block: 100 + elapsed,
                        };
                        let expected = model_prediction(&head, period);
                        assert_eq!(fixed::predict_first_reveal_block(&head, period), Ok(expected), "{head:?}, period={period}");
                        base_mismatches += usize::from(base::predict_first_reveal_block(&as_base(&head), period) != Ok(expected));
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(base_mismatches > 0);
    println!("checked {checked} states; original predictor mismatched {base_mismatches}; submitted predictor mismatched 0");
}

#[test]
fn actual_skipped_owner_slot_and_tempo_reset_are_reachable() {
    // An owner-triggered epoch with inconsistent input consumes the slot but
    // keeps the bounded counter, exactly as drain_pending does.
    let mut trace = PalletTrace(EpochScheduleState {
        last_epoch_block: 80,
        pending_epoch_at: 90,
        subnet_epoch_index: 0,
        tempo: 20,
        blocks_since_last_step: 4,
        current_block: 89,
    });
    trace.coinbase(90, false, true);
    assert_eq!((trace.0.last_epoch_block, trace.0.blocks_since_last_step, trace.0.subnet_epoch_index), (90, 5, 1));
    for block in 91..=105 {
        trace.coinbase(block, true, true);
    }
    assert_eq!(trace.0, inclusion_head());
    assert_eq!(fixed::predict_first_reveal_block(&trace.0, 1), Ok(126));

    // apply_tempo_with_cycle_reset changes tempo and anchor, retaining counter.
    trace.0.tempo = 10;
    trace.0.last_epoch_block = 105;
    assert!(trace.due(106));
    assert!(fixed::should_run_epoch(&trace.0, 106));
}

#[test]
fn no_claim_to_predict_consistency_skips_or_scheduler_deferrals() {
    let head = inclusion_head();
    let mut inconsistent = PalletTrace(head.clone());
    inconsistent.coinbase(106, false, true);
    // Failed input retains counter21, so post-hook lookahead assigns epoch3.
    assert_eq!(inconsistent.assigned_commit_epoch(106), 3);
    let mut deferred = PalletTrace(head);
    deferred.coinbase(106, true, false);
    assert_eq!(deferred.0.pending_epoch_at, 107);
    assert_eq!(deferred.0.subnet_epoch_index, 1);
    assert_eq!(deferred.assigned_commit_epoch(106), 2);
    assert_eq!(deferred.0.blocks_since_last_step, 21);
}

#[test]
fn real_injected_clock_ciphertext_decrypts_exact_pallet_scale_bytes() {
    use bittensor_core::timelock::{commit_payload, decrypt_and_decompress, generate_commit_v2_at, WeightsTlockPayload};
    use bittensor_core::timelock::constants::GENESIS_TIME;
    use codec::{Decode, Encode};
    let head = EpochScheduleState {
        last_epoch_block: 10,
        current_block: 10,
        tempo: 50,
        blocks_since_last_step: 0,
        pending_epoch_at: 0,
        subnet_epoch_index: 0,
    };
    for (offset, round, response) in [
        (100.0, 245, include_str!("quality_epoch_audit/drand245.json")),
        (103.0, 246, include_str!("quality_epoch_audit/drand246.json")),
    ] {
        let drand: serde_json::Value = serde_json::from_str(response).unwrap();
        assert_eq!(drand["round"].as_u64(), Some(round));
        let signature = hex::decode(drand["signature"].as_str().unwrap()).unwrap();
        let (ciphertext, actual_round) = generate_commit_v2_at(GENESIS_TIME as f64 + offset, vec![1,2], vec![65535,1], 42, head.clone(), 1, 12.0, vec![7;32]).unwrap();
        assert_eq!(actual_round, round);
        let plaintext = decrypt_and_decompress(&ciphertext, &signature).unwrap();
        let expected = commit_payload(vec![7;32], vec![1,2], vec![65535,1], 42);
        assert_eq!(plaintext, expected);
        let payload = WeightsTlockPayload::decode(&mut plaintext.as_slice()).unwrap();
        assert_eq!(payload.encode(), plaintext);
        assert_eq!(payload.hotkey, vec![7;32]);
        assert_eq!(payload.uids, vec![1,2]);
        assert_eq!(payload.values, vec![65535,1]);
        assert_eq!(payload.version_key, 42);
        assert!(decrypt_and_decompress(&ciphertext, &[0;48]).is_err());
    }
}
```

`quality_epoch_audit/drand245.json`:

```json
{"round":245,"randomness":"a1b37e53d1756371191a34f6afdbcc7a88baa9eb18c9f2d3d3c40116f2233eec","signature":"97379d60029458835ae22e58d28916ec596636f3c6a8671ef35ee2f48187f8300d1dfd3b1a93f9fa9a55630654e1877e"}
```

`quality_epoch_audit/drand246.json`:

```json
{"round":246,"randomness":"de7b08a7c79a1d94ca3412422c19ed27bbdb908406a43b201d519bc6798ec213","signature":"8d8f3e49b6b3faa18e9c13bcf79b33e649ca03d451fddf4f3dccf5f2f097486c7e03f92ae13e4881699eb2c9aa101408"}
```

</details>

<details>
<summary>Accepted final archive: exact identities and bounded test logs</summary>

`quality-epoch-isolated-artifacts.json`, retained verbatim:

```text
{
  "head": "9b4d8c51aa7d9fb11be6f0e4cef500e02e0fad36",
  "wrapper": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh",
  "wrapper_sha256": "7f812349eb1017d2122230d2daf3592fc5952efee881ffc42585d876687a97ee",
  "compiler_versions": {
    "stable": "rustc 1.99.0 (b940084d7 2026-09-28)",
    "msrv": "rustc 1.89.0 (29483883e 2025-08-04)"
  },
  "source_before": {
    "Cargo.lock": "c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a",
    "Cargo.toml": "0831ada0c8fd51913ee791bfced071d089fc7c161f1b41b24391bbdad750ae12",
    "sdk/bittensor-core/Cargo.toml": "cb01dca2ae8a802bd80dbf4334a877e22fa949345264300169c23fa752e85a87",
    "sdk/bittensor-core/src/lib.rs": "3d2cc1536bbcd632f850824895daea11c4a830c7e5354cbc35828ffa8bef28a1",
    "sdk/bittensor-core/src/timelock/mod.rs": "5dce06b114efd42c3ba550840ec47b0afd1358e928de18dc5e5f58ba042c8b9a",
    "sdk/bittensor-core/src/timelock/epoch_schedule.rs": "3dc094449433717d36228b727dbe79bcfccf09cf3dd711148ebda796d8128e74",
    "sdk/bittensor-core/src/timelock/epoch_schedule_vectors.rs": "ecc035b52291ca54f3b63c7480fa171cba1e9e6b028aa35b6c06b8afc2e6e54d",
    "sdk/bittensor-core/tests/epoch_safety.rs": "d8bc0a3e8e40ceee0f48079ca62f6115d6e048595abe6b274fff132d3c2cbe3e",
    "sdk/bittensor-core/tests/explicit_commit_clock.rs": "f951f140f6d200550c69e857b439c664a7064ec0f81ab438fa18f419e932fab6",
    "sdk/bittensor-core/tests/quality_epoch_audit.rs": "76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50"
  },
  "source_after": {
    "Cargo.lock": "c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a",
    "Cargo.toml": "0831ada0c8fd51913ee791bfced071d089fc7c161f1b41b24391bbdad750ae12",
    "sdk/bittensor-core/Cargo.toml": "cb01dca2ae8a802bd80dbf4334a877e22fa949345264300169c23fa752e85a87",
    "sdk/bittensor-core/src/lib.rs": "3d2cc1536bbcd632f850824895daea11c4a830c7e5354cbc35828ffa8bef28a1",
    "sdk/bittensor-core/src/timelock/mod.rs": "5dce06b114efd42c3ba550840ec47b0afd1358e928de18dc5e5f58ba042c8b9a",
    "sdk/bittensor-core/src/timelock/epoch_schedule.rs": "3dc094449433717d36228b727dbe79bcfccf09cf3dd711148ebda796d8128e74",
    "sdk/bittensor-core/src/timelock/epoch_schedule_vectors.rs": "ecc035b52291ca54f3b63c7480fa171cba1e9e6b028aa35b6c06b8afc2e6e54d",
    "sdk/bittensor-core/tests/epoch_safety.rs": "d8bc0a3e8e40ceee0f48079ca62f6115d6e048595abe6b274fff132d3c2cbe3e",
    "sdk/bittensor-core/tests/explicit_commit_clock.rs": "f951f140f6d200550c69e857b439c664a7064ec0f81ab438fa18f419e932fab6",
    "sdk/bittensor-core/tests/quality_epoch_audit.rs": "76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50"
  },
  "results": [
    {
      "run": "stable",
      "artifacts": [
        {
          "name": "epoch_safety",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/epoch_safety.rs",
          "source_sha256": "d8bc0a3e8e40ceee0f48079ca62f6115d6e048595abe6b274fff132d3c2cbe3e",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/epoch_safety-340815eb7e0bfa52",
          "executable_sha256": "49fce400ad7ef6b3afe0f0bd6463cfd4faa731f7c469c4ef42e9d9acb5b7a851",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        },
        {
          "name": "quality_epoch_audit",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/quality_epoch_audit.rs",
          "source_sha256": "76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/quality_epoch_audit-f6aa9413316ebb6d",
          "executable_sha256": "aa91d1141084746f436e638af81db2d29abc1091ff42e2c94830a4c5c1c9bf25",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        },
        {
          "name": "explicit_commit_clock",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/explicit_commit_clock.rs",
          "source_sha256": "f951f140f6d200550c69e857b439c664a7064ec0f81ab438fa18f419e932fab6",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/explicit_commit_clock-78bf1cd3b7ede866",
          "executable_sha256": "2ee858fa2ac2f69a55b21c1d40a757f3a7a361dda59b66d974460695f6379e82",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        }
      ],
      "stdout_sha256": "d2c6cecfb30ee765c237cd544e7c43a6dd6a42bf63a08cc39bf3eb082ae3716e",
      "stderr_sha256": "dd6c41bacb3db111801042efcd3c5145b6b89aa61ca87aa7dd90378344f13eb8"
    },
    {
      "run": "msrv",
      "artifacts": [
        {
          "name": "epoch_safety",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/epoch_safety.rs",
          "source_sha256": "d8bc0a3e8e40ceee0f48079ca62f6115d6e048595abe6b274fff132d3c2cbe3e",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/epoch_safety-980a6f76cb22ff6f",
          "executable_sha256": "0b21ba84bb5f9a3194e87728ce33522c77c2d42d6c223ef9c75363860bf38043",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        },
        {
          "name": "quality_epoch_audit",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/quality_epoch_audit.rs",
          "source_sha256": "76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/quality_epoch_audit-97dcced79f1308b1",
          "executable_sha256": "1166174b932c5649f0787e1ae5a606bcd0bb9ee31f027ac13e8f893c77268889",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        },
        {
          "name": "explicit_commit_clock",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/explicit_commit_clock.rs",
          "source_sha256": "f951f140f6d200550c69e857b439c664a7064ec0f81ab438fa18f419e932fab6",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/explicit_commit_clock-0229bcf0f56780e9",
          "executable_sha256": "291e5aa54d2673b7bd73aaf1ea2f294ce172ae81be1f52b5e1812addfd0a59e6",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        }
      ],
      "stdout_sha256": "4b698669f4b943c995e78094f00f408c06da01b2effe47f3efc0e1e35ac8414c",
      "stderr_sha256": "91ec6d01c1e9427841c5b904bac87569d22cc61a4579a7551257b1cfa235a9ae"
    },
    {
      "run": "failing-before",
      "artifacts": [
        {
          "name": "quality_epoch_audit",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/tests/quality_epoch_audit.rs",
          "source_sha256": "76e1499b8a19994123eccc7a24b6cf8b686833133ffd667d27a8cf4a29388d50",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/quality_epoch_audit-97dcced79f1308b1",
          "executable_sha256": "1166174b932c5649f0787e1ae5a606bcd0bb9ee31f027ac13e8f893c77268889",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": true,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        }
      ],
      "stdout_sha256": "75130f580bf2f4a0df24bf5d63dfe07d443139ed667762ff007fd77f13c7b4db",
      "stderr_sha256": "ffe7eaa2156874ebd8b20402babb0db2f02da18f2a74477cd6afd31b79a936cd"
    },
    {
      "run": "lib-msrv",
      "artifacts": [
        {
          "name": "bittensor_core",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/src/lib.rs",
          "source_sha256": "3d2cc1536bbcd632f850824895daea11c4a830c7e5354cbc35828ffa8bef28a1",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/bittensor_core-51d2bad3d78d8da3",
          "executable_sha256": "1b5c9019f4cb391ae187ffc1f1086b60cf82ee17cc6f0208594ecf463fc72edd",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        }
      ],
      "stdout_sha256": "0a603759cbedcb28bd63030a9aafda3880771b22799c7dcbae02d3748a4e6be5",
      "stderr_sha256": "36a7355cd75eb9311612c95b1aa598ebd388b631e157970ce9aba712eff9b498"
    },
    {
      "run": "lib-stable",
      "artifacts": [
        {
          "name": "bittensor_core",
          "source": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core/src/lib.rs",
          "source_sha256": "3d2cc1536bbcd632f850824895daea11c4a830c7e5354cbc35828ffa8bef28a1",
          "executable": "~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps/bittensor_core-a89b1d3a2a08d81c",
          "executable_sha256": "4399a4cb2fae150a90313c49e133f865cc067d0ed4fc29d2c3060a6acfc6eb3f",
          "package_id": "path+file://~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/sdk/bittensor-core#0.1.0",
          "fresh": false,
          "profile": {
            "opt_level": "3",
            "debuginfo": 0,
            "debug_assertions": true,
            "overflow_checks": true,
            "test": true
          }
        }
      ],
      "stdout_sha256": "91d9831bb218dd9713920a154e092773a8d8564e7e5161e91f07fbd4afdfdd2e",
      "stderr_sha256": "9dd03ff10ff26f988d1bd7c17df95e5be1dbce5413ecd2c7b9af8da32809057d"
    }
  ],
  "verification": "Exact manifest/source and no-op wrapper compiler invocation checked; each initial isolated executable was compiled (fresh=false). All run results and deliberate base failure verified."
}
```

Final isolated stable selected test output:

```text
running 7 tests
test all_upstream_prediction_vectors ... ok
test inclusion_counter_fire_assigns_post_coinbase_epoch ... ok
test skipped_owner_slot_fires_at_counter_threshold ... ok
test skipped_scheduled_slot_fires_next_block ... ok
test prediction_refuses_overflow_and_disabled_tempo ... ok
test lowered_tempo_fires_next_block_and_counter_is_bounded ... ok
test overflowing_simulation_budget_fails_before_searching ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 4 tests
test clock_and_security_offset_pin_round ... ok
test payload_uses_upstream_scale_definition ... ok
test invalid_clock_and_timestamp_overflow_fail_before_crypto ... ok
test injected_clock_drives_the_real_commit_api ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
running 12 tests
counter-predicate-only patch:107; correct submitted prediction:126
test base::integration::commit_epoch_at_extrinsic_block_table ... ok
real v478 predictor:110; independent pallet trace:126; submitted HEAD:126
test actual_skipped_owner_slot_and_tempo_reset_are_reachable ... ok
test failing_before_regression ... ignored, Run explicitly: expected to fail against the extracted submitted base
test counter_only_change_leaves_inclusion_phase_bug ... ok
test no_claim_to_predict_consistency_skips_or_scheduler_deferrals ... ok
test base::integration::reveal_uses_exact_equality_not_gte ... ok
test base::integration::predict_first_reveal_block_table ... ok
test base::tests::should_run_epoch_auto_fire ... ok
test passing_after_real_base_contrast ... ok
test base::tests::should_run_epoch_tempo_zero ... ok
checked 20568 states; original predictor mismatched 1507; submitted predictor mismatched 0
test independent_pallet_trace_small_state_space ... ok
test real_injected_clock_ciphertext_decrypts_exact_pallet_scale_bytes ... ok
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
Compiler invocation prefix; dependency --extern arguments omitted:
     Running `~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc --crate-name bittensor_core --edition=2021 sdk/bittensor-core/src/lib.rs --error-format=json --json=diagnostic-rendered-ansi,artifacts,future-incompat --crate-type rlib --emit=dep-info,metadata,link -C opt-level=3 -C embed-bitcode=no '--allow=clippy::useless_conversion' '--deny=clippy::unwrap-used' --allow=unexpected_cfgs '--allow=clippy::type_complexity' '--allow=clippy::result_large_err' '--allow=clippy::manual_inspect' '--deny=clippy::indexing-slicing' '--deny=clippy::expect-used' '--deny=clippy::arithmetic-side-effects' --check-cfg 'cfg(substrate_runtime)' -C debug-assertions=on --check-cfg 'cfg(docsrs,test)' --check-cfg 'cfg(feature, values("default", "host", "ledger"))' -C metadata=2042cbeb8709b90d -C extra-filename=-66796e2c20d8baa8 --out-dir ~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps -C incremental=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/incremental -C strip=debuginfo -L dependency=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps ...
```

Final isolated msrv selected test output:

```text
running 7 tests
test inclusion_counter_fire_assigns_post_coinbase_epoch ... ok
test lowered_tempo_fires_next_block_and_counter_is_bounded ... ok
test all_upstream_prediction_vectors ... ok
test overflowing_simulation_budget_fails_before_searching ... ok
test prediction_refuses_overflow_and_disabled_tempo ... ok
test skipped_owner_slot_fires_at_counter_threshold ... ok
test skipped_scheduled_slot_fires_next_block ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 4 tests
test clock_and_security_offset_pin_round ... ok
test invalid_clock_and_timestamp_overflow_fail_before_crypto ... ok
test payload_uses_upstream_scale_definition ... ok
test injected_clock_drives_the_real_commit_api ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
running 12 tests
counter-predicate-only patch:107; correct submitted prediction:126
test base::integration::commit_epoch_at_extrinsic_block_table ... ok
real v478 predictor:110; independent pallet trace:126; submitted HEAD:126
test actual_skipped_owner_slot_and_tempo_reset_are_reachable ... ok
test base::tests::should_run_epoch_auto_fire ... ok
test base::integration::predict_first_reveal_block_table ... ok
test base::integration::reveal_uses_exact_equality_not_gte ... ok
test base::tests::should_run_epoch_tempo_zero ... ok
test failing_before_regression ... ignored, Run explicitly: expected to fail against the extracted submitted base
test no_claim_to_predict_consistency_skips_or_scheduler_deferrals ... ok
test counter_only_change_leaves_inclusion_phase_bug ... ok
test passing_after_real_base_contrast ... ok
checked 20568 states; original predictor mismatched 1507; submitted predictor mismatched 0
test independent_pallet_trace_small_state_space ... ok
test real_injected_clock_ciphertext_decrypts_exact_pallet_scale_bytes ... ok
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
Compiler invocation prefix; dependency --extern arguments omitted:
     Running `~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh ~/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin/rustc --crate-name bittensor_core --edition=2021 sdk/bittensor-core/src/lib.rs --error-format=json --json=diagnostic-rendered-ansi,artifacts,future-incompat --crate-type rlib --emit=dep-info,metadata,link -C opt-level=3 -C embed-bitcode=no '--allow=clippy::useless_conversion' '--deny=clippy::unwrap-used' --allow=unexpected_cfgs '--allow=clippy::type_complexity' '--allow=clippy::result_large_err' '--allow=clippy::manual_inspect' '--deny=clippy::indexing-slicing' '--deny=clippy::expect-used' '--deny=clippy::arithmetic-side-effects' --check-cfg 'cfg(substrate_runtime)' -C debug-assertions=on --check-cfg 'cfg(docsrs,test)' --check-cfg 'cfg(feature, values("default", "host", "ledger"))' -C metadata=ebcdedb780eb0e86 -C extra-filename=-4169ab9ff336486d --out-dir ~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps -C incremental=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/incremental -C strip=debuginfo -L dependency=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps ...
```

Final isolated failing-before selected test output:

```text
running 1 test
test failing_before_regression ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out; finished in 0.00s
assertion `left == right` failed
  left: Ok(110)
 right: Ok(126)
```

Final isolated lib-msrv selected test output:

```text
running 72 tests
test client::reorg_finalization_tests::canonical_inclusion_remains_pending_below_finalized_height ... ok
test client::reorg_finalization_tests::canonical_inclusion_finalizes_at_or_above_its_height ... ok
test client::reorg_finalization_tests::replaced_or_missing_inclusion_hash_is_a_reorg ... ok
test digest::tests::rejects_garbage_metadata ... ok
test keyfiles::tests::env_password_roundtrip ... ok
test keyfiles::tests::crypto_type_as_string_is_accepted ... ok
test codec::corpus_tests::multisig_accounts_match_the_golden_vectors ... ok
test keyfiles::tests::ed25519_keyfile_roundtrip ... ok
test keyfiles::tests::legacy_keyfile_without_crypto_type_defaults_sr25519 ... ok
test keyfiles::tests::mnemonic_keypair_writes_legacy_secret_fields ... ok
test keyfiles::tests::mnemonic_with_derivation_password_omits_phrase_keeps_seed ... ok
test keyfiles::tests::missing_crypto_type_recovered_from_stored_ss58 ... ok
test codec::corpus_tests::storage_keys_match_the_golden_vectors ... ok
test codec::corpus_tests::composed_calls_decode_back ... ok
test codec::corpus_tests::signed_extrinsics_match_the_golden_vectors ... ok
test keyfiles::tests::polkadotjs_export_gets_actionable_error ... ok
test codec::corpus_tests::composed_calls_match_the_golden_vectors ... ok
test codec::corpus_tests::extrinsics_decode_like_the_golden_block ... ok
test codec::corpus_tests::signature_payloads_match_the_golden_vectors ... ok
test digest::tests::digest_matches_polkadot_js_merkleize_metadata ... ok
test codec::corpus_tests::pathological_nesting_errors_instead_of_overflowing ... ok
test keyfiles::tests::raw_hex_private_key_fallback ... ok
test keyfiles::tests::raw_hex_seed_fallback ... ok
test digest::tests::extrinsic_proof_matches_pinned_vectors ... ok
test keyfiles::tests::unknown_json_error_names_found_fields ... ok
test keyfiles::tests::raw_mnemonic_fallback ... ok
test keys::encrypted_json::tests::accepts_standard_scrypt_cost ... ok
test keyfiles::tests::sr25519_keyfile_roundtrip ... ok
test codec::corpus_tests::map_keys_recover_from_the_golden_query_maps ... ok
test keys::encrypted_json::tests::ed25519_passphrase_import_wrong_password ... ok
test keys::encrypted_json::tests::rejects_excessive_scrypt_cost ... ok
test keyfiles::tests::stored_ss58_mismatch_is_rejected ... ok
test keys::encrypted_json::tests::rejects_unsupported_version ... ok
test keys::base58::tests::matches_bs58_reference ... ok
test mlkem::tests::seal_rejects_bad_public_key ... ok
test keys::tests::alice_uri_matches_known_address ... ok
test pow::tests::gpu_mines_and_revalidates_chain_compatible_work ... ignored, requires physical OpenCL GPUs; run explicitly on the GPU test host
test mlkem::tests::seal_roundtrip_with_decapsulation ... ok
test keys::encrypted_json::tests::ed25519_passphrase_import_roundtrip ... ok
test mlkem::tests::twox_128_matches_substrate ... ok
test pow::tests::cpu_mining_preserves_nonce_and_little_endian_target ... ok
test pow::tests::invalid_challenges_and_unbounded_batches_are_rejected ... ok
test mlkem::tests::seal_with_key_hash_prefixes_twox ... ok
test pow::tests::seal_matches_independent_python_vectors ... ok
test signers::ledger::tests::bip44_path_matches_generic_app_layout ... ok
test signers::ledger::tests::device_error_formats ... ok
test keys::tests::public_only_from_ss58_matches_full_key ... ok
test keys::tests::sr25519_verify_roundtrip ... ok
test timelock::epoch_schedule::integration::commit_epoch_at_extrinsic_block_table ... ok
test timelock::epoch_schedule::integration::predict_first_reveal_block_table ... ok
test timelock::epoch_schedule::integration::reveal_uses_exact_equality_not_gte ... ok
test timelock::epoch_schedule::tests::should_run_epoch_auto_fire ... ok
test timelock::epoch_schedule::tests::should_run_epoch_tempo_zero ... ok
test keys::tests::sr25519_verify_bytes_wrapping ... ok
test timelock::tests::inner_ciphertext_unwraps_user_data_envelope ... ok
test transaction::tests::allowing_raw_does_not_disable_spend_or_subnet_guardrails ... ok
test transaction::tests::arbitrary_call_fails_closed_for_every_policy_dimension ... ok
test keys::tests::ss58_rendering_matches_sp_core ... ok
test transaction::tests::compatibility_builders_cannot_downgrade_arbitrary_calls ... ok
test transaction::tests::compatibility_builders_cannot_lower_trusted_spend_or_scope ... ok
test transaction::tests::trusted_move_stake_limit_uses_constructor_derived_netuids ... ok
test transaction::tests::trusted_move_stake_uses_constructor_derived_netuids ... ok
test transaction::tests::trusted_transfer_uses_constructor_derived_bounded_spend ... ok
test timelock::tests::test_encrypt_commitment_format ... ok
test timelock::tests::test_generate_commit_v2_structure ... ok
test digest::tests::digest_is_deterministic_and_spec_version_sensitive ... ok
test keyfiles::tests::ansible_vault_roundtrip ... ok
test codec::corpus_tests::decoder_reproduces_the_shape_corpus ... ok
test timelock::tests::test_encrypt_and_decrypt_static_key ... ok
test timelock::tests::test_get_round_info_and_signature ... ok
test keyfiles::tests::nacl_roundtrip ... ok
test keyfiles::tests::legacy_fernet_roundtrip ... ok
test result: ok. 71 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 5.81s
Compiler invocation prefix; dependency --extern arguments omitted:
     Running `~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh ~/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin/rustc --crate-name bittensor_core --edition=2021 sdk/bittensor-core/src/lib.rs --error-format=json --json=diagnostic-rendered-ansi,artifacts,future-incompat --emit=dep-info,link -C opt-level=3 -C embed-bitcode=no '--allow=clippy::useless_conversion' '--deny=clippy::unwrap-used' --allow=unexpected_cfgs '--allow=clippy::type_complexity' '--allow=clippy::result_large_err' '--allow=clippy::manual_inspect' '--deny=clippy::indexing-slicing' '--deny=clippy::expect-used' '--deny=clippy::arithmetic-side-effects' --check-cfg 'cfg(substrate_runtime)' -C debug-assertions=on --test --cfg 'feature="default"' --cfg 'feature="host"' --cfg 'feature="ledger"' --check-cfg 'cfg(docsrs,test)' --check-cfg 'cfg(feature, values("default", "host", "ledger"))' -C metadata=8519a3c165189096 -C extra-filename=-51d2bad3d78d8da3 --out-dir ~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps -C incremental=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/incremental -C strip=debuginfo -L dependency=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps ...
```

Final isolated lib-stable selected test output:

```text
running 72 tests
test client::reorg_finalization_tests::canonical_inclusion_finalizes_at_or_above_its_height ... ok
test client::reorg_finalization_tests::canonical_inclusion_remains_pending_below_finalized_height ... ok
test client::reorg_finalization_tests::replaced_or_missing_inclusion_hash_is_a_reorg ... ok
test digest::tests::rejects_garbage_metadata ... ok
test keyfiles::tests::env_password_roundtrip ... ok
test codec::corpus_tests::multisig_accounts_match_the_golden_vectors ... ok
test keyfiles::tests::crypto_type_as_string_is_accepted ... ok
test keyfiles::tests::ed25519_keyfile_roundtrip ... ok
test keyfiles::tests::legacy_keyfile_without_crypto_type_defaults_sr25519 ... ok
test keyfiles::tests::mnemonic_keypair_writes_legacy_secret_fields ... ok
test keyfiles::tests::mnemonic_with_derivation_password_omits_phrase_keeps_seed ... ok
test keyfiles::tests::missing_crypto_type_recovered_from_stored_ss58 ... ok
test keyfiles::tests::polkadotjs_export_gets_actionable_error ... ok
test keyfiles::tests::raw_hex_private_key_fallback ... ok
test keyfiles::tests::raw_hex_seed_fallback ... ok
test codec::corpus_tests::signed_extrinsics_match_the_golden_vectors ... ok
test codec::corpus_tests::storage_keys_match_the_golden_vectors ... ok
test codec::corpus_tests::map_keys_recover_from_the_golden_query_maps ... ok
test codec::corpus_tests::composed_calls_decode_back ... ok
test codec::corpus_tests::composed_calls_match_the_golden_vectors ... ok
test keyfiles::tests::unknown_json_error_names_found_fields ... ok
test codec::corpus_tests::extrinsics_decode_like_the_golden_block ... ok
test keys::encrypted_json::tests::accepts_standard_scrypt_cost ... ok
test keys::encrypted_json::tests::rejects_excessive_scrypt_cost ... ok
test codec::corpus_tests::pathological_nesting_errors_instead_of_overflowing ... ok
test keys::encrypted_json::tests::rejects_unsupported_version ... ok
test keyfiles::tests::raw_mnemonic_fallback ... ok
test digest::tests::digest_matches_polkadot_js_merkleize_metadata ... ok
test keys::base58::tests::matches_bs58_reference ... ok
test keys::tests::alice_uri_matches_known_address ... ok
test keyfiles::tests::sr25519_keyfile_roundtrip ... ok
test mlkem::tests::seal_rejects_bad_public_key ... ok
test mlkem::tests::seal_roundtrip_with_decapsulation ... ok
test mlkem::tests::seal_with_key_hash_prefixes_twox ... ok
test mlkem::tests::twox_128_matches_substrate ... ok
test keys::encrypted_json::tests::ed25519_passphrase_import_roundtrip ... ok
test pow::tests::gpu_mines_and_revalidates_chain_compatible_work ... ignored, requires physical OpenCL GPUs; run explicitly on the GPU test host
test pow::tests::cpu_mining_preserves_nonce_and_little_endian_target ... ok
test keys::encrypted_json::tests::ed25519_passphrase_import_wrong_password ... ok
test pow::tests::invalid_challenges_and_unbounded_batches_are_rejected ... ok
test pow::tests::seal_matches_independent_python_vectors ... ok
test signers::ledger::tests::bip44_path_matches_generic_app_layout ... ok
test signers::ledger::tests::device_error_formats ... ok
test timelock::epoch_schedule::integration::commit_epoch_at_extrinsic_block_table ... ok
test timelock::epoch_schedule::integration::predict_first_reveal_block_table ... ok
test keys::tests::public_only_from_ss58_matches_full_key ... ok
test timelock::epoch_schedule::integration::reveal_uses_exact_equality_not_gte ... ok
test timelock::epoch_schedule::tests::should_run_epoch_auto_fire ... ok
test timelock::epoch_schedule::tests::should_run_epoch_tempo_zero ... ok
test timelock::tests::inner_ciphertext_unwraps_user_data_envelope ... ok
test keyfiles::tests::stored_ss58_mismatch_is_rejected ... ok
test transaction::tests::allowing_raw_does_not_disable_spend_or_subnet_guardrails ... ok
test transaction::tests::arbitrary_call_fails_closed_for_every_policy_dimension ... ok
test keys::tests::ss58_rendering_matches_sp_core ... ok
test transaction::tests::compatibility_builders_cannot_downgrade_arbitrary_calls ... ok
test transaction::tests::compatibility_builders_cannot_lower_trusted_spend_or_scope ... ok
test transaction::tests::trusted_move_stake_limit_uses_constructor_derived_netuids ... ok
test keys::tests::sr25519_verify_bytes_wrapping ... ok
test transaction::tests::trusted_move_stake_uses_constructor_derived_netuids ... ok
test transaction::tests::trusted_transfer_uses_constructor_derived_bounded_spend ... ok
test keys::tests::sr25519_verify_roundtrip ... ok
test codec::corpus_tests::signature_payloads_match_the_golden_vectors ... ok
test digest::tests::extrinsic_proof_matches_pinned_vectors ... ok
test timelock::tests::test_encrypt_commitment_format ... ok
test timelock::tests::test_generate_commit_v2_structure ... ok
test digest::tests::digest_is_deterministic_and_spec_version_sensitive ... ok
test codec::corpus_tests::decoder_reproduces_the_shape_corpus ... ok
test keyfiles::tests::ansible_vault_roundtrip ... ok
test timelock::tests::test_encrypt_and_decrypt_static_key ... ok
test keyfiles::tests::nacl_roundtrip ... ok
test timelock::tests::test_get_round_info_and_signature ... ok
test keyfiles::tests::legacy_fernet_roundtrip ... ok
test result: ok. 71 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 4.98s
Compiler invocation prefix; dependency --extern arguments omitted:
     Running `~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh ~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc --crate-name bittensor_core --edition=2021 sdk/bittensor-core/src/lib.rs --error-format=json --json=diagnostic-rendered-ansi,artifacts,future-incompat --emit=dep-info,link -C opt-level=3 -C embed-bitcode=no '--allow=clippy::useless_conversion' '--deny=clippy::unwrap-used' --allow=unexpected_cfgs '--allow=clippy::type_complexity' '--allow=clippy::result_large_err' '--allow=clippy::manual_inspect' '--deny=clippy::indexing-slicing' '--deny=clippy::expect-used' '--deny=clippy::arithmetic-side-effects' --check-cfg 'cfg(substrate_runtime)' -C debug-assertions=on --test --cfg 'feature="default"' --cfg 'feature="host"' --cfg 'feature="ledger"' --check-cfg 'cfg(docsrs,test)' --check-cfg 'cfg(feature, values("default", "host", "ledger"))' -C metadata=338a83eb203c6159 -C extra-filename=-a89b1d3a2a08d81c --out-dir ~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps -C incremental=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/incremental -C strip=debuginfo -L dependency=~/workspace/rtao/target/execute-plan.6Ro0PC/build/debug/deps ...
```

</details>

Final accepted-checkout cleanup on 2026-10-14 removed exactly the recreated worktree and its 20-byte no-op wrapper. Process/open-file counts and both launchd reference counts were zero. Thirty-four symlinks and 58 configuration/environment/Git files were inspected without a dependency reference or scan error. The worktree removal covered 3,087 regular files (148,807,200 bytes), 527 directories and its internal skills symlink. The shared build, bare repository, other wrappers and other owners' caches/state were retained. The exact audit is archived below.

<details>
<summary>Accepted-checkout cleanup record</summary>

```text
{
  "removed_checkout": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248",
  "removed_wrapper": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers/pr-3248.sh",
  "head": "9b4d8c51aa7d9fb11be6f0e4cef500e02e0fad36",
  "lock_before_and_after": "c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a",
  "wrapper_sha256": "7f812349eb1017d2122230d2daf3592fc5952efee881ffc42585d876687a97ee",
  "removed_regular_files": 3087,
  "removed_regular_bytes": 148807200,
  "removed_directories": 527,
  "owned_symlinks": [
    {
      "path": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/.claude/skills",
      "target": "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/pr-3248/.agents/skills"
    }
  ],
  "symlinks_inspected": 34,
  "configuration_environment_Git_files_inspected": 58,
  "process_matches": 0,
  "open_files": 0,
  "launchd_matches": {
    "gui/<uid>": 0,
    "system": 0
  },
  "dependency_references": [],
  "scan_errors": [],
  "lsof_limitations": [
    "Unrelated backup SMB mount unavailable; exact local path has no open file."
  ],
  "retained_live_dependencies": [
    "~/workspace/rtao/target/execute-plan.6Ro0PC/build",
    "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/subtensor.git",
    "~/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/wrappers",
    "Other owners task directories, wrappers and shared source/tool caches"
  ],
  "archived_evidence": "~/workspace/rtao/docs/upstream/validation/quality-epoch.md",
  "note": "Git worktree registration removed by git worktree remove. Build dep-info and retained audit/log/replay text are provenance, not live runtime or virtual-environment dependencies.",
  "user_state_preserved": "No wallet, miner/validator state, private dataset, model asset or recovery journal was present in the exact owned checkout.",
  "removed": true
}
```

</details>
