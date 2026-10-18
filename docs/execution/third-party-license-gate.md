The parity license gate now requires the retained Unicode attribution for
`crates/rtao-core-ext/src/ops/python_printable.rs`. Only that table, at its exact
path and owned by `rtao-core-ext`, permits the additional `AND Unicode-DFS-2016`
clause. The original and upstream-derived license rules remain enforced, along
with the existing `rtao-primitives` CPython/PSF requirement.

The gate checks the table's pinned UCD 15 and CPython 3.12.11 provenance header,
its full inline notice, and the exact notice copies in
`crates/rtao-core-ext/LICENSE-UNICODE` and
`docs/licenses/Unicode-DFS-2016.txt`. Both standalone copies must have SHA256
`68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844`.
`NOTICE` must identify the package, UCD source and license, and contain the full
notice. Checked paths must be regular files; file and parent-directory symlinks
are rejected.

Six regression tests cover the admitted expression and preserved Apache base,
missing table, unrelated package claims, unsupported clauses, missing/truncated/
changed notices, removed provenance or inline copyright, symlinks, and the
retained PSF guard. These are attribution checks. The independent native Unicode
comparison verifies the table's character classifications separately.

All 19 `parity::tests` cases and strict all-target Clippy pass on current Rust
1.99 and Rust 1.89:

```text
cargo test --locked --offline -p xtask parity::tests:: -- --nocapture
cargo clippy --locked --offline -p xtask --all-targets -- -D warnings
cargo +1.89 test --locked --offline -p xtask parity::tests:: -- --nocapture
cargo +1.89 clippy --locked --offline -p xtask --all-targets -- -D warnings
```

All checks use the recorded build cache, wrapper and `PYO3_NO_PYTHON=1`.
Fixtures and bounded validation logs used a local task root and are not
retained in the repository. Both suites removed their fixture directories. The
empty fixture parent was audited and removed: no process, open file, launchd unit or checked configuration
referenced it, and it contained no symlinks or virtual environments. The shared
build cache and wrapper remained live for the wider plan execution.

PLAN.md M0 exit 9 currently allows only `MIT OR Apache-2.0` and
`(MIT OR Apache-2.0) AND Apache-2.0` for every package. That wording excludes the
existing required PSF clause and the new required Unicode clause. The license
gate can enforce those obligations, but the literal exit criterion still needs
a recorded resolution under the plan's execution rules. Suggested replacement:

> 9. `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` exist. Package metadata declares
> `MIT OR Apache-2.0` for original crates and
> `(MIT OR Apache-2.0) AND Apache-2.0` for crates containing upstream-derived
> files. The authenticated CPython datetime port in `rtao-primitives` additionally
> requires `AND PSF-2.0`; the pinned Unicode 15 printable table in `rtao-core-ext`
> additionally requires `AND Unicode-DFS-2016`. `xtask parity check` derives and
> verifies each exact expression and its required retained attribution. The
> `supply` job's license check passes (M0-02).

This preserves Q1's dual license and the Apache obligations for upstream-derived
material. It adds the required clauses for these two identified sources.

## Current resolution, 2026-10-15

The full plan audit corrected M0 exit 9 to the exact expressions above under
execution rule 5. This is a factual correction to the list of third-party
obligations already required by Q1 and present in the source and license gate.
It changes neither the original dual license nor the implemented policy.
Hosted CI must verify the current source; earlier local results are historical,
and their temporary build/log directories were removed in the Q30 cleanup.
