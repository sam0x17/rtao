# Dormant light-client advisory

The maintainer approved Q27's guarded exception on 2026-10-14. It is implemented
in `cargo xtask supply` and the supply gate. The maintainer subsequently approved
Q28's separate exact dormant ruzstd exception; see `supply-ruzstd.md`. Its
reviewed patch is applied. The full supply job passes against the final
canonical workspace and fuzz locks, with both raw audit failures retained; the
final raw evidence is not retained in the repository.

Only [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253) may pass
for this exact lock entry, while it remains absent from every checked graph:

```toml
name = "lru"
version = "0.16.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7f66e8d5d03f609abc3a39e6f08e4164ebf1447a732906d39eb9b99b7919ef39"
```

Cargo locks `subxt-lightclient 0.51.1` through the optional `light-client`
feature of `subxt-rpcs`. It locks `smoldot-light 1.3.2`, which constrains `lru`
to `^0.16.3`. The required feature is disabled throughout the workspace.
The inspected registry contains no compatible fixed parent release:
Subxt's compatible parent remains 0.51.1, and published 1.x `smoldot-light`
releases use the affected `lru` version line. Version 2.0.2 uses `^0.18.4`,
outside the parent's permitted major version. The exception changes no
package source, affected package version, checksum or advisory database.

The guard records both workspace and standalone fuzz audits before applying
policy, including raw terminal and JSON output, stderr, exit status, exact
arguments and lock snapshots. Cargo-audit remains pinned to 0.22.2. Explicit
`--deny unsound` preserves a nonzero raw result for the current advisory's
`unsound` classification. The JSON settings must show no ignores, severity
threshold or target filtering. Schema errors, inconsistent findings or counts,
changed patch ranges, terminal/JSON disagreement and tool failures block.

Before accepting the exception, the guard inspects 53 dependency graphs with
`--target all --edges normal,build,dev`. These include the workspace and fuzz
workspace with all features, every declared facade, neuron and CLI feature,
default/no-feature/all-feature selections and shipped role combinations.
The image's `validator,prometheus` selection is included. Any active `lru`
below 0.18.2, unexpected affected lock identity or missing selected root blocks.
Lock snapshots must still match after inspection.

The exact 14 existing `unmaintained` advisory/package pairs remain visible and
informational, preserving the previous cargo-audit behavior. The list is fixed
in `xtask/src/supply.rs`; unknown maintenance findings block. Beyond the exact
Q27/Q28 dormant findings, every other
vulnerability, unsound warning, notice or yanked finding blocks. Cargo-deny's
existing advisory, license, source and ban checks remain the first blocking
supply step:

```sh
cargo deny --locked --all-features check
cargo xtask supply
```

The first complete fuzz audit also exposed four advisories on `rustls 0.23.45`:
RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348 and RUSTSEC-2026-0349.
The targeted fuzz lock update changes only that package's version to 0.23.46
and its registry checksum. All four findings disappear from the subsequent
unfiltered audit. Rust 1.89 resolves that version with locked/offline metadata
and successfully checks all eight fuzz binaries. The package's declared MSRV
is 1.71.

The evidence below was recorded under a local task root and is not retained in
the repository:

| Evidence | Result |
| --- | --- |
| Pre-update supply run | Complete audits and 53 graphs before the fuzz update; workspace reports one ruzstd vulnerability, fuzz reports that vulnerability plus four rustls vulnerabilities |
| Post-update supply run | Integrated guard after the update: 53 graph inspections pass; both raw audits report only the ruzstd vulnerability and retain the lru unsound finding |
| Workspace-isolated supply run | Final workspace-isolated active guard still exits 1 on ruzstd; all 53 dependency inspections pass and both raw audits remain unfiltered |
| Final supply run | After explicit Q28 approval, the full active supply job exits 0; cargo-deny passes, both raw audits still exit 1, and all 53 graph checks pass on final canonical locks |
| Current and MSRV test logs | Nine supply regressions and the gate regression pass on current Rust and Rust 1.89; the filter also runs two existing parity regressions |
| Clippy and formatting logs | Strict xtask Clippy for all targets in a separate lint target, and owned-file formatting pass |
| Fuzz MSRV log | Rust 1.89 checks all eight fuzz binaries after the update with workspace artifact isolation |
| Post-update fuzz audit | Additional unfiltered fuzz audit confirms the four rustls findings are cleared; ruzstd remains blocking |

The regressions cover activation, unknown affected entries, advisory and
identity changes, duplicate findings, filtering, malformed reports, changed
patch ranges and tool/status failures. The supply gate regression preserves
the cargo-deny command and pinned audit tool. Final tests and native runs use
a distinct workspace compiler wrapper. Clippy runs
in a separate disposable target because cargo-clippy overrides the workspace
wrapper with clippy-driver; earlier shared-target Clippy logs are historical
and do not establish the final lint result. Actionlint and YAML validation
pass for the PR workflow. Its six jobs use free `ubuntu-latest` runners, and
the supply job retains raw audit and graph artifacts for 14 days even after a
failure. That hosted upload and an image build have not been executed locally.

The original supply failure ended with this terminal summary:

```text
error: 1 vulnerability found!
warning: 15 allowed warnings found
```

Those warnings comprise 14 unmaintained notices and the lru unsound finding.
The separate BLS criterion is governed by Q26; see `bls-lock.md`. The
certificate-data license shipment is implemented and documented in
`release-data-license.md`, with exact source bytes retained at
`docs/licenses/webpki-root-certs-LICENSE`. The recorded ruzstd failures above
are pre-approval evidence. Q28 final active supply results are recorded in
`supply-ruzstd.md`; the complete gates remain required for any milestone exit.

The isolated harness and superseded parser rehearsal were checked for running
processes, open files, launchd units, configuration references, symlinks and
virtual-environment dependencies, then removed. The separate final lint target
and the final applied Q28 lint target were also audited and removed. Cleanup
records are not retained in the repository. The shared build/source/tool caches
and workspace wrapper remained live dependencies of the ongoing validation at
that time.
