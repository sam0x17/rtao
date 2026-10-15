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

## Regenerating compatibility inputs in CI

The workflow's manual `refresh_compat=true` mode runs one 20-minute Ubuntu
job and skips the six test jobs. It measures the existing Apple-target
dependency graph with `cargo tree`, records its actual Linux compiler host,
and generates compatibility source reports, followed by the declared parity
inventory. This is not a Mac build or native-platform coverage. It uploads
only four source reports for review (`COMPAT.md`, `compat.json`, the build-weight
JSON and `parity/matrix.json`), with three-day retention, and cannot push a commit. Normal pushes
continue running the unchanged checks against the committed source reports.

The first refresh job succeeded, but artifact review rejected its inflated
counts: CI's `CARGO_TERM_COLOR=always` decorated Cargo's `(*)` marker, so the
counter treated repeated rows as distinct package identities. Removing only
that formatting in an independent read-only check recovers the original
225/361/365/416 counts, with no added packages. The captured command now
forces `CARGO_TERM_COLOR=never`; the parser rejects terminal formatting,
and the existing package-identity regression includes the colored duplicate.
Regenerate the reports from the corrected producer before adopting them.

Corrected run [38682108313](https://github.com/sam0x17/rtao/actions/runs/38682108313)
produced the three compatibility files at `b57e851`. Artifact `11949090125`
matched its API SHA-256 digest
`d447a56f86dd40357e0d2c78991b9568e781fa73f04f349633f2eb3566eced1a`.
Review verified all 23 input hashes, the clean source identity, unchanged
package sets/counts and empty runtime-validation claims before byte-for-byte
adoption in `71f28d2`. Later CLI dependency changes need another hosted refresh.
The parity inventory was added to the refresh after this run; it declares
source status and does not certify test or oracle success.

The optional `refresh_codegen=true` input uses the same single 20-minute free
`ubuntu-latest` job and skips the six normal check jobs, whether or not
`refresh_compat` is also set:

```sh
gh workflow run pr.yml --ref main -f refresh_codegen=true
```

It first measures compatibility from the clean committed checkout, then runs
`cargo run --locked -p xtask -- regen` and refreshes the parity inventory.
The existing `compat-<head-sha>` artifact retains the four reports above. A
separate `generated-<head-sha>` artifact retains these four generated source
files for three days:

| Repository path | ZIP member |
|---|---|
| `crates/rtao-intents/src/generated/metadata.rs` | `rtao-intents/src/generated/metadata.rs` |
| `crates/rtao-intents/src/generated/mod.rs` | `rtao-intents/src/generated/mod.rs` |
| `crates/rtao-reads/src/generated/names.rs` | `rtao-reads/src/generated/names.rs` |
| `crates/rtao-reads/src/generated/mod.rs` | `rtao-reads/src/generated/mod.rs` |

The upload action uses the common `crates/` directory as its archive root.
After regeneration and parity reporting, a shared guard requires every expected
output to be a regular file and rejects changed or untracked source outside the
selected output paths. It uploads source only; it cannot commit or
push, and a successful refresh does not establish that normal checks pass.

Bootstrap this change by pushing the generator and workflow first, while the
committed generated API still compiles. `xtask` depends on `rtao-reads`, so a
root `ReadInterfaceMetadata` reexport added before the generated definition
exists prevents the generator itself from building. Adopt the verified
generated files before adding that root reexport and integrating callers.

Before adopting either artifact, require a successful run whose head matches
the intended generator commit and whose generation and upload steps passed.
Check each ZIP's SHA-256 against the artifact API's `digest`, reject extra,
missing, duplicate or nonregular members, and inspect the exact file set
before writing anything. Restore the generated ZIP's four members only at
their corresponding repository paths. Check the generated attribution and
declarations against that commit's generator and pinned catalog, then adopt
the bytes without hand edits. Verify the compatibility report's clean source
identity, Linux host, Apple graph target and current dependency-input hashes;
review parity declarations without treating them as test evidence. The
subsequent normal hosted checks must compile the public metadata APIs, test
feature availability and pending behavior, and pass `regen --check` against
the adopted output.

The CLI refresh [38683267312](https://github.com/sam0x17/rtao/actions/runs/38683267312)
succeeded on `642623b`. Four-file artifact `11949225713` (39,662 bytes)
matched SHA-256 `6ceb152f7febcc4ce40bf56eec777a4e2fc8a8351d5dc849fbe03cc0b37d7e22`.
All 23 current input hashes and the unchanged package inventories were checked.
The parity matrix changes only by adding D-041/D-042 with unverified oracle
status; compatibility validation remains null/empty. The four outputs were
adopted byte for byte. Normal check acceptance remains separate.

## First complete hosted test enumeration

Run [38684286794](https://github.com/sam0x17/rtao/actions/runs/38684286794)
at `0fbf9bc` passed MSRV, features, no-host, supply and semver. The main job
passed its source, generation, Clippy and dependency gates, then ran 1,678 tests:
1,659 passed and 19 failed, with four other tests skipped. Doctests did not run
because nextest failed. This is not M0 acceptance.

Eighteen owner-pruning tests required `RTAO_TEST_TMPDIR`; the runner supplied
only `RTAO_TEMP_ROOT` and `TMPDIR`. The shared gate environment now points the
test variable at the same disposable `tmp` directory. The remaining failure
was the CLI PSF mutation fixture: it appended a second full NOTICE section,
so mutating one copy left another valid copy. The fixture now appends the
section only when absent, verifies its initial validity and requires exactly
one copy before mutation. The production attribution gate is unchanged.
Both repairs need the next hosted test run; no local build or test was run.

Hosted generation [38686769925](https://github.com/sam0x17/rtao/actions/runs/38686769925)
succeeded at `f841f63`. Generated artifact `11950657025` matched SHA-256
`910f698e1dfc27bb65e39cb4f4ed5f7471b9aae09fba1a4a1c0710c91f88b906`;
report artifact `11950581955` matched
`2b14fcf0ddba8b35e66310d08a56d4a4650bbb22ea3e12bdbd476e060e70b664`.
Review validated exact four-file sets, all 83 intent and 102 read names, pending
flags, 23 current dependency hashes, clean source identity, unchanged profile
package sets and empty runtime-validation claims. All eight files were adopted
byte for byte in memory, with no extraction directory. The bootstrap normal
no-host job correctly rejected the old generated read names before adoption;
the combined source still requires a normal hosted run.

Follow-up [38687383056](https://github.com/sam0x17/rtao/actions/runs/38687383056)
succeeded at `48cf4ee`. Artifact `11950428392` (39,852 bytes) matched SHA-256
`28bc70d1aeac58049c6b7628a95159256e34fba0198c500cc2284232aa6050f4`.
Exact-path, input-hash and package-set review passed before adopting the four
reports in memory. The 1,537-row matrix adds only D-043/D-044; their oracle
status remains unverified. Runtime validation stays empty. The main job now
retains its hosted compiler/tool cache on test failure to avoid repeating cold
builds; all checks and the 45-minute bound remain unchanged.
