# Q31: approved dormant webpki-root-certs exception

The maintainer approved Q31's guarded dormant exception on 2026-10-21. It is
implemented in `cargo xtask supply`. Q27 continues to cover lru and Q28
ruzstd; Q31 adds only the exact webpki-root-certs identity below. No manifest,
lockfile, advisory database or cargo-deny setting was changed to apply Q31.
Both raw audit failures remain visible.

[RUSTSEC-2026-0383](https://rustsec.org/advisories/RUSTSEC-2026-0383) reports
that bundled Mozilla roots before 1.0.10 include `emSign ECC Root CA - G3`,
which can allow certificate paths through distrusted emSign intermediates
whose private keys are now well known. The advisory is a `crypto-failure`
vulnerability; its only patched range is `>=1.0.10`, with no unaffected
range. The workspace, fuzz and two-provider TLS fixture locks already carry
the fixed `webpki-root-certs 1.0.10`, which the active TLS stack uses.

The remaining affected entry is the semver-trick release 0.26.11. It is the
last published 0.26.x release, and no fixed 0.26 release exists. It ships no
certificate data of its own: it re-exports `webpki-root-certs` 1.x, which the
locks resolve to the fixed 1.0.10. Its only lock parent is
`rustls-platform-verifier 0.5.3`, the latest 0.5 release, which declares
`webpki-root-certs ^0.26` as a normal dependency only on `wasm32` and as a
dev-dependency only on FreeBSD. That verifier is locked through
`jsonrpsee-client-transport 0.26.1`'s optional
`tls-rustls-platform-verifier` feature, which still requires
`rustls-platform-verifier ^0.5` in its latest release. The jsonrpsee client
transport is locked through Subxt's RPC crate and is disabled throughout the
workspace.

The approved decision permits only this advisory on this exact dormant lock
entry, alongside the existing Q27 and Q28 rules:

```toml
name = "webpki-root-certs"
version = "0.26.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "75c7f0ef91146ebfb530314f5f1d24528d7f0767efbfd31dce919275413e393e"
```

The rule's fixed version is 1.0.10, so the fixed 1.x lock entry remains an
ordinary unaffected identity and may stay active. Every graph rejects active
webpki-root-certs below 1.0.10, including 0.26.11 and any earlier 1.x
release, along with active ruzstd below 0.9.1 and active lru below 0.18.2.
Coverage remains the workspace and standalone fuzz workspace with all
features, every platform, normal/build/dev dependencies, and all shipped
facade/neuron/CLI selections. Cargo-deny's resolved graph never includes
0.26.11, so its advisory, license, source and ban checks pass unchanged; its
license exception remains pinned to `=1.0.10`.

The guard rejects another advisory on the same package, the lru or ruzstd
advisory moved onto this identity, this advisory moved onto the ruzstd
identity or onto the fixed 1.0.10 entry, changed checksums or sources, older
affected lock entries, a duplicate lock identity, duplicate findings, an
informational reclassification and changed patched/unaffected ranges. Raw
audit filtering, ignores, severity thresholds and target filters still block.

Both complete raw audits are retained before policy runs. The existing
unfiltered settings, schema/count checks, terminal/JSON agreement, tool-status
checks, lock snapshots and end-of-run lock comparison remain enforced. The
raw JSON and terminal output retain the vulnerability and the nonzero
cargo-audit result. The exact existing informational maintenance
advisory/package pairs are unchanged. Every other security advisory and
every unknown maintenance finding remains blocking.

## Verification

Local development checks ran under a recorded local task root; hosted
`ubuntu-latest` CI remains the authoritative validation.

| Check | Before Q31 | With Q31 |
| --- | --- | --- |
| `cargo xtask supply` | Exit 1: blocking advisory RUSTSEC-2026-0383 on webpki-root-certs 0.26.11 | Exit 0: three exact dormant findings pass for each lock |
| Raw workspace/fuzz audits | Each exits 1 and reports ruzstd, webpki-root-certs and the lru unsound finding | Same unfiltered findings and nonzero raw status |
| Actual dependency inspections | All 60 pass; webpki-root-certs 0.26.11 and its 0.5 verifier parent are absent | All 60 pass; all three affected packages are absent |
| `cargo deny --locked --all-features check` | Passes | Passes unchanged |
| Supply guard regressions | 25 pass | 28 pass, including three new webpki-root-certs tests |
| Strict all-target xtask Clippy | | Passes with `-D warnings` |

The full xtask suite passes except the hosted-only core-patch Git fixture,
which refuses to run outside hosted CI by design. Formatting and
`cargo xtask deps-check` pass. The final lock snapshots match the canonical
files.

The new regressions exercise the combined raw lru/ruzstd/webpki-root-certs
failure status, the fixed 1.x entry staying active, activation of 0.26.x and
earlier 1.x releases, unknown affected identities, advisory/package
association even when an exchanged advisory's range also matches, the
advisory moving onto the fixed entry, patch-range changes, filtering,
informational reclassification, duplicate findings and a terminal report
missing the advisory. The existing guard regressions continue to pass. This
evaluation establishes the policy behavior; it does not change the dormant
crate or exercise certificate validation.

The following evidence was recorded under a local task root and is not
retained in the repository:

- The failing supply run before Q31, with both raw audits, lock snapshots and
  all 60 graph commands/statuses.
- The passing supply run with Q31, with the same retained evidence.
- Cargo-deny, deps-check, strict xtask Clippy and xtask test logs.

[PLAN.md execution rule 2](../../PLAN.md#executing-this-plan) forbids skipping,
weakening or reinterpreting an exit check; extending the accepted raw-lock
findings required the separate Q31 decision. That decision is recorded in
PLAN.md and the architecture's dependency hygiene section.
