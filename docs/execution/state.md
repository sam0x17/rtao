# Development state

## Current scope and resource policy (Q30, 2026-10-14)

Current work is client development with substantial validation in free
`ubuntu-latest` GitHub CI. Follow PLAN.md Q30, AGENTS.md and CONTRIBUTING.md.
The earlier continuous local validation campaign is retired. Do not restart
its jobs or recreate its build roots. No local node, testnet, container,
soak, device/Keychain rehearsal, full gates, extra toolchain or release build
is required. Historical successes below do not establish current CI success.

Only `.github/workflows/pr.yml` remains active. The eleven earlier
infrastructure, scheduled, oracle and release workflows are inactive drafts
in `.github/deferred-workflows/`. No GitHub workflow was dispatched during
this cleanup, and no current hosted success is claimed.

## Current client checkpoint, 2026-10-15

Canonical source implements 20 of 83 intents and 38 of 102 reads. It includes
M0-M4 foundations and the feature-harness repair; prepared owner/CLI/offline
candidates remain in the private source recovery archive until reviewed and
integrated. This source checkpoint is being sent to the first hosted CI run.
No current all-job success or milestone exit is claimed.

The plan's license expression list now includes the already required PSF and
Unicode clauses. This clarifies Q1's existing obligations; implementation and
license guards are unchanged. Private recovery is durably excluded from Git
and Docker contexts.

## Disk cleanup completed

Removed the entire task-created
`~/workspace/rtao/target/execute-plan.6Ro0PC` directory. Its measured
allocation before cleanup was 335,709,460 KiB (320.16 GiB). The shared Cargo
cache and fifteen additional compiler target directories were removed.
Tools, copied caches, source clones, disposable fixtures and raw logs were
also removed after checking running processes/open files, launchd/config,
symlinks, virtual environments and Git worktree/alternate dependencies.
No task build, test or node process remains. No remote scratch was created.

Canonical authored source remains in this repository. Unfinished source,
172 patches and handoff metadata are retained privately in
`.local/recovery/2026-10-14/unfinished-source.tar.gz` (152,583,783 bytes).
The archive's 18,888 members were checked against the recovery selection.
Its SHA-256 is
`770d9d50a9c855e61a4e7476c91caef52b6d0161c297c996a9fdf46c0852969b`.
The recovery directory is excluded locally from Git; its README and source
manifest describe recovery. Preserving a candidate does not adopt or
validate it. See [disk-cleanup.md](disk-cleanup.md).

The pre-existing `target/debug` is preserved; the remaining complete target
directory is 547,192 KiB (534 MiB). Shared Cargo/toolchain caches, other
projects and all wallets/recovery state are untouched. Retained durable
wallet/recovery data: `~/.local/share/rtao/testnet-soak`. No service
or task still depends on the deleted temporary root. backup snapshots
are untouched; deleted file allocation is not a promise of an equal immediate
increase in physical free space.

## Historical execution record

The entries below describe the earlier implementation and validation work.
References to active sessions, live scratch caches, outstanding infrastructure
prerequisites or local validation requirements are historical and superseded
by Q30 and the cleanup above. Referenced raw scratch logs were discarded;
bounded repository evidence remains. Preserve the recorded test limitations.

## External prerequisites and the user's 2026-10-14 response

M0-01 requested PLAN.md §12.13's prerequisites. The user answered:
"I can provide testnet tao to whatever wallet address you need; just use my
github; we will skip sophisticated github runners, I want to just use
ubuntu-latest free ones; I don't see why we need github pages, that seems
overkill; I have a ledger we can use" (PLAN.md Q23).

Use `sam0x17` for GitHub and `ghcr.io/sam0x17/rtao-oracle` for the oracle
namespace. All hosted jobs use free Linux x64 `ubuntu-latest`; no paid,
large, ARM-native or macOS hosted runners are required. Docs remain complete
and ship as downloadable rustdoc/book release assets without Pages.

Q29 waives the 24-hour testnet soak and its dedicated funding, subnet setup and
host prerequisites. No funding request, setup transaction or deployment is needed.
Preserve the prepared local wallet and recovery state. The user's Ledger is
available for manual device runs when the checklist is ready; actual run
results and model/app compatibility remain to verify.

Preserve full parity and all release targets. Linux builds need a fixed,
digest-pinned Ubuntu 22.04 container for the glibc 2.35 floor; ARM cross tooling
and resources must be validated and never described as native test coverage.
Mac Studio builds/checks use the recorded temporary root. Keychain parity
requires a separate isolated harness that restores the default keychain and
search list on every exit, preserving user secrets; `cargo xtask gates` changes
no host settings. Equivalent Mac checks and trustworthy provenance for local
Mac artifacts remain unresolved until verified; drafts publish only after all
targets, checksums and required provenance pass. Mainnet-clone feasibility on
free runners also needs measurement; exceeding resources leaves the host
prerequisite pending without authorizing paid or large runners.

Independent execution continues under rule 6; no step is checked merely by
this decision, and no repeated namespace/runner/Pages permission is needed.
PR-U17 was filed against current upstream main using the recorded prototype
and its prior emulator evidence; validation is in `docs/upstream/pr-u17.md`.

## Current verification

The user explicitly approved Q26's BLS build-graph criterion, Q27's exact
dormant lru exception and Q28's exact dormant ruzstd exception. The reviewed
Q28 patch is applied; activation, unknown-entry/advisory and changed-range
regressions are retained. The full supply job passes against the final
canonical locks: cargo-deny passes, both raw audits remain nonzero/visible,
and all 53 dormant graph checks pass. Current Rust and Rust 1.89 pass the
12 guard tests and the gate regression; fresh isolated Clippy passes.
Evidence: `supply-ruzstd.md` and `ci/supply-24734` under the recorded root.
The complete gates still have to pass
before any commit or push. Initial oracle staging reports pending/M2-05 and
retains full verification once real artifacts exist. No rtao commit, push,
milestone tag or completed milestone is claimed. The active goal continues
through M9.

Native Q25 balances use checked u64 rao and distinct TAO/alpha units. Native
properties, input rejection, JSON regressions, Rust 1.89 and strict Clippy
pass; codec replay covers 2,899 samples. Signed authentication nonces are a
separate HTTP protocol domain. Evidence: `primitives.txt` and `btauth.txt`.

M1 source generation, model/shape checks, transport, read-only live probes,
fuzzing, release preparation and feature checks have bounded evidence in
`m1-*.md`. Compatibility ranges remain null without authenticated E2E and
interop reports. Hosted lanes and release provenance remain required.
Pre-existing `target/debug` is preserved.

M2 transaction execution, generated interfaces, fifteen read behaviors,
validator scheduling, CI-only oracle replay and pin automation are integrating.
Current and Rust 1.89 intent library runs pass, including six real FakeChain
weight behavior tests and two registry tests. The validator framework passes
forty-one current/MSRV tests and its compile-fail doctest, with strict Clippy.
Canonical extraction, full regeneration, whole-workspace checks and hosted
oracle/localnet/image runs remain in progress. Exact counts can change while
owners add reviewed regression cases; retained logs identify each source run.

The no-host validator image and standalone TLS probe are authored. Probe
compilation, MSRV, strict Clippy and workflow lint pass. No local Docker daemon
is available, so no actual container/image run is claimed. The explicit
`--synthetic-weights` option now supplies protocol-test scores; a real subnet
application must supply its scoring algorithm. Localnet acceptance and observed
reveals remain required exits. Q29 waives the funded testnet soak.

Filed upstream contributions: U17 #3242, U15 #3243, U14 #3244, U12 #3245,
U1 #3246, U2 #3247 and U5 #3248. Their evidence is in `docs/upstream/`.
Security issues #3251 (Ed25519 erasure) and #3252 (secret URI diagnostics) are
also filed. With U9/U10 below and Zondax #295/#296, the live inventory is eight
PRs and five issues, all open on 2026-10-14. The user requested an independent
accuracy and fix-verification audit of every filing; that audit is active,
and does not imply hosted, physical-device or full-preflight validation.

The audit's Rust review workspaces share the task build cache. An unwrapped
repeat reused a foreign workspace test binary and is excluded as evidence.
Final review runs use a distinct no-op `RUSTC_WORKSPACE_WRAPPER` per checkout;
Cargo includes the wrapper path in workspace artifact hashes. These wrappers
and any corrected review worktrees remain live dependencies until validation
and authorized publication finish. The original semantic probes and all final
accepted runs are identified separately in `docs/upstream/validation/quality-*.md`.
U9 #3249 is filed; its committed-HEAD Rust validation passed. Its ten hosted
workflows failed at startup without Rust jobs. Its exact 207 MB task scratch
was audited and removed. U10 #3250 is filed with portable password/signature
exports. Its password cleanup correction is published at
`64f152725dda64c040e8f3c6435c08f450093a59`; the remote and PR SHAs matched, and the
updated body was verified. Tests on that committed HEAD passed on Rust 1.89/1.99:
eight portable and 71 all-feature library tests, with one existing GPU ignore.
Portable wasm checks and strict Clippy on upstream's pinned Rust 1.89 toolchain
also passed. Rust 1.99 Clippy reports two style lints in unchanged base files. All ten
hosted workflows for the correction failed at startup with zero jobs; no hosted
Rust or Python success is inferred. The review checkouts, Clippy targets and
disposable proof files were audited and removed; compiler wrappers and shared
caches remain live. Evidence is in `docs/upstream/validation/quality-ir-keys.md`.
During U1 validation, the first binding check invoked local Python
through PyO3 discovery, contrary to the CI-only instruction. No oracle,
recorder or corpus ran locally. The incident is recorded in
`docs/upstream/validation/pr-u1-python-discovery-incident.txt`; later checks
and the workspace default set `PYO3_NO_PYTHON=1`.

Completed workers audited and removed their isolated fixture, filing and
rehearsal directories. Shared build, tool, upstream-source, advisory and fuzz
caches under the recorded execution root remain live dependencies. Root
integration logs also use that root. Audit processes, launchd, configuration,
symlinks, virtual environments and Git dependencies before exact cleanup.

M2-22 pin automation is prepared: isolated candidate preparation/adoption,
force replay with unchanged authoritative snapshots and empty report, native
porting/auto-bless integration, read-only upstream-next, and main-image canary
smoke. Twenty-two pin/pipeline/normalization tests pass on current Rust and
Rust 1.89; nine original recorder replay tests and strict Clippy pass.
Workflow/actionlint and shell syntax checks pass. Main/testnet previews both
resolve the pinned v478 commit and fingerprint with no reported changes.
A local force run refuses before candidate/container execution and preserves
pins. Evidence is in `m2-pin-pipeline.md`, `m2-pin-validation.txt`, and the
three retained pin-plan/upstream-preview JSON files. Authenticated hosted
oracle/recording output, current-spec replay, e2e/canary/interop, and actual
force pipeline evidence remain pending; no milestone exit is claimed.
Task-owned test/tool directories were audited and removed. Shared build and
source caches remain live for root integration. Q26 subsequently corrected the
BLS lock/build criterion; complete pin-force and hosted evidence remain pending.

## M3 preparation and final integration

M3 authentication/server, miner runtime, axon publication, miner metagraph sync
and persistence have targeted source/test evidence. The miner guide and the
validator/miner cargo-generate template are prepared. The complete native
template preflight passes: real generation, Rust 1.89 Clippy, both binaries,
integration-executable linking, consumer doctor, and audited project cleanup.
Its result explicitly records native_only=true and milestone_acceptance=false.
The complete book also renders with checksum-pinned mdbook 0.5.4. Generated
localnet/image and hosted acceptance remain required. The local security source review is recorded
in `docs/security/m3.md`. No M3 completion checkbox or release is claimed.

The independent filing audit found three required code corrections. Ledger #296
now contains the price-unit correction at 8645ebdc31d188a722e22396d26fb52144246da4,
with all 5,604 committed-head native tests passing. The guarded password decoder
correction is published as recorded above. Both Unicode follow-ups are
published and remote/body verified: #3246 at
77480991b2b3b39ec828d2da9d7c6ecf31113a81 and #3247 at
849c192e9702fa245a6f16941359290786554d85. Their complete isolated outgoing
current/MSRV, binding-compile, wasm and pinned strict Clippy gates pass. They
use pinned Python 3.12/Unicode 15 data: native independent comparison
covers all 1,112,064 valid scalars, finding 80 extra digits and 5,812 printability
differences in the old Unicode 16 dependency. Root now pins category 0.6.0 and
uses authenticated printable data in the staged formatter. Full required Unicode
notices are retained in source, NOTICE and docs/licenses. Release archive,
standalone asset, checksum/provenance and validator/template image source
integration is prepared and verified; no tagged release or actual image run is
claimed. The audit does not claim hosted Python or device
acceptance. Exact source and proof identities are in quality-unicode.md.

Canonical import now retains the complete registration/uids source files for
pruning provenance outside the watched parity file map. Import/verification and
nine importer regressions pass. Extraction, regeneration and nonstrict parity
checks pass with the explicit first-oracle pending state. Runtime/build dependency
restrictions remain enforced: base64/zeroize match the pinned core, extrinsic
hashing delegates to its API, and tests obtain RPC types through rtao-testkit.
The current dependency check passes against final canonical manifests and
locks after removing direct test/tool imports and routing offload through the
client layer. Extraction, regeneration and the actual strengthened Unicode
attribution/parity gate pass with 1,532 matrix rows and the explicit pending
first oracle. Current-source ASan/coverage fuzzing passes all eight targets,
with 1,379,172 executions; retained evidence describes its fixed-clock and
no-decryption limits. The complete post-correction integration/gates runs
remain in progress.

An additional read-only packaging review identified missing base and CPython
notices in binary/image distributions. Source retains all of them; prepared
archives and images previously included only the certificate and Unicode
data notices. The full six-notice repair is prepared in the release/template/
image guards, with source-bound NOTICE and exact immutable license checks.
Current and MSRV release tests (10 each), template tests (4 each) and isolated
strict Clippy pass, along with shell/image-notice rehearsals and workflow checks.
The root NOTICE now carries the existing CPython parser modification summary.
Current release evidence establishes the earlier data-notice scope; the
expanded bundle still needs final full workspace gates. Evidence is retained
in `release-base-notices.md`; all owned disposable fixtures/targets were
audited and removed.

M4-01's reviewed implementation, checked fixed-point-scale repair and four-file
oracle recipe are adopted in the canonical worktree with exact before/after
hash checks. The generated table has 48 descriptions and 42 storage rows;
all ten methods share one implementation. Native u128 fixed-point
hyperparameters remain separate from Q25's checked u64 balances. Repaired
isolated affected-package tests pass 151 cases on each compiler; the oracle's
21 native tests and strict Clippy pass on both. The hosted recipe has 7,224
inputs but no locally produced Python outputs or hosted acceptance.
Canonical current/MSRV affected-package suites passed 151 each; final server
regressions and staged receipt/Unicode tests passed. Extraction, regeneration
and parity/dependency checks passed through `tmp/root-m4-01-integration-checks.sh`.
The stale build-weight report was remeasured from actual graphs and compatibility
outputs regenerated; repeated full gates passed those checks and remain live
at workspace test enumeration (exec session 7448). Samples identify delayed
new-executable startup in macOS dyld before Rust main, also observed by isolated
workers. No host security setting or gate was weakened; no complete gates pass
is claimed. Evidence counts/hashes are in `m4-hyperparams-validation.json`.
Proof and live snapshots remain under `m4-hyperparams-work` and
`m4-hyperparams-oracle-work`; finished disposable builds were audited/removed.
Evidence: `m4-hyperparams.md`. No M4 completion or milestone exit is claimed.

The previous canonical full gate attempts caught three integration defects:
the authored Unicode notice followed the generated importer block; a private
btauth authentication error response triggered current Clippy's large-error
lint; and two staged calls used a method form rejected by the pinned upstream
lint. Root moved the complete notice before the unchanged generated block,
boxed the private response, and called the existing checked integer helper
directly. The corrected importer and upstream lint pass, workspace Clippy
passed after the response repair, and all 14 current server tests pass.
Initial failure logs remain under `tmp/root-final-*`. The current full gate
run still must finish; those narrow successes do not establish complete gates.

Root adopted M4-02 owner planning, M4-03/04 shield/raw/multisig submission,
M4-05 CPU PoW, all 11 M4-06 owner/registration intents and the 22 M4-07 reads
across 66 source paths. The composition includes the final multisig runtime-cap
checks, D-040's mutable start delay and the nested dynamic-decoder repair.
All baseline and adoption hashes matched; the existing Q29 and license/NOTICE
changes were preserved. Regeneration completed on the combined source,
reporting 20 implemented intents and 38 implemented reads. This counts source
behaviors, not authenticated oracle acceptance. Combined matrix, extraction,
regeneration checks, dependency measurement and affected current/MSRV tests
run through `tmp/root-m4-combined-checks.sh`. The matrix (1,533 rows), extraction,
regeneration, dependency measurement, dependency and parity checks and
formatting pass. The current-Rust affected run exits successfully with 810
native tests and three doctests; its one hosted oracle replay remains ignored.
Rust 1.89 also passes 810 native tests and three doctests with the same hosted
replay ignored. No complete-workspace gate success is inferred
from this subset. Details: `m4-integration.md` and the bounded
manifests under `root-m4-integration/` in the execution root.
Owner orchestration/monitoring (M4-08) and CLI machinery (M4-09) are being
prepared in separate source snapshots while these checks and the feature
harness repair run. Live snapshots/proof remain until adoption and audited
cleanup. The finished 3.7 MiB root composition rehearsal was removed after exact
dependency audits; the reversible source backup and active build cache remain
live. No M4 completion or milestone exit is claimed.

Read-only testnet readiness evidence and the concrete wallet/setup requirements
are recorded in `testnet-prerequisites.md`. Host and existing-versus-new subnet
information were requested; the user selected a new subnet and
`maintainer@build-host`, with low compute use because the host is shared.
Read-only SSH host inspection passed. Builds and PoW stay off that host;
the planned validator has two Tokio worker threads, lower scheduling priority,
resource measurements and bounded log retention. The reviewed native wallet
helper passed seven fixture tests, including 24 altered complete-state cases,
native encrypted recovery and no-replacement failures. Root audited source,
binary/artifact hashes and dependencies, then prepared the new local wallet and
verified identical public-only reinspection. The coldkey funding address is
`<testnet-coldkey>`.
Wallet/recovery data are retained at `~/.local/share/rtao/testnet-soak`
and excluded from scratch cleanup; `testnet-wallet-public.json` contains only
public identities, paths and lifecycle state. No private material was printed.
No chain transaction, remote wallet copy or host service has run. Q29 now waives
the 24-hour testnet soak and its funding, chain setup and host prerequisites;
quote preparation and deployment are stopped. Existing unrelated user wallets
are untouched. All automated validator, localnet, image, oracle and interoperability
checks remain required.

The user explicitly reiterated cleanup of task-created Mac Studio directories
when finished. Finished owned builds/fixtures are dependency-audited and removed;
active build roots, source snapshots and retained wallet/state are recorded live
dependencies. Remote host use so far is read-only SSH inspection. Current local
build host is `workstation.local`; the selected remote soak host is
`build-host.local`. No remote scratch directory was created by the
host inspection or wallet preparation.

One formal decision remains pending: correct M0 exit 9's literal license
wording to include the required retained PSF and Unicode clauses while
preserving Q1's dual license. The proposed text and strengthened attribution
gate are in `third-party-license-gate.md`; actual canonical parity and all
19 attribution regressions and strict xtask Clippy pass on both current Rust
and Rust 1.89. The existing BLS and dormant advisory
decisions are already approved and applied. No main push or milestone exit
will bypass the pending criterion decision or full gates.

Q29 is recorded in PLAN.md and the architecture. The mandatory 24-hour testnet
soak is removed; no soak success is claimed. Its dedicated funding, subnet setup
and host deployment are stopped. All automated acceptance remains required.

The full workspace test run completed after macOS executable startup delays:
1,179 passed and five failed, with four separate skipped tests. The miner fixture
failed when native roots were unavailable; four RPC deadline/cancellation tests
failed during synchronous native-root enumeration for plaintext-only endpoints.
Both independently reviewed repairs are now adopted. Current/MSRV affected RPC
suites each pass 80 tests and the miner integration test passes; strict Clippy
passes both toolchains. Production TLS validation and original test deadlines
remain. The complete gate rerun is active in `tmp/root-tls-repaired-gates-final.log`;
That rerun passed 1,190 of 1,191 tests; all four RPC failures were resolved.
The remaining miner deadline exposed eager axon-publisher native-root loading
even with an explicit IP. The verified two-path lazy publisher repair is now
adopted; current/MSRV publisher tests and the unchanged miner test pass, with
strict Clippy on both. No-host separately passed before publisher adoption.
The complete rerun at `tmp/root-publisher-repaired-gates.log` terminated at
the features job. Its earlier source passed all 1,192 workspace tests
(43.321 seconds), with four separate skipped
cases: three run as isolated children of passing parent tests; immutable hosted
oracle replay remains pending CI data. Doctests, no-host and MSRV also pass.
The features job failed because its temporary no-dev manifest retains feature
forwarding edges to removed dev dependencies. An isolated repair must preserve
all original feature names and normal/build edges and exact retained-lock
provenance; original manifests remain unchanged. The standalone supply gate
passes with the existing exact dormant exceptions, full retained raw audit
findings and activation guards across every checked graph. Semver's pre-baseline
job also exits successfully without inventing a release comparison. The features
repair has now been adopted after canonical M4 current/MSRV validation and
exact source/patch hash checks. It validates the original locked graph before
pruning only dev-only forwarding, preserving invalid-feature errors and real
normal/build/target, alias, collision and weak-feature behavior. Its final
isolated 14 tests and strict Clippy pass on both compilers; the real 95-check
feature matrix, all original profiles and examples pass. The complete canonical
all-job rerun is live at `tmp/root-m4-feature-repaired-gates.log`; no full gate
success or hosted acceptance is claimed.
Evidence: `tls-roots-repair.md`.

Read-only upstream tracking verified that Finney remains spec 478 with the pinned
code hash, final v478 release and published image. Testnet is spec 479; its
authenticated V15/V16 comparison is additive (one call, two errors, two events
and two storage entries). No final v479 tag/release/image or source association
was verified. Keep core/parity v478; record testnet metadata only through the
drift tool at a source-stable point. Evidence is retained under the recorded
root at `runtime-479-drift/`; no runtime behavior range, transaction or public
filing follows from those observations.

Owner proxy routing and CLI machinery remain isolated pending final native
validation. Independent source review found no remaining owner-routing defect;
the provisional 138-pass/one-failure run exposed a test waiting on the wrong
head subscription, now corrected with its original deadline. CLI review led to
the D-041 operation-identity guard for automatic multisig approvals under rule 5;
final tests and canonical registration remain pending. Config/tools/explain
source is ready with 16 authored fixtures, without completed Cargo checks.
M5 offline import review caught and is repairing the core decoded-call versus
encoder input mismatch before adoption. The Keychain harness remains injected
only. Details and exact evidence roots: `m4-cli-preparation.md`.

The recorded eight PRs and five issues were freshly checked on GitHub; all
remain open and no new filing was made. Canonical gates remain live in nextest
enumeration, with source, extraction, pin/parity/dependency and strict workspace
Clippy checks already passed. No complete gate, commit/push, formal milestone
exit or release is claimed. The unanswered license-criterion decision remains
unchanged; all already approved decisions remain applied.
