# PR-U1: portable protocol and native runtime fee queries

Filed 2026-10-14: [RaoFoundation/subtensor#3246](https://github.com/RaoFoundation/subtensor/pull/3246).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-portable-protocol` |
| Original protocol commit | `9e926d1f2f4943f11a23ef13ff517e7857e9cc27` |
| Published review HEAD | `77480991b2b3b39ec828d2da9d7c6ecf31113a81` |
| Dependency | PR-U15 [#3243](https://github.com/RaoFoundation/subtensor/pull/3243), cherry-picked as `fb8274680b5287d8c4e82e77bfe9506d4946a362` |
| Original patch | [pr-u1.patch](pr-u1.patch), original U1 commit only; apply after U15 |
| Original patch SHA256 | `6c11672622c06846dfccc0185b5c08f675cb50d34a0d2285eee8b269f11e4e5b` |
| Corrective patch | [pr-u1-unicode15.patch](pr-u1-unicode15.patch), apply after the original U1 commit |
| Corrective patch SHA256 | `9b02efc8573031160e597ccff563e7bf44e01b6dfc3cded8592b97b478dfbffd` |

The blocking client previously kept request construction and SCALE decoding
private. The ungated `protocol` module exposes metadata, storage,
runtime-API, extrinsic-lookup, dispatch-error and fee helpers, plus `RpcRequest`.
The blocking client uses these helpers, so transport-independent callers can
share the same request and decoding behavior.

Fee and weight estimates encode an immortal extrinsic with a zeroed
signature, append its SCALE `u32` length and call
`TransactionPaymentApi_query_info`. The prior path signed a mortal extrinsic
and called `payment_queryInfo`. The response decoder uses live V15 runtime
API metadata, rejects trailing bytes and checks that the fee fits native
`u64` rao. The public `estimate_fee` return type remains `u128`.

The full diff includes U15 until that dependency merges. Both published
ancestors remain intact; the Unicode correction is an ordinary follow-up
commit. The original patch retains its historical dependency addition.
The corrective patch removes exactly the `unicode-general-category` edge
and 1.1.0 package, seven lockfile lines. The final lockfile is byte-identical
to v478. Diagnostic strings use a generated Unicode 15 printable table
matching CPython 3.12.11, with the complete Unicode notice. The core license
expression is `Apache-2.0 AND Unicode-DFS-2016`.

The Finney v478 V15 fixture is public metadata with no account secrets.
No GPL pallet source was copied.

Rust 1.89.0 and 1.99.0 on macOS ARM64 passed the corrected committed-HEAD gate:

- 27 portable tests: 13 protocol, five fee/error and nine navigation tests.
  One public formatter test covers 21 Unicode, quote, control and escape cases.
- 71 all-feature library tests on each compiler; one existing physical-OpenCL-GPU
  test remains ignored.
- Binding compilation with all features/targets on both compilers. The binding
  defines no Rust unit tests.
- Pinned Rust 1.89.0 core and binding Clippy with all features/targets and warnings denied.
- Portable wasm checks on both compilers, formatting, clean-tree and exact
  source/license/lock/scope guards.

The [corrected pre-push log](validation/quality-unicode-publication/pr-3246-pre-push.log)
and [publication record](validation/quality-unicode-publication/pr-3246-publication.json)
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

Original [pre-push evidence](validation/pr-u1-pre-push.log), original eleven-workflow
startup failures and diagnostics remain in `validation/pr-u1-*`. The
[original execution record](validation/protocol-epoch-execution.md) explains the
earlier Python-discovery mistake and first cleanup. The Unicode publication's
worktrees, hooks and disposable targets were audited and removed separately;
[cleanup evidence](validation/quality-unicode-publication/cleanup-finished.json)
records exact paths and retained shared dependencies.
