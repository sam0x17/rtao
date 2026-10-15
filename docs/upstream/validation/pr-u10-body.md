The public verifier retries a signature over `<Bytes>message</Bytes>`, and the wallet password utility combines XOR/base64 encoding with a process-environment write. Callers needing exact signed bytes or pure password encoding currently have to repeat core logic.

This change adds public `keys::verify_strict`, delegating to the existing pinned Substrate verifier without the wrapper retry, and an ungated `keyfile_env` module exposing pure encode/decode helpers. The portable codec preserves the legacy UTF-8 XOR/base64 format, whitespace handling and errors, and zeroizes temporary buffers. Existing host keyfile get/save functions delegate to it; their API and compatibility behavior remain available. Thin binding functions and `bittensor.sp_core` exports expose strict verification and pure encoding to Python too. No dependency or lockfile change is needed.

Validation on macOS ARM64 with Rust 1.89, repeated on the clean committed review SHA through the task-local pre-push hook:

- Six portable regressions with default features disabled: raw/wrapped sr25519 and ed25519 signatures, malformed signature/address boundaries, fixed ASCII/Unicode/empty password vectors, decoder errors/whitespace and proof that encoding makes no environment write.
- All-feature core library tests: 71 passed; one existing physical-OpenCL-GPU test ignored.
- Strict core and binding Clippy, all features and targets; package formatting and committed-diff checks.
- Portable `wasm32-unknown-unknown` check with default features disabled.

Python binding regressions are included for hosted CI only. No Python interpreter ran locally; binding checks explicitly set `PYO3_NO_PYTHON=1`. The full monorepo preflight was not run because the user's instruction confines Python execution to CI; scoped Rust checks used the authorized `sam0x17` identity. The change touches no generated Python, pallets or runtime code.
