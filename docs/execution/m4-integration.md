# M4 combined source integration

Root adopted 66 paths from eight reviewed patches on 2026-10-14. The exact
composition was rehearsed against the current source before canonical writes.
All before and after hashes matched, and `git diff --check` passed. The parity
matrix was deferred to the real generator rather than copied from a snapshot.

The patches add the owner hyperparameter planner (M4-02), shield/raw submission
and multisig (M4-03/04), CPU PoW (M4-05), all 11 owner/registration behaviors
(M4-06), and 22 owner reads (M4-07). The final composition also includes the
multisig runtime-cap supplement, D-040's mutable `StartCallDelay` and saturating
u64 start-block addition, and the repair preserving empty dynamic composites
inside containers. Decoder leaf registration retains both multisig types and
adds the three owner-read types exactly once. D-039 and D-040 coexist in the
divergence register. Existing Q29 and license/NOTICE changes were preserved.

Exact patch identities, path inventories, rehearsal results, baseline hashes
and adoption hashes remain under the recorded execution root:

- `root-m4-integration/rehearsal.json`
- `root-m4-integration/composed-source.json`
- `root-m4-integration/canonical-adoption.json`
- `root-m4-integration/canonical-before/` (reversible source backup)

The isolated current/MSRV tests and independent reviews prove each patch's
recorded scope. They do not prove the combined source or replace hosted
oracle, localnet, interoperability, image or release acceptance. Source-backed
upstream findings remain local drafts until their exact claims are reviewed.

Canonical regeneration completed successfully in
`tmp/root-m4-combined-regen.log`. It cross-checks 311 calls, 402 storage entries,
136 constants, 124 API methods and 405 errors against authenticated source,
and reports 20 implemented intent behaviors and 38 implemented read behaviors.
The remaining 63 intent and 64 read behaviors remain pending. These are source
coverage counts, not an oracle acceptance result.

The combined check script is `tmp/root-m4-combined-checks.sh`. It runs the real
parity matrix generator, extraction/regeneration checks, target/profile
dependency remeasurement and compatibility output generation, dependency and
parity checks, formatting, and affected-package tests on current Rust and Rust
1.89. Each command retains its log, timestamps and actual exit status under
`tmp/root-m4-combined-*.log`. `PYO3_NO_PYTHON=1` remains set, and the canonical
compiler wrapper distinguishes workspace artifacts from isolated reviews.
The matrix generator passed with 1,533 rows. Extraction, regeneration,
target/profile dependency measurement, dependency and parity checks, and
formatting passed on the combined source. Measured package counts remain
225 core-no-host, 361 validator, 365 miner and 416 owner; only owner activates
core/host. The new report records the changed manifest/lock inputs even though
the package counts are unchanged. The current-Rust affected-package run exited
successfully: 810 native tests and three doctests pass, with the existing one
hosted oracle replay ignored. Rust 1.89 also exits successfully with the same
810 native tests, three doctests and one hosted replay ignored. These are
the seven affected packages, not a new complete-workspace gate result.
`root-m4-integration/validation-start.json` records 393 source
inputs and confirms regeneration changed none of the 66 adopted path hashes;
`validation-progress.json` derives completed command results from real terminal
exit records. `validation-complete.json` retains every terminal check, log hash,
test total and the separately recorded post-validation harness adoption. No
full gate success is inferred.

The previous complete gate attempt passed 1,192 tests, doctests, no-host and
MSRV on earlier source before failing the temporary no-dev feature manifest.
Supply passed separately with the approved exact dormant advisory exceptions
and their activation guards. The isolated feature-harness repair and a fresh
complete `cargo xtask gates` on the combined source remain necessary. The
initial isolated repaired features job passed its full matrix, but independent
review found that removing a dev dependency can hide a syntactically valid
forward to an undefined feature. The final candidate validates the original
locked graph before pruning; actual retained-only collision and independent
weak-forwarding compile cases cover the retained behavior. Its 14 gate
regressions pass on current Rust and Rust 1.89, strict Clippy passes on both,
and its full real features job exits successfully. Patch SHA-256 is
`7762d151298a6a922066a5db6225e914d6802df7e3147c8e21e220afe4fd735e`.
The source lock, commands, profile scope and retained-lock guard stay intact.

`tmp/root-after-m4-feature-gates.sh` verified successful terminal combined
Rust 1.89 validation, all 393 source hashes and every required command exit,
then adopted only the exact reviewed `xtask/src/gates.rs` repair. Before SHA-256
is `1c0e16c7d33a6b1a74c08d0c96f814a5ae48a75ce7efafeff7b72cd3b5c57c8d`;
after is `5c9430fb266a4062c259236d22907e17f15f8a1012ba3f9d1ff5c4dc3672d984`.
The reversible backup, actual apply checks and adoption record are in
`root-feature-harness-adoption/`. Complete canonical gates are now running on
the combined source plus this repair, retaining
`tmp/root-m4-feature-repaired-gates.log` and its eventual actual terminal exit.
No full gate success is inferred from launching this driver.
No main
push, milestone exit or release follows from this adoption alone.

M4-08 owner orchestration/monitoring and M4-09 CLI machinery are being prepared
in isolated snapshots while combined checks and the harness repair run. Their
source is not part of the adopted composition. The pending license-criterion
decision remains unchanged. Q29 waives only the 24-hour testnet soak.

The finished composition rehearsal was removed after checking running
processes, open files, launchd, configuration, symlinks, virtual environments
and Git dependencies. `root-m4-integration/trial-cleanup-audit.json` retains
its exact inventory and the 3,868,601 removed bytes. Live source/proof, the
reversible backup and shared build caches remain dependencies until validation
and integration finish. Finished owned builds must undergo exact dependency
audits before removal. The durable prepared
wallet/recovery root is excluded from disposable cleanup. No remote Studio
task directory, wallet copy or service was created.
