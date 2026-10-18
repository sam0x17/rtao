# M2 secret boundaries

## Current source and scope

The guarded secret JSON parser and wallet public projection are implemented.
The retained hosted [run 38832708259](https://github.com/sam0x17/rtao/actions/runs/38832708259)
at `1edf4d7e859cadb362644196cde5bf278c527c73` records exact passes for all
twelve parser/public-view/core-differential regressions. See the
[M2 security dispositions](../security/m2.md#current-hosted-dispositions-at-1edf4d7)
for S01/S03/S04/S05 repairs and their execution limits. This is evidence for
that exact source, not a new current-checkpoint or platform run.

M2-S02 is fixed by the combined core patch pinned at fork
`sam0x17/subtensor` rev `626f2a4f1f67ab731c1fc3884828751496bc125d` (branch
`rtao-core-patch/v478`, base v478 `850f2755bb05a3851f948489f0ab27b0645cebce`).
Its commits are `eafe84e8` (Ledger PR #3242), `774cc3ac` (ownership repair),
`5a43348e` (derivation correction) and `626f2a4f` (wipe the derivation preimage
and PKCS8 import copies). The combined six-path diff SHA-256 is
`8cf8ff28467ef20a8e47add1a600f60703941dbf3d9af6c2406b9f613e86c489`. The stored
Ed25519 key is `Zeroizing<ed25519_zebra::SigningKey>`; zebra 4.2.0 derives
`Zeroize` over the seed, expanded scalar and nonce prefix, so final drop wipes
it. Public keys and signatures are byte-identical: the RFC 8032 vector and the
Substrate `//Alice` and `//Bob` Ed25519 vectors pass.

The M2-26 final cold-reader security pass on 2026-10-17 used four independent
reviewers, none of them the author (Ed25519 erasure, signing compatibility,
keyfile/password/redaction and patch provenance). Three skeptics verified each
finding, and only findings with at least two confirmations were kept. Four
low-severity findings were confirmed and repaired:

- Hard-junction derivation hashed the parent seed through an unwiped SCALE
  `using_encoded` Vec. It now uses a fixed `Zeroizing` preimage buffer with
  identical bytes.
- Encrypted-JSON import kept decrypted PKCS8 secrets in plain stack arrays.
  They are now `Zeroizing`.
- `rtao_core::keyfiles` re-exports the unrepaired upstream plaintext codec and
  environment-password functions. The module rustdoc now documents this and
  points users to `rtao_core::keyfile_json` and the `rtao_wallet` password
  sources.
- The xtask core-patch Ed25519 source guard was a substring check with a
  tautological `needs_drop` test. It is now a syn-based structural guard over
  every core source file. It rejects any `ed25519::Pair` type position or
  import, requires the exact `Zeroizing<SigningKey>` owner and variant, the
  named regression tests and vectors, and forbids the unwiped `using_encoded`
  derivation. Fork tests `zeroize_clears_every_signing_key_secret` and
  `hard_derivation_keeps_substrate_vectors` cover the repairs.

Verifiers refuted the remaining candidates: inherited `sp_core` `from_string`
seed copies, unnamed Copy move copies (covered by the stack-copy limit
below), a missing Debug test for `GeneratedKey`/`RegenerateFrom` and the fuzz
workspace's use of the unpatched core.

Local evidence on a development workstation: the fork's bittensor-core library tests (72)
pass, the rtao-core, core-ext, signer and wallet tests (250) pass, and
`cargo xtask pin --check` passes against the real fork rev. Hosted CI for the
final head is recorded in [state.md](state.md). M2-26 is complete.

Limits remain explicit. No claim covers erasure of allocator or OS copies,
stack copies left by moving the Copy `SigningKey`, short-lived values inside
zebra, blake2, schnorrkel or the KDFs, or caller-owned inputs. No hardware or
platform execution is claimed. Substantial validation belongs in free hosted
`ubuntu-latest` CI under Q30.

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
buffers. At that snapshot Rtao could not wipe the opaque signing pair: the
pinned Ed25519 Pair was Copy/Clone without a guaranteed secret wipe on Drop,
and M2-S02 remained open and unaccepted. The current section above records its
later fix by the combined core patch.
Historical test results below predate the parser repair.

The serving boundary also enforces approved D-017: decoded certificates over 65 bytes fail after IP validation and complete hex parsing. Ten serving tests pass on current Rust and Rust 1.89. The length error has lower priority than Python's IP and fromhex errors.

Current Rust and Rust 1.89 pass six codec unit tests and six codec integration tests, eight signer tests, two wallet error-chain tests and one wallet derivation/codec test. Strict all-feature/all-target Clippy passes for core-ext/signer. The wallet reader test module was moved after the production items; the combined wallet MSRV all-feature/all-target Clippy now passes. The pinned codec differential expectation update passes its four parent tests on Rust 1.89 and current Rust. The ignored environment child runs through its parent regression. D-036 is registered; the complete workspace gates remain required.

The source-ready function and regression list, the bounded validation logs and the separate certificate proof logs are not retained in the repository. No Python, local oracle containers, generators, manifests, commits, pushes or tags were used. Per-test wallet directories used the recorded task root and their existing cleanup guards. That root and its raw build/tmp paths are retired and absent; no current dependency remains there. Shared global Cargo/toolchain caches and durable wallet/recovery state remain preserved.
