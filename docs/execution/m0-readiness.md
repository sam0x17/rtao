# M0 source readiness, 2026-10-15

A source-only audit found the foundation steps and exit checks represented in
canonical code and the hosted gates. This records readiness for CI, not a
milestone exit. Full local builds, node/device rehearsals and release work
remain outside the active scope under Q30.

| Scope | Source and hosted coverage |
|---|---|
| M0-06 through M0-13 | Pins, lock/fingerprint checks, authenticated imports, dependency graphs, RustPython extraction and the 357-row file map |
| M0-14 through M0-20 | Core seam, BLS/provenance checks, Clippy policy, deny policy, staged probes, generated surface checks and upstream lints |
| M0-21 through M0-25 | Native balances, primitives, JSON regressions, six golden sections, corpus replay, signer and authentication tests |
| M0-26 through M0-28 | Doctor, hosted workflow, README and attribution rules |
| M0-29 and C1-C7 | Final hosted acceptance and milestone record remain pending |

The audit found a gap in exit 8: helper tests did not exercise the real doctor
command on the canonical workspace or its missing-patch failure. Two binary
fixtures now require canonical success and a nonzero missing-patch result with
the exact repair text. They run Cargo only offline and create tiny fixtures
under the existing test root, then clean them on drop.

The positive fixture forces caller color, exercising a second captured-tree
boundary affected by Cargo's colored repeated-node marker. Doctor now forces
plain output for its built-dependency inspection. It retains the same locked,
offline, full-workspace/all-feature/all-target graph and exact BLS policy.
CONTRIBUTING now includes the CLI PSF and core-extension Unicode obligations.

Installed rustfmt and whitespace checks pass. No local Rust test, build, Cargo
inspection, Python, scratch workspace or native system operation ran for this
audit. Execute the new fixtures and all applicable checks in hosted CI before
checking M0-26 or recording M0 acceptance. Earlier partial successes and source
coverage do not establish that result.
