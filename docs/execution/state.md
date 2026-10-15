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
in `.github/deferred-workflows/`. No workflow was dispatched during
the cleanup itself. Subsequent client checkpoints use hosted CI as recorded below.

## Current client checkpoint, 2026-10-15

Hosted generation
[38816297566](https://github.com/sam0x17/rtao/actions/runs/38816297566) passed
source parity at clean `2ad5d1b41673dc4804640edc11e4703ed834ec19`. Both artifact
archives were authenticated and independently fetched twice. Adopted outputs
replace the explicit precompile bootstrap with the actual generated consumer,
add five authenticated Python tables and refresh four reports. Exactly seven
files change; the other 16 generated files and all 23 dependency inputs are
unchanged. Existing legacy runtime-API output is unchanged. Bounded proofs
`2ad5d1b-artifact-inspection.json` and `2ad5d1b-artifact-adoption.json` remain
under `.local/recovery/2026-10-15/`, without archives or extraction directories.

The published matrix has 280/357 source rows reconciled, 77 pending, 20 pending
e2e mappings and 68 divergences. The five new dispositions cover codec, runtime
API, registration, crowdloan and Ledger pytest input, with explicit native
API/strictness and coverage limits. These are source reconciliation results.
Strict M8 completeness remains off; no milestone is accepted. M2-S02 remains
open and unaccepted, and upstream issue #3251 is still open.

Normal [38809901640](https://github.com/sam0x17/rtao/actions/runs/38809901640)
at `55f7ec0ebbbc5f08b685af9c47b8e18c71a2aef3` completed with all five supporting
jobs successful. Its main gate passed 2,963 tests with zero failures and four
skips, then passed the workspace doctests. Both serving-fixture repairs are
therefore confirmed for that source. The gate subsequently failed strict
rustdoc because public StateStore::save/load documentation linked through a
private module to MAX_CHECKPOINT_BYTES. The published correction states the
actual 64 MiB limit without changing visibility, behavior or lint policy.
The exact 781,765-byte terminal log has SHA256
`ba00375b057c261cbb8ec7efb71fb77897bfe305e384b6b2231a6aebeb35c502`;
bounded proof is `check-38809901640.json`. Book generation was not completed.
Follow-up ordinary
[38814558931](https://github.com/sam0x17/rtao/actions/runs/38814558931) at
`4e9447b195abe1d11123b3b153bd1955f81af747` completed with all five supporting
jobs passing. Its main gate passed all 2,968 tests with four skips and workspace
doctests, then failed strict rustdoc on one unresolved PythonBalance link.
Qualify that existing public type's link without changing behavior or lint
policy. The 782,118-byte log has SHA256
`8343a0f3afc901d20745f0f6b5d1c12f053e0f33d7fa7feac4c18141ca115394`;
bounded proof is `check-38814558931.json`. Book generation was not reached.
Newer precompile extraction, CLI canonical-signature notice,
nested Utility.batch and positive offline reconstruction fixtures still
require exact-source hosted execution. The hand-authored positive fuzz seed
has not been executed or validated, and no sanitizer coverage is claimed.

Ordinary [38818850318](https://github.com/sam0x17/rtao/actions/runs/38818850318)
is running at `f21c0919fcf42a73c09e9903d8a8bffa3180d562`, covering the adopted
precompile output and corrected CLI documentation link. Preserve this run.

The next source checkpoint repairs terminal policy refusals during paced
reconnect setup, with four real-session regression fixtures. It also normalizes
public RPC diagnostic text while retaining typed causes and policy metadata,
with eleven fake-startup vectors. Both patches passed independent source review
and direct formatting checks; neither has executed in hosted tests yet.
Complete substrate/interface reviews add two qualified source dispositions,
bringing the map to 282 reconciled and 75 pending; the generated matrix remains
at 280 until hosted refresh/adoption. Native read/write API ownership, raw
pool-only submission and map-presence capability/coverage limits are documented.
The RPC source row stays pending: fragmented HTTP rejection bodies can still
lose diagnostic text, and that separate repair remains under design review.

All current work follows Q30. Local target output remains 547,192 KiB; only
source inspection, direct formatting and bounded bookkeeping ran locally.
No local builds/tests, new temporary root, node, Mac Studio work, device run
or new upstream filing was needed. Preserve wallets, recovery material and
unrelated work.

## Earlier client checkpoints, 2026-10-15

These records retain their own source and execution scope. The current
checkpoint above supersedes their instructions to dispatch or retry checks.

### Earlier header and source checkpoint, superseded by 2f9a30d

Normal [38790070632](https://github.com/sam0x17/rtao/actions/runs/38790070632)
at `da99c3deb6886bca32a1d3c04eb098977068d8d3` stops before tests on
`_subtensor.py`'s extra helper-crate mapping. All five supporting jobs pass.
Raw main log is 61,923 bytes, SHA256
`dc01e374fe32111e15206f169f139132dd17361e8e2dc8ca775f790359584fb8`;
bounded proof is `check-38790070632.json`. Tests/doctests/docs were not reached.
The preceding 97090f2 run caught the combined header's incomplete source path;
splitting that header exposed the remaining helper mappings.

A complete source-header preflight now distinguishes direct derived homes
from reused dependencies in `_subtensor`, sync, crowd and result mappings.
Config, result and root-claim ports receive their actual canonical origins.
The named-table producer emits module origins before its array declaration
so rustfmt preserves column-zero headers. Its generated catalog still needs
hosted regeneration/adoption. The refresh job now runs full source parity
before uploading artifacts, catching these errors before ordinary test jobs.
This source includes all three corrections from the prior ordinary run:
the explicitly absent collateral UID fixture, conviction stderr/singular
wording, and recovery receipt parsing for both canonical hex and legacy arrays.
It also retains failed receipt events for native multisig/recovery consumers
under D-023, while preserving Python result JSON field presence.

Report refresh [38789498154](https://github.com/sam0x17/rtao/actions/runs/38789498154)
succeeds at clean `56469a09a2013c959035cd7303f7d2a4e19ac71a`.
Authenticate the 55,264-byte archive, four regular report members and 23
unchanged dependency inputs. The matrix changes exactly the eight reviewed
file rows: 228/357 reconciled, 129 pending, with 20 pending e2e mappings.
Role inventories and host flags are unchanged; graph changes are only CPU,
time and source provenance. All compatibility, runtime and oracle claims
remain unchanged. Proofs are `564-report-review.json` and
`564-report-adoption.json`; no archive or extraction directory is retained.
The next source batch reaches 236/357 reconciled files, with 121 pending and
20 pending e2e mappings; its generated reports and tests remain pending.
Strict M8 completeness remains off. No milestone has been accepted.

That batch shares the Vault/extension browser selector and launcher: opening
returns after spawn and does not kill an application after ten seconds. A
bounded pipe-child regression exercises lifetime and process-group separation
without a browser or filesystem fixture. Vault enables the existing Unix nix
dependency for executable discovery. QR encoding now preserves Python's actual
single-mode selection and capacity limits; decoded SVG fixtures cover all
three modes and boundary refusals. The plan's byte-only interpretation of
`optimize=0` was a factual error; binary UOS frames still use byte mode.
Eight named natural-name/wallet-compat fixtures preserve the original cases
with explicit native API dispositions. Browser/QR/scan/daemon and two shared
core primitive test-source rows have qualified whole-file source reviews.

The batch restores command-specific timelock help/prompt guidance; adds async
snapshot sibling, cached-symbol and balance helpers; preserves exact classifier
policy payloads alongside sanitized diagnostics; accepts Python edge whitespace
for EVM secrets/artifacts and bridge tokens; handles CRLF/bare-CR artifact and
successful macOS password output; and preserves scalar JSON array coercion.
ABI array acquisition checks entry counts before collection or deserializing
an excess top-level element. Independent source reviews and root rereads pass;
no local build/test or native password/browser service ran. Snapshot siblings
resolve eagerly and need an open connection; D-007 explicitly covers derived
slot timing from the retained runtime. Timelock parser/deadline, precompile
table/diagnostic, Vault browser/QR and picker source gaps remain pending.

All 70 selected S9 regressions passed in prior ordinary run 38780323094 at
`262be704a322e82efcbcaf0ee5764f1adb5519aa`; all 29 selected boundary/test
files are byte-identical at `97090f2`. Immutable run/job/head identities,
raw-log digest and exact PASS lines are in
`m9-security-targeted-ci-38780323094.json`. That run failed three other tests;
these targeted results do not establish C1 or close the findings. M2-S02
Ed25519 final-drop erasure remains open and unaccepted.

Hosted generation [38777439872](https://github.com/sam0x17/rtao/actions/runs/38777439872)
succeeds at clean `5464ccf`. Authenticate two exact archives, all 20 regular
allowlisted members and all 23 unchanged dependency inputs. Only the new
22,297-byte root-claim fixture differs among generated interfaces; both source
blobs/SHA256s and case inventories are checked. The matrix changes exactly
three reviewed rows: 212 reconciled/145 pending, with 21 e2e keys pending.
Existing oracle and compatibility claims, role package inventories and host
flags stay unchanged. Record the hosted CPU accurately as measurement provenance.

Proofs are `546-generation-review.json` and `546-generation-adoption.json`;
no archive/extraction directory remains. Root replaces test-only unwraps with
checked fixture diagnostics, retaining all assertions and workspace lints.
Push this adoption head for normal combined hosted validation. No current
passing tests/doctests/docs or milestone acceptance is claimed.

Generation38776671979 atd7b4a27 compiles and extracts, then regeneration
finds the generic money selector incorrectly requiring a Python helper-table
entry before a custom Crowdloan resolver. Preserve custom resolvers first;
for ordinary constructors keep authenticated identity/fallback precedence,
then use a metadata-mapped native balance unit when a compact wrapper hides
its annotation. Untyped unknown u64 remains a refusal. A real pinned-metadata
fixture covers Crowdloan contribution/cap/minimum arguments and unknown-u64
refusals. Log50,999bytes SHA256
`25153a7a1381b10724723066a49845ba5de5a03b34eb8e55c00e7b332f5eeb28`;
proof `check-38776671979.json`. No adopted artifact or new test success.


Root follow-up aligns decimal ABI integer strings with Python's default
4,300-digit limit, including leading zeroes; hexadecimal remains exempt.
Public coercion fixture source covers both sides, signs, Unicode digits and
separators while retaining native u64 and ABI-width checks. Formatting and
whitespace pass; execution remains pending. Hosted generation38776671979
runs at d7b4a27; this later source-only fix changes none of its dependency,
extraction or report inputs.

Generation run38776446102 at8112797 fails before producing reports: the new
cancellation-aware hash request needs an explicit JSON Value type before its
null check. Add that annotation, preserving the same request and cancellation
path. Raw log51,419bytes SHA256
`b7fba301dd5137e054148f0eb0b567f7549004ee42a998fc8d96b8f9186dfeb4`;
proof `check-38776446102.json`. No generated artifact or new test result.
Retry hosted generation on the corrected clean source before ordinary CI.

Normal [38770765426](https://github.com/sam0x17/rtao/actions/runs/38770765426)
at `237d189` is terminal: all five supporting jobs, main compilation,
strict Clippy and ordinary parity pass. Nextest runs 2,831 tests: 2,784 pass,
47 fail and 4 are skipped. Doctests/docs are not reached. Raw main log is
1,258,642 bytes, SHA256
`b5a3c5b4b04cc477e8b322c38d131a3703149d6877e63296592007e5318acc9a`;
bounded exact-source failures are retained in
`test-failures-38770765426.json`. These results predate the later source batch.

All 47 failure causes now have source corrections or reviewed fixture repairs.
Production corrections include signer-specific metadata digests without stale
cache reuse, result hash hex, authenticated modern preimage length, deterministic
Vault replacement close, upgrade repository validation and decrypted-title
stream. Fake request recording preserves every separate transport call while
excluding internal delegation, including recovery after injected panic.
Fixture repairs preserve one-submit/deadline/nonce/shield, parent-runtime and
exact numeric-bit checks. Independent source rereviews and root cross-review
pass; hosted execution is still required.

The same batch repairs closed-client snapshot/subscription admission and
cancellation, block-decode null placeholders, Python ABI text grammar/network
text and CLI help/attribution. Three reviewed source rows bring the map to
212 reconciled/145 pending, with 21 e2e keys pending. Address prefix evidence is
registered 0/42/128 with unregistered 1000 refusal. The hparams empty view omits its
footer, matching Python; its renamed fixture is reflected in the map.

New source-only catalog fixtures consume all 102 authenticated READ_SAMPLES at
head and snapshot 90, keep exact semantic spots, and assert ordinary intent
summary/fee. A dedicated root-claim extractor projects authenticated tables,
constants and bounded recognized case inputs without weakening the shared
resolver. Its test-only JSON must be generated/adopted in hosted CI before
ordinary execution; both full root-claim rows remain pending their disclosed
inline-input/API adaptations. The refresh allowlist includes exactly that new
output. No local generation, builds/tests, node or temporary directory.


The three focused boundary repairs now pass independent source rereview:
cache/custom-payload decimal strings enforce the 4,300-digit limit; empty
weights-helper names select the default wallet; blocking client/snapshot
balance factories retain registered nonroot symbols without starting cold
work or RPC. A seeded root symbol is ignored, including its optional metadata.
Boundary file rows remain pending their full-file qualifications. Direct
formatting and whitespace checks pass; compilation/execution remain unverified.
At 17:02 UTC normal run38770765426 still has main active and all five supporting
jobs passing. Preserve its ordinary outcome before dispatching the final source.
Further source review found checkpoint temporary-name ownership and closed-client
RPC gaps; focused repairs are in progress. Block lookups also need Python's
null placeholders for individual decode failures. No local builds/tests or
new temporary directories.

Refresh [38771543517](https://github.com/sam0x17/rtao/actions/runs/38771543517)
succeeds at clean `ddd73e5`. Authenticate and adopt exactly four regular report
members, archive digest/size and all 23 committed/current dependency inputs.
Only the explicit rtao-tx serving test-feature manifest hash changes. Role
inventories and host selection remain unchanged. Matrix differences are exactly
five reconciled source rows, two corrected pending root-claim homes, three
qualified client lifecycle bindings and new unverified D064/D065. Source counts
are 209 reconciled/148 pending; 21 e2e mappings remain pending. Every existing
oracle classification and compatibility claim is unchanged. Proof is
`ddd-generation-adoption.json`; no archive/extraction directory remains.

At 16:49 UTC normal run `38770765426` still has main/features active and four
supporting jobs passing. Retain that run to obtain its ordinary test outcome;
the report-adoption commit skips automatic CI. Dispatch the next normal hosted
run on the adoption head after the existing main job concludes. Its results
cannot validate the later batch. No fresh test/docs or milestone acceptance.

Normal run [38770765426](https://github.com/sam0x17/rtao/actions/runs/38770765426)
validates the isolated compiler correction at `237d189`. At inspection supply,
MSRV and semver pass; main, features and no-host remain active. It does not
include the following source batch. The previous run `38769405520` is superseded;
its main failure and passing supporting jobs retain their own exact-source scope.

The next reviewed batch completes generic money-unit selection from both
authenticated Python tables, validates real sibling netuids and retains root
TAO/native-width checks. Six fee-filter counterparts use actual registered
intents and public planning with independent SCALE bytes; their focused test
dependency explicitly enables serving. Root-claim fixtures now preserve all
31 Python case assertions in 18 native bodies, including real shield refusals
before private acquisition. Both root-claim rows stay pending the plan's
authenticated table extraction/consumer requirement, with their nonexistent
home corrected. Three client/config/facade discrepancies from the separate
boundary audit also remain open.

Collateral command parsing/help and complete list fixtures close the earlier
source gaps; D064 discloses its repaired human-list rendering. Misc probes keep
the existing separate 30-second phase bounds, documented in D065 and exercised
through actual timeout/cancellation/closure branches with injected zero budgets.
The ownership-gate test row has four independently verified exact counterparts.
Source maps now declare 209 reconciled rows and 148 pending. Three reviewed
transfer/batch source mappings retain explicit controlled-state, mutation,
atomicity/rollback limits, leaving 21 e2e keys pending. Their tests and the
existing five fee contracts remain ordinary CI targets; no runtime success
is asserted. Current committed matrix/reports still describe the prior source
until authenticated hosted refresh/adoption.

Root review and available independent case/gate reviews found no remaining
source defect in this batch. Direct formatting and whitespace checks pass;
hosted compilation, generation checks and execution remain required. Proofs
include `client-source-batch-root-review.json`, `client-unit-source-identities.json`
and the individual source reports in the existing recovery root. No local
build/test, service, temporary directory, upstream filing or milestone acceptance.

Normal run [38769405520](https://github.com/sam0x17/rtao/actions/runs/38769405520)
at `6480626` passes every ordinary parity layer, including all 63 divergence
declarations with the exact optional D031 deferral. Main Clippy then fails
compiling the new unit fixture: `tempfile` is not an xtask dependency. Extract
the existing document validator and call that same function from the fixture,
preserving all cases without temporary files or a new dependency. The transfer
lifecycle target has no reported compiler diagnostic in the completed Clippy
log; execution remains unverified. Supply, MSRV, no-host and semver pass;
features is still active at inspection. Proof: `check-38769405520.json`.
This small compiler correction is pushed separately from the later source
batch so ordinary hosted validation can proceed while that batch is reviewed.

Normal run [38767926902](https://github.com/sam0x17/rtao/actions/runs/38767926902)
at `bbfa09a` has failed its main check. Supply, MSRV, no-host and semver pass;
features remains active at 16:28 UTC. The earlier parity check now passes the
corrected attribution/discovery layers and all 115 e2e keys, then exposes an
ordering bug: optional pending M10 D031 is required to have pinning tests before
the validator reaches its documented deferral. Limit the empty-list allowance
to that exact pending/M10/addition tuple. A full-register regression exercises
the actual strict validator, then rejects active/M9/changed-class mutations.
Independent source rereview clears the correction. Proof is
`check-38767926902.json`; Clippy, ordinary tests, doctests and docs were not
reached. A fresh normal combined hosted run is required.

Independent review clears the balance/property source mappings, workspace
test discovery and earlier parity-gate ordering. It finds a mapping escape:
a known client/read key could change to an unrelated hosted test and remove
its evidence fields. Both validators now require exact known targets before
hosted-prefix classification; a regression exercises that combined mutation
against an indexed real hosted target, retaining pending/NA handling. Corrected
source rereview finds no remaining issue; hosted execution remains pending.

The Q30 plan audit finds no active costly local milestone requirement. Remove
stale automatic-filing and unconditional Ledger-transcript refresh wording in
the plan and architecture, and clarify that cleanup removed the task-owned
cache inside the retired root. Independent rereview clears those corrections.
Additional proposals remain local. The audit preserves all current C1-C7,
strict source-parity, docs and security-disposition requirements.

The `_money.py` audit traces all eleven current call-amount sites to correct
native currency/sibling-netuid handling. Its whole row stays pending: generic
unit-table selection, override-value validation and an inactive alpha generator
branch still need implementation/coverage. Three new transfer/batch transcript
fixtures preserve exact upstream amounts, production Executor signing, independent
expected calls, explicit receipt state and exact hash/key-qualified balance reads.
Two negative fixtures reject wrong calls and script exhaustion before submission
or state advance. Root and independent source rereview find no remaining defect;
hosted compilation/execution and mapping integration are pending. No chain
economics, atomicity or rollback is claimed. No local build/test, service,
temporary directory or upstream filing.

Refresh [38766936693](https://github.com/sam0x17/rtao/actions/runs/38766936693)
succeeds at clean `41f3838`. Verify and adopt its four exact regular report
members, immutable archive digest/size and all 23 committed/current dependency
inputs. The matrix changes only the two reviewed balance/property dispositions
and five corrected derived-code homes. Source counts are 204 reconciled and
153 pending; all 115 e2e rows, oracle classifications and runtime qualifications
are unchanged. Dependency inventories, host selection and compatibility claims
also stay fixed. Proof is `41f-generation-adoption.json`; no archive or
extraction directory remains. Normal combined hosted validation is next.
Independent rereviews of the balance mapping and final mapping/discovery gates
are active. No local build/test, service, new filing or milestone acceptance.

Normal run [38762528504](https://github.com/sam0x17/rtao/actions/runs/38762528504)
at `60ff9a4` is terminal. All five supporting jobs, main compilation and strict
Clippy pass. Parity checks the new five client-contract bindings and all 115
e2e keys, but stops before tests on a false CLI-derived package home and missing
xtask test discovery. The D022 witness exists and Cargo tests it; the source
index considered only packages under `crates/`. Discover declared explicit
workspace members and immediate-child globs, retaining exclusions, repository
bounds, feature/harness/ignore checks and the inclusion-graph limit. Add focused
positive and refusal fixtures. Preserve the D022 reference and price assertions.
Proof is `check-38762528504.json`, including the full parity-layer report.

Correct five map rows whose dependency/producer references were incorrectly
presented as derived-code homes. Retain actual native ports and literal outputs;
dependency calls remain described in notes. No copyright notice, package license
or attribution gate is removed. Run the same source-evidence check once, before
longer compiler/probe work, preserving every mandatory gate and ordinary test.
The newly reviewed balance/property rows bring source counts to 204 reconciled
and 153 pending. Q25/D035's native u64 domain, excluded signed/rounding behavior
and ten upstream property relationships are explicit; fresh oracle execution
is unverified. `_money.py` stays pending for its remaining producer/consumer trace.

A 15:51 UTC state-only filing refresh still finds eight PRs and five issues,
all open, with unchanged reviewed PR heads and no new filings or GitHub writes.
The earlier source/body audit remains separately dated. Fresh report generation,
combined normal CI and independent gate-integration rereview remain required.
No local build/test, service or temporary directory was created.

Refresh [38761784282](https://github.com/sam0x17/rtao/actions/runs/38761784282)
succeeds at clean `8cd8de5`. Verify/adopt four exact regular report members,
the immutable archive digest/size and all 23 unchanged dependency inputs.
Role inventories/host selection and compatibility claims stay unchanged.
Matrix changes are exactly ten reviewed file rows, two new unverified decisions,
D055's exact module reference and the scoped e2e evidence described below.
All 115 e2e keys remain: 72 intent plans, 6 read fixtures, 5 client contracts,
6 hosted scenario sources, 2 inapplicable and 24 pending. Runtime claims remain
unverified or inapplicable; no fixture result is reported as a chain execution.
Proof is `8cd-generation-adoption.json`; no archive/extraction directory remains.
Combined normal hosted validation and independent gate-integration rereview
remain pending. A spelling-only source inspection finds all 199 registered
divergence functions, including two manually traced module aliases; this does
not establish feature/harness qualification or execution.

Run [38759055202](https://github.com/sam0x17/rtao/actions/runs/38759055202)
is terminal: all five supporting jobs, main compilation and strict Clippy pass.
Parity stops before tests on D055's incorrect `client::tests` module path.
The unchanged fixture is declared in `client::attempt_budget_tests`; correct
only its registered witness path. Its body preserves completed oversized-body
refusals across both timing boundaries, without sleeps or retries. Evidence is
`check-38759055202.json`, with the full raw-log digest. Do not infer nextest,
doctest or docs success. Compatibility refresh `38761443333` at `a9a7ea5`
predates this register correction; its matrix cannot be adopted as current.
The corrected source requires a fresh hosted report followed by normal CI.

Normal run [38759055202](https://github.com/sam0x17/rtao/actions/runs/38759055202)
at `4000bd0` passes all five supporting jobs; main check remains in progress at
inspection. The final corrected reveal-parser rereview finds no remaining
concrete defect within its declared projection scope. Full chain rows remain
pending; no runtime or milestone acceptance follows.

The next source batch reconciles ten more authenticated files after complete
source/fixture review, reaching 202 reconciled/155 pending. Seven intent rows,
HTTP-auth core/its twenty direct test counterparts, and CLI lock commands have
exact native homes. D062 discloses existing owner/window preflight and D063
discloses decoded-account replay identity. Correct the contradictory owner-list
fixture assertion and canonical attribution. Repair CLI contact JSON types and
stable hierarchical alias order; independent rereview closes the natural-key
tie found during that repair. All ten blobs/SHA256 pins are reverified in
`ten-source-identities.json`.

Five independently reviewed policy/fee scenario sources now use real intents
and Executor, independent expected calls, exact fee-query decoding and refusal
diagnostics, with no private-key acquisition or submission. Their exact mapping
entries reduce pending e2e rows from 29 to 24. New validation/matrix code binds
only those five client contracts, requires unverified runtime/fee limits, checks
ordinary CI inclusion and classifies every existing mapping's source evidence.
Strict pending and oracle rules remain intact. The gate integration needs an
independent reread and hosted execution; its review request is retained in
`e2e-mapping-integration-review-request.txt`. Fresh hosted matrix generation and
combined ordinary validation remain required. No local build/test, service,
temporary directory, new filing or milestone acceptance is claimed.

Compatibility refresh
[38757888373](https://github.com/sam0x17/rtao/actions/runs/38757888373)
succeeds at clean `303e631`. Verify/adopt exactly four regular report files,
the archive identity/digest and all 23 unchanged dependency inputs. Role counts
and host selection stay fixed. The matrix changes only the twelve reviewed
core-py source rows, retaining oracle-only/unverified classifications; source
counts are 192 reconciled/165 pending, with 29 e2e mappings pending. Compatibility
and runtime claims are unchanged. Proof is `303-generation-adoption.json`.
No archive, extraction directory or local build was created.

The previous normal run `38756773737` is terminal: supply, MSRV, no-host,
semver and features all pass. Main Clippy's two fixture warnings are corrected.
Independent parser rereview closes the earlier parameter/impl binding findings
and finds one remaining omitted `where` clause check. Require no added impl
constraints and add a source-mutant rejection fixture. This changes no current
pinned projection output. Full chain rows and runtime coverage remain pending.
The combined source and reports require the next normal hosted validation;
no current full test pass or milestone acceptance is established.

Source audit reconciles the twelve `oracle-only` core-py files after reviewing
all 1,956 lines, exact watched blobs/artifact SHA256 pins, and the Dockerfile's
tag/commit/frozen-uv build plus image-identity consumer chain. These are optional
oracle dependency inputs. The previous pending notes required a first corpus
for source reconciliation, contrary to the existing Q30 separation. Keep the
same classification and all image/Python/differential/device/published-wheel
coverage unverified. Native product source and export obligations are unchanged.
The file map is now 192 reconciled/165 pending; 29 e2e mappings remain pending.
Proof is `core-py-source-review.json`; no Python, container or local build ran.
Fresh hosted matrix generation is required before ordinary validation.

Normal run [38756773737](https://github.com/sam0x17/rtao/actions/runs/38756773737)
at `d179b14` passes semver and MSRV at inspection; supply, no-host and features
remain live. Main Clippy stops before tests on two cloned values used only to
create borrowed one-element slices in the new Children ordering fixture. Use
`std::slice::from_ref` at those call sites, preserving both assertions and the
later owned tool arguments. Proof is `check-38756773737.json`. No lint gate or
assertion is disabled; the corrected source needs another normal hosted run.

Compatibility refresh
[38756143897](https://github.com/sam0x17/rtao/actions/runs/38756143897)
succeeds at clean `877a39c`. Verify/adopt its four exact regular report members,
archive identity/digest and all 23 dependency inputs. Only xtask's reviewed
license input hash changes; role packages and host selection are unchanged.
The matrix changes only D-057's two portable pinning references, with all other
source statuses and oracle classifications retained. Runtime validation claims
remain empty. Proof is `877-generation-adoption.json`. The combined source and
reports now need the next normal hosted check; no test or milestone acceptance
is inferred from generation.

Generation [38754665115](https://github.com/sam0x17/rtao/actions/runs/38754665115)
succeeds at clean `d65e5a8`. Both exact archive identities/digests, nineteen
regular allowlisted members and all 23 committed/current dependency inputs
are verified before adoption. Role counts remain 225/362/366/417 and host
selection remains false/false/false/true. The sole generated Rust change is
AnyIntent's reviewed unknown-op delegation; twelve other Rust files and the
environment artifact are byte-identical. Chain rules gain only the reviewed
proxy-codec and reveal payload/epoch-prefix sections; existing rules are
unchanged. Matrix changes are eleven file homes/statuses, three e2e dispositions
and new unverified D-061. Compatibility/runtime/oracle claims do not change.
Bounded proof is `d65-generation-adoption.json`; no archive or extraction
directory remains.

Normal run [38753200118](https://github.com/sam0x17/rtao/actions/runs/38753200118)
is now terminal: all five supporting jobs pass, and main compilation/strict
Clippy pass. Parity stops before tests on xtask's inherited license and a
Unix-only D-057 pinning reference. Declare the existing combined Apache
obligation in xtask's manifest and register the existing unconditional regular
file/rotation witnesses. Keep the Unix FIFO and broken-symlink tests intact as
additional platform fixtures, with their execution still pending. No coverage
or license rule changes. Evidence is `check-38753200118.json`.

The changed manifest needs a fresh hosted dependency/compatibility record,
and the changed D-057 witnesses need an updated matrix. Use a compatibility-only
refresh before the next normal combined run. No local build/test or milestone
acceptance is claimed.

Normal run [38753200118](https://github.com/sam0x17/rtao/actions/runs/38753200118)
at `164247c` passes semver, MSRV, no-host and supply at inspection; main check
and features remain live. It includes the named signing-recorder type. No
current full workspace test success is established.

The next source batch reconciles eight more files: 180 of 357 source rows are
now reconciled, with 177 pending. Two exact ordinary read bindings and the
obsolete core tempo-rejection disposition reduce pending e2e mappings to 29.
Source evidence is in `intent-map-batch-review.txt`, `plan-registry-review.txt`
and `e2e-source-audit.md`. Complete Plan approval JSON and all nine manifest
fields have stronger fixtures. Registry diagnostics now retain Python quoting
and the sorted callable operation set; generated AnyIntent needs hosted refresh.
Subnet creation discloses a pinned exact cost and queue/pruning flow. D-061
records accepted Yuma/Null setter examples instead of Python's rejected numeric
examples, with both-mode CLI round-trip source.

The same batch adds narrow source-backed proxy-codec and reveal-payload/epoch
projections to the existing chain-rules artifact. Independent review found
unchecked parameter and containing-impl bindings; the correction validates
their signatures/owners/configuration and adds five source mutations. A final
corrected-parser reread is unverified. Full proxy/filter, reveal-loop and payment
wrapper rows remain pending. The hosted artifact allowlist retains this exact
chain-rules file for review. Generation/adoption, compilation and execution of
this batch remain pending; no milestone is accepted and no local build ran.

Normal run [38751673871](https://github.com/sam0x17/rtao/actions/runs/38751673871)
at `4f28dbf` passes semver, MSRV, no-host and supply at inspection; features is
still running. The three fixture type errors are gone. Main check stops before
tests on one Clippy type-complexity finding in the nested-policy signing
recorder. Name that fixture type without suppressing the lint. Retained evidence
is `check-38751673871.json`. Final independent rereads find no remaining defect
in the corrected multisig dynamic-role continuation or pinned-only shielding
guard; executed regression closure remains pending. No local build or test ran.
The run is now terminal; features was cancelled when the lint-correction push
superseded it.

Normal run [38750380241](https://github.com/sam0x17/rtao/actions/runs/38750380241)
at `eefbfa2` passes semver, MSRV and supply at inspection; features and no-host
are still running. Main check stops before tests on three fixture compiler
errors: Crowdloan state needs typed seeding in two fixtures, and V15 pallet
metadata exposes `error`, not `errors`. Correct those fixtures and remove the
unused descriptor import. Formatting and diff checks pass; execution of the
corrected source requires the next normal hosted run. The bounded failure
record is `check-38750380241.json`. No local build or test ran.
That run is now terminal: no-host also passed, and its features job was
cancelled when the fixture-correction push superseded the run.

The source batch is committed in `4c9473c`. Generation run
[38749149983](https://github.com/sam0x17/rtao/actions/runs/38749149983) succeeds;
its eighteen exact regular members and both archive digests are verified, along
with all 23 dependency-input hashes. Role inventories and host selections are
unchanged. All thirteen generated Rust files and environment data are identical.
Only reviewed file/divergence matrix rows and measurement provenance change;
no runtime/oracle success is added. The private proof is
`4c-generation-adoption.json`. Reports are adopted for the next normal CI run;
combined compilation, regression execution and final security disposition remain
pending. No local build or extraction directory was created.

Run [38746145922](https://github.com/sam0x17/rtao/actions/runs/38746145922)
at `5125fb9` finishes with all five supporting jobs passing, including supply.
Main compilation and strict Clippy also pass. Parity then stops before tests
on the missing basket producer's canonical attribution and a renamed D-044
fixture reference; both are now corrected in source. The bounded log record
is `check-38746145922.json`. No current nextest/doctest success is established.

The next source batch adds M9 resource/secret/composition repairs and the public
typed multisig adapters. [The M9 report](../security/m9.md) preserves its nine
original findings and supplemental continuation finding, with source-repair
evidence separate from pending hosted execution. Existing upstream filings and
fuzz-source coverage are audited in [m9-upstream-audit.md](m9-upstream-audit.md)
and [m9-robustness.md](m9-robustness.md). The file map has 172 reconciled rows and
185 pending; through-M9 divergence source/fixture decisions are reconciled,
while optional M10 D-031 remains pending. Strict parity and milestone acceptance
remain incomplete. Fresh generated reports and combined hosted tests are next.

The preceding completed normal run
[38742652282](https://github.com/sam0x17/rtao/actions/runs/38742652282) at
`d7143f7` passes features (including unpatched shipped-crate and generic-keyfile
checks), MSRV, no-host and semver. Main check stops before tests on CLI fixture
qualification and Clippy errors; those sources are now corrected. Supply stops
on unsafe-libyaml's informational maintenance notice. The exact reviewed policy
and upstream feature constraints are recorded in
[supply-unsafe-libyaml.md](supply-unsafe-libyaml.md). These corrections need a
fresh normal hosted run. No current full test pass or milestone acceptance is
claimed. M9 security/read-map changes remain a separate uncommitted batch.

Normal run [38723619536](https://github.com/sam0x17/rtao/actions/runs/38723619536)
at `e82937c` passes all five supporting jobs, main compilation and strict
Clippy. Parity now passes the corrected catalog, pending-read and e2e checks,
but detects the facade's inherited license missing the Apache obligation from
its derived blocking adapters. Its manifest now uses the required combined
expression. Nextest/doctests were not reached. The changed manifest requires
a fresh hosted dependency report before ordinary validation.

Normal run [38722567841](https://github.com/sam0x17/rtao/actions/runs/38722567841)
at `f129eb1` passes features, MSRV, no-host, supply and semver. Main compilation
then strict Clippy finds `expect_err` in the new duplicate-read regression.
The fixture now requires the error through fallible test context, preserving
the assertion and the lint. Parity, nextest and doctests were not reached on
this run; another normal hosted run must verify the correction.

Normal run [38720004914](https://github.com/sam0x17/rtao/actions/runs/38720004914)
at `4058413` passes features, MSRV, no-host, supply and semver. Main check passes
compilation and strict Clippy, then fails parity before tests on canonical
attribution, the generator's two escaped rustdoc placeholders, private read
helpers counted as duplicate entry points, and four M6 ordinary read-test
bindings. Source repairs retain strict attribution and exact rendered wording,
count public async read entry points, and allow only the four reviewed read
bindings with assertion/source checks. Negative fixtures preserve rejection of
changed wording, real duplicates and unrelated mappings. The basket generator's
header order also needs hosted regeneration/adoption. Another ordinary hosted
run must execute this source. No local build or test ran.

Generation [38721766957](https://github.com/sam0x17/rtao/actions/runs/38721766957)
at `65d4811` succeeds. Both archive digests, seventeen exact regular members
and all 23 committed dependency inputs are verified before adoption. Its only
generated-code change swaps the two basket header lines; all other generated
interfaces and the entire parity inventory are unchanged. Role inventories
remain 225/361/365/416 with core host disabled for core, validator and miner.
The next normal run also includes the separately reviewed shield repair below.

Source adds the remaining 62 reads and the blocking facade. All five read
groups have fixture source and independent source reviews. Generation run
[38715060741](https://github.com/sam0x17/rtao/actions/runs/38715060741) at
`7953bea` succeeds and its outputs are verified and adopted: fifteen exact
archive members, all 23 dependency hashes, unchanged role package inventories,
62 read availability/output/body changes, four input overrides and D-027's
exact fixture mapping. All 83 intents and 102 reads now have generated
integration. Blocking output has 14 namespaces and 101 catalog methods, with
the curated typed metagraph adapter supplied by hand-written source. Ordinary
combined compilation and execution remain pending. See [m6-read-source.md](m6-read-source.md).

M6's 24 CLI commands and complete 102-query tree now have authored source,
parser/entrypoint/FakeChain fixtures and independent source reviews. The root
basket generator puts its 45 frozen samples and 171 fund baselines in the CLI
only. Run `38716924750` at `c281803` succeeds; its reviewed artifacts supply that
exact table and six rustdoc corrections, with other interfaces unchanged.
Reports from this run are not adopted because parity source advanced after
its commit. Generation run `38718933791` at `e2e19b4` then succeeds and is
verified/adopted: seventeen exact regular members, all 23 dependency hashes,
unchanged role inventories, only the six frozen basket records removed from
the shared catalog, four M6 test mappings and the exact divergence registrations.
All other generated interfaces remain unchanged. Its private evidence is
`m6-checkpoint-adoption.json`; ordinary combined validation is next.

The keyfile parser repair is committed in `f16e97c` after independent source
review. A fresh M4 review records four findings and source repairs in
[m4.md](../security/m4.md): preserve unresolved submission hashes/causes,
escape terminal data, bound public file acquisition and avoid owned secret
output copies. Hosted compilation/regression evidence is missing for these
repairs. Upstream Ed25519/KDF zeroization limits remain unresolved.

The M4 follow-up found that shield polling exhaustion could still report an
unknown outcome as ordinary non-inclusion. Its source repair propagates lookup
errors, preserves the exact attempted carrier hash, and requires finalized
signed expiry plus a full canonical rescan before claiming absence. Four new
regression fixtures cover stalled-chain recovery, RPC/malformed hashes,
finalized absence and reorg recovery. Two independent source reviews find no
remaining concrete defect in that repair; execution remains pending.

M7 source now integrates Ledger, Vault/UOS/QR, the extension bridge and native
CLI factory, EVM keys/ABIs/RPC/transactions, timelocks, the Subxt legacy adapter
and opt-in GPU PoW. All 34 M7 CLI routes are wired. Native balance boundaries
remain checked u64 rao; wider EVM ABI counters retain their own widths. Shared
EVM address helpers are in primitives and reused by intents and EVM. Public
external discovery remains separate from signing acquisition and confirmation.
Authored fixtures and independent source reviews cover these paths; none of
the new M7 fixture execution is established yet. The source review in
`docs/security/m7.md` records repaired plaintext ownership and Vault QR offload,
with scoped coverage and explicit upstream/browser-copy erasure limits.

M7 registers D-033's nominal block-time reveal, D-042's bounded timelock inputs,
and D-051's EVM proof consent/effective-account binding. Thirty-six resolved
signer/timelock export decisions now have exact public aliases. The new QR,
EVM and optional Subxt dependency edges were resolved without builds; hosted
generation, supply/MSRV checks and combined execution remain pending. Ledger
core-patch/replay work, final transport checklist fixtures and later M8/M9
acceptance still remain. Device/HID, emulator, browser/camera, native Keychain,
fresh Python oracle and live-chain interoperability execution are unverified.

Normal run [38714093697](https://github.com/sam0x17/rtao/actions/runs/38714093697)
at `745f3f3` passes features, semver, no-host, MSRV and supply. Main check passes
compilation, strict Clippy, source/parity guards, all 2,022 executed nextest
tests and nine doctests; four nextest tests are skipped. It then fails strict
rustdoc because six generated parameter descriptions contain unescaped HTML
placeholders. The generator and adopted hosted output now render them literally;
the docs check must be rerun. These results exclude the newer M6 source and parser
repair. Bounded evidence is private `check-38714093697.json`.
Generation run [38714402721](https://github.com/sam0x17/rtao/actions/runs/38714402721)
at `afd989a` fails before regeneration because the new blocking-accessor format
string has an unmatched closing brace. The generator source now escapes each
generated scope explicitly. No output from the failed run is adopted.

Normal run [38710720744](https://github.com/sam0x17/rtao/actions/runs/38710720744)
at `06a3598` passes all five other jobs, compilation, strict Clippy and parity.
Nextest runs 2,021 tests: 2,005 pass, 16 fail and four are skipped. Source fixes
retain the original behavior and strengthen fixtures for fixed-width multisig
timepoints, exact business-probe counting, typed storage batching, runtime UTF-8
decoding, all implemented command advertisement, distinct account spellings,
last-allocated nonce semantics and the valid subnet boundary 4095. Explicit
refusals for 4096/65535 are added to the native policy fixture. No assertion or
gate is disabled. The all-83 planning test passes; doctests and docs are not
reached. Evidence: private `check-38710720744.json`. The new read/facade source
was not included in that run and has no compiled/tested claim.

Source through `a929bcd` implements all 83 intents and 40 of 102 reads,
with the corresponding hosted generated output verified and adopted.
The newer crowdloan/root and coldkey intent ports have verified hosted generated
dispatch and availability metadata, with combined validation still pending;
see [m5-crowdloan-root.md](m5-crowdloan-root.md) and [m5-coldkey.md](m5-coldkey.md).
The CLI compiles all intent interfaces and advertises implemented operations.
The generated read interfaces and dispatch for both M5 owner reads are adopted.
No milestone exit or current all-job success is claimed.

The thirteen proxy, children/take and association implementations have verified
generated dispatch, constructor normalization and availability metadata.
Ordinary hosted validation remains pending. See
[m5-proxy-children-association.md](m5-proxy-children-association.md).

The seventeen operations covering lock, collateral, leasing, EVM,
basket/configuration and root claims have verified generated integration.
Their ordinary hosted compilation and fixture execution remain pending. Review
also corrected exact money JSON to match Python's decimal serialization,
with native u64 arithmetic preserved. See
[m5-lock-basket-claim.md](m5-lock-basket-claim.md).

The binary routes generated transactions/queries, config/tools/explain,
weights/axon, wallet management, subnet registration/metagraph views and the M4
hparams commands. Sequential `trim-null` now uses the shared SDK session,
canonical progress checks, retained approval opening and durable receipts.
Source review corrected multi-round continuation and preview waiting. The
executor refuses rebuilt calls that differ from the journaled review before
private acquisition. Source fixtures include one command completing both saved
multisig rounds. This integration awaits hosted compilation and execution.

M5 offline preparation, strict unsigned-JSON import, signature submission and
shared multisig book/cache writers are committed. The importer authenticates
all signing views, refuses serde private carrier injection and preserves nonce
cache state for invalid, stale, expired or foreign preparations. Cache lookup
authenticates exact canonical call bytes. The M5 fuzz target is source only;
no fuzz campaign ran. The remaining 62 reads, other command families and
later milestones still require implementation and acceptance. The full intent
source and M5 CLI integration still require combined hosted acceptance.

Latest inspected normal hosted run
[38710157011](https://github.com/sam0x17/rtao/actions/runs/38710157011)
at `c6bd593` passes MSRV, no-host, supply and semver. The check job compiles
the three corrected sites, then strict Clippy rejects a test MutexGuard whose
borrowed request remains in scope across client.close().await. A lexical
block now ends the guard and request borrows before that await, retaining the
RPC hash assertions. Features remains running at inspection. Tests, doctests
and docs builds were not reached. Another ordinary hosted check is required;
no local build was run.

Earlier normal hosted run
[38709461377](https://github.com/sam0x17/rtao/actions/runs/38709461377)
at `5cba852` fails check/MSRV compilation. Multisig reference collection needs
one named borrow lifetime; the new policy fixture compares NetUid to u16,
and the plan fixture assumes an unavailable H256 FromStr implementation.
Source corrections share the actual ArgMatches borrow, project subnet IDs
without changing order or duplicates, and strictly decode the fixed-width
public PoW hash. No assertion or gate is removed. No-host, supply and semver
pass; features remains running at inspection. Tests, doctests and docs builds
were not reached. The next ordinary hosted run must validate the corrections.
Bounded private evidence is `check-38709461377.json`. Local output remains
534 MB in target and 146 MB in recovery; no local build or scratch root.

Earlier normal hosted run
[38707066144](https://github.com/sam0x17/rtao/actions/runs/38707066144)
at `bad95c0` passes compilation, strict Clippy and all five other normal jobs.
Parity now qualifies the lock fixture, then rejects AddStake's helper-based
fixture for missing its explicit class token. Meaningful existing trade and
claim-aware fixture bindings now name their concrete intent types. A bounded
lexical source audit finds assertion-body references for all 83 classes; the
hosted Cargo/cfg/harness qualification remains authoritative. Tests, doctests
and docs builds were not reached. Private evidence is `check-38707066144.json`.

The final ten ports, 20 M5 CLI routes, 21-constructor policy fixture, all-83
plan fixture and transaction reference have verified generated integration.
Run [38709002368](https://github.com/sam0x17/rtao/actions/runs/38709002368)
at `a929bcd` was verified and adopted unchanged: fourteen exact regular archive
members/digests, 23 dependency hashes, unchanged role inventories, ten pending/
availability changes, four guarded constructors, four bounded JSON bodies,
72 ordinary plan mappings, D-046's new fixture and D-047/D-048. Other generated
interfaces and matrix families remain unchanged. Counts are 83/83 intents and
40/102 reads. Combined hosted execution and acceptance remain pending.
See [m5-final-intents.md](m5-final-intents.md). No new filing or local build.

Earlier normal hosted run
[38705671132](https://github.com/sam0x17/rtao/actions/runs/38705671132)
at `93f562c` passes compilation, strict Clippy and the five other normal jobs.
Parity rejects the lock-settings fixture because its dynamic dispatch does
not explicitly reference the three Rust classes required by the pending gate.
The fixture now matches MoveLock, SetPerpetualLock and SetRejectLockedAlpha
and asserts their decoded fields before checking bytes and previews. The gate
is unchanged. Tests, doctests and docs builds were not reached. Bounded failure
evidence is retained privately as `check-38705671132.json`.

Earlier normal hosted run
[38705303263](https://github.com/sam0x17/rtao/actions/runs/38705303263)
at `1c807e1` fails compilation in check and features because `SimSwapResult`
needs client `ReadValue` registration. The existing model already derives
typed decoding; the source registration is corrected. Supply, no-host and
semver pass; MSRV remains in progress. Tests, doctests and documentation builds
were not reached. Bounded logs and diagnostics are retained privately as
`check-38705303263.json`.

Earlier normal hosted run
[38703332560](https://github.com/sam0x17/rtao/actions/runs/38703332560)
at `6470a11` passes supply, MSRV, features, no-host and semver. The check job
stops at strict Clippy: the leasing pass-through generator clones its Copy
`Percent` and `Option<u32>` arguments. The source generator now recognizes
these Copy types, including optional Copy values. Verified generated output
is adopted; tests and doctests were not reached on this checkpoint. The bounded
private failure record is `check-38703332560.json`.

Seven further ports have verified generated integration: Batch, SetIdentity, SwapHotkey, AddStake,
AddStakeLimit, SwapStake and MoveSwapStake. Batch uses a unanimous instance
signing role, bounded child dictionaries and full semantic preflight aggregation.
The SDK typed entry points refuse a role mismatch before key access; dynamic
and CLI paths use the selected instance role. Hosted execution remains pending.
Generation run [38704675475](https://github.com/sam0x17/rtao/actions/runs/38704675475)
at `429f727` was verified and adopted unchanged: fourteen exact archive members,
23 dependency hashes, unchanged role inventories, seven availability/pending
changes, one identity helper, two leasing Copy corrections, one guarded Batch
constructor and one normalized JSON body. The matrix changes only those seven
intent statuses and D-046's Batch fixture reference. Integrated coverage is
73/83 intents and 40/102 reads; no runtime coverage or milestone exit is claimed.
See [m5-batch-trades-identity.md](m5-batch-trades-identity.md).

Earlier normal hosted run
[38702708753](https://github.com/sam0x17/rtao/actions/runs/38702708753)
at `359b09e` passed supply, no-host and semver but failed check, MSRV and
features during compilation. The collateral read requires registering the
existing `MinerCollateralState` decoder as a client `ReadValue`. Root-claim
fixtures also used the wrong public helper path and an unused import. These
three source corrections are committed in `6470a11`; tests and doctests were
not reached.

Earlier normal hosted run
[38701008514](https://github.com/sam0x17/rtao/actions/runs/38701008514)
on `b0e0aa1` passed compilation, strict Clippy, parity and five other jobs.
Nextest ran 1,872 tests: 1,871 passed, one failed and four were skipped.
All eighteen earlier failures are resolved. The remaining new child-take
fixture expected the metadata default while the Python fake supplied a blanket
map value. The source fixture now seeds an explicit map miss so the actual
ValueQuery default is exercised, retaining the equality-refusal assertions.
Doctests were not reached. This run does not validate the later seventeen ports
or money JSON correction. The bounded failure inventory and log digest are
retained privately as `check-38701008514.json`.

Earlier normal hosted run
[38697413448](https://github.com/sam0x17/rtao/actions/runs/38697413448)
on `60ccce5` passed compilation, strict Clippy, the parity gates and the five
other normal jobs. Nextest ran 1,850 tests: 1,832 passed, 18 failed and four
were skipped. Doctests were not reached. The failures cover CLI schema/output
fixtures, wallet accounting fixtures, stale pending-operation assertions and
an unknown SS58 prefix in a coldkey fixture. Their source fixes and the next
thirteen-intent batch are committed; this run does not validate that batch.
The bounded log digest and exact failure inventory are retained privately as
`check-38697413448.json` in the recovery directory.

Earlier normal hosted run
[38695489489](https://github.com/sam0x17/rtao/actions/runs/38695489489)
on `fa77262` passed MSRV, features, supply, no-host and semver. Compilation and
strict Clippy passed, and D-045's corrected fixture qualified. Parity then
rejected D-046 because it used the default-feature test index, while the normal
nextest and doctest commands enable all features. The source correction aligns
divergence qualification with the indexed ordinary all-feature tests, preserving
cfg, ignore and harness rejections. Earlier missing-import, rendering and
receipt-fixture API issues are corrected in source. Nextest and doctests were
not reached; both still need to pass on the final combined source. Current runs are listed in the
[workflow history](https://github.com/sam0x17/rtao/actions/workflows/pr.yml?query=branch%3Amain).

Hosted regeneration
[38690259736](https://github.com/sam0x17/rtao/actions/runs/38690259736)
and report refresh
[38691760795](https://github.com/sam0x17/rtao/actions/runs/38691760795)
were verified and adopted unchanged in `5f4b8fa` and `3afa90b`.
Review checked artifact digests, exact regular-file members, all 23 dependency
hashes, unchanged role package inventories, the two read-status changes and
D-044/D-045 regression references. Private bounded records are
`m5-read-adoption.json` and `m5-persistence-adoption.json` under
`.local/recovery/2026-10-14/`. Report refresh
[38692634551](https://github.com/sam0x17/rtao/actions/runs/38692634551)
was verified and adopted in `a9c720c`, covering the CLI owner feature and sha2
dependency. Its proof is `m4-pruning-adoption.json` in the same recovery directory.
Hosted generation
[38694967521](https://github.com/sam0x17/rtao/actions/runs/38694967521)
at `70fca29` was verified and adopted unchanged. Exact archive paths/digests,
all 23 dependency inputs, unchanged role inventories, twelve pending-body
removals, nine pass-through additions, the pre-serde guard and matching matrix
changes were checked. Its proof is `m5-crowdloan-root-adoption.json`.
Hosted generation
[38696143491](https://github.com/sam0x17/rtao/actions/runs/38696143491)
at `18679e3` was also verified and adopted unchanged. Its thirteen exact
members include only four availability/pending changes and three call-map
additions; all other generated interfaces and matrix families are unchanged.
All 23 dependency hashes include the updated CLI feature declaration, with
unchanged role package sets/counts and no runtime claims. Its bounded proof is
`m5-coldkey-adoption.json` in the same recovery directory.
Follow-up generation
[38696690648](https://github.com/sam0x17/rtao/actions/runs/38696690648)
at `7c9c8ca` corrects two unused zero-argument pass-through bindings. Review
verified the same thirteen paths and 23 hashes, exactly two parameter-name
changes, unchanged other interfaces/matrix and unchanged role inventories.
Its proof is `m5-fieldless-adoption.json`.
Runtime validation remains null/empty; Linux graph measurement does not
establish native Mac, hardware, oracle or chain interoperability coverage.

No local compilation, tests, Python, mining, wallet operation or chain service
ran in this continuation. Local checks were source inspection, rustfmt and
diff checks. One bounded offline lock resolution added no package versions.
Archive and artifact handling used memory; no temporary root was created.
The retained target output was not rebuilt. Wallets, shared caches, backups and
other projects remain untouched. See [m0-acceptance-audit.md](m0-acceptance-audit.md)
and [disk-cleanup.md](disk-cleanup.md).

## Disk cleanup completed

Removed the entire task-created
`/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC` directory. Its measured
allocation before cleanup was 335,709,460 KiB (320.16 GiB). The task-owned Cargo
cache shared inside that temporary root and fifteen additional compiler target directories were removed.
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
The recovery directory is excluded from Git and Docker contexts; its README and source
manifest describe recovery. Preserving a candidate does not adopt or
validate it. See [disk-cleanup.md](disk-cleanup.md).

The pre-existing `target/debug` is preserved; the remaining complete target
directory is 547,192 KiB (534 MiB). Shared Cargo/toolchain caches, other
projects and all wallets/recovery state are untouched. Retained durable
wallet/recovery data: `/Users/sam/.local/share/rtao/testnet-soak`. No service
or task still depends on the deleted temporary root. Time Machine snapshots
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
`sam@matthews-mac-studio`, with low compute use because the host is shared.
Read-only SSH host inspection passed. Builds and PoW stay off that host;
the planned validator has two Tokio worker threads, lower scheduling priority,
resource measurements and bounded log retention. The reviewed native wallet
helper passed seven fixture tests, including 24 altered complete-state cases,
native encrypted recovery and no-replacement failures. Root audited source,
binary/artifact hashes and dependencies, then prepared the new local wallet and
verified identical public-only reinspection. The coldkey funding address is
`5G3pwJ3ptR4x83zuKzFvTC2TA8WeivVtP1NhgxDxPmi2RAvX`.
Wallet/recovery data are retained at `/Users/sam/.local/share/rtao/testnet-soak`
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
build host is `Sams-MacBook-Pro.local`; the selected remote soak host is
`Matthews-Mac-Studio.local`. No remote scratch directory was created by the
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

## Active source-artifact task, 2026-10-15

No temporary root remains. Hosted archives and logs are inspected in memory;
only bounded adoption and failure records are retained in the existing
private recovery directory. No local builds or workspace copies are created.

The Q30 follow-up audit removes the stale macOS-local requirement and adds
hosted documentation coverage to the existing check job: strict rustdoc, a
rendered book, generated API/LLM index and downloadable development artifacts.
The docs command has a 15-minute limit, reuses the hosted target and requires
GitHub's Linux runner environment. Hosted execution remains pending.
