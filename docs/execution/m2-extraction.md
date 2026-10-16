# M2-03 and M2-04 extraction and catalogs

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 remains open and
unaccepted, so M2-26 and the M2 milestone exit remain open. The record below
preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`cargo xtask extract` now generates the complete M2 extraction and public
`rtao-catalog` surface from authenticated v478 inputs. No Python interpreter
runs. `extract --check` repeats extraction and compares every Rust and JSON
output byte for byte.

The public catalog contains all 83 intents, 102 reads, 18 semantic errors, and
390 chain errors. Intent and read records retain the full catalog JSON,
descriptions, wraps, signatures, source-derived defaults, and source provenance.
Chain records retain descriptions, pallets, documentation URLs, and complete
JSON. Missing source references remain `None`; an absent key is not replaced
with an empty array. All 18 semantic records include the exact Python
remediation and explanation.

`INTENTS` and `READS` are exported at the crate root with `IntentSpec`,
`ReadSpec`, and `ParamSpec`. `specs::intent` and `specs::read` look up stable
names. A missing constructor default has `default_json: None`; an explicit
Python `None` has `Some("null")`. Factory defaults retain a separate flag.
Requiredness comes from the constructor or the CLI's checked rules rather than
from a guessed JSON type. The 102 read registrations must exactly match the
pinned catalog, including dynamically generated scalar hyperparameter reads.
Hidden registrations, aliases, or changed registration helpers are rejected.
The catalog's sole read without a public expression, `metagraph`, inherits the
`subnets` namespace from its authenticated category peers.

`intent_fields.json` and `intent_classvars.json` separately preserve all
constructor fields and effective inherited class variables. Their extraction
checks catalog names, operation names, signer roles, origins, verification
reads, wraps, field sets, and required fields. A class-variable source span
points to the actual defining ancestor. The constructor distinguishes required
`field(metadata=...)`, explicit `None`, literal defaults, and a fresh factory.

The Python declaration index contains 624 values from 36 modules, including
the receipt event table added by M2-06. `py::PYTHON_TABLES` preserves exact
resolved values and source identities; `py::table(path, name)` retrieves one.
Typed exports additionally cover shield bounds, poll/retry counts, retry
needles, result regex patterns and watch-status names, fee warnings, help
overrides, remediations, explanations, and the runtime fee mirror. Both result
patterns require the source's `IGNORECASE` flag. The original 20 settings and
17 RPC values remain generated.

The runtime extractor parses Rust with `syn`. It checks custom transaction
codes and Python's explicit semantic-name projection, the authoritative
`call_filter_group!` list and routing function, and exact Python fee-filter
equality. `rt` exposes those code and call tables, `MAX_SHIELD_ERA_PERIOD`,
four checked production/fast rate-limit pairs, and typed tempo/activity/freeze
bounds. `chain_rules.json` also records the checked transaction-type rules.
The pallet's large `lib.rs` is the importer’s intentional extra input: its
exact destination and SHA256 must match the pin before its derived Git blob
is admitted to extraction.

`env_vars.json` scans all 194 authenticated package modules. It records 35
occurrences covering 28 exact names plus the computed `BT_PW_*` family, with
API, default, missing/empty behavior, truthy fallback, and byte/line spans.
Imports and aliases are followed without executing Python. Binding changes,
method scopes, conditional stores, helper defaults, keyword expressions,
comprehensions, match captures, and RustPython visitor helper gaps are checked.
The password family verifies path normalization, the native reader link, and
the nonempty guard. A changed computed read fails extraction. The inventory is
compared with `parity/env.toml` so new or vanished names need an explicit
consumer or divergence.

At the initial M2 snapshot, the environment map named 12 implemented consumers, 16 pending
milestone gaps, and the documented terminal-presentation omission for
`TERM_PROGRAM`. Pending rows do not claim implemented parity. The map gate
requires exact inventory coverage and rejects absent maps, unsupported or
ambiguous rows, missing reasons/evidence, and consumers that are not existing
public functions or methods in the referenced Rust source.

M2-07's requested pool RPC codes are parsed from the Polkadot SDK revision in
`Cargo.lock`. Extraction reads the immutable Git objects in Cargo's dependency
database, verifies their Git blob identities, and parses both `base::AUTHOR`
and the exact `From<Error>` conversion arms with `syn`. No mutable checkout
file supplies the constants. `rt::POOL_RPC_ALL_CODES` contains all 11 pool
variant codes; `rt::POOL_RPC_CODES` contains the four stable rejection names
used by the client, with the explicit `RejectedFutureTransaction` to `FutureTx`
projection. `pool_rpc_codes.json` records repository, commit, source blobs,
constant arithmetic, Rust variants, and projection. Cargo must have fetched its
locked dependencies before extraction; the Git database is a retained shared
source dependency, not a disposable task directory.

Historical validation used the shared recorded task root
`target/execute-plan.6Ro0PC`, its `build` target directory, and its `tmp`
directory. New generator fixtures are in memory. The existing btauth fixture
test removed its exact `extract/check-<pid>` directory after it finished; its
empty `extract` parent was also removed after process, open-file, launchd,
configuration, symlink, and virtual-environment checks found no dependency.
That task build/source root is retired and absent. Shared global Cargo/source
caches remain preserved, and current extraction runs belong in hosted CI.

The final checks passed:

- `cargo run --locked -p xtask -- extract --check`
- `cargo test --locked -p xtask extract -- --test-threads=2` (44 tests)
- `cargo test --locked -p rtao-catalog` (eight integration tests)
- `cargo clippy --locked -p xtask -p rtao-catalog --all-targets -- -D warnings`
- `cargo +1.89.0 check --locked -p xtask -p rtao-catalog`
- Owned generator, generated catalog, and integration-test formatting.

The new catalog tests compare every complete intent/read/chain record with the
pinned upstream catalogs, compare all typed fields, check the semantic-code
documentation and missing-source distinction, exercise default and own-key
semantics, and round-trip all 624 Python declaration values and source spans.
The home-path regression permits bounded `str(Path.home() / ...)` extraction
while rejecting further path division on a string and arbitrary `str` calls.
