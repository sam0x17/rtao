# M0 foundation verification

Execution remains in M0. No milestone tag has been created and M1-M9 remain
required. Checkboxes are checked only with the commit completing each step.

| Requirement | Current evidence |
|---|---|
| M0-01 prerequisites and PR-U17 | User response recorded as Q23; subtensor#3242 open at the verified review SHA |
| M0-02 dual license | Committed license texts, NOTICE and attribution policy |
| M0-03 workspace | Virtual resolver-3 workspace, facade and xtask committed |
| M0-04 CI nextest profile | Profile committed; lane exclusions now declared with all four test targets |
| M0-05 stable and alias | Committed Rust 1.99.0 pin and cargo xtask alias |
| M0-06 pins | Command-generated v478 pins; finalized finney spec/code verified against release WASM and srtool; all five image digests recorded |
| M0-07 dependency pins | Root monorepo entries and BLS patch at v478; ring-only rustls and digit-exact serde_json settings |
| M0-08 skeletons | All 19 crates and four gated test targets exist; rtao-core build passes |
| M0-09 pin checker | Fingerprint and provenance tests pass; literal BLS lock check fails pending user decision |
| M0-10 import | All 360 exact copies/listing verified offline; no network cache needed for verification |
| M0-11 dependency checker | Eleven checker tests pass; actual raw-lock rule fails pending user decision |
| M0-12 extraction | Exact six btauth literals and source provenance generated; read-only check and seven extractor tests pass |
| M0-13 file parity | All 357 map rows covered and pending; offline gate, license and attribution checks pass |
| M0-14 curated core | Behavior, no-host and MSRV checks pass; raw-lock test remains failing |
| M0-15 Clippy bans | All ten bans present; seven portable-invalid host paths explicitly permitted |
| M0-16 dependency policy | Cargo-deny 0.20.2 passes actual graph; required CC0-1.0 and Unicode-DFS-2016 licenses added from upstream dependencies |
| M0-17 strict verification | Both schemes match pinned Substrate raw verification and reject wrapped signatures in strict mode |
| M0-18 core surface | Every direct curated re-export and ten typed guard paths compile; host-only imports and guards are feature gated |
| M0-19 staged guard | Three headers parsed; current strict-verifier symbol absent in portable and host probes; unexpected diagnostic tests reject failures unrelated to own missing symbols |
| M0-20 upstream lint | Pinned subtensor-linting runs only over staged source/tests; actual staged files pass |

Latest tooling validation: 59 xtask tests passed and cargo clippy -p xtask
--all-targets --locked -- -D warnings passed. These do not imply that the
workspace's raw-lock gates passed. [bls-lock.md](bls-lock.md) records the exact
contradiction and requested decision.

M0-21 through M0-29 still require primitives, JSON/oracle comparisons, fixture
replays, signer and btauth implementations, doctor, CI/local gates, docs, the
complete exit audit and release tag. There is no M0 exit claim.

The M0-21 signed balance decision is now explicitly approved (Q24).
Current focused tests pass for signed properties through ±2^80, exact JSON,
all 2,899 corpus samples, all six golden byte sections, signing and the
btauth vectors. Doctor now derives its pin from the owning downstream
workspace; its seven regressions and real consumer/repair-line checks pass
(`doctor.txt`). The complete supply job and all requested Rust 1.89 profiles
pass. The btauth parser's out-of-u64 error order is under review.

A subsequent independent guard review found gaps in external and inline
module discovery, remapped dependency diagnostics, generic symbol probes,
hosted vector classification and crate-wide lint coverage. Fixes and
regressions are being integrated; the earlier narrow guard evidence above is
not a final gate claim. M1 preparation runs independently while M0's raw BLS
requirement remains pending. The M0-21 through M0-28 implementations are now
present or integrating, but required complete gate and hosted results still
precede completion commits and M0-29.

Q25 superseded the earlier signed-balance checkpoint. The public balance
model now uses u64 rao throughout and checked arithmetic reports overflow or
underflow. D-035 records the user-directed difference from Python's arbitrary
signed local arithmetic and implicit decimal rounding. Native properties,
input/currency boundary tests and valid-chain JSON pass (50 all-feature tests,
including doctests, with 10,000 property cases); strict Clippy passes. These
results replace the signed properties as M0-21 acceptance evidence.
