# M1-01 Rust-only Python table extraction

`cargo xtask extract` preserves the six M0 btauth goldens and generates settings,
RPC constants, and the complete named-table corpus from imported v478 sources.
No Python interpreter runs. The parser is the existing RustPython 0.4.0 fallback.

The generated outputs include `settings.json`, `rpc.json`, and `named_tables.json`
under `vendor/upstream/extracted/`, `rtao-catalog/src/py.rs`, and
`rtao-rpc/src/generated/py_rpc.rs`. The corpus contains 623 values from 35 source
modules, plus 20 settings, 17 RPC values, and the six btauth values. It includes
all 83 catalog intent classes, their defaults and class variables, and the two
multisig adapters. Every table carries a path, blob, tag, commit, and source span.
The Rust files carry Apache-2.0 attribution and their crates declare the compound
license. Generation uses the pinned Rust formatter through stdin and stdout.

The resolver supports constant folding, module and imported names, runtime
environment defaults with distinct empty-value rules, storage and API descriptor
chains, default factories, builtin type tags, and home-relative paths. Imports
are resolved only from authenticated vendored files. Dev sample addresses use
the four fixed dev URIs through the pinned Rust core. Constructor arguments,
source size, expression depth, containers, integer widths, and aggregate
allocations remain bounded.

`SetHyperparameter.wraps` is derived in Rust from `OWNER_HYPERPARAMETERS`, as the
architecture requires for derived tables. Its exact generator AST must match
`tuple(dict.fromkeys(...))`; insertion-order dedup preserves the shared alpha
setter. The output records the derivation and the referenced table's source
span/blob. Arbitrary comprehensions remain outside the evaluator.

Storage descriptors must have the expected literal `Item` payload and target.
Their targets are checked against authenticated Finney V15 metadata. The output
retains both `source_value_type_ident` and the metadata's `value_type_ident`.
One current label differs: Python's explicit `CollateralDrainRatio` descriptor
says `U64F64`, while the core renders its generic metadata type as `FixedU128`.
Both labels are retained for the later hyperparameter generator and fixed-type
mapping; extraction does not discard that distinction.

The RPC diagnostic limits are extracted from `_response_header` and
`_response_detail`, including the body slice. All `_safe_diagnostic` calls must
use the extracted metadata or detail bound. The generated aliases are
`POLICY_METADATA_LIMIT`, `POLICY_DETAIL_LIMIT`, and `POLICY_BODY_LIMIT`.

The regression `extract_check_covers_named_tables` re-extracts every named table,
checks the committed Rust/JSON bytes, and joins every catalog intent class to its
extracted operation. Additional regressions cover exact derived AST changes,
source descriptor payload changes, inconsistent inline diagnostic limits,
unhashable and oversized dictionary mutations, alias allocation growth, nested
and multiple-target mutable aliases, evaluated calls/defaults/decorators,
class-body mutations, and conditional import/function/with rebindings.

Client vocabulary generation adds `errors.json`, `config.json`, and `runtime.json`
under `vendor/upstream/extracted/`, with matching Rust catalog modules `errors`,
`config`, and `runtime_py`. The error generator authenticates the pinned catalog
artifact, checks all 18 semantic codes and 390 chain-name mappings against
Python, and emits the exact custom and standard invalid-transaction tables.
`ErrorCode` has snake-case serde tokens. The classifier preserves exact-name,
dispatch, rate-limit, and ordered text-fallback precedence. Its checked AST
supplies the rate-limit needles, validates each lookup and return, and records
the function span. Final lookup-table bindings and the enum imported by
`result.py` must match their source vocabulary, including after rebindings.

The config module provides all seven home-relative paths and getter environment
names, both cache TTLs, and the settable keys with builtin coercion types.
Each getter must retain `Path(os.getenv(name) or DEFAULT_PATH)`, including its
empty-value semantics. `BuiltinType` retains the extractor's four supported
tags (`Str`, `Bool`, `Int`, `Float`). The runtime module preserves both metadata
fetch strings, the exact cache-header bytes, a checked `u32` format version,
and a checked `usize` file limit. Regression tests reject catalog mismatches,
changed lookup returns or alias bindings, changed path-getter semantics, and
overflowing runtime wire constants. Catalog tests cover serialization, lookup
precedence, transaction names, and ratio-denominator equality.

Historical validation used the recorded `RTAO_TEMP_ROOT`, `CARGO_TARGET_DIR`,
and `TMPDIR`. Current substantial checks run in free `ubuntu-latest` CI under
Q30; the commands below do not require a local rerun:

- `cargo run --locked -p xtask -- extract --check`
- `cargo test --locked -p xtask extract`
- `cargo test --locked -p rtao-catalog`
- `cargo clippy --locked -p xtask -p rtao-catalog --all-targets -- -D warnings`
- `cargo check --locked -p rtao-catalog -p rtao-rpc`
- `cargo +1.89.0 check --locked -p xtask -p rtao-catalog`

The final extraction run passed all 22 selected generator tests and all four
catalog integration tests. Extraction check, strict Clippy, owned formatting,
and the Rust 1.89 check passed. A subsequent wider client/RPC check encountered
private `scale_decode::visitor::{Composite, Variant}` imports in the concurrently
edited `rtao-runtime/src/models/commitment_data.rs`; its owner received the
diagnostics. This failure does not affect the successful catalog and extraction
checks.

The new resolver tests use in-memory source fixtures. The existing M0 fixture
check removes its exact test directory after synchronous reads/writes finish.
The former execution build root is retired and absent; these historical checks
create no current cache dependency. Preserve shared global Cargo/source caches
and unrelated work. This task created no other disposable directories.
