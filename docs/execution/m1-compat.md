# M1 compatibility inputs and guard document

`cargo xtask compat` now generates `compat.json` and `COMPAT.md` from the workspace
version, core and parity pins, immutable Finney metadata index, localnet image
digests, btauth implementation constants and pinned Python project version. It
verifies each stored metadata blob and the Python project's pinned SHA-256 before
using them. Generation and `--check` require no network access or Python process.

The generated schema is `1`. The current `validated_spec_range` is `null`,
`validated_runtimes` is empty, and the release milestone is unset. The recorded
Finney spec 478 inputs have no drift, e2e or interop run evidence establishing a
validated range. A metadata fetch does not establish that evidence. The client
agent consumes this document for its warning guard; the complete M1-13 connection
and runtime-cache behavior is checked separately.

| input | SHA-256 file checksum | BLAKE2b-256 raw metadata hash |
|---|---|---|
| Finney 478 V15 | `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502` | `0x1c3d74d0316aed364c9df9857b175845eb9d215472f5642e9d29085817edec06` |
| Finney 478 V16 | `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7` | `0xaa06a96435a6a75424542e692063cc1b3a3b2c9d5aaa4f0be0173ad0b04cd4c3` |

The runtime identity uses `metadata_hash.kind = "blake2b-256-raw"`. This hashes the
complete metadata blob with BLAKE2b configured for a 32-byte output. The SHA-256
column verifies stored file bytes. Neither identifies the transaction metadata
commitment or a structural surface hash.

The generator accepts a future evidence ledger only when its input hash and report
checksums match. Every selected runtime must occur in the immutable pins and have
passing e2e and interop coverage on every distinct listed image digest. The spec
range must have no missing integer spec, and every selected identity must be
connected through adjacent passing drift reports on the same chain. A singleton
requires an unchanged self comparison. Breaking drift, a failed/skipped report,
missing coverage, stale inputs, changed metadata identities and tampered report
bytes all fail. [The report schema](../compat/README.md) records the required
GitHub run URL and commit provenance. No report or validated ledger was created.

Verification on 2026-10-14:

| command | result |
|---|---|
| `cargo xtask compat` | Generated both outputs |
| `cargo xtask compat --check` | Passed, verified outputs and input/evidence bytes offline |
| `cargo xtask compat --check --milestone M1` | Rejected stale output with exit 1, as expected for the unset milestone; did not write files |
| `cargo test -p xtask compat::tests -- --test-threads=1` | 10 passed, 0 failed |
| `cargo +1.89 test -p xtask compat::tests -- --test-threads=1` | 10 passed, 0 failed |
| `cargo clippy -p xtask --all-targets -- -D warnings` | Passed |
| `rustfmt --edition 2024 --config skip_children=true --check xtask/src/compat.rs` | Passed |

All Cargo invocations used the recorded task root
`~/workspace/rtao/target/execute-plan.6Ro0PC`, its `build` directory as
`CARGO_TARGET_DIR`, and its `tmp` directory as `TMPDIR`. Tests used in-memory
reports and created no private disposable fixtures. That execution root is
retired and absent; no build or temporary directory there is a current live
dependency. Current compatibility generation/checks use bounded free
`ubuntu-latest` CI under Q30, preserving shared global Cargo caches.
