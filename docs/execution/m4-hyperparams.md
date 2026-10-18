# M4-01 hyperparameter preparation

This record describes historical preparation before Q30. Source, dependency,
fixture, security and documentation checks remain required in hosted CI.
Fresh Python/oracle execution is optional targeted hosted evidence. Former
local drivers, snapshots and build caches are retired.

The canonical worktree now contains the reviewed hyperparameter table,
conversion implementation and hosted oracle recipe. M4-01 is not marked
complete by this record: its original canonical checks and hosted differential
execution had not established a complete exit. Q30 no longer makes a fresh
differential run a development prerequisite.

`rtao_primitives::hyperparams::HyperparamTable` implements all ten methods:
`metadata_kind`, `kind_of`, `doc_of`, `short_of`, `normalized`, `annotate`,
`value_forms`, `codec_of`, `raw_bounds` and `to_raw`. The catalog's generated
`TABLE` has 48 descriptions and 42 storage entries, authenticated through
extraction and pinned runtime metadata. The intents namespace delegates to
that table. M4-06's `SetHyperparameter` intent and the M4-02 planner were
subsequently adopted; their combined validation is recorded in
`m4-integration.md`.

Metadata selects unit kind first. Explicit width/signedness selects the codec,
then metadata, then the kind default. The catalog retains both the source
`U64F64` identity and metadata `FixedU128` identity for collateral drain ratio.
The units checks require nonempty short descriptions, a maximum of 38 Unicode
codepoints, and agreement between metadata and hand-declared unit kinds.

Raw results use checked native u128/i128 hyperparameter integers. TAO amounts
retain Q25's u64 bound. Human conversion preserves binary64 multiplication
and half-even rounding. Exact integer division and four-significant-digit
annotation share the existing formatter without changing its default
six-digit behavior. Boolean conversion preserves the pinned direct path.

Independent review found an accepted custom fixed-point scale whose triple
could overflow u128 while producing its input hint. Both new regressions
reproduced the original panic on current Rust and MSRV. A checked comparison
now uses the generic hint when no native upper bound can equal the
mathematical triple. The tests also reject a false hint caused by wrapping
and distinguish the exact representable boundary from the next scale.
Invalid caller-constructed widths or zero scales still bypass the generator's
invariants; this change does not provide general custom-table validation.

The exact 14-path author patch was adopted, SHA-256
`f50251c0643228fb1c45a87eb9fcdd537b53eb3530b66761675811b1091a432f`,
then the two-path scale repair, SHA-256
`5f48121085549822a2d126823c4b3773be845cffc0c80d7ae8dc05add4507897`.
All canonical before hashes, patch application checks and final after hashes
matched. Bounded manifests were recorded under the former temporary root;
only preserved repository/recovery evidence is available after Q30 cleanup.

The author validated all affected packages on Rust 1.99 and 1.89, plus generator
mutation tests, extraction, regeneration, parity and strict Clippy. The repaired
source passes 151 affected-package tests on each toolchain, with zero failures
and one existing hosted-oracle replay ignored. The original five generator and
51 extraction tests pass on both compilers. Disposable author/repair targets
were dependency-audited and removed. Q30 cleanup later retired the remaining
review snapshot and shared builds; neither is a live validation dependency.

The separately reviewed four-path oracle patch, SHA-256
`4bc159adf62db3c3862d24ac79af73edf9007f770192b02ee269e86a566db473`,
is also adopted with exact before/after hash checks. The existing hosted
`dump_pure.py` captures all ten methods for 48 names, one unknown name and seven
precedence fixtures. Each name has 129 typed input cases, totaling 7,224.
The Rust replay requires the exact input inventory, source/storage identities,
integer/error JSON and binary64 bits before comparing the captured outputs.
The existing M2/all intent/read routes are preserved.

All 21 oracle tests and strict all-target/all-feature xtask Clippy pass on both
compilers in the oracle author's isolated validation. Workspace formatting passes.
Rust parses the scripts and checks image inputs; an actual local image attempt
was rejected before Docker. These native tests produce input recipes only.
No Python, Docker or hosted expected-output corpus was run locally. The actual
hosted image build, dump, native differential replay and authenticated artifact
adoption were unverified at that checkpoint and remain optional targeted hosted
coverage under Q30. The oracle's disposable builds were audited/removed; Q30
cleanup retired the remaining review snapshot and raw logs.

The historical canonical integration driver used `PYO3_NO_PYTHON=1` and the then-current compiler wrapper. It ran
affected-package tests on both toolchains, final btauth and staged
receipt/Unicode regressions, extraction, regeneration, parity,
dependency checks and the full `gates` command. Both canonical affected-package
suites passed 151 tests, with zero failures and one hosted replay ignored.
The boxed-response server regressions passed 14 on MSRV after the current
14-test pass; receipt/Unicode suites passed eight on each toolchain.
Extraction, regeneration, parity and dependency checks passed. The raw counts
and log hashes are not retained in the repository.

The full gates correctly rejected the stale dependency-count report. The
maintainer remeasured actual normal/build graphs through `compat --measure-build-weight`
and regenerated compatibility outputs. Counts are 225 core-no-host, 361
validator, 365 miner and 416 owner packages on aarch64-apple-darwin; only owner
activates core/host. Subsequent native-root repairs allowed all 1,192 workspace
tests, doctests, no-host and MSRV checks to pass; the full run then failed in
the no-dev feature harness. That run predates the latest owner/shield/read
adoption. Its evidence is in `tls-roots-repair.md`; the combined M4 source
requires fresh dependency measurement and validation (`m4-integration.md`).
No complete gate success or milestone exit is inferred. Former raw log paths
do not authorize restoration. Current source/pin/dependency guards and combined validation run in hosted CI.
