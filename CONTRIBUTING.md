# Contributing to rtao

Follow [PLAN.md](PLAN.md) and the [architecture](docs/plan/architecture.md).
Implementation follows the client development scope recorded in PLAN.md Q30.
Substantial validation runs in GitHub CI on free `ubuntu-latest` runners.

## Verification

Use the hosted `pr-gates` jobs for workspace tests, Clippy, MSRV, feature
coverage, supply checks and semver. A local full `cargo xtask gates` run is
not a pre-push requirement. Required hosted checks must pass before merging;
pending or unavailable CI remains explicitly unverified.

After dependency or feature manifests change, use the workflow's manual
`refresh_compat=true` mode to regenerate the existing compatibility graph
reports in hosted CI. Download and review its small source artifact, commit
the generated changes, then let normal checks validate that commit. This mode
runs no test matrix, writes no commit and builds no Mac target; the existing
Apple-target report is a dependency graph with its actual Linux host recorded.

Keep local work to source edits, formatting and small inspections. Do not
start cold workspace builds, extra toolchain builds, full nodes, testnets,
Docker services, Python oracles, soaks, fuzz campaigns or release rehearsals
on the laptop or Mac Studio. Use recorded fixtures and fake transports for
client behavior tests. Hardware and infrastructure acceptance are deferred.

Ledger development uses injected backends and existing proof fixtures in the
ordinary hosted checks. No emulator workflow is currently active. A future
targeted emulator dispatch must use free `ubuntu-latest`, pinned app/Speculos
inputs, an explicit timeout and disposable runner storage. It must not trigger
a local app build, device rehearsal or node. Unrun emulator/HID coverage stays
explicit in the signer guide and execution state.

If a focused local Rust check is useful, use one existing target directory
and at most two build jobs, disable incremental output and debug information,
and avoid concurrent Cargo commands. A cold or expanding build belongs in CI.
Keep temporary work beneath one recorded task root, preserve useful source
and bounded results, and remove disposable output when done. Never copy or
clone build caches. See [ci-gates.md](docs/execution/ci-gates.md).

The foundation's maintenance commands are:

| Command | Purpose |
|---|---|
| `cargo xtask pin --check` | Verify committed core/behavior pins and the lock; no pin changes |
| `cargo xtask pin --networks` | Read public genesis hashes twice and generate immutable client network inputs |
| `cargo xtask pin --networks --check` | Verify generated network inputs offline against the committed pins |
| `cargo xtask pin --parity <vN\|auto> --check` | Verify finalized release provenance and refresh image digests |
| `cargo xtask pin --parity <vN\|auto>` | Generate pins from a finalized release |
| `cargo xtask import-upstream` | Import pinned assets and source with hashes |
| `cargo xtask import-upstream --verify-only` | Verify all imported files offline |
| `cargo xtask extract [--check]` | Extract bounded Python literals with Rust, or check generated outputs |
| `cargo xtask regen [--check]` | Generate or verify descriptors and shapes from pinned V15/V16 metadata |
| `cargo xtask compat [--check] [--milestone M<n>]` | Generate or verify compatibility records; validated ranges require recorded evidence |
| `cargo xtask parity check [--strict] [--base <rev>]` | Check all file, symbol, catalog, extraction, behavior-evidence and completeness layers |
| `cargo xtask parity matrix [--check]` | Generate or verify the declared parity inventory |
| `cargo xtask parity report --base <workspace> --out <json> [--autobless]` | Compare a candidate with its baseline and write JSON/Markdown porting reports |
| `cargo xtask parity bless <path>...` | Record explicitly reviewed file reconciliation after source and attribution checks |
| `cargo xtask deps-check [--profile no-host]` | Check dependency layers and portable dependency graphs |
| `cargo xtask core-surface [--check]` | Generate or check compile probes for curated core imports |
| `cargo xtask staged --check` | Check staged module provenance and upstream obsolescence probes |
| `cargo xtask lint-upstream` | Run the pinned monorepo lints on `rtao-core-ext` |

`cargo xtask parity seed` creates the initial map only if none exists. It
refuses to replace reconciliation state. Never hand-edit generated pins,
vendored sources or generated code; use their owning commands.

## Reconciliation and blessing

`parity/map.toml` covers every watched Python file. `pending` means its port
and behavioral evidence have not been reconciled. It permits staged
milestone development and fails strict parity checking.

For each changed upstream file, read the old/new blob diff, update the Rust
implementation and its ported tests, and retain `// pyt:` source references.
Keep Python behavior unless a documented, tested divergence applies. Update
the map's Rust homes, reconciliation blob and tag only after the relevant
tests and parity checks pass. The exact blob is in
`vendor/upstream/tree.txt`. The file gate rejects stale blessed blobs,
missing homes and incorrect licenses.

Use `parity bless` after the behavior and relevant tests have been reviewed,
then regenerate `parity matrix`. The report's `--autobless` option applies only
to already reconciled files with unchanged symbols, extracted behavior and
verified oracle payloads; absent evidence leaves the row unresolved. Pending
operations, exports and future divergence tests remain explicit and fail
strict checking. Pin bumps follow
[the playbook](PLAN.md#103-a-pin-bump-step-by-step), including the CI-only
oracle and recorder work.

## Documentation examples

Edit `README.docify.md` when changing the README. Hosted docs generation uses
Docify 0.4.1 and checks that its output matches `README.md`. The validator, miner
and owner guides embed exported items from the same examples compiled by the
features job. These excerpts do not run neurons or require a local node.

## Attribution and licensing

Original files use `MIT OR Apache-2.0`. Every Rust file copied or derived from
upstream, including staged code, ports and generated extracted tables, starts
with this header (substitute the exact path and full commit):

```rust
// Derived from RaoFoundation/subtensor <path> @ <commit> (Apache-2.0); modified by rtao.
```

Keep existing copyright and attribution notices. Packages containing derived
files use `license = "(MIT OR Apache-2.0) AND Apache-2.0"` as their base
expression; other packages inherit `MIT OR Apache-2.0`. Retained third-party
material adds the obligations below. The parity gate verifies the exact
expressions against source headers and `parity/map.toml`.

The CPython ISO parser in `rtao-primitives` also retains `LICENSE-PSF`, its
source notice, and `AND PSF-2.0` in that package's license expression. The
CPython name-suggestion port in `rtao-cli` retains its own `LICENSE-PSF`,
source notice and `AND PSF-2.0` clause.

The pinned Unicode 15 printable table in `rtao-core-ext` retains
`LICENSE-UNICODE`, its source notice and `AND Unicode-DFS-2016` in that
package's license expression. The full Unicode notice is also retained in
`docs/licenses/Unicode-DFS-2016.txt` and NOTICE.

Vendored assets stay byte for byte, retain their notices and are listed in
NOTICE. LICENSE-APACHE supplies the upstream license text. Commit Cargo.lock;
checks use `--locked`.
