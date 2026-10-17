# M2 read interfaces and behavior

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

The read generator now emits all 102 `ReadName` variants, typed parameter
records, signature defaults, namespace methods and exhaustive dynamic dispatch.
`ReadName::spec()` exposes category, argument documentation, requiredness,
wallet-key fallback metadata, documentation URL, rendering, pinning and the
complete descriptor hit set. `catalog_spec()` retains authenticated signature
and source provenance. At the initial M2 snapshot, 87 pending reads returned
`NotYetPorted(ReadName)` after validation without I/O. The current pending
intent/read arrays are empty; those initial counts are historical. The raw metagraph read stays a free
function and dispatcher entry; generation preserves the curated metagraph
extension method.

The 13 M2 behaviors are implemented. Scalar storage bodies are generated from
reviewed descriptor mappings. `epoch_status` pins once, coalesces its five
storage values, and uses that snapshot for the next-epoch API and Aura constant.
Timestamp output keeps Python's float token and UTC ISO text, including its
microsecond rounding after converting milliseconds to binary64. The raw
`max_weight_limit` remains a u16 integer, as the pinned scalar factory returns.
Weight rows retain sparse and zero rows, include duplicate targets in their
denominator, replace repeated targets with their last value, and sort validators.
LastUpdate differences remain signed; the dispatcher preserves arbitrary signed
Python list indices, including null results for enormous positive indices.
Typed namespace calls still take a native `Uid`. These local indices and block
differences are independent of the user's native checked-u64 balance decision.
Block duration follows Python's float `1000.0` divisor, while timestamp keeps
its separate integer-division and microsecond conversion behavior.

`xtask extract` emits `read_hits.json` from authenticated module ASTs. It follows
module-local helper calls transitively and resolves generated descriptor chains
and module-level `Item`, `Method` and `Constant` literals. It also extracts render
hints and detects pinned reads and map/batch scans. Reviewed out-of-module view
and commitment accesses live in `parity/read_hits_extra.toml`. Generation asserts
that every catalog hit belongs to the combined set H. `RecordingView` attaches a
shared client observer to pinned snapshots and their clones, recording storage,
map scans, APIs and metadata constants. Empty scans and raw key reads are covered.

The default read suite passed 25 tests: eleven M2 integration tests, ten existing
unit tests and four existing integration tests. The M2 tests cover all catalog
names and defaults, all 87 pending failures without I/O, the 13 outputs, epoch
and timestamp boundaries, missing and negative LastUpdate indices, sparse and
duplicate weights, and native identifier limits. All 15 ported catalog reads
run at two block identities; each run touches a subset of H, the sample union
equals H, and every state request uses its selected hash. Observer tests also
cover snapshots, clones, constants, raw keys and empty maps.

Canonical `xtask extract --check` and `regen --reads-only --check` pass, as do
the two focused helper/literal/render extraction tests. Strict Clippy passes for
all read and testkit targets. Rust 1.89 passes the eleven final M2 tests; the
earlier full read suite also passed on that compiler. The testkit Docker library
test target compiles. An isolated audit found the enormous-index and float
duration differences and two imported stake-valuation helper hits; all four
findings were fixed and checked. No Python ran locally. Hosted oracle vectors
and their Rust replay are optional targeted hosted evidence under Q30;
M2-14 source machinery does not assert independent Python behavior. These Rust
checks do not claim oracle or milestone acceptance.

Historical Cargo invocations used the recorded task root
`target/execute-plan.6Ro0PC`, its `build` target and `tmp` directory. A bounded
read-generator bootstrap under that root used the owned generator and extractor
sources while the normal xtask dependency graph was being integrated. After the
canonical checks passed, its process, launchd, configuration, symlink and virtual
environment dependencies were audited. The bootstrap directory and temporary
source copy were removed. That root is retired and absent; no current build
dependency remains there. Shared global Cargo/toolchain caches are preserved. No
wallet, key, private dataset or validator state was created or removed.
