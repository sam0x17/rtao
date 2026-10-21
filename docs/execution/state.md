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

## Q31 dormant webpki-root-certs exception, 2026-10-21

The fixed `webpki-root-certs 1.0.10` lock left the semver-trick release
0.26.11 as the only RUSTSEC-2026-0383 finding. The maintainer approved Q31's
exact dormant exception, alongside Q27's lru and Q28's ruzstd rules. The
supply guard permits only that advisory on the exact registry 0.26.11 entry
with its pinned checksum and patched range >=1.0.10, while every checked
graph excludes it. Both raw audits stay unfiltered and nonzero, and
cargo-deny is unchanged. Locally the supply guard exits 0 with all 60 graph
checks passing; cargo-deny, deps-check, strict xtask Clippy and the guard
regressions pass. Hosted CI remains authoritative.
Evidence: `supply-webpki-root-certs.md`.

## M10 implementation, 2026-10-17

Optional M10 is
implemented at `77f025a` through `6939f1c`, all pushed to `main`; see
[M10 execution record](m10.md). Four new crates: `rtao-compat-synapse`
(M10-01), `rtao-compat-epistula` (M10-02), `rtao-transport-quic` (M10-03)
and `rtao-remote-signer` (M10-04). The facade adds `compat-synapse`,
`compat-epistula`, `transport-quic`, `remote-signer` and
`remote-signer-server`, all in `full`, exposed as
`rtao::compat::{synapse, epistula}`, `rtao::quic` and `rtao::remote_signer`.
`RemoteSigner` is a separate crate rather than an `rtao-signer` feature
because `rtao-btauth` depends on `rtao-signer` (recorded plan deviation).

Local evidence on a development workstation: workspace Clippy clean;
3,384 of 3,385 tests pass (the remaining one is hosted-only by design);
doctests, strict parity and the local supply and features gates pass.
Local interop: Synapse against `bittensor` 10.5.0 39/39 (Rust Dendrite to
Python Axon) and 37/37 (Python Dendrite to Rust Axon), with Python-to-Python
37/37, Rust-to-Rust 39/39 and Python verification of a Rust signature 1/1;
Epistula all four stages against the BitMind and Apex Python references
(gobt by byte-exact vectors only); btlightning Python bindings at `53c090b`
58/58; remote signer Python client to Rust daemon 10/10 and Rust client to
Python peer 2/2.

Hosted evidence at `6939f1ceed5536f8495d6cf090169d0d8afd2ee2`:

- Ordinary [run 39027391242](https://github.com/sam0x17/rtao/actions/runs/39027391242):
  all six required jobs (check, features, supply, msrv, no-host, semver)
  succeed; 3,386 tests pass, none fail, five are skipped; doctests and docs
  pass (exit check 4, C1-C7).
- Dispatch [run 39027402084](https://github.com/sam0x17/rtao/actions/runs/39027402084)
  (`pr.yml` with `m10_interop=true`): job `m10-interop` passes Synapse
  interop against `bittensor==10.5.0` (Rust Dendrite to Python Axon 39
  checks, Python Dendrite to Rust Axon 37, Python-to-Python 37, Rust-to-Rust
  39; exit check 1), all Epistula stages against the BitMind and Apex
  references (gobt by vectors only), QUIC interop against btlightning's
  Python bindings built at `53c090b` with 56/56 including streaming (exit
  check 2) and remote-signer interop. Job `remote-signer-localnet` passes
  `remote_signer_signs_a_localnet_transfer_and_refuses_replay_and_tamper`
  and reports "remote-signer localnet exit check: ok" (exit check 3).

All four M10 exit checks pass and M10-01 to M10-04 are checked in PLAN.md.
The whole plan, including optional M10, is complete: 127/127 active M0-M9
steps plus M10's four.

## M2-26 closure, 2026-10-17

The combined core patch is now pinned at fork
revision `626f2a4f1f67ab731c1fc3884828751496bc125d` (combined six-path diff
SHA-256 `8cf8ff28467ef20a8e47add1a600f60703941dbf3d9af6c2406b9f613e86c489`).
It stores Ed25519 signing state as `Zeroizing<ed25519_zebra::SigningKey>` and
also wipes the hard-derivation preimage and decrypted PKCS8 import copies.
M2-S02 is fixed and the fresh M2-26 cold-reader pass is complete; see
[M2 security](../security/m2.md#final-m2-26-security-pass-2026-10-17).

Local evidence: 72 fork `bittensor-core` library tests, 250
`rtao-core`/`rtao-core-ext`/`rtao-signer`/`rtao-wallet` tests and
`cargo xtask pin --check` against the real fork revision pass. Compatibility
reports were regenerated for the new core revision. Hosted CI for the closing
commit: [run 39016798208](https://github.com/sam0x17/rtao/actions/runs/39016798208) at `2d257b5` passes all six required jobs with **3,205 tests passed, zero failed and four skipped**, plus doctests and documentation. (The first attempt at `a855600` hit the then 45-minute job limit; a re-run exposed one timing-sensitive admission test, whose overall deadline `2d257b5` widened.)

The active M0-M9 register is **127/127 complete**. Earlier sections describe
their own snapshots, including the 126/127 state below.

## Final client-source checkpoint, 2026-10-17

Hosted [38999377082](https://github.com/sam0x17/rtao/actions/runs/38999377082)
passes at exact source head
`6aae5a526ab4adf909f1a0f1ab8b3c789a229404`: **3,204 tests passed,
zero failed and four skipped**, doctests passed, documentation uploaded, and
all six required `check`, `features`, `supply`, `msrv`, `no-host` and `semver`
jobs succeeded. This is the final implemented client source under Q30; no
local build, node, container, testnet, device or release campaign was used.

Hosted report refresh
[38998872513](https://github.com/sam0x17/rtao/actions/runs/38998872513)
authenticated the four generated compatibility reports at `2cd5629`, and the
adopted report commit is an ancestor of the final source head. The source map
is **357/357 reconciled with zero pending blobs**. All 83 intents and 102 reads
are implemented. The 18 `e2e_map.toml` Q30-deferred rows describe optional live/localnet
evidence; they are explicit nonrequirements and are not represented as executed tests.

The active M0-M9 step register is now **126/127 complete**. Every implemented
client, documentation and milestone-specific security step is checked. The
sole open step is M2-26 because M2-S02 remains explicitly unaccepted: the
pinned upstream Ed25519 signing object does not guarantee erasure on final
drop. Consequently M2 and milestones that depend on its formal exit remain
unaccepted even though their individual source steps and applicable hosted
checks are complete. No release, device, platform, fresh Python-oracle,
interoperability or live-chain acceptance is claimed.

The filing inventory was refreshed read-only on 2026-10-17: the existing eight
PRs and five issues remain open at the reviewed heads, with no labels or review
requests and no new filing. The worktree remains bounded; the retired
local build root was not recreated.

## Earlier client checkpoint, 2026-10-16

Hosted [38984859167](https://github.com/sam0x17/rtao/actions/runs/38984859167)
passes at exact `b0aad3ba5c0c88180a1708a60628cdd731c38623`: **3,196 tests
passed, zero failed, four skipped**, nine doctests across 19 suites, successful
documentation upload and all six required jobs. All eight failures from
`4dc5c7b` have exact qualified PASS records, including all seven stack-overflow
cases on normal stacks and the saved registration cost disclosure. This
checkpoint also executes the typed password-help and unknown-fate output fixes.

The bounded checkpoint record (not retained in the repository) is 469,793 bytes, SHA256
`80b4f5f3db680a9b298bff207638107e07e6acfeb6c7bb768cd14a4326b47372`.
It authenticates all 3,196 unique qualified PASS records and 167 committed
source identities. The 753,263-byte main log has SHA256
`998aa1af35b6fbbb46cb4e94a9023f364c0875b49346b7f5a5d8d8e88b21a87b`;
its raw bytes were inspected in memory and not retained.

Newer reviewed client source remains outside this result. It adds
raised diagnostic metadata, endpoint-secret protection in errors and shareable
commands, actual carrier transport narration, retained included receipts after
registration completion errors, opt-in terminal cancellation, and e2e-backed
Docify examples. The subsequent invocation coordinator connects terminal and
async cancellation, registers SIGINT only in main, closes acquired clients and
retains exact started work through receipt hooks and signer release. Original
results survive; a separate latch supplies final status 130 and suppresses
successful replay. Independent source review finds no concrete defect; real
signal/terminal behavior and hosted execution remain unverified. Five new
coordinator tests bring the expected inventory to **3,203 tests and four skips**.
The existing native-password test now joins public wallet resolution to the
actual adapter with an injected process runner, covering lazy source precedence,
exact argv, explicit dialog fallback and redacted errors. No real native process
runs. M4-S5 remains open pending execution of its three source repairs.

Six missing M5 root-subnet inputs are now present in `samples_extra.json`.
Three existing tests build and plan both original and scenario-rewritten public
identities, check exact 1,000,000,001 rao denomination and compare independent
V478 call bytes, including the move-swap one-rao dust. No Python expected output
is authored. The existing xtask intent dependency enables its staking feature
to consume these inputs; package identities and lock bytes do not change.
Independent source review, installed formatting and whitespace checks pass;
hosted execution remains pending. Named-test inventory stays 3,203/four skips.

Lock-only hosted [38987915743](https://github.com/sam0x17/rtao/actions/runs/38987915743)
succeeds at exact `938cc707c5ecb95aa9b2c017f89b94c3f4988b68`. Independent
inspection and a separate refetch authenticate the sole 11-byte addition of the
already pinned Docify 0.2.9 to testkit's dependency list. No package identity or
other lock bytes change; the fuzz lock stays identical. Only that reviewed root
lock edge is adopted. The inspection/adoption records are
not retained in the repository. This preparation runs no full tests.

Dependency and divergence reports still need hosted refresh: source map stays
322/357 reconciled with 35 pending; the divergence register is 76 while the
last generated matrix records 73. Source preparation uses a skip-CI push so
ordinary locked gates do not start with known stale locks/reports. No local
Cargo, codegen, build or test runs. Local `target/` remains absent.

The active M0-M9 checklist has 78 of 127 unique step IDs checked (61.4%).
Optional M10 and removed Q30 requirements are excluded. This measures formal
step acceptance without weighting effort. Source reconciliation is 322 of 357
rows (90.2%), including non-port dispositions; it measures a separate review
inventory. M0/M1 are accepted; whole M2/M3 and M4-M9 remain open.

The current green run accepts none of the newer behavior, held map proposals,
M4-S5, M2-S02 or later milestones. M0/M1 and previously accepted individual
steps retain their recorded evidence. The pending M2-S02 dependency/risk
decision is unchanged; no repeat question or inference of approval is made.

### Earlier narration/display checkpoints (historical)

The following records describe prior states; live/uncommitted wording belongs
to those checkpoints and is superseded by the current result above.

The latest full run, [38979569522](https://github.com/sam0x17/rtao/actions/runs/38979569522),
at `3985fdc926ce3800dd70c27ad79a62ae480d89c6` finishes with **3,195 passed,
one failed and four skipped**. All five supporting jobs pass, including
supply and features. Main compilation and Clippy pass; doctests and the final
documentation stage are not reached. The last fully green checkpoint remains
`6b7ba38` below.

The remaining failure is the saved Ledger signing-notice fixture. It gives a
single member an inline backend override while selecting Vault globally.
Both the pinned Python and native planner apply per-member backend overrides
through repeated-member approval planning; the single external path stops
before acquisition. The candidate supplies two explicit members (Ledger then
extension) and asserts the exact coldkey-unavailable signing refusal. Notice,
acquisition/release, zero-unlock and zero-submission assertions remain. No
production selection rule changes. This correction awaits hosted execution.

The preceding [38976362971](https://github.com/sam0x17/rtao/actions/runs/38976362971)
at `b38db6c` had three failures. The review JSON DEL encoding and unknown-fate
inspection-help tests now pass; the saved member funding correction remains.
The inventory still expects **3,196 tests and four skips**, with no new test
functions in these fixes.

The pushed source batch preserves typed gateway retry guidance through the
remaining confirmed CLI read, PoW, pruning and saved-multisig boundaries.
Raw-call and pruning followup failures also retain known submission identity
and inspection guidance. Pruning retains its durable journal and existing
retry refusal; secondary close errors no longer replace the primary help.
Six existing test functions cover real public read handlers, saved account
derivation, raw approval followup and pre/post-receipt pruning failures.
No new test functions are added. The completed run compiles this batch and
executes its fixtures; the separate saved-notice failure still prevents a full
pass. Pushed name handling registers concrete-wallet public identities and
already-loaded contacts/proxies, then applies account/subnet names at human
prose boundaries. JSON, replay commands and transaction inputs retain their
values. Independent review found the remaining display gaps described below;
the full context source row remains unaccepted.

The completed run above excludes local commit `dc6a276`, which adds
pre-sign root/owner notices, successful direct verification hints and bounded
registration progress forwarding. A failed progress sink is retained until the
submission finishes, the raw receipt hook runs and the signer is released.
The fixture keeps an actual executor call pending until the failing sink is
observed, then checks completion and receipt recovery. Quiet/JSON registration
cases and the original complete 83-intent preview traversal remain covered.

Source review also found that saved-multisig subnet registration skipped its
live cost disclosure by checking the outer approval name. The candidate checks
the semantic registration and exercises the real saved review before source
acquisition. Independent production and fixture source reviews, installed
formatting and whitespace checks pass. The failing-sink fixture exercises
completion and receipt capture; external release under that failure is
source-reviewed only. These changes are outside the completed CI run and have
no hosted execution result yet. The full context row still requires final
disposition of the remaining password/error presentation contracts and
carrier-failure narration.

The subsequent display review found three gaps in `3985fdc`: transaction
arguments passed through name substitution twice; receipt prose bypassed it;
and saved approval members were named only after confirmation. The next
candidate formats review values before one rendering pass, gives receipt
headers/rows their Python prose treatment, and seeds selected public wallet
members before contact/proxy labels. Receipt keys merge before display-label
normalization, preserving distinct `some_key` and `some key` fields. Existing
fixtures cover reverse-reference labels, hostile names, canonical JSON,
selected-name precedence and zero private acquisition. The fixture function
inventory is unchanged. Source checks pass; hosted execution and final review
remain pending. This candidate is also outside the completed run above.

Ordinary [38982324536](https://github.com/sam0x17/rtao/actions/runs/38982324536)
is validating the pushed narration/display corrections at
`4dc5c7b370469f101f3dce2258e0694db6fece7c`. It remains live; no full pass is
established. The separate uncommitted password candidate retains actual
password-source remediation through an owned signer diagnostic, SDK signing
errors and CLI conversion. Only prompted coldkeys retry twice. Quiet/JSON
sessions suppress intermediate errors; the final nonempty error reaches the
ordinary renderer once. Existing wallet fixtures reuse one encrypted keyfile,
count prompt/native attempts and isolate real environment/file CLI cases in
child processes. No child, cryptography or test ran locally.

The native unknown-fate receipt path also needs the retained recovery hash at
the CLI boundary. Some SDK failure messages already include it; others omit
it, and D-023 deliberately omits the native field from canonical JSON. The
candidate preserves stdout and emits separate stderr inspection help, including
quiet mode. Human classified errors keep their code/notes/docs with recovery
help; receipt-output failures retain that hash in the returned error. Existing
fixtures cover classified/unclassified output plus actual SDK watched
submissions, raw receipt hooks, one submission/unlock and a failed output sink.
RPC and primitive hash wrappers remain distinct; shared formatting accepts
their exact 32 bytes. Source review and installed formatting/whitespace checks
pass; hosted execution of this candidate remains pending. No new test functions,
source rows or milestones are accepted. Connection context, raised classified
diagnostics, safe carrier-failure narration and signal disposition remain open.

Hosted report run [38975470963](https://github.com/sam0x17/rtao/actions/runs/38975470963)
passes at `55339cce05d0763950144b8e838c364ca6c0628c`, including source parity.
Artifact 12078644250 is 59,739 bytes, SHA256
`44ed0f18bccda7a3066da14e2854e9199aadbb257646ce336f08e99a37e525b4`.
Inspection and a separate authenticated refetch adopt exactly four reports.
The matrix adds D-073 and four D-068 witnesses; all 23 dependency inputs,
role inventories, host flags, source/e2e counts and runtime/platform/oracle
claims remain unchanged. Inspection SHA256 is
`8bd944e0b1469c0d848895d8206848e6a8a6b7a3cbab662d24188e44ed2da6a0`;
adoption SHA256 is
`b58b2200d1879ceff8ee7dfda89e9c6cd26d1e5707a75fcfc7a7498dbdbbb55d`.
No raw archive is retained. Complete client validation remains pending after
the failed ordinary run above.

Source reconciliation remains 322/357, with 35 pending. The source divergence
register and adopted matrix now both contain 73 entries. M0/M1,
twenty M2 steps and eight M3 steps are accepted; whole M2/M3 and all later
milestones remain open. M2-S02 remains unresolved under its existing decision
request. The local target directory is absent, and validation remains hosted.

### Prior checkpoint details

The latest successful ordinary checkpoint is
[38949537519](https://github.com/sam0x17/rtao/actions/runs/38949537519)
at `6b7ba38787f288c3f080bf33aa46606f100123e8`. All six required jobs
passed: 3,145 tests, four skips and nine doctests across 19 workspace suites,
plus strict Clippy, documentation, source/generated guards, feature profiles,
MSRV, no-host, semver and supply checks. Feature validation also passed the
native template gate, both TLS crypto providers, unpatched-core compilation
and generic keyfiles without encryption support. No local build ran.

The bounded primary proof (not retained in the repository) has SHA256
`a3ba7b4bfa03cc74c3fea2aebc328471ada0e77ab29498beb3798612d25b6728`.
It retains 113 qualified PASS records and 101 source identities. Same-log
supplements retain nine payment and four metagraph witnesses.
The main log was 739,615 bytes, SHA256
`538c058f93bbbbc3c0d01cfb0561fc876cc1a19829254004e45bc3f3a5048de2`;
no raw log or archive is retained.

Source and the generated matrix record 322 of 357 files reconciled, with
35 pending.
The three new source dispositions cover CLI startup, metagraph output and
the payment wrapper's client-visible contract. They do not establish runtime
admission, debit or refund behavior. The exact adoption record (not retained in the repository) has
SHA256
`a3ca105e82aca429dfbdd3e0c602057af007b3dded5ef32e4bb96cd36fa669bf`.
M0/M1 and twenty individual M2 implementation/guide steps remain accepted.
Eight individual M3 implementation/example/guide steps are now checked:
M3-01 through 06, M3-09 and M3-10. Their 61 owning inputs and template tree
are unchanged from the tested checkpoint. The earlier 42 named miner-related
witnesses retain their actual 3,132-test run provenance; the current green
3,145-test result and successful native template gate supply the later C1
evidence. M2-26, M2-S02, M3-11 and whole M2/M3 acceptance remain open.
No later milestone or strict M8 acceptance follows.

The next reviewed source batch changes 31 files and adds 27 test functions,
expecting 3,172 tests with four skips in its future ordinary checkpoint.
It adds multisig aliases and direct followup, owner labels and human output,
authenticated pruning-test inputs, staking boundary fixtures, pre-signing
reserve refusal and shielded inner-finality regressions. All exact reviewed
afterimages and formatting checks match; no local compilation or tests ran.
The pruning inputs and source reports have now been regenerated and adopted
from the successful hosted refresh below. The newer 27 tests still have no
execution credit from the green checkpoint.

The first refresh, run 38954992909 at `11bce1c`, generated the candidate
outputs but failed source parity because the newly mapped CLI entry file
lacked its separate `main.py` attribution header. No artifacts were adopted.
A header-only correction preserves all executable bytes and the existing
`prompt.py` attribution. The corrected refresh,
[38956406939](https://github.com/sam0x17/rtao/actions/runs/38956406939),
passes at exact `1d51d08f979c2a9f6b3acc7f137e4cb601c60397`. Artifact inspection
and a separate refetch adopt exactly six files: the pruning input extraction,
catalog `py.rs` and four reports. The other 17 generated outputs, all 23
dependency inputs and the package/feature graphs remain unchanged. The new
source entry adds four invalid-input cases, five admin-window cases and one
switch-input vector; every prior named-table byte and catalog record is
preserved. Inspection SHA256 is
`768707e72981512b5014180646b2523d2a84241ef0cc07436413b7300b590582`;
adoption SHA256 is
`645c5d25a167529e95a4efee38a70bc3df08aeb524723cb0970a4a91319b598c`.
This report-only success does not execute the 27 newer test functions.

Ordinary run 38958405135 at `b5124b5` stops in strict Clippy before Nextest:
the new direct-multisig fixture holds a mutex guard through asynchronous client
cleanup. MSRV, no-host and semver pass. The correction limits the guard to its
two existing assertions without changing those assertions, production code
or test counts. Full ordinary validation of the newer batch remains pending.

The correction is pushed at `2a7f004149ba89ca5cc03c522a90189b99f6b086`.
[38959809427](https://github.com/sam0x17/rtao/actions/runs/38959809427)
passes Clippy, features, supply, no-host, MSRV and semver, but Nextest reports
3,169 passed, three failed and four skipped. The failures are the new stake-list
and wallet-overview output assertions: table dividers were parsed as cells,
the replacement renderer omitted normal connection-time token-symbol setup,
and expected dust floats used Rust exponent spellings rather than Python's.
Their local corrections preserve the complete record and visible-row checks.
Doctests and documentation upload were not reached; this is not an accepted
checkpoint. The bounded failure record is not retained in the repository.
The successful feature artifact separately proves the generated owner-tool
built and its three tests passed; it does not establish ordinary C1 success.

Five further reviewed regressions are applied locally, with direct rustfmt
and whitespace checks passing: canonical shield hash/receipt recovery,
production hotkey SetWeights submission, proxied AddStake(All), snapshot-bound
typed reads, and a regular commitment's raw-policy refusal, signed submission
and typed reads. They use finite fake transports and supplied observations.
The existing EX14 batch/root-claim hard-stop fixture was already committed in
`11bce1c`; it is not a further test addition. The five new functions await
their own hosted run.
Two additional regressions cover actual production Transfer proxy success and
inner failure, and every visible installed command/group's `--help`. The latter
exposed doctor/completions help-dispatch defects; their offline routes now keep
Clap's successful help status, with malformed-input exit checks retained.
Wallet listing now derives a display address from resolved multisig members,
retains unresolved references and normalizes string thresholds to native u16
numbers. The existing actual-list fixture checks both human/JSON output and
continued strict saved-account refusal. Three direct helper homes receive
canonical attribution. These seven new functions bring the expected future
inventory to 3,179 tests with four skips. Source review, direct formatting and
whitespace checks pass; all changes still require a full hosted checkpoint.
No new source mapping, runtime effect or milestone acceptance is claimed.

Ordinary run [38963964395](https://github.com/sam0x17/rtao/actions/runs/38963964395)
at `9ae3ab985f215223cd4d32d4c63c58c8b50e9da6` stops during Clippy compilation
of the new regular-commitment fixture. `CommitmentRegistration` already derives
the metadata decoder, but the client's `ReadValue` list omitted it; the fixture
also compared `PolicyError` with its violation array. Add the missing typed
leaf and inspect `error.violations`, preserving all refusal and storage assertions.
Independent source/API review and formatting checks pass. All five supporting
jobs passed, including the full feature job. Nextest, doctests and documentation
were not reached. The bounded failure record is
not retained; its 83,940-byte log has SHA256
`a62787764f740ecd2a291d9761de2a1fb136500e0dbfe15e85d7c945847dcca4`.

Two additional reviewed tests are applied for the next hosted batch: actual
queued RegisterSubnet execution with one waiting block and complete result/
progress/receipt assertions, and production root/proxy shield wrapping with
exact signed inner bytes and separate inner/carrier purposes. The registration
fixture supplies subsequent heads only; it does not prove suppression of an
initial duplicate head notification. The shield fixture asserts pool acceptance,
without claiming inner dispatch. Both fixtures remain unexecuted.

Two further source-reviewed functions run all 83 authenticated intent samples
through the actual CLI parser and JSON dry-run handler, and all 102 authenticated
read samples through the actual query parser, dispatcher and JSON renderer.
The existing SDK tables share the extracted pure FakeChain seeds; catalog
loading, exact-set assertions and deterministic public PoW seal binding remain
in the consumers. Public-only keys refuse and count private acquisition; hooks,
author requests and submissions must remain unused. These are in-process
handler tests, without a new installed-process or wallet-factory claim.
The next inventory expects 3,183 tests and four skips. Independent source review,
direct rustfmt and whitespace checks pass; hosted execution remains pending.

Ordinary run [38965848977](https://github.com/sam0x17/rtao/actions/runs/38965848977)
at `0e5c437a4fa6831720dd7f5710fe6961dd080602` passes supply, MSRV,
no-host and semver, but stops before Nextest on two test-only compiler/lint
errors. The extracted subnet helper returns CoreError, so its iterator must
collect that error type before converting at the anyhow boundary. A local
alias names the shield fixture's captured signing records to satisfy strict
Clippy. These corrections preserve every assertion and test count. The existing
CLI advertisement fixture also now requires exact visible transaction-leaf
set equality, closing the remaining no-extra-command assertion gap. Formatting
and source checks pass; the expected inventory remains 3,183/four skips and
still awaits successful hosted execution. Neither the bounded failure
record nor the raw log is retained.

The small compiler/leaf correction is pushed at
`6861dc837aac8dc8fbf76ea942ce3047869bdc76`; ordinary run
[38966898387](https://github.com/sam0x17/rtao/actions/runs/38966898387)
passes all five supporting jobs and strict Clippy, then reports 3,180 passed,
three failed and four skipped. Doctests and documentation upload were not
reached. Its result does not cover the following later source batch.

The failures identify three fixture corrections. Clap's generated `help`
subcommand is invoked without an appended help flag, while every ordinary
visible command still receives `--help`. The 83-operation dry-run traversal
now supplies public-only wallet records for required hotkey name resolution;
its injected private-acquisition, hook and author-RPC sentinels stay intact.
The proxy success event uses an empty array for the metadata's unit tuple
instead of JSON null. All existing traversal and receipt assertions remain.
The bounded failure details are not retained;
the 783,437-byte main log has SHA256
`840ff1c724401ab1f9133b6412a73ae1aa2f67ebae14cf730d50e431418d6a1c`.
No raw log is retained. These corrections require another full hosted run.

The next source batch shares the naturally ordered, nonempty-contact public
name lookup between stake views and pickers. It fixes duplicate local aliases
and empty contacts hiding chain names. Wallet overview now counts hidden
subnets separately from hidden positions, includes the hidden value/threshold,
and emits its ordinary notice on stderr with quiet suppression. Canonical
helpers attribution is separated from wallet-command attribution. A real
stake-list fixture covers local alias precedence and chain fallback; the
existing overview fixture now covers a whole hidden subnet, one hidden position,
quiet suppression and explicit dust display.

The same batch adds the reviewed four-family embedded root-claim public
plan/shield refusal fixture and strengthens actual proxy fee-payer planning
and exact per-family warnings. Its independent source review record is
not retained in the repository. These two additional functions
bring the future inventory to 3,185/four skips. All source changes remain
unexecuted; formatting and whitespace checks pass. No source row or milestone
is promoted by these changes.

Those changes and the three fixture corrections are pushed at `5a12850`.
Ordinary run [38970331580](https://github.com/sam0x17/rtao/actions/runs/38970331580)
finishes with 3,182 passed, three failed and four skipped. Its supply job failed
after `inout 0.2.3` was yanked on 2026-10-16 at 21:27 UTC. The upstream
`0.2.4` release restores a missing output-buffer length check, keeps the same
dependencies and requires Rust 1.85, below this workspace's MSRV.
Hosted lock run [38971146218](https://github.com/sam0x17/rtao/actions/runs/38971146218)
passes at `a42492e`. Artifact 12077076702 is 81,871 bytes, SHA256
`1daf8ece5ca0faf292c5cf2e7438ffdcfd09b9de31dccb9ba7a84ab57bdcf901`.
Inspection and a separate refetch adopt only that package's version/checksum
and sole versioned dependency reference. Reversing those three lines reproduces
the earlier lock byte for byte; the fuzz lock is unchanged. No new advisory
exception or manual lockfile edit was used. The inspection proof (not retained in the repository) has SHA256
`3ea612f0d6e15438a87d30a3e9a291df35395a701944cb23db290c5571afe071`.
The raw failure identity and registry/source comparison are not retained in the repository. The lock-only job establishes no ordinary test success.

The following client repairs are separate from that active run. Weight
activity alarms now use the chain's minimum one-block cutoff, including
root-permitted zero-tempo subnets. Live external signers repeat the carrier-fee
check after public account selection: default shielding downgrades on a known
shortfall, forced/required shielding refuses before private acquisition, and
previews remain warning-only. Root and generated transaction amount options
share the existing bounded Decimal-compatible CLI normalizer; native Money
and Amount constructors retain their strict checked-u64 contract. D-042 and
the CLI guide describe the shared input budgets. Direct source attribution
is corrected for serving, rate limits and the intent-table fixture homes.
Two new fixture functions bring the next expected inventory to 3,187/four
skips; the existing money fixture is strengthened without a new function.
Formatting and whitespace checks pass. Hosted report run
[38971449966](https://github.com/sam0x17/rtao/actions/runs/38971449966) passes
at `af4cc3a`, including source parity. Artifact 12077522768 is 59,608 bytes,
SHA256 `00da75b6d0b6696bf3d551a89c422f55137137cfff8eab370fa4aeddabee2503`.
Inspection and a separate authenticated refetch adopt exactly four reports.
Only the lock dependency fingerprint and measurement context change in the
compatibility reports; the matrix adds D-042's existing generated-money test.
The other 22 dependency inputs, role package inventories, feature graphs,
host flags, source/e2e counts and runtime/platform/oracle claims are unchanged.
The inspection SHA256 is
`9e550b806c6b69079f2abdfd823fb2457a2567ea7c1c23ba7e624cdf86536d6b`;
adoption SHA256 is
`be4eecc9de940dd27501c15c9c1b5a4ee8bc2a12fd2ee496190cbdbdd5051beb`.
The main job also fails: the all-83 CLI harness passes a nonexistent root
`--hotkey` flag, the help traversal appends `--help` inside generated help
paths, and the acknowledgment-timeout fixture observes no fake submission
before its 50 ms admission deadline. The last failure does not prove a false
unknown-fate result: the submission-count assertion runs first. MSRV, no-host,
semver and features pass; doctests are not reached. Bounded evidence is
not retained; the 838,125-byte log has SHA256
`98b20e6f81bddb75395c5fcdc70298e17f952f3b149edb31b8d788eb4e20af69`.
No raw log is retained. Fix these failures before the next ordinary run,
superseding the planned standalone 3,187-test checkpoint. Report generation
accepts no source row or milestone.

The following CLI source batch is separate from that 3,187-test checkpoint.
Successful root unstakes can report remaining yield or offer a separately
confirmed claim. The probe and copied guidance retain the effective owner and
proxy route (D-073); external sources are released before reacquisition, and
the original receipt and recovery hooks stay with the unstake. Human review
cards now retain command context, always show Fees, identify signing/routing
in Signer and show semantic fields in the final Transaction stage, including
saved multisig approvals. Full hostile nested arguments remain covered beside
a short escaping control. Shielded external signing prints the backend's
two-step notice using the shared backend normalizer.

Typed gateway refusals preserve retry help at the connection, transaction and
generated-query boundaries; the public query fixture covers both historical
lookup and dispatched storage errors. Other command-owned string conversions
remain a CX06 source obligation. D-068 explicitly records native extension
terminal confirmation; the existing acquisition-order fixture now covers both
Ledger and extension. No authorization bypass or device execution is added.
Nine new fixture functions expect a later 3,196-test/four-skip checkpoint.
Independent source review, combined-source formatting and whitespace checks
pass; compilation, fixture execution and updated divergence reports remain
pending. The full context row, source acceptance and milestones stay open.

Additional source reconciliation, security dispositions
and exact milestone evidence remain required through M9. The local target
folder remains absent; temporary source/review work is not retained in the repository. No node, testnet, soak or hardware work is scheduled.

## Earlier 2026-10-16 client checkpoints

The following entries are historical results and execution-pending statements
at their recorded source commits. The current checkpoint above supersedes
oldest-result wording and obsolete next-test counts below.

The latest successful ordinary checkpoint is
[38897437100](https://github.com/sam0x17/rtao/actions/runs/38897437100)
at `a6aa2bc5ebde12f4183ec7826e7c1c9fdc90cab3`. All six jobs passed. Main
passed 3,080 tests, zero failed, four skipped and nine doctests across all 19
workspace suites. Strict Clippy, documentation, source/pin/generated guards,
feature profiles, MSRV, no-host, semver and supply checks passed. The bounded
hosted-results record (not retained) authenticates 34 selected PASS records;
its SHA256 is `fadc0d5af652d64707c474093ac6cf8c0bf2b60b647f51aab1f00360000c1eb4`.
The 735,349-byte main log has SHA256
`754a2eb8c31416d59ea15deac55fe9011ed9b9f1407e3f0c00615b5c4eb60b8b`.
No raw log is retained. This executes the wrapped registration-completion
fixture, both original shared documentation examples and the positive unsigned
seed reconstruction/signature-attachment witness. It validates no standalone
fuzz build or campaign.

Twenty individual M2 implementation/guide steps are checked against this
checkpoint: M2-01 through 04, 06 through 18, 21, 22 and 25. M2-26 and the M2
milestone remain unaccepted while M2-S02 is open. Optional hosted checks and
the waived soak retain their existing scope.

The newer source batch completes missing book topics and connects LLM
navigation to the book summary. It adds source-derived alias/divergence indexes,
an agent card and transaction execution guidance; all 19 library roots include
their crate READMEs. Feature labels cover root exports and selected child
implementations. Hosted run 38901469526 regenerated and verified the three
intent outputs; independently refetched adoption retains exactly 187 matching
feature badges, with all other generated code bytes unchanged. Seven new test functions cover three shared offline examples,
signature-form attachment equality and three generator regressions. The
existing metadata-mode fixture now attaches imported enabled payloads, compares
matching disabled payloads and rejects cross-mode signature reuse. Independent
source reviews and direct formatting pass; this batch is outside the current
green checkpoint and expects 3,087 tests in the next ordinary hosted run.

Five missing multisig pytest origins now record native input/compositional
limits without changing executable bodies. The reviewed source mapping is
adopted using the exact wrapped-registration PASS above. Source inputs reach
312 reconciled and 45 pending, now matching the authenticated generated matrix. The complete prepare/attach source row remains pending until
its new attachment test and stronger metadata fixture pass. No later milestone
exit or strict M8 acceptance follows.

The source/report refresh
[38901469526](https://github.com/sam0x17/rtao/actions/runs/38901469526)
at `4df042116a16b731c2d4e8114f71775fd96bbea1` passed. Inspection and a
separate refetch adopted exactly three generated intent files and four reports.
All 16 other generated members are identical. All 23 dependency inputs match;
only the root lint and 19 crate documentation metadata blocks changed their
fingerprints. Role package counts and host flags remain unchanged. Runtime,
oracle and hardware claims are unchanged. The inspection proof SHA256 is
`887b23fcffe7c432a3242a1146e2b2d6848ddb7aeb09be6b9b7b79c34d327b2d`;
adoption proof SHA256 is
`0114c55ed40b31620fd241eb8a501bead49d9f06fe6b5cdc017f6e5b0dd661d7`.
This source-only result does not execute the seven newer tests.

The subsequent ordinary run
[38902988661](https://github.com/sam0x17/rtao/actions/runs/38902988661)
at `e3acd2fdc10f4891b2bc9bfd4b2cb1b2f6c0bb14` failed in
`core-surface --check` before Nextest. Its blanket refusal of conditional
module/export attributes also rejected the new documentation feature labels.
The bounded failure record (not retained) records the 83,215-byte job-log
identity, SHA256
`dd527d8b08962fd9bbdeb611345ed5848f80be5950aae277d34e336285f05a4e`,
and actual failure excerpt; the raw log is not retained. This run supplies no
new test result or source-row acceptance. The green checkpoint above remains
the latest verified ordinary success.

All five other ordinary jobs passed. The follow-up source batch admits only
the documentation feature-label attribute in the surface generator and adds
two guard regressions, retaining refusal of conditional exports and paths.
It also adds five real query-request/dispatch fixtures, eight Null-weight
fixtures, a public blocking PoW-to-plan fixture and an unchanged-sample schema/
constructor fixture. The query prompt fix supplies the missing input-format
hints. These 17 new functions expect 3,104 passed tests and four skipped in
the next ordinary run; they are not executed evidence yet. Query, Null, PoW,
EVM and prepare/attach source-row proposals remain unapplied.

Run [38904971807](https://github.com/sam0x17/rtao/actions/runs/38904971807)
at `447c41eddc6d0f502ba4a1bc550aaab936665c38` completed with five
ordinary jobs passing and the main check failing: 3,103 tests passed, one failed
and four skipped. All 17 new functions and all 61 selected witnesses passed.
The failure is in
`oracle::readiness::tests::q30_strict_source_parity_reports_optional_oracle_as_unverified`:
its invalid-plan mutation still targets an unchecked M2-14 row, while the plan
now records that source-machinery step as checked. The replacement changes
nothing, leaving a valid Q30 declaration where the test expects refusal.
The production readiness guard remains unchanged while this fixture is repaired.
Doctests and the later documentation check were not reached.

The bounded partial hosted-results record (not retained) records the exact
failed-run identity and 61 unique PASS records (SHA256
`db2fa8d8ad99c633eaf927d1e8fe93a39b79aed1d170c471a9d9241a2c026a51`).
The 723,148-byte main log has SHA256
`1cddfb4bc7773909f11d89dbb9d6f811063cee805204ed98c70fade2a7474d96`;
a separate failure record held its failure excerpt. Neither that record nor
the raw log is retained. Passing witnesses in this failed run do not advance the successful
checkpoint, source rows or milestone acceptance.

The correction normalizes that fixture's M2-14 checkbox and checks both valid
states in strict and non-strict modes. Each invalid case must change the plan
and fail under both modes; production readiness/history code is byte-identical.
The same source batch puts ordered raw-call member names and backends in the
review card before confirmation, reports automatic-round provenance, and adds
two regression functions. They cover human/quiet/JSON refusal before private
source acquisition and the absent shield-key fallback versus forced/required
refusal. Independent source reviews and formatting pass. The next ordinary
run expects 3,106 tests, four skipped and 65 selected PASS records; execution
and source-row acceptance remain pending.

The separate EVM source batch adds canonical Solidity ABI import and equality
checks for every watched SDK JSON file, including unregistered files. Nine
new tests exercise the real imported inputs and missing/drift/identity refusals.
A bounded `refresh_canonical_abis` dispatch reuses `compat-refresh` on free
`ubuntu-latest`, imports only 21 independently named files at the existing
parity commit, verifies all 21 focused importer/extractor tests and retains
23 candidate source files plus bounded proof. Its verifier explicitly requires
all nine new test names and preserves every prior artifact, pin and NOTICE
entry. The canonical files have not been imported or adopted yet. Source review,
formatting and Node syntax checks pass; the source commit skips ordinary CI
until the required candidate inputs are reviewed and adopted. Ordinary hosted
validation follows adoption. No EVM row or milestone acceptance follows now.

Canonical candidate run
[38930971428](https://github.com/sam0x17/rtao/actions/runs/38930971428)
passed at `9e881cd473ae3c19bc24621e3dbe9f85a05f8a02`. The normal
import/extraction checks and all 21 focused tests passed. The 63,793-byte
artifact `12059321729` has SHA256
`7db7d069d9165c6c4e7b4c5871836e121dd2a1711b4920bcc24e581811b0ca88`.
In-memory inspection verified all 21 canonical blobs against the pinned SDK
bytes/hashes, reproduced every previous manifest/NOTICE byte after removing
the 21 additions, and authenticated every named test against the job log.
A separate refetch adopted exactly 21 ABI files, `upstream.toml` and `NOTICE`.
The 181,888 ABI bytes are source inputs; no archive, extraction or raw log is
retained. Inspection proof SHA256 is
`c2c4f3b8ea0c21d71f1016802858d4dd68b6e5872cd2dd3ae0e4207d50bc201e`;
adoption proof SHA256 is
`5376363084a936e6d1dd1c148cb210d2007c66a723429225c489257279aeb6ab`.
The full ordinary suite and report refresh remain required. Current EVM source
mapping and milestone acceptance remain pending.

The accepted M0/M1 foundation checkpoint remains
[38873429391](https://github.com/sam0x17/rtao/actions/runs/38873429391)
at `965669d709bd8b8435f76ec7db5def9d471fe0d9`. All six hosted jobs passed.
The main gate passed all 3,051 executed tests, zero failed, four skipped,
all 19 workspace doctest suites (nine doctests), strict Clippy, documentation
and every blocking check. Its 717,929-byte log has SHA256
`9c5a4c537e36e27fdac9e215360db2d591d546313116ad195dc0943cadea34bc`.
The bounded hosted-results proof (not retained) authenticates 175 unique
named PASS records and 129 final source inputs. Overlapping groups retain
123 M0, 45 M1 and 11 new-fixture memberships. Its SHA256 is
`bef2ded037673d97f8016981ab402868776824558289fd895f96aeb53c3a33a0`.
Separate TLS evidence verifies the real RPC exchange with both providers
compiled for the shipped Rustls version and no process default installed.
No raw log or archive is retained.

M0 and M1 satisfy their Q30 client-development exits at this checkpoint.
M1-01 through M1-21 are complete; M1-22/24 remain optional and M1-23 deferred.
This establishes no fresh Python/oracle, live-node, device, native-platform or
release result. S9-11 and the five root-helper follow-up fixtures now pass;
full root-command and pytest reconciliation remain separate.

The following preceding security checkpoint retains its own exact source and
review evidence. Later client work is recorded separately below.

Ordinary hosted
[38832708259](https://github.com/sam0x17/rtao/actions/runs/38832708259)
passed at `1edf4d7e859cadb362644196cde5bf278c527c73`. All six jobs
passed: check, no-host, features, MSRV, semver and supply. The main gate passed
strict Clippy, source/pin/extraction checks, documentation generation, all
3,013 executed tests (four skipped) and workspace doctests. The documentation
artifact uploaded successfully. This supersedes the earlier execution-pending
claims for the integrated EVM, pruning, compiler, transport and extractor
fixtures below. It supplies no Python/oracle, live-runtime, device, sanitizer
or native-platform result.

The authenticated main log is 927,796 bytes with SHA256
`88bc33d05df3a5c14789d615940791ce65c1e6486726926f1b25de2dc15933d3`.
Bounded evidence is not retained in the repository;
no raw log, archive or extraction directory is retained. A separate exact-name
extraction confirms all 70 selected
S9 regressions and all 12 new M2-S03 parser/public-view/differential regressions
passed on this commit. Independent source review and exact executed results
close the original S9-01 through S9-10 defects and M2-S01/S03/S04/S05, retaining
the limits in [M9](../security/m9.md#current-dispositions-at-1edf4d7) and
[M2](../security/m2.md#current-hosted-dispositions-at-1edf4d7).

The source map and adopted generated matrix agree at 312/357 reconciled inputs
and 45 pending after the reviewed multisig-safety disposition. The e2e source map and generated matrix agree at
18 pending mappings and 11 client contracts.
There remain 68 divergences. Passing CI alone does
not reconcile source rows or accept later milestones. Strict M8 remains
off and M2-S02 remains open/unaccepted. Current work continues through M9,
with source review and substantial validation on free hosted CI.

Local `target` is absent, including the retired local build root.
No local build/test, service, macOS-host work, new temporary root or upstream
filing was needed. Preserve wallets, recovery material and unrelated work.

### Reviewed root and policy client batch

Seven additional regression functions are integrated after independent source
review. Root allocate/swap normalize exact decimal spellings to checked u64 rao
at the CLI boundary, including signed zero. Shared Money/Amount constructors
stay strict. D-042 records the 4096-byte and exponent-magnitude budgets; prompted
negative-zero replay uses attached option syntax and is reparsed by its fixture.
Move retains the picked destination name and still propagates a required source
identity lookup failure. Direct multisig Policy checks now name the semantic
operation, matching the executor's refusal diagnostics.

Two further client fixtures cover the exact auto-stake call/coldkey/subnet read
and remaining transfer/Proxy/Sudo unsigned-JSON/signature-attachment paths. The
auto-stake transcript preserves the upstream ignored execution Result and lists
six runtime limits. Its source e2e map has 18 pending keys and 11 client-contract
rows, now reflected in the authenticated generated reports.
The source-file map reaches 307/357 reconciled, 50 pending. The keyfiles row
retains oracle-only status after complete source-input review and exact hosted
PASS evidence for its two narrow native fixtures. Valid Fernet decryption,
the complete encrypted-created wallet scenario and isolated umask-000 execution
remain gaps; source-input reconciliation does not establish a complete native port.

A complete audit corrects 25 M2 execution documents and the validator guide's
metric name. Current instructions retire local caches and restore recipes,
keep optional infrastructure evidence unverified, and distinguish historical
results from current source. Actual event refresh, reveal tracking, parser/S04
proofs and Q30 optional-oracle machinery are described accurately. M2-S02 stays
open/unaccepted; no M2 security or milestone exit is claimed. The remaining
M3-M9 execution-record resource audit reviewed all 31 authored Markdown files.
Thirteen needed resource-policy corrections; 18 required no changes. Exact
patch review and all 13 reconstructed after-image hashes pass. Historical
counts and the durable hyperparameter proof remain intact; local restore/build
instructions and mandatory infrastructure requirements are retired. This
resource audit grants no source-step or milestone acceptance.

Direct rustfmt, whitespace, exact patch/source reviews and all 26 documentation
after-image identities pass. All seven new functions pass at `2cec5c6`, with
the corrected picker fixture, in the successful 3,077-test checkpoint above.

Hosted run [38883630624](https://github.com/sam0x17/rtao/actions/runs/38883630624)
at exact `80a22f8` completed with five supporting jobs passed and main failed:
3,069 tests passed, one failed and four were skipped. Strict Clippy reached
Nextest, but later doctests and documentation gates did not run. The sole
failure was `canonical_picker_answers_use_real_clap_parser_retries_and_final_relationships`:
its supplied conflicting option waived Clap's required address, so the fixture
never entered the expected public prompt context. The corrected fixture first
asserts that behavior, then omits both conflicting fields and checks collection
followed by final conflict rejection. The corrected fixture now passes at
`2cec5c6`; the earlier failed run retains its own source identity and result.

The bounded partial hosted-results record (not retained) authenticates 22 selected
PASS records, including both keyfile fixtures, and the sole failure. Its source
identities and full-run failure remain explicit; it validates none of the seven
newer functions. No raw log is retained. Exact proposal/review identities and
the next report-refresh steps are not retained in the repository.

### Report adoption and next combined hosted checkpoint

Report-only [38888110550](https://github.com/sam0x17/rtao/actions/runs/38888110550)
passed at exact `1af32bd`. The finalized binding authenticates all 36 source
inputs and artifact `12040193494` (58,602 bytes, SHA256
`33a643d7137ee0ae94a183764483926ec702056e69088bcc2aad788363bd1203`).
Inspection and an independent refetch adopted exactly four reports. They change
only the reviewed auto-stake client-contract row, keyfiles oracle-only source
input, one first D-042 pinning test, derived counts and ordinary measurement
metadata. All 23 dependency inputs, role inventories/host flags and other
compatibility claims are unchanged. No archive or extraction directory remains.

Ordinary combined [38889135192](https://github.com/sam0x17/rtao/actions/runs/38889135192)
passes at exact `2cec5c6`, with 3,077 tests and the doctest/docs results above.
Report generation alone supplies no regression execution or milestone acceptance.
The M3 readiness audit finds represented miner/authentication/template source.
Compiled-example wiring is now implemented below, awaiting hosted execution;
shared offline examples remain unfinished and also limit M2 guide acceptance. The security note's
old live-cache claim is retired; historical review identities stay intact.
M2-S02 remains open/unaccepted: direct owning-boundary inspection found no safe
compatible wrapper that erases the dependency's private Ed25519 object. Existing
upstream issue #3251 remains open, with no comments at this inspection.

The subsequent source batch adds one independently reviewed multisig
registration fixture with ordinary and shielded vectors. It uses distinct
member/group/hotkey identities, preserves the inclusion receipt and requires
later-block registration completion for the shielded path. Existing fake
transport support is reused; no production harness or dependency changes.
Formatting and applicability pass. This function is outside the `2cec5c6`
checkpoint and remains unexecuted; the next combined total becomes 3,078.
The multisig-safety source row stays pending.

### Compiled examples and current authentication review

The initial source batch wires pinned Docify 0.4.1 into the validator, miner and
owner examples and generated guides. The README template compiles to the
existing README bytes. Explicit source dependencies keep cached excerpts fresh.
Docify is a facade dev dependency and a dependency of unpublished xtask; it adds
no normal dependency to shipped client profiles. The existing free lock-refresh
lane has a bounded first-Docify resolver preserving old package identities,
metadata and resolved dependency edges while allowing only the two Docify edges
and their reviewed candidate closure. Independent review found a raw-reference
comparison issue; the correction compares resolved identities so Cargo's
qualification of unchanged references does not invent an edge change.

All 13 adopted source after-images, direct formatting and script syntax checks
pass. First lock run [38891932293](https://github.com/sam0x17/rtao/actions/runs/38891932293)
at `e3bd736` resolved ten additions but failed because the guard omitted the
`jobserver` build helper (and `termcolor`). No candidate lock was adopted.
The corrected guard pins all ten observed versions/checksums and permits only
the existing `cc` build tool's optional `libc`/`jobserver` edges within Docify's
closure. Old identities, metadata and resolved edges remain protected. Candidate
locks are retained even after guard failure for review. Second lock run
[38893353758](https://github.com/sam0x17/rtao/actions/runs/38893353758)
at exact `4f02d5a` passed. The authenticated candidate adds exactly those ten
packages and only the two Docify edges plus `cc` to `jobserver`/existing `libc`.
All old identities, metadata and edges are preserved; the fuzz lock is identical.
Independent source review and a separate artifact refetch passed before adoption.
Regenerated reports and hosted compilation/supply/tests remain pending. Current compatibility reports predate
these manifest changes. No generated lock or report is hand-edited. The successful
`2cec5c6` ordinary run validates the preceding sources only. Offline doc examples
and broader M9 documentation completion remain separate.

Hosted report run [38893835867](https://github.com/sam0x17/rtao/actions/runs/38893835867)
at exact `d498d6d` failed compiling Docify 0.4.1 before generating reports. Its
open Git/SHA requirements select API-incompatible git2 0.21 and sha2 0.11.
The correction selects published Docify 0.2.9, whose local export, embed,
embed_run and in-memory Markdown APIs match our uses and need neither Git/SHA
libraries nor their native build helpers. Its published macro source and
package checksums were inspected. A new hosted candidate will preserve the
entire pre-Docify dependency graph; no lock is hand-edited. Hosted lock run
[38895014224](https://github.com/sam0x17/rtao/actions/runs/38895014224)
passes at exact `e4b027f`. The reviewed and separately refetched candidate adds
nine pure Rust documentation packages against that original graph and only
the facade-dev/xtask Docify edges. It removes the failed Git/native-tool closure
and restores the old `cc` edges. Fuzz lock bytes stay identical. This is lock
preparation evidence; compilation, supply and regression execution still follow.

Two independently reviewed shared offline example tests now cover checked
native balances and Python-compatible numeric display. The generated
getting-started chapter and LLM text embed those exact functions. Embedding
only produces Markdown; hosted tests must execute both functions. With the
registration fixture, the next combined main total is 3,080. These additions
remain unexecuted. Btauth-vector, FakeChain-read and intent-plan examples remain
separate documentation work.

The upstream historical basket-index builder is now reconciled as oracle-only
under PLAN section 8.3. All 252 producer lines and native conversion/consumer
paths were reviewed; all frozen scalar/grid/baseline literals match. There is
no native archive sampler and no claim of producer or historical reconstruction
execution. Existing consumer source matches the preceding green checkpoint.

Report run [38895327494](https://github.com/sam0x17/rtao/actions/runs/38895327494)
at `67f0777` compiled the corrected tool and generated reports, then failed the
source-attribution guard. Root move/swap were listed in an additional-sources
comment rather than explicit derived-source headers. Both now have full headers
at the authenticated commit/license; production bodies are unchanged. No reports
from the failed run were adopted. The corrected hosted report check follows.

Hosted report run [38896301076](https://github.com/sam0x17/rtao/actions/runs/38896301076)
at exact `dece106` passes, including source attribution. Authenticate artifact
12044595804 (58,707 bytes, SHA256
`9eefd53289a02dc5b948266ee11a7229aafde024dc03c2b2fa98325be58baca2`),
inspect all four changed source rows and all 23 actual dependency inputs, then
separately refetch and adopt exactly four reports. Only the reviewed four Docify
dependency inputs and normal measurement metadata change in the graph record.
Profile package counts remain 225/362/366/417 and host flags remain
false/false/false/true. Runtime, oracle, hardware and other compatibility claims
stay unchanged. The generated matrix now agrees with source at 311/46.
This passes report/source checks; ordinary combined hosted execution is next
for 3,080 expected tests. No new milestone is accepted.

A fresh reader also completed two cold reads of all five current authentication
sources at `c920891`, finding no surviving concrete implementation defect within
their stated contracts. All five hashes match the preceding hosted checkpoint's
source. [M3](../security/m3.md#current-authentication-source-review-2026-10-16)
records the exact identities and API/deployment limits. This review runs no
validation and does not accept M3 or resolve M2-S02.

### Command review and required-address follow-up, execution pending

The current source batch carries root command summaries and stages through the
actual preview, final confirmation, external signer label and every saved
approval. Display context is bound to the semantic intent before RPC or signer
acquisition. Saved approvals retain their outer transaction labels and effects.
Existing diagnostic preparation remains available.

Ten non-EVM routes now use the existing public address pickers for missing
required fields. Signing-wallet selection precedes local hotkey choices; real
clap validation, field order, proxy reservations and positional replay remain
intact. Independent review corrected three fixture setup errors without changing
production code. Six review-carrier fixtures, six required-address fixtures and
one association fixture are authored, formatted and independently source-reviewed;
all 13 still require hosted compilation and execution.

The association fixture uses the real intent/executor, a finite signed-call
transcript and an exact receipt-pinned UID0 EVM-key read. Its one e2e mapping
has client-contract scope and five explicit runtime limits; registration,
ownership, association effects and real EVM state remain unverified. Source
e2e pending count becomes 19; source-file reconciliation stays 306/357 with
51 pending. Whole root/context/prompt rows, broader signer policy and EVM picker
integration remain separate. Bounded patch/review identities are
not retained in the repository.

Report-only hosted run
[38880542426](https://github.com/sam0x17/rtao/actions/runs/38880542426)
passed source parity at exact `6d3918641039ea10e88b049d1014ef742a90980a`.
Authenticated artifact 12037044413 is 58,405 bytes, SHA256
`2c8bba283bf36bf31a169098ff7e210799811a49489643d1c90030f846e6e5b1`.
Independent refetch adopted exactly four reports. The matrix changes only
the association row and derived counts: 19 pending and ten client contracts.
All other families, 23 dependency identities, role inventories/host flags,
68 divergences and compatibility claims remain unchanged. The bounded adoption
proof, archive and extraction tree are not retained.

A separate complete keyfiles-golden source review found no product defect.
One additional Unix fixture checks replacement of a 0644 keyfile with 0600
permissions, its new parent mode and preserved identity. Existing encrypted
coverage now checks the NaCl prefix and post-decryption signing, with no extra
KDF calls. The patch was independently reviewed; execution remains pending.
Valid legacy Fernet, the exact combined encrypted-create transcript and isolated
umask-000 execution remain unverified. Its oracle-only source row stays pending.
The combined checkpoint must execute all 19 newly added functions since 965669d,
plus the extended fixtures; the expected main total is 3,070. Five supporting
jobs passed on earlier source in run 38877875846; its main check was cancelled
when superseded, before a complete test summary. No earlier subset validates
these newer sources.

Combined hosted run
[38882046195](https://github.com/sam0x17/rtao/actions/runs/38882046195)
at `81e791eef2ba8a7000a22834a115b663391d2335` stops in Clippy before tests:
the association fixture imports `associated_evm_key` from the reads root,
but the public implementation is in `reads::m6_accounts`. Correct that fixture
import without changing the production API or lint policy. The bounded main
diagnostic is not retained; its 67,265-byte source log has SHA256
`6c522c799df32b98f6e0626057441d7110669acd0f6c1026c1af322a9955b05b`.
All 19 new fixture executions remain pending the corrected hosted checkpoint.

### Earlier CLI detail batch, hosted execution pending

The next source batch confirms the signing wallet before resolving a relative
root-swap hotkey, and retains an allocation picker's selected name and rate
without another individual quote RPC. Human wallet lists retain multisig
members, later optional contact/proxy fields and effective proxy defaults.
Pending multisig output retains member labels and full co-signer commands.
Mnemonic-clear guidance appears once on stdout in normal and quiet human
sessions; JSON remains a single document. Five new test functions and extended
actual-command fixtures await hosted compilation and execution. Direct rustfmt,
whitespace checks and independent source reviews pass.

Complete source reviews and exact 965669d results reconcile the metagraph,
FakeNode harness and test_rpc.py rows. The source map and generated matrix now
agree at 306/357 reconciled, 51 pending. Its subsequent command-summary and
required-address source repairs are recorded above; whole source rows stay pending.
Strict M8 remains off and M2-S02 stays open/unaccepted. No local builds/tests or
new upstream filings were used for this batch.

Report-only hosted run
[38876950939](https://github.com/sam0x17/rtao/actions/runs/38876950939)
passed at exact `b06a325b3985cc6dbc21aaa1ff7b9d39cec39bfd`, including the full
source-parity check. Authenticated artifact 12035678693 contains only four
reports (58,349 bytes, SHA256
`2770c94c6e5507a9f829272eb3f7ababa080746637c1db55e4add34ad5340a08`).
Independent refetch confirms exactly those three source-row replacements;
all 23 shipped dependency inputs, role counts/host flags, 20 e2e gaps,
68 divergences and other compatibility claims remain unchanged. The bounded proof
of the reviewed identities, archive and extraction tree are not retained. This report-only run did not execute
the five new fixtures; the ordinary combined checkpoint remains pending.

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
and raw-weight semantics are unchanged. Bounded proposal/review evidence is
not retained in the repository; no build output or extra temporary directory was created.

### Subsequent source disposition and verified report refresh

Complete independent review reconciles `codegen/check.py`: all six functions,
declarations, native producers/consumers and gate callers have explicit homes.
The metadata-name/Unknown-classification and Python-strip-nonblank repairs both
have exact PASS records on `1edf4d7`. Native pinned regeneration and Result/CLI
errors replace Python live generation and stdout conventions; fresh Python,
oracle and live drift remain unverified.

Hosted [38859189354](https://github.com/sam0x17/rtao/actions/runs/38859189354)
passed full source parity at clean `ec263ec243659b3a9925f41862795db66bdcdb56`.
Artifact 12029210729 is 58,018 bytes, SHA256
`da3e79b71704bf997cc96d26aa544949de9330f4a1f47dc0d8d5e736c8802159`.
Authenticated inspection and independent refetch supply exactly four adopted
reports. The only matrix change is the reviewed check.py row, reaching 298/357
reconciled and 59 pending. All 23 dependency inputs, role counts, host flags,
20 pending e2e mappings, 68 divergences and other compatibility claims remain
unchanged. Bounded inspection/adoption proofs, archives and extraction trees are
not retained.
Ordinary [38858957306](https://github.com/sam0x17/rtao/actions/runs/38858957306)
at `2235efb` remains the weight-fix validation checkpoint. Other pending rows,
strict M8 and milestone status are unchanged.

### Earlier source and CLI batch

The current `ab5d792` result supersedes this batch's earlier unexecuted-fixture
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
failed at exact `1ed7445c16b00c8110ebfe76143cba64404e9ee0`: the license guard
could not attribute the newly reconciled pruning home in `rtao-neuron`. Its
combined two-source comment did not supply the exact canonical upstream path.
Split that comment into one canonical header per source without changing the
guard. No report artifact was adopted. The 59,499-byte job log has SHA256
`0971f8c9149cd5390fb2b53e9df618110d4ae8278cf6538bd9ab7d73ad60a681`;
bounded failure evidence is not retained in the repository.

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
passed full source parity at clean `9d7c6f8ff42b4ec86bca8553c20ec2fb67eac11a`.
Authenticate and independently refetch artifact 12029777947 (58,166 bytes,
SHA256 `ebff9b28aa3c2329462ba83031580e438135c7e97e69e598a434fefe83d8a95d`)
before adopting exactly four reports. Only the three reviewed staking, weights
and CLI-pruning rows change in the matrix: source map/matrix now agree at 301/357
reconciled and 56 pending. All 23 dependency inputs, role counts, host flags,
20 pending e2e mappings, 68 divergences and other compatibility claims remain
unchanged. The bounded inspection proof (SHA256
`ab3137fe89e8d2e3ca3cc865126ea1a6d278d6ca9d50d339244f6a0269ff6ed9`)
and adoption proof (SHA256
`469ff440fe3b23ceae2991634b412ff20980935d86042da9b8f825a19d06c7e1`)
are not retained in the repository.

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
and their label/review-request lists are empty. The evidence record is
not retained in the repository; this refresh did not repeat the authored-search
census or source/test audit. No GitHub filing write occurred.

Ordinary [38863766724](https://github.com/sam0x17/rtao/actions/runs/38863766724)
at `4cf93bd99b121583517c03684c844f10aaf309ba` exposes E0382 in the MSRV
CLI binary check: an async move handler consumed original replay argv before
the newly wired finish boundary borrowed it. Retain the owned vector outside
dispatch and give the seven handler closures a shared slice. No replay value
or prompt behavior changes. Direct formatting and whitespace checks pass;
the corrected ordinary checkpoint must establish compilation and the 26 new
fixture results. Bounded MSRV evidence is not retained; the 58,981-byte
log has SHA256 `c776d6f6862cc8b4af9ba8737660b973258c7269eed3f15c3e51ab6088b5d935`.

Corrected ordinary [38864132855](https://github.com/sam0x17/rtao/actions/runs/38864132855)
at `65fa07f90b43a31d6a680c986c70e6303c761143` passes MSRV, no-host and
semver. Its main job stops before tests on two `needless_borrows_for_generic_args`
findings in the new wallet fixture's environment closure. Pass the Copy closure
directly at both calls. Formatting passes; the 26 new fixtures still require
execution on the corrected checkpoint. Main failure evidence is
not retained; the 68,704-byte log has SHA256
`0876f601d04385eb3305af03cfd868c69bd710d0869e004849f0f032617cdb95`.

### Verified source dispositions and the next root-picker repair

All 26 additions in the preceding batches pass at `ab5d792`. The exact named
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
included in the `ab5d792` execution result and awaits hosted validation. The
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
No local Cargo/build/test, service, macOS-host work, new temporary root or filing.

### Authenticated M1 preparation and combined follow-up

Both preparation runs passed at exact `2143d2e`: codegen/report run
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
macOS-host work, new temporary root or filing occurred.

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
is open/unaccepted. Bounded source/adoption reviews are not retained in the repository;
no build output, archive or extraction tree was created.

Hosted report refresh
[38826175917](https://github.com/sam0x17/rtao/actions/runs/38826175917) passed
at clean `a24e4dac5cb993b983bc0c0667f14262ff939c75`, including full source
parity. Authenticated and independently refetched artifact 12015413800
(57,978 bytes, SHA256
`f3af367e6632bef6c2200ca8fee15bb86387e81f5598e348337ca4db52ffa8da`)
supplies exactly four adopted reports. Matrix changes are limited to the thirteen
reviewed source promotions and two appended D-041 tests. All 23 dependency
inputs, role inventories, host flags, 20 pending e2e mappings, 68 divergence
identities and other compatibility claims are unchanged. Inspection and adoption
proofs are not retained in the repository.
The timer patch has independent source approval; its three fixtures and the integrated
compiler, transport and extractor fixtures await ordinary hosted execution.

Ordinary [38827263561](https://github.com/sam0x17/rtao/actions/runs/38827263561)
at `0c4cc196edbf79489b2d9f0568177d36f11e31d2` passed all five supporting
jobs: supply, MSRV, feature graphs, semver and no-host. Its main job passed
Clippy for all targets and all source/pin/extraction checks, then failed strict
rustdoc on two unqualified module links to timelock `seal` and `open`.
Qualify both public targets without changing behavior or lint policy.
The 106,372-byte terminal log has SHA256
`e30baf51a75ef2456d0d676950754e4be50e48c07bd0e9350cbc8dcc331bdafd`;
the bounded proof is not retained. Docs precede nextest in the current
gate, so behavior tests and workspace doctests were not reached. The compiler
corrections are verified, but current fixture execution and documentation
completion remain unverified. The separate weights, staking and source-map
proposals remain ignored and unapplied while this documentation gate is fixed.

Ordinary [38824416747](https://github.com/sam0x17/rtao/actions/runs/38824416747)
at `9e02f8e6e9c23204340fdd7482928b6ac1bd24c1` passed supply, MSRV and
semver; no-host failed strict Clippy on session.rs's Url import,
which the handshake refactor left needed only by tests and frame fuzzing.
Gate that import with the existing test/fuzzing condition; do not suppress
linting. The terminal no-host log is 184,651 bytes, SHA256
`9254c2e86e6d7a6fc83d9ad914780d14822329ec028150f3987a91e8f68a2355`.
Bounded proofs (the check record and its authenticated detail
follow-up) are not retained. Main also failed during strict Clippy compilation: the new public RPC
fixture used primitive H256 where RpcError::FateUnknown requires RPC H256.
Qualify that one constructor while retaining native hashes in client APIs.
Its 77,984-byte terminal log has SHA256
`351ce4a11a4b49f367c5bbe81e8f28722e76af135fb38b564df3a1214fd1cb3d`;
the bounded proof is not retained. No tests or documentation ran
there. The feature job subsequently passed and the run is terminally failed.
Execution of both corrections remains pending. The reviewed source batch and
report refresh above are ready for the next combined ordinary checkpoint.

Hosted report refresh
[38823630645](https://github.com/sam0x17/rtao/actions/runs/38823630645) passed
at clean `996698371d9bb2ea03e32932e41905b4b42ebd44`. Its 57,584-byte artifact was
authenticated and independently refetched; exactly four reports are adopted. The matrix
changes only the two reviewed precompile/RPC source rows, reaching 284/357
reconciled and 73 pending. All 23 dependency inputs match that source; exactly
the reviewed workspace lock and RPC manifest hashes change. Role inventories,
host flags, 20 pending e2e mappings, 68 divergences and other compatibility
claims remain unchanged. Bounded inspection/adoption proofs, archives and
extraction trees are not retained.
The combined hosted gate must still execute the newer transport fixtures,
constructor-refusal correction and documentation fix. Strict M8 remains off,
M2-S02 open/unaccepted and no milestone is accepted.

Hosted report refresh
[38819346876](https://github.com/sam0x17/rtao/actions/runs/38819346876) passed
full source parity at clean `29d62761ac9496b7a54df72571fa79cbed8ddd7e`. Adopted
exactly four report files after authenticating and independently refetching
the archive. Matrix changes are limited to the two reviewed substrate/interface
rows. All 23 dependency inputs, role counts, host flags and other compatibility
claims are unchanged. Bounded inspection/adoption proofs, archives and
extraction trees are not retained.

Hosted generation
[38816297566](https://github.com/sam0x17/rtao/actions/runs/38816297566) passed
source parity at clean `6c9e9ffdde5d04b3b61b0c64e884e115ca8c540c`. Both artifact
archives were authenticated and independently fetched twice. Adopted outputs
replace the explicit precompile bootstrap with the actual generated consumer,
add five authenticated Python tables and refresh four reports. Exactly seven
files change; the other 16 generated files and all 23 dependency inputs are
unchanged. Existing legacy runtime-API output is unchanged. Bounded inspection/adoption proofs, archives and extraction
directories are not retained.

The published matrix has 297/357 source rows reconciled, 60 pending, 20 pending
e2e mappings and 68 divergences. Source mappings retain explicit native API,
strictness and coverage limits. These are source reconciliation results.
Strict M8 completeness remains off; no milestone is accepted. M2-S02 remains
open and unaccepted, and upstream issue #3251 is still open.

Normal [38809901640](https://github.com/sam0x17/rtao/actions/runs/38809901640)
at `7b69f8eed7324cd0f4a7c2c1f77314a37d1928e5` completed with all five supporting
jobs successful. Its main gate passed 2,963 tests with zero failures and four
skips, then passed the workspace doctests. Both serving-fixture repairs are
therefore confirmed for that source. The gate subsequently failed strict
rustdoc because public StateStore::save/load documentation linked through a
private module to MAX_CHECKPOINT_BYTES. The published correction states the
actual 64 MiB limit without changing visibility, behavior or lint policy.
The exact 781,765-byte terminal log has SHA256
`ba00375b057c261cbb8ec7efb71fb77897bfe305e384b6b2231a6aebeb35c502`;
the bounded proof is not retained. Book generation was not completed.
Follow-up ordinary
[38814558931](https://github.com/sam0x17/rtao/actions/runs/38814558931) at
`a7b6a4a665c0bcea146f54389c20706c7e0157a5` completed with all five supporting
jobs passing. Its main gate passed all 2,968 tests with four skips and workspace
doctests, then failed strict rustdoc on one unresolved PythonBalance link.
Qualify that existing public type's link without changing behavior or lint
policy. The 782,118-byte log has SHA256
`8343a0f3afc901d20745f0f6b5d1c12f053e0f33d7fa7feac4c18141ca115394`;
the bounded proof is not retained. Book generation was not reached.
Ordinary [38818850318](https://github.com/sam0x17/rtao/actions/runs/38818850318)
completed at `82aed8719997c9bb71302ba839054ecf3c82f060`. All five supporting
jobs passed. The main gate ran 2,977 tests: 2,976 passed, one failed, four skipped.
The failure is the extractor's constructor-semantic refusal fixture; doctests
and documentation were not reached. The exact 773,213-byte log has SHA256
`572428c4f9513974a72b200c53e5fc947ab20fe882f258bccf469a74b5818754`;
the bounded proof is not retained.

An authenticated second read of that immutable log confirms seven exact named
passes: positive unsigned-seed reconstruction/signing/attachment, Utility.batch
roundtrip, all three canonical CLI notice fixtures, source-table mutations and
comprehension refusals. The proof record is not retained. The positive
seed is validated through its ordinary fixture; fuzz-driver/sanitizer execution,
fresh Python/oracle results and newer-source test success remain unclaimed.

The failed refusal comes from RustPython 0.4 dropping singleton parentheses and
deriving AnnAssign.simple from the remaining Name node. The lexical correction
checks the original statement-to-name source span, retaining the parenthesized
case and adding nested/index variants plus exact mutation diagnostics. Direct
formatting and source inspection pass; hosted execution of the correction is
still required.

Source checkpoint `29d6276` repairs terminal policy refusals during paced
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
at clean `9a9d05344be0535dddb20f097bdd5ec14432329c`. Its artifact was authenticated
and independently fetched twice; both generated locks are adopted. Each adds only
the existing rtao-rpc -> tokio-rustls edge; all package identities and other
bytes are unchanged. The handwritten candidate was excluded. The bounded proof, archives and
extraction trees are not retained. The existing
resolver checks both locks under one 170-second deadline. Source reports are
refreshed for ordinary validation. The main gate now checks documentation
before nextest/doctests, preserving every command and catching link failures earlier.

At this earlier checkpoint, local target output was 547,192 KiB; only
source inspection, direct formatting and bounded bookkeeping ran locally.
No local builds/tests, new temporary root, node, macOS-host work, device run
or new upstream filing was needed. Preserve wallets, recovery material and
unrelated work.

## Earlier client checkpoints, 2026-10-15

These records retain their own source and execution scope. The current
checkpoint above supersedes their instructions to dispatch or retry checks.

### Earlier header and source checkpoint, superseded by 0a92c0e

Normal [38790070632](https://github.com/sam0x17/rtao/actions/runs/38790070632)
at `bf2bf9409260be023933bed33af61b3ed85e2388` stops before tests on
`_subtensor.py`'s extra helper-crate mapping. All five supporting jobs pass.
Raw main log is 61,923 bytes, SHA256
`dc01e374fe32111e15206f169f139132dd17361e8e2dc8ca775f790359584fb8`;
the bounded proof is not retained. Tests/doctests/docs were not reached.
The preceding 9748aae run caught the combined header's incomplete source path;
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
succeeds at clean `72a0d289265948a6444a396bff84acfcc3194662`.
Authenticate the 55,264-byte archive, four regular report members and 23
unchanged dependency inputs. The matrix changes exactly the eight reviewed
file rows: 228/357 reconciled, 129 pending, with 20 pending e2e mappings.
Role inventories and host flags are unchanged; graph changes are only CPU,
time and source provenance. All compatibility, runtime and oracle claims
remain unchanged. Review/adoption proofs, archives and extraction directories
are not retained.
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
an excess top-level element. Independent source reviews and rereads pass;
no local build/test or native password/browser service ran. Snapshot siblings
resolve eagerly and need an open connection; D-007 explicitly covers derived
slot timing from the retained runtime. Timelock parser/deadline, precompile
table/diagnostic, Vault browser/QR and picker source gaps remain pending.

All 70 selected S9 regressions passed in prior ordinary run 38780323094 at
`9291b383e76de053ff07ed73c18383dc30dd0536`; all 29 selected boundary/test
files are byte-identical at `9748aae`. The bounded record of immutable run/job/head
identities, raw-log digest and exact PASS lines is not retained in the repository. That run failed three other tests;
these targeted results do not establish C1 or close the findings. M2-S02
Ed25519 final-drop erasure remains open and unaccepted.

Hosted generation [38777439872](https://github.com/sam0x17/rtao/actions/runs/38777439872)
succeeds at clean `daf7ab1`. Authenticate two exact archives, all 20 regular
allowlisted members and all 23 unchanged dependency inputs. Only the new
22,297-byte root-claim fixture differs among generated interfaces; both source
blobs/SHA256s and case inventories are checked. The matrix changes exactly
three reviewed rows: 212 reconciled/145 pending, with 21 e2e keys pending.
Existing oracle and compatibility claims, role package inventories and host
flags stay unchanged. Record the hosted CPU accurately as measurement provenance.

Review/adoption proofs are not retained in the repository;
no archive/extraction directory remains. Test-only unwraps are replaced with
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
proof not retained. No adopted artifact or new test success.


A follow-up aligns decimal ABI integer strings with Python's default
4,300-digit limit, including leading zeroes; hexadecimal remains exempt.
Public coercion fixture source covers both sides, signs, Unicode digits and
separators while retaining native u64 and ABI-width checks. Formatting and
whitespace pass; execution remains pending. Hosted generation38776671979
runs at 353c6b7; this later source-only fix changes none of its dependency,
extraction or report inputs.

Generation run38776446102 at8112797 fails before producing reports: the new
cancellation-aware hash request needs an explicit JSON Value type before its
null check. Add that annotation, preserving the same request and cancellation
path. Raw log51,419bytes SHA256
`b7fba301dd5137e054148f0eb0b567f7549004ee42a998fc8d96b8f9186dfeb4`;
proof not retained. No generated artifact or new test result.
Retry hosted generation on the corrected clean source before ordinary CI.

Normal [38770765426](https://github.com/sam0x17/rtao/actions/runs/38770765426)
at `e7fa64b` is terminal: all five supporting jobs, main compilation,
strict Clippy and ordinary parity pass. Nextest runs 2,831 tests: 2,784 pass,
47 fail and 4 are skipped. Doctests/docs are not reached. Raw main log is
1,258,642 bytes, SHA256
`b5a3c5b4b04cc477e8b322c38d131a3703149d6877e63296592007e5318acc9a`;
the bounded exact-source failure record is
not retained in the repository. These results predate the later source batch.

All 47 failure causes now have source corrections or reviewed fixture repairs.
Production corrections include signer-specific metadata digests without stale
cache reuse, result hash hex, authenticated modern preimage length, deterministic
Vault replacement close, upgrade repository validation and decrypted-title
stream. Fake request recording preserves every separate transport call while
excluding internal delegation, including recovery after injected panic.
Fixture repairs preserve one-submit/deadline/nonce/shield, parent-runtime and
exact numeric-bit checks. Independent source rereviews and a cross-review
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
succeeds at clean `39f019e`. Authenticate and adopt exactly four regular report
members, archive digest/size and all 23 committed/current dependency inputs.
Only the explicit rtao-tx serving test-feature manifest hash changes. Role
inventories and host selection remain unchanged. Matrix differences are exactly
five reconciled source rows, two corrected pending root-claim homes, three
qualified client lifecycle bindings and new unverified D064/D065. Source counts
are 209 reconciled/148 pending; 21 e2e mappings remain pending. Every existing
oracle classification and compatibility claim is unchanged. The adoption proof
is not retained; no archive/extraction directory remains.

At 16:49 UTC normal run `38770765426` still has main/features active and four
supporting jobs passing. Retain that run to obtain its ordinary test outcome;
the report-adoption commit skips automatic CI. Dispatch the next normal hosted
run on the adoption head after the existing main job concludes. Its results
cannot validate the later batch. No fresh test/docs or milestone acceptance.

Normal run [38770765426](https://github.com/sam0x17/rtao/actions/runs/38770765426)
validates the isolated compiler correction at `e7fa64b`. At inspection supply,
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

Primary review and available independent case/gate reviews found no remaining
source defect in this batch. Direct formatting and whitespace checks pass;
hosted compilation, generation checks and execution remain required. Review
proofs and the individual source reports are not retained in the repository. No local
build/test, service, temporary directory, upstream filing or milestone acceptance.

Normal run [38769405520](https://github.com/sam0x17/rtao/actions/runs/38769405520)
at `4890351` passes every ordinary parity layer, including all 63 divergence
declarations with the exact optional D031 deferral. Main Clippy then fails
compiling the new unit fixture: `tempfile` is not an xtask dependency. Extract
the existing document validator and call that same function from the fixture,
preserving all cases without temporary files or a new dependency. The transfer
lifecycle target has no reported compiler diagnostic in the completed Clippy
log; execution remains unverified. Supply, MSRV, no-host and semver pass;
features is still active at inspection. The proof is not retained.
This small compiler correction is pushed separately from the later source
batch so ordinary hosted validation can proceed while that batch is reviewed.

Normal run [38767926902](https://github.com/sam0x17/rtao/actions/runs/38767926902)
at `5edf4a6` has failed its main check. Supply, MSRV, no-host and semver pass;
features remains active at 16:28 UTC. The earlier parity check now passes the
corrected attribution/discovery layers and all 115 e2e keys, then exposes an
ordering bug: optional pending M10 D031 is required to have pinning tests before
the validator reaches its documented deferral. Limit the empty-list allowance
to that exact pending/M10/addition tuple. A full-register regression exercises
the actual strict validator, then rejects active/M9/changed-class mutations.
Independent source rereview clears the correction. The proof is
not retained; Clippy, ordinary tests, doctests and docs were not
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
or state advance. Primary and independent source rereviews find no remaining defect;
hosted compilation/execution and mapping integration are pending. No chain
economics, atomicity or rollback is claimed. No local build/test, service,
temporary directory or upstream filing.

Refresh [38766936693](https://github.com/sam0x17/rtao/actions/runs/38766936693)
succeeds at clean `02fad70`. Verify and adopt its four exact regular report
members, immutable archive digest/size and all 23 committed/current dependency
inputs. The matrix changes only the two reviewed balance/property dispositions
and five corrected derived-code homes. Source counts are 204 reconciled and
153 pending; all 115 e2e rows, oracle classifications and runtime qualifications
are unchanged. Dependency inventories, host selection and compatibility claims
also stay fixed. The adoption proof is not retained; no archive or
extraction directory remains. Normal combined hosted validation is next.
Independent rereviews of the balance mapping and final mapping/discovery gates
are active. No local build/test, service, new filing or milestone acceptance.

Normal run [38762528504](https://github.com/sam0x17/rtao/actions/runs/38762528504)
at `463c551` is terminal. All five supporting jobs, main compilation and strict
Clippy pass. Parity checks the new five client-contract bindings and all 115
e2e keys, but stops before tests on a false CLI-derived package home and missing
xtask test discovery. The D022 witness exists and Cargo tests it; the source
index considered only packages under `crates/`. Discover declared explicit
workspace members and immediate-child globs, retaining exclusions, repository
bounds, feature/harness/ignore checks and the inclusion-graph limit. Add focused
positive and refusal fixtures. Preserve the D022 reference and price assertions.
The proof, including the full parity-layer report, is not retained in the repository.

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
succeeds at clean `8194cff`. Verify/adopt four exact regular report members,
the immutable archive digest/size and all 23 unchanged dependency inputs.
Role inventories/host selection and compatibility claims stay unchanged.
Matrix changes are exactly ten reviewed file rows, two new unverified decisions,
D055's exact module reference and the scoped e2e evidence described below.
All 115 e2e keys remain: 72 intent plans, 6 read fixtures, 5 client contracts,
6 hosted scenario sources, 2 inapplicable and 24 pending. Runtime claims remain
unverified or inapplicable; no fixture result is reported as a chain execution.
The adoption proof is not retained; no archive/extraction directory remains.
Combined normal hosted validation and independent gate-integration rereview
remain pending. A spelling-only source inspection finds all 199 registered
divergence functions, including two manually traced module aliases; this does
not establish feature/harness qualification or execution.

Run [38759055202](https://github.com/sam0x17/rtao/actions/runs/38759055202)
is terminal: all five supporting jobs, main compilation and strict Clippy pass.
Parity stops before tests on D055's incorrect `client::tests` module path.
The unchanged fixture is declared in `client::attempt_budget_tests`; correct
only its registered witness path. Its body preserves completed oversized-body
refusals across both timing boundaries, without sleeps or retries. The evidence record,
with the full raw-log digest, is not retained. Do not infer nextest,
doctest or docs success. Compatibility refresh `38761443333` at `08fdc0d`
predates this register correction; its matrix cannot be adopted as current.
The corrected source requires a fresh hosted report followed by normal CI.

Normal run [38759055202](https://github.com/sam0x17/rtao/actions/runs/38759055202)
at `5aa29cf` passes all five supporting jobs; main check remains in progress at
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
tie found during that repair. All ten blobs/SHA256 pins are reverified; that
identity record is not retained in the repository.

Five independently reviewed policy/fee scenario sources now use real intents
and Executor, independent expected calls, exact fee-query decoding and refusal
diagnostics, with no private-key acquisition or submission. Their exact mapping
entries reduce pending e2e rows from 29 to 24. New validation/matrix code binds
only those five client contracts, requires unverified runtime/fee limits, checks
ordinary CI inclusion and classifies every existing mapping's source evidence.
Strict pending and oracle rules remain intact. The gate integration needs an
independent reread and hosted execution; its review request is
not retained in the repository. Fresh hosted matrix generation and
combined ordinary validation remain required. No local build/test, service,
temporary directory, new filing or milestone acceptance is claimed.

Compatibility refresh
[38757888373](https://github.com/sam0x17/rtao/actions/runs/38757888373)
succeeds at clean `486a71e`. Verify/adopt exactly four regular report files,
the archive identity/digest and all 23 unchanged dependency inputs. Role counts
and host selection stay fixed. The matrix changes only the twelve reviewed
core-py source rows, retaining oracle-only/unverified classifications; source
counts are 192 reconciled/165 pending, with 29 e2e mappings pending. Compatibility
and runtime claims are unchanged. The adoption proof is not retained.
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
The review proof is not retained; no Python, container or local build ran.
Fresh hosted matrix generation is required before ordinary validation.

Normal run [38756773737](https://github.com/sam0x17/rtao/actions/runs/38756773737)
at `4a0887f` passes semver and MSRV at inspection; supply, no-host and features
remain live. Main Clippy stops before tests on two cloned values used only to
create borrowed one-element slices in the new Children ordering fixture. Use
`std::slice::from_ref` at those call sites, preserving both assertions and the
later owned tool arguments. The proof is not retained. No lint gate or
assertion is disabled; the corrected source needs another normal hosted run.

Compatibility refresh
[38756143897](https://github.com/sam0x17/rtao/actions/runs/38756143897)
succeeds at clean `774baa1`. Verify/adopt its four exact regular report members,
archive identity/digest and all 23 dependency inputs. Only xtask's reviewed
license input hash changes; role packages and host selection are unchanged.
The matrix changes only D-057's two portable pinning references, with all other
source statuses and oracle classifications retained. Runtime validation claims
remain empty. The adoption proof is not retained. The combined source and
reports now need the next normal hosted check; no test or milestone acceptance
is inferred from generation.

Generation [38754665115](https://github.com/sam0x17/rtao/actions/runs/38754665115)
succeeds at clean `7d3441c`. Both exact archive identities/digests, nineteen
regular allowlisted members and all 23 committed/current dependency inputs
are verified before adoption. Role counts remain 225/362/366/417 and host
selection remains false/false/false/true. The sole generated Rust change is
AnyIntent's reviewed unknown-op delegation; twelve other Rust files and the
environment artifact are byte-identical. Chain rules gain only the reviewed
proxy-codec and reveal payload/epoch-prefix sections; existing rules are
unchanged. Matrix changes are eleven file homes/statuses, three e2e dispositions
and new unverified D-061. Compatibility/runtime/oracle claims do not change.
The bounded proof is not retained; no archive or extraction
directory remains.

Normal run [38753200118](https://github.com/sam0x17/rtao/actions/runs/38753200118)
is now terminal: all five supporting jobs pass, and main compilation/strict
Clippy pass. Parity stops before tests on xtask's inherited license and a
Unix-only D-057 pinning reference. Declare the existing combined Apache
obligation in xtask's manifest and register the existing unconditional regular
file/rotation witnesses. Keep the Unix FIFO and broken-symlink tests intact as
additional platform fixtures, with their execution still pending. No coverage
or license rule changes. The bounded evidence record is not retained.

The changed manifest needs a fresh hosted dependency/compatibility record,
and the changed D-057 witnesses need an updated matrix. Use a compatibility-only
refresh before the next normal combined run. No local build/test or milestone
acceptance is claimed.

Normal run [38753200118](https://github.com/sam0x17/rtao/actions/runs/38753200118)
at `1462b5d` passes semver, MSRV, no-host and supply at inspection; main check
and features remain live. It includes the named signing-recorder type. No
current full workspace test success is established.

The next source batch reconciles eight more files: 180 of 357 source rows are
now reconciled, with 177 pending. Two exact ordinary read bindings and the
obsolete core tempo-rejection disposition reduce pending e2e mappings to 29.
Source review records are not retained in the repository. Complete Plan approval JSON and all nine manifest
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
at `d534c3e` passes semver, MSRV, no-host and supply at inspection; features is
still running. The three fixture type errors are gone. Main check stops before
tests on one Clippy type-complexity finding in the nested-policy signing
recorder. Name that fixture type without suppressing the lint. Bounded evidence
is not retained. Final independent rereads find no remaining defect
in the corrected multisig dynamic-role continuation or pinned-only shielding
guard; executed regression closure remains pending. No local build or test ran.
The run is now terminal; features was cancelled when the lint-correction push
superseded it.

Normal run [38750380241](https://github.com/sam0x17/rtao/actions/runs/38750380241)
at `ac39773` passes semver, MSRV and supply at inspection; features and no-host
are still running. Main check stops before tests on three fixture compiler
errors: Crowdloan state needs typed seeding in two fixtures, and V15 pallet
metadata exposes `error`, not `errors`. Correct those fixtures and remove the
unused descriptor import. Formatting and diff checks pass; execution of the
corrected source requires the next normal hosted run. The bounded failure
record is not retained. No local build or test ran.
That run is now terminal: no-host also passed, and its features job was
cancelled when the fixture-correction push superseded the run.

The source batch is committed in `7bc0092`. Generation run
[38749149983](https://github.com/sam0x17/rtao/actions/runs/38749149983) succeeds;
its eighteen exact regular members and both archive digests are verified, along
with all 23 dependency-input hashes. Role inventories and host selections are
unchanged. All thirteen generated Rust files and environment data are identical.
Only reviewed file/divergence matrix rows and measurement provenance change;
no runtime/oracle success is added. The adoption proof is
not retained. Reports are adopted for the next normal CI run;
combined compilation, regression execution and final security disposition remain
pending. No local build or extraction directory was created.

Run [38746145922](https://github.com/sam0x17/rtao/actions/runs/38746145922)
at `8ffdafa` finishes with all five supporting jobs passing, including supply.
Main compilation and strict Clippy also pass. Parity then stops before tests
on the missing basket producer's canonical attribution and a renamed D-044
fixture reference; both are now corrected in source. The bounded log record
is not retained. No current nextest/doctest success is established.

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
`2e2535b` passes features (including unpatched shipped-crate and generic-keyfile
checks), MSRV, no-host and semver. Main check stops before tests on CLI fixture
qualification and Clippy errors; those sources are now corrected. Supply stops
on unsafe-libyaml's informational maintenance notice. The exact reviewed policy
and upstream feature constraints are recorded in
[supply-unsafe-libyaml.md](supply-unsafe-libyaml.md). These corrections need a
fresh normal hosted run. No current full test pass or milestone acceptance is
claimed. M9 security/read-map changes remain a separate uncommitted batch.

Normal run [38723619536](https://github.com/sam0x17/rtao/actions/runs/38723619536)
at `43bedb6` passes all five supporting jobs, main compilation and strict
Clippy. Parity now passes the corrected catalog, pending-read and e2e checks,
but detects the facade's inherited license missing the Apache obligation from
its derived blocking adapters. Its manifest now uses the required combined
expression. Nextest/doctests were not reached. The changed manifest requires
a fresh hosted dependency report before ordinary validation.

Normal run [38722567841](https://github.com/sam0x17/rtao/actions/runs/38722567841)
at `fbdcd91` passes features, MSRV, no-host, supply and semver. Main compilation
then strict Clippy finds `expect_err` in the new duplicate-read regression.
The fixture now requires the error through fallible test context, preserving
the assertion and the lint. Parity, nextest and doctests were not reached on
this run; another normal hosted run must verify the correction.

Normal run [38720004914](https://github.com/sam0x17/rtao/actions/runs/38720004914)
at `fee341a` passes features, MSRV, no-host, supply and semver. Main check passes
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
at `147d953` succeeds. Both archive digests, seventeen exact regular members
and all 23 committed dependency inputs are verified before adoption. Its only
generated-code change swaps the two basket header lines; all other generated
interfaces and the entire parity inventory are unchanged. Role inventories
remain 225/361/365/416 with core host disabled for core, validator and miner.
The next normal run also includes the separately reviewed shield repair below.

Source adds the remaining 62 reads and the blocking facade. All five read
groups have fixture source and independent source reviews. Generation run
[38715060741](https://github.com/sam0x17/rtao/actions/runs/38715060741) at
`e7aa703` succeeds and its outputs are verified and adopted: fifteen exact
archive members, all 23 dependency hashes, unchanged role package inventories,
62 read availability/output/body changes, four input overrides and D-027's
exact fixture mapping. All 83 intents and 102 reads now have generated
integration. Blocking output has 14 namespaces and 101 catalog methods, with
the curated typed metagraph adapter supplied by hand-written source. Ordinary
combined compilation and execution remain pending. See [m6-read-source.md](m6-read-source.md).

M6's 24 CLI commands and complete 102-query tree now have authored source,
parser/entrypoint/FakeChain fixtures and independent source reviews. The root
basket generator puts its 45 frozen samples and 171 fund baselines in the CLI
only. Run `38716924750` at `99775f4` succeeds; its reviewed artifacts supply that
exact table and six rustdoc corrections, with other interfaces unchanged.
Reports from this run are not adopted because parity source advanced after
its commit. Generation run `38718933791` at `e4f5bde` then succeeds and is
verified/adopted: seventeen exact regular members, all 23 dependency hashes,
unchanged role inventories, only the six frozen basket records removed from
the shared catalog, four M6 test mappings and the exact divergence registrations.
All other generated interfaces remain unchanged. Its bounded evidence is
not retained; ordinary combined validation is next.

The keyfile parser repair is committed in `846a4b3` after independent source
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
at `f2dd8a4` passes features, semver, no-host, MSRV and supply. Main check passes
compilation, strict Clippy, source/parity guards, all 2,022 executed nextest
tests and nine doctests; four nextest tests are skipped. It then fails strict
rustdoc because six generated parameter descriptions contain unescaped HTML
placeholders. The generator and adopted hosted output now render them literally;
the docs check must be rerun. These results exclude the newer M6 source and parser
repair. Bounded evidence is not retained.
Generation run [38714402721](https://github.com/sam0x17/rtao/actions/runs/38714402721)
at `ac7e297` fails before regeneration because the new blocking-accessor format
string has an unmatched closing brace. The generator source now escapes each
generated scope explicitly. No output from the failed run is adopted.

Normal run [38710720744](https://github.com/sam0x17/rtao/actions/runs/38710720744)
at `ab77cb4` passes all five other jobs, compilation, strict Clippy and parity.
Nextest runs 2,021 tests: 2,005 pass, 16 fail and four are skipped. Source fixes
retain the original behavior and strengthen fixtures for fixed-width multisig
timepoints, exact business-probe counting, typed storage batching, runtime UTF-8
decoding, all implemented command advertisement, distinct account spellings,
last-allocated nonce semantics and the valid subnet boundary 4095. Explicit
refusals for 4096/65535 are added to the native policy fixture. No assertion or
gate is disabled. The all-83 planning test passes; doctests and docs are not
reached. Bounded evidence is not retained. The new read/facade source
was not included in that run and has no compiled/tested claim.

Source through `ca1e9d5` implements all 83 intents and 40 of 102 reads,
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
at `070587f` passes MSRV, no-host, supply and semver. The check job compiles
the three corrected sites, then strict Clippy rejects a test MutexGuard whose
borrowed request remains in scope across client.close().await. A lexical
block now ends the guard and request borrows before that await, retaining the
RPC hash assertions. Features remains running at inspection. Tests, doctests
and docs builds were not reached. Another ordinary hosted check is required;
no local build was run.

Earlier normal hosted run
[38709461377](https://github.com/sam0x17/rtao/actions/runs/38709461377)
at `d63190b` fails check/MSRV compilation. Multisig reference collection needs
one named borrow lifetime; the new policy fixture compares NetUid to u16,
and the plan fixture assumes an unavailable H256 FromStr implementation.
Source corrections share the actual ArgMatches borrow, project subnet IDs
without changing order or duplicates, and strictly decode the fixed-width
public PoW hash. No assertion or gate is removed. No-host, supply and semver
pass; features remains running at inspection. Tests, doctests and docs builds
were not reached. The next ordinary hosted run must validate the corrections.
Bounded evidence is not retained. Local output remains
534 MB in target; no local build or scratch root.

Earlier normal hosted run
[38707066144](https://github.com/sam0x17/rtao/actions/runs/38707066144)
at `e0849d8` passes compilation, strict Clippy and all five other normal jobs.
Parity now qualifies the lock fixture, then rejects AddStake's helper-based
fixture for missing its explicit class token. Meaningful existing trade and
claim-aware fixture bindings now name their concrete intent types. A bounded
lexical source audit finds assertion-body references for all 83 classes; the
hosted Cargo/cfg/harness qualification remains authoritative. Tests, doctests
and docs builds were not reached. Bounded evidence is not retained.

The final ten ports, 20 M5 CLI routes, 21-constructor policy fixture, all-83
plan fixture and transaction reference have verified generated integration.
Run [38709002368](https://github.com/sam0x17/rtao/actions/runs/38709002368)
at `ca1e9d5` was verified and adopted unchanged: fourteen exact regular archive
members/digests, 23 dependency hashes, unchanged role inventories, ten pending/
availability changes, four guarded constructors, four bounded JSON bodies,
72 ordinary plan mappings, D-046's new fixture and D-047/D-048. Other generated
interfaces and matrix families remain unchanged. Counts are 83/83 intents and
40/102 reads. Combined hosted execution and acceptance remain pending.
See [m5-final-intents.md](m5-final-intents.md). No new filing or local build.

Earlier normal hosted run
[38705671132](https://github.com/sam0x17/rtao/actions/runs/38705671132)
at `ab61f13` passes compilation, strict Clippy and the five other normal jobs.
Parity rejects the lock-settings fixture because its dynamic dispatch does
not explicitly reference the three Rust classes required by the pending gate.
The fixture now matches MoveLock, SetPerpetualLock and SetRejectLockedAlpha
and asserts their decoded fields before checking bytes and previews. The gate
is unchanged. Tests, doctests and docs builds were not reached. Bounded failure
evidence is not retained.

Earlier normal hosted run
[38705303263](https://github.com/sam0x17/rtao/actions/runs/38705303263)
at `ba0a9d8` fails compilation in check and features because `SimSwapResult`
needs client `ReadValue` registration. The existing model already derives
typed decoding; the source registration is corrected. Supply, no-host and
semver pass; MSRV remains in progress. Tests, doctests and documentation builds
were not reached. Bounded logs and diagnostics are not
retained.

Earlier normal hosted run
[38703332560](https://github.com/sam0x17/rtao/actions/runs/38703332560)
at `8c9b4f3` passes supply, MSRV, features, no-host and semver. The check job
stops at strict Clippy: the leasing pass-through generator clones its Copy
`Percent` and `Option<u32>` arguments. The source generator now recognizes
these Copy types, including optional Copy values. Verified generated output
is adopted; tests and doctests were not reached on this checkpoint. The bounded
failure record is not retained.

Seven further ports have verified generated integration: Batch, SetIdentity, SwapHotkey, AddStake,
AddStakeLimit, SwapStake and MoveSwapStake. Batch uses a unanimous instance
signing role, bounded child dictionaries and full semantic preflight aggregation.
The SDK typed entry points refuse a role mismatch before key access; dynamic
and CLI paths use the selected instance role. Hosted execution remains pending.
Generation run [38704675475](https://github.com/sam0x17/rtao/actions/runs/38704675475)
at `358164e` was verified and adopted unchanged: fourteen exact archive members,
23 dependency hashes, unchanged role inventories, seven availability/pending
changes, one identity helper, two leasing Copy corrections, one guarded Batch
constructor and one normalized JSON body. The matrix changes only those seven
intent statuses and D-046's Batch fixture reference. Integrated coverage is
73/83 intents and 40/102 reads; no runtime coverage or milestone exit is claimed.
See [m5-batch-trades-identity.md](m5-batch-trades-identity.md).

Earlier normal hosted run
[38702708753](https://github.com/sam0x17/rtao/actions/runs/38702708753)
at `58f9605` passed supply, no-host and semver but failed check, MSRV and
features during compilation. The collateral read requires registering the
existing `MinerCollateralState` decoder as a client `ReadValue`. Root-claim
fixtures also used the wrong public helper path and an unused import. These
three source corrections are committed in `8c9b4f3`; tests and doctests were
not reached.

Earlier normal hosted run
[38701008514](https://github.com/sam0x17/rtao/actions/runs/38701008514)
on `62940f1` passed compilation, strict Clippy, parity and five other jobs.
Nextest ran 1,872 tests: 1,871 passed, one failed and four were skipped.
All eighteen earlier failures are resolved. The remaining new child-take
fixture expected the metadata default while the Python fake supplied a blanket
map value. The source fixture now seeds an explicit map miss so the actual
ValueQuery default is exercised, retaining the equality-refusal assertions.
Doctests were not reached. This run does not validate the later seventeen ports
or money JSON correction. The bounded failure inventory and log digest are
not retained.

Earlier normal hosted run
[38697413448](https://github.com/sam0x17/rtao/actions/runs/38697413448)
on `ed861a6` passed compilation, strict Clippy, the parity gates and the five
other normal jobs. Nextest ran 1,850 tests: 1,832 passed, 18 failed and four
were skipped. Doctests were not reached. The failures cover CLI schema/output
fixtures, wallet accounting fixtures, stale pending-operation assertions and
an unknown SS58 prefix in a coldkey fixture. Their source fixes and the next
thirteen-intent batch are committed; this run does not validate that batch.
The bounded log digest and exact failure inventory are not
retained.

Earlier normal hosted run
[38695489489](https://github.com/sam0x17/rtao/actions/runs/38695489489)
on `62e6b04` passed MSRV, features, supply, no-host and semver. Compilation and
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
were verified and adopted unchanged in `fd22af7` and `f723bcd`.
Review checked artifact digests, exact regular-file members, all 23 dependency
hashes, unchanged role package inventories, the two read-status changes and
D-044/D-045 regression references. Bounded adoption records are
not retained. Report refresh
[38692634551](https://github.com/sam0x17/rtao/actions/runs/38692634551)
was verified and adopted in `813c914`, covering the CLI owner feature and sha2
dependency; its proof is not retained.
Hosted generation
[38694967521](https://github.com/sam0x17/rtao/actions/runs/38694967521)
at `89035b0` was verified and adopted unchanged. Exact archive paths/digests,
all 23 dependency inputs, unchanged role inventories, twelve pending-body
removals, nine pass-through additions, the pre-serde guard and matching matrix
changes were checked; its proof is not retained.
Hosted generation
[38696143491](https://github.com/sam0x17/rtao/actions/runs/38696143491)
at `b458d29` was also verified and adopted unchanged. Its thirteen exact
members include only four availability/pending changes and three call-map
additions; all other generated interfaces and matrix families are unchanged.
All 23 dependency hashes include the updated CLI feature declaration, with
unchanged role package sets/counts and no runtime claims. Its bounded proof is
not retained.
Follow-up generation
[38696690648](https://github.com/sam0x17/rtao/actions/runs/38696690648)
at `6d43f69` corrects two unused zero-argument pass-through bindings. Review
verified the same thirteen paths and 23 hashes, exactly two parameter-name
changes, unchanged other interfaces/matrix and unchanged role inventories.
Its proof is not retained.
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
temporary build root under `target/`. Its measured
allocation before cleanup was 335,709,460 KiB (320.16 GiB). The task-owned Cargo
cache shared inside that temporary root and fifteen additional compiler target directories were removed.
Tools, copied caches, source clones, disposable fixtures and raw logs were
also removed after checking running processes/open files, launchd/config,
symlinks, virtual environments and Git worktree/alternate dependencies.
No task build, test or node process remains. No remote scratch was created.

Canonical authored source remains in this repository. Unfinished source and
172 patches were archived outside the repository (152,583,783 bytes).
The archive's 18,888 members were checked against the recovery selection.
Its SHA-256 is
`770d9d50a9c855e61a4e7476c91caef52b6d0161c297c996a9fdf46c0852969b`.
Preserving a candidate does not adopt or
validate it. See [disk-cleanup.md](disk-cleanup.md).

At the initial cleanup, the pre-existing `target/debug` was preserved and the
remaining target directory was 547,192 KiB (534 MiB). By the 2026-10-16
checkpoint, `target` is absent. Shared Cargo/toolchain caches, other
projects and all wallets/recovery state are untouched. No service
or task still depends on the deleted temporary root. Deleted file allocation is not a promise of an equal immediate
increase in physical free space.

## Historical execution record

The entries below describe the earlier implementation and validation work.
References to active sessions, live scratch caches, outstanding infrastructure
prerequisites or local validation requirements are historical and superseded
by Q30 and the cleanup above. Referenced raw scratch logs were discarded;
bounded repository evidence remains. Preserve the recorded test limitations.

## External prerequisites decision, 2026-10-14

M0-01 requested PLAN.md §12.13's prerequisites. Decision (PLAN.md Q23): testnet
TAO is available on request, the maintainer's GitHub namespace is used, only
free `ubuntu-latest` runners are used, GitHub Pages is not needed, and a
maintainer-held Ledger is available.

Use `sam0x17` for GitHub and `ghcr.io/sam0x17/rtao-oracle` for the oracle
namespace. All hosted jobs use free Linux x64 `ubuntu-latest`; no paid,
large, ARM-native or macOS hosted runners are required. Docs remain complete
and ship as downloadable rustdoc/book release assets without Pages.

Q29 waives the 24-hour testnet soak and its dedicated funding, subnet setup and
host prerequisites. No funding request, setup transaction or deployment is needed.
A maintainer-held Ledger is
available for manual device runs when the checklist is ready; actual run
results and model/app compatibility remain to verify.

Preserve full parity and all release targets. Linux builds need a fixed,
digest-pinned Ubuntu 22.04 container for the glibc 2.35 floor; ARM cross tooling
and resources must be validated and never described as native test coverage.
macOS builds/checks use a recorded temporary root. Keychain parity
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

The maintainer approved Q26's BLS build-graph criterion, Q27's exact
dormant lru exception and Q28's exact dormant ruzstd exception. The reviewed
Q28 patch is applied; activation, unknown-entry/advisory and changed-range
regressions are retained. The full supply job passes against the final
canonical locks: cargo-deny passes, both raw audits remain nonzero/visible,
and all 53 dormant graph checks pass. Current Rust and Rust 1.89 pass the
12 guard tests and the gate regression; fresh isolated Clippy passes.
Evidence: `supply-ruzstd.md`.
The complete gates still have to pass
before any commit or push. Initial oracle staging reports pending/M2-05 and
retains full verification once real artifacts exist. No rtao commit, push,
milestone tag or completed milestone is claimed. The active goal continues
through M9.

Native Q25 balances use checked u64 rao and distinct TAO/alpha units. Native
properties, input rejection, JSON regressions, Rust 1.89 and strict Clippy
pass; codec replay covers 2,899 samples. Signed authentication nonces are a
separate HTTP protocol domain. Raw evidence logs are not retained in the repository.

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
PRs and five issues, all open on 2026-10-14. An independent
accuracy and fix-verification audit of every filing was requested; that audit is active,
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
recorder or corpus ran locally. The raw incident record is
not retained in the repository; later checks
and the workspace default set `PYO3_NO_PYTHON=1`.

Completed work steps audited and removed their isolated fixture, filing and
rehearsal directories. Shared build, tool, upstream-source, advisory and fuzz
caches under the recorded execution root remain live dependencies.
Integration logs also use that root. Audit processes, launchd, configuration,
symlinks, virtual environments and Git dependencies before exact cleanup.

M2-22 pin automation is prepared: isolated candidate preparation/adoption,
force replay with unchanged authoritative snapshots and empty report, native
porting/auto-bless integration, read-only upstream-next, and main-image canary
smoke. Twenty-two pin/pipeline/normalization tests pass on current Rust and
Rust 1.89; nine original recorder replay tests and strict Clippy pass.
Workflow/actionlint and shell syntax checks pass. Main/testnet previews both
resolve the pinned v478 commit and fingerprint with no reported changes.
A local force run refuses before candidate/container execution and preserves
pins. Evidence is in `m2-pin-pipeline.md`; the raw validation log and the three
pin-plan/upstream-preview JSON files are not retained in the repository. Authenticated hosted
oracle/recording output, current-spec replay, e2e/canary/interop, and actual
force pipeline evidence remain pending; no milestone exit is claimed.
Task-owned test/tool directories were audited and removed. Shared build and
source caches remain live for integration. Q26 subsequently corrected the
BLS lock/build criterion; complete pin-force and hosted evidence remain pending.

## M3 preparation and final integration

M3 authentication/server, miner runtime, axon publication, miner metagraph sync
and persistence have targeted source/test evidence. The miner guide and the
validator/miner cargo-generate template are prepared. The complete native
template preflight passes: real generation, Rust 1.89 Clippy, both binaries,
integration-executable linking, consumer doctor, and audited project cleanup.
Its result explicitly records native_only=true and milestone_acceptance=false.
The complete book also renders with checksum-pinned mdbook 0.5.4. Generated
localnet/image coverage was unverified at that preparation checkpoint and is
optional under Q30. Applicable hosted client checks remain required. The local
security source review is recorded in `docs/security/m3.md`; the current
source review and hosted evidence are tracked separately above. No M3
completion checkbox or release is claimed.

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
differences in the old Unicode 16 dependency. The follow-up now pins category 0.6.0 and
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
and parity/dependency checks passed.
The stale build-weight report was remeasured from actual graphs and compatibility
outputs regenerated; repeated full gates passed those checks and remain live
at workspace test enumeration. Samples identify delayed
new-executable startup in macOS dyld before Rust main, also observed by isolated
workers. No host security setting or gate was weakened; no complete gates pass
is claimed. Raw evidence counts/hashes are not retained in the repository;
finished disposable builds were audited/removed.
Evidence: `m4-hyperparams.md`. No M4 completion or milestone exit is claimed.

The previous canonical full gate attempts caught three integration defects:
the authored Unicode notice followed the generated importer block; a private
btauth authentication error response triggered current Clippy's large-error
lint; and two staged calls used a method form rejected by the pinned upstream
lint. The repair moved the complete notice before the unchanged generated block,
boxed the private response, and called the existing checked integer helper
directly. The corrected importer and upstream lint pass, workspace Clippy
passed after the response repair, and all 14 current server tests pass.
Initial failure logs are not retained. The current full gate
run still must finish; those narrow successes do not establish complete gates.

The canonical worktree adopted M4-02 owner planning, M4-03/04 shield/raw/multisig submission,
M4-05 CPU PoW, all 11 M4-06 owner/registration intents and the 22 M4-07 reads
across 66 source paths. The composition includes the final multisig runtime-cap
checks, D-040's mutable start delay and the nested dynamic-decoder repair.
All baseline and adoption hashes matched; the existing Q29 and license/NOTICE
changes were preserved. Regeneration completed on the combined source,
reporting 20 implemented intents and 38 implemented reads. This counts source
behaviors, not authenticated oracle acceptance. Combined matrix, extraction,
regeneration checks, dependency measurement and affected current/MSRV tests
run as one combined local check. The matrix (1,533 rows), extraction,
regeneration, dependency measurement, dependency and parity checks and
formatting pass. The current-Rust affected run exits successfully with 810
native tests and three doctests; its one hosted oracle replay remains ignored.
Rust 1.89 also passes 810 native tests and three doctests with the same hosted
replay ignored. No complete-workspace gate success is inferred
from this subset. Details: `m4-integration.md`.
Owner orchestration/monitoring (M4-08) and CLI machinery (M4-09) are being
prepared in separate source snapshots while these checks and the feature
harness repair run. Live snapshots/proof remain until adoption and audited
cleanup. The finished 3.7 MiB composition rehearsal was removed after exact
dependency audits; the reversible source backup and active build cache remain
live. No M4 completion or milestone exit is claimed.

Read-only testnet readiness evidence and the concrete wallet/setup requirements
are recorded in `testnet-prerequisites.md`. Host and existing-versus-new subnet
information were requested; the maintainer selected a new subnet and a
shared remote macOS host, with low compute use because the host is shared.
Read-only SSH host inspection passed. Builds and PoW stay off that host;
the planned validator has two Tokio worker threads, lower scheduling priority,
resource measurements and bounded log retention. The reviewed native wallet
helper passed seven fixture tests, including 24 altered complete-state cases,
native encrypted recovery and no-replacement failures. Source,
binary/artifact hashes and dependencies were audited; the new local wallet was
then prepared and passed identical public-only reinspection. Its coldkey is the
maintainer-provided testnet coldkey. Wallet/recovery data are retained outside
the repository and excluded from scratch cleanup. No private material was printed.
No chain transaction, remote wallet copy or host service has run. Q29 now waives
the 24-hour testnet soak and its funding, chain setup and host prerequisites;
quote preparation and deployment are stopped. Existing unrelated wallets
are untouched. All automated validator, localnet, image, oracle and interoperability
checks remain required.

Task-created remote-host directories are cleaned up
when finished. Finished owned builds/fixtures are dependency-audited and removed;
active build roots, source snapshots and retained wallet/state are recorded live
dependencies. Remote host use so far is read-only SSH inspection. No remote scratch directory was created by the
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
remain. The complete gate rerun was then active;
That rerun passed 1,190 of 1,191 tests; all four RPC failures were resolved.
The remaining miner deadline exposed eager axon-publisher native-root loading
even with an explicit IP. The verified two-path lazy publisher repair is now
adopted; current/MSRV publisher tests and the unchanged miner test pass, with
strict Clippy on both. No-host separately passed before publisher adoption.
The next complete rerun terminated at
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
all-job rerun is live; no full gate
success or hosted acceptance is claimed.
Evidence: `tls-roots-repair.md`.

Read-only upstream tracking verified that Finney remains spec 478 with the pinned
code hash, final v478 release and published image. Testnet is spec 479; its
authenticated V15/V16 comparison is additive (one call, two errors, two events
and two storage entries). No final v479 tag/release/image or source association
was verified. Keep core/parity v478; record testnet metadata only through the
drift tool at a source-stable point. Raw evidence is not retained in the
repository; no runtime behavior range, transaction or public
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
only. Details: `m4-cli-preparation.md`.

The recorded eight PRs and five issues were freshly checked on GitHub; all
remain open and no new filing was made. Canonical gates remain live in nextest
enumeration, with source, extraction, pin/parity/dependency and strict workspace
Clippy checks already passed. No complete gate, commit/push, formal milestone
exit or release is claimed. The unanswered license-criterion decision remains
unchanged; all already approved decisions remain applied.

## Earlier source-artifact task, 2026-10-15

No temporary root remains. Hosted archives and logs are inspected in memory;
bounded adoption and failure records are not retained in the repository. No local builds or workspace copies are created.

The Q30 follow-up audit removes the stale macOS-local requirement and adds
hosted documentation coverage to the existing check job: strict rustdoc, a
rendered book, generated API/LLM index and downloadable development artifacts.
The docs command has a 15-minute limit, reuses the hosted target and requires
GitHub's Linux runner environment. Hosted execution was pending at that
checkpoint; run 38832708259 now verifies documentation generation.
