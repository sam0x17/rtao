# M2 secret boundaries

The signer and wallet constructors now replace secret-dependent upstream errors with fixed diagnostics. The shared staged `redacted_derivation_error` helper wipes the discarded owned error message first. URI, mnemonic, seed, private-key and encrypted-JSON failures cannot retain the supplied input through their returned `Display`, `Debug` or `source` chain. Secret offload failures use fixed blocking diagnostics. Actual pinned derivation factories return `Crypto`; malformed keyfile seed hex remains `Keyfile`.

The tests use public disposable material. A valid public BIP39 phrase followed by a public derivation password and newline is a negative control: the pinned URI error includes the phrase and password. The fixed signer and wallet chains exclude both. Valid URI derivation still produces the pinned public-key bytes and signatures; wallet recovery preserves the pinned private-key bytes and scheme after a keyfile round trip.

The raw `rtao::core::keys::Keypair` reexport still returns upstream diagnostics.
The mitigation covers the tested signer, wallet and staged keyfile boundaries;
it does not claim to sanitize every direct upstream primitive. Subtensor #3252
tracks that upstream constructor correction, and the core module documents this
distinction.

The staged codec owns wipe guards for its deserialized string leaves and object keys, serialization Values, extracted private-key Vec, decoded seed Vec and secret hex temporaries. Seed decoding writes into an already guarded buffer, including partial hex failures. Serialization counts bytes first and then allocates its guarded output exactly, avoiding freed secret chunks from Vec growth while retaining the same HashMap iteration and JSON bytes. A completed JSON tree receives its guard before trailing-character validation, so that error path also wipes. Missing-field diagnostics expose reserved static field labels and a fixed marker for unknown input key names.

These guards do not wipe caller-borrowed input, returned output buffers after ownership transfers, or dependency internals. Serde's private parser scratch and partially constructed error-path values remain unwiped; arbitrary-precision Number backing strings also lack wipe access. Supported keyfile secrets are String fields, which the completed-tree guard covers. The pinned core separately zeroizes retained mnemonic and seed buffers. Rtao cannot wipe the opaque signing pair, and the pinned ed25519 Pair is Copy/Clone without a guaranteed secret wipe on Drop. Signer rustdoc, the execution evidence and the architecture ownership statement now disclose that limit.

The serving boundary also enforces approved D-017: decoded certificates over 65 bytes fail after IP validation and complete hex parsing. Ten serving tests pass on current Rust and Rust 1.89. The length error has lower priority than Python's IP and fromhex errors.

Current Rust and Rust 1.89 pass six codec unit tests and six codec integration tests, eight signer tests, two wallet error-chain tests and one wallet derivation/codec test. Strict all-feature/all-target Clippy passes for core-ext/signer. Root moved the wallet reader test module after the production items; the combined wallet MSRV all-feature/all-target Clippy now passes in `tmp/root-m2-wallet-security-clippy-msrv.log`. The pinned codec differential expectation update passes its four parent tests on Rust 1.89 in `tmp/root-m2-codec-differential-security-msrv.log` and current Rust in `tmp/root-resume-codec-current.log`. The ignored environment child runs through its parent regression. D-036 is registered; the complete workspace gates remain required.

Source-ready functions and regression names are recorded in the task root's `tmp/m2-security-source-ready.txt`. Bounded validation logs use `tmp/m2-security-*`, and the separate certificate proof uses `tmp/m2-serving-d017-*`. No Python, local oracle containers, generators, manifests, commits, pushes or tags were used. Per-test wallet directories use the recorded task root and their existing cleanup guards; shared build/tmp caches remain live for root work.
