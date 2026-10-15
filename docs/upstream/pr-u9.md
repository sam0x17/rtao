# PR-U9: shared portable metadata IR

Filed 2026-10-14: [RaoFoundation/subtensor#3249](https://github.com/RaoFoundation/subtensor/pull/3249).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-shared-metadata-ir` |
| Review commit | `0c4ed7585ae9d232577e53d45813f383e0ec2eec` |
| Retained patch | [pr-u9.patch](pr-u9.patch) |
| Patch SHA256 | `b15e1ee4b7d21f7f9bb0b7c16ab928a415758904863aab432c9fdad29fba3e2e` |
| Scope | Seven files: portable core, binding materialization, Rust regressions and one handwritten Python unit test |

The portable core now exports `ir::MetadataIr` and an inherent
`Runtime::metadata_ir()`. The Python binding delegates metadata extraction to
this method and only materializes dicts and lists. Existing fields retain
their names and shapes; additions expose storage prefixes/hashers/key
identities/defaults, indexed events and fields, runtime API signatures,
constant details and ordered signed extensions. SCALE bytes materialize as
integer lists, preserving JSON compatibility.

Extraction rejects broken referenced graphs and malformed enum or storage
shapes. Its iterative, cycle-safe graph walk accepts valid recursive types.
V15 API versions remain unknown until explicitly supplied; the optional V16
helper checks API and method sets and applies versions atomically. Registry
IDs remain local to the source runtime. No dependency, lockfile, generated
Python, pallet or runtime file changed.

Rust 1.89 on macOS ARM64 passed the clean committed-HEAD pre-push gate:

- Eight portable IR regressions, including the upstream pinned golden fixture,
  malformed nested references, recursive types, V14 and V16 version guards.
- All-feature core library tests: 71 passed; one existing physical-OpenCL-GPU
  test ignored.
- Strict core and binding Clippy with all features and targets.
- Portable `wasm32-unknown-unknown` check with default features disabled.
- Core/binding formatting and committed-diff checks.

The [pre-push log](validation/pr-u9-pre-push.log) records the review SHA's
checks. A Python materialization regression is included for hosted CI only.
No Python interpreter ran for this filing; binding checks explicitly used
`PYO3_NO_PYTHON=1`. Upstream's guide requests the `unarbos` actor and full
monorepo preflight. The user's authorized `sam0x17` identity and instruction
to keep Python execution in CI take precedence. The task-local hook checked
that identity, exact fork and branch, clean HEAD, exact seven-file scope and
the scoped Rust gate. No force push or hook bypass was used.

The first Clippy attempt inherited rtao's consumer-only method bans. An empty
`clippy.toml` in the task-local `CLIPPY_CONF_DIR` stopped parent discovery;
upstream Cargo lint rules stayed active. The corrected strict check passed.

Remote checks have not validated this code. Ten workflows ended in
`startup_failure`. [Check Rust run 38561149347](https://github.com/RaoFoundation/subtensor/actions/runs/38561149347)
had zero jobs and zero check-runs. GitHub CLI suggests a workflow-file issue
without a specific diagnostic. The PR is open at the review SHA with no
labels or review requests. No retry, `CI` label, subscriber review request or
deployment authorization was made. Bounded state, workflow and diagnostic
snapshots are retained under `validation/pr-u9-*`.

The isolated checkout and scratch under the recorded temporary root's
`tmp/upstream-metadata-u9/` directory were removed after the dependency audit.
The audit found no running process, open file, launchd unit, configuration,
symlink or virtualenv dependency on this directory; it inspected 41 symlinks
and 96 configuration/environment/Git files. An unrelated SMB backup mount
was unavailable to `lsof`; the scoped local-tree query found no open file.
Cleanup removed 3,155 files, 563 directories and one symlink, about 207 MB.
Its final result is recorded in [pr-u9-cleanup.json](validation/pr-u9-cleanup.json).
Shared pin/build/tool caches and other agents' directories remain live.
Retained hook/gate scripts document completed checks and reference the
removed task scratch; they are not installed hooks.
