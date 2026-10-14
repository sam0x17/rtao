# E — Existing Rust efforts for Bittensor: critical review for rtao

Date: 2026-10-13. Reader: rtao architects (Rust/Substrate experts). Everything below is cited as `path:line`; paths are relative to these roots unless absolute:

- `$UP` = `SP/upstream`
- `$SUB` = `$UP/subtensor` (RaoFoundation/subtensor monorepo @ 850f275, runtime spec_version 478)
- `$META` = `SP/meta` (live finney metadata V16, spec 477)

All git clones under `$UP` are **shallow (depth 1)** (`.git/shallow` present in every repo), so commit counts, contributor lists, stars and issue lists were taken from the GitHub API on 2026-10-13 (`gh api repos/<owner>/<repo>` etc.); crates.io numbers from `https://crates.io/api/v1/crates/<name>` the same day. Facts marked **(inferred)** are my reading, not something I executed or observed directly.

---

## 0. Executive summary

There are eleven distinct Rust codebases that touch subtensor, plus the official `sdk/bittensor-core`. None of them is a viable foundation for rtao as-is, but several contain patterns worth copying:

| Verdict | Project | One-line reason |
|---|---|---|
| Learn from, do not depend on | `inference-labs-inc/subnet-2` (sn2-chain) | The only production-grade *subnet* in Rust (2,462 stars, 1,071 commits). Excellent transport/backend/reconcile patterns; but hard-codes CR version, uses a drand fork with the pre-stateful-epoch reveal-round math, and has a zero-signature fallback in its `Signer`. |
| Learn from (best metadata hygiene) | `CortexLM/buyback` | subxt 0.51, fully dynamic, `verify_metadata()` checks every call's argument names at startup; explicit nonce + 64-block mortality; journal-before-broadcast. Narrow scope (payments/buybacks), 0 stars, 13 commits. |
| Possible optional dependency | `inference-labs-inc/btlightning` (crates.io 0.2.15) | A clean QUIC transport with sr25519 handshake, cert-fingerprint binding, replay cache, flood defenses, tests, fuzz. Its own protocol (not btauth/1), SN2-shaped. |
| Keyfile reference only | `btwallet` 4.1.0 (archived) | Still the de-facto `$NACL` keyfile reference; has a known-broken legacy Fernet path (issue #185), unmaintained `sodiumoxide`, prints/prompts from library code. |
| Reference crypto, cannot be a crates.io dependency | `$SUB/sdk/bittensor-core` | `publish = false`, blocking `reqwest` JSON-RPC client (no tokio/websocket), spec explicitly excludes a Rust transport/codegen/btcli. Its `timelock` (drand v2 epoch simulation), `mlkem`, `keyfiles`, `digest` modules are the authoritative implementations and are pinned to the chain's crate revisions. |
| Avoid | `CortexLM/bittensor-rs` | Biggest surface (72k lines + 33k orphaned), stale metadata (spec 393), a "timelock" that is XOR with a key the committer holds, an MEV-shield format that does not match the pallet, an axon middleware that never verifies signatures, silent-default decoding. |
| Avoid | `one-covenant/bittensor-rs` (crates.io `bittensor-rs` 0.1.1) | subxt 0.44, network access in `build.rs`, committed error dump as `metadata/finney.rs`, encrypted-coldkey decrypt is a stub, commit hash uses keccak over the wrong tuple, `set_weights` returns "success" without inclusion. Used in production by Basilica (SN39). |
| Mine for ideas, do not depend | `unarbos/agcli` | 114k lines, 640 commits (414 by `cursoragent`), subxt 0.38, network in `build.rs`, own AES-GCM keyfile format mislabeled as btcli-compatible, wrong commit hash, pre-`key_hash` MEV-shield format. Good: `ArgUnit` semantic units, exit-code taxonomy, reads `CommitRevealWeightsVersion` from chain, mechanism-weights calls. |
| Mine for process ideas | `ErgodicLabs/btt` | Minimal dynamic subxt 0.50 CLI with the strictest supply-chain posture (pinned toolchain, dep-audit checksum tripwire, btcli keyfile compat verified in CI against pinned pynacl). Tiny chain surface. |
| Historical | `womboai/rusttensor` → `crabtensor` | 400–600 lines; `build.rs` fetches live metadata; hand-copied RPC types; the first "subxt for subtensor" lineage, still used (forked, patched) by Basilica. |
| Not an SDK | `k0jir0/hermes`, `taolk`, `subgrind`, `dataverse-cli`, indexers | No chain client, or hand-rolled JSON-RPC, or unrelated scope. |

**The gap none of them fills** (section 5): a published, semver'd, tokio-native Rust SDK that (a) is metadata-driven but *verifies* the runtime it is talking to, (b) implements the v11 `btauth/1` signed-HTTP scheme (zero Rust implementations exist today — `rg -il "btauth|X-Bittensor-Hotkey"` across all repos matches only agcli prose), (c) does CRv3 timelocked commits with the chain's *stateful* epoch model, (d) supports mechanism weights, and (e) ships a miner/validator framework plus a localnet test harness.

**The user's own `mev-scanner`** (section 6) is already written in the style that fits best: raw `jsonrpsee` + `frame-decode` + `scale-value`, metadata-version-agnostic, zero codegen, `anyhow` contexts, defensive fallbacks. The plan should keep that posture and add structure (crates, typed errors, `tracing`, tests), not replace it with static codegen.

---

## 1. Chain-side baseline used to grade every project

These are the facts a Bittensor Rust client must get right *today*; most reviewed projects fail at least one.

| Item | Fact | Evidence |
|---|---|---|
| Runtime version | finney/testnet metadata on disk is spec 477 (V16); monorepo HEAD is 478 | `$META/finney.json`; task brief |
| `set_weights` | `(netuid: NetUid(u16), dests: Vec<u16>, weights: Vec<u16>, version_key: u64)` | `subxt explore --file=$META/finney.scale pallet SubtensorModule calls set_weights` |
| CR v1 | `commit_weights(netuid, commit_hash: H256)`; `reveal_weights(netuid, uids, values, salt: Vec<u16>, version_key: u64)` | same, `calls commit_weights` / `reveal_weights` |
| CR v1 hash | `BlakeTwo256::hash_of(&(who, netuid_index: NetUidStorageIndex, uids, values, salt, version_key))` — a SCALE-encoded tuple that includes the **hotkey and netuid index** | `$SUB/pallets/subtensor/src/subnets/weights.rs:1524-1532` |
| CRv3 (timelock) | `commit_timelocked_weights(netuid, commit: BoundedVec<u8>, reveal_round: u64, commit_reveal_version: u16)`; `commit_crv3_weights` **no longer exists**; mechanism variants `commit_timelocked_mechanism_weights(netuid, mecid: MechId(u8), commit, reveal_round, commit_reveal_version)` and `commit_crv3_mechanism_weights(netuid, mecid, commit, reveal_round)` (no version arg) | `subxt explore ... calls commit_timelocked_weights`; `calls commit_crv3_weights` → "call not found" |
| CR version | storage `CommitRevealWeightsVersion: u16`, default **4**; checked on commit | `$SUB/pallets/subtensor/src/lib.rs:1314-1316`, `subnets/weights.rs:407` |
| Commit size | `YUMA_COMMIT_SIZE_BYTES = 5000` | `$SUB/pallets/subtensor/src/lib.rs:64`, `subnets/weights.rs:451` |
| Epoch model | stateful: storage `SubnetEpochIndex`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochConsensus`, `MaxEpochsPerBlock`; commits keyed by `current_epoch_with_lookahead(netuid)` | `subxt explore ... storage SubnetEpochIndex` (and the others); `$SUB/pallets/subtensor/src/subnets/weights.rs:1471`, `:428` |
| Mechanisms | `set_mechanism_weights(netuid, mecid, dests, weights, version_key)`, `commit_mechanism_weights(netuid, mecid, commit_hash)`, `reveal_mechanism_weights(netuid, mecid, uids, values, salt, version_key)` | `subxt explore ... calls set_mechanism_weights` etc. |
| Axon | `serve_axon(netuid, version: u32, ip: u128, port: u16, ip_type: u8, protocol: u8, placeholder1: u8, placeholder2: u8)`; `serve_axon_tls(..., certificate: Vec<u8>)` | `subxt explore ... calls serve_axon` |
| MEV shield | pallet `MevShield`, call `submit_encrypted(ciphertext: BoundedVec<u8, ConstU32<8192>>)`; ciphertext = `key_hash(xxhash128(NextKey)) || kem_len(u16) || kem_ct || nonce(24) || aead_ct` with ML-KEM-768 + XChaCha20-Poly1305; storage `NextKey`/`PendingKey`/`CurrentKey` | `$SUB/pallets/shield/src/lib.rs:338-356, :659-675`; `subxt explore ... pallet MevShield storage` |
| Signed HTTP | `btauth/1`: newline-joined `PROTOCOL, scheme, METHOD, path?query, sha256(body) hex, nonce_ns, sender_ss58, receiver_ss58 or "-"`; headers `X-Bittensor-{Version,Hotkey,Receiver,Nonce,Crypto,Signature}`; golden vectors published | `$SUB/sdk/python/bittensor/http_auth.py:58-66, :175-185`; `$SUB/docs/guides/signed-requests.mdx:252-283` |
| Keyfile | `$NACL` + 24-byte nonce + `crypto_secretbox` ciphertext; key = Argon2i v1.3, ops 8, mem 512 MiB, fixed salt `137183dff15a09bc9c90b5518739e9b1`; plus legacy `$ansible` and Fernet formats | `$UP/btwallet/src/keyfile.rs:25, :329-341, :353-378` |
| Runtime APIs | `NeuronInfoRuntimeApi::{get_neuron, get_neuron_lite, get_neurons, get_neurons_lite}` present in V16 metadata (typed, so no hand-copied structs needed) | `subxt explore ... api NeuronInfoRuntimeApi` |
| Ecosystem versions | subxt **0.51.1** (2026-09-30), jsonrpsee 0.26.1, scale-value 0.18.2, frame-decode 0.18.1, scale-decode 0.16.2 | crates.io API, 2026-10-13 |

Everything in V16 metadata uses newtypes (`NetUid(u16)`, `AlphaBalance(u64)`, `MechId(u8)`, `H256([u8;32])`); every dynamic client below relies on subxt's `scale-encode` coercing primitive `Value`s into single-field composites, which works today and is a (quiet) assumption worth pinning with a test.

---

## 2. Per-project reviews

### 2.1 CortexLM/bittensor-rs

**Identity.** `https://github.com/CortexLM/bittensor-rs`, 29 stars, 1 fork, 102 commits, 4 contributors (65 by `echobt`), MIT (c) 2025 Cortex Foundation (`$UP/CortexLM_bittensor-rs/LICENSE:1-3`), last push 2026-04-22, 0 open issues. Not on crates.io (README's `bittensor-core = "0.1"` is aspirational: crates.io has no `bittensor-core`). Downstream: `BaseIntelligence/platform-v2` (32 stars) pins it at git rev `9cf5991` dated 2026-01-08 — i.e. the *pre-rewrite* monolith (the workspace split landed 2026-04-21, commit `4685747` "feat: bittensor-rs complete Rust reimplementation of Python SDK v10.2.0"), so the rewrite broke that consumer's pin **(inferred from dates)**.

**Scope/architecture.** Cargo workspace of 12 crates (`Cargo.toml:1-14`): `bittensor-core` (balance/config/error/pow, 2,017 lines), `bittensor-wallet` (1,270), `bittensor-chain` (9,693), `bittensor-synapse` (918), `bittensor-axon` (axum, 1,528), `bittensor-dendrite` (reqwest, 1,391), `bittensor-metagraph` (934), `bittensor-cli` (7,148), `bittensor-tui` (ratatui, 1,688), `bittensor-pyo3` (5,455), `bittensor-wasm` (1,850, "reimplements core types"), `bittensor-examples`. Edition 2024, rust-version 1.85. Plus a **33,381-line orphaned `src/` tree** (`src/lib.rs:1-17` declares `axon, blocks, chain, cli, config, core, crv4, dendrite, …`) that no workspace member references (`grep -rn '\[lib\]\|\[\[bin\]\]' Cargo.toml bittensor-*/Cargo.toml` shows only the four expected targets) — the previous subxt 0.44 monolith, with its own `NonceManager`, `PairSigner` (`src/chain/signer.rs:1-60`) and a **real** tlock implementation (`src/crv4/encryption.rs:1-16` uses `tle::{tlock::tle, curves::drand::TinyBLS381, stream_ciphers::AESGCMStreamCipherProvider}`). Nine orphaned test files under `tests/` import `bittensor_rs::…` which no crate exports (`tests/metadata_consistency_tests.rs:10`).

**subxt / metadata.** subxt 0.50 (lock 0.50.0). **Static codegen via macro**: `bittensor-chain/src/generated.rs:6` `#[subxt::subxt(runtime_metadata_path = "../metadata/finney.scale")]`; `build.rs:4-12` only emits `rerun-if-changed` and a warning if the file is missing. `metadata/finney.scale` (314,898 bytes) decodes to **spec_version 393** (`subxt explore --file=metadata/finney.scale pallet System constants Version`) — **84 runtime versions behind** finney 477. The generated `commit_timelocked_weights` signature still matches today (`extrinsics/weights.rs:47-62`: `commit: BoundedVec<u8>, reveal_round: u64, commit_reveal_version: u16`), but anything added since 393 (mechanism weights, `commit_crv3_mechanism_weights`, `MevShield` layout, epoch storage) is absent or stale. Config: own `SubtensorConfig` newtype over `SubstrateConfig` with `SubstrateExtrinsicParams` (`bittensor-core/src/config.rs:51-60`); fine while subtensor's custom extensions stay unit-implicit.

**Wallet/keyfile.** `bittensor-wallet/src/keyfile.rs:1-70`: correct `$NACL` layout, `sodiumoxide` Argon2i13 SENSITIVE params and the btwallet salt; a test decrypts a Python-created coldkey fixture (`keyfile.rs`, `decrypt_python_created_coldkey`, fixture `scripts/test_coldkey_meta.json`). `ENCRYPTION.md:1-50` documents the wire format accurately. This is the one part of the crate that is correct and reusable in spirit (but it depends on `sodiumoxide`, unmaintained since 2021).

**Weights / commit-reveal.** Typed wrappers for `set_weights`, `commit_weights`, `reveal_weights`, `commit_timelocked_weights` (`extrinsics/weights.rs`). The **"drand timelock" is not timelock encryption**: `bittensor-chain/src/drand/timelock.rs:10-12` "SHA-256-hashed with a commitment salt … XOR with a SHA-256 counter-mode keystream"; `derive_key(salt, round)` at `:122`; `decrypt_value(…, _randomness_hex)` at `:146-150` ignores the drand randomness entirely; `random_salt()` at `:106-118` is a timestamp-seeded LCG, not a CSPRNG. The chain cannot decrypt such a payload (it expects `tle`/IBE ciphertext for `TinyBLS381`), so this module cannot produce a valid CRv3 commit. The real implementation exists only in the orphaned `src/crv4/`. No epoch-schedule logic. No mechanism-weights calls.

**MEV shield.** `mev_shield/encrypt.rs:6, :59, :85`: "XOR-based encryption … SHA-256 counter mode"; `mev_shield/submit.rs:76-78` frames as `u32 LE len + kem_ct + u32 LE len + payload`. The pallet requires XChaCha20-Poly1305 and `key_hash || u16 kem_len || kem_ct || nonce || aead_ct` (`$SUB/pallets/shield/src/lib.rs:342-350`). Incompatible.

**Networking.** v10 synapse protocol, not btauth/1. Dendrite signs `"{nonce}.{dendrite_hotkey}.{axon_hotkey}.{uuid}.{body_hash}"` (`bittensor-synapse/src/signing.rs:7-15`) — correct for v10 — but emits headers `bt-nonce`, `bt-dendrite-hotkey`, … (`bittensor-dendrite/src/signing.rs:12-19`) while the axon reads `bt_header_dendrite_nonce`, … (`bittensor-axon/src/middleware.rs:21-28`; Python v10 used `bt_header_dendrite_*`, `$UP/bittensor/bittensor/core/synapse.py:625-633`). The Rust dendrite therefore cannot authenticate to the Rust axon. Worse, the axon's `verification_middleware` **never verifies the signature**: `middleware.rs:91-93` "In a full implementation we would verify the Sr25519 signature … For now we just ensure the signing fields are present." Any non-empty `bt_header_dendrite_signature` passes.

**Queries.** Silent-default decoding: `queries/neuron.rs:18-23` `fn decode_val … opt.and_then(|v| v.decode().ok()).unwrap_or_default()` — a type drift after a runtime upgrade yields zeros, not errors. `get_neuron` re-fetches the full per-subnet `Active/Rank/Trust/Consensus/Incentive/…` vectors **per uid** (`neuron.rs:39-79`), and `bittensor-metagraph/src/sync.rs:15-33` calls it sequentially for every uid (O(N) × ~10 whole-vector reads per sync) and drops inactive neurons (`neuron.rs:54-57`). `get_metagraph` returns only `n/total_stake/total_issuance` with `total_weight: 0, total_bond: 0` (`queries/metagraph.rs:35-58`); `get_selective_metagraph` is an alias (`:60-66`).

**Async/tests/CI/docs.** tokio; `cargo llvm-cov` in CI; integration job declares a service container `opentensor/subtensor:latest` on ports 3100/3101 (`.github/workflows/ci.yml:90-92`) while the localnet image's RPC port is 9944 (`$SUB/docs`, `buyback` CI) and the tests README references a nonexistent `./scripts/devnet.sh` from a different repo (`bittensor-chain/tests/README.md:12-19`, "From the workspace root (`llm-proxy/`)") — the integration job is non-functional **(inferred)**. `deny.toml` denies unknown git sources and GPL licenses. Docs: 40+ markdown pages under `docs/`, last commit "comprehensive documentation matching official Bittensor Python SDK quality" — volume, not fidelity.

**Reusable ideas.** Crate split (core/wallet/chain/synapse/axon/dendrite/metagraph/cli) is a reasonable shape; `ENCRYPTION.md` is a good keyfile spec; nothing else.

### 2.2 one-covenant/bittensor-rs (crates.io `bittensor-rs` 0.1.1) + `bittensor-wallet` + `lightning-tensor`

**Identity.** `https://github.com/one-covenant/bittensor-rs`, 3 stars, 40 commits, 3 contributors (`distributedstatemachine` 26 — the same author as the 2024 crates.io `bittensor-wallet` 0.1.0, whose repository URL redirects here), MIT, last push 2026-07-20 (merge of PR #10 "fix/bittensor-metadata-compatibility"). crates.io: `bittensor-rs` 0.1.1 published 2025-12-27, 3,657 downloads (749 recent). Open issues: #8 "Negative TAO amounts cause massive staking values" (2026-02-01), #3 "Will not build under Windows". **In production use**: `one-covenant/basilica` (SN39, 44 stars, pushed 2026-10-12) depends on it by git rev `0f27fd41…` plus a patched `one-covenant/crabtensor` fork (`gh api repos/one-covenant/basilica/contents/Cargo.toml` lines 191-219).

**Architecture.** Workspace `bittensor-rs` (126,709 lines incl. 114,496 lines of generated metadata), `bittensor-wallet` (1,890, pyo3 0.20 cdylib), `lightning-tensor` (ratatui TUI, 1,720). Modules: `connect/{pool,health,monitor,state}`, `retry` (exponential backoff + `CircuitBreaker`), `service` (`Service` façade), `queries`, `extrinsics`, `wallet`, `discovery`, `registration` (`bittensor-rs/src/lib.rs:44-57`).

**subxt / metadata.** subxt 0.44 (lock 0.44.0), `PolkadotConfig`. **`build.rs` performs network I/O at compile time**: `from_url_blocking(url, MetadataVersion::Latest)` (`build.rs:16, :50`) against finney unless `DOCS_RS`/`BITTENSOR_OFFLINE` is set (`:33`), with hash-based caching in `OUT_DIR`. Bundled fallback `metadata/finney.scale` is **spec 365**. `metadata/finney.rs` is a 10-line committed *error message* from `subxt-cli 0.37` ("Cannot decode the provided metadata … variant doesn't exist") containing the author's home path; `metadata/test.rs` and `metadata/local.rs` are byte-identical 3,086,355-byte generated files (`cmp` → identical) and are not what `api.rs:9` includes (it includes `OUT_DIR/metadata.rs`). One genuinely useful trick: type substitution of subtensor newtypes to primitives in codegen (`build.rs:130-141`: `NetUid→u16`, `TaoBalance→u64`, `AlphaBalance→u64`, `PerU16→u16`).

**Wallet/keyfile.** Reads plaintext hotkeys (`secretPhrase`/`secretSeed` JSON or raw mnemonic/hex). **Encrypted coldkeys are not supported**: `wallet/keyfile.rs:220-235` `decrypt_keyfile(data, _password)` always returns `DecryptionError("Encrypted coldkey decryption not yet implemented…")`. The sibling `bittensor-wallet` crate uses **AES-256-GCM + Argon2** (`bittensor-wallet/src/wallet.rs:6-8, :427`), its own format, not btcli's.

**Weights.** `set_weights` normalises then calls `sign_and_submit_default` and returns `ExtrinsicResponse::success().with_message("Weights set successfully")` without waiting for inclusion (`extrinsics/weights.rs:188-200`; same for commit/reveal at `:220`, `:258`). `compute_commit_hash` is **wrong**: `keccak_256` over LE bytes of `uids ∥ weights ∥ salt ∥ version_key` (`weights.rs:124-143`) versus the chain's Blake2-256 of the SCALE tuple including hotkey and netuid (§1). No CRv3, no mechanisms.

**Networking.** None (no axon/dendrite/btauth). `discovery.rs` reads axon info from the metagraph.

**Async/tests/CI/docs.** tokio; `wiremock` dev-dep; `tests/connection_reliability_tests.rs`; CI runs with `BITTENSOR_OFFLINE=1` and installs `protoc` for no visible reason (`.github/workflows/rust-ci.yml`). Docs: thorough rustdoc on `lib.rs`/`service.rs`; `CHANGELOG.md` lists features. Large `BittensorError` enum with `RetryConfig` per category (`error.rs:10-100`) — the categorisation is sensible even if the implementations behind it are thin.

**Reusable ideas.** Connection pool + health checker + circuit breaker shape; codegen type-substitutes; error categories with retry policy. Everything chain-facing must be re-derived from spec 477.

### 2.3 unarbos/agcli

**Identity.** `https://github.com/unarbos/agcli` ("unarbos" = RaoFoundation agent account; last commit by `unconst`), 10 stars, 5 forks, 640 commits, 12 contributors — **414 commits by `cursoragent`** — MIT (c) 2024 Arbos, last push 2026-05-29, 0 issues, 10 open PRs (several 2026-06-30 "fix: Migrate identity + hyperparams to current prod metadata", "fix: alpha-as-TAO unit mixup for stakes"). Not on crates.io; the crates.io name `agcli` belongs to an unrelated crate (0.17.0, "foundation crate for agentic CLIs"). `.orchestrate/` holds the agent run state and hand-off notes.

**Architecture.** Single crate (`src/` 47k lines + 60k lines of tests/docs) with `cli` feature and `sdk-only` feature; modules `chain/{mod,extrinsics,queries,rpc_types,extrinsic_args}`, `wallet/`, `queries/` (Moka memory cache + disk cache + coalescing), `extrinsics/{weights,mev_shield}`, `localnet.rs` (Docker lifecycle), `scaffold.rs`, `admin.rs` (AdminUtils sudo), `events.rs`, `live.rs`; Python package via `python/agcli-py` (pyo3 0.23 + pyo3-async-runtimes).

**subxt / metadata.** subxt **0.38** + `substrate-compat`, sp-core 34, `PairSigner` (`chain/mod.rs:16-18, :218`). **`build.rs` fetches metadata over the network at build time** (V15 then V14, `build.rs:44-50`), caches `metadata.rs`/`metadata.scale` in `OUT_DIR`, honours `SKIP_METADATA_FETCH` (`build.rs:23-28`); CI sets `METADATA_CHAIN_ENDPOINT` to finney (`.github/workflows/ci.yml:17`). The codegen is then largely bypassed: `src/chain/extrinsics.rs` has 46 `subxt::dynamic::tx` call sites vs 43 `api::tx()`; `queries.rs` 18 dynamic vs 21 static. `submit_sudo_raw_call` pre-checks `pallet_by_name/call_variant_by_name` because "subxt panics (rather than returning Err) if the call variant is missing" (`chain/extrinsics.rs:1532-1545`). `tests/extrinsic_encoding.rs` holds golden call bytes, with a CHANGELOG instruction to re-run after metadata refresh.

**Wallet/keyfile.** Native format is **AES-256-GCM, key = Argon2id, file = `salt ∥ nonce ∥ ciphertext`** (`wallet/keyfile.rs:168-200`), with a header comment claiming it matches "the Python bittensor-wallet encryption scheme" (`keyfile.rs:4`) — it does not. Reading btcli `$NACL` files is supported separately and correctly via RustCrypto `argon2` (Argon2i, v0x13, m=524288 KiB, t=8, p=1, fixed salt) + `crypto_secretbox::XSalsa20Poly1305` (`keyfile.rs:345-410`), with `zeroize`. Advisory file locks around keyfiles with timeouts (`keyfile.rs:40-90`), atomic writes with 0600 before rename (`:131-166`).

**Weights / CR.** `set_weights`, `commit_weights`, `reveal_weights`, `commit_timelocked_weights`, mechanism variants (`extrinsics.rs:2476-2550`), `batch_*`. **Reads `CommitRevealWeightsVersion` from storage** before committing (`extrinsics.rs:2433-2457`) — correct. **Commit hash is wrong**: Blake2b-256 over raw LE `uids ∥ values ∥ salt` only (`src/extrinsics/weights.rs:12-33`). **No timelock encryption**: no `tle`/`w3f-bls` dependency (`Cargo.toml`); the CLI's "timelocked" path submits a 32-byte hash as the `commit` (`src/cli/weights_cmds.rs:508-514`), which the chain stores but can never decrypt **(inferred)**.

**MEV shield.** `src/extrinsics/mev_shield.rs:7-8`: `[u16 kem_ct_len][kem_ct][nonce][aead_ct]` + Blake2s commitment — the correct AEAD but the **pre-`key_hash` layout**; today's pallet prepends `xxhash128(NextKey)` (`$SUB/pallets/shield/src/lib.rs:342-350`). Round-trip unit tests exist (`mev_shield.rs:83-152`).

**Networking.** None; docs describe "Epistula" (`docs/philosophy.md:195-203`), not btauth/1.

**Async/robustness.** tokio; retry on "transient" errors decided by **substring matching** (`chain/mod.rs:26-36`: `"onnect"`, `"Ws"`, `"closed"`, `"State already discarded"`); per-signer **`/tmp/agcli-tx-locks/<pubkey>.lock`** `fs2` lock with 60 s spin to serialise nonces across processes (`chain/mod.rs:173-210`). Exit-code taxonomy (`error.rs:10-25`) and dispatch-error surfacing by parsing subxt error strings (`error.rs:52-100`).

**Tests/CI/docs.** 50+ integration test files (`tests/audit_*.rs`, `cli_test_modules/part01-06`, proptest, stress), e2e feature-gated against Docker localnet (`localnet.rs`), nextest profile; CI builds with live finney metadata. Docs are extensive (`docs/llm.txt`, 24 command docs, tutorials, `hyperparameters.md`). `docs/why-agcli.md:9-30` is a marketing comparison.

**Reusable ideas.** `chain/extrinsic_args.rs:7-27` `ArgUnit { TaoRao, AlphaRaw, TaoPerAlphaRao, NormalizedU16, RhoScaleU16, U64OverU64Max, TakeU16_65535, WeightU16, Other }` — a per-argument *semantic unit* table used for CLI parsing, spending limits and docs; exit-code classes; read-CR-version-from-chain; the metadata-presence pre-check before dynamic submission; disk+memory cache with request coalescing (`queries/query_cache.rs`).

### 2.4 ErgodicLabs/btt

**Identity.** `https://github.com/ErgodicLabs/btt`, 2 stars, 210 commits (130 authored as `claude`, 79 `moorkh`), MIT, last push 2026-07-09, 18 open issues (feature backlog: proxy, multisig, offline signing, MEV shield #124, mechanisms #120). Tagged v0.1.0 2026-05-02 with Linux binary release. Not on crates.io (`btt` is an unrelated crate).

**Architecture.** Single binary, 14,231 lines: `cli.rs` (clap), `commands/{chain,stake,subnet,wallet,wallet_keys,transfer,identity,register,axon,weights,child_hotkey,swap_*,liquidity,dynamic_decode,password_file,paths,skill}`, `rpc.rs`, `error.rs`, `output.rs`. JSON envelope `{"ok":true,"data":…}` / `{"ok":false,"error":{"code","message"}}` by default (`README.md:12-24`); `btt skill` emits a SKILL.md for agents.

**subxt / metadata.** subxt 0.50 (lock 0.50.0) with `jsonrpsee`+`native` only, **no codegen, no build.rs, no bundled metadata**: all calls are `subxt::dynamic::tx("SubtensorModule", "commit_weights", vec![Value::u128(..), Value::from_bytes(..)])` (`commands/weights.rs:36-44`), storage/runtime-API values are walked with helpers in `commands/dynamic_decode.rs:1-120` (the module header documents the 14 duplicated helpers it consolidated). `PolkadotConfig`. Only 12 direct dependencies (`Cargo.toml:9-21`); `flock(2)` is declared via `extern "C"` to avoid adding `libc` (`wallet_keys.rs:20-40`).

**Wallet/keyfile.** btcli-compatible `$NACL` write and read using RustCrypto `argon2` + `xsalsa20poly1305` with the btwallet salt (`wallet_keys.rs:319-332`), `Zeroizing` keys, staged atomic writes with backups and a per-wallet flock. **Verified in CI against pinned `pynacl`/`argon2-cffi` wheels by hash, never importing `bittensor`** (`scripts/btcli-compat/check.py:1-35`, `btcli-pins.txt`). Note: the production Argon2 parameters are swapped under `cfg(test)` (`wallet_keys.rs:334-346`) so unit tests do not pay 512 MiB per derivation.

**Weights/CR.** `commit_weights`/`reveal_weights` only; the caller supplies the 32-byte hash (`weights.rs:21-54`); no hashing, no CRv3, no mechanisms. Submission waits for finalization then `wait_for_success` with 120 s timeouts (`weights.rs:127-170`).

**Networking.** None.

**Transport policy.** `validate_url` rejects `ws://` to non-loopback hosts structurally (`rpc.rs:65-90`) but then connects with `from_insecure_url` for everything (`rpc.rs:123-137, :155`) — safe only because the scheme check ran first.

**Tests/CI/process.** Unit tests inline (many), `tests.yml` stable+nightly, `lint.yml` clippy `-D warnings --all-targets`, `dep-audit.yml` (cargo-audit, cargo-deny, cargo-outdated, **same-version-different-bytes checksum tripwire** against `.cryptid/dep-audit/checksums.db`), toolchain pinned to `1.94.1` with rationale (`rust-toolchain.toml:1-20`). `AGENTS.md` codifies dependency discipline, "barbaric review", structural changes must update `btt skill`.

**Reusable ideas.** JSON envelope with stable error codes (`error.rs`); wss-only policy; keyfile compat CI against independent primitives; toolchain pinning rationale; dep-audit tripwire; SKILL.md emission for agents.

### 2.5 womboai/rusttensor → timehash13/crabtensor (+ one-covenant/crabtensor fork)

**rusttensor.** `https://github.com/womboai/rusttensor`, 21 stars, 3 forks, last push 2024-11-21 (GitHub license: none; `Cargo.toml` says MIT), 618 lines, subxt 0.38 + `substrate-compat`. `build.rs` fetches live metadata (`MetadataVersion::Latest`) from finney **on every clean build** (`$UP/womboai_rusttensor/build.rs:14-24`) and pipes through `rustfmt`. Because pre-V15 metadata had no runtime-API types, it hand-copies `NeuronInfo`, `NeuronInfoLite`, `SubnetInfo`, `SubnetHyperparams`, `DelegateInfo`, `StakeInfo` with `Compact<_>` fields (`src/rpc/types.rs`) and a `runtime_api_typing!` macro mapping payload→response (`src/rpc/mod.rs:88-125`) — all drifted from today's types. `wallet.rs` reads only plaintext `secretSeed` JSON (`src/wallet.rs:65-80`).

**crabtensor** (Storb's fork, `https://github.com/timehash13/subtensor-api-rs`, 0 stars, 13 commits, last 2026-03-30, 424 lines, subxt 0.41, sp-core 36). Drops the hand-copied types (V15 metadata carries them), keeps the build-time fetch (`build.rs:25` `MetadataVersion::Latest`), adds `serde` derives for `NeuronInfoLite` via `add_derives_for_type` (`build.rs:64-73`). `normalize_weights` multiplies `weight * u16::MAX` before dividing (`src/weights.rs:26`) — overflows for integer `T`, and the README example passes `f64` which does not implement `Ord` as required by the bound **(inferred: would not compile)**. `serve_axon_payload` hard-codes `version = 1` (`src/axon.rs:345-365`). `one-covenant/crabtensor` (fork, pushed 2025-10-07, subxt 0.38 pinned, "patched fork for metadata compatibility") is what Basilica builds against.

**Verdict.** Historically important lineage (first subxt-on-subtensor pattern, `OnlineClient<SubstrateConfig>` passthrough, typed runtime APIs), too small and too stale to reuse; a cautionary example of build-time network fetches.

### 2.6 inference-labs-inc/subnet-2 (sn2-chain, sn2-miner, sn2-validator)

**Identity.** `https://github.com/inference-labs-inc/subnet-2`, **2,462 stars**, 131 forks, 1,071 commits (988 by `HudsonGraeme`), 10 contributors, MIT, last push 2026-09-30, release 14.14.3 (2026-09-08), Dependabot PRs, `scorecard.yml`/`security.yml`. Toolchain **`nightly-2026-02-22`** (`rust-toolchain.toml`), `#![feature(ip)]` in the validator (`crates/sn2-validator/src/main.rs:1`).

**Architecture.** Workspace: `sn2-types` (protocol structs, tracing init, `signing_message`), `sn2-frame-codec`, `sn2-chain` (2,152 lines: `lib.rs`, `metagraph.rs`, `registration.rs`, `subxt_helpers.rs`, `wallet.rs`, `weights.rs`, `attestation.rs`, `auto_update.rs`), `sn2-verify`, `sn2-circuit-store`, `sn2-validator` (12,080), `sn2-miner` (2,203). Chain deps: subxt 0.50.1 with `reconnecting-rpc-client`, `btwallet` 4.1.0 (`bittensor_wallet`), `btlightning` 0.2.15, `bittensor-drand` **git fork rev `e55ed98` = version 1.3.0** (`Cargo.lock:770-772`), sp-core 34, axum 0.7, mimalloc/jemalloc.

**subxt / metadata.** Fully dynamic over `PolkadotConfig`; no metadata on disk. `subxt_helpers.rs` wraps `subxt::dynamic::storage(PALLET, entry)` + `try_fetch` + `decode_as::<T>()` (`:17-55`). **Backend assembly is the best in the ecosystem**: `connect_chain` builds a `ReconnectingRpcClient` with request timeout 60 s and WS ping 20 s / inactive limit 60 s (`lib.rs:29-32, :86-98`), then a `CombinedBackend` with `no_default_backends()` + chainhead + legacy and **no archive backend**, because "pruned subtensor nodes advertise the archive namespace but answer every archive call with Method not found (-32601)" (`lib.rs:44-62, :103-115`); startup bounded by a 30 s timeout; `is_rpc_disconnect` classifies `DisconnectedWillReconnect` (`lib.rs:121-129`).

**Wallet.** Hotkey via `bittensor_wallet::Wallet::new(...).get_hotkey(None)` (`wallet.rs:12-25`); tests cover hotkey files with/without integer `cryptoType` (`wallet.rs:95-130`). Custom `Signer<PolkadotConfig>` adapter whose `sign` **returns `MultiSignature::Sr25519([0u8; 64])` on any signing error** (`weights.rs:236-249`) — a bad-signature transaction will be broadcast and rejected with `BadProof` instead of failing locally.

**Weights / CRv3.** Only timelocked commits. `COMMIT_REVEAL_VERSION: u64 = 4` **hard-coded** (`weights.rs:18`) rather than read from `CommitRevealWeightsVersion`; `Tempo` defaults to 360 and `RevealPeriodEpochs` to 1 when storage returns `None` (`weights.rs:32, :54-56`). `generate_timelocked_commit` calls the forked `bittensor_drand::generate_commit(uids, values, version_key, tempo, current_block, netuid, reveal_period, BLOCK_TIME, hotkey)` (`weights.rs:60-80`) — the **legacy `(block + netuid + 1) / (tempo + 1)` epoch math** that bittensor-drand 2.0 says "is gone" now that the chain uses `SubnetEpochIndex`/`PendingEpochAt`/configurable tempo (`$UP/bittensor-drand/README.md:11-13`; storage confirmed in spec 477, §1). Reveal rounds will be mis-predicted whenever an owner-triggered epoch or tempo change shifts the schedule **(inferred)**. Excellent recovery pattern: after a lost tx watch, `reconcile_commit` polls `LastUpdate[uid]` for 60 s and treats an advance as success (`weights.rs:137-180`, `commit_reconciled`).

**Metagraph.** `NeuronInfoRuntimeApi_get_neurons_lite` via `runtime_apis().call_raw` with a hand-written `#[derive(Decode)] NeuronInfoLiteRaw` mirror (`metagraph.rs:25-64, :215-225`) and a storage fallback that fans out per-uid reads with `buffer_unordered(32)` (`metagraph.rs:300-330`). Only IPv4 axons (`metagraph.rs`, `registration.rs:40` `bail!("IPv6 is not supported")`). `serve_axon` skips the extrinsic when on-chain `Axons` already matches (`registration.rs:45-60`).

**Networking.** Miner serves QUIC via `btlightning::LightningServer` with validator-permit gating, source-IP allowlist and address validation (`sn2-miner/src/lightning_server.rs:26-45`); HTTP paths sign `"{nonce}:{hotkey}:{payload_hash}"` (`sn2-types/src/lib.rs:49-51`) — SN2-specific, **not btauth/1**.

**Ops.** Sigstore-attested auto-update with pinned Fulcio/Rekor roots (`attestation.rs:1-50`), atomic binary replacement, PM2 restart (`docs/versioning.md`); jemalloc with decay tuning and a written rationale (`sn2-validator/src/main.rs:3-25`); `deny.toml` with per-advisory justifications; fuzz targets for frame codec/tensor JSON; CI with SHA-pinned actions and `--locked`.

**Reusable ideas.** Backend/transport construction, reconcile-after-watch-loss, skip-unchanged `serve_axon`, release attestation, allocator notes. Must not copy: zero-signature fallback, hard-coded CR version, drand 1.3 epoch math, IPv4-only.

### 2.7 inference-labs-inc/btlightning (crates.io `btlightning` 0.2.15)

**Identity.** `https://github.com/inference-labs-inc/btlightning` (repo was `lightning`), 5 stars, 91 commits by one author, MIT (c) 2026 Inference Labs, last push 2026-08-05, 0 issues; crates.io 0.2.15 (2026-08-05), 4,665 downloads (1,086 recent), 16 releases since 2026-02-18; repo `Cargo.toml` still says `version = "0.1.0"` (release tooling bumps it **(inferred)**); PyPI package too. rust-version 1.88.

**Scope.** QUIC transport replacing v10 synapse HTTP: quinn 0.11 + rustls 0.23 (ring), MessagePack with a `NamedSerializer` forcing map encoding (`ARCHITECTURE.md`, "Serialization"), 5-byte frame header, ≤64 MiB payloads, unary + streaming responses, typed handler wrappers. Optional `subtensor` feature (subxt 0.50, `metagraph.rs`) and `btwallet` feature (`BtWalletSigner`, `signing.rs:93-120`).

**Handshake.** Client signs `handshake:{validator_hotkey}:{ts}:{nonce}:{cert_fp_b64}` where `cert_fp = blake2_256(peer TLS cert)` (`src/types.rs:100-110`; `client.rs:1451-1511`); server checks timestamp age (`max_signature_age_secs`) and +60 s skew, 32-hex nonce format, sr25519 signature, nonce replay via `IndexMap<nonce, ts>` with eviction, per-IP 30/min rate limit, optional validator-permit set, then signs `handshake_response:{vk}:{mk}:{ts}:{nonce}:{fp}` (`server/handshake.rs:155-253`). TLS certificate validation is intentionally disabled (`AcceptAnyCertVerifier`, `client.rs:1935-1947`) and identity is bound through the fingerprint. Signing is a sync `Signer` trait called via `spawn_blocking` (`signing.rs:29-35, :43-50`).

**Chain access.** Same explicit chainhead+legacy backend as SN2 (`metagraph.rs:44-86`), `from_insecure_url` for `ws://`; `MetagraphMonitorConfig` background resync (`metagraph.rs:418-445`).

**Tests.** `tests/integration.rs` (2,120 lines), `tests/sn2_contract.rs` (691), `localnet.rs`/`subtensor.rs` feature-gated, proptest, fuzz target for frame parsing, `benchmarks/` vs Python dendrite.

**Assessment for rtao.** Solid, maintained, published, permissively licensed; the natural *optional* transport crate if rtao wants QUIC. It is not interoperable with Python v11 miners (btauth/1 over HTTP) and its handshake is bespoke, so it cannot be rtao's default networking layer. Dependency cost: sp-core 34 (pre-polkadot-sdk-git), quinn/rustls, optional subxt 0.50 (would conflict with rtao on 0.51 until bumped).

### 2.8 CortexLM/buyback (`bittensor-buyback`, unpublished)

**Identity.** `https://github.com/CortexLM/buyback`, 0 stars, 13 commits (all `echobt`), MIT OR Apache-2.0, first release 0.1.0 2026-09-25, last push 2026-09-25. Edition 2024, rust-version 1.88, `#![forbid(unsafe_code)]` (`src/lib.rs:2`).

**Scope.** Alpha payment detection on fresh deposit wallets, sweeps, `burn_alpha`/`recycle_alpha` buybacks; library (`Engine` state machine) + CLI + optional axum/webhook features. 7,575 lines.

**subxt / metadata — the model to copy.** subxt **0.51** (lock 0.51.0), `SubstrateConfig`, fully dynamic ("driven by runtime metadata, so there is no generated code to go stale", `README.md:98-100`). `REQUIRED_CALLS: &[(pallet, call, &[arg names])]` and `REQUIRED_EVENTS` (`src/chain.rs:27-75`); `Chain::verify_metadata()` resolves each call via `pallet_by_name` / `call_variant_by_name`, **compares the field-name list exactly**, resolves each event, and returns evidence lines with call/event indices (`chain.rs:388-427`); a read-only `examples/verify_metadata.rs` is run in CI against localnet (`.github/workflows/ci.yml:55-60`). I cross-checked its `transfer_stake` and `burn_alpha` arg lists against spec 477 — they match (`subxt explore … calls transfer_stake` / `burn_alpha`).

**Transport.** Raw `jsonrpsee` 0.24 WS transport with optional `Authorization: Bearer` header marked `set_sensitive` and URL redaction (`chain.rs:318-345`), `wss://` required unless loopback (`:314-325`), **`LegacyBackend` only** with the comment "public nodes advertise `archive_v1_*` but refuse" (`chain.rs:352-358`), `connect_any` across endpoints with backoff (`:364-380`).

**Transactions.** `prepare()` fetches the nonce, builds `SubstrateExtrinsicParamsBuilder::new().nonce(n).mortal(64)` (`chain.rs:697-720`, `MORTALITY = 64` at `:247`), persists `{action, signer, nonce, tx_hash, birth_block}` before `broadcast()` (journal-before-broadcast, `README.md:166-190`), waits for finalization and asserts the expected pallet event (`chain.rs:723-760`). Fee estimation calls `TransactionPaymentApi_query_info` raw and decodes `RuntimeDispatchInfo` by hand because "subxt's `partial_fee_estimate` assumes a u128 balance; subtensor's `TaoBalance` is u64" (`chain.rs:673-692`, `decode_partial_fee` `:1038-1048` accepts both widths). Runtime-API results decoded with `#[derive(DecodeAsType)]` structs (`chain.rs:118-135`).

**Keys.** Fresh sr25519 via `subxt-signer`; sealed with XChaCha20-Poly1305 under a master key; `zeroize`; HDKD hard paths for static deposit addresses (`README.md:198-214`). No btcli keyfile support (treasury key from mnemonic env/file/keystore).

**Tests/CI.** Unit tests on crypto/state machine/fee math; localnet e2e as a CI service container `ghcr.io/opentensor/subtensor-localnet:devnet-ready` with a readiness loop (`ci.yml:39-62`); cargo-deny; `RUSTDOCFLAGS=-D warnings`.

**Weaknesses.** Error type is stringly (`Error::Chain(String)`, `src/error.rs:5-7`); README has grown an operator-manual tail about legacy recovery (`README.md:443-521`) that signals churn; 0 external users.

**Reusable ideas.** Startup metadata verification with arg-name equality; explicit nonce + mortality + journal; event-asserting broadcast; u64-aware fee decoding; legacy-only backend rationale; bearer/redaction hygiene; localnet CI job shape.

### 2.9 sam0x17/mev-scanner (the user's own code)

**Identity.** `https://github.com/sam0x17/mev-scanner`, 0 stars, last push 2026-03-02, no license file, edition 2024, single `src/main.rs` of 2,259 lines, no tests. Deps (`Cargo.toml:6-18`): `anyhow`, `dashmap`, `frame-decode 0.9`, `frame-metadata 23`, `futures`, `hex`, `jsonrpsee 0.24` (ws-client, macros), `parity-scale-codec`, `scale-value 0.18`, `sp-crypto-hashing`, `serde`, `subxt 0.44` (lock 0.44.2), `tokio`.

**Approach.** Deliberately **dynamic and metadata-agnostic**: fetches `state_getMetadata` over raw jsonrpsee and decodes `RuntimeMetadataPrefixed` with a reserved-prefix check (`main.rs:2093-2106`); handles `RuntimeMetadata::V14|V15|V16` everywhere (`decode_extrinsic` `:1697-1750`, `decode_value` `:1757-1775`); decodes extrinsics with `frame_decode::extrinsics::decode_extrinsic_current` and retries with a `Compact` length prefix when the node returns unprefixed bytes (`:1710-1735`); walks `scale_value::Value` trees generically to find tips, nonces, signers, `AccountId32`s and nested stake calls with case-insensitive field names (`:1556-2020`); validates transactions via raw `state_call("TaggedTransactionQueue_validate_transaction")`, trying the v3 param layout then v2 (`:2108-2135`), decoding `TransactionValidity` with hand-written `Decode` enums (`:2170-2225`); subscribes to `author_subscribePendingExtrinsics` and falls back to polling `author_pendingExtrinsics` on "Method not found" (`:395-440`); `fetch_metadata_at` tries metadata versions `[16, 15, 14]` then legacy (`:1366-1382`); uses `OnlineClient<SubstrateConfig>` + `BackendExt` only for block iteration, `Events::decode_from` and `storage_fetch_value` (`:964-1000, :1356-1392`).

**Style.** `anyhow::Result` with `.context("chain_getBlockHash failed")` on every RPC; `anyhow!` for ad-hoc errors; no `tracing`/`log` — `println!` key=value event lines (`"[{ts}] tx={hash} top={call} priority=… baseline=… tip=…"`, `:735-760`) and a `\r` status line (`print_status_line_once` `:771-820`), `eprintln!` for errors; lock-free `Stats` via `AtomicU64` (`:217-262`), `DashMap` for replacement tracking, `Arc<RwLock<HeadState>>` refreshed by a 6 s ticker task (`:330-345`); `futures::stream::buffer_unordered(fee_concurrency)` for parallel block scans (`:1065-1085`); hand-rolled `Config::from_args` (`:50-130`); `#[derive(Decode)]` structs instead of generated types; zero codegen, zero `build.rs`. Small nits: `Stats::note_tip` is needlessly `async` (`:238`); `eprintln!`/`println!` interleave with the `\r` status line.

**What this tells the plan (section 6):** the user already prefers runtime-metadata-driven decoding, explicit RPC-level control with graceful fallbacks, `anyhow` at the binary layer, and simple structured text logs over frameworks.

### 2.10 btwallet 4.1.0 (archived) — `bittensor_wallet`

**Identity.** `https://github.com/RaoFoundation/btwallet` (opentensor/btwallet redirects), **archived**, 20 stars, 30 forks, last push 2026-07-12 ("docs/pypi-supersession"), 8 open issues incl. **#185 "Bug: legacy_decrypt uses random Fernet key instead of password-derived key"** with an unmerged fix PR #202. crates.io `btwallet` 4.1.0 (2026-05-28), 5,730 downloads (1,324 recent); repo `Cargo.toml` is 4.1.1 unpublished. Library name `bittensor_wallet`, `crate-type = ["cdylib", "rlib"]`, sp-core 34, rust-version 1.74.

**Keyfile correctness.** NaCl path is the reference (`src/keyfile.rs:25, :329-341` Argon2i13 SENSITIVE with fixed salt; `:353-378` encrypt; `:390-405` decrypt). `legacy_decrypt` derives a PBKDF2 key and then discards it, using `Fernet::generate_key()` (`keyfile.rs:407-419`) — legacy-Fernet coldkeys can never be opened (issue #185). `derive_key` uses `.expect()` (`:331, :340`). Library code prints to stdout (`utils::print("Encrypting...\n")` `:363`, password prompts `:171-175`, `ask_password` `:188`) and depends on `colored`, `rpassword`, `clap`, `passwords` (`Cargo.toml:23-37`); `sodiumoxide` 0.2 (unmaintained, links libsodium), `ansible-vault`, `fernet =0.2.1` (needs OpenSSL; `vendored-openssl` feature). `Keypair` supports sr25519 + ed25519 (4.1.0 changelog), mnemonic/seed/uri/encrypted-JSON import, `encrypt/decrypt` (`src/keypair.rs:233-566`); `Wallet::new(name, hotkey, path, config)` resolves `~/.bittensor/wallets` with `shellexpand` (`src/wallet.rs:82-112`).

**Use.** SN2 and btlightning depend on it (`default-features = false`). It is the only published Rust crate that reads real btcli wallets, which is why production Rust subnets use it despite the issues.

### 2.11 bittensor-drand 2.0.0 / bittensor-commit-reveal (archived)

The two on-disk clones are the same commit (`36a7291`, 2026-07-10, "development moved to the subtensor monorepo"); `bittensor-commit-reveal` is a tree-identical copy. `Cargo.toml`: `crate-type = ["cdylib"]` with **non-optional pyo3** and `tle` pinned to `ideal-lab5/timelock` git rev `5416406…`, `w3f-bls =0.1.3`, `ml-kem 0.2.1`, `chacha20poly1305`, `twox-hash` — so it is not consumable as a Rust library (no `rlib`). Content that matters: `WeightsTlockPayload { hotkey, uids, values, version_key }` SCALE payload (`src/drand.rs:21-27`), `encrypt_and_compress(serialized, reveal_round)` (`:55-120`), **`generate_commit_v2(uids, values, version_key, state: EpochScheduleState, reveal_period_epochs, block_time, hotkey)`** which simulates `block_step` via `epoch_schedule::predict_first_reveal_block` and adds `SECURITY_BLOCK_OFFSET = 3` blocks before converting to a drand round (`drand.rs:160-205`; `epoch_schedule.rs:43-130` ports `should_run_epoch`/`run_coinbase`; `constants.rs:38-46`), test vectors in `epoch_schedule_vectors.rs`. Quicknet constants (`constants.rs:14-33`). This module is what rtao's CRv3 must match byte-for-byte, and it now lives (with the same pins) in `$SUB/sdk/bittensor-core/src/timelock/`.

### 2.12 k0jir0/hermes

`https://github.com/k0jir0/hermes`, 0 stars, last push 2026-06-05, MIT (c) 2026 Ryan Watkins. Two crates (`hermes-core`, `hermes-node`, 1,986 lines) with **no chain dependency at all** (`crates/hermes-core/Cargo.toml`: serde, serde_json, thiserror). `consensus.rs` is an `f64` "YC3-style" Yuma port (`:1-80`) with no evidence of validation against the pallet's fixed-point epoch; the rest is Helm/Ansible/Grafana/Bun gateway scaffolding. Not relevant as a dependency; at most a reminder that an off-chain Yuma simulator would be a welcome rtao test utility if derived from `pallets/subtensor/src/epoch`.

### 2.13 Official `$SUB/sdk/bittensor-core` (positioning only; another report covers it)

`Cargo.toml:1-7`: `publish = false`, Apache-2.0, `crate-type = ["rlib"]`. Modules (`src/lib.rs:10-31`): `client` (host), `codec`, `digest` (RFC-0078 merkleized metadata), `keyfiles` (host), `keys`, `mlkem`, `pow` (host, optional OpenCL), `runtime`, `signers` (ledger), `timelock`, `transaction` (host). The client is a **blocking JSON-RPC layer over `reqwest::blocking`** ("adds no second async runtime", `src/client.rs:1-9, :19`) — HTTP, no websocket subscriptions, no tokio. `transaction.rs` adds an `Executor`/`IntentCall`/`Policy` with `Spend` classification (`:1-60`). The spec's non-goals are explicit: no Rust websocket/RPC transport, no Rust codegen/intents, no Rust btcli (`$SUB/sdk/bittensor-core-spec.md`, §3). Dependencies inherit **workspace pins** (`sp-core`, `frame-metadata`, `tle`, vendored `w3f-bls`) from the RaoFoundation polkadot-sdk fork (`$SUB/eco-tests/Cargo.toml:21-30` shows the git rev), so consuming it from outside the monorepo means a path/git dependency that drags that fork into rtao's graph. Its e2e harness spins Docker `ghcr.io/raofoundation/subtensor-localnet:monorepo-sdk` and honours `E2E_ENDPOINT` (`tests/e2e_support/mod.rs:23, :64-80`), with a sharded `e2e-manifest.json` and the `check-bittensor-e2e-tests.yml` workflow (`$SUB/.github/workflows/check-bittensor-e2e-tests.yml:1-70`). Only `subtensor-macros` 0.1.0 from the monorepo is on crates.io (17 downloads).

### 2.14 Other Rust artifacts found (searches: GitHub `bittensor language:rust`, `subtensor language:rust`, crates.io `q=bittensor|subtensor`, GitHub code search `"SubtensorModule" subxt`)

| Project | What | Chain access | Status |
|---|---|---|---|
| `flamewire-bittensor-indexer` 0.1.3 (`unitone-labs/bittensor-indexer`) | generic event-indexer framework (handlers, circuit breaker, JSON/SQLite/Postgres checkpoints) | subxt 0.42.1 `unstable-light-client`; `blocks().subscribe_finalized()` + `block.events()` (`src/indexer.rs:226-257` via API) | 3 stars, 8 commits, last 2025-07-16; 1,676 downloads |
| `sagarregmi2056/subnet-data-indexer-bittensor` | staking-event indexer | subxt 0.42.1 + scale-value 0.18 | 2 stars, 2026-06-01 |
| `taolk` 2.0.3 (`mcjkula/taolk`) | encrypted messaging TUI/CLI "SDK" | **no subxt**: hand-rolled `tokio-tungstenite` JSON-RPC (`src/chain.rs:27-218`), `rusqlite` cache | 3 stars, 75 downloads, 2026-06-20 |
| `subgrind` 0.1.0 | vanity SS58 grinder | none | 0 stars |
| `dataverse-cli` 0.2.0 (`macrocosm-os`) | HTTP client for SN13 API | none (reqwest) | 12 commits |
| `bittensor-wallet` 0.1.0 (crates.io, 2024-08-19) | early wallet by `distributedstatemachine` | none | 1,281 downloads (5 recent); repo now redirects to one-covenant |
| `BaseIntelligence/platform-v2` | decentralized evaluation platform | git-pins CortexLM `bittensor-rs` rev `9cf5991` (pre-rewrite) | 32 stars, pushed 2026-02-23 |
| `one-covenant/basilica` (SN39) + `bigailabs/cathedral-archived` fork | GPU compute subnet | subxt 0.44 + one-covenant `crabtensor` fork + `bittensor-rs` git | 44 stars, pushed 2026-10-12 |
| `bt-decode` 0.8.0 (`$UP/bt-decode`) | pyo3 cdylib SCALE decoder for the old Python SDK | n/a | last 2025-10-28 |
| `prompting` (SN1) | only a competition evaluator in Rust (`shared/competition/.../rust/`) | none | n/a |
| `fiber`, `unconst_Chi`, subnet templates, `async-substrate-interface` | Python only (0 `.rs` files) | n/a | n/a |

Name check: crates.io has no `rtao` (API 404; search empty); GitHub `sam0x17/rtao` exists (created 2026-10-13). crates.io `agcli` and `btt` are taken by unrelated crates; `bittensor`, `bittensor-core`, `bittensor-drand`, `crabtensor`, `rusttensor`, `subtensor-api` are unclaimed.

---

## 3. Comparison matrix

| | CortexLM bittensor-rs | one-covenant bittensor-rs | agcli | btt | rusttensor/crabtensor | SN2 sn2-chain | btlightning | buyback | mev-scanner | btwallet | bittensor-core (official) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Type | SDK workspace + CLI/TUI/pyo3/wasm | SDK + wallet + TUI | CLI+SDK+py | CLI | lib | subnet binaries | QUIC transport lib | payments lib+CLI | tool | wallet lib | core lib |
| subxt (locked) | 0.50.0 | 0.44.0 | 0.38 | 0.50.0 | 0.38 / 0.41 | 0.50.1 | 0.50.1 (opt) | 0.51.0 | 0.44.2 (+jsonrpsee) | none | none (blocking reqwest) |
| Codegen | static macro, committed `.scale` | build.rs codegen from **network** (bundled fallback) | build.rs codegen from **network**; mostly dynamic calls | dynamic | build.rs codegen from **network** | dynamic | dynamic | dynamic | dynamic (frame-decode/scale-value) | n/a | own metadata-driven codec |
| Metadata pinned at | spec **393** | spec **365** bundled; live at build | live at build | live at connect | live at build | live at connect | live at connect | live at connect, **verified** | live at connect (V14/15/16) | n/a | live (V16), digest |
| Config | own `SubtensorConfig` (Substrate params) | `PolkadotConfig` | `SubstrateConfig` | `PolkadotConfig` | `SubstrateConfig` | `PolkadotConfig` | `PolkadotConfig` | `SubstrateConfig` | `SubstrateConfig` | n/a | own |
| btcli `$NACL` coldkeys | read/write (sodiumoxide) | **no** (stub) | read (RustCrypto); own AES-GCM write | read/write (RustCrypto), CI-verified | plaintext hotkeys only | via btwallet (hotkey) | via btwallet (opt) | no | no | reference (sodiumoxide) | reference (host feature) |
| set_weights / CRv1 | yes / yes | yes (no wait) / **wrong hash** | yes / **wrong hash** | – / hash supplied | yes / – | – / – | – | – | – | – | intents |
| CRv3 tlock | **fake** (XOR) | no | no (`tle` absent) | no | no | yes, drand **1.3 legacy epoch math**, CR version hard-coded | – | – | – | – | **v2 stateful epoch sim** |
| Mechanism weights | no | no | yes | no | no | no | – | – | – | – | intents |
| MEV shield | **wrong** (XOR, u32 framing) | no | pre-`key_hash` layout | no (issue #124) | no | no | – | – | – | – | `mlkem` module |
| Neuron networking | v10 synapse (header mismatch, **no sig verify**) | none | none | none | none | QUIC (btlightning) + SN2 HTTP sig | QUIC handshake | none | none | – | none |
| btauth/1 | no | no | no | no | no | no | no | no | no | no | Python only |
| Async model | tokio | tokio + pool/circuit breaker | tokio + caches | tokio | tokio | tokio, reconnecting client, pings | tokio/quinn | tokio | tokio, raw subscriptions | sync | sync (threads) |
| Tests | unit + broken integration | unit + wiremock | 50+ files, e2e localnet (feature) | unit + CI compat | none | unit + fuzz | integration 2k lines, fuzz, localnet (feature) | unit + localnet CI | none | some unit | e2e Docker localnet, golden vectors |
| Docs | large, inaccurate | good rustdoc | large | README+dev guide+AGENTS | README | ops docs | ARCHITECTURE.md | README manual | none | rustdoc-ish | spec |
| License | MIT | MIT | MIT | MIT | MIT (file missing upstream) | MIT | MIT | MIT/Apache | none | MIT | Apache-2.0 |
| Activity (stars / last push) | 29 / 2026-04-22 | 3 / 2026-07-20 | 10 / 2026-05-29 | 2 / 2026-07-09 | 21 / 2024-11-21; 0 / 2026-03-30 | 2,462 / 2026-09-30 | 5 / 2026-08-05 | 0 / 2026-09-25 | 0 / 2026-03-02 | archived 2026-07-12 | active (monorepo) |
| Published | no | crates.io 0.1.1 | no (name taken) | no (name taken) | git tags | binaries | crates.io 0.2.15 | no | no | crates.io 4.1.0 | no (`publish=false`) |

---

## 4. Lessons for rtao

### 4.1 Patterns to adopt (with the source to crib from)

1. **Verify the runtime at startup, dynamically.** Keep a table of every `(pallet, call, [arg names])`, `(pallet, event)`, `(pallet, storage)`, `(runtime_api, method)` rtao uses and check them against `OnlineClient::metadata()` on connect, failing fast with evidence lines (`$UP/CortexLM_buyback/src/chain.rs:27-75, :388-427`). Extend buyback's idea: also check the SCALE *type* of each argument against an expected shape (buyback only checks names), and compare `spec_version` against the range rtao was tested with, logging a warning (not an error) when newer.
2. **Build the subxt backend explicitly.** `ReconnectingRpcClient` with WS ping/inactive limits and request timeouts; `CombinedBackend::no_default_backends().with_chainhead_backend().with_legacy_backend()` — no archive backend, because public/pruned nodes advertise `archive_v1_*` and answer -32601 (`$UP/inference-labs-inc_subnet-2/crates/sn2-chain/src/lib.rs:44-115`; same in `btlightning/src/metagraph.rs:44-86`; buyback goes further and uses legacy only, `chain.rs:352-358`). Bound startup with a timeout; classify `DisconnectedWillReconnect`.
3. **Reconcile transaction outcome from chain state when the watch is lost** (`sn2-chain/src/weights.rs:137-180`): subscriptions do not survive reconnects; after submit, poll an observable effect (e.g. `LastUpdate[uid]`, account nonce, expected event in blocks `birth..birth+mortality`) before declaring failure. Combine with buyback's explicit nonce + 64-block mortality + journal-before-broadcast (`buyback/src/chain.rs:247, :697-760`) for anything that moves funds.
4. **Read protocol parameters from storage, never hard-code them**: `CommitRevealWeightsVersion` (agcli does this, `extrinsics.rs:2433-2457`; SN2 hard-codes 4, `weights.rs:18`), `Tempo`, `RevealPeriodEpochs`, `SubnetEpochIndex`/`LastEpochBlock`/`PendingEpochAt` for reveal-round prediction, `MevShield::NextKey`, `Balances::ExistentialDeposit` (buyback `chain.rs:443-449`).
5. **Port the chain's CRv3 round prediction exactly** from `bittensor-core/src/timelock/epoch_schedule.rs` (= bittensor-drand 2.0 `generate_commit_v2`), including `SECURITY_BLOCK_OFFSET`/`COMMIT_INCLUSION_BLOCK_OFFSET`, and pin `tle`/`w3f-bls` to the same revisions as `pallet-drand` so ciphertexts are decryptable (`$UP/bittensor-drand/README.md:4` states this requirement explicitly). Carry the `epoch_schedule_vectors.rs` test vectors.
6. **Keyfiles with RustCrypto, verified against independent primitives.** btt and agcli show `$NACL` read/write with `argon2` + `xsalsa20poly1305`/`crypto_secretbox` and no C libsodium (`$UP/ErgodicLabs_btt/src/commands/wallet_keys.rs:319-332`, `$UP/unarbos_agcli/src/wallet/keyfile.rs:345-410`); btt's `scripts/btcli-compat/check.py` verifies byte-for-byte against hash-pinned `pynacl`/`argon2-cffi` **without** importing `bittensor`. Add fixed golden vectors (a committed `$NACL` blob + password, as CortexLM does in `bittensor-wallet/scripts/test_coldkey_meta.json`). Also implement `$ansible` and the Fernet legacy path *correctly* (btwallet's is broken, issue #185) if rtao claims full compat, and zeroize.
7. **Signer as a sync trait executed off the runtime** (`btlightning/src/signing.rs:43-50`, `spawn_blocking` at `:29`), with implementations for in-memory keypairs, btcli wallets and callbacks (HSM/remote). Never return a dummy signature on error (SN2 `weights.rs:236-249`).
8. **Semantic units on arguments** (`agcli/src/chain/extrinsic_args.rs:7-27`): tag every amount argument as TAO-rao, alpha-raw, price (rao/alpha), normalized-u16, etc., and derive both type-safe newtypes (`Rao`, `Alpha`, `NetUid`, `MechId`) and CLI/doc rendering from one table. one-covenant issue #8 (negative f64 → huge u64) and agcli PR #15 ("alpha-as-TAO unit mixup") are the bugs this prevents.
9. **u64 balances.** subtensor's `TaoBalance`/`AlphaBalance` are `u64` newtypes; subxt helpers that assume `u128` (fee estimation) must be replaced (`buyback/src/chain.rs:673-692, :1038-1048`).
10. **Transport policy**: refuse plaintext `ws://` except to loopback, structurally parsed (`btt/src/rpc.rs:65-120`; `buyback/src/chain.rs:314-325`); redact URLs/keys in errors; optional bearer header for private endpoints (`buyback/src/chain.rs:330-337`).
11. **Metagraph sync**: runtime API `NeuronInfoRuntimeApi_get_neurons_lite` first (typed via metadata, no hand-copied structs), storage fan-out as fallback with bounded concurrency (`sn2-chain/src/metagraph.rs:160-330`); never one-uid-at-a-time whole-vector refetches (CortexLM `neuron.rs:39-79`).
12. **Output/error discipline for agents**: stable machine-readable error codes (`btt/src/error.rs:5-20`; agcli `error.rs:10-25` exit codes), JSON envelope for CLIs, a `SKILL.md`/`llm.txt` generated from the real command surface (btt `commands/skill.rs`, agcli `docs/llm.txt`).
13. **Process**: pinned toolchain with a written bump procedure (`btt/rust-toolchain.toml`), `cargo-deny` + `cargo-audit` + lockfile checksum tripwire (`btt/scripts/dep-audit/README.md`), SHA-pinned GitHub Actions and `--locked` builds (SN2 `ci.yml`), localnet as a CI service container with a readiness loop (`buyback/.github/workflows/ci.yml:39-62`) or the official `E2E_ENDPOINT`/`LOCALNET_IMAGE` convention (`$SUB/sdk/bittensor-core/tests/e2e_support/mod.rs:23-80`), golden-vector tests for every signed payload (btauth/1 vectors in `$SUB/docs/guides/signed-requests.mdx:252-283`), encoding regression tests with golden call bytes (`agcli/tests/extrinsic_encoding.rs`).
14. **QUIC handshake design** if rtao offers a QUIC transport: cert-fingerprint-bound signatures, nonce replay cache with eviction, timestamp window + future skew, per-IP handshake rate limit, `Incoming::ignore()` for non-allowlisted sources, Retry-based address validation (`btlightning/ARCHITECTURE.md`, `src/server/handshake.rs:155-270`).
15. **Release hygiene for subnet binaries**: Sigstore-attested auto-update with pinned roots (`sn2-chain/src/attestation.rs:1-50`) — optional, but the only example of supply-chain-safe self-update in the ecosystem.

### 4.2 Patterns to avoid (with the evidence)

1. **Network access in `build.rs`** (one-covenant `build.rs:50`, agcli `build.rs:44-50`, rusttensor/crabtensor `build.rs:25`): non-reproducible builds, CI coupled to finney uptime, generated API drifting under the developer's feet. If rtao ever ships static types, generate them with an explicit `cargo xtask` from a committed, spec-version-stamped `.scale` and diff them in CI.
2. **Committed metadata without a spec-version stamp or freshness check** (CortexLM spec 393, one-covenant spec 365, plus an error dump committed as `metadata/finney.rs` and two identical 3 MB generated files). Any bundled metadata must carry `spec_version` in its filename/manifest and be compared to the live chain at startup.
3. **Hand-copied runtime types** (rusttensor `rpc/types.rs`, SN2 `NeuronInfoLiteRaw`, buyback `RawStakeInfo`) when V15/V16 metadata already carries the runtime-API types; prefer `DecodeAsType` from metadata, or if a mirror struct is unavoidable, assert its type-string against metadata at startup.
4. **Re-implemented chain crypto without vectors**: wrong commit hashes in two projects (one-covenant `weights.rs:124-143` keccak; agcli `weights.rs:12-33` blake2b over raw bytes), two incompatible MEV-shield formats (CortexLM XOR; agcli pre-`key_hash`), a non-timelock "timelock" (CortexLM `drand/timelock.rs:10-12, :146-150`), a fake keyfile "compatibility" (agcli AES-GCM, one-covenant AES-GCM). Every chain-defined computation in rtao needs a test vector derived from the pallet source or the official core.
5. **Silent defaults on decode failure** (`CortexLM/.../queries/neuron.rs:18-23`, SN2 defaults for `Tempo`/`RevealPeriodEpochs`); decode errors must surface as typed errors.
6. **Fire-and-forget "success"** (`one-covenant/.../weights.rs:188-200`): report inclusion/finalization and dispatch result, or an explicit "submitted, unconfirmed" state.
7. **Assuming `PolkadotConfig`/`SubstrateConfig` forever**: works only while subtensor's `SubtensorTransactionExtension`, `SudoTransactionExtension`, `DrandPriority`, `CheckShieldedTxValidity` remain unit-implicit. rtao should define its own `Config` and verify the extension list from metadata at startup (none of the reviewed projects do).
8. **String-matching transient errors** (`agcli/src/chain/mod.rs:26-36`) and **`/tmp` lock files for nonce serialisation** (`:173-179`); use typed subxt/jsonrpsee error variants and an in-process nonce manager plus on-chain reconciliation.
9. **Library code that prints, prompts, or panics** (btwallet `keyfile.rs:171-175, :188, :340, :363`).
10. **Unmaintained or C-linking crypto** (`sodiumoxide` in btwallet/CortexLM; `fernet`+OpenSSL; `ansible-vault`) when RustCrypto equivalents exist and are already proven compatible (btt, agcli).
11. **Monolithic single-crate SDKs with 100k+ lines generated by agents** (agcli: 414/640 commits by `cursoragent`; btt: 130/210 by `claude`; CortexLM: `echobt`): fine as tools, but the churn (agcli's six open "fix: migrate to current prod metadata" PRs) shows what happens without a verification layer. rtao's modular crates with a thin verified chain layer are the antidote.
12. **Orphaned code and docs** (CortexLM's 33k-line `src/`, tests README from another repo, CI job that cannot run): keep `cargo deny`/CI honest with `--workspace --all-targets` and delete dead trees.
13. **Nightly toolchains** in libraries (SN2 `nightly-2026-02-22`, `#![feature(ip)]`); rtao should build on stable with a pinned MSRV.
14. **cdylib-only crates** (bittensor-drand, bt-decode) that cannot be consumed from Rust.

---

## 5. What gap rtao fills; collaborate vs depend vs rewrite

**No existing project provides any of these together, and most provide none:**

1. A **published, semver'd, tokio-native Rust SDK** on current subxt (0.51) with a **verified dynamic** chain layer. Today: buyback is the only 0.51 user and is a payments engine; the only published SDK (`bittensor-rs` 0.1.1) is subxt 0.44 with stub coldkeys.
2. **btauth/1** (v11 signed HTTP) in Rust — zero implementations. CortexLM implements v10 synapse headers (inconsistently, without verification); SN2 and btlightning use their own schemes. This is the single largest functional gap for "build a subnet in Rust that interoperates with Python v11 miners/validators".
3. **CRv3 with the stateful epoch model + mechanism weights** in an async library: SN2 has CRv3 with the legacy epoch math and no mechanisms; agcli has mechanisms but no tlock; bittensor-core has the right math but is blocking, unpublished and polkadot-sdk-fork-pinned.
4. **A subnet framework** (validator loop skeleton: metagraph sync, scoring hooks, weight normalisation/clipping per `max_weight_limit`, commit scheduling by epoch; miner server: axum + btauth middleware, blacklist/priority hooks; owner tooling: hyperparameters, identity, mechanisms, commitments) with a **localnet-backed test harness** developers can run. SN2 is the closest but is application code, not a framework.
5. **Keyfile compatibility done with RustCrypto and proven with vectors**, including legacy formats, in a crate that is a library (no prompts/prints).

**Depend on / collaborate with:**

- **btlightning** (MIT, published, maintained, tested): candidate for an *optional* `rtao-transport-quic` adapter rather than a rewrite. Coordinate on bumping its subxt feature to 0.51 (or make rtao's QUIC adapter avoid its `subtensor` feature entirely). Its author (Inference Labs) is the most active Rust-on-Bittensor shop; worth an early conversation about btauth/1 interop over QUIC.
- **bittensor-core (official)**: do not depend on the crate (unpublished, blocking client, polkadot-sdk-fork pins), but treat `timelock/`, `mlkem/`, `keyfiles/`, `digest/`, `codec/` and the e2e harness as the **reference** rtao's implementations are tested against, and track the monorepo's `bittensor-core-spec.md` phases. If RaoFoundation ever publishes `bittensor-core` with an `rlib` and crates.io-resolvable deps, rtao's `rtao-crypto` could become a thin re-export. Worth proposing upstream (the spec's phase 5 mentions uniffi/napi bindings, not a Rust SDK, so a Rust consumer is a new argument for publishing).
- **btwallet**: use only as a test oracle for keyfile vectors (it is archived and has a known bug); do not depend on it.
- **buyback / CortexLM**: nothing to depend on; borrow the verification pattern. The author (`echobt`) iterates fast and could be a reviewer.
- **one-covenant / Basilica**: a potential *consumer* (they maintain forks because `bittensor-rs` and `crabtensor` drifted) — a reason to ship rtao early with a migration note.
- **agcli (RaoFoundation agent account)**: its `docs/hyperparameters.md`, `extrinsic_args.rs` units table and the `audit_*` test matrix are a checklist of surface area; nothing to import.

**Rewrite** everything chain-facing. The cost is modest: the dynamic subxt layer the good projects converge on (SN2/buyback/btt) is a few thousand lines; the hard parts (btauth, CRv3 epoch simulation, keyfiles, MEV shield) have authoritative Python/pallet references and golden vectors to test against.

---

## 6. Notes on the user's mev-scanner style (what the plan should respect)

Observed in `$UP/sam0x17_mev-scanner/src/main.rs` (cited in §2.9):

- **Dynamic over static**: raw `state_getMetadata` + `frame-decode` + `scale-value`, version-agnostic across V14/V15/V16, no codegen, no `build.rs`. rtao's chain layer should keep this (dynamic subxt 0.51 or raw jsonrpsee + frame-decode where subxt gets in the way), and reserve any generated types for *optional* convenience crates.
- **Fallback ladders at the RPC edge**: subscription → polling on "Method not found"; validate_transaction v3 → v2; metadata 16 → 15 → 14 → legacy; Compact length-prefix retry. Design rtao's transport with explicit capability probing (`rpc_methods`) and fallbacks rather than assuming a node profile.
- **`anyhow` with `.context()` at the binary layer**; for library crates rtao should expose `thiserror` enums (chain/decode/transport/auth/keyfile) that still carry context strings, and convert to `anyhow` only in bins/examples. The user's code never swallows errors silently — keep that (contrast CortexLM's `unwrap_or_default`).
- **Logging**: key=value single-line events and a status line, no `tracing` subscriber. The plan should use `tracing` in libraries (structured fields) but default to a compact key=value formatter so output looks like what the user already writes; keep stdout for data, stderr for diagnostics.
- **Concurrency**: `buffer_unordered` with a `--concurrency` knob, atomics/`DashMap` for shared counters, one background ticker task for head state. Fits a tokio-native SDK with bounded fan-out for metagraph sync and block scans.
- **Minimal dependencies, hand-rolled CLI args** — the plan can use `clap` in tools but should keep core crates lean (`btt`'s discipline resonates with this).
- **No tests in the scanner** — the plan must add the test culture (golden vectors, localnet e2e) rather than assume it.

---

## 7. Open questions for the architects

1. Whether to consume `tle`/`w3f-bls` at the exact `pallet-drand` revisions (git pins, no-std fork vendored in the monorepo) — required for decryptable CRv3 ciphertexts — and how to keep those pins in sync with runtime upgrades (a CI job diffing `$SUB/Cargo.toml` pins is the obvious answer).
2. Whether rtao's own `subxt::Config` should hard-fail or warn when metadata lists a non-unit transaction extension it does not know (today all four custom extensions are unit-implicit).
3. Whether to support `serve_axon_tls` certificates and IPv6 axons from day one (no reviewed project does IPv6; SN2 bails).
4. How much of the v10 synapse scheme to accept server-side for transition (the signed-requests guide documents the legacy formula, `$SUB/docs/guides/signed-requests.mdx:284-295`).
5. Whether to offer btlightning as an optional transport or to specify a btauth/1-over-QUIC profile and contribute it upstream.
6. Whether to publish `rtao` as one facade crate plus `rtao-*` sub-crates; the names `rtao`, `bittensor`, `bittensor-core`, `subtensor-api` are free on crates.io, `agcli`/`btt`/`bittensor-rs`/`bittensor-wallet` are taken.
