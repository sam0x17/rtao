# PR-U4 dependency proof

This probe builds the pinned core with its proposed dependency declaration:
`w3f-bls = { version = "=0.1.3", features = ["std"] }`. Its generated manifest
declares a separate workspace, contains no patch, and uses its own reviewed lock.
The workspace's existing BLS patch remains in force. The committed isolated
lock was generated offline with Rust 1.89 and resolves exactly one registry
`w3f-bls` package, version 0.1.3. Its checksum is
`7335e4c132c28cc43caef6adb339789e599e39adbe78da0c4d547fad48cbc331`.

Run `prepare.sh` with the recorded `RTAO_TEMP_ROOT` and `CARGO_TARGET_DIR`. It
creates a pinned monorepo worktree below the task root, changes the core's BLS
declaration, stages the probe and builds it with `--locked`. The metadata check
requires exactly one `w3f-bls`, version 0.1.3, from the registry.

The executable accepts an input JSON path, an output JSON path, and an optional
recorded Quicknet signature JSON path. Use `past-request.json` and the committed
round-1 signature for the fixed past-round vector. Inputs for localnet contain
the scenario's validator hotkey, UID and a future drand round. The output carries
the canonical core weight payload, inner ciphertext, portable envelope and the
fixed test entropy. Fixed keys and seeds belong only to these public test vectors.

The e2e helper compares these bytes with encryption through the workspace's
vendored BLS dependency. Both BLS builds independently decrypt the past-round
envelope with the committed Quicknet signature. It then submits a future-round
commit on the hosted localnet. The live
proof requires a matching `TimelockedWeightsRevealed` event and the resulting
weight row. Localnet execution belongs to the free `ubuntu-latest` workflow.

The public Quicknet round-one response is retained at
`vendor/recorded/drand/quicknet-1.json`; its adjacent provenance JSON records the
HTTPS URL and body hash. Its randomness is the SHA256 of the signature. The
offline cross-decryption test checks that signature against a ciphertext
encrypted with the pinned core's Quicknet public key and round identity.
`pr-u4-past-vector.json` and its provenance retain the tested deterministic
vector, source identity and isolated-lock hash.

After retaining the JSON evidence and source diff, remove the probe worktree
through `git worktree remove`, then audit and remove its task-scoped source clone
and build preparation directory. Shared Cargo builds may remain live for the
parent task. No wallet or production validator state is used.
