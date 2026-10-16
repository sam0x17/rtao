# M2-14 parity machinery

## Current source and scope

Current client-source parity uses the explicit Q30 optional-oracle policy in
`xtask/src/oracle_readiness.rs`. A first absent corpus is `optional-unverified`
in ordinary and strict source checks, and the exact checked M2-14 source
machinery row is permitted. Existing or historically committed oracle outputs
still require authentication/replay; absent, partial, corrupt or removed
evidence cannot be relabeled verified. Strict M8 and source/e2e dispositions
remain separate requirements.

The current `parity/pending.toml` arrays for intents and reads are empty. The
74/87 inventories, pending exports/divergences and matrix totals below describe
the initial M2 snapshots; they are not the current source inventory. Full source
checks and report generation belong in bounded free `ubuntu-latest` CI.
M2-S02 remains open/unaccepted, so M2-26 and M2 exit remain open.

## Historical preparation and validation

`cargo xtask parity check` now runs file, license, symbol, catalog, extraction,
chain-source, call-coverage, environment, pending-operation, public-export,
divergence, e2e, oracle and generated-matrix checks. Each layer returns its own
result, and a failure prints the complete report. The required `pr-gates`
check job already calls this command; its checkout now retains history for the
shrink-only register's actual GitHub event baseline.

The catalog gate compares all 83 intent, 102 read, 18 semantic-error and 390
chain-error records. It rejects duplicate names and partial registries. Parsed
intent interfaces must retain field names, unknown-field rejection,
requiredness, exact extracted defaults, signer/origin types, descriptor wraps,
and complete normalized schema kinds and documentation. Read specs retain
parameter metadata, defaults, own-key rules, docs, categories, exact rendering
labels and complete chain-hit sets. Catalog hits may be a subset of complete H;
reviewed helper hits and stronger pinning require citations and reasons.

Python symbol fingerprints use parsed source and lexer tokens. Changes join
Rust items through parsed doc aliases, and unmatched names remain visible.
Rust test evidence follows Cargo targets and reachable source modules. Ignored
functions, disabled targets, unsupported attributes, orphan files, unknown or
platform conditions, and conditional module substitutions cannot qualify.
Cargo defaults and explicit declared feature contexts are evaluated; the
pending/e2e gate uses the required all-features Rust test context. This does not
claim a live localnet run. Ordinary divergence evidence uses Cargo defaults.

At the initial M2 checkpoint, the pending register contained 74 intents and 87 reads. The register's complement
must match actual tested behavior implementations; interface-only types and
`NotYetPorted` bodies do not qualify. The baseline comes from the GitHub event
base or explicit local revision. Pending growth requires a moved parity pin and
new catalog names. Public exports cover all 15 Python `__all__` lists, including
the authenticated root registry derivation. Missing exact aliases have explicit
pending review decisions in `exports_extra.toml`.

`divergences.toml` now contains all 35 Appendix C decisions. D-006 and D-035
retain their active tests and existing user decisions; D-011 is withdrawn.
The remaining 32 entries are explicitly pending. D-006 links filed upstream
PR-U5 #3248. The e2e map is authenticated against all 112 manifest names plus
three parsed Python test functions; it rejects vanished keys and invalid Rust
test evidence.

`cargo xtask parity matrix [--check]` writes or verifies the deterministic
`parity/matrix.json`: 1,529 rows across files, intents, reads, semantic errors,
chain errors, exports, divergences, environment, e2e and pending chain-assertion
families. Its status
semantics distinguish declared implementation from recorded oracle evidence;
recorded does not mean a successful replay. `parity check` detects stale output.
No file row has been blessed merely for having an interface or alias.

`parity bless <path>...` validates authenticated source, proposed file state and
attribution before recording an explicit reconciliation. The pin-bump API
`parity report --base <workspace> --out <json> [--autobless]` writes JSON and a
Markdown companion, changed symbols with Rust homes, added/removed intent/read
names, and pending deltas. Automatic reconciliation requires an already
reconciled non-vendored file, unchanged complete module tokens, unchanged
extracted behavior and verified unchanged oracle payloads. Missing proof fails
closed. Catalog normalization removes only source line/end-line/URL metadata;
schema and behavioral keys named URL, tag, commit, blob or provenance survive.
The report never silently marks pending behavior implemented.

A bounded same-input report rehearsal produced the retained
`m2-parity-report-smoke.json` and `.md`: no changed files, 74/87 pending,
unchanged extracted behavior and unavailable oracle proof. No rows were
reconciled. Its disposable baseline was audited for processes, open files,
launchd, configuration references, symlinks and virtual environments, then
removed from the recorded task root. That root is retired and absent; its
build/probe output is not a current dependency. Shared global Cargo Git
databases remain preserved.

Error extraction now supplies `py::MODULE_ERROR_DOCS` in declaration order,
including repeated names and empty documentation, and exact
`py::DISPATCH_ERROR_DOCS`. The named corpus contains 625 declarations from 37
modules. `error_descriptions.json` authenticates the assembly module and all 21
per-pallet description tables, validates the exact bounded aggregation form,
rejects duplicate leaf keys and overlaps, and proves all 390 assembled Python
strings equal the catalog descriptions. Each description retains source blob
and entry line spans; no Python interpreter ran.

Validation passed:

- 36 parity tests on current Rust and Rust 1.89.
- 49 extraction tests, including assembly, leaf-duplicate, changed-description,
  schema/registry/default, source-gating and safe-normalization mutations.
- Eight `rtao-catalog` integration tests.
- Canonical `cargo xtask extract --check`.
- Strict Clippy for xtask/catalog and owned-file formatting.

Rust 1.89 strict all-target Clippy also passed after simplifying the environment
signature guards and making the table-invalidating recursion an associated
function. The existing annotated `SetWeights` behavior test passed on Rust 1.89.

The shield consumer assertion has explicit pending evidence in
`chain_pending.toml`: `ShieldPeriod` belongs to M4-03. Runtime maximum extraction
and Python's default-bound inequality pass now. The marker must disappear when
the type/test lands, cannot survive a completed M4-03, and fails strict checking.
The final source-citation parser traverses Rust token groups, so assertion macro
identifiers qualify while literals do not. The existing `SetWeights` test now
names its helper-constructed value's type explicitly. Missing committed oracle
manifest/vectors were a gate finding at that historical checkpoint. Current
Q30 source parity uses the optional-unverified policy above.
A read-only GitHub query found no runs on main, so there is no hosted green run
claimed here. Literal BLS gates, labels and review requests remain untouched.

The checked layer totals include 13 covered pallets and 262 calls, 399 public
export names (222 pending decisions), and six mapped e2e tests with 108 pending
keys and one retired key. Matrix verification passed. The JSON report and
Markdown companion both preserve the same pending counts and catalog deltas.

Strict mode was also exercised and failed on pending file, environment,
operation, export, divergence and e2e state, as intended. Its bounded output is
retained in `m2-parity-strict.txt`.

During final integration, the sibling M3 immunity read landed. The refreshed
pending gate confirms nine tested intents and 16 tested reads, with 74/86 pending.
The complete final gate passes every layer except oracle, which fails because
the committed manifest/vectors are absent. Chain-source reports 77 selective
fields, 19 proxy variants, checked freeze hashes and the explicit M4-03 pending
consumer assertion. The SS58 input-carrier migration is fully regenerated;
the catalog accepts its exact JSON string contract and its requiredness
mutation remains effective. Canonical extraction and strict Rust 1.89
all-target Clippy passed on this integrated state. This is local implementation
evidence; hosted oracle and milestone exit evidence remain pending.

The old `parity-files` disposable directory was also audited and removed; its
753-byte verification result was already identical to retained
`docs/execution/parity-files.txt`. No live dependency was removed.

The initial oracle-readiness implementation had one bootstrap state. A missing first
manifest is reported as `status: pending`, milestone `M2-05`, only in nonstrict
mode with exactly one unchecked completion row for both M2-05 and M2-14. The
oracle directory could contain only the three reviewed generation inputs
(`lock.json`, `runtime-lock.json`, `samples_extra.json`). Any other file or
subdirectory, including partial or corrupt output, enters the unchanged full
`oracle::check` path and cannot be treated as pending.

The missing-manifest path reuses parity's actual GitHub event/local baseline
selection. The real baseline and HEAD are always inspected; explicit `--base`
supplements them. Complete Git history is required. Any oracle output recorded
in those histories, including output removed by a later commit, forbids initial
staging. Invalid or unavailable event baselines fail closed, including when an
explicit older base is supplied. The first-push all-zero SHA is accepted only
in the push `before` field at the full canonical 40-character width. Missing or
malformed PR and merge-group fields cannot fall back to a push baseline.

The ordinary `committed_vectors_replay_without_python` test uses the shared
readiness function, including its later Q30 optional-unverified policy. When real artifacts exist, it must run full authentication
and native replay before reporting checked. Standalone `xtask oracle --check`
remains unconditional and still fails on this checkout's absent manifest.
Every gate command and every other parity layer remains active; gate-command
preservation is checked for all six jobs. No BLS condition or workflow changed,
and no hosted-vector success is claimed by that source policy.

Nine readiness regressions pass on current Rust and Rust 1.89. They cover
completion and strict boundaries, corrupt or partial output, removed evidence,
actual event baselines and malformed fields, shallow history, merged-branch and
root-commit artifacts, and replacement refs or legacy grafts. History traversal
uses `--full-history --root` so Git's path simplification and `log.showRoot`
configuration cannot hide artifacts. Replacement refs and grafts are disabled
for commit resolution and traversal. All 18 oracle-filter tests,
including the ordinary shared test and command-preservation regression, and all
35 parity tests pass on Rust 1.89. Strict all-target Clippy for xtask/catalog and
owned-file formatting pass. The final nonstrict parity CLI exits zero with the
oracle pending and every other layer checked. Its regenerated matrix has 1,530
rows across ten families.

Fixtures contain only native Git repositories and invalid test output, use
disabled hooks/signing and
isolated Git configuration, and remove their exact task-root directories after
completion. No Python or real oracle vectors were produced.
