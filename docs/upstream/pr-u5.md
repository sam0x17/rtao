# PR-U5: epoch safety and explicit timelock clock

Filed 2026-10-14: [RaoFoundation/subtensor#3248](https://github.com/RaoFoundation/subtensor/pull/3248).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-timelock-epoch-safety` |
| Review commit | `9b4d8c51aa7d9fb11be6f0e4cef500e02e0fad36` |
| First commit | `4f1f9cefdda8dab60997cd2001149ddf6b630997` |
| Retained patch | [pr-u5.patch](pr-u5.patch), both normal commits against v478 |
| Patch SHA256 | `cecf6926b7536ae44d85dc8072d696dd64806c30ec7cd990a43491f2e6d9c33b` |

The timelock predictor compared the stored step counter with the maximum
possible tempo, missing the subnet's actual counter safety threshold. It now
uses `counter > tempo`, caps simulated increments at `tempo + 1` and assigns
the commit epoch after inclusion-block coinbase. Reveals run before coinbase;
the commit extrinsic runs after it.

For head 105, last epoch block 90, tempo 20 and counter 20, inclusion block
106 consumes an epoch before assigning the commit. The first reveal for
period one is 126. The old predictor returns 110; changing only the counter
predicate still returns 107. The original vector module remains, with an
added inclusion-phase vector. A follow-up commit uses checked budget and end
arithmetic, rejecting overflow before a potentially enormous search.

`generate_commit_v2_at(now, ...)` accepts an explicit clock; the existing
`generate_commit_v2` API delegates using the system clock. Pure public helpers
validate round arithmetic and encode the existing payload definition. The
simulator assumes successful future epochs and cannot predict inconsistent
inputs or per-block epoch-cap deferrals. Callers must follow the epoch
actually assigned to the included commit.

Rust 1.89 on macOS ARM64 passed the committed-HEAD pre-push gate:

- Seven portable schedule tests, including all original prediction vectors.
- Four clock/payload/API tests; the real encryptor returns rounds 245/246
  for injected timestamps and parseable existing-format ciphertexts.
- 71 all-feature library tests; one existing physical-OpenCL-GPU test ignored.
- Strict core/binding Clippy, portable wasm check and package formatting.

The [pre-push log](validation/pr-u5-pre-push.log) records these checks. Binding
compilation set `PYO3_NO_PYTHON=1`. Python oracle and live-runtime integration
evidence remain pending. The five-file change uses the existing Apache SDK
definitions; no GPL pallet source was copied. The task-local gate and cleanup
are documented in the [shared execution record](validation/protocol-epoch-execution.md).

The PR is open at the review SHA with no labels or review requests. Ten remote
workflows ended in `startup_failure`.
[Check Rust run 38556993032](https://github.com/RaoFoundation/subtensor/actions/runs/38556993032)
had zero jobs and zero check-runs. GitHub CLI suggests a workflow-file issue,
with no specific API diagnostic. Vercel requires authorization. No retry,
forbidden label, review request or deployment authorization was made.
Snapshots remain in `validation/pr-u5-*.json` and
`validation/pr-u5-rust-view.txt`; remote checks have not validated this PR.

The rtao-owned staged predictor must carry the same checked simulation-budget
guard. This upstream worker did not modify rtao product code or PLAN.md.
