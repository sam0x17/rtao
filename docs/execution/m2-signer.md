# M2 signer roles and public key views

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`rtao-signer` now exports `SignerRole`, the sealed `Role` markers `Coldkey` and `Hotkey`, `KeyView`, `KeySource<R>`, `DynKeySource`, and `IntentKeyView`. `SignerRole` parses only the two exact role tokens and returns a typed `InvalidRole` error for other input. A source that supports one role can reject the other dynamically with `RoleUnavailable`; the typed path rejects that combination at compile time.

`KeyView` exposes attached public keys. Wallet implementations must read these views without obtaining a signer or unlocking. A bare signer has no attached hotkey or coldkey and instead exposes its own identity through `bare_signer_account()`. `IntentKeyView` applies the pinned Python rules: explicit hotkey overrides win before public I/O; attached wallet keys win next; a bare identity supplies a default only for the intent's signing role. A coldkey-signed intent therefore cannot silently default its hotkey to the bare coldkey. Missing-default messages match `intents/base.py`; actual public-file failures propagate without identity fallback.

Bare signers still sign either role. `KeypairSigner` clones share an `Arc<Keypair>` until its final owner drops. The pinned core separately zeroizes retained mnemonic and seed buffers. At that snapshot Rtao could not wipe its opaque signing pair, and the pinned ed25519 pair had no guaranteed secret wipe on drop; the combined core patch later fixed this as M2-S02. Borrowed constructor inputs remain the caller's responsibility. Constructor errors replace secret-dependent upstream text with fixed diagnostics after wiping the discarded owned message. `Arc<S>` forwards the complete `Signer` contract, including capabilities, prepared requests and raw-signing refusal. This permits a non-clone backend held as `Arc<dyn Signer>` to use the same key-source adapters. Wallet implementations remain independent and may implement only their supported `KeySource<R>` markers.

Validation:

- Seven role tests verify cross-role defaults, exact diagnostics, explicit override precedence, public views without unlocks, real signatures after source drop, dynamic identity/capabilities, unknown roles, and refusal before signer acquisition.
- Six existing signer tests retain exact prepared/raw payload signing, signature discriminator validation, hardware raw-signing refusal, derivation behavior, and secret-redacted Debug output.
- A compile-fail documentation test proves a hotkey-only source cannot satisfy a coldkey operation.
- All 14 tests pass with no default features on current Rust and all features on Rust 1.89. Strict all-feature, all-target Clippy passes.
- Client and btauth consumers compile with all features after the adapter changes.

The later secret-boundary remediation adds a pinned URI-leak negative control, full constructor error-chain checks and valid password-URI derivation evidence. All eight software signer tests pass on current Rust and Rust 1.89, with strict all-feature/all-target Clippy. Detailed owned-buffer and dependency limits are recorded in `docs/execution/m2-secret-boundaries.md`.

No Python ran locally. Tests use deterministic in-memory keys and no user wallet, password, Keychain or Ledger state. No disposable fixture directories were created. The former task root and its build/log paths are retired and absent; preserve shared global Cargo/toolchain caches. Native balance limits remain unchanged; current BLS checks use the approved Q26 built-dependency policy. At that snapshot, M2-S02 remained open and unaccepted; it is now fixed by the combined core patch.
