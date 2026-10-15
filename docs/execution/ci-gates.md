# M0-27: hosted gates

Q30 supersedes this document's original local verification procedure. Full
gates, MSRV, feature matrices, supply checks and substantial tests run in
GitHub CI on free `ubuntu-latest` runners. Local full gates and prerequisite
tool installations are not required before a push. Required hosted checks
must pass before merging. No hosted success is inferred from local results.

`.github/workflows/pr.yml` defines `check`, `no-host`, `msrv`, `features`,
`supply` and `semver` on free Linux x64 `ubuntu-latest`. Pushes to `main`,
pull requests and merge groups trigger it. Superseded runs for the same ref
are cancelled; each job has a 45-minute timeout. Its token has read-only
contents permission.

This is the only active workflow. The former node/container, scheduled,
release and upkeep workflows are retained as inactive reference drafts in
`.github/deferred-workflows/`. They cannot launch from pushes or schedules.

GitHub executes the job definitions in `xtask/src/gates.rs`. The existing
gates command remains a CI entry point; do not invoke it as a local pre-push
routine. Even `cargo xtask gates --list` can first compile xtask on a cold
machine, so inspect the source when only the command list is needed. A
blocking hosted failure stops its job. The approved Q26 built-dependency BLS
criterion and Q27/Q28 guarded dormant advisory exceptions remain in force.

The previous execution root `target/execute-plan.6Ro0PC` is being retired by
the disk cleanup recorded in `state.md`. Do not recreate its caches or
restart its queued commands. Local work follows `AGENTS.md` and
`CONTRIBUTING.md`; no local node, container, oracle, soak, device rehearsal or
release build is required. The implementation details and validation entries
below describe the earlier execution, not instructions to resume it.

Gates never installs a Rust toolchain, changes system settings or runs Python,
Docker E2E targets or the separate Keychain harness. GitHub installs Rust
1.89 on its disposable MSRV host; it is not a local prerequisite. The `ci` nextest
profile excludes `e2e`, `interop`, `core_oracle` and `ledger_emu`; doctests run
separately. All test/build/scratch outputs and advisory database rehearsals
stay under `RTAO_TEMP_ROOT`.

`ci/tool-versions.toml` pins nextest 0.9.143, cargo-hack 0.6.45, cargo-deny
0.20.2, cargo-audit 0.22.2 and cargo-semver-checks 0.51.0. Matching existing
executables are reused; a missing or different release is installed with
`cargo install --locked --version` into the task root's `ci/tools`, with its
build in `ci/install-build`. Those tools follow stable; the facade and CLI
alone are checked with `cargo +1.89`.

The commands cover the architecture's role profiles, each facade and intent
feature and the full btauth/signer feature powersets. cargo-hack's
`--no-dev-deps` rewrites manifests and makes Cargo prune dev-only lock entries.
Combining it directly with the architecture's `--locked` command fails once
the workspace has dev dependencies. Gates therefore copies actual tracked
and untracked repository source into `ci/features-worktree-<pid>`, removes
dev dependencies there, resolves its graph offline and verifies that every
retained lock package has exactly the original name, version, source and
checksum before running the unchanged locked cargo-hack commands. Role
profile checks still run against the original workspace. The original
manifests and lock are never handed to cargo-hack for mutation.

M0-27 explicitly defers `regen --check` to M1-04. It becomes required as soon
as `regen.rs` appears or M1-04 is complete. Real validator, miner and owner
examples enter the features job at M2-18, M3-06 and M4-17; an example is checked
as soon as present, and a completed step with a missing example is rejected.
No placeholder example is counted as coverage. Semver does no comparison or
tool installation until a reachable `rtao-v*` baseline exists. Comparison
failures are informational before 1.0 and block from 1.0; malformed baselines
and tool setup failures always fail.

## Action pin evidence

Verified through each repository's GitHub Git reference and commit APIs on
2026-10-14, including peeling the annotated rust-cache tag:

| Action | Release | Immutable commit |
|---|---|---|
| `actions/checkout` | `v7.0.1` | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| `actions-rust-lang/setup-rust-toolchain` | `v2.0.0` | `ecabd13d1c56bd1345c230e542e9144811ad706f` |
| `Swatinem/rust-cache` | `v2.9.2` | `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` |

Rust setup's built-in cache is disabled; the workflow invokes the explicit
pinned cache action. Its checked composite implementation also references
that same rust-cache commit. All action references use actual immutable SHAs.

## Historical validation

- Seven gate tests pass, covering job selection, stopping on blocking failure,
  MSRV/profile scope, semver failure policy, future-step enforcement, exact
  tool versions and no-dev lock provenance (including rejection of changed
  versions, sources, checksums or added packages). xtask Clippy passes with
  `-D warnings` for the prepared M0 module set. A later rerun stops at
  independent M1 extraction work that declares `extract_resolved.rs` before
  creating it; final validation waits for those writes to finish.
- actionlint 1.7.12 validates the real workflow. Its downloaded Darwin ARM64
  archive matched the release checksum
  `aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f`.
  Ruby's YAML parser also confirmed all six free-Ubuntu jobs.
- The actual `check` job verified all 360 upstream artifacts, then stopped at
  the unchanged raw BLS lock failure (`found 2`). Commands after that failure
  have not been proven by this job run.
- The complete features job passes: no-dev snapshot provenance, all 30 facade
  feature checks, all 25 intent feature checks, all 48 combined btauth/signer
  powerset checks, all nine role combinations and no facade features. The
  role/no-feature checks use the actual original workspace. Real profile
  examples are still due at their named later steps.
- The complete MSRV job passes: the no-default combined validator/miner/
  prometheus profile, full facade and CLI all check on Rust 1.89. It exposed a
  Rust 1.89 compiler error in alpha-balance struct-update closures; explicit
  field construction fixed it without changing the MSRV.
- Both actual `no-host` profile builds pass; its dependency step stops while
  xtask's independent `extract_resolved.rs` module is still unwritten. The
  remaining dependency and Clippy checks require that source write to finish.
- The complete `supply` job passes after version-scoped license clarifications
  for pinned `procedural-fork` and `subtensor-linting`. cargo-deny verifies the
  actual root Apache-2.0 license bytes; the procedural fork retains its Parity
  Apache-2.0 headers. Advisories, bans, licenses and sources all pass. The
  pinned cargo-audit reports no vulnerabilities and 14 allowed maintenance
  warnings. No advisory ignore or package exclusion was added.
- `semver` before the first tag succeeds without installing its tool or
  inventing a baseline. No hosted workflow result is claimed before a push.

The historical local results above do not establish current hosted success.
Use the hosted jobs for remaining validation. Retain bounded results and
remove task-created scratch after checking live dependencies; retired local
build caches are disposable.

The completed rehearsal's `ci/actionlint`, `ci/install-build` and
`ci/features-worktree-49303`, `ci/features-worktree-57984` and
`ci/features-worktree-13309` were removed after terminal-session/process and
open-file checks, user/system launchd checks, configuration searches, workspace
symlink-target inspection and virtual-environment discovery found no live
dependencies. The pinned tools, advisory cache and shared build cache were
retained during that earlier verification. Q30's completed cleanup has since
removed the retired task root in full.

## First hosted runner findings, 2026-10-15

Checkpoint `450c914335daa6a5da76a6c30908186e84f05e6e` starts the real six-job
workflow at https://github.com/sam0x17/rtao/actions/runs/38680556857.
The check and no-host jobs compiled the native client but exposed missing
source caches before their offline graph checks:

- The upstream core projection follows upstream's separate lock, which needs
  `bitcoin-internals` through an older `bitcoin_hashes` than the workspace.
  A workspace-only fetch cannot prepare that graph. The check job now calls
  `pin --check --prepare-cache` to fetch the exact committed core/parity
  projection before the unchanged offline `pin --check`. The mode cannot
  change pins or combine with release/network/candidate modes.
- The no-host job compiled its two selected profiles, then its all-target
  dependency audit needed optional `aes-ctr` sources not used by those builds.
  It now fetches the locked workspace graph before running the audit.

These repairs prepare disposable hosted caches; they retain all source,
identity, lock and offline checks. Formatting and source review pass. Their
current Rust tests and the complete jobs still need the next hosted run.
