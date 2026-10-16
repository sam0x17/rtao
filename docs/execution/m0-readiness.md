# M0 source readiness, 2026-10-16

The current source audit and authenticated hosted run 38873429391 at `d01830c`
accept the foundation implementations and named fixtures for client development.
All six jobs pass; 123 exact M0 PASS records bind the final audited inputs,
with 55 unchanged blobs and five reviewed workflow/plan/document deltas.
The combined checkpoint satisfies the M0 development exit and C1-C7 under Q30.
Full local builds, node/device rehearsals and releases remain outside Q30.

| Scope | Source and hosted coverage |
|---|---|
| M0-06 through M0-13 | Pins, lock/fingerprint checks, authenticated imports, dependency graphs, RustPython extraction and the 357-row file map |
| M0-14 through M0-20 | Core wrapper, BLS/provenance checks, Clippy policy, deny policy, staged probes, generated surface checks and upstream lints |
| M0-21 through M0-25 | Native balances, primitives, JSON regressions, six golden sections, corpus replay, signer and authentication tests |
| M0-26 through M0-28 | Doctor, hosted workflow, README and attribution rules |
| M0-29 and C1-C7 | Source/fixture evidence and the accepted combined checkpoint are recorded |

An earlier audit found a gap in exit 8: helper tests did not exercise the real doctor
command on the canonical workspace or its missing-patch failure. Two binary
fixtures now require canonical success and a nonzero missing-patch result with
the exact repair text. They run Cargo only offline and create tiny fixtures
under the existing test root, then clean them on drop.

The positive fixture forces caller color, exercising a second captured-tree
boundary affected by Cargo's colored repeated-node marker. Doctor now forces
plain output for its built-dependency inspection. It retains the same locked,
offline, full-workspace/all-feature/all-target graph and exact BLS policy.
CONTRIBUTING now includes the CLI PSF and core-extension Unicode obligations.

Both binary doctor fixtures now have exact PASS records at `d01830c`, including
the caller-color and missing-patch cases. The current completion record is in
[m0-acceptance-audit.md](m0-acceptance-audit.md). No local Rust test, build, Cargo,
Python, scratch workspace or native system operation ran for these audits.
