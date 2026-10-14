# F — Rust technology stack options for rtao (evaluated 2026-10-13)

Audience: the rtao architects (expert Rust/Substrate). Every version below was verified today against crates.io's API (raw JSON saved under `T/crates/<name>.json`), docs/sources in the local cargo registry, GitHub's API, or files on disk. Anything not verified is marked **[inferred]** or **[not verified]**.

Path legend (all absolute paths start with `SP`, abbreviated `S/`):

| Abbrev | Path |
|---|---|
| `S/` | `SP` |
| `U/` | `S/upstream` (read-only clones) |
| `MONO/` | `U/subtensor` (RaoFoundation/subtensor @ 850f275) |
| `CORE/` | `MONO/sdk/bittensor-core` |
| `META/` | `S/meta` (finney V16 metadata, subxt explore dumps, `finney_codegen.rs`) |
| `F/` | `S/feasibility` (scratch crates I built/measured today) |
| `T/` | `S/research/tmp` (crates.io / GHCR JSON captured today) |
| `R/` | `REG` (unpacked published crates: subxt 0.51.1 family, frame-decode 0.18.1, jsonrpsee 0.26.1, argon2 0.5.3, sodiumoxide 0.2.7, libsodium-sys 0.2.7, …) |
| `RESEARCH/` | `S/research` (sibling reports A1/A2/B/C1/E/G/H/I and the two feasibility notes) |

---

## 0. Executive summary

1. **subxt 0.51.1 (2026-09-30, MSRV 1.88, edition 2024) is the right client layer.** It no longer depends on `sp-core` (only as a dev-dependency), `subxt-core` was deleted in 0.50 (last `subxt-core` on crates.io is 0.44.3, 2026-03-12), metadata V16 is supported (`subxt-metadata` `from_v16`), and the `#[subxt::subxt]` static codegen of the full finney V16 metadata compiles and signs correctly against finney today (`RESEARCH/feasibility-subxt.md`; re-measured here). Sources: `R/subxt-0.51.1/Cargo.toml` (`[package]`, `[features]`, `[dependencies.*]`), `T/crates/subxt.json`, `T/crates/subxt-core.json`.
2. **Subtensor's four custom transaction extensions need no custom `Config`** today because `frame-decode` skips any extension whose `extra`/`implicit` types encode to zero bytes (`R/frame-decode-0.18.1/src/methods/extrinsic_encoder.rs:1216-1316`), and all four are empty composites with unit implicit (`META/finney.json:28356-28432`). If upstream ever adds a payload to one of them, 0.51's `DefaultExtrinsicParamsBuilder::custom_extension(name, value)` covers values but **not** non-empty implicit data or authorization extensions (`R/subxt-0.51.1/src/config/default_transaction_extensions.rs:139-146, 234-243`; open issue paritytech/subxt#2276). rtao must ship a startup guard + CI drift job (§1.5, §10).
3. **Backend:** the 0.51 default `CombinedBackend` (built by `OnlineClient::from_url`/`from_rpc_client`) fails on the public finney WS endpoint with `-32601 Method not found`; `LegacyBackend` and `ChainHeadBackend` work (`RESEARCH/feasibility-subxt.md`, corroborated by CortexLM/buyback `U/CortexLM_buyback/src/chain.rs:353-355`). rtao must construct the backend explicitly; `LegacyBackend` is the safe default.
4. **Dependency weight (measured today, `F/*.crates.txt`, `F/build-timings.log`, Apple M5 Max 18 cores, rustc 1.99.0, warm registry):** hand-rolled jsonrpsee 0.26 + frame-decode 0.18 + scale-* + schnorrkel stack = **186 crates, 11.3 s wall / 95 s CPU** clean release build; subxt 0.51 dynamic-only = **228 crates, 15.2 s / 155 s**; subxt with the 60,420-line generated finney module = **228 crates, 25.7 s / 183 s**; `bittensor-core` as a git dependency = 207 (no `host`) / 330 (`host`) crates (sibling measurement, `RESEARCH/feasibility-bittensor-core-gitdep.md`).
5. **Hand-rolling buys 42 fewer crates and ~60 CPU-seconds**, at the cost of re-implementing signer payloads (V4/V5), extension encoding, nonce/mortality handling, backends, transaction watching and metadata-per-spec caching — all of which subxt already does and tests. Not recommended for the core; the user's own `mev-scanner` (`U/sam0x17_mev-scanner/Cargo.toml`) already uses subxt 0.44 for the client and `frame-decode`/`scale-value` for decoding, which is exactly the hybrid pattern this report recommends.
6. **`bittensor-core`'s runtime/codec engine is not a viable dependency**: `publish = false`, blocking `reqwest` transport, pulls the RaoFoundation polkadot-sdk fork (`sp-core 37.0.0` from git) and the vendored no-std `w3f-bls 0.1.3` via `[patch.crates-io]` — a crates.io crate cannot consume it (`CORE/Cargo.toml:1-7`, `CORE/src/client.rs:1-16`, `MONO/Cargo.toml:227,299,310,335`, `MONO/Cargo.lock`).
7. **smoldot light client is not viable for finney** today: `subxt-lightclient 0.51.1` pins `smoldot-light ^1.3.1` while crates.io is at 3.0.0 (2026-10-07); finney's chainspec has no `lightSyncState`, one `/ws` bootnode, a custom `grandpaWarpSyncCheckpoint` extension, and the node needs a "one-time initial GRANDPA set ID = 3" patch to warp-sync finney at all (`MONO/chainspecs/raw_spec_finney.json` top-level keys; `MONO/node/src/service/grandpa_warp_sync.rs:8-33`). **[inferred]** stock smoldot cannot verify finney's GRANDPA history.
8. **Keyfile crypto can be pure Rust.** The `$NACL` format is libsodium `crypto_pwhash_argon2i` (Argon2**i** v0x13, `OPSLIMIT_SENSITIVE = 8`, `MEMLIMIT_SENSITIVE = 536870912` bytes, fixed 16-byte salt) → 32-byte key → `crypto_secretbox` (XSalsa20-Poly1305) with the 24-byte nonce stored after the `$NACL` prefix (`CORE/src/keyfiles/mod.rs:22-94`; `R/libsodium-sys-0.2.7/src/sodium_bindings.rs:117-132`). `argon2` + `crypto_secretbox` (both RustCrypto) implement exactly these primitives (`R/argon2-0.5.3/src/{algorithm.rs:40,version.rs:18,params.rs:42-61}`; `R/subxt-signer-0.51.1/src/polkadot_js_compat.rs:9-12` already uses `crypto_secretbox::XSalsa20Poly1305`). Golden-vector tests against real keyfiles are mandatory before claiming parity.
9. **The timelock (CRv3) dependency is the one real publishability blocker**: the monorepo pins `tle` to a 2024-11-15 git revision that requires `w3f-bls = "=0.1.3"` and arkworks 0.4 (`MONO/Cargo.toml:310`, `MONO/Cargo.lock`, tle manifest at that rev lines 19-40 [web]); crates.io `tle` is 0.1.0 from 2023-11-26 with 1,735 downloads; the repo HEAD moved to `timelock 0.3.0` on arkworks 0.5 (2025-09-05). Recommendation: feature-gated `rtao-timelock` that depends on crates.io `w3f-bls =0.1.3` (the vendored fork only patches no-std for the runtime; a std client does not need it) and either the pinned `tle` git rev (unpublishable) or a vendored port of the ~100-line encrypt path, validated against `pallet-drand` decryption on localnet.
10. **Version generations matter:** subxt 0.51.1 sits on the RustCrypto "0.10 generation" (sha2 0.10, hmac 0.12, pbkdf2 0.12, blake2 0.10, curve25519-dalek 4, aead 0.5). Newest releases of argon2 (0.6), blake2 (0.11), sha2 (0.11), ed25519-dalek (3.0, curve25519-dalek 5), chacha20poly1305 (0.11), ml-kem (0.3) are the "0.11 generation". Mixing doubles the crypto graph; stay on the 0.10 generation (argon2 0.5.3, ed25519-dalek 2.2.0, chacha20poly1305 0.10, ml-kem 0.2.3) until subxt moves. The MEV-shield pallet itself pins `ml-kem 0.2.2` + `chacha20poly1305 0.10` (`MONO/Cargo.toml:313-314`).
11. **Toolchain:** Rust stable is **1.99.0** (released 2026-10-01; local `rustc 1.99.0 (b940084d7 2026-09-28)`), beta 1.100.0 due 2026-11-12. Edition 2024 everywhere. MSRV floor is set by subxt at **1.88** (jsonrpsee 1.87, proptest 1.88, testcontainers 1.88, config 0.15 1.88).
12. **Recommended stack (one line each; full table in §0.1):** tokio 1.53 · subxt 0.51.1 (`jsonrpsee`,`native`,`reconnecting-rpc-client`) + subxt-signer (sr25519 only, no `ecdsa` → drops the C `secp256k1-sys`) · jsonrpsee 0.26.1 only for custom WS headers · axum 0.8.9 + tower 0.5.3 + tower-http 0.7.1 for the btauth/1 server · reqwest 0.13.5 (rustls) for the btauth/1 client · schnorrkel 0.11.5 / ed25519-dalek 2.2.0 / argon2 0.5.3 / crypto_secretbox 0.1.1 / pbkdf2 0.12 · merkleized-metadata 0.5.1 (optional) · ml-kem 0.2.3 + chacha20poly1305 0.10 (optional) · clap 4.6.7 + config 0.15.27 · tracing 0.1.44 + tracing-subscriber 0.3.23 · metrics 0.24.6 + metrics-exporter-prometheus 0.18.3 · proptest 1.12 / insta 1.49 / wiremock 0.6.5 / testcontainers 0.28 / criterion 0.8.2 · cargo-nextest, cargo-deny 0.20.2, cargo-semver-checks 0.51.0, cargo-audit 0.22.2 (same pin as upstream CI), cargo-hack, zepter 1.88.1 · docify 0.4.1 + rustdoc + mdbook 0.5.4 · release-plz 0.3.172.

### 0.1 Recommended stack table

| Area | Crate (version verified 2026-10-13) | Features / notes | Evidence |
|---|---|---|---|
| Runtime | `tokio = "1.53.2"` | `rt-multi-thread, macros, sync, time, signal` (+`test-util` dev) | `T/crates/tokio.json` (1.53.2, 2026-10-03, MSRV 1.71) |
| Chain client | `subxt = "0.51.1"` | `default-features = false, features = ["jsonrpsee","native","reconnecting-rpc-client"]`; construct `LegacyBackend` explicitly | `R/subxt-0.51.1/Cargo.toml [features]`; `RESEARCH/feasibility-subxt.md` |
| Signer | `subxt-signer = "0.51.1"` | `default-features = false, features = ["sr25519","std","subxt","native"]` (omit `ecdsa` → no `secp256k1-sys` C build; measured −2 crates `F/subxt-min-check.crates.txt`) | `R/subxt-signer-0.51.1/Cargo.toml [features]` |
| Raw RPC | `subxt-rpcs = "0.51.1"` | `LegacyRpcMethods`, `ReconnectingRpcClient` (feature `reconnecting-rpc-client`) | `R/subxt-rpcs-0.51.1/Cargo.toml [features]`; `R/subxt-0.51.1/examples/rpc_client.rs` |
| Codegen (xtask) | `subxt-codegen = "0.51.1"`, `subxt-metadata = "0.51.1"` | `CodegenBuilder` against pinned `.scale`, committed output | `R/subxt-codegen-0.51.1/src/lib.rs` (CodegenBuilder) |
| Decoding | `scale-value 0.18.2`, `scale-decode 0.16.2`, `scale-encode 0.10.1`, `frame-decode 0.18.1`, `frame-metadata 23.0.1`, `parity-scale-codec 3.7.5`, `scale-info 2.11.6` | re-exported by subxt as `subxt::ext::*`; depend directly only in crates that must build without subxt | `R/subxt-0.51.1/src/lib.rs:85-99` |
| JSON-RPC (direct) | `jsonrpsee = "0.26.1"` | only if custom WS headers/auth are needed: `default-features=false, features=["async-client","ws-client","client-ws-transport-tls"]`; must match subxt's 0.26 | `R/subxt-rpcs-0.51.1/Cargo.toml [dependencies.jsonrpsee]`; buyback pattern `U/CortexLM_buyback/src/chain.rs:312-352` |
| HTTP server (btauth/1) | `axum = "0.8.9"`, `tower = "0.5.3"`, `tower-http = "0.7.1"`, `http = "1.5.0"` | middleware for nonce window/replay cache, body hashing | `T/crates/{axum,tower,tower-http,http}.json` |
| HTTP client (btauth/1) | `reqwest = "0.13.5"` | `default-features=false, features=["rustls","json","http2"]` (feature name `rustls` as used by buyback with 0.13) | `T/crates/reqwest.json`; `U/CortexLM_buyback/Cargo.toml` |
| sr25519 | `schnorrkel = "0.11.5"` | signing context `b"substrate"` | `R/subxt-signer-0.51.1/src/sr25519.rs:25,181,207` |
| ed25519 | `ed25519-dalek = "2.2.0"` | 2.x shares curve25519-dalek 4 / sha2 0.10 with schnorrkel; 3.0.0 moves to curve25519-dalek 5 / sha2 0.11 | crates.io deps of 2.2.0 vs 3.0.0 (fetched today) |
| Mnemonic → seed | `bip39 = "2.2"` + `pbkdf2 0.12` + `hmac 0.12` + `sha2 0.10` | subxt-signer embeds the substrate-bip39 derivation; don't pull `bip39 3.0.0` (duplicate) | `R/subxt-signer-0.51.1/src/crypto/seed_from_entropy.rs:11-30` |
| Hashing | `blake2 = "0.10"`, `sha2 = "0.10"`, `sp-crypto-hashing = "0.1.0"` | blake2b-512 for ss58, blake2_256/twox for storage keys | `R/subxt-utils-accountid32-0.51.1/Cargo.toml`; `R/subxt-0.51.1/Cargo.toml [dependencies.sp-crypto-hashing]` |
| ss58 | `subxt::utils::AccountId32` (base58 0.2 + blake2) or `bs58 0.5.1` | prefix 42, `b"SS58PRE"` | `R/subxt-utils-accountid32-0.51.1/src/lib.rs:65,129` |
| Keyfiles | `argon2 = "0.5.3"`, `crypto_secretbox = "0.1.1"`, `zeroize 1.9`, `secrecy 0.10` | Argon2i v0x13, m=524288 KiB, t=8, p=1, 32-byte key; XSalsa20-Poly1305 | §3.3 |
| Legacy keyfiles (optional feature) | `fernet = "0.2.2"` (needs OpenSSL) **or** hand-rolled Fernet (AES-128-CBC + HMAC-SHA256), `ansible-vault 0.2.1` | legacy `gAAAAA` and `$ANSIBLE_VAULT` formats | `CORE/src/keyfiles/mod.rs:139-175`; `T/crates/{fernet,ansible-vault}.json` |
| RFC-0078 digest (optional) | `merkleized-metadata = "0.5.1"` | needed only to send `CheckMetadataHash` = Enabled | `CORE/Cargo.toml` digest section; `R/merkleized-metadata-0.5.1/Cargo.toml` |
| MEV shield (optional) | `ml-kem = "0.2.3"`, `chacha20poly1305 = "0.10"`, `twox-hash 2.1` | match the pallet's generation | `MONO/Cargo.toml:313-314`; `CORE/Cargo.toml` mlkem section |
| Timelock CRv3 (optional) | `w3f-bls = "=0.1.3"` + `tle` (git rev `5416406…`) or vendored port | see §3.5 | `MONO/Cargo.toml:310`; `MONO/Cargo.lock` |
| Fixed-point | own `rtao-fixed` newtypes over `i128/u128/i64` bits with const FRAC (or `fixed = "1.31"` types) | see §4 | `META/finney_codegen.rs:60162-60200` |
| CLI/config | `clap = "4.6.7"` (`derive`,`env`), `config = "0.15.27"` (or `figment 0.10.19`, stale since 2024-05) | | `T/crates/{clap,config,figment}.json` |
| Logging/metrics | `tracing 0.1.44`, `tracing-subscriber 0.3.23` (`env-filter`,`json`), `metrics 0.24.6`, `metrics-exporter-prometheus 0.18.3` | | `T/crates/*.json` |
| Testing | `proptest 1.12.0`, `insta 1.49.0`, `wiremock 0.6.5`, `testcontainers 0.28.0`, `criterion 0.8.2` | localnet image `ghcr.io/raofoundation/subtensor-localnet:<v477|devnet|sha-…>` | §6 |
| Tooling | `cargo-nextest 0.9.148`, `cargo-deny 0.20.2`, `cargo-semver-checks 0.51.0`, `cargo-audit 0.22.2`, `cargo-hack 0.6.45`, `cargo-llvm-cov 0.9.1`, `cargo-machete 0.9.2`, `zepter 1.88.1`, `typos-cli 1.51.1` | | `T/crates/*.json` |
| Docs | `docify 0.4.1`, rustdoc doctests, `mdbook 0.5.4` | | §7 |
| Release | `release-plz 0.3.172` (or `cargo-release 1.1.6`) | | §9 |
| Toolchain | Rust 1.99.0 stable, edition 2024, MSRV 1.88 | | §8 |

---

## 1. Substrate client layer

### 1.1 What subxt 0.51.1 ships

Crate family on crates.io, all 0.51.1 released 2026-09-30 with `rust-version = "1.88.0"`, `edition = "2024"`: `subxt`, `subxt-signer`, `subxt-rpcs`, `subxt-codegen`, `subxt-metadata`, `subxt-lightclient`, `subxt-macro`, `subxt-utils-accountid32`, `subxt-utils-fetchmetadata`, `subxt-utils-stripmetadata`, `subxt-cli` (`T/crates/subxt*.json`; `R/subxt-0.51.1/Cargo.toml [package]`). **`subxt-core` no longer exists** past 0.44.3 (2026-03-12): the 0.50.0 changelog entry removed `subxt-core` and `subxt-historic` and moved the core logic into `frame-decode` (subxt `CHANGELOG.md` [web], `T/crates/subxt-core.json`). `subxt/src/lib.rs:33-43` keeps a hidden `pub extern crate alloc` "so that the code generated by subxt-codegen can avoid relying on std things … this may change if we move things back to a no-std core/common crate". Consequence: there is no `no_std` subxt today; `frame-decode`, `scale-encode`, `scale-decode`, `scale-value`, `frame-metadata` are the no_std-capable building blocks (`R/frame-decode-0.18.1/Cargo.toml [features] std`).

Feature flags (`R/subxt-0.51.1/Cargo.toml [features]`):

- `default = ["jsonrpsee", "native"]`; exactly one of `native`/`web` must be set (`lib.rs:24-31` `compile_error!`).
- `jsonrpsee` → `dep:jsonrpsee` (0.26.1, feature `jsonrpsee-types`) + `subxt-rpcs/jsonrpsee` + `runtime` (tokio `rt`).
- `reconnecting-rpc-client` → `subxt-rpcs/reconnecting-rpc-client` (adds `finito` backoff + tokio `sync`; `R/subxt-rpcs-0.51.1/Cargo.toml [features]`).
- `light-client` → `subxt-lightclient` + `subxt-rpcs/light-client` (smoldot-light `^1.3.1` native; smoldot `^2.1.0` for web; `R/subxt-lightclient-0.51.1/Cargo.toml`).
- `runtime-wasm-path` (macro reads a runtime wasm), `unstable-metadata`, `web`, `docs`.
- jsonrpsee's transport (via `subxt-rpcs` `native`): `jsonrpsee/async-client`, `client-ws-transport-tls`, `ws-client` → `soketto 0.8.1` WebSocket + `tokio-rustls 0.26` + `rustls 0.23` + `rustls-platform-verifier 0.5` (`R/jsonrpsee-client-transport-0.26.1/Cargo.toml`). It is **not** tokio-tungstenite.

Dependencies that matter for weight (`R/subxt-0.51.1/Cargo.toml [dependencies.*]`): `frame-decode 0.18.1` (with `legacy-types`, which drags `serde_yaml 0.9.34+deprecated` into every build — visible in `F/subxt-dynamic-check.crates.txt`), `frame-metadata 23.0.0`, `scale-decode 0.16.2`, `scale-encode 0.10.1`, `scale-value 0.18.1`, `scale-bits 0.7.0`, `scale-info 2.11.4`, `scale-info-legacy 0.4.2`, `scale-type-resolver 0.2.0`, `sp-crypto-hashing 0.1.0`, `primitive-types 0.13.1`, `keccak-hash 0.11.0`, `impl-serde 0.5.0`, `derive-where`, `either`, `hex`, `serde`/`serde_json` (`raw_value`), `thiserror 2`, `tracing`, `tokio` (optional), `tokio-util`. **No `sp-core`/`sp-runtime`** except as dev-dependencies (`sp-core 36.1.0`, `sp-keyring 41.0.0`, `sp-runtime 41.1.0`).

### 1.2 Client model in 0.51 ("at block" API, metadata per spec version)

- `OnlineClient::<T>::from_url(url)` / `from_rpc_client(rpc)` build a `CombinedBackend` (`R/subxt-0.51.1/src/client/online_client.rs:69-143`); `from_backend(Arc<B>)` (`:149`) lets you pick `LegacyBackend`, `ChainHeadBackend`, `ArchiveBackend` or `CombinedBackend` (`src/backend.rs:24-27`). `CombinedBackend` "will lookup the methods available to it from the RPC client and call methods accordingly" (`src/backend/combined.rs:5-6`); on finney's public WS endpoint that choice produced `RPC -32601 Method not found` for the storage read while `LegacyBackend`/`ChainHeadBackend` worked (`RESEARCH/feasibility-subxt.md`, table); buyback hit the same and hard-codes `LegacyBackend` (`U/CortexLM_buyback/src/chain.rs:353-355`).
- `api.at_current_block()` pins to the latest **finalized** block (`online_client.rs:271-283`), `api.at_block(number_or_hash)` (`:285`), `at_block_hash_and_number` (`:333`). For each block subxt calls `Core_version` (unless `Config::spec_and_transaction_version_for_block_number` answers) and resolves metadata through `Config::metadata_for_spec_version` / `set_metadata_for_spec_version`, fetching and converting V8–V16 on a miss (`online_client.rs:345-401`; `src/config.rs` trait `Config`, ≈ lines 76-152). **This replaces the removed `updater()`/runtime-upgrade subscription** (0.50.0 changelog: "Runtime-update monitoring (`updater()`) is removed because metadata is chosen per block"). After a subtensor runtime upgrade the next `at_current_block()` transparently switches metadata; static addresses still validate per call (below).
- Block streams: `stream_blocks()` (finalized), `stream_best_blocks()`, `stream_all_blocks()` (`online_client.rs` ≈ 207-269).
- Storage: `at.storage().entry(addr)?` → `fetch(keys)`, `try_fetch`, `iter(prefix)`, `fetch_raw` (`src/storage.rs:77-136`; `src/storage/storage_entry.rs:169-228`); values expose `bytes()`, `decode()` (→ `scale_value::Value`), `decode_as::<T>()`, `visit` (`src/storage/storage_value.rs:36-77`). `entry()` returns `Result`, which is where a static address's 32-byte validation hash is checked against the live metadata (`src/storage/address.rs:34,60,89-93`; generated code embeds the hash, e.g. `META/finney_codegen.rs:20462-20480` for `SubtensorModule.SubnetMovingPrice`).
- Events: `at.events().fetch()`, `iter()`, `find::<E>()`, `find_first`, `has` (`src/events.rs:63-170`).
- Transactions (`src/transactions.rs`): `create_v4_unsigned` (164), `create_v5_signable` (390), `create_signed` (405), `sign_and_submit_then_watch[_default]` (427/445), `sign_and_submit` (493), `from_bytes` (82, pre-signed bytes from an offline/HSM signer), `submit_and_watch` (672), `submit` (692), `validate()` (732, `TaggedTransactionQueue_validate_transaction`), `partial_fee_estimate()` (758). Progress: `TransactionProgress::next/wait_for_finalized/wait_for_finalized_success` (`src/transactions/transaction_progress.rs:61-112`).
- Offline: `OfflineClient::new_with_config(config)` (`src/client/offline_client.rs:9-16`) — the config must supply genesis hash, spec/tx versions and metadata (`config.rs` docs on `genesis_hash`, `spec_and_transaction_version_for_block_number`, `metadata_for_spec_version`). Required for air-gapped coldkey signing.
- Runtime APIs: typed via codegen (`subtensor::runtime_apis().subnet_info_runtime_api().get_subnet_hyperparams(NetUid(1))` worked in `F/subxt-check/src/main.rs:41-45`) or `dynamic::runtime_api_call::<_, T>("SwapRuntimeApi","current_alpha_price",(netuid,))` (buyback `chain.rs:478-530`). Both go through `state_call`.
- Reconnection: `ReconnectingRpcClient::builder().build(url)` (`R/subxt-0.51.1/examples/rpc_client.rs`), which "automatically reconnects when the connection is lost but it doesn't retain subscriptions and pending method calls when it reconnects" (`R/subxt-rpcs-0.51.1/src/client/reconnecting_rpc_client/mod.rs:5-12`; `DisconnectedWillReconnect` at `:63`). Validators' block streams must resubscribe.
- Light client: `LightClient::relay_chain(chain_spec_json)` + `OnlineClient::from_rpc_client` (`examples/light_client.rs`) — see §1.9.3 for why this does not work for finney.

### 1.3 Codegen options

Three ways to get the same `subxt_codegen::CodegenBuilder` output (`R/subxt-codegen-0.51.1/src/lib.rs:4-6`: "used by the `#[subxt]` macro and `subxt codegen` CLI command, but can also be used directly"):

1. **`#[subxt::subxt(runtime_metadata_path = "…")]` proc-macro.** Attributes documented in `R/subxt-0.51.1/src/lib.rs:140-420`: `runtime_metadata_path`, `runtime_path` (wasm, feature `runtime-wasm-path`), `runtime_metadata_insecure_url` (explicitly "not recommended in production"), `crate = "…"`, `substitute_type(path, with)` (incl. generic pattern matching and `::subxt::utils::Static<T>` wrapping; `src/utils/static_type.rs:18`), `derive_for_all_types`, `derive_for_type(path, derive)`, `generate_docs` (docs off by default), `runtime_types_only`, `no_default_derives`, `unstable_metadata`. Default derives since 0.43 are `EncodeAsType`/`DecodeAsType` + `Debug` only — `codec::Encode/Decode` must be re-added via `derive_for_*` (0.43.0 changelog; visible in `META/finney_codegen.rs:60164-60170`).
2. **`subxt codegen --file finney.scale` CLI** → committed file. Output for finney V16: **60,420 lines / 3,359,283 bytes** after rustfmt (`META/finney_codegen.rs`, `wc -l`).
3. **`subxt-codegen` as a library** in an `xtask`/`build.rs`: `CodegenBuilder::new()` with `set_type_substitute`, `add_derives_for_type`, `runtime_types_only`, `generate(metadata)` → `TokenStream` (`R/subxt-codegen-0.51.1/src/lib.rs` `CodegenBuilder` fields: `type_substitutes`, `derives_for_type`, `attributes_for_type_recursive`, …). `subxt-codegen` depends on `scale-typegen 0.12.0`, `syn 2.0.77`, `subxt-metadata` (`R/subxt-codegen-0.51.1/Cargo.toml`).

Measured cost of the generated module (`F/build-timings.log`, clean `cargo build --release`, same machine, same subxt graph): dynamic-only **15.24 s wall / 155.5 s user** vs with the full finney module **25.68 s wall / 183.1 s user** → the 60k-line module costs ≈ 28 CPU-seconds (≈ 10 s wall at 18 cores). Note the earlier feasibility run recorded 107.6 s wall / 171.5 s user for the same crate (`F/subxt-clean-release-build.log` tail) — the user-CPU figures agree; the wall-clock difference was contention/downloads, so compare CPU seconds.

Important detail for type ergonomics: subtensor wraps primitives in newtypes (`subtensor_runtime_common::NetUid(u16)`, `currency::AlphaBalance`, `TaoBalance`, `sp_arithmetic::per_things::PerU16` …), so generated signatures take `NetUid` not `u16` (`F/subxt-check/src/main.rs:9`, `META/finney_codegen.rs:57110`). `substitute_type` can map these to rtao's own newtypes implementing `EncodeAsType`/`DecodeAsType` (`src/lib.rs` docs on `substitute_type`), keeping `runtime_types::…` paths out of rtao's public API.

### 1.4 Dynamic API

`subxt::dynamic` re-exports `scale_value::{At, Value, value}` and the dynamic constructors `tx`, `constant`, `storage`, `runtime_api_call`, `view_function_call`, `custom_value` (`R/subxt-0.51.1/src/dynamic.rs:5-25`). Since 0.50 "Dynamic APIs are generic over key and return types": `dynamic::storage::<(u16,), u16>("SubtensorModule", "Tempo")` and the result can be `decode()`d to `Value` or `decode_as::<MyStruct>()` with a `#[derive(DecodeAsType)]` struct (`examples/dynamic.rs`; working subtensor usage in `U/CortexLM_buyback/src/chain.rs:436-530, 765-785, 947`). Dynamic addresses have `validation_hash: None` (`storage/address.rs:103`) → no codegen-vs-runtime check; a typo in a pallet/entry name is a runtime error.

### 1.5 `Config`, transaction extensions, and subtensor's custom extensions (the critical correctness point)

**Finney metadata (V16, spec 477) declares 13 transaction extensions**, including `SubtensorTransactionExtension`, `SudoTransactionExtension`, `DrandPriority`, `CheckShieldedTxValidity` (all empty composites with unit `implicit`) and `CheckMetadataHash` (`META/finney.json:28356-28432`; identifiers at `:57286-57306`; established-facts list in the task). The chain's `ChargeTransactionPayment` is a wrapper type named `ChargeTransactionPaymentWrapper` but its **identifier** is still `"ChargeTransactionPayment"` (`META/finney.json:28304-28330`, `:57301`) — subxt matches on identifier, so its known `ChargeTransactionPayment` extension applies.

**How 0.51 encodes them** (`R/subxt-0.51.1/src/config/default_transaction_extensions.rs`): `SubstrateConfig` uses `DefaultTransactionExtensions<T>` = a struct holding the `KnownDefaultTransactionExtensions<T>` tuple (`VerifySignature, CheckSpecVersion, CheckTxVersion, CheckNonce, CheckGenesis, CheckMortality, ChargeAssetTxPayment, ChargeTransactionPayment, CheckMetadataHash`, lines 19-29) plus a `BTreeMap<String, Value>` of custom values (37-40). Encoding is delegated to `frame-decode`, which walks the metadata's extension list and **skips any extension whose value type (and separately whose implicit type) is empty**: `encode_transaction_extension_values` filters `!is_type_empty(arg.id)` and, for a *missing* extension whose type is `Option<T>`, encodes `None` (`R/frame-decode-0.18.1/src/methods/extrinsic_encoder.rs:1216-1250`); implicits likewise (`:1256-1275`); `is_type_empty` treats zero-length arrays, all-empty composites and tuples as empty, "ones that are mentioned in the metadata but only used in the node side and require no bytes to be given" (`:1277-1316`). This is exactly subtensor's situation, which is why `create_signed` + `validate()` returned `Invalid(Payment)` (i.e. the node decoded all 13 extensions and got as far as fee payment) with no custom `Config` (`RESEARCH/feasibility-subxt.md`, table and interpretation).

**If upstream ever gives one of those extensions a payload:**
- a value-only payload (non-empty `ty`, empty `implicit`) can be supplied per transaction with `DefaultExtrinsicParamsBuilder::custom_extension(name, value)` — the builder checks the name exists in the metadata, that the value encodes to the declared type, rejects duplicates and conflicts with known names (`default_transaction_extensions.rs:100-173`, `:395-420`; 0.51.0 changelog);
- a non-empty **implicit** is rejected: "requires non-empty implicit data, which is not supported" (`:139-146`, `:183-204` comment "mirrors the check frame-decode uses to skip extensions when encoding");
- custom extensions can never be **authorization** extensions (`:234-243`, referencing open issue paritytech/subxt#2276 "Custom transaction extensions cannot be marked as authorization extensions", opened 2026-09-03, state open per GitHub API today).
- In those cases rtao would need its own `Config` with a custom `TransactionExtensions` tuple implementing `TransactionExtension<T>` + `frame_decode::extrinsics::TransactionExtension` (`src/config/transaction_extension_traits.rs:11-52`; tuples up to 26 elements are supported, `:84-120`). Budget this as a ~1-2 day task, not a redesign.

**`CheckMetadataHash`:** subxt's extension always encodes mode `Disabled` and an empty implicit — "but for now we never provide a hash and so this is empty" (`src/config/transaction_extensions.rs:104-123`, `CheckMetadataHashMode` 153-166). The feasibility signed extrinsic was 125 bytes with mode byte = Disabled. Enabling RFC-0078 verification requires computing the digest with `merkleized-metadata 0.5.1` (`R/merkleized-metadata-0.5.1/Cargo.toml`: blake3 1.5, frame-metadata ≥21, scale-decode 0.16) and a custom `CheckMetadataHash` implementation that emits `Enabled` + the 32-byte implicit; `bittensor-core` already does the digest for Ledger (`CORE/Cargo.toml` digest section; `CORE/src/digest/mod.rs`). Recommend an optional `metadata-hash` feature, default off.

**Recommended guard (both options):** at client start, iterate `metadata.extrinsic().transaction_extensions_by_version(v)` and fail if any identifier outside the 9 known names has a non-empty `ty` or non-empty `implicit` (the same `is_type_empty` rule), and run the same check nightly against `MONO/main` metadata so a change is seen before it reaches finney (`RESEARCH/E-existing-rust-efforts.md` §2 reaches the same conclusion).

### 1.6 Signer (`subxt-signer` 0.51.1)

- `sr25519::Keypair::from_uri(&SecretUri)`, `from_phrase(&bip39::Mnemonic, password)`, `from_secret_key([u8;32])`, `derive(junctions)`, `public_key()`, `sign(msg) -> Signature([u8;64])`, free `verify(sig, msg, pubkey)`, `dev::{alice,…}`, `PublicKey::to_account_id()/to_address()` (`R/subxt-signer-0.51.1/src/sr25519.rs:70-322`). Signing context is `b"substrate"` (`:25`, used at `:181` and `:207`) — identical to sp-core.
- Secret URI grammar is a copy of `sp_core::crypto::DeriveJunction` semantics: `phrase/path0//hard///password`, `0x`-hex mini-secret, `DEV_PHRASE` when no phrase (`src/crypto/secret_uri.rs:12-44`).
- Mnemonic → 64-byte seed = PBKDF2-HMAC-SHA512, 2048 rounds, salt `"mnemonic" + password`, over the **entropy** (not the NFKD phrase as in BIP-39 proper), "taken from `substrate-bip39` so that we can keep dependencies in line, and is the same logic that sp-core uses" (`src/crypto/seed_from_entropy.rs:11-30`). The mini-secret is the first 32 bytes. This is what makes subxt-signer addresses match btcli/btwallet; `substrate-bip39 0.6.1` on crates.io (2026-03-26) is the standalone equivalent if needed.
- `polkadot-js-compat` feature decrypts polkadot-js JSON keystores with `scrypt` + `crypto_secretbox::XSalsa20Poly1305` (`src/polkadot_js_compat.rs:8-32`) — the same NaCl primitive the bittensor `$NACL` keyfile uses (§3.3), proving `crypto_secretbox` is the drop-in for `sodiumoxide::crypto::secretbox`.
- Default features include `ecdsa` → `secp256k1 0.30` + `secp256k1-sys` (C). Not needed for Bittensor hotkeys/coldkeys; disable.
- No ed25519 in subxt-signer. Bittensor supports ed25519 keypairs (`CORE/src/keys/mod.rs` `CRYPTO_ED25519 = 0`), so rtao needs `ed25519-dalek` (2.2.0 keeps curve25519-dalek 4/sha2 0.10 like schnorrkel 0.11.5; 3.0.0 is curve25519-dalek 5/sha2 0.11 — crates.io dependency lists fetched today). Verification semantics: sp-io verifies ed25519 with ed25519-zebra (ZIP-215 rules) **[inferred from polkadot-sdk, not verified on disk]**; only malformed signatures differ.
- `verify` in bittensor-core additionally accepts the polkadot-js `<Bytes>…</Bytes>` wrapper (`CORE/src/keys/mod.rs:401-410`); btauth/1 verification must replicate that fallback.

### 1.7 Release cadence and breaking-change history

Every 0.x minor is breaking. From crates.io (`T/crates/subxt_versions.json`, newest patch per minor) and the CHANGELOG [web]:

| Version | crates.io date | MSRV | Notable breaking/major changes |
|---|---|---|---|
| 0.38.1 | 2025-01-24 (0.38.0 2024-10-24) | 1.74 | V5 extrinsic decoding; `ChainHeadBackend` + reconnecting client stabilized |
| 0.39.0 | 2025-02-05 | 1.81 | removed `substrate-compat` feature and `sp_*` impls |
| 0.40.x | 2025-03-06 | 1.81 | dropped the `polkadot-sdk` umbrella dependency |
| 0.41.0 | 2025-03-11 | 1.81 | new `subxt-rpcs` crate; V5 tx construction |
| 0.42.1 | 2025-05-12 | 1.85 | **metadata V16** + View Functions; `Config::Hash` removed; edition 2024 |
| 0.43.1 | 2025-07-18 → 2025-12-09 | 1.85 | **`Encode`/`Decode` derives removed from generated types**; mortal-by-default online |
| 0.44.3 | 2025-08-29 → 2026-03-12 | 1.85 | prerelease `subxt-historic`; last version with `subxt-core` |
| 0.50.x | 0.50.0 2026-03-02 → 0.50.3 2026-08-06 | 1.88 | **redesign**: historic blocks, `subxt-core`/`subxt-historic` removed, `Config` is a value, `ExtrinsicParams`→`TransactionExtensions`, `at_latest`→`at_current_block`, storage `entry()` API, `updater()` removed, errors split per API, smoldot bump (0.50.1), frame-decode 0.18 + `Option<T>` unknown-extension default (0.50.3) |
| 0.51.0 | 2026-09-16 | 1.88 | `DefaultTransactionExtensions`/`DefaultExtrinsicParams` became structs; `custom_extension()`; V5 picks newest extension version |
| 0.51.1 | 2026-09-30 | 1.88 | **jsonrpsee 0.24.5 → 0.26.1 and tower 0.4 → 0.5 in a patch release** (crates.io dependency lists for subxt-rpcs 0.51.0 vs 0.51.1, fetched today) |

(The changelog text says "[0.50.0] - 2025-12-17" while crates.io shows 0.50.0 published 2026-03-02 and the GitHub release on 2026-03-12; treat the crates.io date as authoritative.) Eight breaking minors in 23 months; versions 0.45–0.49 do not exist ("a deliberately large bump up from 0.44.0 to signify the extent of the changes"). Repo activity: pushed 2026-10-12, 491 stars, 94 open issues (GitHub API today). Practical consequence: **rtao must not leak subxt types through its public API** except behind an explicit `subxt` feature/`ext` module, so a subxt major can be absorbed in one crate.

### 1.8 Dependency weight and build time (measured today)

Scratch crates in `F/` (manifests in `F/<name>/Cargo.toml`, crate lists in `F/<name>.crates.txt`, timings in `F/build-timings.log`; `cargo tree -e normal --prefix none | sort -u`, clean `cargo build --release`, Apple M5 Max 18 cores, rustc 1.99.0, warm registry cache):

| Stack | Unique crates (normal deps) | Lockfile packages | Clean release build | `target/release` |
|---|---|---|---|---|
| **hand-rolled**: jsonrpsee 0.26 (ws, tls) + frame-decode 0.18 + frame-metadata 23 + scale-value/decode/encode/info + codec + sp-crypto-hashing + schnorrkel + bip39 + bs58 + blake2 + tokio + serde | **186** | 242 | **11.33 s wall / 95.4 s user** | 424 MB |
| **subxt 0.51.1 dynamic only** (default features + subxt-signer default) | **228** | 397 | 15.24 s / 155.5 s | 662 MB |
| **subxt minimal features** (no `ecdsa`) | 226 | 397 | 15.89 s / 159.5 s | 652 MB |
| **subxt + static codegen of full finney V16** (`F/subxt-check`) | 228 | 397 | 25.68 s / 183.1 s | 664 MB |
| `bittensor-core` git dep, no `host` | 207 | — | `cargo check` 110 s wall (sibling) | — |
| `bittensor-core` git dep, `host` | 330 | — | `cargo check` 54 s wall / 212 s user (sibling) | — |
| reference: user's `mev-scanner` (subxt 0.44 + jsonrpsee 0.24 + frame-decode 0.9) | — | 364 | — | — |
| reference: CortexLM/buyback (subxt 0.51.0 + jsonrpsee 0.24.11) | — | 475 | — | — |

The 42 crates subxt adds over the hand-rolled set are mostly codegen/proc-macro machinery that is compiled **even when the macro is not used** (`subxt-macro`, `subxt-codegen`, `scale-typegen`, `syn`, `regex`, `heck`, `proc-macro-error2`), wasm-bindgen shims, `bitvec`, `secp256k1(-sys)` (from subxt-signer `ecdsa`), `scrypt`/`pbkdf2`/`password-hash`, `keccak-hash`, `impl-serde`, `derive-where`, `web-time` (`comm -13 F/handrolled-check.crates.txt F/subxt-dynamic-check.crates.txt`). Both graphs contain `ring` (rustls) and subxt's contains `serde_yaml 0.9.34+deprecated` via frame-decode's `legacy-types`.

### 1.9 Alternatives

#### 1.9.1 Hand-rolled: jsonrpsee + frame-metadata + scale-value/scale-decode (mev-scanner / buyback style)

- `U/sam0x17_mev-scanner/Cargo.toml`: `subxt = "0.44"` for `OnlineClient`/`SubstrateConfig`/`Events`, plus direct `jsonrpsee 0.24` (`ws-client`,`macros`), `frame-decode 0.9` (`decode_extrinsic_current`), `frame-metadata 23`, `scale-value 0.18`, `sp-crypto-hashing`; a single 2,259-line `main.rs` (`src/main.rs:1-31` imports). It is read-only (decode extrinsics/events); it never signs.
- `U/CortexLM_buyback`: subxt 0.51 **dynamic** API for storage/runtime calls/tx (`src/chain.rs:17-19, 436-530, 703, 1140-1217`) with a hand-built jsonrpsee `WsTransportClientBuilder` so it can add an `Authorization: Bearer` header and a 15 s connect timeout, then `subxt::rpcs::RpcClient::new(client)` + `LegacyBackend` (`src/chain.rs:312-358`). Note it locks `jsonrpsee 0.24.11` with `subxt 0.51.0` (`Cargo.lock`); on 0.51.1 that must become 0.26.x.
- What a fully hand-rolled client must re-implement (all present in subxt/frame-decode): V4 signed/V5 general encoding and signer payloads incl. the >256-byte blake2 hashing rule and extension skipping (`extrinsic_encoder.rs:405-520, 1216-1316`), mortality/era, nonce retrieval, metadata-per-spec caching, `author_submitAndWatchExtrinsic` state machine, event decoding with per-phase filtering, storage key hashing (`twox64concat`/`blake2_128concat`/`identity`) and paged iteration, runtime API arg encoding/result decoding, dispatch-error → module-error resolution. The `bittensor-core` crate did all this in ≈4,000 lines (`CORE/src/client.rs` 1,770 lines, `codec/*` ≈2,500 lines) with a blocking transport.
- Verdict: use hand-rolled `frame-decode`/`scale-value` **for decoding paths where subxt's typed API is awkward** (historic extrinsics, generic explorers) and for a `no_std`/wasm "chain-defined" crate; do not re-implement the client.

#### 1.9.2 `bittensor-core`'s own runtime/codec engine

Facts: `publish = false`, edition 2021 (`CORE/Cargo.toml:1-7`); transport is `reqwest::blocking` HTTP ("adds no second async runtime"; `CORE/src/client.rs:1-16`); depends on `sp-core` from the RaoFoundation polkadot-sdk fork (`sp-core 37.0.0` git, `MONO/Cargo.lock`), `frame-metadata` workspace pin, `tle` git rev + vendored `w3f-bls` via `[patch.crates-io]` (`MONO/Cargo.toml:299-310, 335`), optional C deps (`sodiumoxide`, vendored OpenSSL for `fernet`, `hidapi`) behind `host` (`CORE/Cargo.toml` `[features] host`). A downstream crate must replicate the `[patch.crates-io]` and cannot be published to crates.io (`RESEARCH/feasibility-bittensor-core-gitdep.md`). Valuable as an **oracle** (golden fixtures, shape corpus, keyfile and btauth vectors — `RESEARCH/B-bittensor-core-rust.md` §7) and as a reference for the chain-defined formats, not as a dependency. Also: crates.io `sp-core` is at 43.0.0 (2026-07-02) vs the fork's 37.0.0 → two incompatible `sp-core`s if rtao ever pulled both; subxt avoids the problem by not depending on `sp-core` at all.

#### 1.9.3 smoldot light client

- Pinning: `subxt-lightclient 0.51.1` → `smoldot-light ^1.3.1` (native) / `smoldot ^2.1.0` (web) (`R/subxt-lightclient-0.51.1/Cargo.toml`); crates.io `smoldot-light` is 3.0.0 and `smoldot` 5.0.0 (both 2026-10-07; repository now `paritytech/smoldot`, old `smol-dot/smoldot` redirects to `paritytech/smoldot-archive`; `T/crates/smoldot-light.json`, GitHub API). Enabling `light-client` adds the smoldot + `wasmi` graph (visible in the lockfile: `smoldot 2.2.0`, `smoldot-light 1.3.2`, `wasmi 0.40.0`).
- Finney specifics: `MONO/chainspecs/raw_spec_finney.json` (12.1 MB) has top-level keys `bootNodes` (one entry: `/dns/bootnode.finney.chain.opentensor.ai/tcp/30333/ws/p2p/12D3KooWRwbMb85RWnT8DSXSYMWQtuDwh4LJzndoRrTDotTR5gDC`), `badBlocks`, `codeSubstitutes` (1 entry), `grandpaWarpSyncCheckpoint` (custom extension: `finalizedBlockHeader` + `grandpaAuthoritySet`), and **no `lightSyncState`**. The node code special-cases finney's GRANDPA warp sync with `Config::OneTimeInitialSetId(3)` for genesis `0x2f0555cc…` and reads the checkpoint "from the chain-spec extension" (`MONO/node/src/service/grandpa_warp_sync.rs:8-33`; `warp_sync_checkpoint.rs:16-19`). Consensus is Aura + GRANDPA (`MONO/runtime/src/lib.rs:366-374`).
- **[inferred]** stock smoldot (which only understands standard `lightSyncState`) cannot warp-sync finney because the authority-set-id history is discontinuous; it would have to sync from genesis through a single bootnode. Not tested here. Decision: do not plan a light-client mode; keep `light-client` out of rtao's default features.

### 1.10 Decision matrix

Scores 1 (worst) – 5 (best). "Upstream-tracking friction" = effort when subtensor changes types/calls/extensions; "subxt-tracking friction" = effort when subxt releases a breaking minor.

| Criterion | subxt static codegen (macro or committed `subxt-codegen` output) | subxt dynamic (`dynamic::storage::<K,V>` + `DecodeAsType` structs) | hand-rolled (jsonrpsee + frame-decode + scale-value) | `bittensor-core` codec/client (git dep) |
|---|---|---|---|---|
| Correctness of signing / extensions | **5** — frame-decode encoder, tested; feasibility signed + validated on finney | **5** — same encoder | **2** — must re-implement V4/V5 payloads, extension skipping, mortality; high-risk surface | 3 — works upstream (112-test e2e), but blocking HTTP, no watch stream semantics for async validators |
| Correctness of reads (type drift detection) | **5** — 32-byte validation hash per address (`storage/address.rs:89-93`) + compile-time field types | 3 — names are strings, `validation_hash: None`; drift surfaces as decode errors at runtime | 3 — same as dynamic | 3 — name-based descriptors |
| Runtime performance | **5** — static `DecodeAsType` into concrete types; no `Value` tree | 4 — `decode_as::<T>()` is equally fast; `Value` only if used | 4 — same crates | 3 — `Value` shape engine (+rayon batch) |
| Compile time / weight | 3 — 228 crates; +28 CPU-s for the 60k-line module (can be trimmed with `runtime_types_only` + selective use) | 4 — 228 crates, 155 CPU-s | **5** — 186 crates, 95 CPU-s | 2 — 207/330 crates + polkadot-sdk fork, 110 s `cargo check` |
| Upstream-tracking friction (subtensor changes) | 3 — regenerate; renamed/moved runtime types ripple into rtao unless hidden behind `substitute_type`/own newtypes; **but** every change is a reviewable diff of generated code | **4** — resilient to type moves; breaks only on renamed entries/fields; needs a pinned-metadata validation test to catch silently | 4 — same as dynamic | 2 — tied to the monorepo's exact revision |
| subxt-tracking friction (subxt breaking minors, 8 in 23 months) | 2 — generated API surface changes with subxt (0.43 derives, 0.50 redesign) | 3 — smaller API surface used | **5** — no subxt; only frame-decode/scale-* minors | 5 — none |
| Type safety for subnet authors | **5** — typed calls/storage/runtime APIs, `NetUid` newtypes | 2 — strings + tuples; must hand-write every struct | 2 | 3 — `IntentCall` strings + `Value` |
| Publishability (crates.io) | 5 | 5 | 5 | **1** — git-only, `[patch]` required |
| no_std / wasm "chain-defined" core | 2 — subxt is std | 2 | **4** — frame-decode/scale-* are no_std-capable | 3 — wasm32 (std) only |

**Recommendation:** subxt 0.51 as the client, **static codegen generated by an `xtask` with `subxt-codegen` from pinned metadata and committed** (reviewable diffs on each runtime upgrade, no proc-macro in the build, rust-analyzer-friendly), with `substitute_type` mapping `NetUid`/`TaoBalance`/`AlphaBalance`/`substrate_fixed::*`/`PerU16` to rtao newtypes so the public API never names `runtime_types::…`; rtao's "intent" layer (set_weights, commit_crv3, serve_axon replacement, stake ops, hyperparams) is hand-written over the generated calls; `dynamic::*` is exposed for explorer-style tooling and for reads that must survive type moves; `frame-decode`/`scale-value` directly for extrinsic/event decoding in the mev-scanner style. If the architects prefer the macro form, the cost is the same crate set and ≈10 s wall per clean build.

---

## 2. Async / network stack

- **tokio 1.53.2** (2026-10-03, MSRV 1.71) — subxt requires `tokio/rt` under `jsonrpsee`+`runtime` (`R/subxt-0.51.1/Cargo.toml [features] runtime`). `T/crates/tokio.json`.
- **jsonrpsee 0.26.1** (2026-09-30, edition 2024, MSRV 1.87.0; `R/jsonrpsee-0.26.1/Cargo.toml`) — the only version compatible with subxt 0.51.1 (`subxt-rpcs 0.51.1` requires `^0.26.1`; 0.51.0 required `^0.24.5` — crates.io dependency lists). Use it directly only to build a custom WS transport (headers, timeouts) and hand it to `subxt::rpcs::RpcClient::new` as buyback does (`U/CortexLM_buyback/src/chain.rs:312-352`). Features: `client-ws-transport-tls` = `ws` + `tls-rustls-platform-verifier` (rustls 0.23 with OS trust store), `ws-client`, `async-client`; the WS implementation is `soketto 0.8.1` (`R/jsonrpsee-client-transport-0.26.1/Cargo.toml`). The v11 Python transport uses the `websockets` asyncio client (`grep` of `MONO/sdk/python/bittensor/_transport/*.py`, import lines 38-39), so WS JSON-RPC is the common denominator.
- **tokio-tungstenite 0.30.0** (2026-07-11) — not needed for RPC; SN2 uses it for its own validator↔miner WebSocket protocol (`U/inference-labs-inc_subnet-2/Cargo.toml`: `tokio-tungstenite 0.24` + `axum 0.7`). Only relevant if rtao offers a WS transport for subnet traffic.
- **axum 0.8.9** (2026-04-14, MSRV 1.80) + **tower 0.5.3** + **tower-http 0.7.1** (2026-08-31; `trace`, `timeout`, `limit`, `cors`, `request-id`) + **http 1.5.0** + **hyper 1.12.0** / **hyper-util 0.1.21** — for the btauth/1 server side. btauth/1 (`MONO/sdk/python/bittensor/http_auth.py`): headers `X-Bittensor-Version/Hotkey/Receiver/Nonce/Crypto/Signature` (`:58-66`), payload = sha256 hex of the raw body + decimal nanosecond nonce + sender ss58 + receiver + method/path (`:157-186`, "The wire format is deliberately language-neutral — sha256 over the raw body", `:36`), `nonce = time.time_ns()` (`:213`), replay cache of (hotkey, nonce) with 60 s retention (`:116-154`). Rust needs: `sha2 0.10`, hex, a tower middleware that buffers the body (`axum::body::to_bytes` with a limit), verifies with `schnorrkel`/`ed25519-dalek` (and the `<Bytes>` wrapper fallback, §1.6), and a `DashMap`/LRU nonce store. CortexLM already pairs axum 0.8 + tower-http 0.6 + reqwest 0.12 for this role (`U/CortexLM_bittensor-rs/bittensor-axon/Cargo.toml`), and SN2 ships a native axum validator (`U/inference-labs-inc_subnet-2/crates/sn2-validator/Cargo.toml`).
- **reqwest 0.13.5** (2026-09-08, MSRV 1.85, hyper 1.x) with `default-features = false, features = ["rustls", "json"]` (buyback's working 0.13 feature set, `U/CortexLM_buyback/Cargo.toml`) for the btauth/1 client and drand round fetches. `bittensor-core` still uses `reqwest 0.12.15` blocking (`CORE/Cargo.toml:57-60`).
- TLS: `rustls 0.23.45` (stable; 0.24.0-dev.1 exists), `tokio-rustls 0.26.6`, `ring 0.17.14` present in both measured graphs (`F/*.crates.txt`). Pin one TLS backend (`ring`) across jsonrpsee/reqwest/axum to avoid `aws-lc-rs` duplication.
- QUIC alternative for subnet traffic: `btlightning 0.2.15` (quinn 0.11 + rustls 0.23, `U/inference-labs-inc_btlightning/crates/btlightning/Cargo.toml`) — third-party, out of scope for the core.

---

## 3. Cryptography

### 3.1 sr25519 / ed25519 / hashing

- `schnorrkel 0.11.5` (2025-07-14; deps curve25519-dalek ^4.1.3, merlin 3, sha2 0.10, rand_core 0.6 — crates.io deps today). Context `b"substrate"` for sign/verify (§1.6). Repo now `paritytech/schnorrkel` (pushed 2026-08-30).
- `ed25519-dalek 2.2.0` (curve25519-dalek 4, sha2 0.10) — same generation as schnorrkel and subxt-signer; `3.0.0` (2026-07-06) would add curve25519-dalek 5 + sha2 0.11 + rand_core 0.10 duplicates.
- Hashing: `blake2 0.10` (ss58 checksum, `blake2_128concat`/`blake2_256` via `sp-crypto-hashing 0.1.0`, which subxt and frame-decode already use), `sha2 0.10` (btauth body hash, Fernet legacy), `sha3 0.10` (only if keccak needed; subxt pulls `keccak-hash 0.11` for eth configs). RustCrypto 0.11-generation crates (`sha2 0.11.1`, `blake2 0.11.0`, `sha3 0.12.0` — all 2026) are not yet used by subxt; adopting them now duplicates the graph.

### 3.2 ss58 and AccountId32

Bittensor uses the generic Substrate prefix 42 (`CORE/src/keys/mod.rs:31` `DEFAULT_SS58_FORMAT = 42`; encoding at `:54-70` is "byte-identical to sp-core's `to_ss58check_with_version`"). `subxt::utils::AccountId32` encodes/decodes with `SUBSTRATE_SS58_PREFIX = 42` and `b"SS58PRE"` using `base58 0.2.0` + `blake2 0.10.6` (`R/subxt-utils-accountid32-0.51.1/src/lib.rs:65,129`; `Cargo.toml`). `ss58-registry 1.51.0` (2024-10-07) is unnecessary unless rtao wants to render non-42 prefixes; `bs58 0.5.1` is the common base58 crate if rtao implements its own `Ss58` type (bittensor-core has its own `keys/base58.rs`).

### 3.3 Keyfile formats (must match btwallet / bittensor-core bit-for-bit)

Source of truth: `CORE/src/keyfiles/mod.rs` (identical scheme in the archived `U/btwallet/src/keyfile.rs:25,330-338,396-402`).

1. **`$NACL` (current):** `derive_key` = `sodiumoxide::crypto::pwhash::argon2i13::derive_key(&mut key[32], password, NACL_SALT, OPSLIMIT_SENSITIVE, MEMLIMIT_SENSITIVE)` with `NACL_SALT = b"\x13q\x83\xdf\xf1Z\t\xbc\x9c\x90\xb5Q\x879\xe9\xb1"` (16 bytes, fixed) (`mod.rs:22, 59-71`). libsodium constants: `crypto_pwhash_argon2i_ALG_ARGON2I13 = 1`, `SALTBYTES = 16`, `OPSLIMIT_SENSITIVE = 8`, `MEMLIMIT_SENSITIVE = 536870912` (`R/libsodium-sys-0.2.7/src/sodium_bindings.rs:117,121,131,132`). Ciphertext layout: `b"$NACL" || nonce[24] || secretbox::seal(data, nonce, key)` (`mod.rs:86-94`); decrypt reads the nonce from bytes 5..29 (`:74-84`).
   **Pure-Rust equivalent:** `argon2::Argon2::new(Algorithm::Argon2i, Version::V0x13, Params::new(m_cost = 536870912/1024 = 524288 KiB, t_cost = 8, p_cost = 1, Some(32)))` then `hash_password_into(password, &NACL_SALT, &mut key)` (`R/argon2-0.5.3/src/algorithm.rs:40` `Argon2i = 1`, `version.rs:18` `V0x13`, `params.rs:42-61` defaults differ: m=19 MiB, t=2, p=1 — so parameters must be explicit). The memlimit→m_cost (bytes/1024) and lanes = 1 mapping is libsodium's `crypto_pwhash_argon2i` behaviour **[well known; libsodium source not on disk, not verified here]**. Then `crypto_secretbox::XSalsa20Poly1305::new(&key).decrypt(nonce, ct)` (same primitive subxt-signer uses, `R/subxt-signer-0.51.1/src/polkadot_js_compat.rs:9-12`). Cost per derivation: 512 MiB RAM and t=8 passes — cache the derived key per password within a process. `argon2 0.6.0` (2026-08-27) is API-compatible but moves to blake2 0.11/edition 2024 and removed the `std` feature (RustCrypto changelog [web]); prefer 0.5.3 while on the 0.10 generation. `sodiumoxide 0.2.7` (2021-06-24, unmaintained) should not be used.
2. **Legacy Fernet (`gAAAAA…`):** key = PBKDF2-HMAC-SHA256(password, `LEGACY_SALT = b"Iguesscyborgslikemyselfhaveatendencytobeparanoidaboutourorigins"`, **10,000,000** iterations) → URL-safe base64 → `Fernet::decrypt` (`mod.rs:23, 139-151`). `fernet 0.2.2` (2024-05-16) needs OpenSSL (bittensor-core vendors it, `CORE/Cargo.toml:27-32`); Fernet is AES-128-CBC + HMAC-SHA256 over a versioned token, trivially re-implementable with `aes`/`cbc`/`hmac` RustCrypto crates if a pure-Rust build is wanted **[format from the Fernet spec; not re-verified here]**. Feature-gate it (`legacy-keyfiles`).
3. **`$ANSIBLE_VAULT`:** `ansible-vault 0.2.1` (2022-10-28) decrypt only (`mod.rs:164-169`). Feature-gate.
4. **polkadot-js JSON keystore:** scrypt + XSalsa20-Poly1305 — `subxt-signer`'s `polkadot-js-compat` feature or `CORE/src/keys/encrypted_json.rs`.
5. **Env-var password obfuscation:** base64(XOR(password, env_var_name)) (`mod.rs:97-137`) — trivial.
6. Detection is by prefix (`$NACL`, `$ANSIBLE_VAULT`, `gAAAAA`; `mod.rs:29-56`).

Mandatory test: round-trip and cross-decrypt real keyfiles produced by btcli/btwallet (`RESEARCH/B-bittensor-core-rust.md` §7 lists upstream golden fixtures) — the unit test `nacl_roundtrip` in `CORE/src/keyfiles/mod.rs:389-416` shows the expected shape.

### 3.4 Mnemonics

`bip39 2.2.2` (edition 2018) is used by subxt-signer (`^2.1.0`), bittensor-core (`2.0`), btwallet (`2.2.2`). `bip39 3.0.0` was published 2026-09-17; don't adopt until subxt does (`T/crates/bip39.json`). The seed derivation must be the substrate variant (§1.6), not BIP-39's phrase-based PBKDF2.

### 3.5 Timelock (CRv3) — `tle` + `w3f-bls`

- bittensor-core's encrypt path: `tle::<TinyBLS381, AESGCMStreamCipherProvider, OsRng>(…)` with `w3f_bls::EngineBLS` (`CORE/src/timelock/mod.rs:26-32, 125`), inheriting the workspace pins "so ciphertexts never drift from what pallet-drand can decrypt" (`CORE/Cargo.toml` timelock section).
- Pins: `tle = { git = "https://github.com/ideal-lab5/timelock", rev = "5416406cfd32799e31e1795393d4916894de4468" }` (`MONO/Cargo.toml:310`), resolving to `tle 0.1.0` (`MONO/Cargo.lock`). At that rev `tle/Cargo.toml` requires `w3f-bls = "=0.1.3"`, `ark-{ff,ec,serialize,bls12-381} 0.4.0`, `aes-gcm 0.10.2`, `chacha20poly1305 0.10.1` (lines 19-40, fetched from GitHub today). The rev is dated 2024-11-15; the repo (4 stars, pushed 2025-09-05) has since renamed the crate to `timelock 0.3.0` on arkworks 0.5 (root `Cargo.toml:71-79` at HEAD).
- `w3f-bls`: the monorepo vendors upstream 0.1.3 "plus one patch removing std usage from double_pop.rs so the crate builds for the runtime's no-std targets" (`MONO/vendor/w3f-bls/Cargo.toml:1-6`) and patches crates.io (`MONO/Cargo.toml:335`); the fork's `sp-core` separately pulls crates.io `w3f-bls 0.1.9`, so two copies coexist in the monorepo (`MONO/Cargo.lock`). crates.io `w3f-bls 0.2.0` (2026-04-03, repo now `paritytech/bls`) is on arkworks 0.5.
- crates.io `tle 0.1.0` (2023-11-26, 1,735 downloads) predates the pinned rev; whether its ciphertext format equals the pinned rev's is **[not verified]**.
- Options for rtao: (a) depend on the pinned git rev + crates.io `w3f-bls =0.1.3` (std build does not need the no-std patch) — correct but **unpublishable**; (b) vendor/port the IBE encrypt path (`tle::tlock::tle` is ~100 lines over `w3f-bls` + `ark-bls12-381` + AES-GCM) into `rtao-timelock`, pin `w3f-bls =0.1.3` + `ark-* 0.4`, and prove equivalence by having `pallet-drand` on localnet decrypt rtao ciphertexts (the only test that matters). Note arkworks 0.4 vs 0.5 will coexist in the graph if anything else (e.g. subxt `light-client` → smoldot) brings 0.5; keep `light-client` off. The sibling B report §2.4.1 also flags a verified divergence in the epoch-schedule port.

### 3.6 ML-KEM (MEV shield)

Pallet-side pins: `ml-kem 0.2.2`, `chacha20poly1305 0.10` (`MONO/Cargo.toml:313-314`); client side `CORE/Cargo.toml` mlkem section (`ml-kem 0.2.1`, `chacha20poly1305 0.10`, `twox-hash 2.1`). crates.io: `ml-kem 0.3.2` (2026-05-10; 0.3.0 changelog: edition 2024/MSRV 1.85, `kem 0.3`, `sha3 0.11`, `rand_core 0.10`, `getrandom 0.4`, `hybrid-array 0.4`, Wycheproof vectors), `chacha20poly1305 0.11.0` (aead 0.6). FIPS 203 wire format is the same across versions **[inferred from the changelog; not KAT-verified here]**, but matching the pallet's generation (0.2.x + 0.10) avoids duplicate `sha3`/`rand_core`/`getrandom` stacks and is what agcli and bittensor-core do (`U/unarbos_agcli/Cargo.toml`). Feature-gate as `mev-shield`.

### 3.7 RFC-0078 merkleized metadata

`merkleized-metadata 0.5.1` (2026-06-12; deps `blake3 1.5`, `frame-metadata >=21`, `scale-decode 0.16`, `scale-info 2.11.6`, `array-bytes 9.1.2`; `R/merkleized-metadata-0.5.1/Cargo.toml`). Needed only for `CheckMetadataHash` = Enabled or Ledger-style signing proofs (`CORE/src/digest/mod.rs`). Optional feature.

---

## 4. Numerics: `substrate-fixed` in runtime API types

- The runtime returns fixed-point values in several runtime-API structs: `DynamicInfo.moving_price: I96F32` (`MONO/pallets/subtensor/src/rpc_info/dynamic_info.rs:5,32`), `DelegateInfo` computations in `U64F64` (`delegate_info.rs:5,28-51`), basket info `spot_price/display_price/stake_price/staker_yield/staker_twr/bag_index/stake_index/display_shares/display_beta: U64F64` (`basket_info.rs:6,81-148`), `SubnetHyperparams.alpha_sigmoid_steepness: I32F32` and `HyperparamValue::{I32F32, U64F64}` (`subnet_info.rs:6,118,141-142`). Pin: `substrate-fixed = { git = "https://github.com/encointer/substrate-fixed.git", tag = "v0.6.0" }` (`MONO/Cargo.toml:249`); crates.io has `substrate-fixed 0.6.0` published the same day the repo was last pushed (2025-05-01) — likely identical **[not diffed]**.
- In metadata/codegen the types appear as `substrate_fixed::FixedI128<Frac>` / `FixedU128<Frac>` / `FixedI64<Frac>` with a single SCALE field `bits: i128|u128|i64` and the fractional-bit count encoded as a typenum chain, e.g. `FixedI128<UInt<UInt<UInt<UInt<UInt<UInt<UTerm,B1>,B0>,B0>,B0>,B0>,B0>>` = `0b100000` = 32 fractional bits = I96F32 (`META/finney_codegen.rs:60162-60200`, `:56851`, `:57110`).
- Representation options for a client: (1) keep the generated generic structs and add an extension trait that reads `bits` and divides by `2^FRAC` (FRAC known per field); (2) `substitute_type(path = "substrate_fixed::FixedI128<A>", with = "rtao::fixed::FixedI128<A>")` with rtao's own `EncodeAsType`/`DecodeAsType` newtype whose decode visitor derives FRAC from the typenum type path (works for dynamic decoding too); (3) map to the `fixed` crate (`fixed 1.31.0`, 2026-03-20, MSRV 1.93 — too high for a 1.88 MSRV) whose `FixedI128<U32>` has the same two's-complement bit layout **[inferred: substrate-fixed is a fork of `fixed`; not verified]**. Recommendation: (2) with `const FRAC: u32` newtypes `I96F32`, `U64F64`, `I32F32` offering `to_f64()`, `from_bits()`, `to_bits()`, `Display` with exact decimal rendering, and proptests against `bits / 2^frac`. Avoid `f64` in any consensus-relevant math (weights normalization is u16-based anyway).

---

## 5. Config / CLI / observability

- `clap 4.6.7` (2026-09-14, MSRV 1.85, edition 2024) with `derive`, `env`, `wrap_help` — universal in the ecosystem (agcli, btt, SN2, buyback, CortexLM).
- Layered config: `config 0.15.27` (2026-09-30, MSRV 1.88, edition 2024, actively maintained) vs `figment 0.10.19` (last release 2024-05-17). Both do TOML + env overlays with serde; `config` is the maintained choice; `figment` has nicer profiles/provenance. Either is fine; prefer `config` for maintenance risk. Env conventions should mirror the Python SDK's (e.g. the wallet password env-var scheme in §3.3 item 5; network names `finney|test|local`).
- `tracing 0.1.44` + `tracing-subscriber 0.3.23` (`env-filter`, `json`, `fmt`) — subxt/jsonrpsee emit `tracing` events already (`R/subxt-0.51.1/Cargo.toml [dependencies.tracing]`).
- Metrics: `metrics 0.24.6` + `metrics-exporter-prometheus 0.18.3` (2026-04-30) — SN2 uses this pair (`U/inference-labs-inc_subnet-2/Cargo.toml`: 0.23/0.15).

---

## 6. Testing and tooling

- **Unit/async:** `#[tokio::test]` with `tokio/test-util` for paused time (btlightning does this, `U/inference-labs-inc_btlightning/crates/btlightning/Cargo.toml`).
- **Property tests:** `proptest 1.12.0` (2026-10-10, MSRV 1.88) — used by agcli (`extrinsic_encoding` proptests) and btlightning; `quickcheck 1.1.0` (2026-02-10, MSRV 1.85) as a lighter alternative. SCALE round-trips of every rtao newtype and of the fixed-point types are the obvious targets.
- **Snapshots:** `insta 1.49.0` + `cargo-insta 1.49.0` (2026-10-03) for decoded metadata/intent descriptors and generated-code diffs.
- **HTTP mocks:** `wiremock 0.6.5` (2025-08-24, edition 2024) for btauth/1 client tests and drand fetches (CortexLM and one-covenant both use it).
- **Mock RPC:** `subxt-rpcs` feature `mock-rpc-client` (`R/subxt-rpcs-0.51.1/Cargo.toml [features]`; CortexLM uses it in `bittensor-chain/Cargo.toml`) to unit-test backends without a node.
- **Localnet containers:** `testcontainers 0.28.0` (2026-08-06, MSRV 1.88) `GenericImage` over the published image `ghcr.io/raofoundation/subtensor-localnet` (`MONO/.github/workflows/docker-localnet.yml:92-98` derives the name from the repo; tags policy in `:1-11`: branch tags `main|devnet|testnet|mainnet`, immutable `:v<spec_version>` from mainnet builds, `:latest` = `:main` (`MONO/.github/scripts/resolve-localnet-image.sh:37-41`), `pr-N` manual). Tags present today (`T/tags_raofoundation_subtensor-localnet.json`): `devnet, latest, main, mainnet, testnet, v432, v473, v475, v477`, 57 `sha-<40hex>` (incl. `sha-850f2755…` = this checkout), run-scoped `build-*-{amd64,arm64}`, legacy `ci-*`. The image bundles both `fast-runtime` (0.25 s blocks) and `non-fast-runtime` binaries for amd64+arm64 (`docker-localnet.yml:120-137`; `MONO/Dockerfile-localnet:99-113`), exposes 9944/9945/9946 + 30334-30336, entrypoint `/scripts/localnet.sh`, `CMD ["True"]` (fast blocks; pass `False` for 12 s) (`Dockerfile-localnet:89-96`; `MONO/docs/guides/local-development.mdx:13-19, 41-45`). The sibling H report §1.4 documents healthcheck semantics, funded dev accounts, `BT_DEFAULT_TOKEN_WALLET`, and the upstream Rust harness's "wait for `Imported #1`" readiness rule. Pin by digest in rtao's test manifest; the upstream harness's own default tag `monorepo-sdk` does not exist (H §1.4).
- **Runner:** `cargo-nextest 0.9.148` (2026-10-08) — upstream runs `cargo nextest run --workspace --all-features` + `cargo test --doc` (`MONO/.github/workflows/check-rust.yml:261-264`).
- **Supply chain / hygiene:** `cargo-deny 0.20.2` (licenses, bans incl. duplicate-version policy for `jsonrpsee`/`sha2`/`blake2`, advisories), `cargo-audit 0.22.2` (the exact pin upstream uses, `MONO/.github/workflows/cargo-audit.yml:36-56`), `cargo-semver-checks 0.51.0` (2026-10-03; tool needs Rust 1.93 to build — fine for CI, irrelevant to lib MSRV), `cargo-hack 0.6.45` (feature power-set), `cargo-machete 0.9.2`, `cargo-udeps 0.1.61`, `zepter 1.88.1` (feature propagation — upstream runs `zepter run check`, `check-rust.yml:381-408`), `typos-cli 1.51.1`, `taplo-cli 0.10.0`.
- **Benchmarks:** `criterion 0.8.2` (2026-02-04, MSRV 1.86) or `divan 0.1.21` (2025-04-10). Targets: metagraph decode (`get_metagraph` payloads), weights encode, keyfile KDF (fixed cost), btauth sign/verify throughput.
- **Coverage:** `cargo-llvm-cov 0.9.1`.
- Upstream's own Rust CI as a template: `cargo fmt --check`, `clippy --workspace --all-targets -D warnings` per feature set, `cargo check` fail-on-warnings, wasm32 checks for the portable core (`check-rust.yml:131, 167, 216-221, 297-302`), `cargo doc --document-private-items` deployed to Pages (`rustdocs.yml:32`).

---

## 7. Documentation

- **docify 0.4.1** (2025-02-21; features `default = []`, `generate-readme`; edition 2024 per crates.io; repo `sam0x17/docify`, last push 2025-02-21, 11 open issues — `T/crates/docify.json`, GitHub API). Macros: `#[docify::export]`, `#[docify::export_content]`, `docify::embed!("path.rs", item)` (renders as an `ignore` block), `docify::embed_run!` (runs as a doctest), `docify::compile_markdown!` for `<!-- docify::embed!("…", item) -->` comments in Markdown files/directories; the crate regenerates its own README with `#[cfg(all(doc, feature = "generate-readme"))] compile_markdown!("README.docify.md", "README.md");` (docify `src/lib.rs:79-80` [web]; docs.rs/docify/0.4.1 [web]). Plan: every example in `rtao`'s docs is an `#[docify::export]`ed test in `examples/` or `tests/`, embedded with `embed_run!` where it can run offline (SCALE, keys, btauth) and `embed!` where it needs a node; README/mdbook pages compiled with `compile_markdown!`. Caveat: docify has not had a release since 2025-02 and the open-issue count suggests edge cases; since the user authors it, that is a feature, not a risk.
- rustdoc: `#![doc = include_str!("../README.md")]`, `#![deny(missing_docs)]` (subxt does: `R/subxt-0.51.1/Cargo.toml [lints.rust] missing_docs = "deny"`), docs.rs metadata `all-features`/`rustdoc-args = ["--cfg", "docsrs"]`.
- **mdbook 0.5.4** (2026-07-06, MSRV 1.88) for the guide; `mdbook test` can run Rust code blocks, but docify-compiled markdown keeps a single source of truth.

---

## 8. MSRV / edition

- Rust stable **1.99.0** (GitHub release tag published 2026-10-01; `rustc 1.99.0 (b940084d7 2026-09-28)` installed locally; releases.rs lists beta 1.100.0 for 2026-11-12 and nightly 1.101.0). Edition **2024** is the norm for the stack (subxt, jsonrpsee, clap, config, frame-decode, smoldot, argon2 0.6, blake2 0.11, …; the monorepo workspace is also edition 2024 with toolchain 1.89, `MONO/Cargo.toml [workspace.package]`, `MONO/rust-toolchain.toml`).
- MSRV floor imposed by dependencies: subxt 1.88, jsonrpsee 1.87, config 0.15 1.88, proptest 1.12 1.88, testcontainers 0.28 1.88, mdbook 1.88, zepter 1.88; btlightning 1.88; everything else lower. Recommend **MSRV = 1.88** declared in `rust-version`, tested in CI, with a policy "bump at most to stable−2 when a dependency forces it". `fixed 1.31` (1.93) and `cargo-semver-checks` (1.93, tool only) are the only >1.88 items encountered.

---

## 9. Release / CI

- **release-plz 0.3.172** (2026-10-12, very active; release PRs with changelog + per-crate versioning for workspaces) vs **cargo-release 1.1.6** (2026-09-16, MSRV 1.92). For a multi-crate workspace published in dependency order, release-plz's PR flow plus `cargo-semver-checks` is the lower-friction choice; cargo-release is fine for manual tagging. `cargo-dist 0.32.0` only if rtao ships binaries (CLI).
- GitHub Actions matrix (recommendation): `ubuntu-latest` (x86_64) + `ubuntu-24.04-arm` + `macos-14` for check/clippy/test; MSRV job on 1.88; `cargo hack --feature-powerset --depth 2 check`; `cargo deny check`; `cargo audit` (same advisory-ignore discipline as upstream's `cargo-audit.yml`); nightly job that (a) pulls `ghcr.io/raofoundation/subtensor-localnet:{devnet,testnet,mainnet}` and runs the e2e suite via testcontainers, (b) fetches live finney/testnet metadata and runs the extension/entry drift check (§1.5), (c) rebuilds the committed codegen against `MONO/main` metadata and opens a PR on diff. Docker is available on the Ubuntu runners only. Upstream's pipeline (`MONO/.github/workflows/{check-rust,cargo-audit,rustdocs,docker-localnet}.yml`) is a good reference for runner setup (sccache, rust-cache).

---

## 10. Risks

1. **subxt breaking cadence** — 8 breaking minors in 23 months, a redesign in 0.50 and a public-dependency bump (jsonrpsee 0.24→0.26, tower 0.4→0.5) inside patch 0.51.1 (§1.7). Mitigation: subxt types stay behind rtao traits (`ChainClient`, `Signer`), one adapter crate (`rtao-subxt`), lockfile committed, `cargo deny` bans duplicate `jsonrpsee`.
2. **Custom transaction extensions gaining payloads** — today they encode to zero bytes (§1.5); a non-empty implicit or an authorization extension would require a custom `Config` (issue #2276 open). Mitigation: startup guard + nightly drift job against `MONO/main`.
3. **`CombinedBackend` default fails on finney's public endpoint** (`-32601`); must construct `LegacyBackend` explicitly; `ChainHeadBackend` works but load-balancer behaviour is unverified for long subscriptions.
4. **Reconnecting client drops subscriptions** (`reconnecting_rpc_client/mod.rs:5-12`) — validators must resubscribe and re-pin `at_current_block()`.
5. **Timelock dependency is unpublishable as pinned** (git `tle` rev + `w3f-bls =0.1.3` + arkworks 0.4; upstream repo moved on to 0.3.0/ark 0.5; 4-star project, 1,735 crates.io downloads) — §3.5. Mitigation: vendored `rtao-timelock` validated against `pallet-drand` on localnet; keep behind a feature.
6. **`sp-core` version split** — fork `sp-core 37.0.0` (bittensor-core) vs crates.io 43.0.0 vs subxt's dev-only 36.1.0; never depend on `sp-core`; interop through raw bytes/ss58 strings.
7. **Vendored no-std `w3f-bls` + `[patch.crates-io]`** in the monorepo means `bittensor-core` cannot be consumed from crates.io; two `w3f-bls` copies already coexist upstream (`MONO/Cargo.lock`).
8. **RustCrypto generation split** (sha2/blake2/argon2/chacha20poly1305/ml-kem/ed25519-dalek majors in 2026) — pin the 0.10 generation to match subxt-signer; expect a coordinated bump when subxt moves.
9. **`sodiumoxide` is dead (0.2.7, 2021)**; pure-Rust replacement must be golden-tested against real `$NACL` keyfiles; Argon2i at 512 MiB/t=8 per unlock is slow by design — cache.
10. **Deprecated/C deps inside subxt's graph**: `serde_yaml 0.9.34+deprecated` via frame-decode `legacy-types` (cannot be turned off downstream), `secp256k1-sys` (avoidable: drop `ecdsa`), `ring` (rustls; acceptable). `cargo deny`/`audit` must carry documented ignores, as upstream does (`cargo-audit.yml:56-70`).
11. **Light client not viable for finney** (§1.9.3); subxt also lags smoldot by two majors.
12. **Static codegen churn**: generated `runtime_types::…` paths move when upstream reorganizes crates (`subtensor_runtime_common` newtypes) — hide behind `substitute_type` + rtao newtypes; commit generated code so diffs are reviewable.
13. **`CheckMetadataHash` disabled by default** in subxt; if the chain ever requires `Enabled`, rtao needs the merkleized digest path (§1.5).
14. **`frame-decode` fallback encodes `None` for unknown `Option<T>` extensions** (0.50.3) — silent but safe; worth a log line in the guard.
15. **Public endpoint variability**: the feasibility run and buyback's comment both show the WS endpoint advertising `archive_v1_*`/`chainHead_v1_*` but refusing some; test each supported endpoint per backend in CI.
16. **MSRV pressure**: subxt/jsonrpsee/config/testcontainers already sit at 1.87–1.88 and bump MSRV in minors (subxt: 1.74 → 1.81 → 1.85 → 1.88 across 2025-26); a "stable−2" policy would have been violated repeatedly, so define MSRV as "whatever subxt requires, at most stable−1".

---

## 11. Open questions

1. Will subxt accept non-empty implicit / authorization custom extensions (issue #2276)? Affects how much `Config` code rtao must own if subtensor extends `DrandPriority`/`CheckShieldedTxValidity`.
2. Does RaoFoundation intend to publish `bittensor-core` (blocked by the `w3f-bls` patch and `publish = false`)? If yes, rtao could reuse its keyfile/timelock modules instead of porting.
3. Is crates.io `tle 0.1.0` byte-compatible with the pinned 2024-11-15 git rev (and with `timelock 0.3.0`)? Needs a ciphertext round-trip test through `pallet-drand`.
4. Which public endpoints reliably serve `chainHead_v1_*` for long-lived subscriptions (ChainHeadBackend) vs legacy `state_subscribeStorage`? Only a single read was exercised per backend today.
5. Is a smoldot-format checkpoint (`lightSyncState`) obtainable for finney, and would smoldot accept finney's set-id discontinuity? Untested; low priority.
6. Are golden `$NACL`/legacy keyfiles with known passwords available in `MONO/sdk/python/tests/fixtures` (sibling B §7 says yes) so the pure-Rust KDF can be proven equal before release?
7. `substrate-fixed` crates.io 0.6.0 vs encointer git tag v0.6.0 — identical? (same date; diff before depending, or avoid the crate entirely with rtao newtypes).
8. ML-KEM: confirm KAT equality between `ml-kem 0.2.x` (pallet) and 0.3.x before ever upgrading the client generation.
9. Python interop target: should rtao's keyfile/btauth golden tests run the v11 Python package inside Docker (sibling H §3.5) given the 100%-Rust constraint applies to rtao itself, not its CI oracles?

---

## Appendix A — Measurement artifacts (reproducible)

- `F/handrolled-check/{Cargo.toml,src/main.rs}` — jsonrpsee 0.26 `state_getMetadata` → `RuntimeMetadataPrefixed` decode, blake2_256.
- `F/subxt-dynamic-check/{Cargo.toml,src/main.rs}` — subxt 0.51 `LegacyBackend`, `dynamic::storage::<(u16,),u16>("SubtensorModule","Tempo")`.
- `F/subxt-min-check` — same with `default-features = false`.
- `F/subxt-check` (from the earlier feasibility run) — `#[subxt::subxt(runtime_metadata_path = "../../meta/finney.scale")]`, typed storage + runtime API + `create_signed` + `validate()`; log `F/subxt-check/run.log`.
- Crate lists: `F/<name>.crates.txt` (`cargo tree -e normal --prefix none`); diff: `comm -13 F/handrolled-check.crates.txt F/subxt-dynamic-check.crates.txt` (44 lines incl. the roots).
- Timings: `F/build-timings.log` (host line + `/usr/bin/time -p cargo build --release` per crate, sequential, after `cargo clean`).
- crates.io snapshots: `T/crates/<crate>.json` (95 crates), `T/crates/subxt_versions.json`; fetch script `T/fetch_crates.sh`.
- GHCR tag lists: `T/tags_raofoundation_subtensor-localnet.json`, `T/tags_raofoundation_subtensor.json`.

## Appendix B — Version table (crates.io, fetched 2026-10-13; `max_stable_version (date, rust-version, edition)`)

| Crate | Version | Crate | Version |
|---|---|---|---|
| subxt | 0.51.1 (2026-09-30, 1.88, 2024) | subxt-core | 0.44.3 (2026-03-12, 1.85) — discontinued |
| subxt-signer / -rpcs / -codegen / -metadata / -lightclient / -cli | 0.51.1 (2026-09-30, 1.88) | jsonrpsee | 0.26.1 (2026-09-30, 1.87, 2024) |
| tokio | 1.53.2 (2026-10-03, 1.71) | tokio-tungstenite | 0.30.0 (2026-07-11, 1.85) |
| axum | 0.8.9 (2026-04-14, 1.80) | tower / tower-http | 0.5.3 (2026-01-12) / 0.7.1 (2026-08-31) |
| reqwest | 0.13.5 (2026-09-08, 1.85) | hyper / hyper-util / http | 1.12.0 (2026-10-06) / 0.1.21 / 1.5.0 |
| rustls / tokio-rustls | 0.23.45 (0.24.0-dev.1 pre) / 0.26.6 | schnorrkel | 0.11.5 (2025-07-14) |
| ed25519-dalek | 3.0.0 (2026-07-06, 1.85, cd5) ; 2.2.0 (cd4) | substrate-bip39 / bip39 | 0.6.1 (2026-03-26) / 3.0.0 (2026-09-17; use 2.2.2) |
| crypto_secretbox | 0.1.1 (2023-05-16; 0.2.0-pre.0) | argon2 | 0.6.0 (2026-08-27, 1.85); 0.5.3 recommended |
| sodiumoxide | 0.2.7 (2021-06-24) | tle / w3f-bls | 0.1.0 (2023-11-26) / 0.2.0 (2026-04-03) |
| ml-kem / chacha20poly1305 | 0.3.2 (2026-05-10) / 0.11.0 (2026-06-28); use 0.2.3 / 0.10 | merkleized-metadata | 0.5.1 (2026-06-12) |
| substrate-fixed / fixed | 0.6.0 (2025-05-01) / 1.31.0 (2026-03-20, 1.93) | ss58-registry / bs58 | 1.51.0 (2024-10-07) / 0.5.1 (2024-03-19) |
| sp-core / sp-runtime / sp-io / sp-keyring / sp-arithmetic | 43.0.0 / 49.0.0 / 49.0.0 / 49.0.0 / 29.0.0 | polkadot-sdk (umbrella) | 2609.0.0 (2026-10-08) |
| sp-crypto-hashing | 0.1.0 (2024-02-13) | frame-metadata | 23.0.1 (2025-12-09) |
| scale-info / scale-value / scale-decode / scale-encode / frame-decode / parity-scale-codec / scale-typegen | 2.11.6 / 0.18.2 / 0.16.2 / 0.10.1 / 0.18.1 (2026-08-06) / 3.7.5 / 0.12.0 | smoldot-light / smoldot | 3.0.0 / 5.0.0 (2026-10-07) |
| clap | 4.6.7 (2026-09-14, 1.85, 2024) | config / figment | 0.15.27 (2026-09-30, 1.88) / 0.10.19 (2024-05-17) |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | metrics / metrics-exporter-prometheus | 0.24.6 / 0.18.3 |
| proptest / quickcheck | 1.12.0 (2026-10-10, 1.88) / 1.1.0 | insta / cargo-insta | 1.49.0 / 1.49.0 (2026-10-03) |
| wiremock | 0.6.5 (2025-08-24) | testcontainers | 0.28.0 (2026-08-06, 1.88) |
| criterion / divan | 0.8.2 (1.86) / 0.1.21 | cargo-nextest | 0.9.148 (2026-10-08, tool MSRV 1.91) |
| cargo-deny / cargo-audit | 0.20.2 (1.88) / 0.22.2 (2026-06-05) | cargo-semver-checks | 0.51.0 (2026-10-03, tool MSRV 1.93) |
| cargo-hack / cargo-machete / cargo-udeps / cargo-llvm-cov | 0.6.45 / 0.9.2 / 0.1.61 / 0.9.1 | zepter / typos-cli / taplo-cli | 1.88.1 / 1.51.1 / 0.10.0 |
| docify | 0.4.1 (2025-02-21) | mdbook | 0.5.4 (2026-07-06, 1.88) |
| release-plz / cargo-release / cargo-dist | 0.3.172 (2026-10-12) / 1.1.6 (1.92) / 0.32.0 | sha2 / sha3 / blake2 / zeroize / secrecy | 0.11.1 / 0.12.0 / 0.11.0 (all 2026; use 0.10-gen) / 1.9.1 / 0.10.3 |
| serde / thiserror / anyhow / futures / async-trait | 1.0.229 / 2.0.21 / 1.0.104 / 0.3.34 / 0.1.92 | ansible-vault / fernet | 0.2.1 (2022-10-28) / 0.2.2 (2024-05-16) |
| Name availability | `rtao` — **not on crates.io** (404 today); `btwallet` 4.1.0, `bittensor-rs` 0.1.1, `bittensor-wallet` 0.1.0, `btlightning` 0.2.15 exist; `bittensor`, `bittensor-core`, `subtensor`, `crabtensor`, `rusttensor` do not | | |
