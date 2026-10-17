# M2 intent interfaces and generation

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

The initial M2-09/M2-10 implementation is recorded below. Hosted client source checks are required under Q30; first-oracle replay is optional/unverified. The current intent/read pending arrays are empty, so the 74/87 counts and unimplemented-body descriptions below are historical.

At that initial snapshot, `cargo xtask regen` generated all 83 intent structs, extracted constructor defaults, signer/origin/output contracts, typed `WRAPS`, `AnyIntent` dispatch, and 74 explicit `NotYetPorted` implementations. Every class has its Python rustdoc alias and the unchanged catalog description. Each Python group is gated by its Cargo feature. With no group enabled, `AnyIntent` is uninhabited and the crate still compiles.

Generation checks the source/catalog field and required sets, refuses unknown override or pending entries, requires an explicit currency source for unsuffixed money fields, validates pass-through calls against metadata and `WRAPS`, and refuses orphan or symlink outputs. `codegen/intent_types.toml` selects native field widths, exact money, mapping/list weights, typed result data and scalar/plural SS58 input carriers. Money uses checked native u64 rao under Q25/D-035. The PoW timeout uses `serde_json::Number` to preserve Python's integer default of 300.

`codegen/intent_callmap.toml` records all 46 reviewed pass-through members. The generator validates their argument and conversion routes. At the initial M2 snapshot it emitted `TransferAll`; the other 45 members were assigned to later M4/M5 behavior hooks and resolvers. No transfer-all build body was hand-written.

`AnyIntent::from_tool` requires a JSON object, ignores the top-level `op`, rejects unknown fields, fills extracted defaults and runs validation. Unknown or disabled operation lookup precedes the object-shape error. Set/commit weight construction canonicalizes mappings into sorted UID and float lists, matching Python's `_as_pairs`. Weight text accepts Python's whitespace, digit separators and Unicode decimal digits before checked finite-ratio conversion. Weight booleans become 0.0 or 1.0 at that input boundary, as Python's `float(weight)` requires; other Ratio inputs remain numeric. UID text uses Python's base-ten integer syntax before checked u16 conversion. Money JSON remains exact decimal strings. SS58 input fields use a validated `Ss58Address` carrier to preserve the caller's network prefix in JSON and summaries; call arguments explicitly extract the binary `AccountId`. `AnyIntent::into_dyn` and the zero-size `rtao_intents::Registry` implement the executor's parser injection without a tx-to-intents dependency cycle:

```rust
Executor::new(client).with_registry(Arc::new(rtao_intents::Registry))
```

The intent owner wired `regen_intents.rs`, `regen_reads.rs` and scoped `--intents-only`/`--reads-only` modes. The reads owner controls read generation and the 87 read entries in the shared pending register.

Current validation includes all-83 constructor/default/field/requiredness/unknown-field/tool-roundtrip checks, mapping/list numeric decoding and duplicate UID checks, object-only tool arguments, Python float and boolean weight coercion, and supported non-42 SS58 prefixes. Four generator tests cover mandatory money currency sources, optional native integer widths, configured scalar/plural input addresses versus wire-account types, and the reviewed 46-call membership. Logs live under the recorded task root's `tmp/m2-intent-input-*` and `tmp/m2-intent-address-config-*` files. Both current Rust and Rust 1.89 pass 38 intent library tests, two native oracle-input tests, four generator tests, no-feature compilation, and strict all-target Clippy for primitives/intents/xtask. All 46 primitives library tests pass on both toolchains. The ordinary regeneration check passes, including after moving address mappings into TOML; that configuration change emits identical Rust. Seven policy-equivalence tests pass, including 73,728 core/native policy cases; the localnet e2e test target compiles with its Docker feature. The hosted replay test remains explicitly ignored until verified CI artifacts exist.

Main CI was inspected before implementation: GitHub returned no runs for `main`. No CI labels, reviews, commits, pushes or milestone tags were created.

The task-scoped bootstrap generator directory was audited against running processes, launchd, repository configuration, symlinks and virtual-environment dependencies, then removed. The former task root and its build/log paths are retired and absent. Preserve shared global Cargo/toolchain caches; current generation and validation belong in hosted CI.
