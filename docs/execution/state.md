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

## Current client checkpoint, 2026-10-16

The latest completed ordinary run is
[38865058430](https://github.com/sam0x17/rtao/actions/runs/38865058430)
at `3f1aa6731abfccb15a12c2a52da23ff08850f278`. All six hosted jobs passed.
The main gate passed all 3,040 executed tests, zero failed, four skipped,
workspace doctests, strict Clippy, documentation and every blocking check.
Its 715,611-byte log has SHA256
`55de87df70e9afc863da7175a9dc7ed809ce239c310f8b2e2eef0b84ef46e975`.
Bounded proofs are `check-38865058430.json`, `3f1aa67-named-results.json` and
`m0-3f1aa67-hosted-results.json`. The named proof binds all 26 new CLI,
extractor and pruning fixtures, 26 existing PoW/fake-GPU fixtures and four
actual sample consumers to exact PASS records and executed source blobs.
The separate M0 proof binds all six jobs, 60 audited inputs and 123 exact
foundation fixture results. No raw log or archive is retained.

The following preceding security checkpoint retains its own exact source and
review evidence. Later client work is recorded separately below.

Ordinary hosted
[38832708259](https://github.com/sam0x17/rtao/actions/runs/38832708259)
passed at `7bb8c0b888b5e634c8ca1236f05ceb9b6f81a48a`. All six jobs
passed: check, no-host, features, MSRV, semver and supply. The main gate passed
strict Clippy, source/pin/extraction checks, documentation generation, all
3,013 executed tests (four skipped) and workspace doctests. The documentation
artifact uploaded successfully. This supersedes the earlier execution-pending
claims for the integrated EVM, pruning, compiler, transport and extractor
fixtures below. It supplies no Python/oracle, live-runtime, device, sanitizer
or native-platform result.

The authenticated main log is 927,796 bytes with SHA256
`88bc33d05df3a5c14789d615940791ce65c1e6486726926f1b25de2dc15933d3`.
Bounded evidence is `check-38832708259.json` and
`7bb8c0b-hosted-results.json` under `.local/recovery/2026-10-15/`;
no raw log, archive or extraction directory is retained. A separate exact-name
extraction, `m9-security-current-ci-38832708259.json`, confirms all 70 selected
S9 regressions and all 12 new M2-S03 parser/public-view/differential regressions
passed on this commit. Independent source review and exact executed results
close the original S9-01 through S9-10 defects and M2-S01/S03/S04/S05, retaining
the limits in [M9](../security/m9.md#current-dispositions-at-7bb8c0b) and
[M2](../security/m2.md#current-hosted-dispositions-at-7bb8c0b).

The reviewed source map and adopted generated matrix now agree at 303/357
reconciled and 54 pending after the authenticated refresh below.
There remain 20 pending e2e mappings and 68 divergences. Passing CI does not
reconcile the remaining source rows or accept a milestone. Strict M8 remains
off and M2-S02 remains open/unaccepted. Current work continues through M9,
with source review and substantial validation on free hosted CI.

Local `target` is absent, including the retired `execute-plan.6Ro0PC` root.
No local build/test, service, Mac Studio work, new temporary root or upstream
filing was needed. Preserve wallets, recovery material and unrelated work.

### Verified weight-clipping repair

Source review found a public-helper result mismatch for finite minimum-subnormal
scores: clipping can underflow its final denominator to zero. Python raises;
Rust previously returned successful NaNs. Refuse either sign of zero with a
native `WeightsError` and Python's message before division. Existing `conform`
already rejected those NaNs before call composition; no invalid submission was
established. The regression exercises both helper and conform, plus valid
subnormal normalization, unclipped output and uniform fallback branches.

Independent source review, direct rustfmt and whitespace checks pass. Hosted
run 38858957306 now verifies compilation and this regression. The later complete
source review reconciles weights in the batch below; native checked u64 balances
and raw-weight semantics are unchanged. Bounded proposal/review evidence remains
under the existing recovery root, with no build output or extra temporary directory.

### Subsequent source disposition and verified report refresh

Complete independent review reconciles `codegen/check.py`: all six functions,
declarations, native producers/consumers and gate callers have explicit homes.
The metadata-name/Unknown-classification and Python-strip-nonblank repairs both
have exact PASS records on `7bb8c0b`. Native pinned regeneration and Result/CLI
errors replace Python live generation and stdout conventions; fresh Python,
oracle and live drift remain unverified.

Hosted [38859189354](https://github.com/sam0x17/rtao/actions/runs/38859189354)
passed full source parity at clean `6c4f82c0618c81b12784eefb97875b2ad379f724`.
Artifact 12029210729 is 58,018 bytes, SHA256
`da3e79b71704bf997cc96d26aa544949de9330f4a1f47dc0d8d5e736c8802159`.
Authenticated inspection and independent refetch supply exactly four adopted
reports. The only matrix change is the reviewed check.py row, reaching 298/357
reconciled and 59 pending. All 23 dependency inputs, role counts, host flags,
20 pending e2e mappings, 68 divergences and other compatibility claims remain
unchanged. Bounded proofs are `6c4f82c-report-inspection.json` and
`6c4f82c-report-adoption.json`; no archive or extraction tree is retained.
Ordinary [38858957306](https://github.com/sam0x17/rtao/actions/runs/38858957306)
at `50b7239` remains the weight-fix validation checkpoint. Other pending rows,
strict M8 and milestone status are unchanged.

### Earlier source and CLI batch

The current `3f1aa67` result supersedes this batch's earlier unexecuted-fixture
and pending-report statements. Their source scope and coverage limits remain.

Complete authenticated source reviews reconcile staking (all 14 intents),
weights (all 19 functions/methods) and the shared pruning product workflow.
Direct native homes and secondary attributions are recorded, with explicit
typed/native policies and separate pytest/oracle limits. Correct the architecture's
stale summation claim: CPython 3.12 sums are compensated; cumulative sums remain
left folds. Source map: 301/357 reconciled, 56 pending. The adopted matrix remains
at 298/59 until the next authenticated hosted refresh.

Repair the reversed account-name cache at the actual AppContext confirmation
boundary. Two fixtures check local wallet/hotkey choices, contacts, qualified
references, pasted addresses and unchanged selected identities. Also finish
public prompt guidance after successful commands using original argv, with
JSON/quiet/dry-run suppression and escaped input line breaks. The price command
teaches its actual native SDK read. Five parser/handler/finish and fake-client
fixtures cover these behaviors. Independent source reviews and formatting pass;
these seven new fixture functions remain unexecuted. Prompt/output source rows
stay pending their remaining caller/content reviews. No local builds or tests ran.

### CLI follow-up and failed report refresh

Report-only hosted [38862240133](https://github.com/sam0x17/rtao/actions/runs/38862240133)
failed at exact `b59fd13140ddfac01281e5a3b816b85219231e44`: the license guard
could not attribute the newly reconciled pruning home in `rtao-neuron`. Its
combined two-source comment did not supply the exact canonical upstream path.
Split that comment into one canonical header per source without changing the
guard. No report artifact was adopted. The 59,499-byte job log has SHA256
`0971f8c9149cd5390fb2b53e9df618110d4ae8278cf6538bd9ab7d73ad60a681`;
`check-38862240133.json` retains bounded failure evidence.

Independently reviewed CLI follow-ups restore transfer-history dates, signed
balances, names, failed markers and checked successful root-TAO totals at the
actual command/renderer boundary. JSON retains the original records, including
unknown fields; human total overflow refuses before partial output. Chain
errors offer `rtao explain` only for a catalog member or known semantic code.
Also confirm a default local wallet before omitted sign/decrypt inputs, using
the ordinary public context and preserving original argv/history. Strict
parsing, environment/flag precedence and noninteractive suppression remain.
Four output/command and six wallet-order fixture functions are added; they and
the earlier seven fixtures await hosted compile, lint and execution. Direct
rustfmt and whitespace checks pass. Output/prompt and pytest source rows remain
pending; source/matrix counts, strict M8 and milestone status are unchanged.

### Corrected reports and the next hosted test batch

Hosted [38862997033](https://github.com/sam0x17/rtao/actions/runs/38862997033)
passed full source parity at clean `f48535dbfd3b75dcdc9511c259b322c2b9a8ab38`.
Authenticate and independently refetch artifact 12029777947 (58,166 bytes,
SHA256 `ebff9b28aa3c2329462ba83031580e438135c7e97e69e598a434fefe83d8a95d`)
before adopting exactly four reports. Only the three reviewed staking, weights
and CLI-pruning rows change in the matrix: source map/matrix now agree at 301/357
reconciled and 56 pending. All 23 dependency inputs, role counts, host flags,
20 pending e2e mappings, 68 divergences and other compatibility claims remain
unchanged. Bounded proofs are `f48535d-report-inspection.json` (SHA256
`ab3137fe89e8d2e3ca3cc865126ea1a6d278d6ca9d50d339244f6a0269ff6ed9`)
and `f48535d-report-adoption.json` (SHA256
`469ff440fe3b23ceae2991634b412ff20980935d86042da9b8f825a19d06c7e1`).

The subsequent independently reviewed extractor repair verifies the sample
constructor's imported native identity. Its samples-only closed producer policy
rejects mutations, unsupported calls and imports, changed implicit callbacks or
their builtin dependencies, failed unused declarations/defaults and unknown
backend exports. Previously cached producer modules are rechecked. Five actual
resolver fixtures retain all six original outputs/spans and exercise the
refusals; no currently incorrect pinned sample value was established. The
samples row remains pending hosted validation and final whole-source disposition.

Four new pruning fixtures drive real session/Executor/FakeChain paths for the
two-mechanism implicit target128, pending-target priority, complete 32 key-only
queue probes and empty TWC values at the last legacy index. Python origins and
native vector adaptations are explicit. The complete pytest row remains pending
its admin table and integrated cooldown/recovery cases. These nine additions
and the preceding seventeen CLI fixtures await hosted compile/lint/execution.
Direct rustfmt and whitespace checks pass; no local build/test or retained
archive/extraction tree was needed.

A read-only 2026-10-16 06:50 UTC refresh confirms that all thirteen recorded
upstream filings remain open, all eight PRs are unmerged at their reviewed heads,
and their label/review-request lists are empty. Evidence is
`upstream-state-2026-10-16.json`; this refresh did not repeat the authored-search
census or source/test audit. No GitHub filing write occurred.

Ordinary [38863766724](https://github.com/sam0x17/rtao/actions/runs/38863766724)
at `9bd2d02788ebf22c546f8c59099386f9ab976524` exposes E0382 in the MSRV
CLI binary check: an async move handler consumed original replay argv before
the newly wired finish boundary borrowed it. Retain the owned vector outside
dispatch and give the seven handler closures a shared slice. No replay value
or prompt behavior changes. Direct formatting and whitespace checks pass;
the corrected ordinary checkpoint must establish compilation and the 26 new
fixture results. Bounded MSRV evidence is `check-38863766724.json`, 58,981-byte
log SHA256 `c776d6f6862cc8b4af9ba8737660b973258c7269eed3f15c3e51ab6088b5d935`.

Corrected ordinary [38864132855](https://github.com/sam0x17/rtao/actions/runs/38864132855)
at `821142045101e74137f673c6c31890f514527a0a` passes MSRV, no-host and
semver. Its main job stops before tests on two `needless_borrows_for_generic_args`
findings in the new wallet fixture's environment closure. Pass the Copy closure
directly at both calls. Formatting passes; the 26 new fixtures still require
execution on the corrected checkpoint. Main failure evidence is
`check-38864132855.json`, 68,704-byte log SHA256
`0876f601d04385eb3305af03cfd868c69bd710d0869e004849f0f032617cdb95`.

### Verified source dispositions and the next root-picker repair

All 26 additions in the preceding batches pass at `3f1aa67`. The exact named
proof is 24,010 bytes, SHA256
`87b0f69da8e44828bca1d1d0d48788fcd74f60b48950e57bd0d7143ef7dc587d`.
Independent whole-source review plus the five actual-resolver, four all83/all102
consumer and 26 native PoW/fake-GPU PASS records reconcile `samples.py` and
`pow_registration.py`. Canonical origins are added to their actual native homes.
Source map reaches 303/357 reconciled and 54 pending; generated matrix remains
301/56 pending hosted refresh. Typed public fixtures replace internal dev_wallet;
PoW returns a typed proof and retains explicit backend, nonce-search, cleanup and
valid-challenge policies. No arbitrary-prefix Python helper, physical GPU,
fresh oracle, complete pytest/e2e equivalence or strict M8 claim follows.

The independent M0 audit and 123 named current results supply the foundation
completion record. Source/fixture steps are checked; the combined follow-up
checkpoint must still pass applicable hosted checks before milestone exit.
M1's finite source/test/documentation gaps remain recorded separately.

New source review found S9-11: a root-picker display label can override another
listed validator's full address. Restore numeric/address/label precedence and
add one actual allocation-preparation fixture with collision and control cases.
It checks typed intent args, exact replay and absence of author RPCs. Independent
source/API/fixture review and direct formatting pass; this new fixture is not
included in the `3f1aa67` execution result and awaits hosted validation. The
whole root-helper row remains pending. [M9](../security/m9.md) records the finding;
M2-S02 remains open/unaccepted and no new upstream filing was made.

### M1 source-extracted RPC fixtures and maintained TLS check

The current M1 audit identifies two hand-copied pytest input tables, seven
missing exact test origins and a two-provider check retained only as historical
evidence. Replace the tables with a guarded Rust AST producer over the pinned
`test_rpc.py`, preserving bytes/tuple carriers, signed codes, ordering and
source/decorator spans. Three actual-resolver fixtures exercise literal updates
and refusal of changed bindings, executable defaults and unsupported shapes.
Existing policy tests consume the generated corpus directly. Independent source
review and formatting pass; generation, compilation and execution are pending.
Whole pytest source reconciliation and refused-connect cleanup coverage remain
separate obligations.

Prepare an isolated, nonshipped `ci/tls-two-providers` workspace for the actual
RPC/FakeNode TLS exchange with both ring and AWS-LC compiled for Rustls 0.23.46.
The existing features job will run one serialized test after an independent
lock is generated/reviewed through the bounded hosted preparation mode. Shipped
locks and dependency policies remain unchanged. Neither preparation nor source
review establishes compiled-provider execution or SignedClient HTTPS coverage.

Five M1 execution documents now distinguish historical results from Q30's
current hosted-only checks, deferred infrastructure and absent retired caches.
The next source checkpoint skips ordinary CI until its generated corpus,
four reports and isolated lock are authenticated and adopted. The subsequent
combined checkpoint must pass before milestone acceptance. Source map stays
303/357 reconciled, 54 pending; adopted matrix stays 301/56 pending generation.
No local Cargo/build/test, service, Studio work, new temporary root or filing.

### Authenticated M1 preparation and combined follow-up

Both preparation runs passed at exact `d5132a0`: codegen/report run
[38871417189](https://github.com/sam0x17/rtao/actions/runs/38871417189) and isolated
TLS-lock run
[38871434589](https://github.com/sam0x17/rtao/actions/runs/38871434589).
All 19 generated members, four reports and two TLS preparation members were
inspected in memory and independently refetched. Exactly six generated/report
paths plus `ci/tls-two-providers/Cargo.lock` were adopted. The bounded coordinated adoption
proof is 18,348 bytes, SHA256
`ce9a3d479f0d86644b449f770c874715ad4576d7f003bfc0c32997d9b0dc81de`.
The two new RPC tables retain all seven literal cases and exact source spans;
all previous corpus/catalog bytes and 17 other generated members are unchanged.
The matrix promotes only the reviewed samples/PoW rows to 303/54. All 23 shipped
dependency inputs, role counts, host flags, 20 e2e gaps and 68 divergences remain
unchanged. No archive, extraction tree or local target was created.

The isolated lock is 127,044 bytes, SHA256
`f193c79d7eab8fd4bbc06c7e0099a3bb3f8fdb46b39be8719c72c5fcd9730fe1`.
Graph proof requires one shared Rustls 0.23.46 with ring and AWS-LC features.
Nine identities are new relative to the shipped lock, including the fixture;
all git identities remain pinned. Read-only RustSec checks find the listed
AWS-LC and generic-array advisories fixed in the resolved versions. This manual
range review is not a fresh full cargo-audit result. Shipped supply gates and
AWS-LC bans remain intact. The compiled-provider test still requires execution.

Five independently reviewed actual root-command fixtures and the strengthened
pagination assertion are integrated. The repair restores cached subnet identity/unit, compact
fund addresses, ordered local/contact/NAV aliases, matchable-key prefix ambiguity
and the canonical hint's upstream quiet exception. Exact number/address
reservations remain. Keep whole root-helper and command rows pending until
hosted results and their separate complete-source dispositions are reconciled.

The complete RPC review traces all 40 functions and 91 explicit assertions.
A real refused-initialization socket observer and the batch's ten-result
cardinality assertion are integrated; existing cleanup and batch production code require no repair.
Metagraph review finds one supported direct `from_dynamic` text gap: Python
accepts ASCII separators between hex byte pairs. The repair shares the existing
exact commitment parser through `pyfmt`, preserves commitment diagnostics and
corrects three source spans. One new public adapter fixture covers accepted separators,
split-nibble refusal and non-ASCII error precedence. Canonical chain text uses
byte lists; no wire-path failure was demonstrated.

The M1 resource audit covers all 23 execution documents and corrects 17 further
historical cache/reproduction/platform requirements. Direct formatting and
whitespace checks pass. The combined hosted run must execute the authored
fixtures; the expected Nextest total is 3,051 plus one separate TLS test.
Preparation alone accepts no milestone or full source/pytest row. Strict M8
remains off, M2-S02 stays open/unaccepted, and no local build/test, service,
Studio work, new temporary root or filing occurred.

## Prior integrated client checkpoint, 2026-10-15

The following records describe earlier checkpoints. Their pending execution
claims and dispatch instructions are superseded by the current result above;
their source-review and coverage limits remain applicable.

The current source batch reconciles thirteen complete source relationships: all
ten EVM CLI modules, basket display pricing and both optional recorders. The
source map and adopted generated matrix agree at 297/357 reconciled and 60
pending. Seven EVM input corrections
preserve exact-empty option semantics, one shared fund-all decision and the
alpha-only root guard. Fourteen new injected fixture functions exercise actual
handlers, intent construction, ABI words, preview/confirmation and refusal
boundaries. They compiled under hosted Clippy but have not executed yet. A distinct-price basket
fixture now detects incorrect dust inclusion and the exact cutoff.

The same batch repairs first-approval journal authentication: derive the original
multisig opening from the retained receipt/event and explicit reviewed timepoint,
compare block-end state, then repeat that authentication on restart. New and
explicit nonexecuting approvals each have same-opening/replaced-opening cases;
old artificial receipts now use their actual inclusion position. Preserve raw
evidence, D-023, D-041, S9 and no automatic reopening. Already-Yuma no-op ordering
is corrected. A separately reviewed timer repair now starts the wait budget on
the first wait and clears it after each accepted batch, matching Python. A final
partial sleep permits a state recheck; cancellation and failed-window retries
retain the deadline. Three paused-time FakeChain fixtures cover these boundaries
without changing three-stall, journal, finality or submission-draining guards.
Both complete null-pruning rows remain pending, including their full pytest
mapping and hosted execution obligations.

Complete codegen/check.py review found missing metadata/classifier and nonblank
prose guards. Native parity now checks those contracts with targeted source
fixtures; the whole-file row remains pending follow-up review and hosted tests.
Source sample declarations have a separate bounded proposal, also still pending.
No Python/oracle, device, live runtime or sanitizer execution is inferred.
All new fixtures and strict M8/milestone acceptance remain unverified; M2-S02
is open/unaccepted. Bounded source/adoption reviews live under the existing
recovery root; no build output, archive or extraction tree was created.

Hosted report refresh
[38826175917](https://github.com/sam0x17/rtao/actions/runs/38826175917) passed
at clean `d2d5add9eb68806f37d1b90eac372b0db97ac0c9`, including full source
parity. Authenticated and independently refetched artifact 12015413800
(57,978 bytes, SHA256
`f3af367e6632bef6c2200ca8fee15bb86387e81f5598e348337ca4db52ffa8da`)
supplies exactly four adopted reports. Matrix changes are limited to the thirteen
reviewed source promotions and two appended D-041 tests. All 23 dependency
inputs, role inventories, host flags, 20 pending e2e mappings, 68 divergence
identities and other compatibility claims are unchanged. Inspection and adoption
proofs are `d2d5add-report-inspection.json` and `d2d5add-report-adoption.json`.
The timer patch has independent source approval in
`null-pruning-wait-independent-review.txt`; its three fixtures and the integrated
compiler, transport and extractor fixtures await ordinary hosted execution.

Ordinary [38827263561](https://github.com/sam0x17/rtao/actions/runs/38827263561)
at `dc5c09a8a0e451eaee256cfc517b5a68c8782b56` passed all five supporting
jobs: supply, MSRV, feature graphs, semver and no-host. Its main job passed
Clippy for all targets and all source/pin/extraction checks, then failed strict
rustdoc on two unqualified module links to timelock `seal` and `open`.
Qualify both public targets without changing behavior or lint policy.
The 106,372-byte terminal log has SHA256
`e30baf51a75ef2456d0d676950754e4be50e48c07bd0e9350cbc8dcc331bdafd`;
bounded proof is `check-38827263561.json`. Docs precede nextest in the current
gate, so behavior tests and workspace doctests were not reached. The compiler
corrections are verified, but current fixture execution and documentation
completion remain unverified. The separate weights, staking and source-map
proposals remain ignored and unapplied while this documentation gate is fixed.

Ordinary [38824416747](https://github.com/sam0x17/rtao/actions/runs/38824416747)
at `d9ae66842d0cdb261b353ea0284b1f5936c324b4` passed supply, MSRV and
semver; no-host failed strict Clippy on session.rs's Url import,
which the handshake refactor left needed only by tests and frame fuzzing.
Gate that import with the existing test/fuzzing condition; do not suppress
linting. The terminal no-host log is 184,651 bytes, SHA256
`9254c2e86e6d7a6fc83d9ad914780d14822329ec028150f3987a91e8f68a2355`.
Bounded proofs are `check-no-host-38824416747.json` and its authenticated detail
follow-up. Main also failed during strict Clippy compilation: the new public RPC
fixture used primitive H256 where RpcError::FateUnknown requires RPC H256.
Qualify that one constructor while retaining native hashes in client APIs.
Its 77,984-byte terminal log has SHA256
`351ce4a11a4b49f367c5bbe81e8f28722e76af135fb38b564df3a1214fd1cb3d`;
bounded proof is `check-main-38824416747.json`. No tests or documentation ran
there. The feature job subsequently passed and the run is terminally failed.
Execution of both corrections remains pending. The reviewed source batch and
report refresh above are ready for the next combined ordinary checkpoint.

Hosted report refresh
[38823630645](https://github.com/sam0x17/rtao/actions/runs/38823630645) passed
at clean `7cc750178a17d89424b0c95616d493adab16b52c`. Its 57,584-byte artifact was
authenticated and independently refetched; exactly four reports are adopted. The matrix
changes only the two reviewed precompile/RPC source rows, reaching 284/357
reconciled and 73 pending. All 23 dependency inputs match that source; exactly
the reviewed workspace lock and RPC manifest hashes change. Role inventories,
host flags, 20 pending e2e mappings, 68 divergences and other compatibility
claims remain unchanged. Bounded proofs are `7cc7501-report-inspection.json`
and `7cc7501-report-adoption.json`; no archive or extraction tree is retained.
The combined hosted gate must still execute the newer transport fixtures,
constructor-refusal correction and documentation fix. Strict M8 remains off,
M2-S02 open/unaccepted and no milestone is accepted.

Hosted report refresh
[38819346876](https://github.com/sam0x17/rtao/actions/runs/38819346876) passed
full source parity at clean `d8db07d43f5c7e32623e8614b2e09f07629abeb9`. Adopted
exactly four report files after authenticating and independently refetching
the archive. Matrix changes are limited to the two reviewed substrate/interface
rows. All 23 dependency inputs, role counts, host flags and other compatibility
claims are unchanged. Bounded proofs are `d8db07d-report-inspection.json` and
`d8db07d-report-adoption.json`; no archive or extraction tree is retained.

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

The published matrix has 297/357 source rows reconciled, 60 pending, 20 pending
e2e mappings and 68 divergences. Source mappings retain explicit native API,
strictness and coverage limits. These are source reconciliation results.
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
Ordinary [38818850318](https://github.com/sam0x17/rtao/actions/runs/38818850318)
completed at `f21c0919fcf42a73c09e9903d8a8bffa3180d562`. All five supporting
jobs passed. The main gate ran 2,977 tests: 2,976 passed, one failed, four skipped.
The failure is the extractor's constructor-semantic refusal fixture; doctests
and documentation were not reached. The exact 773,213-byte log has SHA256
`572428c4f9513974a72b200c53e5fc947ab20fe882f258bccf469a74b5818754`;
bounded proof is `check-38818850318.json`.

An authenticated second read of that immutable log confirms seven exact named
passes: positive unsigned-seed reconstruction/signing/attachment, Utility.batch
roundtrip, all three canonical CLI notice fixtures, source-table mutations and
comprehension refusals. Proof is `f21c091-named-test-results.json`. The positive
seed is validated through its ordinary fixture; fuzz-driver/sanitizer execution,
fresh Python/oracle results and newer-source test success remain unclaimed.

The failed refusal comes from RustPython 0.4 dropping singleton parentheses and
deriving AnnAssign.simple from the remaining Name node. The lexical correction
checks the original statement-to-name source span, retaining the parenthesized
case and adding nested/index variants plus exact mutation diagnostics. Direct
formatting and source inspection pass; hosted execution of the correction is
still required.

Source checkpoint `d8db07d` repairs terminal policy refusals during paced
reconnect setup, with four real-session regression fixtures. It also normalizes
public RPC diagnostic text while retaining typed causes and policy metadata,
with eleven fake-startup vectors. Both patches passed independent source review
and direct formatting checks; neither has executed in hosted tests yet.
Complete substrate/interface reviews added two qualified source dispositions;
at that checkpoint the map and matrix agreed at 282 reconciled and 75 pending.
Native read/write API ownership, raw pool-only submission and map-presence
capability/coverage limits are documented.
The next source checkpoint repairs fragmented HTTP rejection-body diagnostics.
It retains the complete TLS transport through rejection and preserves the actual
verified WebSocket on success. Capture keeps at most 4096 entity bytes under a
64 KiB wire budget, 1024-byte chunk lines and the original handshake deadline;
known HTTP status/policy headers survive optional capture failure. Eight private
unit fixtures and one ws/wss integration fixture cover fragmentation, buffered
frames, cancellation, deadlines, framing and resource limits. These fixtures are
authored and source-reviewed; compilation and execution remain pending.

Complete precompile and RPC source dispositions bring the source map to
284 reconciled and 73 pending. The published generated matrix now agrees
after refresh 38823630645. Precompile table/notice blockers are resolved
by authenticated adopted output and source changes; full selector/runtime/oracle
execution remains unclaimed. RPC retains explicit typed/budget/replay policies
and the inherited pre-ack subscription-cancellation limitation.

Hosted lock refresh
[38822187126](https://github.com/sam0x17/rtao/actions/runs/38822187126) succeeded
at clean `cc32b5d011514ddf399c8ebe3f21e83643d819cd`. Its artifact was authenticated
and independently fetched twice; both generated locks are adopted. Each adds only
the existing rtao-rpc -> tokio-rustls edge; all package identities and other
bytes are unchanged. The handwritten candidate was excluded. Bounded proof is
`cc32b5d-lock-adoption.json`, without archives or extraction trees. The existing
resolver checks both locks under one 170-second deadline. Source reports are
refreshed for ordinary validation. The main gate now checks documentation
before nextest/doctests, preserving every command and catching link failures earlier.

At this earlier checkpoint, local target output was 547,192 KiB; only
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

At the initial cleanup, the pre-existing `target/debug` was preserved and the
remaining target directory was 547,192 KiB (534 MiB). By the 2026-10-16
checkpoint, `target` is absent. Shared Cargo/toolchain caches, other
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

## Historical verification before Q30

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

## Earlier source-artifact task, 2026-10-15

No temporary root remains. Hosted archives and logs are inspected in memory;
only bounded adoption and failure records are retained in the existing
private recovery directory. No local builds or workspace copies are created.

The Q30 follow-up audit removes the stale macOS-local requirement and adds
hosted documentation coverage to the existing check job: strict rustdoc, a
rendered book, generated API/LLM index and downloadable development artifacts.
The docs command has a 15-minute limit, reuses the hosted target and requires
GitHub's Linux runner environment. Hosted execution was pending at that
checkpoint; run 38832708259 now verifies documentation generation.
