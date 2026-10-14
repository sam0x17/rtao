# rtao, architecture B: a type-driven veneer over bittensor-core

Date: 2026-10-13. Lens: Rust-idiomatic and type-driven. Reader: an expert Rust/Substrate engineer who knows FRAME and
subtensor internals. The user's decisions are taken as given: git dependencies are fine, crates.io does not matter, rtao is a
veneer over the monorepo crate `sdk/bittensor-core`, and the scope is full parity with the v11 Python package.

## Conventions

| Abbreviation | Path |
|---|---|
| `R/` | `SP/research/` (research reports A1, A2, B, C1, C2, D, E, F, G, H, I, CORRECTIONS, feasibility-*) |
| `UP/` | `SP/upstream/subtensor/` (monorepo @ `850f2755`, spec 478 on main) |
| `CORE/` | `UP/sdk/bittensor-core/src/` |
| `PY/` | `UP/sdk/python/bittensor/` (v11 Python package) |
| `META/` | `SP/meta/` (finney/devnet metadata) |
| `CAT/` | `SP/docs/` (bittensor.com `intents.json`, `reads.json`, `errors.json`, `index.json`, `llms*.txt`) |
| `BENCH/` | `SP/design-bench/` (measurements made for this document; see Appendix A) |

Every claim carries a citation. **[measured]** means I ran it for this document (Appendix A). **[verified]** means read
in source or reproduced from a sibling report's command. **[inferred]** means a reasoned claim nobody has executed.

---

## 0. Summary

rtao is a Cargo workspace of 23 library crates, a CLI, and an `xtask`. Every byte the chain defines comes from
bittensor-core: keys, keyfiles, signature payloads, v4 extrinsic assembly, storage key hashing, the RFC-0078 digest, the
ML-KEM shield envelope, drand tlock ciphertexts, PoW seals and the Ledger APDU path. rtao pins it to the finalized-mainnet
git tag (`tag = "v477"` resolves to `5c6e83e2`) and replicates the monorepo's `w3f-bls` patch.

rtao builds the rest itself:

- a tokio-native WebSocket JSON-RPC session that ports the semantics of `PY/_transport/rpc.py`;
- a typed layer generated from pinned metadata;
- typed intents and reads that carry their signer role, origin, units and outputs in the type system;
- btauth/1;
- a columnar metagraph;
- the validator, miner and owner framework;
- a `btcli`-equivalent CLI generated from the same registries as the library.

Parity with Python is a CI gate. It diffs rtao's registries against the vendored bittensor.com catalogs and runs a metadata
coverage gate ported from `UP/sdk/python/codegen/check.py`.

The codec choice drives most of the design, so I measured it rather than inferring it. The test decoded a live finney
`SubnetInfoRuntimeApi.get_metagraph` response (256 UIDs, 49,859 bytes) and a synthesized 2,500-UID response (408,250
bytes) five ways **[measured, Appendix A]**:

| Decode strategy | 256 UIDs | 2,500 UIDs | Throughput |
|---|---|---|---|
| A. bittensor-core `Runtime::decode_id` → `Value` (cyscale shapes) | 330 µs | 2.68 ms | ~150 MB/s |
| B. `scale-value::scale::decode_as_type` → `Value` | 911 µs | 7.31 ms | ~55 MB/s |
| C. `parity-scale-codec` `#[derive(Decode)]` mirror (owned, typed, positional) | 29.8 µs | 261 µs | ~1.6 GB/s |
| D. `scale-decode` `DecodeAsType` mirror (subxt's static-type decode path) | 1,380 µs | 11.0 ms | ~36 MB/s |
| D2. as D with `[u8; 32]` keys and only the hot fields | 776 µs | 6.48 ms | ~64 MB/s |
| E. borrowed columnar view (proposed `MetagraphView`) | 6.5 µs | 57 µs | ~7.6 GB/s |

Three conclusions follow:

1. Typed positional decoding is about 11x faster than bittensor-core's dynamic `Value`. That decoder spends roughly
   45% of its time rendering AccountIds as ss58 strings: ss58-encoding the 512 keys alone took 140–158 µs
   **[measured]**.
2. subxt's static types do not buy decode speed. `DecodeAsType` is the slowest strategy measured.
3. A borrowed view is another 4.6x faster than owned typed structs.

For the record, each one-shot HTTPS `state_call` of `get_metagraph` (a new TLS connection per call) took 0.71–0.88 s
from this machine **[measured]**. On the validator hot path, CPU decode is not the bottleneck. The decode choice matters for miners that check every request
against the metagraph, for scans across all 128 subnets, and for allocation pressure. It is not about RPC latency.

So the design is a **hybrid**:

- generated `parity-scale-codec` types for typed access, guarded per runtime spec by a structural *shape hash* computed
  from the live metadata;
- bittensor-core's `Value` engine as the fallback, which decodes any spec by its live registry, and for every JSON
  surface (CLI `--json`, agent tools), because its shapes are the Python SDK's shapes (`CORE/codec/decode.rs:1-8`);
- a borrowed columnar view for the metagraph.

subxt is not a dependency (§3.4 gives the reasons).

### Decisions that define this design

1. **Pin bittensor-core by finalized-mainnet tag and replicate the patch.** Downstream workspaces must also carry
   `[patch.crates-io] w3f-bls` at the same tag. Without it Cargo resolves two `w3f-bls 0.1.3` sources and bittensor-core
   fails to compile with E0277 (`TinyBLSDrandQuicknet: EngineBLS` not satisfied) **[measured]**. With it, `tag = "v477"`
   builds in 12.9 s release without `host` and derives Alice's address correctly **[measured]**.
2. **Reuse bittensor-core's byte engine, not its product layer.** `Executor<'a>` borrows the blocking `&Client`
   (`CORE/transaction.rs:998-1016`). `IntentCall`'s security semantics are private (`CORE/transaction.rs:103-111`), so
   `Policy::check(&IntentCall, ..)` (`:937`) cannot classify rtao's typed intents. rtao re-implements those async and
   proposes upstream seams (§1.7).
3. **No subxt.** There is one metadata engine: bittensor-core `Runtime` over V15, which parses in 1.0 ms **[measured]**.
   subxt's typed decode is the slowest path measured. It adds about 42 crates over a hand-rolled jsonrpsee/frame-decode
   stack, mostly codegen and proc-macro machinery compiled even when unused (F §1.8). It shipped 8 breaking minors in
   23 months (F §1.7).
4. **Hybrid codec with shape hashes.** Typed positional decode runs only when the live type's structural hash equals the
   codegen-time hash. Otherwise the call falls back to the dynamic `Value` path (reads) or refuses (signing).
5. **Own transport.** A tokio-tungstenite JSON-RPC session ports `rpc.py`:
   - idempotent pending frames are resent on reconnect;
   - `author_submit*` is never resent (`FateUnknown`);
   - subscriptions fail loudly;
   - endpoint pools rotate;
   - policy refusals are terminal;
   - reads fall back to an archive pool on "State already discarded".

   It uses the legacy RPC namespace only.
6. **Signer role, origin and output are associated types.** `Executor::execute<I: Intent, K: KeySource<I::Role>>`, so a
   coldkey intent does not compile against a hotkey-only key source.
7. **Units carry context.** Wire newtypes (`TaoRao`, `AlphaRao`, `NetUid`, `MechId`, `NetUidStorageIndex`, `PerU16`,
   fixed-point types) are substituted into generated call signatures. Domain amounts are `Balance<Tao>` and
   `Balance<Alpha>`, and the alpha one carries its netuid. Mixing TAO and alpha is a compile error. Mixing two netuids is
   a checked error.
8. **One static registry per kind** (`INTENTS`, `READS`) feeds `execute_tool` JSON, the CLI, JSON-schema catalogs, docs
   and `llms.txt`. CI diffs it against the vendored bittensor.com catalogs.
9. **Fail-safe transactions by construction:**
   - RAII nonce leases;
   - mortal eras by default (128 blocks);
   - a shield era type that cannot exceed the chain's 8 blocks (`UP/runtime/src/check_mortality.rs:19,94-100`);
   - retry decisions on a typed `PoolRejection` enum, never on raw message substrings;
   - `CallBytes` stamped with the spec version they were composed under.
10. **Events match by `(pallet, name)` with per-event shape hashes, never by index.** Event indices moved between 461 and
    477 (H §2.2).
11. **btauth/1 verifies through bittensor-core's `keys::verify`.** Its acceptance set is bit-identical to Python's,
    including the `<Bytes>` tolerance; a strict mode is opt-in. The Python check of retention against the freshness
    window becomes a constructor invariant.
12. **Timelock reuses bittensor-core's tlock and `WeightsTlockPayload` bytes.** rtao carries a corrected port of the epoch
    scheduler, using the pallet's `BlocksSinceLastStep > tempo` predicate rather than the core's `> MAX_TEMPO`. The clock
    is injectable, and drand is fetched asynchronously.
13. **Wallet loading accepts bittensor-core `host`** (libsodium and vendored OpenSSL) for byte-identical keyfiles. rtao
    proposes upstream to un-gate the plaintext keyfile JSON codec.
14. **Every type that can decode positionally is property-tested** against the dynamic decoder, per pinned spec. That is
    the type-driven safety net under decision 4.
15. **Upstream tracking runs as four automated jobs:**
    - metadata drift (structural);
    - catalog drift (semantic);
    - core-tag bump;
    - upstream-file watch (protocol and taxonomy sources).

    Their output is a published `compat.json`.

---

## 1. Boundary with bittensor-core

### 1.1 Pinning and bumping

```toml
# rtao/Cargo.toml (workspace root)
[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false }

[patch.crates-io]
# The monorepo patches w3f-bls to its vendored copy (UP/Cargo.toml:299,335). Cargo ignores [patch] in dependencies,
# so every workspace that builds bittensor-core must repeat it, at the SAME tag as bittensor-core.
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Facts behind this pin:

- `v<spec>` tags exist on the remote and point at the release-train commits. `git ls-remote` shows `v477 → 5c6e83e2`,
  `v475 → d1718c99`, `v473 → f87cada6`, plus abandoned pre-release tags `v470`, `v471` and `v476`, which H §1.2 confirms
  were never finalized **[verified]**.
- Tag pinning works: a fresh crate with the two lines above resolved `bittensor-core` to
  `git+…?tag=v477#5c6e83e2…`, built in 12.9 s release (84.6 s CPU, no `host`, 241 lockfile packages), and printed
  `5GrwvaEF…KutQY` for `//Alice` (`BENCH/../design-tagpin/tagpin.log`) **[measured]**.
- The patch is mandatory. Without it Cargo locked two `w3f-bls 0.1.3` (crates.io for `tle` at its git rev, and the
  monorepo's vendored path copy for bittensor-core). The build then failed in bittensor-core with four
  `E0277 TinyBLSDrandQuicknet<…>: EngineBLS is not satisfied` errors (`BENCH/../design-bench-nopatch`) **[measured]**.
- A patch at a *different* tag than the dependency reproduces the two-sources failure. This follows from the
  two-sources mechanism above **[inferred]**. Hence one source of truth for the tag; see the bump job.
- The bittensor-core `host` feature (default upstream) adds libsodium, vendored OpenSSL, `rayon`, blocking `reqwest` and
  `libloading` (`UP/sdk/bittensor-core/Cargo.toml:105-126`; 330 crates per R/feasibility-bittensor-core-gitdep.md). rtao
  turns it on only through `rtao-core/host`, which `rtao-wallet` and PoW mining need (§1.6, item 5).

**Bump procedure** (`cargo xtask core-bump --tag v<N>`, run by the `core-bump` CI job in §12):

1. Refuse unless `v<N>` is a finalized mainnet release: the GitHub release has `isPrerelease == false` and the finalized
   finney runtime reports `spec_version == N`, the same check as `UP/.github/scripts/mainnet-finalized-runtime.sh:19-26`.
2. Rewrite the tag in both `[workspace.dependencies]` and `[patch.crates-io]`. Rewrite `rtao-core/src/pin.rs`
   (`BITTENSOR_CORE_TAG`, `BITTENSOR_CORE_COMMIT`, `PINNED_SPEC`). Run `cargo update -p bittensor-core -p w3f-bls`.
3. Run the **seam contract suite**, `crates/rtao-core/tests/seam.rs`. It is a compile-and-behaviour test of every
   bittensor-core item rtao imports (§1.2) against the golden vectors (§11.1). It also runs an `xtask core-diff` that
   prints `git diff v<old>..v<N> -- sdk/bittensor-core` so reviewers see upstream changes inside the seam.
4. Open a PR labelled `core-bump`. CI runs the full offline suite and the localnet lanes against `:v<N>`.

`rtao doctor` (CLI) reads the user's `Cargo.lock`. It fails if there is more than one `w3f-bls` source or if the patch
tag differs from `rtao_core::pin::BITTENSOR_CORE_TAG`, which makes the cryptic E0277 diagnosable.

### 1.2 Used as-is

rtao re-exports these through `rtao-core` (§1.8) and never reimplements them.

| Area | bittensor-core items (file:line) | rtao consumer |
|---|---|---|
| Keys, ss58 | `Keypair` (`CORE/keys/mod.rs:151-162`), `from_uri` (264), `from_mnemonic` (211), `from_seed` (243), `from_private_key` (287), `generate_mnemonic` (322), `sign` (391-399), `verify` with `<Bytes>` fallback (402-410), free `verify` (457-469), `ss58_from_public` (57), `public_key_from_ss58` (48), `CRYPTO_ED25519/SR25519` (28-29) | `rtao-units::AccountId` ss58, `rtao-signer::KeypairSigner`, `rtao-auth::verify` |
| Polkadot-js keystore import (`host`) | `Keypair::from_encrypted_json` (`CORE/keys/mod.rs:338-340`), the public entry to the private `keys::encrypted_json` module (`:25`) | `rtao-wallet` (`regen-coldkey --json-path`) |
| Keyfiles (`host`) | detectors (`CORE/keyfiles/mod.rs:29-47`), `encrypt_keyfile_data` (86), `decrypt_keyfile_data` (153-177), `get_password_from_environment` (111), `serialized_keypair_to_keyfile_data` (179), `deserialize_keypair_from_keyfile_data` (287) | `rtao-wallet` |
| Metadata view | `Runtime::parse` (V14/V15 only, `CORE/runtime/mod.rs:163-228`), `pallet` (230), `storage_entry` (238), `module_error` (340), `apis`/`extrinsic` fields (137-157), `StorageInfo` (29) | `rtao-client::RuntimeCache` |
| Signing payload and v4 assembly | `TxParams` (`CORE/codec/extrinsic.rs:19-30`), extension-ordered `PAYLOAD_FIELDS` (38-55), `signature_payload_parts` (217-236), `signature_payload` (240-253, blake2 when > 256 bytes), `encode_signed_extrinsic` (260-316, v4 only at 267-272), `decode_extrinsic` (321), `era_birth` (407-412), `multisig_account_id` (419-437) | `rtao-client::tx` (§3.8) |
| Storage hashing | `hash_param` (`CORE/codec/storage.rs:21`), `storage_prefix` (60), `decode_storage_key_params` (101) | typed storage keys (§3.6) |
| Dynamic codec | `Runtime::compose_call` (`CORE/codec/encode.rs:532-585`), `Runtime::decode_id` (`CORE/codec/decode.rs:296`), `Value` (`CORE/codec/value.rs:12-33`), `decode_batch`/`decode_map_page`/`decode_map_changes` (`CORE/codec/batch.rs:55,86,111`) | raw calls (`rtao call`), dynamic reads, JSON output, archive decode under unknown specs |
| RFC-0078 | `ChainInfo` (`CORE/digest/mod.rs:34-40`), `metadata_digest` (102-107), `generate_extrinsic_proof` (117) | `CheckMetadataHash` Enabled path; Ledger |
| MEV shield | `mlkem::seal` (`CORE/mlkem/mod.rs:53-107`), `twox_128` (37) | `rtao-client::shield` (§3.9) |
| Timelock bytes | `WeightsTlockPayload` (`CORE/timelock/mod.rs:53`), `UserData` (62), `encrypt_and_compress` (104), `decrypt_and_decompress` (143), `encrypt_at_round` (230), `reveal_round`/`inner_ciphertext` (87, 97), `decrypt_with_signature` (319), quicknet constants (`CORE/timelock/constants.rs:14-44`) | `rtao-timelock` |
| PoW | `pow::seal` (`CORE/pow/mod.rs:15-21`), `mine_cpu` (51), `PREFIX_LEN` (11), OpenCL `GpuMiner` (`host`) | `rtao-neuron::registration::pow` |
| Ledger (`ledger`) | `LedgerDevice::{open, app_version, address, sign}` (`CORE/signers/ledger.rs:52-160`) | `rtao-signer::LedgerSigner` |
| Errors | `CoreError` (`CORE/error.rs:10-30`) | mapped once into `rtao_core::Error` |

### 1.3 Wrapped (blocking to async, or product defaults changed)

| bittensor-core item | Why wrapped | rtao wrapper |
|---|---|---|
| `decrypt_keyfile_data` (Argon2i, 512 MiB, t=8; `CORE/keyfiles/mod.rs:59-72`) | blocks a core for seconds | `rtao_core::offload(..)` (`tokio::task::spawn_blocking`); the decrypted `Keypair` is cached per process in `Zeroizing` |
| `pow::mine_cpu`, `GpuMiner::mine` | CPU/GPU bound | dedicated blocking task; refreshes the challenge every block (5-block validity, C1 §4.2) |
| `LedgerDevice::sign` (HID read, effectively infinite timeout, `CORE/signers/hid.rs:34`) | blocks until the user approves | `spawn_blocking` inside `LedgerSigner::sign`, behind a cancellable wrapper |
| `timelock::get_round_info`, `decrypt`, `get_reveal_round_signature` (blocking reqwest, `CORE/timelock/mod.rs:242-315`) | blocking I/O | **not called**: `rtao-timelock::DrandClient` (async reqwest, quicknet endpoint failover) fetches the signature, then calls the portable `decrypt_with_signature` |
| `timelock::generate_commit_v2` (`CORE/timelock/mod.rs:160-197`) | reads `SystemTime::now()` internally (`:174-180`), and `predict_first_reveal_block` uses the divergent predicate (§1.6) | **not called**: `rtao-timelock::crv3` carries the corrected scheduler and an injected `Clock`, then calls `encrypt_and_compress(&payload.encode(), round)` |
| `Runtime::storage_key(entry, &[Value])` (`CORE/codec/storage.rs:69-94`) | takes `Value` params | typed keys: `storage_prefix` precomputed at codegen; `hash_param(hasher, &key.encode())` per component |
| `Runtime::compose_call(.., &Value)` | dynamic | typed calls encode `[pallet_index, call_index] ++ args.encode()`, with indices resolved by name from the live `Runtime` (§3.8); `compose_call` stays for the raw path |

### 1.4 Not used, and why

| Item | Evidence | Replacement |
|---|---|---|
| `client::Client` | blocking `reqwest` over HTTP; `ws(s)://` rewritten to `http(s)://` (R/B §4.1, `CORE/client.rs:150-179`); no subscriptions; polls `chain_getBlock` for receipts; 120 s receipt timeout (`:31`); fee via deprecated `payment_queryInfo` (`:551-564`); no nonce cache | `rtao-transport` + `rtao-client` |
| `transaction::Executor<'a>` | holds `client: &'a Client` (`CORE/transaction.rs:998-1016`); `execute_with` signs with era `Some(64)` (`:1081-1101`) | `rtao-tx::Executor<T: Transport>` |
| `transaction::IntentCall` + 22 constructors | private `SecuritySemantics` (`:103-111`); `encode(&Client)` (`:828-830`); `Value` params | typed intents (§4) |
| `transaction::Policy::check(&IntentCall, ..)` | same four limits as Python's `Policy` (`PY/intents/plan.py:21-85`) but cannot see rtao intents' spend/netuids | `rtao-tx::Policy::check(&dyn PolicySubject, fee)`: same semantics, about 40 lines (§4.4) |
| `Client::submit_shielded` | signs both carrier and inner with the 64-block era (`CORE/client.rs:622-654`; `DEFAULT_ERA_PERIOD = 64` at `:32`), which `CheckMortality` rejects as `Stale` (`UP/runtime/src/check_mortality.rs:19,94-100`); returns the carrier result | `rtao-client::shield` (8-block `ShieldPeriod`, inner-receipt resolution, §3.9) |
| `Value` decode on hot typed reads | ~150 MB/s vs ~1.6 GB/s typed **[measured]** | generated types (§3.4) |

### 1.5 What bittensor-core lacks and where rtao supplies it

| Gap | rtao supplier |
|---|---|
| Async transport, subscriptions, reconnect, endpoint pools, policy refusals, archive fallback | `rtao-transport` (§3.2) |
| V16 metadata (`Runtime::parse` rejects it, `CORE/runtime/mod.rs:192-199`) | runtime: fetch V15 via `Metadata_metadata_at_version(15)`; offline: `xtask` decodes V16 with `frame-metadata` for deprecation info and `transaction_extensions_by_version` |
| Typed API (calls, storage, runtime APIs, events, constants) | `rtao-chain` (generated, §3.5) |
| Nonce cache, mortality policy, fee via `state_call`, watch-based outcome resolution, nested-wrapper failure detection, reconciliation | `rtao-client` (§3.8) |
| Signer abstraction (only in-process `Keypair` today) | `rtao-signer` (§6) |
| 83 intents, 102 reads, 14 namespaces, the 18-code `ErrorCode` taxonomy (core only snake-cases names, `CORE/client.rs:1396-1411`) | `rtao-intents`, `rtao-reads`, `rtao-errors` |
| btauth/1, metagraph model, framework, CLI | `rtao-auth`, `rtao-http`, `rtao-metagraph`, `rtao-neuron`, `rtao-cli` |

### 1.6 Divergences found in bittensor-core and how rtao handles each

1. **Epoch predicate.** `should_run_epoch` tests `blocks_since_last_step > MAX_TEMPO_U64` (`CORE/timelock/epoch_schedule.rs:54`),
   while the pallet tests `BlocksSinceLastStep > tempo` (`UP/pallets/subtensor/src/coinbase/run_coinbase.rs:1231`)
   **[verified]**. This matters after an owner `set_tempo` re-anchors `LastEpochBlock`. rtao's `rtao-timelock::epoch` is a
   port of the pallet predicate. It keeps upstream's `epoch_schedule_vectors.rs` cases and adds vectors for the safety-net
   branch. An upstream PR fixes the core.
2. **Shield era.** 64 blocks in `Client::submit_shielded` against a chain maximum of 8. rtao does not use that path (§1.4).
   Upstream PR: use 8 for both legs.
3. **V16 rejected** (`CORE/runtime/mod.rs:192-199`). rtao fetches V15, which reports extrinsic `version == 4` and the
   identical 13-extension list on live finney (`BENCH/bench-info.txt`) **[measured]**.
4. **`<Bytes>` acceptance in `verify`** (`CORE/keys/mod.rs:402-410, 457-469`). It matches Python's verifier, which is what
   parity wants (§7). Strict verification has no public entry point because `verify_with_crypto` (`:78-100`) is private.
   Upstream PR: expose `verify_exact`.
5. **Keyfile JSON is `host`-gated together with the C crypto** (`CORE/lib.rs:15-16`). Even plaintext hotkey files need the
   libsodium/OpenSSL build. Upstream PR: move the JSON codec
   (`serialized_keypair_to_keyfile_data`/`deserialize_keypair_from_keyfile_data`) out of `host`.
6. **`w3f-bls` path dependency forces a downstream patch** (§1.1). Upstream PR: declare `w3f-bls = "=0.1.3"` (a crates.io
   requirement) in `sdk/bittensor-core/Cargo.toml` instead of `{ workspace = true }`. Inside the monorepo the root
   `[patch.crates-io]` (`UP/Cargo.toml:335`) still redirects it to `vendor/w3f-bls`. Downstream, both bittensor-core and
   `tle` resolve the same crates.io copy, so no downstream patch is needed. The `tle` build against crates.io `w3f-bls`
   succeeded in the no-patch run, and only bittensor-core failed. Whether the vendored no-std edit matters for std builds is
   **[inferred]** not to (R/F §3.5).
7. **`IntentCall` and `Policy` are closed** (§1.4). Upstream PR: a `PolicySubject` trait (`spend()`, `netuids()`,
   `affects_all_subnets()`, `is_raw()`), so `Policy::check` is reusable by any intent layer.
8. **`save_password_to_environment` uses `std::env::set_var`** (`CORE/keyfiles/mod.rs:126-138`), which is unsound under
   threads. rtao never calls it. It only reads `BT_PW_*` through `get_password_from_environment`.

### 1.7 Upstream contributions (proposed, non-blocking)

| PR | Size | Effect on rtao |
|---|---|---|
| `w3f-bls = "=0.1.3"` in bittensor-core's manifest | 1 line | removes the mandatory downstream patch |
| `should_run_epoch` uses `tempo` | 1 line + vectors | rtao can drop its epoch port and call `generate_commit_v2`/`predict_first_reveal_block` (with a clock parameter, below) |
| `generate_commit_v2_at(now_secs: f64, ..)` | small | injectable clock; deterministic tests |
| `submit_shielded` era 8 | 2 lines | correctness for the core's own users |
| `keys::verify_exact` | small | btauth strict mode without a second crypto stack |
| JSON keyfile codec out of `host` | small | hotkey-only miners build without libsodium/OpenSSL |
| `PolicySubject` trait | small | rtao reuses `Policy` |
| Publish metadata V15/V16 as release assets (`watch-mainnet-release.yml`) | small | removes a live fetch in the drift job (H §8.3) |

### 1.8 The `rtao-core` facade

```rust
// crates/rtao-core/src/lib.rs (excerpt)
pub mod pin {
    pub const BITTENSOR_CORE_TAG: &str = "v477";
    pub const BITTENSOR_CORE_COMMIT: &str = "5c6e83e2f4936f257234f246957f75684c35227c";
    pub const PINNED_SPEC: u32 = 477;
}
pub use bittensor_core::{codec::Value, runtime::Runtime};
pub mod keys { pub use bittensor_core::keys::{Keypair, ss58_from_public, public_key_from_ss58, verify,
    CRYPTO_ED25519, CRYPTO_SR25519, DEFAULT_SS58_FORMAT}; }
pub mod digest { pub use bittensor_core::digest::{ChainInfo, metadata_digest, generate_extrinsic_proof}; }
pub mod tlock { pub use bittensor_core::timelock::{WeightsTlockPayload, UserData, encrypt_and_compress,
    decrypt_with_signature, encrypt_at_round, reveal_round, inner_ciphertext, constants}; }
pub mod extrinsic { pub use bittensor_core::codec::extrinsic::{TxParams, era_birth, multisig_account_id, multisig_ss58}; }
pub mod storage { pub use bittensor_core::codec::storage::{hash_param, storage_prefix}; }
pub mod dynamic { pub use bittensor_core::codec::decode::Cursor; } // decode_id, decode_map_page, compose_call are
                                                                   // inherent methods on Runtime (§3.6 *_dyn paths)
pub mod shield { /* fn seal(pk: &MlKemPublicKey, inner: &[u8]) -> Result<ShieldCiphertext, Error> over mlkem::seal(.., true) */ }
#[cfg(feature = "host")] pub mod pow { pub use bittensor_core::pow::{seal, mine_cpu, PREFIX_LEN, MAX_BATCH}; }
#[cfg(feature = "host")] pub mod keyfiles { pub use bittensor_core::keyfiles::*; }
#[cfg(feature = "ledger")] pub mod ledger { pub use bittensor_core::signers::ledger::{LedgerDevice, LedgerAddress}; }

/// Typestate wrapper over the payload/assembly functions: the spec it was built under is part of the value.
pub struct UnsignedExtrinsic { runtime: std::sync::Arc<Runtime>, call: CallBytes, params: TxParamsOwned }
impl UnsignedExtrinsic {
    pub fn payload(&self) -> Result<SigningPayload, Error>;               // signature_payload(_parts)
    pub fn attach(self, sig: &Signature) -> Result<SignedExtrinsic, Error>; // encode_signed_extrinsic
}
pub struct SignedExtrinsic { pub bytes: bytes::Bytes, pub hash: [u8; 32], pub spec_version: u32 }

/// Run blocking core work (KDFs, PoW, HID) off the async runtime.
pub async fn offload<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T;

#[derive(Debug, thiserror::Error)]
pub enum Error { #[error("keyfile: {0}")] Keyfile(String), #[error("wrong password")] WrongPassword,
    #[error("not in runtime: {0}")] NotInRuntime(String), #[error("codec: {0}")] Codec(String),
    #[error("crypto: {0}")] Crypto(String), #[error("device: {0}")] Device(String) }
impl From<bittensor_core::CoreError> for Error { /* 1:1; Rpc/Policy variants are unreachable through rtao */ }
```

`CallBytes`, `Signature`, `SigningPayload` and `TxParamsOwned` are defined in `rtao-core`, the lowest layer, because
`UnsignedExtrinsic` needs them. `rtao-client` and `rtao-signer` re-export them.

Features: `host` (enables `bittensor-core/host`), `ledger` (`bittensor-core/ledger`). `rtao-core` has no tokio dependency
other than `offload`, behind a `tokio` feature that defaults on.

---

## 2. Workspace and crate layout

### 2.1 Principles

- **One concern per crate, no cycles, and generated code in its own crate** (`rtao-chain`), so a runtime bump recompiles
  one crate and its dependents. Hand-written domain crates do not churn.
- **Optional crates for optional ecosystems**: EVM, legacy wire formats, QUIC, hardware and browser signers. The facade
  `rtao` has feature flags, and subnet binaries depend on the facade.
- **Typed public API.** `bittensor_core::Value` appears only in `*_dyn` escape hatches and JSON surfaces.
- **No network in `build.rs`.** Metadata and fixtures are committed and pinned (E §4.2 item 1).

### 2.2 Crates

| Crate | Purpose | Public surface (main items) | Features | Depends on (rtao crates) |
|---|---|---|---|---|
| `rtao` | facade | `prelude::*`, re-exports per feature | see §2.4 | all, optional |
| `rtao-core` | bittensor-core veneer (§1.8) | `pin`, `keys`, `digest`, `tlock`, `keyfiles`, `ledger`, `CallBytes`, `Signature`, `UnsignedExtrinsic`, `SignedExtrinsic`, `offload`, `Error` | `host`, `ledger`, `tokio` | — |
| `rtao-units` | money, ids, ratios, fixed point (§5) | `Balance<U>`, `Tao`, `Alpha`, `Amount<U>`, `TaoRao`, `AlphaRao`, `NetUid`, `MechId`, `NetUidStorageIndex`, `Uid`, `PerU16`, `Permill`, `Perbill`, `U64F64`, `I96F32`, `I64F64`, `I32F32`, `Proportion`, `PriceRao`, `Blocks`, `AccountId` | `serde`, `schemars` | `rtao-core` (ss58) |
| `rtao-chain` | **generated** typed chain surface (§3.5) | `calls::<pallet>::*`, `storage::<pallet>::*`, `runtime_apis::<api>::*`, `events::<pallet>::*`, `constants::*`, `errors::ERRORS`, `MetagraphField`, `ShapeHash`, `SPEC_RANGE` | `serde` | `rtao-units`, `rtao-core` |
| `rtao-transport` | async WS JSON-RPC session (§3.2) | `RpcSession`, `Endpoints`, `SessionConfig`, `ReadMethod`, `Subscription<T>`, `RpcError`, `PolicyRefusal` | `rate-limit` | — |
| `rtao-client` | chain access (§3.3–3.11) | `Client`, `ClientBuilder`, `Network`, `View`, `Snapshot`, `RuntimeCache`, `Events`, `NonceManager`, `NonceLease`, `Mortality`, `ShieldPeriod`, `Submission`, `Receipt`, `BlockStream`, `wait_for_*` | `blocking`, `disk-cache` | `rtao-transport`, `rtao-chain`, `rtao-units`, `rtao-core`, `rtao-errors` |
| `rtao-errors` | taxonomy (§4.6) | `ErrorCode` (18), `ChainError`, `ChainErrorInfo`, `CUSTOM_TX_ERRORS`, `classify()`, `explain()` | — | `rtao-chain` |
| `rtao-signer` | signing seam (§6.3) | `Signer`, `Role`, `Coldkey`, `Hotkey`, `KeySource<R>`, `KeypairSigner`, `PreparedExtrinsic`, `SigningContext` | `ledger`, `vault`, `extension`, `remote` | `rtao-core`, `rtao-units` |
| `rtao-wallet` | btcli-compatible wallets (§6.1–6.2) | `Wallet`, `WalletPath`, `HotkeyFile`, `ColdkeyFile`, `PasswordResolver`, `PasswordSource`, address/proxy/multisig books | `keychain`, `prompt`, `create` | `rtao-signer`, `rtao-core/host` |
| `rtao-tx` | transaction product layer (§4.1–4.4) | `Intent`, `OriginKind` markers, `IntentMeta`, `DynIntent`, `Executor`, `ExecOptions`, `Policy`, `Plan`, `ExtrinsicResult<O>`, `Spend`, `Built<P>`, `compose::{sudo, proxy, batch_all, as_multi}` | — | `rtao-client`, `rtao-signer`, `rtao-errors` |
| `rtao-reads` | 102 reads + 14 namespaces (§4.5) | `Read`, `ReadEntry`, `READS`, `client.subnets()`, `.staking()`, … `Pinning` | — | `rtao-client`, `rtao-chain` |
| `rtao-timelock` | drand + CRv3 (§9.6) | `DrandClient`, `Round`, `Timelocked<T>`, `crv3::commit`, `epoch::{EpochScheduleState, predict_first_reveal_block}`, `Clock` | — | `rtao-core`, `rtao-client` |
| `rtao-intents` | 83 intents + hyperparameter table (§4.3, §9.5) | `INTENTS`, one struct per op (`AddStake`, `SetWeights`, …), `Hyperparameter`, `WeightsInput`, `NormalizedWeights` | — | `rtao-tx`, `rtao-reads`, `rtao-timelock` |
| `rtao-metagraph` | metagraph model + sync (§8) | `Metagraph`, `MetagraphView`, `NeuronRef`, `MetagraphNeuron`, `FieldSet`, `MetagraphSync`, `MetagraphDiff`, `SubnetInfo`, `DynamicInfo`, … | `persist` | `rtao-reads`, `rtao-chain` |
| `rtao-auth` | btauth/1 core, no I/O (§7.1) | `build_payload`, `sign`, `Verifier`, `VerifyConfig`, `Caller`, `AuthError`, `NonceStore`, `InMemoryNonceStore` | — | `rtao-core`, `rtao-units`, `rtao-signer` |
| `rtao-http` | btauth/1 for axum/tower/reqwest (§7.2) | `BtAuthLayer`, `Caller` extractor, `CallerPolicy`, `MetagraphPolicy`, `HotkeyRateLimit`, `SignedClient` | `server`, `client`, `redis` | `rtao-auth`, `rtao-metagraph` (opt) |
| `rtao-neuron` | framework (§9) | `Validator`, `ValidatorRunner`, `Miner`, `MinerServer`, `OwnerTools`, `WeightSetter`, `AxonPublisher`, `Registration`, `NeuronConfig`, `StateStore` | `http`, `persist` | everything above |
| `rtao-evm` | EVM helpers (§9.8) | `h160_to_ss58`, `EvmKeystore`, `Precompiles`, `EvmRpc`, `EvmNetwork`, `wei` conversions | — | `rtao-units`, `rtao-wallet` |
| `rtao-cli` | the `rtao` binary, btcli-equivalent (§10) | binary only | `ledger`, `vault`, `extension`, `evm` | everything |
| `rtao-macros` | proc-macros | `#[derive(Intent)]`, `#[rtao::read]`, `#[derive(Hyperparams)]` | — | — |
| `rtao-testkit` | test infrastructure (§11) | `FakeChain`, `Localnet`, `golden::*`, `RecordingView`, `py_interop::*` | `docker` | `rtao-client`, `rtao-tx` |
| `rtao-compat-synapse` | optional legacy v10 wire format (§7.3) | `SynapseHeaders`, `PythonStr`, `body_hash`, `DendriteSigner`, `AxonVerifier` | — | `rtao-auth` |
| `rtao-compat-epistula` | optional Epistula v2 dialects (§7.3) | `EpistulaSigner`, `EpistulaVerifier`, `Dialect` | — | `rtao-auth` |
| `rtao-quic` | optional btlightning adapter (§7.4) | `QuicTransport` | — | `rtao-signer` |
| `xtask` | maintainer tooling, not a dependency | `codegen`, `drift`, `parity`, `record`, `corpus`, `docs`, `core-bump`, `import-upstream`, `compat-matrix` | — | most |

### 2.3 Dependency direction

```
                  rtao-macros (proc-macro, no deps on others)
rtao-core ──► rtao-units ──► rtao-chain(gen) ──► rtao-errors
    │             │              │                   │
    │             ▼              ▼                   ▼
    │        rtao-signer    rtao-transport ──► rtao-client ◄─────────┘
    │             │                               │   │
    ├──► rtao-wallet(host)          rtao-reads ◄──┘   └──► rtao-tx ◄── rtao-signer
    │                                  │                     │
    └──► rtao-timelock ◄── rtao-client │                     │
                       │               ▼                     ▼
                       └──────► rtao-intents ◄───────────────┘
                                       │
   rtao-auth ──► rtao-http ◄── rtao-metagraph ◄── rtao-reads
                     │              │
                     └──► rtao-neuron ◄── rtao-intents, rtao-wallet
                                 │
              rtao-cli ◄─────────┘   rtao (facade) re-exports all of the above by feature
```

Invariants checked by `cargo xtask deps-check` (`cargo metadata` plus an allowlist):

- `rtao-transport` has no chain knowledge (no `rtao-chain` dependency).
- `rtao-auth` has no tokio, axum or reqwest dependency. It is usable from any server stack.
- Only `rtao-wallet` and `rtao-neuron/pow` enable `rtao-core/host`.
- `subxt` appears nowhere.

### 2.4 Facade features

```toml
# crates/rtao/Cargo.toml (features)
[features]
default    = ["client", "tx", "reads", "intents", "metagraph", "timelock", "auth", "wallet"]
client     = ["dep:rtao-client"]
tx         = ["client", "dep:rtao-tx", "dep:rtao-signer"]
reads      = ["client", "dep:rtao-reads"]
intents    = ["tx", "reads", "timelock", "dep:rtao-intents"]
metagraph  = ["reads", "dep:rtao-metagraph"]
timelock   = ["client", "dep:rtao-timelock"]
auth       = ["dep:rtao-auth"]
wallet     = ["dep:rtao-wallet"]                   # implies rtao-core/host (libsodium, vendored OpenSSL)
http       = ["auth", "dep:rtao-http", "rtao-http/server", "rtao-http/client"]
neuron     = ["intents", "metagraph", "http", "wallet", "dep:rtao-neuron"]
pow        = ["rtao-neuron?/pow"]                  # CPU PoW via bittensor-core (enables rtao-core/host)
gpu-pow    = ["pow", "rtao-neuron?/gpu-pow"]        # bittensor-core's OpenCL miner, loaded at runtime
ledger     = ["rtao-signer/ledger"]                # hidapi
vault      = ["rtao-signer/vault"]                 # QR/UOS
extension  = ["rtao-signer/extension"]             # browser bridge
remote-signer = ["rtao-signer/remote"]
keychain   = ["rtao-wallet/keychain"]              # macOS Keychain (security-framework)
redis      = ["rtao-http/redis"]
evm        = ["dep:rtao-evm"]
blocking   = ["rtao-client/blocking"]              # sync facade, parity with Python's SyncClient
compat-synapse  = ["dep:rtao-compat-synapse"]
compat-epistula = ["dep:rtao-compat-epistula"]
quic       = ["dep:rtao-quic"]
```

### 2.5 Edition, MSRV, toolchain

- Edition 2024 in every crate.
- **MSRV 1.89**, which is the monorepo's pinned toolchain (`UP/rust-toolchain.toml`: `channel = "1.89"`). bittensor-core is
  CI-tested there. subxt's 1.88 floor no longer applies because subxt is not used. Declared in `rust-version`, and a CI job
  builds with 1.89.
- `rust-toolchain.toml` pins the development and CI toolchain to `1.99.0`, the current stable (R/F §8).
- Async functions in traits use stable return-position `impl Future + Send`. No `async-trait` boxing is needed on hot paths.

### 2.6 Workspace manifest (abridged)

```toml
[workspace]
resolver = "3"
members = ["crates/*", "xtask"]

[workspace.package]
edition = "2024"
rust-version = "1.89"
license = "MIT"

[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false }
codec = { package = "parity-scale-codec", version = "3.7.5", features = ["derive"] }  # same version bittensor-core uses
scale-info = "2.11.6"
frame-metadata = { version = "23.0.1", features = ["current", "decode"] }              # xtask + shape hashing
scale-decode = "0.16.2"        # IgnoreVisitor for event splitting; already in the graph via merkleized-metadata
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "sync", "time", "net"] }
tokio-tungstenite = { version = "0.30", features = ["rustls-tls-webpki-roots"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["raw_value"] }
schemars = "1"
thiserror = "2"
tracing = "0.1.44"
bytes = "1"
dashmap = "6"
axum = "0.8.9"
tower = "0.5.3"
tower-http = "0.7.1"
reqwest = { version = "0.13.5", default-features = false, features = ["rustls", "json"] }
clap = { version = "4.6.7", features = ["derive", "env"] }
zeroize = "1"
docify = "0.4.1"

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

### 2.7 A subnet owner's manifests

Validator binary (hotkey on the box, CRv3 commits, metagraph sync, btauth client for querying miners):

```toml
[package]
name = "sn-foo-validator"
edition = "2024"

[dependencies]
rtao  = { git = "https://github.com/sam0x17/rtao.git", tag = "rtao-v0.6.0", default-features = false,
          features = ["neuron"] }            # neuron => intents, metagraph, http (client+server), wallet, timelock
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "signal"] }
anyhow = "1"

# Mandatory until the upstream manifest change in §1.7 lands. The tag MUST equal rtao::core::pin::BITTENSOR_CORE_TAG;
# `rtao doctor` checks it.
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Miner binary (axum server behind btauth/1, publishes its axon, no weights):

```toml
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao.git", tag = "rtao-v0.6.0", default-features = false,
         features = ["metagraph", "http", "wallet", "intents"] }   # intents => ServeAxon, ResetAxon, BurnedRegister
axum  = "0.8.9"
tokio = { version = "1.53", features = ["macros", "rt-multi-thread", "signal"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

The git URL `github.com/sam0x17/rtao` is assumed from the skeleton's author field; the repository has no remote
configured **[inferred]**. rtao ships a `cargo generate` template (`templates/subnet`) that writes both manifests,
`main.rs` skeletons (§9), a `rtao.toml` and a CI workflow that runs the subnet against the localnet image.

### 2.8 Build weight

- bittensor-core without `host`, plus `scale-value`, `scale-decode` and `frame-metadata`: 14.1 s wall / 93 s CPU clean
  release on Apple M5 Max (18 cores), rustc 1.99 **[measured]**.
- bittensor-core alone at `tag = "v477"`: 12.9 s / 84.6 s **[measured]**.
- With `host`: 330 unique crates, `cargo check` 54 s wall / 212 s CPU (R/feasibility-bittensor-core-gitdep.md).
- For comparison, bittensor-core plus subxt 0.51 is 392 crates (R/feasibility-veneer-coexistence.md). rtao avoids
  subxt's codegen and macro crates.
- rtao's own additions (tokio, tungstenite, axum, reqwest, clap, schemars) are estimated at +60–90 crates **[inferred]**.
- The generated `rtao-chain` is scoped (§3.5) and uses `parity-scale-codec` derives. It should compile faster than
  subxt's full 60,420-line module, which cost about 28 CPU-s (F §1.3) **[inferred]**.

---

## 3. Chain client

### 3.1 Async model

- tokio multi-threaded runtime. `Client` is `Clone + Send + Sync` (an `Arc<Inner>`) with no global state. All chain I/O
  is `async`.
- One background **supervisor task per `RpcSession`** owns the socket (§3.2).
- One **runtime-version watcher** follows heads and invalidates the head runtime when `spec_version` changes. This
  replaces Python's 12 s TTL re-validation (`PY/_transport/runtime.py:142-176`); the TTL stays as a fallback when head
  subscriptions are unavailable.
- CPU-heavy core work (Argon2i, PoW, Ledger HID) goes through `rtao_core::offload`.
- Typed decoding is fast enough (§0) to run inline. Bulk dynamic decoding of map pages uses bittensor-core's rayon batch
  functions (`CORE/codec/batch.rs:55-135`) via `offload` when a page holds more than 64 items, the core's
  `PARALLEL_THRESHOLD` (`:23`).
- `blocking` feature: `rtao::blocking::Client` owns a private runtime and proxies every method. This mirrors Python's
  `SyncClient`, a mechanical proxy over the one async implementation (R/A2 §1.5).

### 3.2 Transport: `rtao-transport`

Python treats its WS session semantics as product behaviour: they are battle-tested, and the core spec calls them subtle
(`UP/sdk/bittensor-core-spec.md` §3). jsonrpsee's WS client does not reconnect at all. subxt-rpcs adds a
`ReconnectingRpcClient`, but it "doesn't retain subscriptions and pending method calls when it reconnects" (R/F §1.2, `subxt-rpcs-0.51.1/src/client/reconnecting_rpc_client/mod.rs:5-12`), so it cannot express "resend
idempotent frames, never resend submits". rtao therefore owns a small session on `tokio-tungstenite`, estimated at about 1k lines (Python's `rpc.py` is 812 lines,
R/A1 §1). tokio-tungstenite also exposes the HTTP handshake response (status and headers) needed for policy refusals.

```rust
pub struct Endpoints { pub primary: Url, pub fallbacks: Vec<Url>, pub archive: Vec<Url> }

pub struct SessionConfig {
    pub handshake_timeout: Duration,      // 10 s   (PY/_transport/rpc.py:60, 86-91)
    pub silence_timeout: Duration,        // 60 s with requests pending → presumed dead (rpc.py:323, 733-741)
    pub max_retries_per_endpoint: u32,    // 5      (rpc.py:322, 633-634)
    pub retry_forever: bool,              // false
    pub subscription_buffer: usize,       // 2048, oldest dropped (rpc.py:70, 280-291)
    pub max_frame_bytes: usize,           // 64 MiB (a 2,500-UID metagraph is ~0.8 MB hex, BENCH)
    pub headers: http::HeaderMap,         // e.g. Authorization for private nodes (marked sensitive)
    pub plaintext_ws: PlaintextPolicy,    // LoopbackOnly (default) | Allow; E §4.1 item 10
    pub rate_limit: Option<governor::Quota>, // client-side, off by default
}

/// Methods that may be transparently resent after a reconnect. `author_submit*` is deliberately absent:
/// it can only go through `submit`/`submit_and_watch`, so the resend path cannot see it (type-level guarantee).
#[non_exhaustive]
pub enum ReadMethod { StateCall, StateGetStorage, StateGetStorageAt, StateQueryStorageAt, StateGetKeysPaged,
    StateGetRuntimeVersion, StateGetMetadata, ChainGetHeader, ChainGetBlock, ChainGetBlockHash, ChainGetFinalizedHead,
    SystemAccountNextIndex, SystemProperties, SystemChain, RpcMethods }

impl RpcSession {
    pub async fn connect(endpoints: Endpoints, cfg: SessionConfig) -> Result<Self, TransportError>;
    pub async fn call<R: DeserializeOwned>(&self, m: ReadMethod, params: impl Serialize + Send) -> Result<R, RpcError>;
    pub async fn call_batch(&self, calls: Vec<(ReadMethod, serde_json::Value)>) -> Result<Vec<Result<Box<RawValue>, RpcError>>, RpcError>;
    pub async fn subscribe<T: DeserializeOwned>(&self, kind: SubscriptionKind) -> Result<Subscription<T>, RpcError>;
    pub async fn submit(&self, xt: &[u8]) -> Result<H256, SubmitError>;                  // author_submitExtrinsic
    pub async fn submit_and_watch(&self, xt: &[u8]) -> Result<Subscription<TxStatus>, SubmitError>;
    pub fn state(&self) -> tokio::sync::watch::Receiver<SessionState>;  // Connected{endpoint} | Reconnecting{attempt} | Failed
    pub fn archive(&self) -> &ArchivePool;                               // lazily connected (§3.7)
}

pub enum RpcError {
    Call { code: i64, message: String, data: Option<Box<RawValue>> },
    Policy(PolicyRefusal),                  // terminal: never retried or rotated
    StateDiscarded { block: H256 },         // → archive fallback
    Timeout, Closed, Decode(serde_json::Error),
}
pub struct PolicyRefusal { pub status: Option<u16>, pub retry_after: Option<Duration>,
                           pub policy: Option<String>, pub reason: Option<String> }
pub enum SubmitError { Rpc(RpcError), Pool(PoolRejection), FateUnknown { hash: H256 } }
```

Semantics, each ported from the cited Python line and covered by a `FakeChain` test (§11.3):

- **Reconnect.** Every pending frame is kept. After reconnecting, unanswered `ReadMethod` frames are resent *before* the
  `connected` gate opens. In-flight `submit*` futures fail with `FateUnknown` (`rpc.py:686-725`).
- **Subscriptions.** Open subscriptions receive `Err(SubscriptionLost)`, carrying Python's guidance "may already be on
  chain" (`rpc.py:582-602`). Early notifications that race the subscribe response are buffered (64 ids,
  `rpc.py:297-300, 775-793`). Higher layers resubscribe (§3.11).
- **Endpoint pool.** Order is primary, then fallbacks. Rotate on connect failure or after `max_retries` consecutive
  failures. Backoff is `min(0.25·n, 5)` s, and `min(2·cycle, 30)` s between whole-pool cycles with `retry_forever`
  (`rpc.py:315-345, 648, 650-684`). Network presets come from `PY/settings.py:33-77`, including fallback and archive pools.
- **Policy refusals are terminal and surfaced** (`rpc.py:53-59, 92-124, 190-241`):
  - handshake HTTP 403/429 with `X-RateLimit-Policy`, `X-RateLimit-Reason` or `Retry-After`;
  - JSON-RPC `-32004`/`-32005` with `data.{policy,reason,retry_after}`;
  - WS close code 1008.
- **"State already discarded for <hash>"** maps to `StateDiscarded` and is retried on the archive pool
  (`PY/_substrate.py:383-400`).
- **Diagnostics are sanitized**: URL userinfo stripped, control characters removed, length bounded (`rpc.py:136-187`).

**Backend selection against public endpoints.** rtao uses only the legacy namespace that the v11 SDK uses (C2 §7):
`state_*`, `chain_*`, `author_*`, `system_*`. It does not use `chainHead_v1_*`, `archive_v1_*` or `transaction_v1_*`.
Evidence:

- subxt's auto-selecting `CombinedBackend` failed on finney's WS endpoint with `-32601`, while the legacy backend worked
  (R/feasibility-subxt.md).
- Pruned nodes advertise `archive_v1_*` but refuse it (SN2 and buyback comments, E §2.6–2.8).

At connect, rtao calls `rpc_methods` and builds a `Capabilities` set. If `author_submitAndWatchExtrinsic` or the head
subscriptions are missing (some load balancers), it uses a fallback ladder in the style of the user's `mev-scanner`
(E §6):

- submit with `author_submitExtrinsic`, then follow inclusion by polling `chain_getBlock` from the submission height
  until `birth + period`;
- poll heads at `slot/4`, clamped to 50 ms..3 s (bittensor-core's polling cadence, `CORE/client.rs:338-355`).

**Rate limits.** Public endpoints are documented at about 1 request/s/IP (G §4.1). rtao never retries a policy refusal.
`SessionConfig::rate_limit` is an opt-in token bucket for public endpoints. Heavy reads default to runtime APIs over
storage scans, and `query_map` pages default to 100 keys (`PY/_transport/interface.py:304`). The "storage work rate limit
exceeded" (`-32004`) is the trigger to avoid (R/A2 §7).

### 3.3 Runtime cache and metadata

```rust
pub struct RuntimeEntry {
    pub spec: SpecVersion, pub tx_version: u32, pub genesis: H256,
    pub core: Arc<bittensor_core::runtime::Runtime>,   // parsed V15
    pub chain_info: rtao_core::digest::ChainInfo,       // spec name "node-subtensor", prefix 42, decimals 9, "TAO"
    shapes: ShapeCache,                                 // DashMap<DescriptorId, ShapeStatus>, filled lazily (§3.5)
    indices: IndexCache,                                // (pallet, call) → (u8, u8) and (pallet, event) lookups by name
}
pub struct RuntimeCache { /* LRU(4) spec → Arc<RuntimeEntry>; LRU(512) block hash → spec; disk tier */ }
impl RuntimeCache {
    pub async fn at_head(&self) -> Result<Arc<RuntimeEntry>>;
    pub async fn for_block(&self, hash: H256) -> Result<Arc<RuntimeEntry>>; // spec from the PARENT's runtime version
}
```

- **Metadata fetch.** `state_call("Metadata_metadata_at_version", 0x0f000000)`, unwrapping the
  `Option<OpaqueMetadata>`, with `state_getMetadata` as the fallback (`PY/_transport/runtime.py:292-310`). V15 parses in
  1.0 ms per 400,688-byte blob **[measured]**. Python's original codec build took 337 ms
  (`UP/sdk/bittensor-core-spec.md:360-382`). A missing `transactionVersion` is a hard error (`runtime.py:42-54`).
- **Per-block resolution.** Block → parent hash → `state_getRuntimeVersion(parent)` → spec → entry
  (`runtime.py:178-207`). Concurrent fetches for one spec share a future (`runtime.py:209-238`).
- **Disk tier.** It is byte-compatible with Python's cache: `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache`,
  files named `md1-<genesis16hex>-<spec>.bin`, each holding magic `STMD` + `<IB>` (`transaction_version` u32 LE,
  `is_v15` u8) + raw blob. Writes are atomic and the 8 newest are kept (`runtime.py:36-121`). Rust and Python processes on
  one host share it.
- **Block-hash caching** applies only to blocks at or below the finalized head, which avoids reorg hazards
  (`PY/_transport/interface.py:187-216`).

### 3.4 Typed vs dynamic: the codec decision

The options, with measurements:

| Option | Decode speed (256 / 2,500 UIDs) | Drift detection | Fit with bittensor-core | Notes |
|---|---|---|---|---|
| bittensor-core dynamic `Value` | 330 µs / 2.68 ms **[measured]** | none (shapes are whatever arrives) | native | Python-compatible shapes, pinned by the 2,899-sample shape corpus (`UP/sdk/python/tests/fixtures/shape_corpus/corpus.json`); AccountIds become ss58 strings, ~45% of the time |
| subxt static (`DecodeAsType` on generated types) | 1,380 µs / 11.0 ms; 776 µs / 6.5 ms hot-only **[measured]** | 32-byte validation hash per address (`subxt/src/storage/address.rs:89-93`, R/F §1.2) | second metadata engine (V16 via subxt-metadata) and second signing path | 8 breaking minors in 23 months; jsonrpsee bumped inside a patch release (R/F §1.7) |
| `scale-value` dynamic | 911 µs / 7.31 ms **[measured]** | none | redundant with the core's `Value` | — |
| **generated `parity-scale-codec` types + rtao shape hash** | **29.8 µs / 261 µs [measured]** | structural hash per descriptor, checked against the live registry per spec (below) | uses the core's `Runtime` registry for the check | positional, so it is only safe when shapes match; hence the hash gate |
| borrowed columnar view (metagraph only) | **6.5 µs / 57 µs [measured]** | same hash gate | — | hand-written parser for the single hottest type (§8.2) |

The feasibility notes agree on the non-decode costs:

- subxt dynamic-only is 228 crates and 155 CPU-s for a clean release build; the 60k-line static module adds ~28 CPU-s
  (R/feasibility-subxt.md; F §1.3, §1.8).
- bittensor-core's `Runtime::parse` took 3.9 ms in the coexistence run, for a single cold parse of the
  `state_getMetadata` (V14) blob in a debug-profile program (R/feasibility-veneer-coexistence.md). Here it is 1.0 ms,
  the average of 20 warm release-mode parses of V15 (Appendix A). Both are negligible next to the RPC.
- Upstream's own acceptance table puts bittensor-core decode at 60–77 MB/s *including* Python object materialization
  (`UP/sdk/bittensor-core-spec.md:360-382`). That is consistent with the ~150 MB/s measured here for the Rust-only
  `Value` construction.

**Decision.** Use generated typed decoding gated by shape hashes, with the dynamic `Value` path as fallback and for JSON.

**Why not subxt.** None of subxt's strengths outweighs having a second engine:

- typed signatures are generated by rtao's own codegen;
- block following is done by the transport (§3.11);
- the watch state machine is ported from Python (§3.8);
- historic pre-V14 decoding is not supported by Python either, because bittensor-core parses only V14 and V15
  (`CORE/runtime/mod.rs:163-199`; `runtime.py:292-310` falls back to V14 at most).

subxt would add a metadata engine, a signer, extension handling and its release churn, all of which bittensor-core
already covers. The measured coexistence (R/feasibility-veneer-coexistence.md) shows rtao *could* add subxt later behind
an adapter, because the SCALE stack unifies to single versions.

### 3.5 Code generation from metadata (`xtask codegen` → `rtao-chain`)

**Inputs (committed).**

- `metadata/<network>/<spec>.v16.scale` and `.v15.scale`, recorded at the first finalized block of the runtime.
- `metadata/index.toml` with `{network, spec, block_hash, code_hash, sha256_v15, sha256_v16, recorded_at}`.
- The procedure follows H §3.3. The localnet image's metadata is *not* a substitute: it has `faucet` at call 60 and
  fast-runtime constants (H §1.1).

**Scope** (`codegen/scope.toml`):

- the 13 pallets of Python's coverage gate (`UP/sdk/python/codegen/check.py` `COVERED_PALLETS`): SubtensorModule,
  Balances, Proxy, Utility, Multisig, Crowdloan, Swap, LimitOrders, MevShield, EVM, Commitments, AdminUtils, Contracts;
- plus `System` (`Account`, `Events`, `Number`), `Timestamp.Now`, `Aura.SlotDuration`, `Drand` (`LastStoredRound`,
  `Pulses`), `TransactionPayment` events, `Sudo`;
- every subtensor runtime API (`SubnetInfoRuntimeApi` v2, `NeuronInfoRuntimeApi`, `StakeInfoRuntimeApi`,
  `DelegateInfoRuntimeApi`, `SwapRuntimeApi`, `SubnetRegistrationRuntimeApi`, `BetaBasketRuntimeApi` v5,
  `ProxyFilterRuntimeApi`, `ShieldApi`) plus `TransactionPaymentApi`, `AccountNonceApi` and `Metadata` (C2 §1.0).

**Outputs** (`crates/rtao-chain/src/generated/`, rustfmt'd):

- `types.rs`: every registry type reachable from the scope, as plain Rust with
  `#[derive(Encode, Decode, Clone, Debug, PartialEq, Eq)]` and an optional `serde`.
- `calls/<pallet>.rs`: one struct per call with typed fields, plus

  ```rust
  impl Call for SetMechanismWeights {
      const PALLET: &'static str = "SubtensorModule";
      const NAME: &'static str = "set_mechanism_weights";
      const SHAPE: ShapeHash = ShapeHash(hex!("…"));
      const PINNED_INDEX: (u8, u8) = (7, 119); // documentation and golden tests; live indices are resolved by name
  }
  ```

- `storage/<pallet>.rs`, for example:

  ```rust
  pub const TEMPO: StorageEntry<(NetUid,), u16, ValueQuery> =
      StorageEntry::new("SubtensorModule", "Tempo", &[Hasher::Identity], prefix_hex!("…"), ShapeHash(…));
  ```

  `ValueQuery`/`OptionQuery` comes from the metadata modifier, so `Default` versus `Optional` miss semantics are in the
  type.
- `runtime_apis/<api>.rs`, for example:

  ```rust
  pub const GET_METAGRAPH: RuntimeApiMethod<(NetUid,), Option<Metagraph>> =
      RuntimeApiMethod::new("SubnetInfoRuntimeApi", "get_metagraph", ShapeHash(…));
  ```

  This includes `api_version` gating: `get_subnet_hyperparams_v3` is `api_version(2)`, `BetaBasketRuntimeApi` methods
  span v2–v5 (R/A2 §5.7 item 1).
- `events/<pallet>.rs`: one struct per event with `impl Event { PALLET, NAME, SHAPE }`. Events are never matched by index.
- `constants/<pallet>.rs`: `Constant<T>` descriptors.
- `errors.rs`: `(pallet_index, error_index) → (pallet, name, docs)` for every pallet (the Python `_generated/errors.py`
  analogue, 405 entries).
- `deprecations.rs`: from V16 `deprecation_info`, emitted as `#[deprecated(note = "…")]` on calls such as
  SubtensorModule 70, 73, 139 and 140 (H §1.1).
- `extensions.rs`: the expected transaction-extension list (13 identifiers, with their `ty`/`implicit` shapes) for the
  startup guard (§3.8).
- `metagraph_field.rs`: `MetagraphField` (77 variants) derived from the field order of `SelectiveMetagraph`. That order
  matches `SelectiveMetagraphIndex` 0..=76 one-to-one: `netuid` first, `collateral_earned` = 76
  (`UP/pallets/subtensor/src/rpc_info/metagraph.rs:119-240` vs `:488-650`) **[verified]**. A vendored copy of the enum
  names is asserted equal, as the drift tripwire.

**Substitutions and overrides.** Type-driven units come from codegen, not hand tables:

- `codegen/substitutions.toml` maps runtime paths to rtao types: `subtensor_runtime_common::NetUid` →
  `rtao_units::NetUid`, `…::MechId`, `…::NetUidStorageIndex`, `…::currency::TaoBalance` → `TaoRao`, `…::AlphaBalance` →
  `AlphaRao`, `sp_arithmetic::per_things::PerU16` → `PerU16`, `sp_core::crypto::AccountId32` → `AccountId`.
  `substrate_fixed::FixedU128<Frac>` / `FixedI128<Frac>` / `FixedI64<Frac>` map to `U64F64`/`I96F32`/`I64F64`/`I32F32`
  by decoding the typenum `Frac` from the type parameters (R/F §4).
- `codegen/overrides.toml` holds semantic fixes the metadata cannot express:
  - `Metagraph.total_stake: Vec<Compact<TaoBalance>>` → `Vec<Compact<StakeWeightRao>>`. It is
    `alpha + tao·tao_weight`, not TAO (R/A2 §4.1; `stake_utils.rs:269-320`).
  - `Owner`, `Keys`, `SubnetOwnerHotkey` are generated with a `ZeroAccountIsNone` query, so the all-zero default decodes
    to `None` (R/A2 §9 item 9).
  - `limit_price` arguments of `add_stake_limit`, `remove_stake_limit`, `remove_stake_full_limit` and siblings are
    declared `TaoBalance` (`R/_call_index_map.txt` rows 34, 35, 46), but they are prices (rao per alpha), not amounts. They
    become `PriceRao`, so an amount cannot be passed as a limit.
  - `name`/`symbol: Vec<Compact<u8>>` → `CompactBytesText`. Bytes ≥ 64 take two bytes each (C2 §1.1.1). The benchmark's
    first borrowed parser tripped on exactly this.

**Shape hash.** It is structural and independent of type ids, which are not stable across specs
(`UP/sdk/bittensor-core-spec.md` §4.2):

```
shape(id) = blake2_256(SCALE(node(id)))
node(id)  = match def {
  Composite(fs)   => ("C", [(f.name?, shape(f.ty)) ...]),
  Variant(vs)     => ("V", [(v.index, v.name, [(f.name?, shape(f.ty)) ...]) ...]),
  Sequence(t)     => ("S", shape(t)),     Array(n, t) => ("A", n, shape(t)),
  Tuple(ts)       => ("T", [shape(t)...]), Compact(t)  => ("K", shape(t)),
  Primitive(p)    => ("P", p),             BitSequence(o, s) => ("B", shape(o), shape(s)),
}  // a type id already on the recursion stack contributes ("R", depth); paths and docs are excluded
```

- Paths are excluded because path-only renames do not change bytes. An example is the runtime newtypes moving between
  crates, which reshuffles subxt's generated `runtime_types::…` paths (R/F §10 item 12).
- Field names are included, so a reordering or rename that keeps bytes compatible still invalidates positional decode.
  That is the conservative choice.
- `xtask` computes the hashes for the pinned metadata. `RuntimeEntry::check(desc)` computes and caches them for a live
  spec on first use. Any mismatch is a typed `ShapeMismatch { descriptor, pinned, live }` (§3.6).

**Regeneration per runtime release.**

- `cargo xtask codegen --network finney --spec <N>` runs from committed files only.
- Generated headers omit `spec_version`, as Python's do (`UP/sdk/python/codegen/emit_python.py:22-30`). A spec bump that
  changes no surface, the common case per R/CORRECTIONS.md item 2 (477→478 differs only in the `System.Version`
  constant), produces a zero-line diff apart from `metadata/index.toml` and `SPEC_RANGE`.
- The PR gate `cargo xtask codegen --check` regenerates offline and diffs. The drift job opens regeneration PRs (§12.2).

### 3.6 Typed and dynamic access API

```rust
pub trait View: Send + Sync {
    fn at_block(&self) -> Option<BlockRef>;                       // None = head (Client); Some = pinned (Snapshot)
    fn runtime(&self) -> impl Future<Output = Result<Arc<RuntimeEntry>>> + Send;
    fn pin(&self) -> impl Future<Output = Result<Snapshot>> + Send; // Client → Snapshot at head; Snapshot → clone

    fn storage<K: StorageKey, V: Decode, Q: Query>(&self, e: &StorageEntry<K, V, Q>, key: K::Args)
        -> impl Future<Output = Result<Q::Out<V>>> + Send;        // ValueQuery → V; OptionQuery → Option<V>
    fn storage_batch<K, V, Q>(&self, e: &StorageEntry<K, V, Q>, keys: impl IntoIterator<Item = K::Args> + Send)
        -> impl Future<Output = Result<Vec<Q::Out<V>>>> + Send;   // one state_queryStorageAt at one hash
    fn storage_iter<K, V, Q, P: StoragePrefix<K>>(&self, e: &StorageEntry<K, V, Q>, prefix: P)
        -> impl Stream<Item = Result<(K::Decoded, V)>> + Send;    // getKeysPaged + queryStorageAt per page, pinned
    fn runtime_api<I: Encode, O: Decode>(&self, m: &RuntimeApiMethod<I, O>, input: I)
        -> impl Future<Output = Result<O>> + Send;                // state_call("{Api}_{method}")
    fn constant<T: Decode>(&self, c: &Constant<T>) -> impl Future<Output = Result<T>> + Send;
    fn events(&self) -> impl Future<Output = Result<Events>> + Send;

    // Dynamic escape hatches: bittensor-core Value, Python-shaped (used by `rtao query --json` and raw tooling).
    fn storage_dyn(&self, pallet: &str, item: &str, keys: &[Value]) -> impl Future<Output = Result<Value>> + Send;
    fn runtime_api_dyn(&self, api: &str, method: &str, args: &[Value]) -> impl Future<Output = Result<Value>> + Send;
}

pub enum ReadError { Shape(ShapeMismatch), NotInRuntime(String), Rpc(RpcError), Decode(codec::Error),
                     ApiVersion { api: &'static str, need: u32, have: u32 } }
```

- **Keys** are hashed with `rtao_core`'s `hash_param` over `K::Args::encode()`, with prefixes precomputed. Maps iterate
  pinned to one hash, and free key components are recovered for reversible hashers (`Blake2_128Concat` 16 bytes,
  `Twox64Concat` 8, `Identity` 0; `CORE/codec/storage.rs:46-57,96-164`).
- **Shape mismatch.** Typed accessors return `ReadError::Shape` and never mis-decode. Reads in `rtao-reads` declare a
  `Fallback::Dynamic` strategy, which re-reads through `*_dyn` and converts with a per-read `TryFrom<Value>`, or
  `Fallback::Fail`.
- **Events** come from `System.Events` at the view's block. They are split per record using the live registry: pallet
  and variant names are resolved from indices, and fields are skipped with `scale-decode`'s `IgnoreVisitor`.
  `events.find::<NetworkAdded>()` decodes only records whose `(pallet, name)` matches and whose per-event shape hash
  matches. The fallback is `events.iter_dyn()` (bittensor-core `decode_id` on the record). This follows H §2.2, where
  event 149 changed meaning between 461 and 477.

### 3.7 Views, snapshots and block pinning

- `Client` implements `View` at the head. `client.at(BlockId::{Head, Finalized, Number(n), Hash(h)})` returns a
  `Snapshot { block: BlockRef { number, hash }, runtime: Arc<RuntimeEntry>, client }`. Every call passes `at = hash`.
  Constants come from the snapshot's own runtime, an improvement on Python, which always reads them from the head codec
  (`PY/snapshot.py:66-68`).
- **Pinning is enforced by type, not convention.** A read declares `const PINNING: Pinning` (§4.5). The dispatcher calls
  `view.pin()` for `Pinning::Required` reads before `fetch`, replacing Python's easy-to-forget `view = await view.at()`
  (R/A2 §1.3 lists four unpinned multi-RPC reads). A `Pinning::Gather` read gets the unpinned view and must document why.
- **Archive fallback.** A `StateDiscarded` error on any pinned read retries once on the archive pool. Lite nodes keep
  about 300 blocks (C2 §6).

### 3.8 Extrinsic pipeline

**Composed calls carry their spec.**

```rust
pub struct CallBytes { bytes: Bytes, spec: SpecVersion, hash: [u8; 32] /* blake2_256(bytes) */ }
pub trait Call: Encode + Send + Sync { const PALLET: &'static str; const NAME: &'static str; const SHAPE: ShapeHash; }
impl RuntimeEntry {
    /// Resolve (pallet, call) indices by NAME on the live runtime, verify SHAPE, encode `[p, c] ++ args`.
    pub fn compose<C: Call>(&self, call: &C) -> Result<CallBytes, ComposeError>;
    /// Raw/dynamic path (`rtao call`): bittensor-core compose_call over Value (CORE/codec/encode.rs:532-585).
    pub fn compose_dyn(&self, pallet: &str, call: &str, args: &Value) -> Result<CallBytes, ComposeError>;
}
```

- Nesting (Sudo, Proxy, Utility, Multisig) splices `CallBytes`. Composing two `CallBytes` from different specs is an
  error, so the dry run and the signed bytes cannot diverge across an upgrade. This parallels
  `PY/intents/plan.py:89-135` ("execute submits exactly `plan.call`").
- **Startup guard on the extension list.** On first use of a spec, compare the live V15 `signed_extensions` against
  `rtao-chain`'s expected list. Fail closed if an unknown identifier has a non-zero-sized `ty` or implicit. bittensor-core
  already fails closed for such extensions (`CORE/codec/extrinsic.rs:89-118,123-158`), so this only turns a late codec
  error into an early typed one. Live finney V15 lists the 13 expected identifiers **[measured]**.

**Nonces.** RAII leases replace Python's dict cache:

```rust
pub struct NonceManager { accounts: DashMap<AccountId, Arc<parking_lot::Mutex<NonceState>>> }
// A sync mutex, never held across `.await`, so Drop can settle a lease. The first-use chain fetch is coordinated per
// account by a tokio::sync::OnceCell outside the lock.
pub struct NonceLease { account: AccountId, nonce: u32, state: Arc<…>, settled: bool }
impl NonceManager {
    pub async fn lease(&self, who: &AccountId) -> Result<NonceLease>;          // first use: system_accountNextIndex
    pub async fn lease_explicit(&self, who: &AccountId, n: u32) -> NonceLease; // pin (shield inner = n+1)
}
impl NonceLease {
    pub fn get(&self) -> u32;
    pub fn accepted(mut self) { self.settled = true; }                 // pool accepted: keep local increment
    pub fn rejected(mut self) { /* clear account cache */ self.settled = true; }
}
impl Drop for NonceLease { fn drop(&mut self) { if !self.settled { /* clear cache: fate unknown or local failure */ } } }
```

These match Python's rules:

- first request per account asks the chain, later ones increment locally (`PY/_transport/extrinsics.py:55-84`);
- any failure between signing and inclusion clears the account (`PY/_substrate.py:753-772`);
- prepared-for-external-signing extrinsics always fetch fresh (`interface.py:589-590`).

The `Drop` impl makes "forgot to clear on an error path" impossible.

**Mortality.**

```rust
pub enum Mortality { Immortal, Mortal(Period) }            // default Mortal(Period::P128) (PY/settings.py:22)
pub struct Period(u32);                                    // power of two in 4..=65536 after era_birth rounding
pub enum ShieldPeriod { P4, P8 }                           // the ONLY era type the shield path accepts (chain max 8)
```

- Mortal eras anchor at the **finalized** head (Python parity, `interface.py:485-499`). The birth comes from
  `bittensor_core::era_birth` (`CORE/codec/extrinsic.rs:407-412`), and the era block hash from the finalized-only hash
  cache.
- The default period is 128, Python's product choice. bittensor-core's 64 is not used.

**Signing.**

1. `UnsignedExtrinsic::payload()` yields `SigningPayload { bytes, parts: (extra, additional), call, params }` via
   `signature_payload_parts` and `signature_payload` (blake2 if longer than 256 bytes).
2. The `Signer` returns a `Signature` (§6.3).
3. `attach()` calls `encode_signed_extrinsic`. Its variant byte is the `MultiSignature` index = crypto type, and its
   length is validated (`CORE/codec/extrinsic.rs:260-316`).

`CheckMetadataHash` is `Disabled` (`0x00`/`None`) unless the signer declares `METADATA_DIGEST`. In that case
`metadata_digest(v15_bytes, chain_info)` goes into `TxParams.metadata_hash` and the mode becomes `Enabled`
(`CORE/codec/extrinsic.rs:181-207,217-236`).

**Fees.**

- `estimate_fee(call, signer_address)` assembles an immortal extrinsic with a zeroed 64-byte signature at the current
  nonce, then calls `state_call("TransactionPaymentApi_query_info", xt ‖ u32_le(len))`.
- The result is decoded with the generated `RuntimeDispatchInfo` (u64 balance; the u128 assumption is wrong for
  subtensor, per E §2.8). This follows `PY/_transport/interface.py:714-731`.
- `estimate_weight` reads `weight` from the same call, for multisig `max_weight`.

**Submission and outcome.**

```rust
pub enum Wait { Pool, Inclusion, Finalization }           // execute default: Finalization (PY/executor.py:782-788)
pub struct Submission { pub hash: [u8; 32], pub birth: BlockNumber, pub period: Option<Period> }
pub enum TxStatus { Future, Ready, Broadcast, InBlock(H256), Retracted(H256), FinalityTimeout(H256),
                    Finalized(H256), Usurped(H256), Dropped, Invalid }
pub struct Receipt { pub block: BlockRef, pub index: u32, pub events: Events, pub fee: Option<Balance<Tao>>,
                     pub weight: Weight, pub outcome: DispatchOutcome }
pub enum DispatchOutcome { Success,
    Failed(ChainError),                                    // System.ExtrinsicFailed: Module{index, error[0]} → name/docs
    NestedFailed { wrapper: Wrapper, error: ChainError },  // Sudid/SudoAsDone/ProxyExecuted/MultisigExecuted = Err,
                                                           // Utility.BatchInterrupted/ItemFailed (PY/_transport/utils/receipt.py:8-52)
    ShieldFailed(ShieldFailure) }                          // MevShield.* failure events (receipt.py:104-167)
```

- The watch follows `author_submitAndWatchExtrinsic`. `usurped`, `retracted`, `finalityTimeout`, `dropped` and `invalid`
  are terminal errors (`PY/_transport/extrinsics.py:352-374`).
- On `inBlock`/`finalized`, rtao fetches the block, finds the extrinsic by blake2_256 of the raw bytes, filters events by
  index, and derives success/failure, nested failures, and the fee from `TransactionPayment.TransactionFeePaid.actual_fee`
  (`interface.py:677-712`; `receipt.py:75-96`). It never fabricates an outcome: if the extrinsic is not in the block, the
  result is `ExtrinsicNotFound`.
- **Watch lost on reconnect.** Python returns "fate unknown" (`rpc.py:686-711`). rtao returns
  `SubmitError::FateUnknown { hash }` by default. With `ExecOptions::reconcile = true`, it scans blocks
  `birth..=birth+period` for the hash before giving up. This is SN2's reconcile pattern (E §2.6), made finite by mortality.
- **Pool rejections are parsed into a typed `PoolRejection`**: `Custom(u8)` resolved through `CUSTOM_TX_ERRORS`
  (codes 0–27 and 255, `PY/error_map.py:488-518`), and standard `InvalidTransaction` variants by Python's needle table
  (`error_map.py:524-541`).
- **Retries.** `ExecOptions::retries` (default 0) retries only `PriorityTooLow` and `Stale` *on the normal path*, waiting
  one block. Python's substring test also matched the shield era rejection (R/A1 §10 item 5). rtao's typed
  classification never retries `Stale` for a shielded carrier, because that means the era was wrong.

**Offline and external signing.** `prepare_call(call, address, opts) -> PreparedExtrinsic`, then
`submit_signature(prepared, sig)`.

- `PreparedExtrinsic` serializes to the same JSON as Python's `UnsignedExtrinsic.to_dict`
  (`PY/_transport/contract.py:67-134`). That covers the `SignerPayloadJSON` with big-endian SCALE-width hex numerics
  (`extrinsics.py:87-132`), so Python-prepared payloads can be signed by Rust tools and the reverse.
- It refuses to submit if `spec_version` **or** `transaction_version` changed. Python checks spec only (R/A1 §10 item 16).

### 3.9 Shielded submission (MEV shield)

This ports `Executor._submit_encrypted_call` and `_resolve_shielded_inner` (`PY/executor.py:1016-1214`) with the chain's
limits in types:

1. Read `MevShield.NextKey` through the typed storage entry. Retry twice, one block apart, if absent
   (`executor.py:1027-1034`).
2. `n = system_accountNextIndex(signer)`. Lease `n` explicitly for the carrier and `n+1` for the inner extrinsic.
3. Sign the inner call at `n+1` with `ShieldPeriod::P8`.
4. `rtao_core::shield::seal(pk, inner_bytes)` wraps `mlkem::seal(pk, inner, include_key_hash = true)`. It returns
   `ShieldCiphertext`, a newtype whose constructor rejects more than 8,192 bytes (the `BoundedVec<u8, 8192>` bound, E §1)
   before signing.
5. Compose `MevShield.submit_encrypted(ciphertext)` and sign it at `n` with `ShieldPeriod::P8`. Submit with `Wait`
   default `Inclusion` (`executor.py:905-907`).
6. Resolve the inner extrinsic by scanning from the carrier's inclusion block to `included_at + 8` for the inner hash,
   with bounded waits. Under `Finalization`, rescan if a reorg replaced the block. If it never lands, the result becomes
   "the decrypted extrinsic … was not included before its era expired" (`executor.py:1205-1214`).
7. **Policy and fees.** With `max_fee` set and no estimate, the result is a hard `PolicyError`, matching Python's
   asymmetry (R/A1 §10 item 4). If the intent reports `required_free`, the carrier fee is estimated by pricing
   `submit_encrypted` with an 8,192-byte ciphertext, and execution blocks when reserve plus carrier exceeds free TAO
   (`executor.py:955-979`).

`Intent::MEV` is `MevShield::{Off, Default, Required}`. `Required` redirects `execute` to the shielded path
(`executor.py:812-824`). `Default` affects only the CLI's default (R/A1 §10 item 12).

### 3.10 Proxy, sudo, batch and multisig composition

The composition order is ported verbatim from `_compose_intent_call` (`PY/executor.py:175-219`):

```
semantic = intent.semantic()                         // adapters unwrap to the inner intent
origin   = proxy_for.map(OriginView::proxied).unwrap_or(intent.origin_view(keys))
built    = semantic.build(&BuildCx { view, origin, .. })
call     = if I::Origin == Root && !is_sudo(built) { Sudo::sudo(built) } else { built }
call     = if let Some(real) = proxy_for { Proxy::proxy(real, force_proxy_type, call) } else { call }
call     = intent.wrap_call(call)                    // saved-multisig adapter: Multisig.as_multi / as_multi_threshold_1
```

- `ProxyType` is generated from metadata, a 19-variant enum (`PY/intents/proxy.py:21-41`). A string the chain does not
  know is a parse error, not a runtime failure.
- **Batch.** `Utility.batch_all` only. Nesting is refused. Spend aggregates (any unbounded child makes the batch
  unbounded), netuids are the union, all children must share one signer role (enforced by a `Batch<R: Role>` type
  parameter), and extras are namespaced `"{i}:{op}.{key}"` (`PY/intents/batch.py:54-164`).
- **Multisig.** The account comes from `rtao_core::multisig_account_id`, with signatories sorted by raw bytes
  (`CORE/codec/extrinsic.rs:419-437`). `Multisig.Multisigs[account, call_hash]` gives the existing timepoint.
  `max_weight` comes from `estimate_weight`. The funds preflight covers the deposit base plus threshold times the factor
  (`PY/multisig.py:121-158`). Later rounds pin the exact prior call bytes, because rebuilds are not byte-stable (timelock
  randomness, `PY/executor.py:185-193`).
- `register_subnet` completion: §9.4.

### 3.11 Block streams and waiting helpers

- `client.blocks(Finality::{Best, Finalized}) -> BlockStream` follows `chain_subscribe{New,Finalized}Heads`. Unlike
  Python's stream, which dies on reconnect (`rpc.py:582-602`), it **resubscribes and backfills** missed heights by number
  through `chain_getBlockHash(n)`, so consumers see every height exactly once (finalized) or are told about reorgs (best).
- `wait_for_block(n)`, `wait_for_timestamp(t)` and `wait_for_epoch(netuid) -> EpochEvent` port `PY/client.py:376-444`.
  `wait_for_epoch` watches `LastEpochBlock[netuid]`; it does not use a modular formula (C2 §0 item 6).
- `EpochClock` combines `get_next_epoch_start_block`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex` and the
  admin freeze window to answer "next epoch at", "is the admin window open" and "blocks until reveal" (§9).
- `block_time()` comes from `Aura.SlotDuration` (12 s on mainnet, 0.25 s on fast localnets; never hard-coded,
  `PY/_substrate.py:423-433`).

---

## 4. Intents and reads parity

The target is 83 intents (`CAT/intents.json`, 83 entries, matching 83 `@register` decorators, R/A1 §2.3) and 102 reads
in 14 categories (`CAT/reads.json`, R/A2 §3). By signer: 75 intents are coldkey-signed and 8 hotkey-signed
(`associate_evm_key`, `commit_weights`, `reset_axon`, `reveal_weights`, `serve_axon`, `serve_axon_tls`,
`serve_prometheus`, `set_weights`). By origin: 74 `signed`, 7 `subnet_owner`, 2 `root`. Together they wrap 116 distinct
`(pallet, call)` pairs **[verified with jq over `CAT/intents.json`]**.

### 4.1 The `Intent` trait

```rust
// rtao-signer (Role markers live at the signing layer because KeySource<R: Role> needs them; rtao-tx re-exports)
pub trait Role: sealed::Sealed + Send + Sync + 'static { const ROLE: RoleKind; }
pub enum Coldkey {}                        impl Role for Coldkey { const ROLE: RoleKind = RoleKind::Coldkey; }
pub enum Hotkey {}                         impl Role for Hotkey  { const ROLE: RoleKind = RoleKind::Hotkey;  }

// rtao-tx
pub trait OriginKind: sealed::Sealed + 'static { const ORIGIN: Origin; }
pub enum Signed {}  pub enum SubnetOwner {}  pub enum Root {}   // Root ⇒ executor wraps Sudo.sudo

pub trait Intent: Serialize + DeserializeOwned + JsonSchema + Clone + fmt::Debug + Send + Sync + 'static {
    const OP: &'static str;                              // registry key; CLI `rtao tx <op-kebab>`
    type Role: Role;                                     // which wallet key signs
    type Origin: OriginKind;                             // dispatch privilege
    type Pending: Send + 'static;                        // build-time data (e.g. a reveal round); () for most
    type Output: IntentOutput;                           // final typed result data; () for most
    const WRAPS: &'static [ChainCall];                   // generated call descriptors (coverage gate input)
    const VERIFY: Option<&'static str> = None;           // read that confirms the effect (mandatory if Origin = Root)
    const MEV: MevShield = MevShield::Off;               // Off | Default (CLI default) | Required
    const ALL_AMOUNT_FIELDS: &'static [&'static str] = &[];

    fn summary(&self) -> Cow<'static, str>;
    fn spend(&self) -> Spend;                            // None | Bounded(Balance<Tao>) | Unbounded
    fn netuids(&self) -> NetuidScope;                    // Listed(SmallVec<[NetUid; 2]>) | All
    fn build<V: View>(&self, cx: &BuildCx<'_, V>)
        -> impl Future<Output = Result<Built<Self::Pending>, IntentError>> + Send;
    fn preflight<V: View>(&self, cx: &PreflightCx<'_, V>)
        -> impl Future<Output = Result<IntentPreflight, IntentError>> + Send { default_preflight(self, cx) }
    /// Turn build-time data plus the receipt into the typed output. It may read chain state: register_subnet follows
    /// heads here until NetworkAdded (§9.4). The derive supplies the trivial impl when Pending = Output = ().
    fn finish<V: View>(&self, cx: &FinishCx<'_, V>, pending: Self::Pending, receipt: &Receipt)
        -> impl Future<Output = Result<Self::Output, IntentError>> + Send;
}

pub struct Built<P> { pub call: CallBytes, pub pending: P }
pub struct IntentPreflight { pub effects: Vec<String>, pub warnings: Vec<String>, pub blocks: Vec<String>,
    pub required_free: Option<Balance<Tao>>, pub available_free: Option<Balance<Tao>>,
    pub estimated_fee: Option<Balance<Tao>>, pub facts: Vec<(String, String)>, pub facts_title: FactsTitle }
```

What the types buy, relative to Python's runtime checks:

- **Signer role.** `Executor::execute<I, K: KeySource<I::Role>>` (§4.4). A validator binary that loads only a hotkey file
  can call `execute(&SetWeights{..}, &hotkey)`. Writing `execute(&Transfer{..}, &hotkey)` is a compile error. Python
  checks this at run time via `resolve_signer` (`PY/signing.py:227-263`).
- **Origin.** `Root` intents must set `VERIFY`, enforced by a `const` assertion the derive emits. Python enforces it in
  CI (`check.py:325-333`).
- **Outputs.** Python puts extras in `ExtrinsicResult.data: dict` (`PY/result.py:697-750`). rtao types them:
  `SetWeights::Output = WeightsSubmitted { path: WeightsPath, reveal_round: Option<u64> }`,
  `RegisterSubnet::Output = SubnetRegistered { netuid, mode, registered_at_block, registration_price, … }`
  (`PY/executor.py:358-386,501-526`), `CreatePureProxy::Output = PureCreated { pure, spawner, height, ext_index }`
  (`executor.py:571-593`), `MultisigExecute::Output = MultisigCall { call_hash, call_data }`.
- **Wraps.** `WRAPS` lists generated call descriptors, for example
  `&[calls::subtensor_module::SetMechanismWeights::DESC, calls::subtensor_module::CommitTimelockedMechanismWeights::DESC]`.
  If a runtime renames or removes a wrapped call, regenerating `rtao-chain` breaks the intent's compilation. That is the
  earliest possible drift signal.
- **Units.** Intent fields use domain units (§5). A `RemoveStake` amount is `Amount<Alpha>`, and building the call
  converts it to `AlphaRao` only if its netuid equals the call's netuid. Python raises `ValueError` at run time
  (`PY/intents/_money.py:152-163`).

Adapters reuse the trait:

- `MultisigAdapter<I: Intent<Role = Coldkey>>` and `MultisigThreshold1Adapter<I>` override `wrap_call`/`origin_view` and
  forward `semantic()` to the inner intent (`PY/intents/multisig.py:276-398`).
- `ProxyAnnounced<I>` (`execute_proxy_announced`) keeps Python's quirk of building the inner intent against the signing
  wallet (R/A1 §10 item 6). It is documented, and listed in `parity/divergences.toml` as "kept".

### 4.2 Derive macro and registry

```rust
/// Set weights for miners on a subnet.                     // first line = summary; full doc = description
///
/// Auto-selects plain `set_mechanism_weights` or a timelocked commit from `CommitRevealWeightsEnabled`.
#[derive(Intent, Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[intent(op = "set_weights", role = Hotkey, origin = Signed, pending = Option<Round>, output = WeightsSubmitted,
         wraps(SubtensorModule::set_mechanism_weights, SubtensorModule::commit_timelocked_mechanism_weights))]
#[serde(deny_unknown_fields)]          // Python's from_args rejects unknown args (PY/intents/base.py:357-399)
pub struct SetWeights {                 // fields mirror upstream input_schema exactly; only `netuid` is required
    /// Subnet whose miners the weights score.
    pub netuid: NetUid,
    /// Miner UIDs, parallel to `weights`. Omit when `weights` is a uid→weight map.
    #[serde(default)] pub uids: Option<Vec<Uid>>,
    /// Relative weight per miner: a uid→weight map, or a list parallel to `uids`.
    #[serde(default)] pub weights: Option<WeightsArg>,   // #[serde(untagged)] enum { Map(BTreeMap<Uid, f64>), List(Vec<f64>) }
    /// Mechanism index within the subnet.
    #[serde(default)] pub mechid: MechId,
    /// Weights version key checked against the subnet's required version.
    #[serde(default)] pub version_key: u64,
    /// Preserve exact u16 ratios on Null subnets.
    #[serde(default)] pub raw_u16: bool,
}
```

`#[serde(flatten)]` is deliberately avoided: serde does not support it together with `deny_unknown_fields`, and it would
make the JSON schema's property names differ from upstream's. `build` converts `(uids, weights)` into the internal
`WeightsInput` (§9.2).

The derive generates:

- `impl IntentMeta`: name, summary, description, role, origin, verify, wraps, the `schemars` input schema with
  `x-rtao-*` CLI hints (address kind, money unit, `"all"` support), and docs URL `/docs/tx/<op-kebab>`
  (`PY/settings.py:129-133`);
- the `Intent` associated items;
- a `const _: () = assert!(...)` for the root/verify rule.

**Why intents are hand-ported structs rather than generated from `intents.json`.** The catalog carries each intent's
*interface*: `input_schema`, `wraps`, `signer`, `origin`, `verify`. It does not carry the *behaviour*. Python implements
that behaviour in 8,335 lines of `intents/*.py` (R/A1 §1): preflight reads, slippage limits, `"all"` resolution, weights
conformance, root-claim fee models and receipt post-processing. So generating the full intent is impossible, and
hand-writing the interface is wasteful. `cargo xtask parity scaffold <op>` therefore generates the *interface* from the
catalog entry: the struct with fields, doc comments and serde attributes from `input_schema`, the `#[intent(...)]`
attribute from `signer`/`origin`/`verify`/`wraps`, and a `todo!()` `build`. Its header links the upstream source lines from
the entry's `sources`. A developer ports only the behaviour, and the parity gate (§4.7) keeps the interface honest from
then on.

The static registry is an explicit, sorted list, chosen over link-time collection (`inventory`/`linkme`) for
determinism:

```rust
// rtao-intents/src/lib.rs
rtao_macros::registry! { pub static INTENTS: [IntentEntry] = [
    AddCollateral, AddProxy, AddStake, AddStakeLimit, AnnounceColdkeySwap, AssociateEvmKey, AssociateHotkey, Batch,
    BurnedRegister, ClaimRoot, ClaimRootWithHotkey, /* … all 83, sorted by OP; a test asserts sorted + unique … */
]; }
pub struct IntentEntry {
    pub meta: &'static IntentMetaStatic,
    pub from_json: fn(serde_json::Value) -> Result<Box<dyn DynIntent>, serde_json::Error>,
}
pub trait DynIntent: Send + Sync {          // object-safe projection for agents/CLI; blanket-implemented for I: Intent
    fn op(&self) -> &'static str;
    fn to_json(&self) -> serde_json::Value; // Balances as exact decimal strings (PY/intents/base.py:357-399)
    fn plan<'a>(&'a self, ex: &'a Executor, keys: &'a dyn DynKeySource, opts: PlanOptions)
        -> BoxFuture<'a, Result<Plan, ExecError>>;
    fn execute<'a>(&'a self, ex: &'a Executor, keys: &'a dyn DynKeySource, opts: ExecOptions)
        -> BoxFuture<'a, Result<ExtrinsicResult<serde_json::Value>, ExecError>>;
}
```

The `dyn` path uses `DynKeySource::signer(role: RoleKind) -> BoxFuture<Result<Box<dyn DynSigner>>>`. It necessarily
re-introduces a run-time role check (`SignError::RoleUnavailable { role }`), which is exactly Python's behaviour. Typed
callers keep the compile-time check, and agents and the CLI get Python's semantics.

`client.execute_tool("transfer", json!({..}), &wallet)` dispatches through `INTENTS`. This is the agent path and
mirrors `PY/client.py:483-488`. `client.tools()` returns the catalog in exactly the shape of Python's
`list_tools()` (`PY/intents/registry.py:43-66`): `{name, summary, description, signer, origin, verify, wraps, docs_url,
input_schema}`.

### 4.3 Representative implementations

**`SetWeights::build`** (the validator hot path):

1. Pin a snapshot.
2. Read `WeightsPreflight` at that block: `Uids`, `CommitRevealWeightsEnabled`, `WeightsSetRateLimit`,
   `LastUpdate[NetUidStorageIndex::new(netuid, mechid)]`, `MinAllowedWeights`, `MaxWeightsLimit`,
   `SubnetEpochConsensus`, plus `ValidatorPermit` under Null (`PY/intents/weights.py:154-215`).
3. Fail with typed errors that carry the same `ErrorCode`s: `NotRegistered`, `RateLimited { blocks_remaining }`,
   `NotAuthorized`.
4. Conform `WeightsInput` into `NormalizedWeights` (§9.2).
5. If commit-reveal is enabled, build the CRv3 commit via `rtao_timelock::crv3::commit` (§9.6) and compose
   `CommitTimelockedMechanismWeights { netuid, mecid, commit, reveal_round, commit_reveal_version }`. The version is read
   from `CommitRevealWeightsVersion`, not hard-coded (E §4.1 item 4). Otherwise compose `SetMechanismWeights`.

**`AddStake::build`:**

- `amount: Amount<Tao>`; `All` means `System.Account.free − ExistentialDeposit − 500_000 rao`
  (`PY/intents/staking.py:248-267`).
- With slippage protection (default on, 5% tolerance), read `SwapRuntimeApi.current_alpha_price[netuid]` and compose
  `add_stake_limit { limit_price = price·(1+tol), allow_partial = false }`. Otherwise compose `add_stake`
  (`staking.py:596-606`).
- `spend()` is `Bounded(amount)`, or `Unbounded` for `All`.
- The preflight quote comes from `sim_swap_tao_for_alpha` and is best-effort.

**`RegisterSubnet::finish`:** waits for `NetworkAdded` whose `SubnetOwner`/`SubnetOwnerHotkey` match, across the queued
path (§9.4). `SetWeights` carries `Pending = Option<Round>` from `build` into `Output = WeightsSubmitted`.
`CreatePureProxy::finish` reads `Proxy.PureCreated` from the receipt's events.

### 4.4 Executor, Plan, Policy, ExtrinsicResult: mapping from Python

```rust
pub struct Executor { client: Client, policy: Option<Policy>, nonces: Arc<NonceManager> }
impl Executor {
    pub async fn plan<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: PlanOptions) -> Result<Plan, ExecError>;
    pub async fn execute<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: ExecOptions)
        -> Result<ExtrinsicResult<I::Output>, ExecError>;
    pub async fn execute_shielded<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: ShieldOptions)
        -> Result<ExtrinsicResult<I::Output>, ExecError>;
    pub async fn submit_call<K: KeySource<R>, R: Role>(&self, call: CallBytes, keys: &K, o: ExecOptions)
        -> Result<ExtrinsicResult<()>, ExecError>;                    // raw: requires policy.allow_raw_calls
    pub async fn prepare_call(&self, call: CallBytes, address: &AccountId, o: PrepareOptions) -> Result<PreparedExtrinsic>;
    pub async fn submit_signature(&self, p: PreparedExtrinsic, sig: Signature, w: Wait) -> Result<ExtrinsicResult<()>>;
}
pub struct ExecOptions { pub policy: Option<Policy>, pub proxy_for: Option<AccountId>, pub force_proxy_type: Option<ProxyType>,
    pub mortality: Mortality /* Mortal(P128) */, pub wait: Wait /* Finalization */, pub retries: u8 /* 0 */,
    pub reconcile: bool /* false */, pub wait_for_registration: bool /* true */, pub registration_timeout: Option<Duration> }

pub struct Policy { pub max_fee: Option<Balance<Tao>>, pub max_spend: Option<Balance<Tao>>,
                    pub allowed_netuids: Option<BTreeSet<NetUid>>, pub allow_raw_calls: bool }
pub trait PolicySubject { fn op(&self) -> &str; fn spend(&self) -> Spend; fn netuids(&self) -> NetuidScope; fn is_raw(&self) -> bool; }
impl Policy { pub fn check(&self, s: &dyn PolicySubject, fee: Option<Balance<Tao>>) -> Vec<Violation>; }

pub struct Plan { pub op: &'static str, pub summary: String, pub role: RoleKind, pub signer_address: AccountId,
    pub fee: Option<Balance<Tao>>, pub effects: Vec<String>, pub warnings: Vec<String>, pub violations: Vec<Violation>,
    pub spend: Spend, pub args: serde_json::Value, call: CallBytes /* private: execute submits exactly this */ }

pub struct ExtrinsicResult<O> { pub success: bool, pub message: String, pub block_hash: Option<H256>,
    pub extrinsic_id: Option<ExtrinsicId /* "{block}-{idx:04}" */>, pub explorer_url: Option<Url>,
    pub fee: Option<Balance<Tao>>, pub events: Events, pub error: Option<ChainError>, pub data: O,
    pub extras: serde_json::Map<String, Value> }
impl<O> ExtrinsicResult<O> { pub fn raise_for_failure(self) -> Result<Self, ChainError>; pub fn to_json(&self) -> Value; }
```

| Python (`PY/…`) | rtao | Notes |
|---|---|---|
| `Intent` class vars `op, signer, origin, verify, wraps, all_amount_fields, mev_shield_*` (`intents/base.py:138-177`) | associated consts and types | compile-time role/origin |
| `Intent.build` → `CallBytes \| BuiltCall` (`base.py:177-186,414-422`) | `build` → `Built<Pending>`, then `finish` → `Output` | extras typed; receipt-derived data (netuid, pure proxy) is typed too |
| `preflight(dispatch_origin, fee_payer, call)` (`base.py:204-226`) | `preflight(&PreflightCx { dispatch_origin, fee_payer, call })` | same split (§3.10) |
| `spend()`, `touches_netuids()`, `affects_all_subnets()` (`base.py:300-322`) | `spend()`, `netuids() -> NetuidScope::{Listed, All}` | one enum replaces two methods |
| `Policy(max_fee_tao, max_spend_tao, allowed_netuids, allow_raw_calls)` + fail-closed fee (`plan.py:21-85`) | `Policy` over `Balance<Tao>` | identical rules |
| `Plan` + `to_dict` (`plan.py:89-135`) | `Plan` + `to_json` (same keys: `fee_tao`, `spend_tao`, `spend_unbounded`, `ok`, `docs_url`) | |
| `Executor.execute` loop (`executor.py:774-889`) | `Executor::execute` | typed retries (§3.8) |
| `ExtrinsicResult` (`result.py:697-750`) | `ExtrinsicResult<O>` | `success = true` with `Wait::Pool` keeps Python's meaning ("accepted by pool", R/A1 §10 item 1); the type also exposes `status: SubmitStatus::{Pooled, Included, Finalized}` |
| `ChainError(message, name, code)` + `remediation/description/docs_url` (`result.py:501-562`) | `ChainError { name, pallet, code, message, remediation, description, docs_url }` | §4.6 |
| `PolicyError(violations)` (`result.py:687-693`) | `ExecError::Policy(Vec<Violation>)`, `code() = PolicyViolation` | |
| `RpcConnectionError` / `RpcPolicyError(status_code, retry_after, policy, reason)` (`result.py:45-73`) | `ExecError::Transport(RpcError::Policy(PolicyRefusal))` | |

### 4.5 Reads

```rust
pub trait Read: Send + Sync + 'static {
    const NAME: &'static str;                       // "epoch_status"; CLI `rtao query epoch-status`
    const CATEGORY: Category;                       // 14 categories = 14 namespaces (PY/namespaces.py:279-294)
    const PINNING: Pinning;                         // Single | Required | Gather
    const TARGETS: &'static [ChainTarget];          // exact chain "hits": storage/runtime-api/constant descriptors
    const RENDER: Option<RenderHint> = None;        // Grouped{key} | Matrix{row,col,value} (PY/reads/base.py:33-54)
    type Params: Serialize + DeserializeOwned + JsonSchema + Send;
    type Output: Serialize + Send;
    fn fetch<V: View>(view: &V, p: Self::Params) -> impl Future<Output = Result<Self::Output, ReadError>> + Send;
}

/// Epoch timing for a subnet at one block.
#[rtao::read(name = "epoch_status", category = EpochsTiming, pinning = Required,
             targets(storage(SubtensorModule::Tempo), storage(SubtensorModule::LastEpochBlock),
                     storage(SubtensorModule::BlocksSinceLastStep), storage(SubtensorModule::PendingEpochAt),
                     storage(SubtensorModule::SubnetEpochIndex), runtime_api(SubnetInfoRuntimeApi::get_next_epoch_start_block),
                     constant(Aura::SlotDuration)))]
pub async fn epoch_status<V: View>(view: &V, /** Subnet to query. */ netuid: NetUid) -> Result<EpochStatus, ReadError> { … }
```

- The macro generates a `Read` impl, a `ReadEntry` in `READS` (sorted static, like `INTENTS`), and a namespace method:
  `client.epochs().epoch_status(netuid)` and `snapshot.epochs().epoch_status(netuid)`. The namespace structs `Balances`,
  `Chain`, `Collateral`, `Delegation`, `Epochs`, `Hyperparameters`, `Identity`, `Leasing`, `Locks`, `Neurons`, `Prices`,
  `Staking`, `Subnets`, `Weights` mirror `PY/namespaces.py:279-294`.
- **Declared targets are verified, not trusted.** In tests every read runs on a `RecordingView` over `FakeChain` (§11.3),
  which logs each descriptor touched; the test asserts the log equals `TARGETS`. Python scrapes read *source* with
  regexes to compute `hits` (R/A2 §5.6; `website/.../generate.py:405-487`). rtao's `hits` are exact by construction and
  feed the catalog's `hits` field directly.
- **Outputs** are typed structs whose `Serialize` output matches Python's JSON for the same read. Examples: `Balance`
  serializes as an exact decimal string; `epoch_status` returns `EpochStatus { netuid, block, tempo, epoch_index, … }`
  (`PY/reads/epochs.py:84-128`). §11 verifies this on localnet against Python.
- **Address params** accept `impl IntoAccountId`, a small trait implemented for `AccountId`, ss58 `&str` (fallible),
  `&Wallet` and keypairs. A `hotkey_*` parameter picks the hotkey and a `coldkey_*` parameter the coldkey public view,
  mirroring `PY/reads/base.py:108-128`. On the JSON/CLI path, ss58 strings and wallet names are resolved by the CLI context.
- **Python divergences adopted deliberately** (each listed in `parity/divergences.toml`):
  - `hotkey_owner`, `miner_collateral` and similar return `None` for the all-zero default account (R/A2 §9 item 9);
  - the four Python "gather" reads that straddle blocks (`delegate_take`, `coldkey_swap_announcement`,
    `hotkey_identities`, `miner_collateral`; R/A2 §1.3) are `Pinning::Required` in rtao.

### 4.6 Error taxonomy (`rtao-errors`)

- `ErrorCode` is the 18-variant enum, serialized snake_case exactly as `PY/error_map.py:23-41`.
- `CHAIN_ERRORS` is a generated `phf::Map<&'static str, ChainErrorInfo { code, description, docs_url, pallets }>`. It is
  built from two sources:
  - error *existence* and pallet/index come from metadata (`rtao-chain::errors`);
  - *classification* and prose come from the vendored `parity/upstream/errors.json` (390 `chain_errors`, 18 `codes`;
    `CAT/errors.json`).

  The generator fails when a chain error has no classification. That is the `--names` gate (`check.py:354-420`) ported as
  a codegen error, not a separate script.
- `CUSTOM_TX_ERRORS: [(u8, &str); 29]` covers codes 0–27 and 255 (`PY/error_map.py:488-518`). A test parses a vendored
  copy of `UP/common/src/transaction_error.rs` and asserts the code set is equal.
- `classify(name, message) -> ErrorCode` uses the precedence of `PY/result.py:483-498`: name table, then dispatch errors,
  then `*RateLimit*`, then a message-substring fallback. The substring fallback applies only to *unnamed* text errors,
  and typed pool rejections (§3.8) never reach it.
- `explain(code_or_name)` backs `rtao explain`. `ChainError::docs_url()` follows `/docs/errors/chain/<Name>` or
  `/docs/errors/<code-with-dashes>` (`result.py:501-562`).

### 4.7 Parity matrix and CI enforcement

`parity/` holds:

- `upstream/{intents,reads,errors,index}.json`, vendored from `https://www.bittensor.com/catalog/*.json` and
  `/code/index.json`, with `index.json.commit` recorded (currently `850f2755…`);
- `raw_only.toml`, the port of Python's `RAW_ONLY` per pallet (`check.py:58-300`) plus rtao-only additions;
- `mev.toml`, MEV flags parsed from upstream Python sources by `xtask`, because the catalog lacks them;
- `divergences.toml`, deliberate differences, each with an id, an upstream reference, a reason, and `kind = kept | improved | unsupported`;
- the generated `PARITY.md`.

`cargo xtask parity --check` runs in PR CI and is also exposed as `#[test]`s in `rtao-intents`/`rtao-reads`:

1. **Intent set.** `INTENTS` names equal `upstream/intents.json` names. Per intent: `signer`, `origin`, `verify` and the
   `wraps` *set* are equal. The `input_schema` property names, `required` set and JSON types are equal after normalizing
   ($ref inlining, description stripping).
2. **Read set.** Names, `params` (name to JSON type) and `category` are equal. `hits` equal `TARGETS`, modulo upstream's
   ten no-hit reads (R/A2 §3.12, §5.6).
3. **Errors.** Every upstream `chain_errors` name exists in `CHAIN_ERRORS` with the same `code`. The `codes` key set is
   equal.
4. **Chain coverage** (offline, from the pinned metadata). Every call of every covered pallet is either wrapped by some
   `I::WRAPS` or listed in `raw_only.toml`. Stale or double listings fail. Root intents declare a `VERIFY` read that
   exists in `READS` (`check.py:303-346`).
5. **MEV flags** equal `mev.toml`.
6. **Behavioural parity.** Recorded Python outputs exist for every read (`fixtures/parity/reads/<name>.json`, recorded on
   localnet by the interop job, §11.5).

`PARITY.md` (generated) has one row per intent and per read with five columns:
`implemented | unit-tested | localnet-e2e | python-interop | divergence-id`. A second table covers the non-catalog
surfaces:

- transport semantics;
- runtime cache;
- wallets and passwords;
- http_auth;
- timelock UX;
- metagraph;
- signers (wallet, ledger, vault, extension);
- EVM;
- hyperparameters;
- PoW;
- the CLI groups.

**Upstream additions are detected** by the nightly `catalog-drift` job (§12.3). It fetches the live catalogs, rewrites
`parity/upstream/*`, and opens a PR. Check 1, 2 or 3 then fails, naming exactly what to implement or allowlist.

---

## 5. Money and units (`rtao-units`)

### 5.1 Wire units versus domain units

The chain declares units as type paths: `TaoBalance(u64)`, `AlphaBalance(u64)`, `NetUid(u16)`, `MechId(u8)`,
`NetUidStorageIndex(u16)` and `PerU16(u16)`. Each SCALE-encodes as the bare integer (C1 §1; `UP/common/src/lib.rs:383-402`).
The netuid an alpha amount belongs to is **context**, not part of the wire value. rtao models the two layers separately:

```rust
/// Wire-level, context-free (what generated calls/storage use). Transparent SCALE.
#[derive(Encode, Decode, CompactAs, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct TaoRao(pub u64);       // CompactAs: Compact<TaoRao> encodes like Compact<u64> (Metagraph columns)
#[derive(/* same */)] pub struct AlphaRao(pub u64);
#[derive(/* same */)] pub struct StakeWeightRao(pub u64);   // Metagraph.total_stake (alpha + tao·tao_weight)

/// Domain-level, context-carrying (what intents, reads and users see).
pub trait Unit: sealed::Sealed + Copy + Send + Sync + 'static { type Ctx: Copy + Eq + Hash + fmt::Debug + Send + Sync; }
#[derive(Clone, Copy)] pub enum Tao {}    impl Unit for Tao   { type Ctx = (); }
#[derive(Clone, Copy)] pub enum Alpha {}  impl Unit for Alpha { type Ctx = NetUid; }

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Balance<U: Unit> { rao: u64, ctx: U::Ctx }      // Balance<Tao>: 8 bytes; Balance<Alpha>: u64 + NetUid

impl Balance<Tao> {
    pub const ZERO: Self;
    pub const fn from_rao(rao: u64) -> Self;
    pub fn from_tao_str(s: &str) -> Result<Self, ParseError>;       // exact: integer + up to 9 fractional digits
    pub fn from_tao_f64_lossy(t: f64) -> Result<Self, ParseError>;  // explicit name, rejects NaN/negative/overflow
    pub fn to_decimal_string(&self) -> String;                      // exact, e.g. "1.500000000"
}
impl Balance<Alpha> {
    pub const fn from_rao(rao: u64, netuid: NetUid) -> Self;
    pub fn netuid(&self) -> NetUid;
    pub fn checked_add(self, o: Self) -> Result<Self, UnitError>;   // UnitMismatch{left,right} | Overflow
    pub fn to_wire_for(self, call_netuid: NetUid) -> Result<AlphaRao, UnitError>; // the Python call_amount check
}
impl Balance<Tao> { pub fn checked_add(self, o: Self) -> Option<Self>; pub fn checked_sub(self, o: Self) -> Option<Self>;
                    pub fn saturating_sub(self, o: Self) -> Self; }   // no panicking `Add`/`Sub` impls on money
impl TaoRao  { pub fn tao(self) -> Balance<Tao>; }
impl AlphaRao { pub fn in_subnet(self, netuid: NetUid) -> Balance<Alpha>; }
```

Properties:

- **TAO and alpha cannot be mixed: it is a compile error.** `Balance<Tao> + Balance<Alpha>` has no impl. Python raises
  `UnitMismatchError` at run time (`PY/balance.py:1-25,92-116`).
- **Alphas of different subnets are a checked error.** `checked_add` returns `UnitMismatch`, the same semantics as
  Python, but surfaced as a `Result`, never a panic. netuid 0 alpha is TAO 1:1 on the root subnet. That conversion is the
  only one provided: `Balance<Alpha>::root_as_tao()`, which errors unless `netuid == 0`.
- **Generated call parameters are typed by the runtime's own unit identity.** `add_stake.amount_staked: TaoRao`,
  `remove_stake.amount_unstaked: AlphaRao`. Passing alpha where TAO is expected fails to compile. Python discovers the
  unit from generated annotations and a fallback table at run time (`PY/intents/_money.py:89-128`).
- **`Amount<U> = Exact(Balance<U>) | All`** is the intent-field type. `All` is accepted only on fields the derive lists in
  `ALL_AMOUNT_FIELDS`. Its serde form is a decimal string, a number, or `"all"`, matching Python's money JSON schema
  (`money_schema`, `PY/intents/base.py:63`).
- **Exact serialization.** `Serialize` emits exact decimal strings (`PY/intents/base.py:357-399`), and `Deserialize`
  accepts strings and integers. Floats are accepted only through the explicitly lossy constructors, a deliberate
  tightening of `Money = int | float | str | Decimal | Balance` (`_money.py:29-31`).
- **Display.** `Balance<Tao>` prints `τ1.5`. `Balance<Alpha>` prints with a `SymbolTable` from `TokenSymbol` storage
  (`balance.display(&symbols)` gives `1.5β`, falling back to `α₄₂`; `PY/balance.py:127-137`). The symbol is a display
  context, not a field, which keeps `Balance` `Copy` and small (16 bytes for alpha with alignment).
- **Large and other quantities** get their own types: `SubnetVolume(u128)`, `PriceRao(u64)` (rao per alpha, from
  `current_alpha_price`, R/A2 §2), `LimitPrice(PriceRao)`, `Spend::{None, Bounded(Balance<Tao>), Unbounded}`.

### 5.2 Identifiers

```rust
#[derive(Encode, Decode, CompactAs, …)] pub struct NetUid(pub u16);  impl NetUid { pub const ROOT: Self = Self(0); }
#[derive(Encode, Decode, CompactAs, …)] pub struct MechId(pub u8);
#[derive(Encode, Decode, …)] pub struct Uid(pub u16);
#[derive(Encode, Decode, …)] pub struct NetUidStorageIndex(pub u16);
impl NetUidStorageIndex {
    pub const STRIDE: u16 = 4096;                                             // GLOBAL_MAX_SUBNET_COUNT (PY/settings.py:31)
    pub fn new(netuid: NetUid, mechid: MechId) -> Result<Self, IdError>;      // mechid*4096 + netuid, checked
    pub fn split(self) -> (NetUid, MechId);                                   // the metagraph `netuid` field of a mechagraph
}
pub struct AccountId([u8; 32]);  // Display = ss58 (prefix 42) via rtao_core::keys::ss58_from_public; FromStr checks prefix 42
```

`NetUidStorageIndex` is the key of `Weights`, `Bonds`, `LastUpdate`, `Incentive`, `WeightCommits`,
`TimelockedWeightCommits`, `CRV3WeightCommits*` and `ConsensusByMechanism` (C2 §0 item 5). Generated storage entries take
it as their key type, so passing a bare `NetUid` to `LAST_UPDATE` is a compile error. Python's preflight builds the
index by hand (`PY/intents/weights.py:164-167`), a known foot-gun (R/A1 §10 item 8).

### 5.3 Ratios and fixed point

| Type | Wire | Value | Used by |
|---|---|---|---|
| `PerU16(u16)` | u16 | x/65535 | take, incentive, dividends, consensus, trust, kappa, alpha_low/high |
| `Permill(u32)`, `Perbill(u32)`, `Percent(u8)` | int | x/1e6, x/1e9, x/100 | `RATIO_TYPE_DENOMINATORS` (`PY/hyperparams.py:27-52`) |
| `PerMillion(u64)` | u64 | x/1e6 | `bonds_moving_avg` |
| `U64Normalized(u64)` / `Proportion` | u64 | x/u64::MAX | child proportions (`PY/reads/delegation.py:156-166`), `tao_weight` |
| `U64F64(u128)` | `{bits: u128}` | bits/2^64 | `burn_increase_mult`, `collateral_drain_ratio`, basket pricing, conviction |
| `I96F32(i128)` | `{bits: i128}` | bits/2^32 | `moving_price`, `RootClaimableThreshold`, `BasketRate` |
| `I64F64(i128)` | `{bits: i128}` | bits/2^64 | `SubnetEmaTaoFlow` |
| `I32F32(i64)` | `{bits: i64}` | bits/2^32; `int_part()` = bits >> 32 | `alpha_sigmoid_steepness` (read flattens to the integer part, `PY/reads/subnets.py:126-147`) |

- All of them are `#[repr(transparent)]`, with `Encode`/`Decode` identical to the one-field composite. They provide
  `to_f64()`, `from_bits`/`to_bits`, an exact decimal `Display` (bits × 10^k / 2^frac in u256), and checked
  `from_f64(..)`.
- No consensus-relevant math uses `f64`. Weights are u16-based (§9.2). Client-side projections that Python does in
  floats, such as the `subnet_convictions` decay (`PY/reads/locks.py:188-286`), are ported in fixed point to match the
  pallet. That is a deliberate improvement, listed in `divergences.toml`.
- The fixed-point mapping is generated from the typenum `Frac` parameter (§3.5), so a new runtime fixed type maps without
  hand edits. An unknown `Frac` is a codegen error.

### 5.4 Durations and hyperparameter kinds

- `Blocks<T = u64>` (the wire width varies: tempo, immunity and burn half-life are u16; rate limits are u64),
  `Epochs(u64)` and `BlockNumber(u64)`. `Blocks::to_duration(block_time)` uses the detected block time, never the
  hard-coded 12 s.
- Hyperparameter kinds form a closed enum: `U16Fraction`, `U64Fraction`, `PerMillion`, `Rao`, `Blocks`, `Epochs`,
  `Difficulty` (u64::MAX = disabled), `U64F64`, `Int{signed,bits}`, `Bool` (`PY/hyperparams.py:55-80`). The typed value
  per setter is in §9.5.
- The CLI input rule is the same as Python's: a plain integer is the raw on-chain value, and a value with a decimal point
  is the human form (`PY/hyperparams.py:10-13,581-770`). It is implemented once in `HyperparamValue::parse_cli`.

---

## 6. Wallets and signing

### 6.1 Wallet layout and keyfiles (`rtao-wallet`, requires `rtao-core/host`)

- **Layout** is exactly btcli's: `~/.bittensor/wallets/<name>/coldkey` (encrypted), `coldkeypub.txt`,
  `hotkeys/<hotkey>` (plaintext), `hotkeys/<hotkey>pub.txt`. Defaults are `default`/`default`, and the compound form is
  `"cold/hot"` (`PY/wallet.py:11,29-70`; G §4.3).
- **Formats** come entirely from bittensor-core. Writing uses `$NACL` (Argon2i13 SENSITIVE with fixed salt, then
  secretbox, `CORE/keyfiles/mod.rs:22,59-95`). Reading accepts `$NACL`, `$ANSIBLE_VAULT` and legacy Fernet
  (PBKDF2-SHA256, 10M iterations, `:139-170`). The JSON quirks are handled: `" ss58Address"` with a leading space, a
  numeric or string `cryptoType`, the derivation order `secretPhrase` → `secretSeed` → `privateKey` → `ss58Address`, and a
  retry with the other crypto type on address mismatch (`:213-364`). Polkadot-js JSON imports through
  `Keypair::from_encrypted_json`. The point of the veneer is that keyfiles are byte-compatible because they are the same
  code.
- **File hygiene** follows Python: files 0o600 and directories 0o700, written via temp file, fsync and rename. rtao never
  overwrites without an explicit `Overwrite::Yes`, and never prompts when stdin is not a TTY (`PY/keyfiles.py:128-163`;
  `PY/wallets.py:503-513`). Hotkeys are created without a password (`PY/wallets.py:239-245`).
- **Decryption runs on `offload`.** One unlock per process, as in `WalletSigner` (`PY/signing.py:143-224`). The keypair
  is held behind `Arc<Zeroizing<…>>`, and secrets never reach `Debug` or tracing (`secrecy::SecretString` for
  passwords).

```rust
pub struct Wallet { name: String, hotkey: String, path: PathBuf, cold: OnceCell<Arc<ColdkeyUnlocked>>,
                    passwords: PasswordResolver }
impl Wallet {
    pub fn open(spec: &WalletSpec) -> Result<Self, WalletError>;           // "cold" | "cold/hot" | struct
    pub fn coldkeypub(&self) -> Result<AccountId, WalletError>;            // never unlocks
    pub fn hotkey_file(&self) -> Result<HotkeyFile, WalletError>;          // plaintext, no password
    pub async fn unlock_coldkey(&self) -> Result<Arc<ColdkeyUnlocked>, WalletError>; // resolver chain, offloaded KDF
    pub async fn create(name: &str, opts: CreateOptions) -> Result<(Self, Mnemonic), WalletError>; // feature "create"
    pub async fn regen_coldkey(&self, from: KeySourceSpec, password: SecretString) -> Result<(), WalletError>;
}
```

### 6.2 Password resolution order

`PasswordResolver` is an explicit chain. Each source is a variant, so callers can drop, add or reorder sources, and tests
can inject them. The default order is Python's (`PY/wallets.py:62-118`):

```rust
pub enum PasswordSource {
    Explicit(SecretString),            // 1. SDK caller / tests
    EnvVar("BT_WALLET_PASSWORD"),      // 2.
    PerKeyfileEnv,                     // 3. BT_PW_<PATH with separators and dots → '_', uppercased> (PY/keyfiles.py:102-104),
                                       //    value = base64(password XOR var-name): rtao_core::keyfiles::get_password_from_environment
    File(PathBuf),                     // 4. --wallet-password-file / BT_WALLET_PASSWORD_FILE, first line, trimmed
    MacosKeychain,                     // 5. feature "keychain": generic password service "bittensor.coldkey", account = wallet
                                       //    name, the same item `btcli wallet keychain save` writes (PY/macos_password.py:8,62-82),
                                       //    via security-framework (no shell-out)
    MacosDialog,                       // 6. feature "keychain": osascript dialog (opt-in, like --macos-password)
    Prompt,                            // 7. feature "prompt": rpassword, TTY only; never in --json/non-TTY mode
}
```

- The resolver refuses a wrong password **only** for interactive prompts, with two re-prompts (`PY/cli/context.py:1900-1935`).
  Every other source fails fast with `WrongPassword { source }`.
- rtao never writes `BT_PW_*` into its own environment. `save_password_to_environment` uses `set_var`, which is unsound
  under threads (§1.6 item 8).
- An optional `SecretService` source (Linux `keyring`) is an rtao extension, off by default and listed in
  `divergences.toml` as `improved`.

### 6.3 The `Signer` trait

```rust
// rtao-signer
pub enum CryptoType { Ed25519 = 0, Sr25519 = 1 }            // btwallet codes (CORE/keys/mod.rs:28-29); ecdsa unsupported
bitflags! { pub struct Capabilities: u8 {
    const RAW_PAYLOAD     = 1;   // signs the exact payload bytes (keypairs, remote)
    const PAYLOAD_JSON    = 2;   // wants Polkadot-JS SignerPayloadJSON (browser extensions)
    const METADATA_DIGEST = 4;   // requires CheckMetadataHash Enabled + RFC-0078 proof (Ledger generic app)
    const OUT_OF_BAND     = 8;   // human round trip (Vault QR); executor applies two-stage/warm-up rules
} }

pub struct SigningContext<'a> {
    pub payload: &'a [u8],                         // exact bytes (blake2-hashed if > 256 B)
    pub parts: (&'a [u8], &'a [u8], &'a [u8]),     // call, included_in_extrinsic, included_in_signed_data
    pub payload_json: &'a SignerPayloadJson,       // lazily built; numerics are BE hex at SCALE width (extrinsics.py:87-132)
    pub metadata: Option<&'a MetadataProofCtx>,    // v15 bytes + ChainInfo for digest/proof when METADATA_DIGEST
    pub purpose: SignPurpose,                      // Extrinsic | ShieldInner | ShieldCarrier | BtAuth | Message
}

pub trait Signer: Send + Sync {
    fn account(&self) -> &AccountId;
    fn crypto(&self) -> CryptoType;
    fn capabilities(&self) -> Capabilities;
    fn sign<'a>(&'a self, cx: &'a SigningContext<'a>) -> impl Future<Output = Result<Signature, SignError>> + Send + 'a;
}
pub enum Signature { Raw64([u8; 64]), Multi65([u8; 65]) }   // 65 = MultiSignature with a version prefix (hardware)

pub trait KeySource<R: Role>: Send + Sync {
    type Signer<'a>: Signer + 'a where Self: 'a;
    fn address(&self) -> Result<AccountId, SignError>;                                 // public view, never unlocks
    fn signer(&self) -> impl Future<Output = Result<Self::Signer<'_>, SignError>> + Send; // may unlock lazily
}
```

`KeySource` implementations, which define what can sign what at compile time:

| Type | `KeySource<Coldkey>` | `KeySource<Hotkey>` | Notes |
|---|---|---|---|
| `Wallet` | yes (unlocks via resolver) | yes (plaintext hotkey) | the CLI and owner tooling default |
| `HotkeyFile` | **no** | yes | validators and miners: coldkey intents do not compile |
| `ColdkeyOnly` (`Wallet::coldkey_only()`) | yes | no | cold-wallet machines |
| `KeypairSigner` (from URI, mnemonic, seed) | yes | yes | dev keys `//Alice`, tests; a bare `Signer` signs either role, as in Python (`PY/intents/base.py:258-298`) |
| `LedgerSigner` (feature `ledger`) | yes | yes (a bare signer signs either role, as in Python; normally coldkey) | `METADATA_DIGEST`: the executor sets `TxParams.metadata_hash = metadata_digest(v15, chain_info)` and passes `generate_extrinsic_proof(..)`; device I/O offloaded; ed25519 path `m/44'/354'/account'/0'/index'` (R/B Appendix A) |
| `VaultSigner` (feature `vault`) | yes | yes (bare signer) | QR/UOS through a local page; `OUT_OF_BAND`; mortal era ~25 min; shielded needs two scans inside 8 blocks, so the executor prepares both payloads before the first scan (`PY/vault/*`; G §4.4) |
| `ExtensionSigner` (feature `extension`) | yes | yes (bare signer) | compatible with Python's bridge protocol (`127.0.0.1:39295`, token file `~/.bittensor/extension_bridge.token`; G §4.4); `PAYLOAD_JSON`; CLI-oriented |
| `RemoteSigner` (feature `remote`) | yes | yes | rtao extension: HTTP/mTLS signing service (HSM/KMS) receiving `SigningContext` minus secrets |
| `MultisigSigner<K>` | via adapters (§3.10) | — | |

- **Object safety.** `Signer` and `KeySource` use return-position `impl Future`, which keeps typed calls allocation-free
  but is not object-safe. `DynSigner` and `DynKeySource` (boxed futures) are blanket-implemented over them for the
  registry, CLI and agent paths (§4.2).
- **`Signer::sign` is async.** Hardware and remote signers block, and in-memory signers return immediately. A signer
  error never becomes a dummy signature: SN2's zero-signature fallback is the anti-pattern (E §2.6).
- **Role per intent** is `I::Role`: 75 coldkey, 8 hotkey (§4). For *fee-bearing* hotkey calls the runtime charges the
  owning coldkey (478 wrapper; `UP/runtime/src/fee_filters.rs:16-34`). Plan emits Python's `COLDKEY_FEE_WARNING` when any
  leaf of the composed call is in the vendored `COLDKEY_PAYS_FEE_CALLS` table (`PY/fee_filters.py:12-98`). The weights,
  serving and associate calls are `Pays::No` (C1 §0 item 4).

---

## 7. Neuron networking

### 7.1 btauth/1 core (`rtao-auth`, no I/O, no async runtime)

The normative sources are `UP/docs/guides/signed-requests.mdx:35-101,252-289` and `PY/http_auth.py:58-343`, summarized
in R/D §1.

**Headers.** `X-Bittensor-Version: 1`, `X-Bittensor-Hotkey: <ss58>`, `X-Bittensor-Receiver: <ss58>` (optional),
`X-Bittensor-Nonce: <unix ns, decimal>`, `X-Bittensor-Crypto: ed25519` (absent means sr25519) and
`X-Bittensor-Signature: 0x<lowercase hex of 64 bytes>`. Names are case-insensitive. The reserved
`X-Bittensor-Drand-Round` and `X-Bittensor-Drand-Randomness` headers are neither emitted nor interpreted.

**Exact payload bytes.** Eight UTF-8 lines joined by `\n`, with no trailing newline:

```
btauth/1
<sr25519|ed25519>                         ← re-derived from the parsed crypto type, never echoed from the header
<METHOD, uppercase>
<request target: path + "?" + query exactly as on the wire, percent-encoded; http::uri::PathAndQuery verbatim>
<sha256(raw body bytes), lowercase hex>    ← e3b0c442…b855 for an empty body
<nonce ns, decimal>
<sender ss58>
<receiver ss58, or "-" if unbound>
```

```rust
pub const PROTOCOL: &str = "btauth/1";
pub fn build_payload(scheme: CryptoType, method: &http::Method, target: &http::uri::PathAndQuery, body: &[u8],
                     nonce_ns: u64, sender: &AccountId, receiver: Option<&AccountId>) -> Vec<u8>;

pub async fn sign<S: Signer + ?Sized>(signer: &S, method: &http::Method, target: &http::uri::PathAndQuery,
    body: &[u8], receiver: Option<&AccountId>, clock: &impl Clock) -> Result<http::HeaderMap, SignError>;
    // a fresh nonce per call; retries MUST re-sign (signed-requests.mdx:101-102, 163-167)

pub struct VerifyConfig { pub max_age: Duration /* 10 s */, pub allowed_skew: Duration /* 2 s */,
                          pub require_receiver: bool /* true */, pub wrapped: WrappedPolicy /* AcceptLikePython */ }
pub struct Verifier<S: NonceStore> { me: AccountId, cfg: VerifyConfig, store: S }
impl<S: NonceStore> Verifier<S> {
    /// Fails if store.retention() < max_age + allowed_skew. Python raises ValueError per request
    /// (http_auth.py:332-339); here it is a construction-time invariant.
    pub fn new(me: AccountId, cfg: VerifyConfig, store: S) -> Result<Self, ConfigError>;
    pub async fn verify(&self, headers: &http::HeaderMap, method: &http::Method, target: &http::uri::PathAndQuery,
                        body: &[u8], now: UnixNanos) -> Result<Caller, AuthError>;
}
pub struct Caller { pub hotkey: AccountId, pub nonce_ns: u64, pub crypto: CryptoType }
pub enum AuthError { MalformedAuth(&'static str), WrongReceiver(&'static str), StaleRequest(&'static str),
                     ReplayedRequest, BadSignature }   // messages byte-identical to Python's (safe to return to callers)
pub trait NonceStore: Send + Sync {
    fn retention(&self) -> Duration;
    fn check_and_store(&self, hotkey: &AccountId, nonce_ns: u64) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
pub struct InMemoryNonceStore { /* sharded HashSet + time wheel; bounded by max_entries (DoS guard), default retention 60 s */ }
```

Verification order is normative, so `verify` follows it exactly (`http_auth.py:242-343`):

1. Shape: version `"1"`, hotkey, decimal nonce, crypto (default sr25519), hex signature.
2. Receiver: required by default and must equal `me`.
3. Freshness: `now - nonce > max_age` is stale; `nonce - now > skew` is "future". rtao compares integer `i128`
   nanoseconds, where Python compares floats.
4. Signature, under the declared scheme only, over the payload rebuilt with the **received** receiver value.
5. Replay store, consulted only after the signature verifies.

`WrappedPolicy::AcceptLikePython` verifies through `rtao_core::keys::verify`. That function accepts both raw and
`<Bytes>…</Bytes>`-wrapped signatures (`CORE/keys/mod.rs:457-469`) and is the *same function* the Python verifier calls:
`http_auth.py:55,325` → `PY/sp_core.py:5,16` (`verify = bittensor_core.verify`) → the PyO3 forwarder
`UP/sdk/bittensor-core-py/src/keys.rs:188-193` → `keys::verify`. The acceptance sets are identical by construction
**[verified]**. `WrappedPolicy::Strict` uses the
proposed upstream `verify_exact` (§1.7), with an interim `schnorrkel` sr25519 (`signing_context(b"substrate")`) plus
ed25519 path. The docs say other implementations "are not required to accept the wrapped form"
(`signed-requests.mdx:239-243`).

**Golden vectors** (`fixtures/btauth/vectors.json`, transcribed from `signed-requests.mdx:252-289` and
`UP/sdk/python/tests/unit/test_http_auth.py:19-36`):

- sender `//Alice` (`5GrwvaEF…KutQY`), receiver `//Bob` (`5FHneW46…694ty`), nonce `1752076800000000000`;
- body `{"prompt": "hello"}` (19 bytes), sha256 `341c5744…c3cc`, request `POST /generate?stream=false`;
- sr25519: payload equality, plus sign→verify both ways (signatures are randomized);
- ed25519 (`5FA9nQDV…TTpu`): the signature must be exactly `0x1a6b6319…f704`;
- every negative case in `test_http_auth.py`: lowercase header keys, `Crypto: SR25519`/`1` accepted, stale at
  `nonce + 11 s`, future at `nonce − 3 s`, replay, a retention-0 store re-accepting, tampered body/method/path →
  `BadSignature`, missing header → `MalformedAuth`, `Crypto: ecdsa` → `MalformedAuth`.

The test file's policy carries over: breaking a vector requires a protocol version bump, not a fixture edit
(`test_http_auth.py:3-6`).

### 7.2 Server and client adapters (`rtao-http`)

```rust
// server (feature "server": axum 0.8 / tower 0.5)
pub struct BtAuthLayer<S: NonceStore, P: CallerPolicy> { verifier: Arc<Verifier<S>>, policy: Arc<P>, body_limit: usize }
impl<Svc, S, P> tower::Layer<Svc> for BtAuthLayer<S, P> { type Service = BtAuthService<Svc, S, P>; /* … */ }
// 1. buffer the body (limit, default 1 MiB) BEFORE any extractor; 2. target = req.uri().path_and_query() verbatim;
// 3. verify → on AuthError: 401 {"detail": "<python message>"} (the FastAPI recipe's shape, signed-requests.mdx:111-141);
// 4. policy.admit(&caller) → 403 {"detail": ...} for verified-but-unwelcome; 5. insert Caller + Priority into extensions;
// 6. re-inject the buffered body. Streaming request bodies are rejected (the hash needs all bytes); responses may stream.
impl<St: Send + Sync> axum::extract::FromRequestParts<St> for Caller { /* reads the extension */ }

pub trait CallerPolicy: Send + Sync {
    fn admit(&self, caller: &Caller) -> impl Future<Output = Result<Priority, Rejection>> + Send;
}
pub struct MetagraphPolicy {                        // "authentication says who; policy decides" (migration.mdx:462-471)
    pub metagraph: tokio::sync::watch::Receiver<Arc<Metagraph>>,
    pub require_registered: bool, pub require_validator_permit: bool, pub min_stake: Option<StakeWeightRao>,
    pub priority: PriorityFn,                       // default: caller stake weight (legacy template convention)
}
pub struct HotkeyRateLimitLayer { /* governor keyed by Caller.hotkey */ }
#[cfg(feature = "redis")] pub struct RedisNonceStore { /* SET btauth:{hotkey}:{nonce} 1 NX PX retention */ }

// client (feature "client": reqwest 0.13, rustls)
pub struct SignedClient<S: Signer> { http: reqwest::Client, signer: Arc<S>, clock: SystemClock }
impl<S: Signer> SignedClient<S> {
    pub fn request(&self, method: Method, url: Url, receiver: &AccountId) -> SignedRequestBuilder<'_, S>;
}
impl<S: Signer> SignedRequestBuilder<'_, S> {
    pub fn json<T: Serialize>(self, body: &T) -> Self;     // the body is materialized, then signed
    pub async fn send(self) -> reqwest::Result<reqwest::Response>; // signs per attempt; retries happen above signing
}
```

- Fan-out to many miners is `futures::stream::iter(..).buffer_unordered(n)`. The base URL comes from the metagraph:
  `http://{ip}:{port}`, or `[v6]:port` for IPv6 (`PY/metagraph.py:563-574`).
- Hot-path costs: with `MetagraphView` and a `HashMap<AccountId, Uid>` index, admitting a request is O(1). The 6.5 µs
  metagraph decode (§0) makes per-block refreshes cheap in CPU terms on the miner side; the RPC (0.7–0.9 s) dominates.

### 7.3 Legacy compatibility, as optional crates

- **`rtao-compat-synapse`** handles v10 `bt_header_*` interop with SN8, SN13 and SN21-style peers (R/D §6). It provides:
  - the header codec (`bt_header_axon_*`, `bt_header_dendrite_*`, `bt_header_input_obj_*` dummies, `name`, `timeout`,
    `computed_body_hash`);
  - the body hash `sha3_256(concat(sha3_256_hex(str(field)) for field in required_hash_fields))`, through a `PythonStr`
    trait that reproduces Python `str()` for str, int, float, bool, list and dict, defined per schema and tested against
    Python 10.5;
  - the signing message `"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"`;
  - the axon window `nonce > now − 4 s − timeout` with a monotonic table per `hotkey:uuid`, and the response-side axon
    signature;
  - status semantics (401/403/404/408/500/503) and raw-chunk streaming over `text/event-stream` (R/D §2.1–2.9).

  It is off by default. The docs mark it as a bridge (`signed-requests.mdx:291-321`).
- **`rtao-compat-epistula`** implements the spec dialect (`{sha256}.{uuid}.{ts_ms}.{signed_for}`, version `"2"`, 8,000 ms
  window). Knobs cover the BitMind (future-skew, 15 s), gobt and Apex (no uuid) dialects (R/D §3). It reuses `NonceStore`
  for uuids.

### 7.4 QUIC (`rtao-quic`, optional)

- A thin adapter over `btlightning` 0.2.15, which SN2 uses (R/D §4). It keeps wire compatibility with SN2, including its
  Python bindings for interop tests, instead of re-implementing the msgpack framing.
- It is not part of the default stack:
  - per-request authentication is absent (identity is bound to the connection);
  - the `protocol = 4` meaning "QUIC" collides with every SDK's default of 4 (R/D §4.7);
  - it pulls `sp-core 34` from crates.io next to bittensor-core's fork `sp-core 37` **[inferred from btlightning's
    manifest, R/D §4.1]**.
- The adapter disables btlightning's `subtensor` feature (subxt 0.50) and feeds it metagraph data from rtao.

---

## 8. Metagraph and typed models (`rtao-metagraph`)

### 8.1 Construction

One `SubnetInfoRuntimeApi.get_metagraph(netuid)` call, optionally followed by
`query_map(Commitments.CommitmentOf[netuid])` and `query_map(Commitments.RevealedCommitments[netuid])`, all pinned to one
block. This is the v11 recipe (`PY/metagraph.py:238-291`).

- The live finney response is ~49–51 KB for 256 UIDs, and a one-shot HTTPS `state_call` took 0.71–0.88 s
  **[measured]**. C2 §5.1
  estimates ~510 KB at 2,500 UIDs; the synthesized 2,500-UID payload is 408 KB **[measured]**.
- Variants:
  - `get_selective_metagraph(netuid, FieldSet)` with the generated `MetagraphField` (77 variants, §3.5). The `FieldSet`
    is a `u128` bitset. Presets are `FieldSet::VALIDATOR_LOOP` (C2 §5.2: block, tempo, last_step,
    blocks_since_last_step, weights_version, weights_rate_limit, activity_cutoff, the CR settings, num_uids, hotkeys,
    axons, active, validator_permit, last_update, emission, dividends, incentives, block_at_registration, total_stake)
    and `FieldSet::MINER_POLICY` (hotkeys, validator_permit, total_stake, validators).
  - `get_mechagraph(netuid, mechid)` and `get_selective_mechagraph`. The record's `netuid` field holds the storage index,
    and `NetUidStorageIndex::split()` recovers `(netuid, mechid)` (C2 §0 item 5).
  - `neurons(lite)` via `NeuronInfoRuntimeApi.get_neurons_lite`/`get_neurons` for the `neurons` read.
- `get_all_metagraphs`, `get_subnet_state` and `get_delegates` are never used in loops. They cost multiple megabytes or
  O(subnets × n) storage reads (C2 §5.1, §9 item 7).

### 8.2 Representation: borrowed columns over owned bytes

```rust
pub struct Metagraph {
    raw: bytes::Bytes,                      // the SCALE response (Option<Metagraph> payload), kept
    cols: Columns,                          // offsets of fixed-width columns + decoded compact columns
    pub header: MetagraphHeader,            // netuid, mechid, name, symbol, identity, owners, block, tempo, last_step,
                                            // pool terms, hyperparams (typed: AlphaRao/TaoRao/I96F32/PerU16/...)
    pub at: BlockRef, pub spec: SpecVersion,
    by_hotkey: OnceLock<HashMap<AccountId, Uid>>,   // O(1) caller lookup for miners
    pub commitments: Option<Commitments>,           // uid → NeuronCommitment, + unregistered by hotkey
}
struct Columns { n: u16, hotkeys: Range<usize>, coldkeys: Range<usize>, axons: Range<usize> /* 34 B each */,
    active: Range<usize>, validator_permit: Range<usize>,                    // borrowed fixed-width slices
    last_update: Vec<u64>, emission: Vec<AlphaRao>, dividends: Vec<PerU16>, incentives: Vec<PerU16>,
    consensus: Vec<PerU16>, block_at_registration: Vec<u64>, alpha_stake: Vec<AlphaRao>, tao_stake: Vec<TaoRao>,
    total_stake: Vec<StakeWeightRao>, collateral: Option<[Vec<AlphaRao>; 3]>, identities: Vec<Option<Range<usize>>>,
    alpha_dividends: Range<usize> }

impl Metagraph {
    pub fn len(&self) -> usize;
    pub fn hotkey(&self, uid: Uid) -> &AccountId;          // &raw[h + 32*uid ..] reinterpreted, no copy
    pub fn axon(&self, uid: Uid) -> AxonInfo;              // 34-byte fixed record decoded on demand → SocketAddr
    pub fn neuron(&self, uid: Uid) -> NeuronRef<'_>;       // row view; all columns
    pub fn by_hotkey(&self, hk: &AccountId) -> Option<NeuronRef<'_>>;
    pub fn validators(&self) -> impl Iterator<Item = NeuronRef<'_>>;
    pub fn iter(&self) -> impl ExactSizeIterator<Item = NeuronRef<'_>>;
    pub fn to_owned_neurons(&self) -> Vec<MetagraphNeuron>; // Python-shaped rows (PY/metagraph.py:127-161)
    pub fn to_json(&self) -> serde_json::Value;             // equals the Python Metagraph JSON (parity fixture, §11.5)
}
```

- **Decode cost** **[measured, BENCH strategy E vs C, Appendix A]**: 6.4–6.6 µs versus 29.6–32.8 µs (owned typed) at
  256 UIDs, and 57 µs versus 261 µs at 2,500. The view does not allocate the 2×n×32-byte key copies or the axon structs.
- **Safety.** The parser is bounds-checked and returns `Result` instead of the benchmark's `assert!`/`unwrap`. It is a
  `cargo fuzz` target (§11.6) and a property test against the generated owned `Metagraph` type, which is itself
  property-tested against bittensor-core's dynamic decode (§11.2).
- **Shape gate.** It is gated by the shape hash of `get_metagraph`'s output type. On mismatch, construction falls back to
  the dynamic `Value` and fills an owned variant: `Columns` is an enum with `Borrowed { ranges into raw }` and
  `Owned { Vec<AccountId>, Vec<AxonInfo>, … }`, and both serve the same accessors. It logs
  `shape_mismatch{descriptor="SubnetInfoRuntimeApi.get_metagraph"}`.
- **Deprecated columns.** `rank`, `trust` and `pruning_score` are empty vectors on chain
  (`UP/pallets/subtensor/src/rpc_info/metagraph.rs:782,803-804`). `subnet_emission`, `pending_root_emission` and
  `tao_dividends_per_hotkey` are zero (C2 §0 item 4). The view parses and skips them, and exposes them only as
  `#[deprecated]` accessors returning zeros, matching v11's `0.0` rendering (`PY/metagraph.py:311-313,357-362`).

### 8.3 Other typed models

These are generated wire types (`rtao-chain`) plus thin hand-written domain types with `From<wire>` (C2 §9 item 2):

- `SubnetInfo` (Tempo, Burn, SubnetworkN);
- `DynamicInfo`;
- `SubnetHyperparams` (from `get_subnet_hyperparams_v3`'s forward-compatible `Vec<HyperparamEntry>`, keyed by
  `HyperparamName` with a typed value per §5.4; unknown names are preserved as `Other(name, HyperparamValue)`);
- `NeuronInfoLite`;
- `StakePosition` (5 of `StakeInfo`'s 9 fields, as in `PY/reads/staking.py:16-62`; `locked`, `tao_emission` and `drain`
  are always zero on chain);
- `DelegateInfo` (take as `PerU16`, `return_per_1000` as `Balance<Tao>`);
- `AxonInfo` (`ip: u128`, `ip_type`, `port`, `protocol` as opaque `u8`, default 4; `fn socket_addr()`);
- `NeuronCommitment` (with the plain/sealed/revealed/failed status machine and `reveals_at` from the drand round;
  `PY/metagraph.py:45-125,475-560`);
- `LockState`, `MinerCollateralState`, `EpochStatus`, `SwapQuote`, `BasketSummary`, `Lease` and `Crowdloan`.

### 8.4 Sync cadence helpers

```rust
pub struct MetagraphSync { /* tokio task */ }
pub enum Cadence { EveryEpoch, EveryNBlocks(u32), Interval(Duration), OnEvents(EventSet) }
impl MetagraphSync {
    pub fn spawn(client: Client, netuid: NetUid, mechid: MechId, fields: FieldSet, cadence: Cadence) -> Self;
    pub fn current(&self) -> tokio::sync::watch::Receiver<Arc<Metagraph>>;
    pub fn diffs(&self) -> tokio::sync::broadcast::Receiver<MetagraphDiff>;   // uid → (old hotkey, new hotkey), permits
}
```

- `EveryEpoch` uses `EpochClock` (§3.11), refreshing at `LastEpochBlock` advance plus one block.
- `OnEvents` watches `NeuronRegistered`, `AxonServed`, `HotkeySwapped*` and `EpochTriggered` in finalized blocks
  (G §7 item 22).
- After `N` consecutive failures (default 3) the client reconnects, the SN2 pattern (G §2.4 item 12).
- The defaults follow observed practice: validators `EveryEpoch` with the `VALIDATOR_LOOP` field set, and miners
  `Interval(600 s)` with `MINER_POLICY` (SN2's 600 s miner refresh, G §3.4 item 7).
- `MetagraphDiff` drives score resets when a uid's hotkey changes (G §2.4 item 13). Scores are keyed by hotkey root
  (`HotkeyRoot`/`HotkeySuccessor` lineage), not by uid (G §2.4 item 13; `UP/docs/guides/mining/collateral.mdx:249-259`).

### 8.5 Persistence

v11 has none ("persist `mg.raw` yourself", `UP/docs/migration.mdx:308-317`). rtao adds an optional `persist` feature with
a self-describing file that the same parser can reload:

```
.rtmg = magic "RTMG" | u8 version | SCALE(Header{ genesis: H256, block: BlockRef, spec: u32, netuid: u16, mechid: u8,
                                               fields: Option<u128>, recorded_at_unix: u64 }) | raw response bytes
```

It is written atomically under `~/.bittensor/rtao/metagraphs/<network>/<netuid>/<block>.rtmg`. rtao never reads v10
`.pt` pickles (R/I §3.3). The validator template's `state.npz` reader is out of scope unless requested (R/I §7 item 5).

---

## 9. Framework for owners, validators and miners (`rtao-neuron`)

v11 Python ships no neuron stack: Axon, Dendrite and Synapse are gone (`UP/docs/migration.mdx:455-480`). rtao's
framework replaces the v10 stack with v11 semantics. The product requirements are G §7 (MUST/SHOULD/COULD).

### 9.1 Lifecycle abstractions

```rust
pub struct NeuronCtx { pub client: Client, pub executor: Executor, pub netuid: NetUid, pub mechid: MechId,
    pub hotkey: HotkeyFile, pub metagraph: watch::Receiver<Arc<Metagraph>>, pub epochs: EpochClock,
    pub http: SignedClient<HotkeyFile>, pub state: StateStore, pub shutdown: CancellationToken }

pub trait Validator: Send + 'static {
    type State: Serialize + DeserializeOwned + Default + Send;           // persisted (scores, step, …)
    fn on_start(&mut self, cx: &NeuronCtx, state: &mut Self::State) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn step(&mut self, cx: &NeuronCtx, state: &mut Self::State, tick: Tick) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn weights(&self, mg: &Metagraph, state: &Self::State) -> WeightsInput;  // relative, by uid or hotkey
    fn on_metagraph_diff(&mut self, d: &MetagraphDiff, state: &mut Self::State) { default_reset_changed_hotkeys(d, state) }
}
pub struct ValidatorRunner<V: Validator> { /* builder: cadence, weight policy, persistence interval, health log */ }
impl<V: Validator> ValidatorRunner<V> { pub async fn run(self, v: V) -> anyhow::Result<()>; }
```

`ValidatorRunner` runs a `tokio::select!` loop: `tick` (default 100 ms, as in SN2), metagraph diffs, epoch events, the
weight cadence, a state-save interval (default 300 s) and SIGTERM/ctrl-c. Shutdown drains in-flight weight submissions
(the SN2 validator-loop structure, G §2.5 item 15).

```rust
pub trait Miner: Send + Sync + 'static {
    fn router(&self) -> axum::Router;                                     // handlers see `Caller` from btauth
}
pub struct MinerServer<M: Miner> { /* bind addr, BtAuthLayer, MetagraphPolicy, rate limit, AxonPublisher */ }

pub struct OwnerTools { /* Executor + Wallet (coldkey) + EpochClock */ }
impl OwnerTools {
    pub async fn register_subnet(&self, hotkey: &AccountId, wait: RegistrationWait) -> Result<SubnetRegistered>;
    pub async fn start_call(&self, netuid: NetUid) -> Result<()>;          // checks subnet_start_schedule first
    pub async fn set_hyperparameter(&self, netuid: NetUid, h: Hyperparameter) -> Result<()>; // admin-window aware
    pub async fn status(&self, netuid: NetUid) -> Result<OwnerStatus>;     // both "on" flags, schedule, windows
}
```

`OwnerStatus` surfaces both activation flags: `subnet_is_active` from `start_call` (`SubtokenEnabled`), and the root-only
`subnet_emission_enabled`, which starts false (G §1.4 item 13).

### 9.2 Weights: normalization, cadence, rate limits, version key, mechanisms, Null

**Types make "submit unnormalized weights" unrepresentable.**

```rust
pub enum WeightsInput { ByUid(BTreeMap<Uid, f64>), Parallel { uids: Vec<Uid>, weights: Vec<f64> }, ByHotkey(Vec<(AccountId, f64)>) }
pub struct WeightRules { pub self_uid: Uid, pub max_weight_limit: PerU16, pub min_allowed_weights: u16,
                         pub consensus: EpochConsensus /* Yuma | Null{permit_holder} */ }
pub struct NormalizedWeights { uids: Vec<Uid>, values: Vec<u16> }       // only constructor: conform()
pub struct RawU16Weights { uids: Vec<Uid>, values: Vec<u16> }           // only via NullProof (Null subnets)
impl WeightsInput {
    pub fn conform(&self, r: &WeightRules) -> Result<NormalizedWeights, WeightsError>;
    pub fn raw_u16(&self, proof: NullProof) -> Result<RawU16Weights, WeightsError>;
}
```

`conform` is a line-for-line port of Python's `_conform` (`PY/intents/weights.py:54-114,218-258`):

1. Validate: equal lengths, finite and non-negative values, uids within u16, no duplicates.
2. If this is not a lone self-weight and `max_weight_limit < 65535`, run `clip_to_max_weight`: the cutoff search with
   `epsilon = 1e-7`, redistributing mass rather than discarding it.
3. Max-upscale: `round_ties_even(w / max · 65535)`. Python's `round()` is round-half-to-even, so `f64::round_ties_even`
   (stable since Rust 1.77) is required for byte parity.
4. Drop zeros.
5. Enforce `len ≥ MinAllowedWeights`, with the self-weight exemption and an `InvalidArgument` code.

Vectors are recorded from Python (§11.1). `raw_u16` requires a `NullProof` token that only the Null-mode preflight can
mint, so the raw path is unavailable on Yuma subnets by type (`PY/intents/weights.py:228-243,261-265`).

**WeightSetter (validator side).**

```rust
pub struct WeightSetter { /* Executor, HotkeyFile, netuid */ }
pub enum WeightCadence { EveryEpoch, WhenRateLimitAllows, Interval(Duration) }  // default EveryEpoch
pub struct WeightPolicy { pub cadence: WeightCadence, pub version_key: VersionKey /* Zero | Fixed(u64) | FromChain */,
    pub mechanisms: MechanismSel /* Zero | All | Listed */, pub hold_previous_on_all_zero: bool /* true */,
    pub retries: u8 /* 2: bt.set_weights parity, PY/_subtensor.py:167-205 */, pub reconcile: bool /* true */ }
```

- **Path selection** happens per submission from `CommitRevealWeightsEnabled` (default true on chain, C1 §2.3):
  - CR on: `commit_timelocked_mechanism_weights` (call 118). `commit_reveal_version` comes from
    `CommitRevealWeightsVersion` (4 on finney), never hard-coded (E §4.1 item 4).
  - CR off: `set_mechanism_weights` (call 119).
  - It never emits legacy `set_weights` (0) or `commit_crv3_mechanism_weights` (117) (C1 §7.5).
- **Rate limits.** `WeightsSetRateLimit` (default 100 blocks) applies to the commit when CR is on, and `LastUpdate` is
  bumped at commit (C1 §2.2–2.3). The setter computes `blocks_remaining` from
  `LastUpdate[NetUidStorageIndex(netuid, mechid)][uid]` and schedules rather than failing.
- **Pool contention.** Only one pending weight commit per `(hotkey, netuid_index)` is admitted; the provides-tag is
  `weight-commit` (C1 §2.2). The setter therefore never resubmits blindly. `reconcile` polls `LastUpdate[uid]` after a
  lost watch, the SN2 pattern (E §2.6).
- **Mechanisms.** One submission per `mecid < MechanismCountCurrent`, each with its own rate-limit slot (C1 §2.3 step 5).
- **Version key.** The default is `Zero`, v11's default (`UP/docs/migration.mdx:400-404`). `FromChain` reads
  `WeightsVersionKey` and submits `max(stored, own)`. Owner bumps invalidate in-flight commits at reveal time
  (G §1.5 item 16).
- **Null consensus.** Only the elected permit holder may set weights (`ensure_null_weight_permit`, C1 §2.3). Raw integers
  are preserved, and the ciphertext budget is `floor(32 KiB / mechanism_count)`, versus 5,000 bytes on Yuma. The
  preflight mints `NullProof` only for the permit holder.
- **Hold-previous guard.** If every miner weight is zero, the setter re-submits the previous row instead of an owner-only
  row that would burn the epoch (SN2 practice, G §2.6 item 21).

### 9.3 Serving the axon

```rust
pub struct AxonPublisher { /* Executor, HotkeyFile, netuid, AxonSpec, recheck: Duration (300 s) */ }
pub struct AxonSpec { pub external: ExternalAddr /* Fixed(SocketAddr) | Detect{ services, allow_ipv6 } */,
                      pub protocol: u8 /* 4 */, pub version: u32 /* 1 */, pub certificate: Option<NeuronCertificate> }
```

- **IP detection** uses ipify-style services with a 5 s timeout. The override is `--external-ip`, or
  `BT_AXON_EXTERNAL_IP`, which is SN2's convention; v11 Python removed the `BT_AXON_*` variables because it has no axon
  (G §4.2).
  Non-global IPv4 is rejected (loopback, RFC 1918, CGNAT, link-local) (G §3.3 item 5). IPv6 is encoded
  (`ip_type = 6`, `u128`) because the chain accepts it (C1 §3); SN2 refuses it (G §8 item 9).
- **Idempotent.** The publisher reads `Axons[netuid, hotkey]` first and serves only on change (SN2,
  `sn2-chain/src/registration.rs:44-62`). It is `ServingRateLimit`-aware (default 50 blocks).
- **Validation in types.** Port `0` and the IPv4 `127.0.0.1` are rejected before submission (`InvalidPort`,
  `InvalidIpAddress`, C1 §3). A certificate longer than 65 bytes is a type error, where the chain would silently drop it
  (C1 §3).
- `ResetAxon` on move or shutdown is optional. `ServeAxonTls` and `ServePrometheus` are intents (§4).

### 9.4 Registration

- **`burned_register`** (MEV-required). The preview splits `Burn[netuid]` into burned and locked parts using
  `CollateralLockShare` (`PY/intents/registration.py:27-44`). `register_limit` is exposed as a raw-path helper with a
  `limit_price: PriceRao`.
- **`pow_register`.** The challenge prefix is built from the live block hash (`work_block ≤ 5` blocks old). Mining runs
  through `rtao_core::pow::mine_cpu`, or `GpuMiner` under `gpu-pow`, in `offload` batches of up to `MAX_BATCH`
  (1,048,576). The challenge refreshes each block, and the work is re-verified on CPU. This ports
  `PY/pow_registration.py:18-253`. The seal is
  `keccak256(sha256(b"subtensor-pow-register-v1" ‖ netuid_le ‖ block_hash ‖ hotkey ‖ coldkey ‖ nonce_le))`
  (`CORE/pow/mod.rs:15-21`; `UP/pallets/subtensor/src/subnets/registration.rs:28-42`).
- **`root_register`** and confirmation: the `uid` read and the `NeuronRegistered` event.
- **`register_subnet` completion** ports `_complete_subnet_registration` (`PY/executor.py:333-568`). It scans the receipt
  for `NetworkAdded`. On `NetworkRegistrationQueued` it rescans the inclusion block (on-idle events are not in the
  receipt), then each height to the head, then follows heads. It accepts only a `NetworkAdded` whose `SubnetOwner` and
  `SubnetOwnerHotkey` match and which is not still in `NetworkRegistrationQueue`. Progress is reported through an
  `mpsc` of `RegistrationProgress::{Queued, Waiting, Registered}`, and `registration_timeout` is optional.

### 9.5 Hyperparameter management

- `hyperparams.toml` transcribes Python's `HYPERPARAMS` table: name → kind, doc, short (≤ 38 chars), signed, bits,
  minimum, maximum (`PY/hyperparams.py:91-408`). CI checks it against the upstream file hash (§12.4) and against
  metadata unit identities. That is the `--units` gate (`UP/sdk/python/codegen/check.py:472-507`), as a codegen test.
- `#[derive(Hyperparams)]` turns the table into:

  ```rust
  #[non_exhaustive]
  pub enum Hyperparameter {   // 34 owner-settable names over 33 AdminUtils setters (PY/intents/hyperparameters.py:31-66);
                              // alpha_low/alpha_high share sudo_set_alpha_values. Payload = setter arg type (C1 §5.3).
      EpochConsensus(EpochConsensus), Tempo(Blocks<u16>), ImmunityPeriod(Blocks<u16>), MinAllowedWeights(u16),
      WeightsVersion(u64), ActivityCutoffFactor(u32), MinBurn(Balance<Tao>), MaxBurn(Balance<Tao>),
      BondsMovingAvg(PerMillion), BondsPenalty(PerU16), ServingRateLimit(Blocks<u64>), CommitRevealPeriod(Epochs),
      MaxAllowedUids(u16), BurnIncreaseMult(U64F64), BurnHalfLife(Blocks<u16>), CollateralLockShare(PerU16),
      CollateralDrainRatio(U64F64), AdjustmentAlpha(U64Normalized), Rho(u16), MaxDifficulty(u64),
      AlphaSigmoidSteepness(i16), MinChildkeyTake(PerU16), OwnerImmuneNeuronLimit(u16),
      AlphaLow(PerU16), AlphaHigh(PerU16), CommitRevealWeightsEnabled(bool), LiquidAlphaEnabled(bool),
      RegistrationAllowed(bool), PowRegistrationAllowed(bool), Yuma3Enabled(bool), BondsResetEnabled(bool),
      TransfersEnabled(bool), OwnerCutEnabled(bool), OwnerCutAutoLockEnabled(bool),
  }
  ```

  Each variant's payload type is the AdminUtils setter's argument type, after unit substitution. The mapping to the
  generated call is exhaustive and checked by `match`. A new setter added upstream fails the coverage gate (§4.7 item 4)
  until it is mapped or listed raw-only.
- **Special cases are ported:**
  - `AlphaLow`/`AlphaHigh` read `AlphaValues[netuid]` and keep the other side (`PY/intents/hyperparameters.py:138-143`);
  - fixed-point values encode as `{bits}`;
  - `epoch_consensus` takes `Yuma|Null`.
- **Bounds** from the table are validated client-side: tempo 360–50,400, `collateral_lock_share ≤ 62258`,
  `burn_increase_mult` 1–3, and so on (G §1.5 item 16).
- **Chain gates are surfaced before signing**:
  - the per-parameter rate limit `OwnerHyperparamRateLimit` × tempo (default 2 tempos) via `LastRateLimitedBlock`;
  - the admin freeze window, `next_epoch_start − now < AdminFreezeWindow` (default 10) (C1 §5.2; G §1.5 item 17).

  `OwnerTools::set_hyperparameter` returns `NotYet { earliest_block }` instead of a doomed submission.
- **Reads.** `subnet_hyperparameters` (v3 entries) maps into `SubnetHyperparams`, with human rendering from the kind table
  (`PY/hyperparams.py:411-555`).

### 9.6 Timelock helpers and CRv3 (`rtao-timelock`)

```rust
pub struct DrandClient { /* async reqwest; quicknet endpoints from CORE/timelock/constants.rs:27-33; failover */ }
pub struct Round(pub u64);
impl Round { pub fn at(t: SystemTime) -> Self; pub fn reveal_time(self) -> SystemTime; } // genesis 1_692_803_367, 3 s
pub struct Timelocked<T> { pub round: Round, envelope: Vec<u8>, _t: PhantomData<T> } // SCALE UserData{encrypted_data, reveal_round}
impl<T: Encode + Decode> Timelocked<T> {
    pub fn seal(value: &T, at: RevealAt /* Duration | Time | Round */) -> Result<Self>;   // encrypt_at_round
    pub async fn open(&self, drand: &DrandClient) -> Result<T, TimelockError>;           // NotReady | ... (PY/timelock.py:68-72)
}
pub mod crv3 {
    pub async fn commit<V: View>(view: &V, w: &NormalizedWeights, hotkey: &AccountId, version_key: u64,
                                 clock: &impl Clock) -> Result<Crv3Commit, TimelockError>;
    pub struct Crv3Commit { pub ciphertext: BoundedCiphertext, pub reveal_round: Round, pub reveal_block: BlockNumber }
}
pub mod epoch {               // port of the PALLET scheduler (§1.6 item 1); vectors from CORE/timelock/epoch_schedule_vectors.rs
    pub struct EpochScheduleState { pub last_epoch_block: u64, pub pending_epoch_at: u64, pub subnet_epoch_index: u64,
                                    pub tempo: u16, pub blocks_since_last_step: u64, pub current_block: u64 }
    pub fn should_run_epoch(s: &EpochScheduleState, block: u64) -> bool;     // pending → true; bsls > tempo → true; …
    pub fn predict_first_reveal_block(head: &EpochScheduleState, reveal_period_epochs: u64) -> Result<u64, EpochError>;
}
```

`crv3::commit` performs these steps:

1. Read `Tempo`, `RevealPeriodEpochs`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex`, `BlocksSinceLastStep`
   and `Aura.SlotDuration` at **one** pinned block (`PY/intents/weights.py:283-306`).
2. Predict the reveal block with the corrected predicate.
3. Target `+3` blocks (`SECURITY_BLOCK_OFFSET`, `CORE/timelock/constants.rs:39`).
4. `round = floor((now + Δblocks·block_time − GENESIS)/3)`, minimum 1, with `now` from the injected `Clock`.
5. SCALE-encode `WeightsTlockPayload { hotkey: hotkey.to_vec(), uids, values, version_key }`
   (`CORE/timelock/mod.rs:53`).
6. `encrypt_and_compress(payload, round)`, then enforce the size limits: 5,000 bytes on Yuma, the Null budget on Null
   (C1 §2.3).

The model ignores `MaxEpochsPerBlock` deferral, as upstream does; the +3 offset absorbs small deferrals (C1 §10 item 4).
It is listed in Risks (§15). A pool-level `InvalidRevealRound` (custom code 15) means the round is below
`Drand.LastStoredRound`. The setter re-reads and recomputes once.

Commitments with timelocked fields go through `Commitments.set_commitment`, which is raw-only in Python's intent layer
(G §8 item 11). rtao provides a typed `SetCommitment` helper on the raw path, plus `Timelocked<T>` for field payloads.
The reveal status machine reads `CommitmentOf` and `RevealedCommitments` (`PY/metagraph.py:475-560`).

### 9.7 Configuration and CLI conventions for neuron binaries

```rust
#[derive(clap::Args, serde::Deserialize)]
pub struct NeuronConfig {
    #[arg(short = 'n', long, env = "BT_NETWORK", default_value = "finney", alias = "subtensor.network")]
    pub network: Network,                    // finney | test | archive | devnet | local | ws(s)://… (PY/settings.py:33-41,93-107)
    #[arg(long, env = "BT_CHAIN_ENDPOINT", alias = "subtensor.chain_endpoint")] pub chain_endpoint: Option<Url>, // overrides "local"
    #[arg(short = 'w', long, env = "BT_WALLET", default_value = "default", alias = "wallet.name")] pub wallet: String,
    #[arg(short = 'H', long, env = "BT_WALLET_HOTKEY", default_value = "default", alias = "wallet.hotkey")] pub wallet_hotkey: String,
    #[arg(long, env = "BT_WALLET_PATH", default_value = "~/.bittensor/wallets", alias = "wallet.path")] pub wallet_path: PathBuf,
    #[arg(long, env = "BT_WALLET_PASSWORD_FILE")] pub wallet_password_file: Option<PathBuf>,
    #[arg(long)] pub netuid: NetUid,
    #[arg(long, value_delimiter = ',')] pub fallback_endpoints: Option<Vec<Url>>,   // "none" pins
    #[arg(long, value_delimiter = ',')] pub archive_endpoints: Option<Vec<Url>>,
    #[arg(long)] pub retry_forever: bool,
    #[arg(long, env = "BT_AXON_EXTERNAL_IP", alias = "axon.external_ip")] pub external_ip: Option<IpAddr>,
    #[arg(long, alias = "axon.port")] pub port: Option<u16>,
    #[arg(long, env = "BITTENSOR_LOG", alias = "logging.level")] pub log: Option<LogLevel>,
}
```

- Precedence is flag > environment > config file > default (`UP/docs/cli.mdx:21-22`).
- v11 flag spellings are primary. v10 dotted spellings are clap aliases, the SN2 precedent
  (`sn2-validator/src/cli.rs:9-24`; R/I §3.4).
- **Genesis identity.** `Network` presets carry the genesis hashes (finney `0x2f0555cc…36c03`, testnet `0x8f9cf856…`,
  devnet `0x07789904…`; H §1.1). The client refuses to sign against an endpoint whose genesis does not match its claimed
  network.
- **Logging.** `tracing` with a compact key=value formatter by default, in the style of the user's `mev-scanner` (E §6).
  Library crates never print.

### 9.8 EVM helpers (`rtao-evm`, optional)

This is the parity surface of `PY/evm/*`:

- `h160_to_ss58`, which mirrors `blake2_256("evm:" ‖ h160)` (`PY/intents/evm.py:26-33`), and `ss58_to_h160_truncated`;
- the keystore-V3 EVM keys stored beside the wallet;
- network facts (chain ids 964/945);
- the precompile catalog with vendored ABIs;
- a minimal EVM JSON-RPC client;
- 18-vs-9 decimal conversions (`WEI_PER_TAO`);
- `association_proof`, the EIP-191 signature over `hotkey_pubkey ‖ keccak256(scale(u64 block))` (`PY/intents/association.py:44-95`).

It uses `alloy-primitives`/`alloy-sol-types` for ABI encoding and `alloy-signer-local` for signing. The `alloy` graph
(k256) stays out of default builds. The intents `fund_evm_key`, `evm_withdraw` and `associate_evm_key` live in
`rtao-intents` and use only the address math.

---

## 10. CLI (`rtao-cli`, binary `rtao`)

### 10.1 Generation from the registries

Python's `btcli tx` and `btcli query` are generated from the intent and read registries (`PY/cli/tx.py:1-11,487-495`;
`PY/cli/query.py:135-263`). rtao does the same from `INTENTS` and `READS`, driven by their JSON schemas, so agents' JSON
and the CLI share one parsing path:

```rust
fn command_for(entry: &IntentEntry) -> clap::Command;  // properties → --kebab-flags; type/enum/x-rtao-* hints → value
                                                       // parsers; `description` → help; `required` → required args
fn args_to_json(entry: &IntentEntry, m: &clap::ArgMatches, cx: &AppCx) -> serde_json::Value; // + address resolution
fn run_tx(entry: &IntentEntry, json: Value, cx: &AppCx) -> Result<ExitCode> {
    let intent = (entry.from_json)(json)?;            // the same serde path as execute_tool (typed validation in TryFrom)
    let plan = intent.plan(&cx.executor, cx.keys(), cx.plan_opts()).await?;   // fee, effects, warnings, violations
    cx.output.plan(&plan);                            // review card (facts/facts_title)
    if cx.dry_run { return Ok(ExitCode::SUCCESS) }    // --dry-run: full plan, no submission (UP/docs/agents.mdx:74-85)
    cx.confirm_or_decline()?;                         // non-TTY or --json without --yes → declined, exit 1
    let r = intent.execute(&cx.executor, cx.keys(), cx.exec_opts()).await?;
    cx.output.result(&r)                              // --json: ExtrinsicResult::to_json on stdout
}
```

- **Schema hints.** The derive emits `x-rtao-address: hotkey|coldkey|any`, `x-rtao-money: tao|alpha`,
  `x-rtao-all: true` and `x-rtao-list: csv|json`. They encode Python's conventions:
  - addresses accept ss58, an address-book name, a proxy-book name, a saved multisig, or `HOTKEY`/`WALLET/HOTKEY`;
  - `--hotkey` and `--coldkey` default to the configured wallet, and destinations never default (`UP/docs/cli.mdx:61-67`);
  - list fields accept CSV or JSON (`PY/cli/tx.py:62-82`).
- **Help panels** group by pallet; privileged intents are grouped by origin (`PY/cli/tx.py:470-484`).
- **Query rendering** uses `RenderHint` (`Grouped`/`Matrix`) for non-record outputs, tables for records, and
  `--json` otherwise.

### 10.2 Command tree (v11 names primary, hidden v9 aliases)

`rtao` has:

- `tx <op>` (83, generated) and `query <read>` (102, generated);
- `call Pallet.function --args '{json}' [--sudo] [--multisig NAME]`, raw, through `compose_dyn` and Policy
  `allow_raw_calls`;
- `tools` (intent catalog JSON in Python's `list_tools` shape), `reads`, and `explain <code|ChainErrorName>`;
- hand-written groups mirroring `PY/cli/main.py:99-144`:
  - `wallet`: create, new-coldkey, new-hotkey, regen-*, show, list, balance, overview, sign, verify, swap-*, keychain;
  - `stake`: add, remove, move, move-swap, transfer, swap, burn, unstake-all*, show, list, set-auto, child*;
  - `root`, `subnets` (list, show, hyperparameters, burn-cost, create-cost, price, metagraph, register, create);
  - `conviction`;
  - `hparams` (set, get, start, check-start, set-identity, set-symbol, trim, trim-null-batch, mech count/emissions);
  - `addr`, `evm`, `proxy` (+ `proxy book`), `multisig`, `config`;
  - `misc` (weights commit/reveal/set, axon set/reset, collateral, timelock, crowd, extension, upgrade);
- `doctor`, which is rtao-only: the Cargo.lock patch check (§1.1), endpoint capabilities, genesis check and clock skew.

**Global flags** follow `PY/cli/globals.py:52-375`: `-n/--network`, `-w/--wallet`, `-H/--wallet-hotkey`,
`--wallet-path`, `--wallet-password-file`, `--json`, `-y/--yes` (alias `--no-prompt`), `--dry-run`, `-q`,
`-v/-vv/-vvv`, `--proxy-for`, `--force-proxy-type`, `--signer wallet|ledger|vault|extension`, `--signer-address`,
`--signatory`, `--ledger[-account|-index]`, `--mev-shield/--no-mev-shield`, `--fallback-endpoints`,
`--archive-endpoints`, `--retry-forever`.

**Behaviour contracts:**

- Exit codes: 1 for a declined mutation, 2 for a missing required option (`PY/cli/context.py:2088-2103`).
- JSON on stdout, diagnostics on stderr. `BITTENSOR_LOG` = error|warning|info|debug|trace; `-vvv` dumps raw websocket
  frames (`PY/cli/logs.py:8-91`).
- MEV-shield precedence: required > flag > config `mev_shield` > intent default. Shielding is downgraded when free TAO
  cannot cover the carrier fee (`PY/cli/context.py:260-292,1047-1080`).

**Files are shared with btcli** (read/write compatible, atomic writes, corrupt files renamed `.corrupt.<ts>`):

- `$BTCLI_CONFIG` or `~/.bittensor/btcli.json` (keys `network, wallet, wallet_hotkey, wallet_path, json, quiet,
  signer_address, extension_browser, fallback_endpoints, archive_endpoints, retry_forever, proxy_for, mev_shield`);
- `btcli_proxies.json`, `btcli_addresses.json`, `btcli_multisigs.json`;
- the subnet-name cache (24 h) and the token-symbol cache (7 d) (`PY/config.py:12-95,138-416`).

### 10.3 Scope per milestone (§14)

| Milestone | CLI scope |
|---|---|
| M3 | `query` (validator/miner reads), `wallet list/balance/new-*/regen-*`, `misc axon set/reset`, `misc weights set`, `subnets metagraph`, `doctor` |
| M4 | `tx` (all 83, generated), `call`, `tools`, `explain`, `stake *`, `hparams *`, `proxy`, `multisig`, `root` |
| M5 | `query` (all 102), `subnets *`, `conviction`, config/books compatibility tests against btcli |
| M7 | `--signer ledger/vault/extension`, `evm *`, `misc timelock/crowd/extension/upgrade` |

---

## 11. Testing

The pyramid has five layers:

1. pure golden-vector tests;
2. type-driven property tests;
3. `FakeChain` behaviour tests;
4. docker localnet e2e;
5. Python interop e2e.

Fuzzing and benchmarks run beside them. The unit and golden layers run on every PR. Localnet runs on PR label and
nightly. Interop runs nightly and before release.

### 11.1 Golden fixtures reused from upstream

Fixtures are vendored under `fixtures/upstream/` with `MANIFEST.toml` (source path, upstream commit `850f2755…`,
sha256). `cargo xtask import-upstream --rev <sha>` refreshes them and reports diffs (H §3.4). Licences are Apache-2.0 with
attribution.

| File | Upstream path | Assertions in rtao |
|---|---|---|
| `golden-461.json` (1,508,346 B; spec 461, genesis `0x1dc00e76…`; embeds `metadata.v15_hex` and `v14_hex`) | `UP/sdk/python/tests/fixtures/golden.json` | build `RuntimeEntry` from the embedded V15 (like `CORE/codec/mod.rs:80-109`); see the sections below |
| `corpus-461.json` (1,076,611 B; 797/802 types; 2,899 samples) | `UP/sdk/python/tests/fixtures/shape_corpus/corpus.json` | (a) the shape-hash function is total and deterministic over all 802 types (no panics, stable across two registries); (b) rtao's dynamic JSON rendering (bittensor-core `Value` → `serde_json`) equals the corpus `decoded` (`CORE/codec/value.rs:58-102` is the reference renderer); (c) for every sample whose type is reachable from a generated type: generated `Decode` of `scale_hex` equals `rtao_core::Value` projection (§11.2) |
| `ledger_proof_vector.json` | `UP/sdk/bittensor-core/fixtures/ledger_proof_vector.json` | `generate_extrinsic_proof` equality through the `rtao-signer` Ledger path (`ledger` feature) |
| btauth vectors | `UP/docs/guides/signed-requests.mdx:252-289`, `UP/sdk/python/tests/unit/test_http_auth.py:19-36` + negatives | §7.1 |
| RFC-0078 digest `affbcc25…11c0` over the 461 V15 with ChainInfo `("node-subtensor", 42, 9, "TAO", 461)` | `CORE/digest/mod.rs:150-201` | through `rtao-core` |
| Epoch-schedule vectors | `CORE/timelock/epoch_schedule_vectors.rs` (86 lines; a private module, `CORE/timelock/mod.rs:14`, so the file is vendored, not imported) | rtao's corrected port reproduces them; extra vectors for `BlocksSinceLastStep > tempo` |
| PoW seal vectors | `CORE/pow/mod.rs:88-105` | through `rtao-core` |
| Null raw-weight rows (2,500 entries) | `UP/sdk/python/tests/unit/test_null_weights.py:13-45` | `RawU16Weights` preserves `[65534, 1, 1, …]` exactly |

The sections of `golden.json` covered:

- `storage_keys` (10 keys, incl. double/triple maps): typed key encoding equals `key_hex`.
- `calls` (10, incl. nested `Sudo`/`Utility.batch`/`Proxy.proxy`): typed `compose` *and* `compose_dyn` equal `data_hex`.
  Pallet and call indices are unchanged 461→477 (H §2.2), so the generated types apply.
- `signature_payloads` (immortal, and mortal with period 64): the `UnsignedExtrinsic::payload()` bytes.
- `extrinsics` (2, fake signature `1..=64`): `attach()` gives `extrinsic_hex` and `extrinsic_hash` byte for byte.
- `multisig` (3) and `ss58` (4).
- `constants`, `storage_values` (incl. the `NetworksAdded[9999]` miss → default).
- `events` and `receipt`: `Events::find::<Transfer>()`, fee from `TransactionFeePaid`, `ExtrinsicSuccess`.
- `query_maps` (4): `storage_iter` key recovery.
- `runtime_calls` (4): typed `get_neurons_lite(1)`, `get_subnet_hyperparams(1)` and
  `get_stake_info_for_hotkey_coldkey_netuid` when the 461 shape hash matches 477, otherwise the dynamic path;
  `AccountNonceApi`.
- `block`: `decode_extrinsic` of the block body.
- `payment_info`: the `RuntimeDispatchInfo` decode.

**Vectors recorded by the interop job and then frozen.** Upstream has no committed keyfile ciphertext vectors (R/B §8).
The Python interop job (§11.5) records them once and commits them:

- `$NACL`, legacy Fernet and Ansible keyfiles with known passwords, plus btcli-created wallets;
- Python `conform()` inputs and outputs for 200 random and edge-case weight vectors (ties, clipping, `MaxWeightsLimit`
  boundaries, self-weight);
- Python JSON for every read (`fixtures/parity/reads/`).

From then on, rtao tests against these frozen files offline.

**rtao's own recorder** (`cargo xtask record --image …:v<spec>`) mirrors `UP/sdk/python/scripts/record_golden.py` so the
JSON schema stays compatible (H §5.1). It adds a metagraph response, a CRv3 composed commit and a v5 encoding probe.
Read-only finney vectors go to `golden-<spec>-finney-ro.json`.

### 11.2 Type-driven property tests (the safety net for positional decoding)

- **typed == dynamic, for every generated type and pinned spec.** A `proptest` strategy generates SCALE bytes from the
  *registry* (porting `ByteGen` from `UP/sdk/python/scripts/record_shape_corpus.py:47-130`: compact widths, every enum
  variant, sequences 0–3, depth ≤ 8). For each type `T` in `rtao-chain`, it asserts:
  - `T::decode(bytes)` succeeds iff `rtao_core` `decode_id` succeeds, with bittensor-core's strict cursor so both reject
    non-canonical bools and compacts (`CORE/codec/decode.rs:40-52`);
  - `Value::from(&T)` (a generated projection) equals the dynamic `Value`;
  - `T::encode()` round-trips the bytes.

  This is what makes positional decoding trustworthy, and it runs per committed metadata file.
- **Shape-hash soundness.** For random registry mutations (a field rename, reorder, compact flip, new variant, changed
  array length), the hash changes. For path- or doc-only edits it does not.
- **Units.** Exact decimal round trips; `checked_add` across netuids errors; overflow returns `None`;
  `NetUidStorageIndex::new(n, m).split() == (n, m)`; `to_wire_for` rejects a foreign netuid.
- **Weights.** `conform` is idempotent on its own output, max value 65535, no zeros, and deterministic. It differs from
  the Python vectors by zero, not by ±1.
- **Nonce leases.** A random interleaving of lease, accept, reject and drop never yields a nonce gap or reuse within one
  process. This is checked with `tokio::time::pause` and a fake chain counter.
- **btauth canonicalization**, as listed in R/D §8.3 item 6: arbitrary method, target, body and nonce; percent-encoding
  preserved; boundary at exactly `max_age`/`allowed_skew`.

### 11.3 `FakeChain`: the mock transport (`rtao-testkit`)

This is the Rust analogue of `UP/sdk/python/tests/harness/fake_node.py` (an in-process WebSocket JSON-RPC server with
drop/refuse/silence injection) combined with `fake_substrate.py` (dict-backed state) (H §5.4; R/A2 §6.3):

```rust
let chain = FakeChain::builder()
    .metadata(include_bytes!("../../../metadata/finney/477.v15.scale"))   // Runtime parsed by bittensor-core
    .genesis(FINNEY_GENESIS).spec(477, 1)
    .seed_storage(&storage::subtensor_module::TEMPO, (NetUid(1),), 360u16)  // typed seeding via descriptors
    .seed_runtime_api(&runtime_apis::subnet_info::GET_METAGRAPH, (NetUid(1),), golden_metagraph())
    .on_submit(Script::InBlockThenFinalized { events: golden_receipt_events() })
    .inject(Fault::DropAfterFrames(3)).inject(Fault::PolicyClose1008).inject(Fault::Silence(Duration::from_secs(61)))
    .spawn().await;                                                          // ws://127.0.0.1:<ephemeral>
```

What runs on it:

- every read at the head and pinned (Python does this at block 90; `test_reads_table.py:100-110`);
- `RecordingView` target verification (§4.5);
- every intent's `plan`, with permissive default state chosen so preflights pass, like `DEFAULT_STORAGE`
  (`fake_substrate.py:34-77`);
- executor retries, `FateUnknown`, reconcile scans, shield inner resolution and register_subnet queued completion;
- the transport semantics of §3.2 under each fault.

Sample parameters for all 102 reads come from a ported `READ_SAMPLES` table (`UP/sdk/python/tests/harness/samples.py`).
A meta-test fails if a read has none.

### 11.4 Localnet e2e (docker)

- **Images** (`ghcr.io/raofoundation/subtensor-localnet`), pinned by digest in `tests/localnet.toml`:

  | Lane | Tag | Status |
  |---|---|---|
  | pinned | `v477` (the compat floor; mainnet builds only, H §1.3) | required |
  | `mainnet` | equals `v477` today | required |
  | `testnet` | — | required; failures open an issue rather than block |
  | `devnet` | — | allowed to fail |
  | `main` | equals `latest` | weekly, allowed to fail |
  | wasm-override | the `mainnet` node with a released `subtensor.wasm` via `SUBTENSOR_LOCALNET_RUNTIME_WASM` (H §1.3) | for specs without images |

  Do not use upstream's default tag `monorepo-sdk`; it does not exist (H §6 item 1).
- **Modes.** `True` gives fast blocks: 250 ms, tempo 10, and up to 32 epochs per block (H §1.3). Timing tests run under
  `False` (12 s blocks), such as CRv3 reveal-round prediction, rate-limit math and the metagraph cadence.
- **Harness.** It uses the plain `docker` CLI (no testcontainers dependency), with an env contract copied from upstream
  (`UP/sdk/bittensor-core/tests/e2e_support/mod.rs:23-120`): `E2E_ENDPOINT` reuses a running node, `LOCALNET_IMAGE`
  overrides the image, and `SKIP_PULL` skips the pull. It maps ephemeral ports `-p 127.0.0.1::9944`, waits for the image
  healthcheck to report `healthy`, falling back to `Imported #1` in the logs, and runs `docker rm -f` on drop.
- **Accounts.** Alice, Bob and Charlie hold 1,000,000 TAO each; Alice is sudo; subnets 0 and 1 exist at start (H §1.3).
- **Sharding.** `nextest` test groups run one container per test *binary group*, with serial tests inside a group.
  Upstream runs one container per test across 32 shards (`check-bittensor-e2e-tests.yml:434-558`).
- **Test inventory:**
  - mirror the 112-entry `e2e-manifest.json` scenarios (73 `intent_*` plan tests + 39 behaviour tests; H §3.5);
  - plus rtao-specific tests:
    - validator sets weights on the plain path (CR disabled via sudo), and on CRv3 with the chain auto-reveal observed
      as a `TimelockedWeightsRevealed` event;
    - mechanism weights with `mecid` 1;
    - a Null-mode raw u16 row;
    - the metagraph view equals the dynamic decode on real state;
    - a btauth client and server between two rtao processes;
    - a shielded `burned_register` whose inner receipt is resolved;
    - `register_subnet` with `start_call`;
    - hyperparameters respecting the admin window;
    - nonce pipelining of 50 transfers with no gaps;
    - reconnect during `submit_and_watch` yielding `FateUnknown`, then reconcile;
    - the PoW register path (non-fast runtime).
- **Weekly mainnet-clone lane (optional).** `node-subtensor build-patched-spec` from `raw_spec_finney.json` (20–35 min
  sync, `UP/clones/scripts/clone-mainnet.sh:12-36`), then the read-only suite over all netuids. This catches state that
  decodes on localnet but not at mainnet scale (H §3.5).

### 11.5 Python interop e2e (docker compose)

The rtao crates are 100% Rust. Python appears only as a CI oracle (R/F §11 item 9). The compose file runs:

- the localnet;
- `python:3.12` with `uv pip install bittensor==<stable matching the pinned spec>` (11.3.0 for the 475/477 era; H §2.5);
- an rtao test binary.

| Scenario | Direction | Assertions |
|---|---|---|
| btauth/1 | Python `http_auth.sign` → rtao `BtAuthLayer`; rtao `SignedClient` → the FastAPI recipe server (`signed-requests.mdx:104-157`) | both schemes; GET with empty body, POST JSON, percent-encoded target, unbound receiver with and without opt-out, stale, future, replay, tampered body; status codes and `detail` strings equal |
| Validator ↔ miner | Python v11 miner (FastAPI + `verify`) ↔ rtao validator; rtao miner (`MinerServer`) ↔ Python validator (httpx `HotkeyAuth`) on a localnet subnet | requests authenticated, policy 403 for unregistered callers, weights set by the rtao validator visible in Python's `weights` read |
| Wallets | `btcli wallet new-coldkey/new-hotkey/regen-coldkey --seed` → rtao opens, unlocks and signs; rtao creates → `btcli wallet show/sign` reads | addresses equal; `$NACL` round trip; legacy Fernet decrypt; `BT_WALLET_PASSWORD` and `BT_PW_*` resolution |
| Wire bytes | Python emits `compose`, `signature_payload` and fake-signed extrinsics for a fixed call set at the *current* spec (`record_golden.py:239-311`) | rtao equals them byte for byte (a spec-current golden.json) |
| Read outputs | for all 102 reads, Python `client.read(name, **sample)` and `rtao query <name> --json` at the same pinned block | normalized JSON equal, or the entry listed in `divergences.toml` |
| Metagraph | `client.subnets.metagraph(n)` vs `Metagraph::to_json()` | equal |
| Prepared extrinsics | Python `prepare_call` → rtao `submit_signature`, and the reverse | accepted on chain |
| Runtime disk cache | Python writes `md1-*.bin`; rtao reads it, and the reverse | same parsed spec |

### 11.6 Fuzzing

`cargo fuzz` targets run briefly on a nightly cron, following SN2's practice (H §3.8):

- `metagraph_view` (arbitrary bytes, and mutations of real payloads);
- `btauth_verify` (headers × body);
- `events_split`;
- `transport_frame` (JSON-RPC frames, subscription notifications);
- `keyfile_json` (through bittensor-core's parser);
- `ss58_parse`;
- `prepared_extrinsic_json`.

### 11.7 Benchmarks (criterion)

The baselines are this document's measurements (Appendix A), tracked in CI history. The gate is a gross regression
(> 30%):

- metagraph decode (strategies A, C and E) on the committed finney payloads and the 2,500-UID synthetic payload;
- `Runtime::parse` (V15);
- typed storage key derivation (10k keys);
- `query_map` page decode (typed vs `decode_map_page`);
- sign + assemble (upstream: 0.03 ms, `UP/sdk/bittensor-core-spec.md:360-382`);
- btauth sign and verify throughput;
- weights `conform` at 256 and 2,500 entries.

---

## 12. Upstream tracking

Upstream moves fast: 9 finalized mainnet runtimes in 20 days (v466→v477), abandoned pre-release tags, and `main` running
ahead of mainnet (H §0 items 2–3, §1.2). So tracking is automated end to end, and humans review classified PRs.

### 12.1 PR gates (offline, fast)

```yaml
# .github/workflows/pr.yml (abridged)
jobs:
  check:
    strategy: { matrix: { os: [ubuntu-24.04, ubuntu-24.04-arm, macos-14] } }
    steps:
      - uses: actions/checkout@<sha>               # SHA-pinned actions (E §4.1 item 13)
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
      - run: cargo nextest run --workspace --all-features --locked
      - run: cargo test --doc --workspace --all-features --locked
      - run: cargo xtask codegen --check            # regenerate rtao-chain from committed metadata, diff
      - run: cargo xtask parity --check             # §4.7 (vendored catalogs, coverage gate, mev flags)
      - run: cargo xtask deps-check                 # §2.3 invariants (no subxt, host only where allowed)
  msrv:   { steps: [ "cargo +1.89 check --workspace --all-features --locked" ] }
  hack:   { steps: [ "cargo hack check --feature-powerset --depth 2 -p rtao" ] }
  supply: { steps: [ "cargo deny check", "cargo audit (0.22.2; documented ignores like UP/.github/workflows/cargo-audit.yml:53-85)" ] }
```

Docker exists only on the Ubuntu runners, so the localnet lanes run there (R/F §9).

### 12.2 `drift`: runtime metadata (structural), every 6 hours

For each of finney, testnet, devnet and the `localnet:main` image:

1. JSON-RPC `chain_getFinalizedHead`, then `state_getRuntimeVersion(at)`, then
   `state_getStorageHash(":code" = 0x3a636f6465, at)`. Compare `(spec, code_hash)` with `metadata/index.toml`, as in
   `UP/.github/scripts/mainnet-finalized-runtime.sh:19-26`. If unchanged, stop.
2. Fetch `Metadata_metadata_at_version` 15 and 16 at that block, and store `metadata/<net>/<spec>.v{15,16}.scale`. For
   finney, also cross-check the release asset `subtensor-digest.json` (wasm `blake2_256` and the `apis` table) when tag
   `v<spec>` exists (H §1.1).
3. Run a semantic diff with `cargo xtask drift diff old new`. `subxt diff` reports no events, errors, indices or constants
   (H §2.3), so rtao needs its own. The diff covers:
   - pallets (index, name);
   - calls (index, name, argument names and shape hashes, deprecation);
   - events (index, name, field shape);
   - errors (index, name);
   - storage (hashers, key/value shapes, modifier, default);
   - constants (type, value);
   - runtime APIs (name, version, method shapes);
   - transaction extensions (identifier, `ty`/`implicit` shape);
   - extrinsic versions, and `System.Version` (`spec_name` must stay `node-subtensor`).

   It classifies each change:
   - **breaking**: a moved pallet, call or error index; changed arguments of a wrapped call; a removed wrapped call or
     used storage entry; an extension change with a non-empty payload; a `transaction_version` change; V15 no longer
     offered;
   - **behavioural**: a moved event index; a storage type or default change; a runtime-API version bump; a changed shape
     hash on any descriptor rtao types;
   - **additive**: new calls, events, errors, storage or APIs;
   - **surface-unchanged**: only `System.Version` changed. This is the common case (R/CORRECTIONS.md item 2: 477→478
     differs in one byte).
4. Regenerate `rtao-chain`, then run the offline suite and the §11.2 property tests for the new metadata file.
5. Open or refresh one PR per network, titled `drift(<net>): <old>→<new> [<class>]`. The body carries the classified
   diff, the verbatim `subxt diff`, the GitHub release notes when finney, and the catalog diff (§12.3) when available.
   For a surface-unchanged change, the PR only touches `metadata/index.toml` and `SPEC_RANGE` and can auto-merge after
   CI.
6. The PR's e2e lanes run against `:v<spec>` (finney), `:testnet` or `:devnet`. Devnet drift is "upcoming" and finney
   drift is "now". Hours to days of lead time come from the train order: devnet first, then testnet, then mainnet
   (`UP/docs/internals/release-process.mdx:14-72`).

### 12.3 `catalog-drift`: the Python surface (semantic), daily

- Fetch `https://www.bittensor.com/catalog/{intents,reads,errors}.json` and `/code/index.json`, and diff against
  `parity/upstream/` keyed by name (R/I §5.4; H §1.6):
  - new or removed intents and reads;
  - `wraps`, `input_schema`, `params`, `category` and `verify` changes;
  - error reclassifications and new chain errors.
- Open a PR that rewrites `parity/upstream/*`. The parity gate then fails until each item is implemented or allowlisted
  in `divergences.toml`.
- The catalogs regenerate only on production site deploys, after mainnet releases (`watch-mainnet-release.yml:482-525`).
  They lag testnet, so §12.4 provides earlier signals from source.

### 12.4 `upstream-files`: protocol and taxonomy sources, daily

This watch hashes upstream files on the `main`, `testnet` and `mainnet` branches. It opens an issue with the diff when a
hash changes, and a PR when an rtao-vendored derivative must change.

| Upstream file | rtao derivative |
|---|---|
| `sdk/python/bittensor/http_auth.py`, `docs/guides/signed-requests.mdx` | btauth vectors and messages; a bump to `btauth/2` (H §4c) |
| `sdk/python/bittensor/error_map.py`, `common/src/transaction_error.rs` | `CUSTOM_TX_ERRORS`, classification |
| `sdk/python/bittensor/hyperparams.py` | `hyperparams.toml` |
| `sdk/python/bittensor/intents/*.py` | `parity/mev.toml`, preflight semantics |
| `sdk/python/bittensor/_transport/rpc.py`, `interface.py`, `runtime.py` | transport semantics review |
| `sdk/python/bittensor/settings.py` | network presets, endpoints, eras |
| `sdk/python/bittensor/{wallets,keyfiles,macos_password}.py` | password order |
| `sdk/python/codegen/check.py` | `raw_only.toml`, covered pallets |
| `runtime/src/fee_filters.rs` | `COLDKEY_PAYS_FEE_CALLS` |
| `runtime/src/check_mortality.rs` | `ShieldPeriod` maximum |
| `pallets/subtensor/src/coinbase/run_coinbase.rs` (`should_run_epoch`) | `rtao-timelock::epoch` |
| `pallets/subtensor/src/rpc_info/metagraph.rs` (`freeze_struct` hashes `93460b9b…`, `bb742022…`) | metagraph view parser, `MetagraphField` |
| `eco-tests/**` | early warning for metagraph-shape changes (H §1.8) |

### 12.5 `core-bump`: on each finalized `v<spec>` tag

The trigger is the finney drift job observing a new finalized spec whose tag exists and is not a pre-release. The job
runs `cargo xtask core-bump --tag v<spec>` (§1.1), the seam contract suite (§1.1 step 3) and the full CI, then opens a
PR. If the seam suite fails, the PR is labelled `core-breaking` and pinned to the previous tag until a human resolves it.

### 12.6 Live probes and the compatibility matrix

**Probes**, every 6 hours, read-only: finality lag below 10 minutes, as upstream's smoke tests assert
(`UP/ts-tests/suites/smoke/test-block-finalization.ts:16-29`); `spec_version`; the `:code` hash; the recomputed V15 digest
against the pin; metagraph decodes for netuid 1 and the largest netuid, through both the typed path (shape-hash OK?) and
the view. Results publish as a status badge.

**`compat.json` / `COMPAT.md`** is generated by `xtask compat-matrix` at release time. The row below is illustrative:

| rtao | bittensor-core tag (commit) | spec range validated | metadata pins | localnet images (digest) | Python interop tested | btauth |
|---|---|---|---|---|---|---|
| 0.6.0 | v477 (5c6e83e2) | 475–478 | finney 477, testnet 477, devnet 478 | `v477@sha256:…`, `testnet@…`, `devnet@…` | bittensor 11.3.0 / 11.3.1rc68 | btauth/1 |

"Validated" means no *breaking* class in the drift diffs across the range, plus the e2e lanes passing on those image
digests (H §3.9).

**Runtime guard.** On connect, the client compares the chain's `(spec, metadata hash)` with `SPEC_RANGE`. Outside the
range, it logs a warning. Typed reads keep working wherever shape hashes match, and fall back to dynamic decoding
elsewhere. Signing refuses calls whose shape hash mismatches. This is the type-driven analogue of Python's
spec-mismatch refusal (`PY/_transport/interface.py:617-623`).

### 12.7 Release process

- Versions: one workspace version, `rtao-vX.Y.Z` tags, release PRs generated by `release-plz`, and a changelog with
  `cargo-semver-checks` labels.
- Distribution: the git tag itself, because crates.io is not a goal. `cargo-dist` builds release binaries for `rtao`
  (the CLI) for linux x86_64/aarch64 and macOS arm64/x86_64, with checksums and GitHub artifact attestations.
- Major-version policy: a breaking drift bumps the minor version while the project is 0.x and the major version from
  1.0. Additive drift is a patch.

### 12.8 Maintenance playbook

| Upstream event | Automated | Human work |
|---|---|---|
| New runtime, surface unchanged (the common case) | drift PR updates `index.toml` and `SPEC_RANGE` and auto-merges after CI | none |
| New runtime, additive | drift PR with regenerated `rtao-chain`; coverage gate lists new calls | decide: wrap as intent, typed read, or `raw_only.toml` (≈ ½ day) |
| New runtime, behavioural (event index move, storage type change, shape hash change on a typed descriptor) | drift PR; property tests over the new metadata; typed paths fall back automatically on live nets meanwhile | update overrides or domain conversions (½–1 day) |
| New runtime, breaking (moved indices, wrapped-call args changed, extension payload added, `transaction_version` bump) | drift PR fails; live guard refuses signing on affected calls | fix encoders and intents (1–3 days); patch release; upstream coordination if an extension needs a custom encoder (bittensor-core's `PAYLOAD_FIELDS`) |
| New bittensor-core tag | core-bump PR with seam suite and `core-diff` | review seam changes; adopt fixes from §1.7 and delete rtao's workaround |
| Python adds an intent or read | catalog-drift PR; parity gate fails | implement (½–1 day per intent, 1–2 h per read) or allowlist |
| Python changes the error taxonomy | catalog-drift PR (`errors.json`) | regenerate `CHAIN_ERRORS` (minutes) |
| btauth/1 → btauth/2 | upstream-files issue | implement v2 side by side: verify both, sign the configured one, keep v1 for a deprecation window (½–1 day) |
| Python transport semantics change | upstream-files issue | port the change and add a `FakeChain` fault case |
| New localnet image or tag | probe + e2e lanes | bump the digest in `tests/localnet.toml` |
| Toolchain or dependency advisories | Dependabot/Renovate + `cargo audit` | bump; MSRV moves only when bittensor-core's toolchain does |

---

## 13. Documentation

### 13.1 rustdoc and doc tests with docify

- `#![deny(missing_docs)]` in every library crate. Each crate's `README.md` is included with
  `#![doc = include_str!("../README.md")]`. docs.rs-style metadata (`all-features`, `--cfg docsrs`) drives the CI
  rustdoc build, deployed to GitHub Pages because crates.io is not used.
- **Every example is a compiled test.** Examples live in `crates/*/examples/*.rs` and `tests/*.rs`, marked
  `#[docify::export]`, and are embedded into rustdoc and markdown with `docify::embed_run!` when they run offline (units,
  btauth, typed SCALE against `FakeChain`) or `docify::embed!` when they need a node.
- `docify::compile_markdown!` regenerates `README.md` files from `README.docify.md` sources, so prose never drifts from
  the code (R/F §7).
- The user maintains docify, so its 2025-02 release date is not a risk.
- Generated `rtao-chain` items carry metadata docs, with `generate_docs` on, and deprecation notes from V16.

### 13.2 The book (`book/`, mdbook 0.5)

Its shape mirrors bittensor.com's taxonomy (`CAT/bittensor.com_llms.txt:29-129`; R/I §5.6). An agent or human who knows
upstream's docs can navigate by analogy.

| Section | Content |
|---|---|
| `index`, `quickstart` | `cargo generate` the subnet template, connect, read the metagraph, set weights on localnet |
| `sdk` | the Rust primitives: `Client`/`View`/`Snapshot`, `Wallet`/`Signer`/`KeySource`, `Balance<U>`, intents, `ExtrinsicResult<O>` |
| `agents` | the eight steps of `UP/docs/agents.mdx` in Rust/CLI form: search, discover (`rtao tools`), execute by name, preview (`--dry-run`), bound (`Policy`), branch on `ErrorCode`, non-interactive rules, escape hatch |
| `cli` | generated command reference + conventions |
| `concepts/*` | short Rust-specific notes; chain concepts link to `https://www.bittensor.com/llms.mdx/docs/concepts/<slug>/content.md` instead of copying them |
| `guides/*` | validator loop, miner server with btauth, owner tooling, CRv3 and timelock, MEV shield, Ledger, local development (localnet image, fast blocks), Python interop |
| `migration/*` | three tables: v11 Python → rtao (intent class → struct, read → namespace method, `ExtrinsicResult` fields, env vars); v10 → rtao (Axon/Dendrite/Synapse → `MinerServer`/`SignedClient`/btauth); bittensor-core native SDK → rtao (`Client`/`Executor`/`IntentCall` → async equivalents) |
| `reference/tx/<op>.md`, `reference/query/<name>.md`, `reference/errors/{<code>,chain/<Name>}.md`, `reference/hyperparameters/<name>.md` | **generated** by `cargo xtask docs` from `INTENTS`, `READS`, `CHAIN_ERRORS` and `hyperparams.toml`; section layout copied from upstream's generated pages (R/I §5.2), with `## Rust` replacing `## Python` and `## On-chain implementation` linking to `https://www.bittensor.com/code/...#Lx-Ly` from the vendored `sources`/`hits` |
| `parity`, `compat` | generated `PARITY.md` and `COMPAT.md` |

### 13.3 Machine-readable catalogs and `llms.txt`

- **`catalog/{intents,reads,errors}.json`** reuse upstream's schemas verbatim (R/I §5.3), so tools written against
  bittensor.com work unchanged. Each entry adds a `rust` object:
  `{"type": "rtao::intents::SetWeights", "op_const": "SetWeights::OP", "role": "Hotkey", "output":
  "WeightsSubmitted", "docs": "<rustdoc url>"}` for intents, and `{"method": "client.epochs().epoch_status(netuid)",
  "returns": "EpochStatus"}` for reads. `hits` comes from `Read::TARGETS` and is exact (§4.5).
- **`llms.txt`** is generated by `cargo xtask docs --llms` with upstream's preamble conventions
  (`CAT/bittensor.com_llms.txt:3-27`):
  - how to search: fetch the index, `rg` `llms-full.txt`, and fetch single pages as raw markdown (rtao's own path
    scheme, `/llms/<path>.md`);
  - the catalog URLs;
  - a `rtao tools` / `rtao reads` / `rtao explain` cheat sheet;
  - the page tree with one-line descriptions;
  - a **mapping block** from each `tx/<op>` and `query/<name>` to the upstream raw page
    (`https://www.bittensor.com/llms.mdx/docs/tx/<op>/content.md`), so an agent reads upstream semantics beside the Rust
    signature;
  - a "chain source" footer pinned to the vendored `index.json.commit`.
- **`llms-full.txt`** concatenates every book page as `# <Title> (/<path>)` blocks, the upstream format (R/I §5.1).
- **Drift gate.** `cargo xtask docs --check` regenerates the reference, catalogs and llms files and diffs them in PR CI,
  as upstream's `generate.py --check` does (`UP/website/apps/bittensor-website/README.md:15-22`).

---

## 14. Milestones

Effort is in engineer-weeks for one engineer who knows Substrate, and is **[inferred]**. Each milestone ends in a demoable
state with machine-checkable exit criteria.

| # | Phase | Deliverables | Exit criteria | Effort |
|---|---|---|---|---|
| M0 | Skeleton and seam | workspace and crate shells; toolchain files; PR gates (§12.1); `rtao-core` pin and re-exports; seam contract suite; `xtask metadata record` for finney/testnet/devnet (V15+V16, `index.toml`) | CI green on 3 OSes; `metadata/finney/477.*` committed; golden `extrinsics[*].extrinsic_hex` reproduced through `rtao-core` | 1 |
| M1 | **First typed read against finney** | `rtao-transport` (session, reconnect, policy refusals, archive pool); `FakeChain` v0 with faults; `RuntimeCache` (V15 fetch, Python-compatible disk cache); `xtask codegen` v0 (SubtensorModule storage, SubnetInfoRuntimeApi, System); shape hash; typed==dynamic property tests; `rtao-units` core; `View`/`Snapshot`; `MetagraphView` | `cargo run --example tempo -- finney 1` prints `Tempo(1)` through a typed `StorageEntry`; netuid 1's metagraph decodes through the view with its shape hash verified live; Appendix A reproduced as criterion benches; transport fault suite green | 2–3 |
| M2 | Transactions for validators and miners | `CallBytes`/compose, extension guard, nonce leases, mortality, signing via bittensor-core, fees via `state_call`, watch and receipts (nested failures, fee), typed pool rejections, retries and reconcile; `rtao-errors`; `KeypairSigner`; `rtao-wallet` (keyfiles, full password chain); `rtao-tx` + `#[derive(Intent)]`; intents `set_weights` (plain + CRv3 with corrected epoch port and `DrandClient`), `commit_weights`, `reveal_weights`, `serve_axon`/`_tls`/`serve_prometheus`/`reset_axon`, `burned_register`, `pow_register`, `root_register`, `transfer`, `transfer_all`; localnet harness | On `:v477` (fast and 12 s modes): plain weights path and CRv3 path, with `TimelockedWeightsRevealed` observed; idempotent `serve_axon`; `burned_register`; 50 pipelined transfers with no nonce gap; Python-recorded `conform` vectors match exactly | 3–4 |
| M3 | Networking and framework MVP | `rtao-auth` (vectors), `rtao-http` (layer, extractor, `MetagraphPolicy`, `SignedClient`, redis store), `rtao-metagraph` (sync, diffs, persist), `rtao-neuron` (`ValidatorRunner`, `MinerServer`, `AxonPublisher`, `WeightSetter`, `NeuronConfig`), subnet template, CLI M3 scope; Python interop job v1 | A mixed-language subnet works both ways on localnet (Python v11 miner ↔ rtao validator; rtao miner ↔ Python validator); btauth matrix green; btcli-created wallets sign in rtao | 2–3 |
| M4 | Full intent parity | the remaining ~70 intents (staking with slippage and `All`, moves/swaps/transfers, baskets, root claims, collateral, locks, children/take, identity, coldkey swap, crowdloan/leasing, proxy incl. pure/announced, multisig + adapters, batch, typed hyperparameters, mechanisms, trims, symbol, stake burn, EVM-side intents, root intents with sudo + verify); MEV shield path; `register_subnet` completion; composition; parity checks 1/4/5; generated `tx`, `call`, `tools`, `explain` | `xtask parity --check` green for intents; every intent planned on localnet (mirrors upstream's 73 `intent_*`); a representative submit per pallet group; shielded `burned_register` resolves its inner receipt | 4–6 |
| M5 | Full read parity + automation | all 102 reads and 14 namespaces; pinning enforcement; `RecordingView` target checks; archive fallback; Python read-output parity; drift, catalog-drift, upstream-files, core-bump and probe jobs; `compat.json` | parity checks 2/3/6 green; the drift job has opened a correctly classified PR on a real spec change (e.g. devnet 478); first compat matrix published | 3–4 |
| M6 | CLI parity and docs | hand-written groups, btcli config/books compatibility, non-interactive rules; book, generated reference, catalogs, `llms.txt`/`llms-full.txt`; docs drift gate | rtao alone reproduces the "typical loop" of `UP/docs/guides/local-development.mdx:132-146` (create wallets, fund, register subnet, `start_call`, register neurons, show metagraph); `docs --check` green | 3–5 |
| M7 | Signers, EVM, compat | `LedgerSigner` (digest + proof), `VaultSigner`, `ExtensionSigner`, `RemoteSigner`; `rtao-evm`; timelock UX parity; `rtao-compat-synapse`, `rtao-compat-epistula`, `rtao-quic` | `PARITY.md`: every row implemented and tested, or listed in `divergences.toml`; Ledger proof vector plus one manual device test; prepared-extrinsic interop both ways | 3–5 |

The total is **21–35 engineer-weeks**. After M2 the work parallelizes: M4 (intents) runs alongside M5 (reads) and M6
(CLI and docs), because all three consume the same `rtao-tx`/`rtao-reads` contracts. Two engineers would reach full
parity in roughly 4–5 calendar months **[inferred]**.

---

## 15. Risks and open questions

### 15.1 Risks

| # | Risk | Likelihood / impact | Mitigation |
|---|---|---|---|
| 1 | **bittensor-core is an internal crate**: `publish = false`, version stuck at 0.1.0, API shaped by the Python binding (R/B §7, §9.2). A tag bump can rename or reshape items inside rtao's seam. | high / medium | Pin by tag; keep the seam to the items in §1.2; the seam contract suite plus `core-diff` on each bump; contribute the §1.7 PRs so the seam is designed rather than incidental. |
| 2 | **The mandatory downstream `[patch.crates-io] w3f-bls`**, and E0277 if the tag mismatches **[measured]**. Every subnet repository inherits this footgun. | certain until the upstream fix / medium | `cargo generate` template, `rtao doctor`, README; the one-line upstream manifest change (§1.6 item 6) removes it. |
| 3 | **C toolchain through `host`**: libsodium and vendored OpenSSL (Perl and a C compiler needed; `UP/.github/workflows/build-core-wheels.yml:93-97,111-114`), even for plaintext hotkey files. This complicates Docker images, musl and cross-compiles. | high / low–medium | Document the prerequisites; ship a builder image; upstream PR to un-gate the JSON keyfile codec (§1.6 item 5). |
| 4 | **Positional decoding mis-decodes if the shape-hash function has a bug.** | low / high | typed==dynamic property tests per committed metadata (§11.2); fuzzing; the hash includes field names and compact flags; unknown specs exercise the hash on first use and log; a `RTAO_FORCE_DYNAMIC=1` escape hatch for incident response. |
| 5 | **Owning the WS session** means about 1k lines of subtle concurrency (reconnect, resend, subscription loss). | medium / high | A port of `rpc.py` *with its tests*; `FakeChain` fault injection; one supervisor task, so there is no shared mutable socket state; a narrow `ReadMethod` type so submits cannot be resent by construction. |
| 6 | **The parity target moves.** 83/102 grows, the taxonomy is reclassified, and catalogs lag mainnet deploys (H §1.6). | certain / medium | catalog-drift and upstream-files jobs; a parity gate with explicit `divergences.toml` entries (each with an expiry milestone), so releases are not hostage to non-critical upstream additions. |
| 7 | **CRv3 reveal-round prediction drifts from the pallet.** The predicate is now corrected, but `MaxEpochsPerBlock` deferral is not modelled (C1 §2.3). A late round is **silently lost**; an early round is rejected with custom code 15. | medium / high for validators | The upstream-files watch on `run_coinbase.rs`; CRv3 e2e at 12 s blocks; the `WeightSetter` watches for `TimelockedWeightsRevealed` and alerts when a commit's reveal is not observed by `reveal_block + N`. |
| 8 | **Duplicate crypto stacks**: the fork's `sp-core 37` (bittensor-core), `sp-core 34` (btlightning, optional) and alloy/k256 (EVM, optional). | medium / low | Keep them behind features; `cargo deny` duplicate bans for the default graph. |
| 9 | **Public endpoint variability**: unsupported namespaces, `-32601`, rate limits (R/feasibility-subxt.md; G §4.1). | high / medium | Legacy namespace only; `rpc_methods` capability probing with fallbacks; per-endpoint CI probes; terminal policy errors with `retry_after`. |
| 10 | **Shield key rotation**: `NextKey` belongs to the N+2 author and expires at now+3 (R/A1 §6.5). Inner extrinsics can fail decryption or expire. | medium / medium | Inner-receipt resolution (§3.9); `ShieldFailed` outcomes typed; retry policy re-reads `NextKey`; localnet e2e. |
| 11 | **Secrets handling**: offloaded KDF threads, the keychain, btauth nonce-store memory. | low / high | `Zeroizing`/`secrecy` throughout; no `Debug` on secret types; a bounded `InMemoryNonceStore`; the Redis store for multi-process deployments. |
| 12 | **Hardware and browser signer parity** (Vault QR, extension bridge) costs a lot for little use in Rust deployments. | high / low | Scheduled last (M7); listed as `unsupported` in `divergences.toml` if they slip. |
| 13 | **Python behaviours that are arguably bugs**: unpinned gather reads, `None` checks that never fire on zero-account defaults, float projections. | certain / low | rtao deviates deliberately and documents each case (`kind = improved`); interop tests normalize those reads. |
| 14 | **Measured numbers come from one machine** (Apple M5 Max). | — | Re-baseline in Linux CI with criterion (§11.7). |

### 15.2 Open questions

1. Will RaoFoundation take the seam PRs (§1.7)? The most valuable are the one-line `w3f-bls` manifest change, the epoch
   predicate fix, `verify_exact`, and un-gating the keyfile JSON codec. Is there appetite for bittensor-core as a public
   Rust surface (R/B §11)?
2. Does crates.io `w3f-bls 0.1.3`, used by downstream builds once the manifest change lands, produce tlock ciphertexts
   identical to the vendored copy, so pallet-drand decrypts them? The vendored diff is described as a no-std-only edit
   (`UP/vendor/w3f-bls/Cargo.toml:1-6`). This needs a localnet decrypt test before relying on it.
3. Extrinsic v5: finney advertises `[4, 5]`, but bittensor-core (`CORE/codec/extrinsic.rs:267-272`) and Python sign v4.
   When will either move? rtao follows bittensor-core.
4. btauth verification default: Python-compatible (accepts `<Bytes>`-wrapped signatures, chosen here) or strict?
   Upstream says other implementations need not accept the wrapped form (`signed-requests.mdx:239-243`).
5. Will `CheckMetadataHash = Enabled` ever become mandatory? rtao supports it (digest path) but defaults to Disabled,
   like Python and subxt.
6. Is `Metagraph.total_stake` intentionally declared `TaoBalance` while carrying stake weight? rtao overrides it to
   `StakeWeightRao` (R/A2 §9 item 2). It needs runtime-team confirmation.
7. Can upstream expose `SelectiveMetagraphIndex` in metadata or a runtime API? rtao derives it from the struct field
   order (§3.5), which matches today but is an implicit contract.
8. Should rtao model `MaxEpochsPerBlock` deferral in reveal-round prediction, or keep upstream's +3-block margin?
9. Should rtao share Python's runtime disk cache directory by default? Writes are atomic on both sides, but two writers
   from two languages could race on eviction. This document assumes yes.
10. Where does the rtao repository live? This document assumes `github.com/sam0x17/rtao`.

---

## Appendix A: Measurements made for this document

**Environment.** Apple M5 Max (18 cores), macOS (Darwin 25.6), rustc/cargo 1.99.0, `--release`, single-threaded
decode loops, median of 300 iterations (60 for payloads over 200 KB), warm-up excluded.

**Data.** Fetched 2026-10-13 from `https://entrypoint-finney.opentensor.ai` at finalized head
`0x6b58953ad764b33b419d4a6c8b5432518e542a4e3e1af33cf3471fea474d196f` (spec 477, transaction version 1):

- V15 metadata via `state_call Metadata_metadata_at_version 0x0f000000`, 400,688 bytes after unwrapping the `Option`;
- `SubnetInfoRuntimeApi_get_metagraph` for netuids 1, 3 and 64: 49,859, 48,248 and 51,231 SCALE bytes, each RPC taking
  0.71–0.88 s.

The files are in `BENCH/data/`.

**The synthetic 2,500-UID payload** is netuid 1's typed decode with every per-UID column cyclically extended to 2,500 and
re-encoded: 408,250 bytes.

**Code** (`BENCH/src/main.rs`), one function per strategy:

- **A**: `bittensor_core::runtime::Runtime::decode_id(output_type_of(get_metagraph), Cursor)` → `Value`;
- **B**: `scale_value::scale::decode_as_type(.., &PortableRegistry)`;
- **C**: `#[derive(codec::Decode)]` mirror of `Option<Metagraph>` (`UP/pallets/subtensor/src/rpc_info/metagraph.rs:14-118`);
- **D**: `#[derive(scale_decode::DecodeAsType)]` mirror with an `AccountId([u8;32])` newtype;
- **D2**: D with `[u8;32]` keys and only 18 hot fields;
- **E**: a hand-written borrowed columnar parser (fixed-width columns borrowed, compact columns decoded once).

**Results** (`BENCH/bench-run-3.txt`). Run 2 (`BENCH/bench-run-2.txt`, without D2) agrees within 7% on every strategy
column; the ss58 timing varies by ±11%:

| Payload | A core `Value` | B scale-value | C codec typed | D DecodeAsType | D2 hot-only | E view | ss58 of all keys |
|---|---|---|---|---|---|---|---|
| finney netuid 1 (49,859 B, 256 UIDs) | 329.8 µs | 910.8 µs | 29.8 µs | 1,380.2 µs | 775.5 µs | 6.5 µs | 158 µs |
| finney netuid 3 (48,248 B) | 326.3 µs | 876.3 µs | 29.6 µs | 1,363.5 µs | 765.8 µs | 6.6 µs | 152 µs |
| finney netuid 64 (51,231 B) | 354.5 µs | 921.8 µs | 32.8 µs | 1,430.9 µs | 752.2 µs | 6.4 µs | 143 µs |
| synthetic 2,500 UIDs (408,250 B) | 2,683 µs | 7,312 µs | 261 µs | 11,001 µs | 6,482 µs | 57.1 µs | 1,273 µs |

**Other measurements:**

- `Runtime::parse` of the live V15 blob: 0.98–1.04 ms on average over 20 runs.
- The live V15 view reports extrinsic version 4 and the 13 extension identifiers listed in §3.8 (`BENCH/bench-info.txt`).
- Builds:
  - bittensor-core without `host` plus the scale-* stack: 14.1 s wall / 93 s CPU (`BENCH/build.log`);
  - bittensor-core at `tag = "v477"` alone: 12.9 s / 84.6 s; this resolved `5c6e83e2` and printed Alice's address
    correctly (`…/design-tagpin/tagpin.log`);
  - without `[patch.crates-io] w3f-bls`: 4 × E0277 in bittensor-core (`…/design-bench-nopatch`).

**Reproduce.** `cd BENCH && cargo build --release && ./target/release/design-bench data`. Setting `ONLY_INFO=1` prints
only the metadata facts.

## Appendix B: Evidence index

- **Research.** The reports listed below. Sections read in full: A1, A2, B, D, G, H. Sections 0, recommendations,
  decisions and open questions read in C1, C2, E, F and I. Also `R/CORRECTIONS.md`, `R/feasibility-*.md`,
  `R/_call_index_map.txt`.
  - `R/A1-v11-intents-transport.md`
  - `R/A2-v11-reads-metagraph-codegen.md`
  - `R/B-bittensor-core-rust.md`
  - `R/C1-chain-extrinsics-hyperparams-tx.md`
  - `R/C2-chain-state-runtime-apis-shapes.md`
  - `R/D-neuron-protocols.md`
  - `R/E-existing-rust-efforts.md`
  - `R/F-rust-stack-options.md`
  - `R/G-owner-validator-miner-lifecycle.md`
  - `R/H-testing-and-upstream-tracking.md`
  - `R/I-legacy-sdk-and-docs-corpus.md`
- **bittensor-core, read directly:**
  - `CORE/lib.rs`, `error.rs`, `runtime/mod.rs` (130–230);
  - `codec/{extrinsic.rs, encode.rs (525–590), storage.rs (60–100), decode.rs (40–60, 290–335), value.rs (1–60), batch.rs (1–30), mod.rs (1–40)}`;
  - `keys/mod.rs` (76–101, 140–170, 380–470), `keyfiles/mod.rs` (1–180);
  - `timelock/{mod.rs (150–200), epoch_schedule.rs (10–145)}`, `digest/mod.rs` (30–110), `mlkem/mod.rs` (45–110),
    `pow/mod.rs` (1–60), `signers/ledger.rs` (80–160);
  - `client.rs` (500–660), `transaction.rs` (1–140, 820–1120), `UP/sdk/bittensor-core/Cargo.toml`.
- **Python, read directly:**
  - `PY/intents/{base.py (100–422), registry.py, plan.py, weights.py (50–270)}`, `PY/reads/base.py`;
  - `PY/error_map.py` (20–75, 480–545), `PY/result.py` (40–75, 690–750);
  - `PY/wallets.py` (55–120), `PY/keyfiles.py` (95–110), `PY/http_auth.py` (55–156), `PY/_transport/protocols.py`,
    `PY/signing.py` (1–100);
  - `PY/balance.py` (1–60), `PY/hyperparams.py` (85–160), `PY/metagraph.py` (125–240), `PY/namespaces.py` (270–294);
  - `PY/cli/tx.py` (1–90, 440–495), `PY/cli/query.py` (130–180), `PY/client.py` (method list), `PY/evm/__init__.py`;
  - `PY/sp_core.py` (5, 16) and `UP/sdk/bittensor-core-py/src/keys.rs` (188–193) for the btauth verify path;
  - `UP/sdk/python/codegen/check.py` (25–75, 300–360).
- **Chain:**
  - `UP/runtime/src/check_mortality.rs` (15–106), `UP/runtime/src/transaction_payment_wrapper.rs` (318–330),
    `UP/runtime/src/lib.rs` (1640–1668);
  - `UP/pallets/subtensor/src/coinbase/run_coinbase.rs` (1215–1240);
  - `UP/pallets/subtensor/src/rpc_info/metagraph.rs` (14–118, 119–240, enum at 488–650);
  - `UP/pallets/subtensor/src/lib.rs` (308–325, 398–480);
  - `UP/Cargo.toml` (workspace pins), `UP/rust-toolchain.toml`;
  - `UP/docs/guides/signed-requests.mdx` (225–250), `UP/sdk/bittensor-core-spec.md` (80–140).
- **Catalogs** (jq): `CAT/intents.json` (83 entries; signer/origin counts; 116 unique wrapped calls), `CAT/reads.json`
  (102 entries, 14 categories), `CAT/errors.json` (18 codes, 390 chain errors), `CAT/index.json` (commit `850f2755…`);
  `UP/sdk/bittensor-core/tests/e2e-manifest.json` (73 `intent_*` + 39 `test_*`).
- **Live**: `git ls-remote --tags https://github.com/RaoFoundation/subtensor.git` (v464–v477); the finney JSON-RPC calls
  in Appendix A.
