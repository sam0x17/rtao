# M4 combined source integration

The first section records historical integration before Q30. Its local drivers,
raw logs, snapshots and shared build caches were retired during cleanup. Do not
resume those commands or restore the old root. Current source/fixture/security
and documentation checks remain required in free `ubuntu-latest` CI; oracle,
localnet, interop and image execution are optional targeted hosted evidence,
and release work is deferred.

66 paths from eight reviewed patches were adopted on 2026-10-14. The exact
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

Exact patch identities, path inventories, rehearsal results, baseline hashes,
adoption hashes and a reversible source backup were recorded under the former
execution root; they are not retained in the repository.

The isolated current/MSRV tests and independent reviews prove each patch's
recorded scope. They do not prove the combined source or establish oracle,
localnet, interoperability, image or release execution. Source-backed
upstream findings remain local drafts until their exact claims are reviewed.

Canonical regeneration completed successfully. It cross-checks 311 calls, 402 storage entries,
136 constants, 124 API methods and 405 errors against authenticated source,
and reports 20 implemented intent behaviors and 38 implemented read behaviors.
The remaining 63 intent and 64 read behaviors remain pending. These are source
coverage counts, not an oracle acceptance result.

The historical combined script ran the
parity matrix generator, extraction/regeneration checks, target/profile
dependency remeasurement and compatibility output generation, dependency and
parity checks, formatting, and affected-package tests on current Rust and Rust
1.89. Each command recorded its log, timestamps and actual exit status.
`PYO3_NO_PYTHON=1` was set, and the then-current
compiler wrapper distinguished workspace artifacts from isolated reviews.
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
The validation records covered 393 source inputs and confirmed regeneration
changed none of the 66 adopted path hashes; completed command results were
derived from real terminal exit records. Those records, including every
terminal check, log hash, test total and the separately recorded
post-validation harness adoption, are not retained in the repository. No
full gate success is inferred.

The previous complete gate attempt passed 1,192 tests, doctests, no-host and
MSRV on earlier source before failing the temporary no-dev feature manifest.
Supply passed separately with the approved exact dormant advisory exceptions
and their activation guards. The isolated feature-harness repair and a fresh
complete `cargo xtask gates` on the combined source were still pending at that
checkpoint. Q30 now requires applicable hosted jobs, with no local rerun. The
initial isolated repaired features job passed its full matrix, but independent
review found that removing a dev dependency can hide a syntactically valid
forward to an undefined feature. The final candidate validates the original
locked graph before pruning; actual retained-only collision and independent
weak-forwarding compile cases cover the retained behavior. Its 14 gate
regressions pass on current Rust and Rust 1.89, strict Clippy passes on both,
and its full real features job exits successfully. Patch SHA-256 is
`7762d151298a6a922066a5db6225e914d6802df7e3147c8e21e220afe4fd735e`.
The source lock, commands, profile scope and retained-lock guard stay intact.

A post-validation driver verified successful terminal combined
Rust 1.89 validation, all 393 source hashes and every required command exit,
then adopted only the exact reviewed `xtask/src/gates.rs` repair. Before SHA-256
is `1c0e16c7d33a6b1a74c08d0c96f814a5ae48a75ce7efafeff7b72cd3b5c57c8d`;
after is `5c9430fb266a4062c259236d22907e17f15f8a1012ba3f9d1ff5c4dc3672d984`.
The reversible backup, actual apply checks and adoption record are not retained
in the repository. Complete canonical gates were then launched on the combined
source plus this repair. The driver was retired during Q30 cleanup; no full gate success is inferred from its launch.
No main
push, milestone exit or release follows from this adoption alone.

At that checkpoint, M4-08 owner orchestration/monitoring and M4-09 CLI
machinery were being prepared in isolated snapshots while combined checks and
the harness repair ran. Their source was outside that composition, and the
license-criterion decision was still pending. Q29 waived the testnet soak;
Q30 later retired the costly local work described here.

The finished composition rehearsal was removed after checking running
processes, open files, launchd, configuration, symlinks, virtual environments
and Git dependencies. Its cleanup audit recorded the exact inventory and
3,868,601 removed bytes. Q30 cleanup later removed
the remaining disposable snapshots, shared builds and raw logs after dependency
checks. Authored source and bounded repository/recovery evidence remain; no
validation depends on the retired caches. The durable prepared
wallet/recovery root is excluded from disposable cleanup. No remote task
directory, wallet copy or service was created.

## Client-only continuation, 2026-10-15

Q30 retires the local validation campaign described above. Source checkpoint
`3fec0bb553d62d39ef245c1fcd7bbe56dd31ab27` is pushed; its first hosted run is
https://github.com/sam0x17/rtao/actions/runs/38680556857. No current hosted
success or milestone exit is inferred from starting it.

Restored the M4-08 owner API and M4-17 owner example from the private source
recovery archive. Every patch matched the recovery manifest and applied to
the canonical checkpoint without a conflict:

| Patch | SHA-256 |
|---|---|
| M4-08 base | `a65d26f9a499232e538675464e492ef1c6ebc98382ee7616b4a9d13269b6d1ca` |
| Executor retry ownership | `b5a6e24a648d33c821b90d16ea0cdeca951255dbe4b8fd18187bf51da272b01f` |
| Final owner proxy routing | `5910aea3c02e1991f5cf34405234ff35ca764a493032c3be655d9ef6779f28c2` |
| Owner example | `d2b83a3bc1dbd6552ae8ecebd10e6ab113cb382e8102c88e563766a96b4ed1fd` |

The composition preserves the original signer for proxy routing, checks the
represented owner's state, validates live proxy grants before unlocking,
and gives owner orchestration control over its bounded paid retry. Null
pruning retains its recovery journal through ambiguous outcomes. The example
previews public plans and requires `--execute` before submitting.

Current formatting and patch/source checks pass. The recovered fixture tests
and example have not been compiled or executed during this continuation;
hosted CI must establish current acceptance. CLI/offline candidates remain
unadopted. No local build, node, container or device operation was started.

The owner feature manifest and example registration change the authenticated
compatibility report inputs. Refresh the derived build-weight/compatibility
records in hosted CI before claiming acceptance; do not edit their hashes
by hand. The source-only integration scratch was audited and removed after
adoption; the private recovery archive retains the original candidates.
