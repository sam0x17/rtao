`Runtime::metadata_ir()` currently lives only in the PyO3 binding, so portable Rust code generators must repeat its metadata traversal. This change exposes serializable IR records through `bittensor_core::ir` and an inherent `Runtime::metadata_ir()` method. The binding delegates extraction to core and only constructs Python objects.

Existing Python fields retain their names and shapes. Additions include call indices and registry IDs, storage prefixes/modifiers/hashers/key identities/default bytes, constant details, indexed events and fields, runtime API signatures, and ordered signed extensions. SCALE bytes materialize as integer lists so the returned dictionary remains JSON-compatible. V15 API versions stay unknown until explicitly populated; the optional V16 helper checks the API and method sets before applying versions atomically. Broken referenced type graphs and malformed enum or storage shapes return errors, while valid recursive types terminate safely. Registry IDs remain local to the source runtime.

Validation on macOS ARM64 with Rust 1.89, repeated against the clean committed review SHA by the task-local pre-push hook:

- Eight portable IR regressions, including the existing pinned golden metadata fixture, malformed nested references, recursive types, V14 behavior and explicit V16 version guards.
- All-feature core library suite: 71 passed; one existing physical-OpenCL-GPU test ignored.
- Strict core and binding Clippy with all features and targets; package formatting and committed-diff checks.
- Portable `wasm32-unknown-unknown` check with `--no-default-features`.

A Python dictionary/materialization regression is included for hosted CI. No Python interpreter ran locally; all binding checks set `PYO3_NO_PYTHON=1`. The full monorepo preflight was not run because the user's instruction confines Python execution to CI; scoped Rust validation used the authorized `sam0x17` identity. There are no dependency, lockfile, generated Python, pallet or runtime changes.
