# Q28: approved dormant ruzstd exception

The user approved Q28 on 2026-10-14 through the explicit asynchronous reply
"Allow guarded dormant exception". The reviewed
[supply-ruzstd.patch](supply-ruzstd.patch) is applied to the active guard.
Q27 continues to cover lru; Q28 adds only the exact ruzstd identity below.
The full active supply job passes against the parent's canonical locks,
including the pinned Unicode 0.6.0 dependency. Both raw audit failures remain
visible. No manifest or lockfile was changed to apply Q28.

The advisory describes a decompression bomb: a small compressed input can
cause several gigabytes of allocation and terminate the decoder process.
The fixed version is `>=0.9.1`. The [upstream investigation](ruzstd-research.md)
found no compatible fixed release: the selected Smoldot parent requires
`ruzstd ^0.8.0`, the latest 0.8 release remains 0.8.3, and published newer
Smoldot majors still retain that constraint. Upgrading to 0.9.1 cannot satisfy
it. The affected decoder is locked through Subxt's disabled light-client
dependency; all 53 actual supply graphs contain no ruzstd package.

The approved decision permits only this advisory on this exact dormant
lock entry, alongside the existing Q27 rule:

```toml
name = "ruzstd"
version = "0.8.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8"
```

The patch maps each exact package identity to its own advisory and fixed
version. It rejects another advisory on the same package, exchanged lru/ruzstd
advisory IDs, changed checksums or sources, older affected lock entries,
duplicate findings and changed patched/unaffected ranges. Every graph rejects
active ruzstd below 0.9.1 and active lru below 0.18.2. Coverage remains the
workspace and standalone fuzz workspace with all features, every platform,
normal/build/dev dependencies, and all shipped facade/neuron/CLI selections.

Both complete raw audits are retained before policy runs. The existing
unfiltered settings, schema/count checks, terminal/JSON agreement, tool-status
checks, lock snapshots and end-of-run lock comparison remain enforced. The
candidate retains the vulnerability in raw JSON and terminal output, including
the raw nonzero cargo-audit result. Cargo-deny and the exact existing 14
informational maintenance advisory/package pairs are unchanged. Every other
security advisory and every unknown maintenance finding remains blocking.

The candidate was tested as a separate crate under the recorded task root.
Its compile-time workspace path points to the real rtao workspace, so native
evaluation reads the real locks and runs the real 53 dependency inspections.
It uses its own workspace wrapper and test target. Strict Clippy uses a
separate lint target because cargo-clippy replaces the workspace wrapper.

The final active verification exits 0 for `cargo xtask gates --job supply`:
cargo-deny passes all advisory, ban, license and source checks, both raw
cargo-audit commands still exit 1 for each lock, and all 53 graph inspections
pass. Current Rust and Rust 1.89 each pass the 12 guard regressions and the
gate regression (plus two existing parity tests selected by the filter).
Strict all-target xtask Clippy passes in a fresh separate target, and owned
formatting passes. Final lock snapshots match the current canonical files.
This is the supply job's result; the complete gates remain required.

| Pre-approval check | Q27 guard before Q28 | Separate Q28 candidate |
| --- | --- | --- |
| Native guard result | Exit 1 on RUSTSEC-2026-0344 | Exit 0 after both exact dormant rules pass |
| Raw workspace/fuzz audits | Each exits 1 and reports ruzstd plus the lru unsound finding | Same unfiltered findings and nonzero raw status |
| Actual dependency inspections | All 53 pass | All 53 pass; both affected packages are absent |
| Current Rust and Rust 1.89 | Nine guard regressions and the gate regression pass | All 12 guard regressions pass, including three new ruzstd tests |
| Strict Clippy | All-target xtask check passes in its own lint target | All-target candidate check passes in its own lint target |

The new regressions exercise activation, unknown affected identities,
advisory/package association even when the exchanged advisory's range also
matches, patch-range changes, duplicate findings, filtering, and combined
raw lru/ruzstd failure status. The existing nine regressions continue to pass.
This evaluation establishes the proposed policy behavior; it does not patch
the vulnerable decoder or run an OOM reproducer. Full gates and hosted results
are not inferred from the isolated candidate.

Evidence paths are relative to
`/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC`:

- `ci/supply-24734`: final approved active guard against canonical locks,
  retaining both unfiltered nonzero audits and all 53 passing graph checks.
- `tmp/supply-q28-active-gate.log`, `tmp/supply-q28-active-gate.status`:
  the full supply job, including cargo-deny, exits 0.
- `tmp/supply-q28-active-current-tests.log`,
  `tmp/supply-q28-active-msrv-tests.log`: final active guard and gate regressions.
- `tmp/supply-q28-active-clippy.log`, `tmp/supply-q28-active-validation.json`:
  final separate-target Clippy and bounded source/lock/status proof.
- `ci/supply-32425`: final active guard, with both raw audits, lock snapshots
  and all 53 graph commands/statuses before Q28 approval.
- `ci/supply-92594`: separate candidate against the same real locks, with
  both raw audits, lock snapshots and all 53 graph commands/statuses.
- `tmp/supply-q28-current-tests.log`, `tmp/supply-q28-msrv-tests.log`:
  all 12 candidate regressions pass on both toolchains.
- `tmp/supply-q28-clippy.log`: strict candidate Clippy in a separate target.
- `tmp/supply-q28-live.log`, `tmp/supply-q28-live.status`: candidate native
  result, with raw vulnerability output still visible.
- `tmp/supply-isolated-clippy.log`: final strict active xtask Clippy in a
  separate target.
- `tmp/supply-q28-validation.json` and `tmp/supply-q28-candidate.lock`:
  bounded source/wrapper hashes, evaluation context, statuses and candidate
  dependency versions.

The patch SHA-256 is
`ad2b3fb04ff93ddd408dd89a21ea66abead93b55e3f7d48da98f2b45b9ae4a4c`.
Its active source baseline SHA-256 is
`17e09e630d788b475af4adec807e1299511435e5e4a50a514b133fd8808237cc`.
The source matched that baseline throughout the pre-approval evaluation.
After approval it matches the tested candidate SHA-256,
`36a211ea1f8d79d1141b408c7b63e61c4fe33284d76d7dea0ef80662e7042876`.

[PLAN.md execution rule 2](../../PLAN.md#executing-this-plan) says:
"Never skip, weaken or reinterpret an exit check; if one cannot pass as written,
record why in the progress log and ask the user." The user's best-effort
instruction prompted the compatible-update investigation and the completed
rustls fix. Extending the accepted raw-lock findings required the separate
Q28 criterion decision. That decision is now recorded in PLAN.md and the
architecture's dependency hygiene section. Full gates remain required before
any commit, push or milestone exit.

The separate candidate crate, its test/lint targets and the active guard's
isolated lint target were checked for running processes, open files, launchd
units, configuration references, inbound symlinks and virtual-environment
dependencies, then removed. `tmp/supply-q28-cleanup.json` records that audit.
The final applied-guard lint target was also dependency-audited and removed;
`tmp/supply-q28-active-cleanup.json` records its retained results and live caches.
The patch, complete raw evidence and bounded result files remain. Shared
build/source/tool caches and the parent's workspace wrapper remain live for
its ongoing integration.
