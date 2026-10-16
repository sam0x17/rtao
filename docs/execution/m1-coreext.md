# M1-02 portable metadata and protocol extensions

The staged crate now provides `ir::MetadataIr`,
`ops::{metadata, storage, runtime_api}`, and `value_ext::ValueExt`, while
preserving `verify_strict`. These helpers do no network, filesystem, or clock
I/O. All chain encoding and decoding goes through the pinned core.

## API and attribution

`ir` is derived from
`sdk/bittensor-core-py/src/runtime.rs::metadata_ir` at subtensor v478,
commit `850f2755bb05a3851f948489f0ab27b0645cebce`, Apache-2.0.
It retains the original Python IR fields and their shapes: pallet names and
indices; calls, ordered argument names and identities, and joined docs;
indexed errors; storage names and value identities; constant-name lists;
and API method-name lists. Added fields expose call indices, storage prefixes,
modifiers, hashers, unhashed key identities and registry IDs, default bytes,
constant types and values, indexed events and field identities, full API
signatures, and signed extensions in encoding order.

`MetadataIr::from_runtime` and `RuntimeMetadataIr::metadata_ir` produce the IR.
Referenced type graphs, call/event/error enum shapes, and key/hasher counts are
checked. Graph validation is iterative and cycle-safe, including error fields,
generic parameters, and nested API outputs. Registry IDs describe only the parsed runtime; generated public
descriptors must continue using names and structural hashes.

V15 has no API versions. `RuntimeApiIr::version` therefore starts as `None`.
`set_api_versions` takes trait names and advertised versions and applies them
atomically, rejecting unknown names and conflicting replacements.
`set_v16_api_versions` accepts already-decoded V16 metadata and also requires
matching API and method-name sets. The caller must fetch V15, V16, and advertised
runtime versions at the same pinned runtime. This helper does not compare
cross-version registry shapes or infer versions from method presence.

The three `ops` modules extract the protocol portions of
`sdk/bittensor-core/src/client.rs` at the same v478 commit, Apache-2.0:

- `metadata::{v15_request, legacy_request, decode_v15, decode_legacy}` builds
  requests and strictly unwraps `Option<OpaqueMetadata>`. It keeps an absent
  version separate from malformed JSON, hex, SCALE, or trailing bytes. The
  transport caller owns the precise Python missing-method fallback policy.
- `storage::{storage_info, storage_request, decode_result, decode_storage,
  decode_storage_bytes}` borrows the live storage description, delegates keys
  to `Runtime::storage_key`, and decodes live default bytes or optional misses.
- `runtime_api::{runtime_api_method, encode_params, request, decode_result,
  decode_bytes}` resolves live V15 descriptions, checks arity, encodes ordered
  arguments through `encode_id`, and strictly decodes the output type.

`ops::RpcRequest` holds a method and positional JSON arguments. Transport,
request IDs, block-hash validation, and retry policy remain caller-owned.
Storage requests retain the core's ability to construct partial map prefixes;
a scalar fetch caller must supply the complete map key.

`value_ext` extracts the same core client's nine value-navigation helpers:
`field`, `variant`, `variant_name`, `as_str`, `as_u128`, `as_bool`,
`value_bytes`, `tuple_item`, and `first_byte`. Both free functions and trait
methods are available. Matching retains first matching string-keyed fields,
full-width unsigned conversion, repeated lowercase `0x` prefixes, and UTF-8
for other strings. Byte extraction follows singleton wrappers iteratively,
with a 256-wrapper bound matching the core codec's nesting bound.

All seven staged modules carry portable symbol deletion probes for PR-U1,
PR-U9, PR-U15, and the existing PR-U10. No upstream filing is claimed by this
implementation record; the root executor owns external filings.

## Validation

Historical validation used `target/execute-plan.6Ro0PC`, its shared `build`
directory, and its `tmp` directory. That root is retired and absent. Current
substantial checks run in free `ubuntu-latest` CI under Q30, with no local
build, toolchain installation or Python execution required.

- Eight IR tests cover explicit synthetic V15 field identities, indices,
  constants, key tuples/hashers/defaults, unnamed event fields, serde
  round trips, dangling nested references, recursive graphs, atomic version merges, matching/mismatched
  V16 names, V14's empty API surface, and the pinned spec-461 golden metadata.
- Thirteen protocol tests cover versioned metadata requests and wrappers,
  live storage key construction and miss semantics, runtime API arity,
  positional encoding, and strict malformed/trailing-byte failures.
- Nine value-navigation tests cover every helper, integer boundaries,
  malformed shapes, 256/257 wrapper depths, and 65,536-level malformed input
  disposed iteratively by the test.
- Two preexisting strict-signature tests remain in place.

Full strict Clippy passed over all crate targets on Rust 1.99.0. The exact
pinned monorepo source lints passed over all 12 staged source/test files through
both the previously validated `ci/runner/xtask` binary and a final direct
`cargo xtask lint-upstream` invocation.

`cargo +1.89.0 check -p rtao-core-ext --lib --locked` passed. Rustup installed
the normalized `1.89.0` alias; the workspace already had a `1.89` alias. The
new toolchain was retained as useful MSRV tooling. This historical installation
creates no current setup requirement; existing shared toolchains are preserved.

All 32 crate tests passed under `--locked`; strict Clippy and the exact upstream
source lint passed after final changes. The normal/build dependency graph has
577 printed rows and no Tokio, axum, hyper, reqwest, or `*-sys` package. It builds
the pinned git `w3f-bls` only; the separate workspace-wide literal lockfile
requirement remains owned by the root executor.

`cargo xtask staged --check` passed against the final source, identifying all
seven portable staged symbols as absent upstream. `cargo fmt -p rtao-core-ext
--check`, doc-test discovery, and the final `cargo +1.89 check --lib --locked`
also passed.

The guard reviewer independently verified portable and host comparisons for
all seven probes, the current curated core surface, and the pinned source
lints. `cargo check -p rtao-core --no-default-features --locked` passed after
the root executor re-exported `ir`, `ops`, and `value_ext` through the core.

The protocol fixture's outer enum types were corrected to avoid registering
the unit output type as an outer event enum. The protocol worker's offline
invocation also refreshed the shared lock for a concurrent btauth dependency;
the root executor was notified and retains lockfile ownership.
The historical staged gate updated the executor's shared `staged-probe`
directory. Those probe/build paths are under the retired execution root and
are not current live dependencies. No separate temporary root or private build
cache was created by this implementation. Current staged and workspace checks
belong in hosted CI; shared global Cargo/toolchain caches remain preserved.
