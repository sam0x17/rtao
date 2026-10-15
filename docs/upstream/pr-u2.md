# PR-U2: nested receipt failures and native fee fallback

Filed 2026-10-14: [RaoFoundation/subtensor#3247](https://github.com/RaoFoundation/subtensor/pull/3247).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-receipt-nested-failures` |
| Original receipt commit | `aadeb06bb517b24428f4d7f64a351ccc8765e502` |
| Published review HEAD | `849c192e9702fa245a6f16941359290786554d85` |
| Dependencies | PR-U1 [#3246](https://github.com/RaoFoundation/subtensor/pull/3246) and PR-U15 [#3243](https://github.com/RaoFoundation/subtensor/pull/3243) |
| Original patch | [pr-u2.patch](pr-u2.patch), original receipt commit only; apply after original U1 |
| Original patch SHA256 | `d407ea6ce223090cc0f429b139ebc9017da2219d2d5b17f858dc19a19ce727b8` |
| Corrective patch | [pr-u2-unicode15.patch](pr-u2-unicode15.patch), apply after the original U2 commit |
| Corrective patch SHA256 | `b081ddcb1aeac769ec2288136426c39df5ca9ecb50571da8503a9a8a89023400` |

A failed `Sudo.sudo` could produce a successful blocking-client receipt.
`Sudid` decodes to `{sudo_result: Err(...)}`, while the old resolver looked for
`result`, then tested the whole attribute map for `Err`. Outer
`System.ExtrinsicSuccess` hid the inner failure.

Ungated `protocol::receipt` resolves the six Python-supported nested failure
events and five MevShield failure forms. The blocking client uses it without
changing `TxOutcome` field types or semantic-code conversion. The last
`TransactionFeePaid.actual_fee` takes precedence; otherwise the resolver uses
checked Treasury/Balances deposit sums. Fee amounts must fit native `u64`
rao. Missing required fee, weight and outer failure attributes return an error;
nested wrapper discovery follows Python and skips absent result fields.

The principal regression encodes `Sudid(Err(BadOrigin))` followed by outer
success through Finney v478 V15 `System.Events` metadata and decodes the bytes
using the portable storage helper. The decoded attributes contain `sudo_result`
and no `result`; the resolver returns failure. Live M5 Sudo-wrapper evidence
remains pending.

All original U15/U1/U2 ancestry remains intact. U1 and U2 received equivalent
ordinary Unicode corrective commits on their existing branches without
rebasing. The full diff includes the original dependencies until they merge;
the retained patches distinguish the original receipt change and its correction.
The corrective patch removes exactly the Unicode dependency edge and 1.1.0
package, seven lockfile lines, restoring a lockfile byte-identical to v478.
The inherited formatter uses a generated Unicode 15 printable table matching
CPython 3.12.11 with the complete Unicode notice. The core license expression
is `Apache-2.0 AND Unicode-DFS-2016`. No GPL pallet source was copied.

Rust 1.89.0 and 1.99.0 on macOS ARM64 passed the corrected committed-HEAD gate:

- Four receipt regressions and 27 inherited portable tests on each compiler.
  One public formatter test covers 21 Unicode, quote, control and escape cases.
- 71 all-feature library tests on each compiler; one existing physical-OpenCL-GPU
  test remains ignored.
- Binding compilation with all features/targets on both compilers. The binding
  defines no Rust unit tests.
- Pinned Rust 1.89.0 strict all-feature/all-target core and binding Clippy.
- Portable wasm checks on both compilers, formatting, clean-tree and exact
  source/license/lock/scope guards.

The [corrected pre-push log](validation/quality-unicode-publication/pr-3247-pre-push.log)
and [publication record](validation/quality-unicode-publication/pr-3247-publication.json)
record the ordinary protected-hook push. Functional/wasm checks use a unique
compiler wrapper and separate disposable target per checkout; Clippy uses its
own separate target because it overrides the workspace wrapper. Strict current
Clippy is not claimed because unchanged base-file lints remain. All correction
Cargo checks set `PYO3_NO_PYTHON=1`; no local Python interpreter was used.

The PR and fork branch agree at the corrected SHA. The authorized body update
was fetched and verified; labels and review requests remain empty. Ten workflows
for this SHA ended in `startup_failure`, each with zero jobs. There are zero
check-runs; Vercel requires authorization. Hosted Python tests and live integration
remain pending. Snapshots are retained under `validation/quality-unicode-publication/`.

Original [pre-push evidence](validation/pr-u2-pre-push.log), original eleven-workflow
startup failures and diagnostics remain in `validation/pr-u2-*`. The
[original execution record](validation/protocol-epoch-execution.md) explains
the original gate and first cleanup. The Unicode publication's worktrees,
hooks and disposable targets were audited and removed separately;
[cleanup evidence](validation/quality-unicode-publication/cleanup-finished.json)
records exact paths and retained shared dependencies.
