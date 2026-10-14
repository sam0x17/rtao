# Feasibility check: consuming `bittensor-core` from the monorepo as a Cargo git dependency

Run on 2026-10-13 from `scratchpad/feasibility/core-check` (source in that directory).

## Setup

```toml
[dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", rev = "850f2755bb05a3851f948489f0ab27b0645cebce", default-features = false }

[patch.crates-io]
# the monorepo patches w3f-bls to its vendored no-std fork (vendor/w3f-bls); a downstream
# crate must replicate the patch because [patch] sections of dependencies are ignored by Cargo
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", rev = "850f2755bb05a3851f948489f0ab27b0645cebce" }
```

```rust
let kp = bittensor_core::keys::Keypair::from_uri("//Alice", bittensor_core::keys::CRYPTO_SR25519 /* = 1 */).unwrap();
println!("{}", kp.ss58_address()); // 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
```

## Results (no default features, i.e. without the `host` feature)

- Resolves and compiles: the git source exposes the `bittensor-core` package and Cargo also found the vendored `w3f-bls` package inside the same git checkout for the `[patch.crates-io]` entry, even though `vendor/w3f-bls` is excluded from the monorepo workspace.
- Pulls the RaoFoundation `polkadot-sdk` fork (`sp-core`, `sp-io`, `sp-runtime`, `frame-metadata` 23) and the ideal-lab5 `tle` rev through the monorepo's workspace pins.
- Clean `cargo check` (not build): **110 s wall / 77 s user** on Apple Silicon.
- Unique crates in the graph: **207**.
- `cargo run` of the smoke program printed the correct Alice address, so the no-`host` build links fine on macOS.
- API notes: `Keypair::from_uri(uri, crypto_type: u8)` with `CRYPTO_ED25519 = 0`, `CRYPTO_SR25519 = 1`; `DEFAULT_SS58_FORMAT = 42`; `ss58_address()` takes no arguments.

## With default features (`host`: sodiumoxide, openssl-vendored/fernet, ansible-vault, pbkdf2, scrypt, rayon, reqwest blocking, libloading)

Result: `cargo check` with default features also succeeds on macOS (sodiumoxide 0.2.7, fernet 0.2.1 with vendored OpenSSL, tle, w3f-bls all compiled): **54 s wall / 212 s user** for the check, **330 unique crates** in the graph.

## Implications for rtao

- A git dependency on the monorepo crate is technically workable (pin to a `v<spec_version>` tag), but it is **not publishable to crates.io** (crates.io rejects git dependencies and `[patch]` sections are not honored for dependents), and it drags the polkadot-sdk fork into every downstream build.
- The fork pins mean `sp-core` types from `bittensor-core` and types from a crates.io `subxt`/`sp-core` would be different crates; interop must go through plain byte arrays (`[u8; 32]` public keys, `Vec<u8>` call bytes, ss58 strings), which both sides already use.
- Two sustainable paths exist: upstream a crates.io release of `bittensor-core` (unblocking: the w3f-bls patch — would need the no-std fix released on crates.io or the dependency made optional), or rtao owning a mirrored/ported "chain-defined" crate that is validated against the same golden fixtures (`sdk/python/tests/fixtures/golden.json`, shape corpus, keyfile goldens, btauth vectors).
