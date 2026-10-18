# PR-U10: portable password codec and strict signature verification

Filed 2026-10-14: [RaoFoundation/subtensor#3250](https://github.com/RaoFoundation/subtensor/pull/3250).

| Item | Verified value |
|---|---|
| Base | `850f2755bb05a3851f948489f0ab27b0645cebce` (`main`, `v478`) |
| Fork and branch | `sam0x17/subtensor`, `sdk-pure-key-helpers` |
| Initial review commit | `c0208a82ab2a17a91c87654f3d6ebc0dc887cd20` |
| Published audit correction | `64f152725dda64c040e8f3c6435c08f450093a59` |
| Retained patch | [pr-u10.patch](pr-u10.patch) |
| Patch SHA256 | `269fc23b74651935e4b239d6a9cffdb48d81eeccc9d8de48554d318e4a47b2f0` |
| Scope | Nine files; 263 insertions, 25 deletions; core helpers, binding exports and regressions |

The independent audit found that the original decoder guarded its Vec only
after successful base64 decoding. The published follow-up installs the guard
before decoding, reserves the complete output bound to prevent sensitive
reallocation, and guards the host wrappers' owned environment Strings. It adds
two late-error regressions and preserves the compatibility format and behavior.
The remote branch and PR SHA, exact posted body, committed-HEAD current/MSRV
tests, pinned Clippy and wasm checks were verified. See
[the audit record](validation/quality-ir-keys.md) for evidence and cleanup limits.
The sections below retain the original filing's evidence; the nine-file scope
and patch checksum above describe that initial commit.

An ungated `keyfile_env` module exposes pure
`encode_password_for_environment` and `decode_password_from_environment`.
It preserves the legacy UTF-8 XOR/base64 format, whitespace handling and error
strings, with temporary buffers held in `Zeroizing`. Host `keyfiles` reexports
these helpers and delegates its existing get/save behavior. The compatibility
setter continues to change process state; callers can use the pure encoder
instead. Both helpers compile without the host feature and for wasm.

Public `keys::verify_strict` delegates to the existing private pinned
Substrate verifier. It verifies exactly the supplied message bytes, without
the public verifier's `<Bytes>` retry. Existing public verification APIs retain
their behavior. Thin binding functions and `bittensor.sp_core` exports expose
strict verification and pure encoding to Python.

The portable retirement target is
`keyfile_env::encode_password_for_environment` with no required feature, rather
than the host-only `keyfiles` alias. The pure decoder is included so the entire
staged module can retire coherently. `base64` and `zeroize` were already ordinary
core dependencies; no manifest or lockfile changed.

Rust 1.89 on macOS ARM64 passed the exact clean review HEAD through the
owned pre-push hook:

- Six regressions with default features disabled: strict raw/wrapped sr25519
  and ed25519 signatures, malformed input boundaries, fixed password vectors,
  decoder whitespace/errors and proof of no environment write.
- All-feature core library tests: 71 passed; one existing physical-OpenCL-GPU
  test ignored.
- Strict core/binding Clippy with all features and targets.
- Portable `wasm32-unknown-unknown` check, package formatting and
  committed-diff checks.

The pre-push log (not retained in the repository) started with the exact review
SHA. Python binding regressions are included for hosted CI only. No Python
interpreter ran for this filing; binding checks explicitly set
`PYO3_NO_PYTHON=1`. Upstream's guide requests the `unarbos` actor and full
monorepo preflight. The maintainer's account (`sam0x17`) and the project
decision to keep Python in CI take precedence. The hook checked actor, exact fork/branch,
clean HEAD, nine-file scope and all scoped Rust checks. No force push or
hook bypass was used. An empty task-local `clippy.toml` isolated upstream
checks from rtao consumer bans while retaining upstream Cargo lint rules.

The first SSH connection closed while the hook waited on the shared build
cache. All code checks passed, but the push exited 141 and the fork branch
remained absent. A normal push with SSH keepalives succeeded after repeating
the same full hook on the same commit. The bounded first transport log is not
retained in the repository.

Remote checks have not validated the code. Ten workflows ended in
`startup_failure`; [Check Rust run 38564404907](https://github.com/RaoFoundation/subtensor/actions/runs/38564404907)
had zero jobs and zero check-runs. GitHub CLI suggests a workflow-file issue
without a specific diagnostic. At that recorded check, the PR was open at the
initial review SHA with no labels or review requests. No workflow retry, `CI` label, subscriber review
request or deployment authorization was made. The PR body snapshot is retained
as [`validation/pr-u10-body.md`](validation/pr-u10-body.md); bounded state and
diagnostics are not retained in the repository.

The isolated checkout and task scratch directory were removed. The dependency audit found no live
process, open file, launchd unit, configuration, symlink or virtualenv
reference; 42 symlinks and 97 configuration/environment/Git files were
inspected. The scoped local-tree query found no open file. Exact-path cleanup
removed 3,160 files, 568 directories and one symlink, about 207 MB. Shared
pin/build/tool caches and unrelated work remain live.
