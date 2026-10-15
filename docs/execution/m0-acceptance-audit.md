# M0 acceptance audit, 2026-10-15

Source inspection at `2331a32166520d6b7de49eea3316e79abaae056e` found the
applicable M0 fixtures and hosted checks represented. That snapshot failed
hosted compilation; its source repair still needs hosted acceptance. Q30 limits
acceptance to client development and bounded free `ubuntu-latest` CI. Local
campaigns, release tags, publication, hardware and infrastructure setup remain deferred.

## Source coverage

| M0 exit | Canonical source and hosted coverage |
|---|---|
| 1-2: six golden sections and corpus replay | `crates/rtao-testkit/tests/goldens.rs` and `crates/rtao-testkit/tests/corpus.rs`, included in the normal `check` job |
| 3: btauth vectors and negative cases | `crates/rtao-btauth/tests/conformance.rs`, included in `check`; all 20 upstream test names have Rust counterparts |
| 4: portable dependency graph | `xtask/src/deps_check.rs` and the `no-host` job |
| 5: one built core-pinned BLS | `crates/rtao-core/tests/lock.rs` and shared dependency policy enforce Q26's exact dormant-entry exception |
| 6: import, extraction, parity, surface, staged and upstream lint guards | All are ordered steps in `xtask/src/gates.rs::steps` for the `check` job |
| 7: arbitrary-precision JSON | `crates/rtao-primitives/tests/json.rs` covers Value-first float parsing, large-integer round trips and comparator regressions |
| 8: real doctor success and exact patch repair | `crates/rtao-cli/tests/entrypoint_contract.rs` exercises both through the compiled binary |
| 9: licenses and notices | Root license/NOTICE files, package expressions, parity attribution checks and hosted `supply` |
| 10: hosted checks and C1-C7 | `.github/workflows/pr.yml` and `xtask/src/gates.rs`; final acceptance remains pending |

The native-balance properties in `crates/rtao-primitives/src/money.rs` cover all
ten upstream property names, adapted to Q25's checked u64 domain, with `// pyt:`
references. Negative and oversized inputs, arithmetic boundaries and currency
checks are present. Python's signed arbitrary-size balance model is superseded
by Q25/D-035.

The btauth audit found missing per-test provenance required by PLAN.md §9.2.
This change adds `// pyt:` references to the 20 corresponding tests without
changing assertions. These annotations record source relationships; runtime
results remain pending. Each name was checked against the vendored
`sdk/python/tests/unit/test_http_auth.py` at v478 commit
`850f2755bb05a3851f948489f0ab27b0645cebce`. Its Git blob is
`0a2fdf4710f4c6a3868aad7f300414775e218d71`, matching `tree.txt` and the
extracted fixture provenance. The golden tests consume
`vendor/upstream/extracted/btauth.json`; extraction and import checks guard its
identity. These source relationships do not prove runtime conformance.

## Remaining acceptance work

Latest normal run
[38702708753](https://github.com/sam0x17/rtao/actions/runs/38702708753)
at `359b09e` passes supply, no-host and semver. Check, feature and MSRV jobs
fail compilation because `MinerCollateralState` lacks its client `ReadValue`
registration; the check job also identifies a test helper import path and an
unused root-claim test import. All three source corrections are prepared.
Nextest and doctests were not reached on this seventeen-port checkpoint.

Earlier normal run
[38701008514](https://github.com/sam0x17/rtao/actions/runs/38701008514)
at `b0e0aa1` passes compilation, strict Clippy, parity and the five other jobs.
Nextest ran 1,872 tests: 1,871 passed, one failed and four were skipped; all
eighteen earlier failures are corrected. The remaining child-take fixture
needs an explicit map miss to test the metadata default rather than the
imported Python fake's blanket storage value. That source repair and the
later seventeen ports need combined hosted acceptance. Doctests were not reached.

Earlier normal run
[38697413448](https://github.com/sam0x17/rtao/actions/runs/38697413448)
at `60ccce5` passes compilation, strict Clippy, parity and the five other jobs.
Nextest ran 1,850 tests: 1,832 passed, 18 failed and four were skipped. Doctests
were not reached. CLI schema/output, wallet fixture, pending-operation and
unknown-prefix failures require correction and another normal hosted run.
No acceptance checkbox is established by the passing subset.

Earlier normal run
[38695489489](https://github.com/sam0x17/rtao/actions/runs/38695489489)
at `fa77262` passes MSRV, features, supply, no-host and semver. Compilation
and strict Clippy pass. D-045's portable fixture now qualifies, but parity
uses the default-feature index to reject D-046's crowdloan tests even though
normal CI runs nextest and doctests with all features. The source correction
uses the indexed ordinary all-feature tests while retaining cfg, harness and
ignore rejection. Nextest and doctests were not reached. The coldkey and
generated fieldless-call corrections also require a successful combined run;
the earlier result cannot validate them.

1. C1 and exit 10 require a successful normal hosted run on the final pushed
   commit. Run [38690012949](https://github.com/sam0x17/rtao/actions/runs/38690012949)
   targets `2331a32`; all six normal jobs failed. The inspected MSRV log reports
   E0599 at `crates/rtao-reads/src/reads/owner.rs:98`: the wire `TaoBalance`
   has no `rao()` method. Commit `e79b6ac` repairs the source to use its native
   wire field. Normal run
   [38690242483](https://github.com/sam0x17/rtao/actions/runs/38690242483) at
   `3aaabc7` passed semver, MSRV and supply but failed strict CLI Clippy and
   the generated-read drift check. Both require correction before acceptance.
   A run cannot validate later source changes. The preceding `2e79b66` run
   failed during Clippy on an invalid PoW test constructor, fixed in `b322577`.
   Its nextest and doctest steps were not reached. Earlier run `38684286794` had 19 test
   failures and did not run doctests. No all-green result is carried forward.
2. C3 and M0-29 require the final hosted evidence and milestone record, then
   completion checkboxes for applicable steps. M0-06 through M0-29 remain
   unchecked. Their unchecked state alone does not show missing code.
3. C6 requires current status documentation. This audit identified stale BLS
   decisions and local/release requirements in `m0-foundations.md`, plus a
   superseded client checkpoint in `state.md`. Both are now reconciled with
   pushed source and pending hosted validation. Reconcile them again with
   the final accepted commit and actual hosted results at milestone exit.

C2 requires explicit limits: optional oracle, localnet, interop, native-platform
and device execution has not been established by this audit. The `ci` nextest
profile excludes only `e2e`, `interop`, `core_oracle` and `ledger_emu`; ordinary
M0 fixtures remain included. The `check` job runs workspace doctests after
nextest. Generated report or codegen artifacts prove the reviewed output and
source identities they record; they cannot substitute for these test results.

C4 retains PR-U17's existing reviewed evidence. Upstream
[subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242) remains
open at recorded head `eafe84e8a97376e7902118ebb204b9c53e3df55e`; its recorded
startup failures are not successful validation. This audit requires no new
filing. C5 needs current pin/generated identity checks on the accepted source;
an unadopted refresh artifact cannot satisfy that requirement. C7 defers release
and hardware work without asserting release readiness.

No build, test, Python process, scratch directory or GitHub write was used for
this audit. Only source comments and this bounded record were changed.
