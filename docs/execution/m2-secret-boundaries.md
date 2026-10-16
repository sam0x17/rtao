# M2 secret boundaries

## Current source and scope

The guarded secret JSON parser and wallet public projection are implemented.
The retained hosted [run 38832708259](https://github.com/sam0x17/rtao/actions/runs/38832708259)
at `7bb8c0b888b5e634c8ca1236f05ceb9b6f81a48a` records exact passes for all
twelve parser/public-view/core-differential regressions. See the
[M2 security dispositions](../security/m2.md#current-hosted-dispositions-at-7bb8c0b)
for S01/S03/S04/S05 repairs and their execution limits. This is evidence for
that exact source, not a new current-checkpoint or platform run.

M2-S02 remains open and unaccepted: the opaque Ed25519 object lacks guaranteed
final-drop erasure. Passing CI or wiping exported buffers does not close that
owning boundary. M2-26 and the M2 milestone exit remain open. Substantial
validation belongs in free hosted `ubuntu-latest` CI under Q30.

## Historical preparation and validation

The signer and wallet constructors now replace secret-dependent upstream errors with fixed diagnostics. The shared staged `redacted_derivation_error` helper wipes the discarded owned error message first. URI, mnemonic, seed, private-key and encrypted-JSON failures cannot retain the supplied input through their returned `Display`, `Debug` or `source` chain. Secret offload failures use fixed blocking diagnostics. Actual pinned derivation factories return `Crypto`; malformed keyfile seed hex remains `Keyfile`.

The tests use public disposable material. A valid public BIP39 phrase followed by a public derivation password and newline is a negative control: the pinned URI error includes the phrase and password. The fixed signer and wallet chains exclude both. Valid URI derivation still produces the pinned public-key bytes and signatures; wallet recovery preserves the pinned private-key bytes and scheme after a keyfile round trip.

The raw `rtao::core::keys::Keypair` reexport still returns upstream diagnostics.
The mitigation covers the tested signer, wallet and staged keyfile boundaries;
it does not claim to sanitize every direct upstream primitive. Subtensor #3252
tracks that upstream constructor correction, and the core module documents this
distinction.

The staged codec owns wipe guards for its deserialized string leaves and object keys, serialization Values, extracted private-key Vec, decoded seed Vec and secret hex temporaries. Seed decoding writes into an already guarded buffer, including partial hex failures. Serialization counts bytes first and then allocates its guarded output exactly, avoiding freed secret chunks from Vec growth while retaining the same HashMap iteration and JSON bytes. A completed JSON tree receives its guard before trailing-character validation, so that error path also wipes. Missing-field diagnostics expose reserved static field labels and a fixed marker for unknown input key names.

The 2026-10-15 source repair replaces serde parsing in the codec and wallet
public inspection with a private JSON projection. Unescaped strings and
numeric tokens borrow the input; escaped strings use one preallocated wiping
buffer. Keys and values retain their guards during duplicate replacement,
partial parses and trailing validation. The shared public-field callback
validates the full document before exposing borrowed public fields. An
independent source review found no concrete defect. Twelve new regression
functions were authored at that checkpoint. Their later exact hosted passes
are recorded above; the initial pending state is historical.

The direct codec now enforces the wallet reader's 16 MiB limit, including
historical raw forms, with an exact-boundary fixture. Serde's private carrier
keys are ordinary JSON object keys here, following Python's semantics.

These guards do not wipe caller-borrowed input, transferred output buffers,
third-party KDF internals or chunks freed by earlier wallet-reader buffer
growth. The pinned core separately zeroizes retained mnemonic and seed
buffers. Rtao cannot wipe the opaque signing pair: the pinned Ed25519 Pair is
Copy/Clone without a guaranteed secret wipe on Drop. M2-S02 remains open and
unaccepted.
Historical test results below predate the parser repair.

The serving boundary also enforces approved D-017: decoded certificates over 65 bytes fail after IP validation and complete hex parsing. Ten serving tests pass on current Rust and Rust 1.89. The length error has lower priority than Python's IP and fromhex errors.

Current Rust and Rust 1.89 pass six codec unit tests and six codec integration tests, eight signer tests, two wallet error-chain tests and one wallet derivation/codec test. Strict all-feature/all-target Clippy passes for core-ext/signer. Root moved the wallet reader test module after the production items; the combined wallet MSRV all-feature/all-target Clippy now passes in `tmp/root-m2-wallet-security-clippy-msrv.log`. The pinned codec differential expectation update passes its four parent tests on Rust 1.89 in `tmp/root-m2-codec-differential-security-msrv.log` and current Rust in `tmp/root-resume-codec-current.log`. The ignored environment child runs through its parent regression. D-036 is registered; the complete workspace gates remain required.

Source-ready functions and regression names are recorded in the task root's `tmp/m2-security-source-ready.txt`. Bounded validation logs use `tmp/m2-security-*`, and the separate certificate proof uses `tmp/m2-serving-d017-*`. No Python, local oracle containers, generators, manifests, commits, pushes or tags were used. Per-test wallet directories used the recorded task root and their existing cleanup guards. That root and its raw build/tmp paths are retired and absent; no current dependency remains there. Shared global Cargo/toolchain caches and durable wallet/recovery state remain preserved.
