# Feasibility check: bittensor-core + subxt 0.51 in one crate (the "veneer" stack)

Run on 2026-10-13 from `scratchpad/feasibility/core-plus-subxt` (source in that directory), with the user's
direction in force: git dependencies are fine, crates.io publishability is not a goal, build rtao as a veneer
over `bittensor-core`.

## Setup

- `bittensor-core` as a git dependency at monorepo rev `850f2755` (default features, i.e. with `host`), plus the
  `[patch.crates-io] w3f-bls` entry pointing at the same git rev.
- `subxt 0.51.1` (`default-features = false`, features `jsonrpsee`, `native`, `reconnecting-rpc-client`),
  `subxt-rpcs 0.51.1`, `tokio 1.53`.

## Results

- **Resolves and compiles.** Debug build 54 s wall (255 s user) on Apple Silicon. 392 unique crates
  (552 packages in Cargo.lock, 52 names with more than one version — mostly legacy crypto generations pulled by
  bittensor-core's `host` keyfile formats (digest 0.9/sha2 0.9/hmac 0.8/cipher 0.2 via sodiumoxide/fernet/
  ansible-vault) and arkworks 0.4 (tle/w3f-bls 0.1.3) next to 0.5).
- **The SCALE/metadata stack unifies to single versions**: parity-scale-codec 3.7.5, scale-info 2.11.6,
  scale-decode 0.16.2, scale-encode 0.10.1, scale-value 0.18.2, frame-metadata 23.0.1, frame-decode 0.18.1,
  schnorrkel 0.11.5, curve25519-dalek 4.1.3, blake2 0.10.6, jsonrpsee 0.26.1, sp-crypto-hashing 0.1.0.
  So types from `scale-value`/`frame-metadata`/`scale-info` can cross between subxt and bittensor-core
  without conversion. `sp-core` is only pulled once (37.0.0 from the RaoFoundation polkadot-sdk fork, via
  bittensor-core); subxt 0.51 does not depend on sp-core.
- **Runs against live finney** (spec 477): subxt `OnlineClient` over an explicit `LegacyBackend` read the head;
  raw metadata fetched with `state_getMetadata` over the same async `subxt_rpcs::RpcClient` (357,499 bytes) was
  parsed by `bittensor_core::runtime::Runtime::parse(bytes, spec_version, transaction_version, 42)` in
  **3.9 ms**; `bittensor_core::keys::Keypair::from_uri("//Alice", CRYPTO_SR25519)` produced the correct address.

## Conclusion

The veneer architecture is mechanically sound: an async tokio transport (subxt-rpcs / subxt backends, or
jsonrpsee directly) can feed raw bytes to bittensor-core's chain-defined engine (Runtime/codec, extrinsic
payload + v4 assembly, keys, keyfiles, timelock, ML-KEM shield, digest, PoW) and to subxt's typed decoding,
in one process, with one copy of the SCALE stack.

Caveats: `bittensor-core`'s `Runtime::parse` accepts V14/V15 and rejects V16 (research A1); fetch V15 via
`Metadata_metadata_at_version(15)` (or use `state_getMetadata`, which returned a parseable blob here) for
bittensor-core, and V16 for subxt if static codegen/types are used.
