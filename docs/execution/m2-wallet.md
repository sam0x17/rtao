# M2 wallet and portable keyfile evidence

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 remains open and
unaccepted, so M2-26 and the M2 milestone exit remain open. The record below
preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`WalletRef` resolves the btcli directory layout and the `wallet/hotkey`
shorthand. It exposes public identities without unlocking. `Wallet`,
`HotkeyFile` and `ColdkeyFile` implement the signer role contracts; file sources
support only their own signing role. Clones share a lazy unlock cache. KDF and
keyfile work run through `rtao_core::offload`.
Wallet and hotkey names must be single path components; `.` and `..` are
rejected to keep writes inside the selected wallet directory.

Public views read public companions or public fields in a plaintext hotkey.
They never call the secret-key decoder or decrypt a file. `PublicKeyInfo`
includes `AccountId` and `CryptoType` for fee estimates without signer
acquisition. Legacy missing schemes default to sr25519 as in the pinned core.
Tests preserve these views with absent or deliberately unusable secret files.

The staged `keyfile_json` module ports the pinned core's codec and format
detection behavior. Its later guarded parser/projection is described in
[m2-secret-boundaries.md](m2-secret-boundaries.md). It preserves secret-field precedence, crypto-type strings,
address-based retry, the legacy leading-space address key, raw hex/mnemonic
files and the Polkadot-JS import hint. Core reexports keep these operations
available without `host`; encrypted keyfiles and encrypted JSON import call
the core behind `encrypted`. The portable wallet feature tree contains no
`bittensor-core/host` edge.

Creation and regeneration return recovery material through `SecretString`
without printing it. Mnemonic, seed, URI, private-key and encrypted-JSON sources
use upstream derivation. Secret buffers use `Zeroizing`; the later public JSON
projection validates the complete document before exposing borrowed fields and
guards owned escaped strings. Caller-owned input remains outside that wipe
boundary. New files are 0600 and new
directories 0700. Replacement writes use a sibling temporary, fsync and rename.
Without overwrite permission, an atomic hard-link install refuses concurrent
replacement. Existing directory permissions remain untouched. Symlink writes
are refused, and write attempts invalidate cached signers.

Password resolution follows Python steps 1-4 and 7: nonempty explicit input,
the wallet environment variable, the encoded keyfile variable, an explicit or
environment password file, then the caller's permitted prompt callback.
The library default never prompts or reads stdin. Wrong-password errors record
the first chosen source and fail without trying later sources. Keychain and
native-dialog requests use the caller's injected `NativePasswordProvider`
after earlier sources are exhausted; platform adapters belong to M4. `keyfile_env` encodes and decodes the pinned XOR/base64 format
without changing process variables.

The password-file description in architecture §6.2 was corrected from “first
line” to “whole file, trimmed”: the pinned `wallets.py:96` calls
`read_text().strip()`. Tests preserve multiline contents and Python's empty-file
branch.

M2-S05 adds a regular-descriptor check and a 16 MiB cap to both keyfile and
password-file reads (D-037). Unix opens are nonblocking before descriptor
validation, so a FIFO without a writer is rejected promptly. Regular symlink
targets remain supported. Reads use wiping byte buffers before UTF-8 validation
and preserve password precedence and whole-file trimming. The nine library
tests pass on current Rust and Rust 1.89, including oversized sparse files,
directories, FIFOs, regular symlinks and invalid UTF-8. Logs are
`tmp/root-m2-wallet-security-{current,msrv}.log` under the recorded root.

Historical verification:

- Rust 1.89 default and encrypted all-target wallet suites passed: six password
  tests, including 256 Unicode samples, and ten integration tests per suite.
- Strict all-target wallet Clippy passed with default and all features.
- The Rust 1.89 compile-fail doctest confirms a `HotkeyFile` cannot satisfy
  `KeySource<Coldkey>`.
- The Rust 1.89 core host differential test passed its three checks and isolated
  environment-reader child. Both codecs agree on decoding, errors, public
  identity, signing capability and value-equal serialized JSON.
- `cargo xtask staged --check` passed all 15 staged modules. The JSON codec is
  reported as upstream but host-gated, with the `keyfiles-json` feature absent;
  the pure environment encoder remains absent upstream.
- Concurrent creation, overwrite refusal, cache sharing/invalidation, public
  views, permissions, symlink handling and mnemonic/seed/private-key regeneration
  are covered. The encrypted test creates a disposable NaCl coldkey, unlocks it,
  checks cache reuse and rejects a wrong explicit password.

Keyfile detection and JSON fuzz sources and fixed disposable seeds are present
under `fuzz/`, and both targets are in the manifest and the nightly driver's
target list. Both compile with the standalone lock. Instrumented execution is
recorded separately. No encrypted
KDF is fuzzed. No local Python, real wallet, Keychain or host password store was
used. Temporary tests stayed under the recorded task root and were removed
after checking processes, open files, launchd, configuration, symlinks and
virtual-environment dependencies. That former task build/probe root is retired
and absent; shared global Cargo/toolchain caches remain preserved. No local
rebuild, tool installation or fuzz campaign is required under Q30.
