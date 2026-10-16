# M0 foundation verification

M0 is accepted for client development at `d01830c` by ordinary hosted
run 38873429391: all six jobs pass, with 3,051 tests passed, zero failed and
four skipped, plus all 19 workspace doctest suites. Independent source review
and 123 exact M0 PASS records cover the requirements below. The 60-input audit
retains 55 unchanged blobs and five reviewed workflow/plan/document deltas.
Q30 keeps substantial checks on free Ubuntu CI; releases, devices and local
infrastructure remain deferred.

| Requirement | Source and current acceptance evidence |
|---|---|
| M0-01 prerequisites and PR-U17 | Q23/Q29/Q30 record the current GitHub and resource decisions; subtensor#3242 retains its reviewed evidence |
| M0-02 license | Dual-license texts, NOTICE and package attribution expressions are committed |
| M0-03 workspace | Virtual resolver-3 workspace, facade and xtask are committed |
| M0-04 nextest | The `ci` profile excludes only the four optional infrastructure targets |
| M0-05 toolchain and alias | Rust 1.99.0 and the `cargo xtask` alias are committed |
| M0-06 pins | Command-generated v478 source, metadata, WASM and image identities are recorded |
| M0-07 dependency pins | Root monorepo/BLS pins, ring-only TLS and exact JSON settings are committed |
| M0-08 crates | All 19 crates and four gated infrastructure test targets exist |
| M0-09 pin checker | Source/provenance guards include the approved Q26 built-BLS criterion |
| M0-10 import | Hosted import guard verifies the current 362 copied artifacts |
| M0-11 dependencies | Dependency/layer checks enforce Q26 and the precise dormant supply exceptions |
| M0-12 extraction | Pinned literal extraction and source provenance are checked by hosted guards |
| M0-13 parity | File/symbol inventories, pending behaviors, licenses and attribution are mechanically checked |
| M0-14 core | Curated reexports retain no-host and feature guards |
| M0-15 Clippy | Forbidden core/host paths are declared; strict all-target checking runs in CI |
| M0-16 supply | Q27/Q28 allow only the recorded dormant findings; full raw audits and activation guards remain |
| M0-17 verification | Strict verification covers both algorithms and wrapped-message refusal |
| M0-18 surface | Typed core-surface probes cover the curated public boundary |
| M0-19 staged | Header, symbol, diagnostic and upstream-presence guards are committed |
| M0-20 upstream lint | Pinned linting covers the staged source and tests |
| M0-21 balances | Q25/D-035 use checked native u64 rao; ten adapted upstream properties cover arithmetic and unit boundaries |
| M0-22 JSON | Value-first domain parsing, exact large integers and structural comparator regressions are committed |
| M0-23 replays | Six golden byte sections and all 2,899 codec samples have ordinary hosted fixtures |
| M0-24 signers | Role/capability and signing fixtures are committed |
| M0-25 btauth | Vector and negative-case fixtures represent all 20 upstream authentication tests, now with per-test provenance |
| M0-26 doctor | Real-binary success and exact missing-patch diagnostics fixtures are committed |
| M0-27 CI | Six bounded free Ubuntu jobs cover check, no-host, MSRV, features, supply and semver |
| M0-28 docs | README, CONTRIBUTING and execution records describe the current source and Q30 limits |
| M0-29 completion record | Exact d01830c evidence, active step completion and the Q30 development exit are recorded |

The detailed source audit and accepted checkpoint are in
[m0-acceptance-audit.md](m0-acceptance-audit.md). Earlier focused local test
results are historical evidence only. The former signed arbitrary-size
balance model was superseded by Q25; the BLS decision was approved in Q26.
Neither is a pending user decision.

The current client checkpoint and exact hosted evidence are maintained in
[state.md](state.md) and the acceptance audit. Native Mac, oracle, interoperability
and release coverage remain unverified. M1 has its own read-path acceptance;
later milestones and full source/e2e parity remain separate work.
