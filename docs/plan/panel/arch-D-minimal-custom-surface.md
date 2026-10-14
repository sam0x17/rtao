# rtao — Architecture proposal D: minimal rtao-owned logic, maximal upstream reuse

Design panel, architect D. Written 2026-10-13 against the RaoFoundation/subtensor monorepo at `850f2755` (spec 478 on
`main`; finney/testnet at 477) and the research reports in `scratchpad/research/`. Reader: an expert Rust/Substrate
engineer who knows FRAME and subtensor internals.

## Path conventions and evidence markers

| Prefix | Absolute location |
|---|---|
| `MONO/` | `SP/upstream/subtensor/` |
| `CORE/` | `MONO/sdk/bittensor-core/src/` |
| `COREPY/` | `MONO/sdk/bittensor-core-py/src/` |
| `PY/` | `MONO/sdk/python/bittensor/` |
| `PYT/` | `MONO/sdk/python/tests/` |
| `CAT/` | `MONO/website/apps/bittensor-website/public/catalog/` (byte-identical to the fetched `scratchpad/docs/{intents,reads,errors}.json`; checked with `cmp` in this session) |
| `RES/` | `SP/research/` |
| `META/` | `SP/meta/` |
| `REG/` | `REG/` (unpacked crates: subxt-rpcs 0.51.1, jsonrpsee 0.26.1, scale-decode 0.16.2, ...) |

Markers: **[V]** read on disk in this session (file:line given); **[R]** taken from a research report (report + section
given; the report cites the primary source); **[I]** my inference or estimate, not verified. `RES/CORRECTIONS.md` overrides
the reports: the 478 fee-extension identifier did not change on the wire, 478 changes no client-visible metadata, and the
"do not depend on bittensor-core" recommendations in reports B/E/F/G are superseded (their technical points still apply).

---

## 0. Thesis, ownership budget, and where this lens hurts

### 0.1 Thesis

Three kinds of upstream assets already exist, and rtao should consume all three instead of rewriting them:

1. **Chain-defined computation**, released in lock-step with every runtime: `bittensor-core` (keys, keyfiles, the
   metadata-driven SCALE engine, signature payload and v4 extrinsic assembly driven by the metadata extension list,
   multisig derivation, era math, ML-KEM shield sealing, RFC-0078 digest and Ledger proof, drand tlock, PoW seal, Ledger HID).
   Byte-exactness is pinned upstream by `PYT/fixtures/golden.json`, the shape corpus and the 112-test localnet e2e harness
   (RES/B §5, §8).
2. **Declared SDK surface**, published as data at the same git tag: `CAT/intents.json` (83 intents with signer, origin,
   verify read, wrapped calls, JSON Schema), `CAT/reads.json` (102 reads with params, categories, chain hits),
   `CAT/errors.json` (18 semantic codes, 390 classified chain error names); the generated descriptors in `PY/_generated/`;
   and literal tables in the Python source (`PY/error_map.py`, `PY/result.py`, `PY/hyperparams.py`, `PY/settings.py`,
   `PY/fee_filters.py`, `MONO/sdk/python/codegen/check.py`, `PYT/harness/samples.py`, `PYT/harness/fake_substrate.py`).
3. **Executable reference behavior**: the v11 Python SDK itself at the same tag, whose in-memory `FakeSubstrate`
   (`PYT/harness/fake_substrate.py:1-21`) composes and plans every intent with no chain.

rtao therefore owns only:

- **I/O**: an async tokio transport, caches, subscriptions and nonce management around bittensor-core's pure functions;
- **generators**: an `xtask` that turns (1)-(2) into Rust data, typed descriptors, intent/read scaffolding, CLI metadata,
  reference docs and `llms.txt`;
- **ported behavior**: intent `build`/`preflight` bodies, the executor flow, read projections, CLI command groups, neuron
  loops, proven against (3) running as a CI oracle;
- **a staging crate** (`rtao-core-ext`) holding code that belongs in bittensor-core, written as if already upstream, each
  module tied to a named upstream PR and deleted when that PR ships in a tag.

Everything hangs off **one pin**: a finalized monorepo runtime tag `v<spec>` recorded in `upstream.toml`. From that one tag
CI derives the `bittensor-core` git dependency and its `w3f-bls` patch, the catalogs, the goldens, the corpus, the btauth
vectors, the extracted Python and runtime tables, the Python oracle, the localnet image tag and the Rust toolchain. A pin bump
is one `cargo xtask pin --tag vNNN` and one PR.

### 0.2 Ownership budget

[I] Estimates, for scale only. The v11 Python package is 59,723 lines [V: `wc -l` over `PY/**/*.py`].

| Kind | Where | Est. Rust lines | Review burden |
|---|---|---|---|
| Reused as-is | bittensor-core (`CORE/` ~13.6k lines incl. tests, RES/A1 §1) | 0 | upstream's |
| Staged for upstream | `rtao-core-ext` | ~1.2k | must match upstream style; deleted on merge |
| Generated | `rtao-catalog`, generated parts of `rtao-tx`/`rtao-reads`, docs | ~10k | diff review only |
| I/O and runtime | `rtao-chain`, `rtao-signer`, `rtao-btauth` | ~9k | genuinely rtao's |
| Ported behavior | `rtao-tx` bodies, `rtao-reads` bodies, `rtao-cli` groups | ~25k | oracle-checked against Python |
| Framework | `rtao-neuron` | ~3k | rtao's (v11 Python ships no networking or neuron framework: `MONO/docs/migration.mdx:455-480`, RES/G §6.1) |
| Tooling | `xtask`, `tools/`, `rtao-testkit` | ~6k | rtao's |

The lens removes chain-defined code and constant tables from rtao's ownership almost entirely. It cannot remove the product
behavior layer: bittensor-core's own executor covers 22 of 83 intents, is blocking, and diverges from Python on era, shield
and receipts (RES/A1 §9). Upstream's spec rules out an intent layer in the core (`MONO/sdk/bittensor-core-spec.md:85-97`),
so rtao has to port that layer from Python.

### 0.3 Where this lens hurts, and the mitigation for each

| Cost of reuse | Evidence | Mitigation in this design |
|---|---|---|
| bittensor-core is synchronous: Ledger HID blocks until the user approves, keyfile unlock runs Argon2i at 512 MiB / t=8, PoW mines on threads, drand is fetched with blocking reqwest | `CORE/signers/ledger.rs:118-156` [V]; `CORE/keyfiles/mod.rs:59-72` [R B §3.3]; `CORE/timelock/mod.rs:242-315` [R B §3.4] | `spawn_blocking` adapters live only in `rtao-signer` and `rtao-tx::pow`. Drand is fetched with rtao's async reqwest client and finished with the core's pure `decrypt_with_signature` (`CORE/timelock/mod.rs:319`) |
| Blocking HTTP `Client`: no subscriptions, polling block streams, no nonce cache, no fallback pool | `CORE/client.rs:1-8,150-179,338-355,897-991` [V/R B §4.1] | Not used on production paths. rtao drives the core's *pure* functions from its own async transport. The blocking client is kept as a differential test oracle and re-exported unchanged for users who want blocking Rust (§1.5) |
| Dynamic `Value` codec in cyscale shapes: `AccountId32` decodes to an ss58 `String` and enums to dicts | `CORE/codec/value.rs:1-33`, `CORE/codec/decode.rs:1-8` [V/R] | `Value` stays on the dynamic and JSON surfaces, where its Python shapes give CLI `--json` parity for free. Typed APIs decode with `scale-decode` against the same `PortableRegistry` (§1.6), and typed call builders are generated from metadata idents (§3.6) |
| Catalogs carry no return types and no parameter defaults (0 of the schema properties in `CAT/intents.json` have `default`) | `jq` over `CAT/intents.json` [V] | Defaults and class flags are extracted from Python source until an upstream catalog PR adds them (PR-U12). Read return models are hand-written and checked against the metadata registry (§8) |
| Build weight and C toolchain: `host` pulls libsodium, vendored OpenSSL (needs Perl), secp256k1-sys and the polkadot-sdk fork; 330 crates for the core alone, 392 with subxt | RES/feasibility-bittensor-core-gitdep.md; RES/feasibility-veneer-coexistence.md | Accept for v1, with a CI build image and caches. Upstream PR-U10 splits `host` into finer features. Using `subxt-rpcs` instead of full subxt avoids subxt's codegen machinery [I: fewer crates than 392] |
| No semver: `publish = false`, version never stamped, API shaped by the Python seam | `MONO/sdk/bittensor-core/Cargo.toml:1-7` [V]; RES/B §7 | Pin to immutable tags. A `core_surface` compile probe references every core symbol rtao uses, so a bump fails at the exact symbol. A nightly canary builds against `main` |
| Core behavior gaps (receipts, error taxonomy, shield era, epoch predicate) | §1.4 | Fixed in `rtao-core-ext` with upstream PRs; rtao never calls the affected core paths |
| Downstream workspaces must repeat `[patch.crates-io] w3f-bls` | Cargo ignores `[patch]` in dependencies (RES/feasibility-bittensor-core-gitdep.md) | Template repo and `rtao doctor`. M2 tests whether a std build without the patch produces ciphertexts pallet-drand decrypts (open question Q1) |
| Python runs in rtao's CI (oracle, table extraction, upstream recorders) | — | CI only, with outputs committed. Building rtao or any subnet binary never invokes Python. A pure-Rust extractor is a drop-in option (§11.8) |

---
## 1. Boundary with bittensor-core

### 1.1 How the core is pinned and bumped

**The pin.** One workspace dependency plus the patch the core's own workspace needs (Cargo ignores `[patch]` sections
in dependencies; RES/feasibility-bittensor-core-gitdep.md):

```toml
# rtao/Cargo.toml (workspace root) — the tag below is written by `cargo xtask pin`, never by hand
[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false, features = ["host"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

- The tag is the **finalized mainnet runtime** `v<spec>`. Immutable lightweight `v<spec>` tags exist from v430 onwards
  (RES/H §1.2); `git tag` in the local clone lists `v466 … v477` [V]. Pre-releases that never finalized (v470, v471, v476:
  RES/H §1.2) are never valid pins. The bump job therefore checks the finalized finney runtime over RPC (the same check as
  `MONO/.github/scripts/mainnet-finalized-runtime.sh:19-26`, RES/H §2.1) instead of trusting that a tag exists.
- The feasibility builds used `rev = 850f2755` (spec 478 on `main`). Pinning to `v477` instead is the same mechanism; the
  first pin PR re-runs the build matrix because the core may differ between `v477` and `850f275` (the shallow clone cannot
  show this) [I].
- `upstream.toml` is the machine-readable record of the pin, read by xtask and CI:

```toml
[monorepo]
url = "https://github.com/RaoFoundation/subtensor.git"
tag = "v477"
commit = "…40 hex, resolved by xtask pin…"
spec_version = 477
toolchain = "1.89"                       # copied from MONO/rust-toolchain.toml at the tag [V: channel = "1.89"]

[localnet]
image = "ghcr.io/raofoundation/subtensor-localnet"
tag = "v477"                             # per-spec images exist for v432, v473, v475, v477 (RES/H §1.3)
digest = "sha256:…"

[networks.finney]
genesis = "0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03"   # PY/settings.py:45 (RES/A1 §7)
metadata_v15 = "vendor/metadata/finney/477.v15.scale"
metadata_v16 = "vendor/metadata/finney/477.v16.scale"
code_hash = "0x…"                        # blake2_256(:code) at the recorded block

[artifacts]                              # monorepo path -> vendored path + sha256 (verified on every CI run)
"sdk/python/tests/fixtures/golden.json"              = { to = "vendor/upstream/golden.json", sha256 = "…" }
"sdk/python/tests/fixtures/shape_corpus/corpus.json" = { to = "vendor/upstream/corpus.json", sha256 = "…" }
"sdk/bittensor-core/fixtures/ledger_proof_vector.json" = { to = "vendor/upstream/ledger_proof_vector.json", sha256 = "…" }
"website/apps/bittensor-website/public/catalog/intents.json" = { to = "vendor/upstream/catalog/intents.json", sha256 = "…" }
# … reads.json, errors.json, PY/_generated/*.py, PY/vault/static/vault.html, PY/extension/static/bridge.html, PY/evm/abi/*.json
```

**Bumping the pin** (`cargo xtask pin --tag vNNN`, run by the pin-bump bot in §12.2):
1. Check that `vNNN` is finalized on finney (RPC `state_getRuntimeVersion` at the finalized head, plus the `:code` hash).
2. Rewrite the tag in `Cargo.toml` (dependency and patch), run `cargo update -p bittensor-core -p w3f-bls`, and update
   `upstream.toml` (commit, spec, toolchain from `MONO/rust-toolchain.toml`, localnet digest).
3. Re-vendor every `[artifacts]` entry from a checkout at the tag, then re-run extraction (§4.1, §11.8) and
   `cargo xtask regen`.
4. Regenerate the Python oracle snapshots at the tag (§11.5).
5. Run all gates: the `core_surface` probe, unit, oracle, the localnet e2e on `:vNNN`, and interop. Then open the PR with a
   porting report: every file under the watched upstream paths that changed between the two tags, mapped to the rtao module
   that ports it (`parity/source_map.toml`).

### 1.2 Used as-is (no wrapper beyond type conversion)

Line numbers are definition sites in `CORE/` at `850f275` [V unless marked].

| Area | Items rtao calls directly | Class (RES/B §3.14) | rtao consumer |
|---|---|---|---|
| Metadata model | `runtime::Runtime::parse` (runtime/mod.rs:163, V14/V15 only, :193-198), pub fields `types: PortableRegistry`, `pallets`, `extrinsic`, `apis`, `metadata_bytes` (:137-157); `pallet` :230, `pallet_at` :234, `storage_entry` :238, `constant` :242, `resolve` :257, `type_ident` :272, `module_error` :340; `StorageInfo` :29, `PalletInfo` :51, `SignedExtensionInfo` :65, `ExtrinsicInfo` :74, `RuntimeApiMethodInfo` :89 | PRODUCT shape over CHAIN data | `rtao-chain` runtime cache; codegen IR |
| Encode | `Runtime::compose_call` codec/encode.rs:532 (pre-composed calls splice in as `Value::Bytes`); `encode_id` :176; `coerce_account_id` :491 (accepts ss58, 0x-hex or 32 raw bytes) | CHAIN bytes | every call, storage key and runtime-API argument rtao encodes |
| Decode | `decode_id` codec/decode.rs:296, `decode_call_value` :658, `batch::{decode_batch :55, decode_map_page :86, decode_map_changes :111}`, `value::Value` value.rs:12, `to_corpus_json` value.rs:58 | CHAIN bytes / PRODUCT shapes | dynamic reads, events, CLI JSON |
| Storage keys | `storage::{storage_prefix :60, hash_param :21, concat_hash_len :48}`, `Runtime::storage_key` :69, `decode_storage_key_params` :101 | CHAIN | `query`, `query_map`, `query_batch` |
| Extrinsics | `extrinsic::TxParams` :19; `signature_payload_parts` :217 and `signature_payload` :240 (metadata-ordered extensions, fail-closed on unknown non-empty extension :89-118); `encode_signed_extrinsic` :260 (v4 only, :268-273); `decode_extrinsic` :321; `era_birth` :407; `multisig_account_id` :419 | CHAIN (golden-pinned) | `rtao-chain::submit`, offline prepare/attach |
| Keys | `keys::Keypair` keys/mod.rs:151 (`from_uri` :264, `from_mnemonic` :211, `from_seed` :243, `from_private_key` :287, `generate_mnemonic` :322, `from_encrypted_json` :338, `sign` :391, `verify` :402); `public_key_from_ss58` :48; `ss58_from_public` :57; free `verify` :457 (also accepts the `<Bytes>` wrapper); `CRYPTO_ED25519/SR25519` :28-29; `DEFAULT_SS58_FORMAT` :31 | CHAIN + COMPAT | `rtao-types::AccountId`, `rtao-signer`, `rtao-btauth` |
| Keyfiles (`host`) | detection `keyfile_data_is_encrypted_*` keyfiles/mod.rs:29-47, `encrypt_keyfile_data` :86, `decrypt_keyfile_data` :153, `get_password_from_environment` :111, `serialized_keypair_to_keyfile_data` :179, `deserialize_keypair_from_keyfile_data` :287 (all the btwallet JSON quirks, RES/B §3.3) | COMPAT | `rtao-signer::wallet` |
| Timelock | `WeightsTlockPayload` timelock/mod.rs:53, `UserData` :62, `reveal_round` :87, `inner_ciphertext` :97, `encrypt_and_compress` :104, `decrypt_and_decompress` :143, `encrypt_at_round` :230, `decrypt_with_signature` :319; constants timelock/constants.rs:6-51 (`GENESIS_TIME`, `DRAND_PERIOD`, `SECURITY_BLOCK_OFFSET`, `COMMIT_INCLUSION_BLOCK_OFFSET`, `DRAND_ENDPOINTS`); `epoch_schedule::EpochScheduleState` epoch_schedule.rs:17 (data type only) | CHAIN | weights commit-reveal, timelock UX |
| MEV shield | `mlkem::seal(pk, plaintext, include_key_hash)` mlkem/mod.rs:53, `twox_128` :37 | CHAIN | `rtao-chain::shield` |
| RFC-0078 | `digest::ChainInfo` digest/mod.rs:34, `metadata_digest` :102, `generate_extrinsic_proof` :117 | CHAIN/COMPAT | `CheckMetadataHash` mode Enabled, Ledger |
| PoW (`host`) | `pow::seal` pow/mod.rs:15, `mine_cpu` :51, `PREFIX_LEN` :11; `pow::opencl::{gpu_devices :181, GpuMiner :501}` | CHAIN / PRODUCT | `rtao-tx::pow` |
| Ledger (`ledger`) | `signers::ledger::LedgerDevice::{open :61, app_version :71, address :87, sign :124}` (unhashed payload plus proof in; 65-byte MultiSignature out) | COMPAT | `rtao-signer::LedgerSigner` |
| Errors | `error::CoreError` error.rs:11 | PRODUCT | wrapped into rtao error enums |

### 1.3 Wrapped (thin adapters; behavior unchanged)

| Core item | Wrapper | Why |
|---|---|---|
| `Runtime` | `rtao_chain::RuntimeHandle(Arc<Runtime>)` cached per `(genesis, spec_version)` in memory and on disk | adds caching and V15 fetch; parsing stays in the core (3.9 ms on live finney metadata: RES/feasibility-veneer-coexistence.md) |
| `Keypair` | `rtao_signer::KeypairSigner` implementing `rtao_types::Signer` | the async signer seam (§6); signing runs inline (sign plus assemble ≈ 0.03 ms per the core spec's table, RES/A2 §7) |
| `LedgerDevice::sign` | `LedgerSigner` (one device mutex, `spawn_blocking`) | blocks on user approval |
| keyfile decrypt | `Wallet::unlock` (`spawn_blocking`, unlock once per process, `Zeroizing`) | Argon2i cost |
| `pow::mine_cpu` / `GpuMiner::mine` | `PowSolver` (dedicated threads, `CancellationToken`, refreshes the challenge block per batch as `PY/pow_registration.py` does, RES/B §3.7) | long-running CPU/GPU work |
| `Value` | `rtao_types::DynValue` newtype: `Display` via `to_corpus_json`, plus typed accessors | keeps core types out of rtao's typed public API |

### 1.4 What the core lacks, how rtao supplies it, and what goes upstream

| # | Gap | Evidence | rtao supplies (crate::module) | Upstream proposal |
|---|---|---|---|---|
| G1 | Async transport, subscriptions, reconnect, endpoint pool, nonce cache | `CORE/client.rs:1-8` ("small blocking JSON-RPC layer") [V]; spec non-goal `MONO/sdk/bittensor-core-spec.md:87-91` [V] | `rtao-chain::{transport, nonce, subs}` (§3) | none: upstream excluded this deliberately; rtao keeps it |
| G2 | Protocol logic is private inside the blocking client: `fetch_metadata` :1234, `decode_storage` :1352, runtime-API encode (`runtime_call` :469-499, `runtime_api_method` :1334), `find_extrinsic` :942, `decode_outcome` :993, `dispatch_error` :1365 | `CORE/client.rs` [V] | `rtao-core-ext::ops` (sans-IO copies) | **PR-U1**: `bittensor_core::client::ops`, pure functions the blocking client also calls; no behavior change |
| G3 | Receipt parity: the core checks only `Proxy.ProxyExecuted`, `Sudo.Sudid`, `Multisig.MultisigExecuted` (client.rs:1023-1046). Python also checks `Sudo.SudoAsDone`, `Utility.BatchInterrupted/ItemFailed` and the MevShield failure events, and falls back to `Treasury.Deposit`/`Balances.Deposit` for the fee | `PY/_transport/utils/receipt.py` `_NESTED_RESULT_EVENTS` [V]; RES/A1 §4.10 | `rtao-core-ext::ops::receipt` (Python semantics) | **PR-U2**: bring `decode_outcome` to parity (benefits the core's e2e) |
| G4 | Error taxonomy: the core's `semantic_error_code` is snake_case(name) plus aliases, not the 18 `ErrorCode`s | client.rs:1396-1411 [V]; `PY/error_map.py:23-41` [V] | generated `rtao_catalog::errors` from `CAT/errors.json` plus extracted tables (§4.6) | **PR-U3**: export the custom tx codes, invalid-tx needles, dispatch errors, per-code explanations and remediation overrides in `errors.json` (the generator is `MONO/website/.../scripts/generate.py:1227-1283`, RES/H §1.6) |
| G5 | Epoch predicate diverges from the pallet: core `blocks_since_last_step > MAX_TEMPO_U64` vs pallet `BlocksSinceLastStep > tempo` | `CORE/timelock/epoch_schedule.rs:54` [V]; `MONO/pallets/subtensor/src/coinbase/run_coinbase.rs` `should_run_epoch_with_tempo` :1232 [V] | `rtao-core-ext::epoch_schedule` (pallet-faithful copy, upstream vectors plus new divergent-case vectors) | **PR-U5**: one-line fix plus vectors. Until it lands, rtao's reveal rounds can differ from Python's in that edge case; this is a documented, deliberate divergence |
| G6 | `generate_commit_v2` reads wall-clock time internally and uses the G5 predicate | timelock/mod.rs:160-195 [V] | rtao builds `WeightsTlockPayload` and calls `encrypt_and_compress` itself with an injectable clock (§9.2) | folded into PR-U5 (take `now` as a parameter) |
| G7 | Blocking client signs shield carrier and inner extrinsic with a 64-block era; the runtime rejects `submit_encrypted` with era > 8 (`Stale`) | client.rs:32, :622-654 [V]; `MONO/runtime/src/check_mortality.rs:19,94-100` [V] | rtao never calls `Client::submit_shielded`; `rtao-chain::shield` uses 8 and scans for the inner receipt (§3.8) | **PR-U7**: use `MAX_SHIELD_ERA_PERIOD` and add the inner scan |
| G8 | `Policy::check` takes only `IntentCall`, whose trusted constructors are private (22 of them) | transaction.rs:55-90, :929-976 [V] | port of `PY/intents/plan.py:21-85` with Python wording, plus a differential test against `Policy::check` on the 22 shared constructors (§4.3) | **PR-U8** (low priority): public `PolicyInput { spend, netuids, affects_all_subnets, raw }` |
| G9 | Metadata IR exists only in the Python binding | `COREPY/runtime.rs:579-660` [V] | `rtao-core-ext::ir` (same IR plus storage key types, hashers, events and runtime-API signatures, as RES/A2 §5.7 recommends) | **PR-U9**: `Runtime::metadata_ir() -> MetadataIr` (serde) in the core, so the Python binding and rtao share one implementation |
| G10 | `host` bundles keyfile JSON parsing with libsodium, vendored OpenSSL, reqwest, rayon, ansible-vault and scrypt | `MONO/sdk/bittensor-core/Cargo.toml` `[features]` [V] | rtao enables `host` and accepts the C toolchain | **PR-U10**: split into `keyfiles` (pure JSON), `keyfiles-nacl`, `keyfiles-legacy`, `drand-http`, `rpc-client`, `parallel` |
| G11 | `save_password_to_environment` calls `std::env::set_var` (documented as not thread-safe) | keyfiles/mod.rs:126-138 [V/R B §10.4] | pure `encode_password_for_environment(name, pw) -> String` in `rtao-core-ext::keyfile_env` (the CLI prints `export …`) | **PR-U11**: pure encoder next to the existing reader |
| G12 | Signing path hard-codes `tip = 0` and `metadata_hash = None`; no signer abstraction | client.rs:507-549 [V] | rtao builds `TxParams` itself and uses `Signer` capabilities (§6.4) | none: product surface |
| G13 | Fee estimation uses the deprecated `payment_queryInfo` RPC. Python calls `TransactionPaymentApi_query_info` via `state_call`, with a zero signature and an immortal era | client.rs:551-576 [V]; RES/A1 §4.4 | `rtao-core-ext::ops::fee` builds the probe extrinsic | part of PR-U1 |
| G14 | 83 intents and 102 reads versus 22 constructors and ~9 typed read helpers; no plan/preflight/results model | RES/A1 §9; RES/B §4.3 | `rtao-tx`, `rtao-reads` (§4) | none: upstream excludes it (spec §3) |
| G15 | Extension list guard: the core fails closed only at signing time | extrinsic.rs:89-118 [V] | at connect, `rtao-chain` calls `signature_payload_parts` once with dummy params. If it fails, rtao refuses signing mode and says why | none: reuses the core's own guard |

`rtao-core-ext` rules: it depends only on `bittensor-core`, `serde` and `parity-scale-codec`. It has no tokio, follows the
monorepo lints (§2.5), and every module starts with `//! UPSTREAM-TARGET: <path> (PR-Ux)`. `cargo xtask staged --check`
fails once the pinned core exports the target symbol, which forces the staged copy to be deleted.

### 1.5 The blocking `Client`/`Executor`: decision

| Option | Assessment |
|---|---|
| A. Wrap `bittensor_core::Client` in `spawn_blocking` for everything | Viable for a low-rate validator MVP: about 1 RPC/s, one `get_metagraph` per tempo, receipts by per-block polling. It cannot reach parity: no subscriptions (`wait_for_epoch`, block streams), HTTP rewriting of `ws://` (client.rs:1300-1308), era 64, broken shield path (G7), no fallback pools or policy-refusal handling, no `State already discarded` archive fallback, the deprecated fee RPC, one blocking thread per in-flight call, and a known hazard when the internal reqwest runtime is dropped inside an async context (RES/B §6) |
| B. rtao async I/O that calls the core's pure functions and the sans-IO `ops` (staged → PR-U1) | **Chosen.** Every chain-defined byte still comes from the core. rtao owns only I/O and policy, and both are ported from the Python transport |
| C. Ask upstream for an async client in the core | Conflicts with `bittensor-core-spec.md:87-91`. Proposed only as an optional `tokio` feature after PR-U1 lands; rtao does not depend on it |

The blocking client keeps two roles:
1. **Differential oracle**: `rtao-testkit/tests/differential.rs` runs the same reads (`metagraph`, `neurons`,
   `subnet_hyperparameters`, `stake_rao`, `query`, `query_map`, `runtime_call`) through `bittensor_core::Client` and
   `rtao_chain::Client` against one localnet block hash and asserts equal values.
2. **Blocking facade**: feature `core-client` re-exports `bittensor_core::{Client, Executor, IntentCall, Policy, Plan}` as
   `rtao::blocking::*` without changes. rtao does not build its own sync facade.

### 1.6 What rtao does about the dynamic `Value` codec

- **Encoding goes through the core's encoder, always.** Typed rtao values convert to `bittensor_core::codec::Value` via a
  one-method trait (`IntoCoreValue`, §5.1): `NetUid(1)` becomes `Value::Int(1)`, `AccountId` becomes `Value::Bytes(32 bytes)`
  (accepted by `coerce_account_id`, encode.rs:491-509 [V]), and nested calls become `Value::Bytes(call)`. There is one
  encoder, pinned by the golden vectors. The cost is a small `Value` allocation per argument; the core's spec measures
  `compose_call` under 50 µs (`MONO/sdk/bittensor-core-spec.md:360-382`, RES/A2 §7).
- **Decoding has two paths over one registry:**
  1. *Dynamic*: `Runtime::decode_id` returns a `Value` in Python shapes (corpus-pinned). Used for `DynValue` reads, events,
     receipts, the CLI `--json` and agent tools. Because the shapes match Python's, rtao's JSON for dynamic data matches
     `btcli --json` without a projection layer.
  2. *Typed*: `scale_decode::DecodeAsType::decode_as_type(&mut bytes, type_id, &runtime.types)`. This works because
     `scale-type-resolver 0.2.0` implements `TypeResolver for PortableRegistry` (`REG/scale-type-resolver-0.2.0/src/portable_registry.rs:59` [V]),
     and the core's `Runtime.types` is a `scale_info::PortableRegistry` from the shared scale-info 2.11.6
     (RES/feasibility-veneer-coexistence.md). The derive decodes named composites **by field name and ignores extra input
     fields** (`REG/scale-decode-derive-0.16.2/src/lib.rs:222-236,356-371` [V]). Hand-written typed models therefore keep
     decoding when the runtime adds fields, and fail loudly (`CannotFindField`) only on removals or renames.
- **No subxt codegen and no subxt `OnlineClient`.** Using them would put a second metadata model, encoder and signer next to
  the core's. subxt has shipped 8 breaking minors in 23 months (RES/F §1.7), and static codegen emits 60,420 lines per spec
  (RES/F §1.3). rtao uses `subxt-rpcs` 0.51.1 only, as the JSON-RPC client. It depends on `codec`, `frame-metadata`,
  `jsonrpsee`, `futures`, `serde` and (optionally) `tokio`/`finito`, and does not pull subxt's codegen
  (`REG/subxt-rpcs-0.51.1/Cargo.toml` `[features]` and `[dependencies.*]` [V]).

---
## 2. Workspace and crate layout

### 2.1 Tree

```
rtao/
├─ Cargo.toml                 # workspace; [workspace.dependencies] bittensor-core (git tag) + [patch.crates-io] w3f-bls
├─ rust-toolchain.toml        # = upstream.toml toolchain (1.89 today)
├─ upstream.toml              # the single pin (§1.1)
├─ crates/
│  ├─ rtao/                   # facade: re-exports + features; the ONLY crate subnet owners depend on
│  ├─ rtao-core-ext/          # staged-for-upstream additions to bittensor-core (no tokio)
│  ├─ rtao-types/             # units, newtypes, fixed point, AccountId, Signer trait, IntoCoreValue
│  ├─ rtao-catalog/           # GENERATED data: intent/read/error specs, extracted tables, chain descriptors, call builders
│  ├─ rtao-chain/             # async client: transport, runtime cache, views, reads primitives, submission, shield, waits
│  ├─ rtao-signer/            # wallets/keyfiles/passwords + Keypair/Wallet/Ledger/Vault/Extension/Remote signers
│  ├─ rtao-tx/                # intents (generated structs + ported bodies), executor, plan/policy, results, errors
│  ├─ rtao-reads/             # reads (generated specs/dispatch/namespaces + ported bodies), typed models, metagraph
│  ├─ rtao-btauth/            # btauth/1 core + server (tower/axum) + client (reqwest) + legacy-v10 bridge
│  ├─ rtao-neuron/            # framework: runners, metagraph sync, epoch clock, weight setter, axon publisher, config
│  ├─ rtao-evm/               # EVM helpers using upstream ABIs (optional)
│  ├─ rtao-transport-quic/    # optional adapter over btlightning (no reimplementation)
│  ├─ rtao-cli/               # `rtao` binary: btcli-equivalent
│  └─ rtao-testkit/           # FakeChain (subxt-rpcs MockRpcClient), localnet harness, fixtures, oracle loaders
├─ xtask/                     # pin, import-upstream, extract, regen, check-models, drift, parity, oracle, record, staged, lint-upstream, …
├─ tools/                     # extract_py.py (stdlib ast only), oracle/dump_intents.py, oracle/dump_reads.py
├─ vendor/                    # pinned upstream artifacts (+ MANIFEST with sha256), metadata blobs, extracted JSON, oracle JSON
├─ parity/                    # matrix.json (generated), pending.toml, exclusions.toml, source_map.toml
├─ book/                      # mdbook; reference/ generated
└─ ci/                        # localnet + interop docker compose, build image
```

### 2.2 Crates

| Crate | Purpose | Public surface (main items) | Features | Depends on |
|---|---|---|---|---|
| `rtao` | facade and the single dependency for downstream code; keeps a future rename to `bittensor-rs` a one-line change | `prelude`, re-exports `chain`, `tx`, `reads`, `types`, `signer`, `btauth`, `neuron`, `evm`, `blocking` | `default = ["chain","tx","reads","btauth"]`; `neuron`, `btauth-server`, `btauth-client`, `btauth-redis`, `btauth-legacy-v10`, `ledger`, `vault`, `extension`, `evm`, `quic`, `metrics`, `core-client` | all below (optional per feature) |
| `rtao-core-ext` | staged upstream code (§1.4) | `ops::{metadata, storage, runtime_api, receipt, dispatch_error, fee}`, `ir::MetadataIr`, `epoch_schedule::*`, `keyfile_env::*`, `shield::MAX_SHIELD_ERA_PERIOD` | none | `bittensor-core`, `serde`, `parity-scale-codec` |
| `rtao-types` | the unit and identity vocabulary | `Tao`, `Alpha`, `Balance`, `Amount<T>`, `Spend`, `NetUid`, `MechId`, `NetUidStorageIndex`, `Uid`, `BlockNumber`, `AccountId`, `PerU16`, `Permill`, `U64F64`, `I96F32`, `I64F64`, `I32F32`, `U64Normalized`, `CryptoType`, `Signer` trait, `SigningRequest`, `MultiSignature`, `IntoCoreValue`, `DynValue` | `serde` (default) | `bittensor-core` (keys, codec::Value), `scale-decode`, `scale-encode`, `serde` |
| `rtao-catalog` | generated, data-only | `INTENTS: [IntentSpec; 83]`, `READS: [ReadSpec; 102]`, `ErrorCode`, `CHAIN_ERRORS`, `MODULE_ERRORS`, `py::*` tables, `rt::*` tables, `chain::{storage,calls,constants,events,runtime_apis}` descriptors, `calls::*` typed builders | none | `rtao-types`, `bittensor-core` |
| `rtao-chain` | async chain access | `Client`, `ClientConfig`, `Network`, `View`/`ViewExt`, `Head`, `Snapshot`, `StorageItem`/`RuntimeApiMethod`/`ConstantItem` access, `CallBytes`, `SubmitOptions`, `ExtrinsicReport`, `UnsignedExtrinsic`, `RpcSession`, `blocks()`, `wait_for_*` | `metrics` | `rtao-core-ext`, `rtao-types`, `rtao-catalog`, `subxt-rpcs` (jsonrpsee, native, reconnecting-rpc-client), `jsonrpsee` 0.26, `tokio`, `tracing`, `lru` |
| `rtao-signer` | key custody and signing | `Wallet`, `WalletLike`, `PasswordSource`, `resolve_wallet_password`, `KeypairSigner`, `WalletSigner`, `LedgerSigner`, `VaultSigner`, `ExtensionSigner`, `RemoteSigner` | `ledger` (core `ledger`), `vault`, `extension`, `keychain` (macOS) | `rtao-types`, `bittensor-core` (`host`), `tokio` |
| `rtao-tx` | intent and executor layer | `Intent`, `IntentSpec`, `AnyIntent`, 83 intent structs, `Executor`, `Plan`, `Policy`, `IntentPreflight`, `ExtrinsicResult`, `ChainError`, `PolicyError`, `TxError`, `pow::PowSolver`, `timelock` UX | `pow-gpu` (exposes core OpenCL miner) | `rtao-chain`, `rtao-signer`, `rtao-catalog`, `rtao-core-ext` |
| `rtao-reads` | the read layer | 102 read fns, `ReadName`, `dispatch()`, namespaces (`balances()`, `subnets()`, …, 14 + chain), models (`Metagraph`, `MetagraphNeuron`, `NeuronInfoLite`, `DynamicInfo`, `StakeInfo`, `DelegateInfo`, `SubnetHyperparameters`, …) | none | `rtao-chain`, `rtao-catalog`, `rtao-types` |
| `rtao-btauth` | btauth/1 | `build_payload`, `sign`, `verify`, `Caller`, `AuthError`, `NonceStore`, `InMemoryNonceStore`; `server::{BtAuthLayer, VerifiedCaller, MetagraphPolicy}`; `client::SignedRequest`; `legacy::verify_v10` | `server` (axum 0.8, tower 0.5, http 1), `client` (reqwest 0.13), `redis`, `legacy-v10` | `rtao-types`, `bittensor-core` (keys::verify), `sha2`, `hex` |
| `rtao-neuron` | subnet framework | `ValidatorRunner`, `MinerRunner`, `OwnerTools`, `MetagraphSync`, `EpochClock`, `WeightSetter`, `AxonPublisher`, `NeuronConfig`, `StateStore` | `metrics` | `rtao-tx`, `rtao-reads`, `rtao-btauth`, `rtao-chain` |
| `rtao-evm` | v11 `PY/evm/` parity | address mirroring (`h160_to_ss58`), precompile ABIs embedded from `PY/evm/abi/*.json`, eth JSON-RPC helpers, EVM key association signing | — | `rtao-tx`, `alloy-primitives`, `alloy-json-abi`, `alloy-sol-types` [I: versions chosen at M6] |
| `rtao-transport-quic` | QUIC transport for SN2-style subnets | `QuicClient`/`QuicServer` adapters with metagraph policy | — | `btlightning` 0.2.x without its `subtensor` feature (that feature pulls subxt 0.50: RES/D §4.1) |
| `rtao-cli` | `rtao` binary | command tree (§10) | `ledger`, `vault`, `extension`, `evm` | everything |
| `rtao-testkit` | test infrastructure | `FakeChain`, `Localnet`, `fixtures::{golden, corpus, btauth}`, `oracle::{intents, reads}` | `docker` | `subxt-rpcs` (`mock-rpc-client`), `tokio` |
| `xtask` | maintenance tooling, not shipped | `pin`, `import-upstream`, `extract`, `regen`, `check-models`, `drift`, `parity`, `oracle`, `record`, `staged`, `lint-upstream`, `core-surface`, `localnet-image`, `porting-report`, `compat` | — | `bittensor-core`, `frame-metadata`, `syn`, `subtensor-linting` (git, same tag) |

### 2.3 Dependency direction

Layers; every edge points to a lower layer, and there are no cycles:

```
L0  bittensor-core (git tag)  ·  subxt-rpcs + jsonrpsee  ·  tokio  ·  scale-decode/scale-encode (shared stack)
L1  rtao-core-ext (→ core)                      rtao-types (→ core keys/Value, scale-*)
L2  rtao-catalog (→ types, core Value)          rtao-signer (→ types, core keyfiles/ledger/digest)    rtao-btauth (→ types, core keys::verify)
L3  rtao-chain (→ core-ext, types, catalog, subxt-rpcs)
L4  rtao-tx (→ chain, signer, catalog, core-ext)                rtao-reads (→ chain, catalog, types)
L5  rtao-neuron (→ tx, reads, btauth)   rtao-evm (→ tx)   rtao-transport-quic (→ reads, btlightning)
L6  rtao (facade; every L1–L5 crate optional by feature)   rtao-cli (→ rtao)   rtao-testkit, xtask (dev only)
```

Rules, enforced by `cargo deny` bans plus one `xtask` check:
1. Only `rtao-core-ext`, `rtao-types`, `rtao-catalog` (call builders produce core `Value`s), `rtao-signer`,
   `rtao-chain`, `rtao-tx`, `rtao-btauth`, the facade (only for the `blocking` re-export), `xtask` and `rtao-testkit` may
   name `bittensor_core::*`. Every other crate goes through rtao types.
2. Only `rtao-chain` (and `rtao-testkit`) may name `subxt_rpcs`/`jsonrpsee`. A transport library change touches one crate.
3. `rtao-catalog` has no logic beyond `const fn` constructors and lookups. Its sources are 100% generated.
4. `rtao-btauth` without `server`/`client` features has no I/O and no tokio.

### 2.4 Optional versus core

Core (default features): `rtao-types`, `rtao-catalog`, `rtao-chain`, `rtao-signer` (wallet/keypair), `rtao-tx`,
`rtao-reads`, `rtao-btauth` (pure). Optional: the framework (`neuron`), btauth server and client adapters, the Redis nonce
store, the legacy v10 bridge, the Ledger/Vault/Extension signers, macOS keychain, EVM, QUIC, metrics and the blocking
re-export. MEV shielding is not optional: it uses only `mlkem::seal`, which the core always compiles.

### 2.5 Edition, MSRV, lints

- rtao crates use edition 2024. The core is edition 2021 (`MONO/sdk/bittensor-core/Cargo.toml:4` [V]), which is fine.
- `rust-version` equals the monorepo toolchain pin (1.89, `MONO/rust-toolchain.toml` [V]). That is at least subxt-rpcs'
  1.88 floor (RES/F §8), and it guarantees rtao compiles inside the monorepo unchanged. `xtask pin` updates it, and CI runs
  an MSRV job on exactly that toolchain.
- Clippy deny-set copied from the monorepo workspace: `arithmetic-side-effects`, `expect-used`, `indexing-slicing`,
  `unwrap-used` (`MONO/Cargo.toml` `[workspace.lints.clippy]` [V]). Money types therefore expose
  `checked_*`/`saturating_*` methods rather than panicking `Add` impls (§5).
- The monorepo also runs a syn-based lint pass over every `.rs` file: `RequireFreezeStruct` (structs deriving
  `Encode`/`Decode` need `#[freeze_struct]`), `ForbidAsPrimitiveConversion` (bans `as_*()`), `ForbidKeysRemoveCall`,
  `RequireExplicitPalletIndex`, and `ForbidSaturatingMath` in tests (`MONO/build.rs` and `MONO/support/linting/src/*` [V]).
  `cargo xtask lint-upstream` runs the same `subtensor-linting` crate (git dependency, same tag) over rtao's sources in CI.
  Consequence: rtao defines SCALE types with `DecodeAsType`/`EncodeAsType` rather than `codec::{Encode, Decode}`. Where a
  codec derive is unavoidable it uses `subtensor_macros::freeze_struct`, as the core does (`CORE/timelock/mod.rs:51,60` [V]).

### 2.6 Upstreaming path: rtao as `sdk/bittensor-rs`

Recommendation: plan for it, but do not depend on it. Propose it to RaoFoundation after M5 (intent and read parity), when
the oracle shows rtao is a faithful Rust twin of the Python package. The layering already matches what the move needs:

| In rtao today | In the monorepo |
|---|---|
| `bittensor-core` as git tag dependency + downstream `[patch]` | `path = "../bittensor-core"`; the root `[patch]` already exists (`MONO/Cargo.toml:334-335`, RES/B §2.2) |
| `rtao-core-ext` | merged into `sdk/bittensor-core` as PR-U1…U11 (the crate disappears) |
| vendored catalogs/goldens/tables at a tag | read from the same commit (`sdk/python/tests/fixtures`, `website/.../catalog`); `xtask import-upstream` becomes a no-op |
| `tools/extract_py.py` | replaced by importing the Python registries directly (as `website/.../generate.py` does), or dropped once PR-U3/U12 put everything in catalogs |
| rtao's localnet harness | joins `check-bittensor-e2e-tests.yml` with its own manifest, like the core's 112-test list (RES/B §5) |
| drift bot | redundant: metadata and code change in the same PR; the mainnet-clone drift gate already exists (RES/A2 §0.3) |
| crate names `rtao-*` | `bittensor-rs-*`; downstream changes one line (`rtao = { package = "bittensor-rs", … }`) because everything re-exports through the facade |
| MIT license | monorepo is Apache-2.0 (`MONO/LICENSE` [V]); **recommend dual MIT OR Apache-2.0 from day one** so the move needs no relicensing, and vendored Apache-2.0 fixtures sit cleanly next to rtao code (NOTICE file) |

Things that would block the move, and how rtao avoids them from day one: a toolchain newer than the monorepo's (MSRV
policy above); lint violations (lint-upstream gate); dependencies the monorepo's `cargo-audit` ignore list or `deny` config
would reject [I: rtao runs `cargo audit` with the same pinned `cargo-audit 0.22.2`, `MONO/.github/workflows/cargo-audit.yml:53-85`, RES/B §2.3];
long-running Python steps in the build (rtao has none).

### 2.7 Subnet owner manifests

Validator:

```toml
[package]
name = "sn-foo-validator"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"

[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao.git", tag = "rtao-v0.4.0", features = ["neuron", "btauth-client", "metrics"] }
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }
anyhow = "1"
serde = { version = "1", features = ["derive"] }

# Required: Cargo ignores [patch] in dependencies, so the final workspace repeats bittensor-core's patch.
# Use the same tag as rtao's pin (`rtao doctor` checks Cargo.lock and prints the expected line).
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Miner (serves btauth/1 over axum, never signs with the coldkey):

```toml
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao.git", tag = "rtao-v0.4.0", default-features = false, features = ["chain", "reads", "btauth-server", "neuron"] }
axum = "0.8"
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

`default-features = false` drops nothing chain-defined: the `bittensor-core` dependency is always built with `host`
(keyfiles) until PR-U10. rtao is not published to crates.io, so releases are git tags `rtao-vX.Y.Z` (§12.6). A
`cargo generate` template (`rtao-subnet-template`) ships both manifests, a validator and a miner skeleton, and the docker
compose used for localnet testing.

---
## 3. Chain client (`rtao-chain`)

`rtao-chain` is a port of the Python transport (`PY/_transport/*`, 3,687 lines; `PY/_substrate.py`, 851 lines; RES/A1 §1),
with every chain-defined step delegated to the core. The Python transport, not the core's blocking client, is the
behavioral reference: it is the one the parity target uses (RES/B §3.13: Python imports only the chain-defined surface
from the core).

### 3.1 Async model

- tokio multi-thread runtime. `Client` is `Clone + Send + Sync` (an `Arc` inside), and all I/O methods are `async`.
- Work kept off runtime threads: `Runtime::parse` (runs in `spawn_blocking`, once per spec), keyfile unlock, Ledger, PoW,
  and decoding of payloads above 256 KiB (`spawn_blocking` [I: threshold to be tuned by benchmark, §11.7]; a 2,500-UID
  metagraph is ~510 KB, RES/C2 §5.1).
- Everything else runs inline: sr25519/ed25519 signing, call composition, storage key hashing, and decoding of ordinary
  responses (the core decodes at 60-77 MB/s per `MONO/sdk/bittensor-core-spec.md:360-382`, RES/A2 §7).
- Cancellation: every waiting API takes a timeout. Python's watch has no overall deadline apart from the 60 s silence
  watchdog (RES/A1 §4.11), so rtao's default is also no deadline; the timeout is opt-in.

### 3.2 Transport

**Library choice.** `subxt-rpcs` 0.51.1 over `jsonrpsee` 0.26.1 WebSocket (soketto, rustls; RES/F §2). It provides typed
legacy methods (`state_get_storage`, `state_get_keys_paged`, `state_query_storage_at`, `state_call`,
`state_get_runtime_version`, `chain_get_block(_hash)`, `chain_get_header`, `chain_get_finalized_head`,
`chain_subscribe_{new,all,finalized}_heads`, `state_subscribe_runtime_version`, `system_account_next_index`,
`author_submit_extrinsic`, `author_submit_and_watch_extrinsic`), all in `REG/subxt-rpcs-0.51.1/src/methods/legacy.rs:34-388` [V].
It also exposes the `RpcClientT` trait (`src/client/rpc_client_t.rs:26` [V]), `RoundRobinRpcClient`, a
`ReconnectingRpcClient` (reconnects but "doesn't retain subscriptions and pending method calls",
`src/client/reconnecting_rpc_client/mod.rs:5-12` [V]) and `MockRpcClient` (`mock-rpc-client` feature).

**Backend selection.** Legacy JSON-RPC namespaces only (`state_*`, `chain_*`, `author_*`, `system_accountNextIndex`), which
is exactly what v11 uses (RES/C2 §7). rtao never builds subxt's `CombinedBackend` (it fails with `-32601` on the finney WS
endpoint: RES/feasibility-subxt.md) and has no chainHead mode. The custom `subnetInfo_*`/`neuronInfo_*`/`swap_*` RPCs are
not used (RES/C2 §0.1).

**`RpcSession`** (rtao-owned; implements `RpcClientT` so `subxt_rpcs::LegacyRpcMethods` runs on top of it). This is a port
of `PY/_transport/rpc.py` semantics (RES/A1 §6.1, §4.11):

```rust
pub struct RpcSession { inner: Arc<SessionInner> }
pub struct TransportPolicy {
    pub handshake_timeout: Duration,      // 10 s     (rpc.py:60)
    pub silence_timeout: Duration,        // 60 s     (rpc.py:323) -> jsonrpsee PingConfig inactive limit + request timeout
    pub max_retries: u32,                 // 5        (rpc.py:322) consecutive failures before rotating endpoint
    pub retry_forever: bool,              // false
    pub backoff: Backoff,                 // min(0.25 s * n, 5 s); full-pool cycles min(2 s * cycle, 30 s) (rpc.py:648, 676-678)
    pub max_in_flight: usize,             // rtao addition: semaphore, default 16 [I]
    pub subscription_buffer: usize,       // 2048, oldest dropped (rpc.py:70, 280-291)
}
impl RpcSession {
    pub async fn connect(endpoints: EndpointPool, policy: TransportPolicy) -> Result<Self, TransportError>;
    pub async fn request<R: DeserializeOwned>(&self, method: &str, params: RpcParams) -> Result<R, TransportError>;
    pub async fn subscribe<R: DeserializeOwned>(&self, sub: &str, params: RpcParams, unsub: &str)
        -> Result<Subscription<R>, TransportError>;
}
pub enum TransportError {
    PolicyRefusal(RpcPolicyError),   // HTTP 403/429 at handshake, JSON-RPC -32004/-32005, WS close 1008: terminal, never rotated
    FateUnknown { method: &'static str, extrinsic_hash: Option<H256> }, // author_submit* in flight during a drop
    StateDiscarded,                  // "State already discarded for <hash>" -> archive pool retry
    SubscriptionLost,                // pushed into open subscriptions on reconnect
    Exhausted { attempts: u32 },     // max_retries × len(urls) when !retry_forever
    Rpc(jsonrpsee::core::ClientError),
}
```

- **Pool.** `[url] + fallback_urls`, rotating on connect failure or after `max_retries` consecutive failures. The archive pool
  is opened lazily on the first `StateDiscarded`. Default pools come from extracted `PY/settings.py:56-77` (`FALLBACK_ENDPOINTS`,
  `ARCHIVE_ENDPOINTS`; RES/A1 §7). An empty list pins the endpoint, as in Python.
- **Reconnect.** On drop, unanswered *idempotent* requests are re-issued on the new connection before callers are released.
  `author_submitExtrinsic` and `author_submitAndWatchExtrinsic` are never re-sent: the caller gets `FateUnknown` (Python
  behavior, `rpc.py:62-66,686-711`), and the executor surfaces it unchanged. Open subscriptions receive `SubscriptionLost`.
  Head streams built on top (§3.11) resubscribe automatically.
- **Policy refusals.** JSON-RPC `-32004`/`-32005` carry `policy`/`reason`/`retry_after` in `data`, which rtao parses fully.
  WS close code 1008 is surfaced by jsonrpsee. **Known gap:** jsonrpsee 0.26 reports a rejected handshake only as
  `WsHandshakeError::Rejected { status_code }` without response headers (`REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:188-210,522-524` [V]),
  so `X-RateLimit-Policy`/`-Reason`/`Retry-After` (Python `rpc.py:92-124`) are unavailable at handshake. In v1, rtao
  reports the status code with `policy`/`reason`/`retry_after = None`. If parity requires the headers, a soketto-based
  `TransportSenderT`/`ReceiverT` that keeps them is ~200 lines [I].
- **Rate limits.** Public endpoints answer `-32004 "storage work rate limit exceeded"` (RES/A2 §7). rtao keeps Python's
  `query_map` page size of 100 (`PY/_transport/interface.py:304`, RES/A2 §7) and splits `state_queryStorageAt` into chunks of
  at most 256 keys at one pinned hash. Python sends all keys in one call; chunking at one hash gives identical results with
  smaller requests. Concurrency is bounded by `max_in_flight`.
- **Sanitization.** URLs have userinfo stripped and diagnostics are bounded (`rpc.py:136-187`). Non-loopback `ws://` is
  accepted as Python accepts it, but logged as a warning. `strict_tls` refuses it, the posture some Rust tools take
  (RES/E §4.1.10).

### 3.3 Runtime (metadata) cache

- **Fetch.** `state_call("Metadata_metadata_at_version", 0x0f000000)`, unwrap `Option<Vec<u8>>`, and fall back to
  `state_getMetadata` (V14) on error. This is the core's own algorithm (`CORE/client.rs:1234-1267` [V]), staged as
  `rtao-core-ext::ops::metadata::{V15_REQUEST, unwrap_metadata_at_version}`. V15 is required because the core rejects V16
  (`CORE/runtime/mod.rs:193-198` [V]) and RFC-0078 digests are computed over V15 (RES/H §1.1). `transactionVersion` missing
  from `state_getRuntimeVersion` is a hard error, as in `PY/_transport/runtime.py:42-54` [V].
- **Memory.** An LRU of 4 `Arc<Runtime>` keyed by `spec_version`, plus 512 `block_hash → spec_version` entries
  (`runtime.py:123-143`). A block's runtime is resolved via its **parent** hash (`runtime.py:178-207`). Concurrent fetches for
  one spec share a single future.
- **Head revalidation.** `state_subscribeRuntimeVersion` triggers a refetch on upgrade; if that subscription is unavailable,
  a 12 s TTL poll of `state_getRuntimeVersion` takes over (Python's only mechanism, `runtime.py:142-176`). Detection is the
  same or faster.
- **Disk cache shared with Python.** Same directory (`$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache/`), file
  name `md1-<genesis[..16]>-<spec>.bin`, magic `STMD`, header `<IB` (u32 LE transaction_version, u8 is_v15), raw
  `MetadataVersioned` bytes, atomic temp+rename, at most 8 files (`PY/_transport/runtime.py:35-121` [V]). A host running
  btcli and an rtao validator shares one cache, and the format is tested in both directions by the interop lane (§11.6).
- **Connect-time guards** (fail fast, with evidence lines in the error):
  1. the extension guard G15 (signing disabled if the extension list has an unknown non-empty extension);
  2. genesis hash equals the network table's (refuse to sign against a mismatched endpoint; RES/H §1.1);
  3. each descriptor rtao uses exists with the expected key arity and value ident (§3.5);
  4. `spec_version` is inside the validated range in `compat.json`. Outside it, rtao warns, keeps reads working
     (metadata-driven), and leaves signing on (the payload is metadata-driven and guarded by 1).

### 3.4 Views, pinning, and the read primitives

```rust
pub trait View: Send + Sync {
    fn at_hash(&self) -> Option<H256>;                                          // None = head
    fn pinned(&self) -> impl Future<Output = Result<Snapshot, ChainError>> + Send;   // Head -> Snapshot at current head; Snapshot -> self
    fn runtime(&self) -> impl Future<Output = Result<RuntimeHandle, ChainError>> + Send;
    fn storage(&self, key: &[u8]) -> impl Future<Output = Result<Option<Vec<u8>>, ChainError>> + Send;
    fn storage_batch(&self, keys: &[Vec<u8>]) -> impl Future<Output = Result<Vec<Option<Vec<u8>>>, ChainError>> + Send;
    fn keys_paged(&self, prefix: &[u8], count: u32, start: Option<&[u8]>) -> impl Future<Output = Result<Vec<Vec<u8>>, ChainError>> + Send;
    fn state_call(&self, method: &str, args: &[u8]) -> impl Future<Output = Result<Vec<u8>, ChainError>> + Send;
}
pub trait ViewExt: View { /* provided methods, all generic over View */
    async fn query<K: IntoCoreValues, V: FromStorage>(&self, item: &StorageItem<K, V>, keys: K) -> Result<V, ChainError>;
    async fn query_dyn(&self, pallet: &str, item: &str, keys: &[Value]) -> Result<DynValue, ChainError>;
    async fn query_as<T: DecodeAsType>(&self, pallet: &str, item: &str, keys: &[Value]) -> Result<Option<T>, ChainError>;
    fn query_map<'a, K, V>(&'a self, item: &'a StorageItem<K, V>, prefix: K::Prefix) -> impl Stream<Item = Result<(K, V), ChainError>> + 'a;
    async fn query_batch<K: IntoCoreValues + Clone, V: FromStorage>(&self, item: &StorageItem<K, V>, keys: &[K]) -> Result<Vec<V>, ChainError>;
    async fn call<A: IntoCoreValues, R: FromRuntimeApi>(&self, m: &RuntimeApiMethod<A, R>, args: A) -> Result<R, ChainError>;
    async fn call_dyn(&self, api: &str, method: &str, args: &[Value]) -> Result<DynValue, ChainError>;
    async fn constant<V: FromConstant>(&self, c: &ConstantItem<V>) -> Result<V, ChainError>;   // from the view's runtime, never an RPC
    async fn events(&self) -> Result<Vec<EventRecord>, ChainError>;
    async fn timestamp(&self) -> Result<SystemTime, ChainError>;
    async fn block_time(&self) -> Result<Duration, ChainError>;       // Aura.SlotDuration / 1000, never hard-coded (RES/A2 §2)
}
```

(`ViewExt` is shown with `async fn` for brevity. The real methods are `fn … -> impl Future<Output = …> + Send` (RPITIT,
stable since 1.75), so callers can `tokio::spawn` them. rtao has no `dyn View`: reads are generic, and the CLI and tools
dispatch through generated `match` tables (§4.5), so `async-trait` boxing is not needed.)

- **Storage.** The key is built with `Runtime::storage_key(entry, &values)` (core). Miss semantics come from `StorageInfo`
  (`Default` decodes `default_bytes`; `Optional` gives `None`), exactly as `PY/_transport/storage.py:8-27` does. Typed
  values decode via `FromStorage`: primitives and newtypes through scale-decode, composites through `DecodeAsType` models,
  and `DynValue` through `decode_id`.
- **Map iteration.** `state_getKeysPaged` + `state_queryStorageAt` per page, all at one hash (Python pins `query_map` to one
  `chain_getHead`, `interface.py:347-351`). Free key components are recovered by `Runtime::decode_storage_key_params`
  (reversible hashers only, `CORE/codec/storage.rs:101-164` [V]). Large pages go to `batch::decode_map_page` (rayon above 64
  items, `CORE/codec/batch.rs:23-49`).
- **Runtime APIs.** `state_call("{Api}_{method}", args)` with arguments encoded from the V15 `RuntimeApiMethodInfo.inputs`
  type ids and the output decoded by `output` id (the staged `ops::runtime_api`, mirroring `CORE/client.rs:469-499` [V]).
  On spec 477 no API returns `Vec<u8>` double-encoded results (RES/C2 §0.2), so the legacy table is needed only for
  pre-V15 archive blocks. It is deferred (§15).
- **Events.** `System.Events` decodes through the core, whose `EventRecord` rendering (`{event_index, module_id, event_id,
  attributes}`, `CORE/codec/decode.rs:531-630,723-737` [V]) is what Python's receipt logic pattern-matches. rtao matches
  by `(pallet, event)` name, never by index: event indices are not stable across specs (SubtensorModule 149 changed meaning
  between 461 and 477: RES/H §2.2).

### 3.5 Generated chain descriptors and their validation

`cargo xtask regen` reads the pinned `vendor/metadata/finney/<spec>.v15.scale` through `bittensor_core::runtime::Runtime`
and the staged `ir::MetadataIr` (the IR from `COREPY/runtime.rs:579-660`, extended with storage key idents and hashers, event
field idents and runtime-API signatures). It emits `rtao-catalog/src/generated/chain/{storage,calls,constants,events,runtime_apis,errors}.rs`:

```rust
// @generated by `cargo xtask regen` — do not edit. (No spec_version in the header, so a spec bump with
// an unchanged surface produces no diff; same rule as PY/_generated: emit_python.py:22-30, RES/A2 §0.3.)
pub mod subtensor_module {
    /// Storage `SubtensorModule.Tempo` — `Map<Identity> NetUid -> u16`, Default.
    pub const TEMPO: StorageItem<(NetUid,), u16> = StorageItem::new("SubtensorModule", "Tempo", &["NetUid"], "u16");
    pub const KEYS: StorageItem<(NetUid, u16), AccountId> = StorageItem::new("SubtensorModule", "Keys", &["NetUid", "u16"], "AccountId32");
    pub const AXONS: StorageItem<(NetUid, AccountId), DynValue> = StorageItem::new("SubtensorModule", "Axons", &["NetUid", "AccountId32"], "AxonInfo");
}
pub mod subnet_info_runtime_api {
    pub const GET_METAGRAPH: RuntimeApiMethod<(NetUid,), DynValue> = RuntimeApiMethod::new("SubnetInfoRuntimeApi", "get_metagraph", &["NetUid"], "Option<Metagraph>");
}
```

- **Type mapping** uses the metadata's last-path-segment idents (`CORE/runtime/mod.rs:272` `type_ident` [V]):
  `u8…u128`, `bool` → Rust primitives; `NetUid`/`MechId`/`NetUidStorageIndex`/`PerU16`/`Percent`/`Permill`/`Perbill` → rtao
  newtypes; `TaoBalance` → `Tao`; `AlphaBalance` → `AlphaRao` (a bare amount; the netuid is attached by the read that knows
  it, §5); `AccountId32` → `AccountId`; `FixedU128`/`FixedI128` with fractional-bit paths → rtao fixed-point; anything else
  → `DynValue`. Typed composite access uses `query_as::<Model>` from `rtao-reads`, which keeps the generated crate free of
  model types and the dependency graph acyclic.
- **Validation at connect** (the CortexLM/buyback pattern, RES/E §4.1.1). A registry generated alongside the descriptors
  lists every descriptor rtao code actually uses. At connect rtao checks each against the live `Runtime`: existence, key
  arity, key idents and value ident. A mismatch is reported with the descriptor and the live shape, and the descriptor is
  marked degraded: typed access returns `ChainError::Shape`, while dynamic access still works. `strict` mode turns this into a
  connect error.
- **Drift relevance.** Because only names and idents are emitted, the regenerated diff for a runtime release is exactly the
  surface diff.

### 3.6 Generated typed call builders

From the same IR (`calls[].args[].{name, type_ident}`, which is what `PY/_generated/calls.py` contains, RES/A2 §5.3), xtask
emits one function per call of the covered pallets. Arguments are typed via the ident map above, so the unit enforcement
Python does at runtime (`PY/intents/_money.py:89-128` keys on `TaoBalance`/`AlphaBalance` idents, RES/A1 §2.2) becomes a
compile-time check:

```rust
pub mod subtensor_module {
    pub fn add_stake(hotkey: AccountId, netuid: NetUid, amount_staked: Tao) -> Call { … }
    pub fn remove_stake(hotkey: AccountId, netuid: NetUid, amount_unstaked: AlphaRao) -> Call { … }
    pub fn commit_timelocked_mechanism_weights(netuid: NetUid, mecid: MechId, commit: Vec<u8>, reveal_round: u64, commit_reveal_version: u16) -> Call { … }
}
pub struct Call { pub pallet: &'static str, pub function: &'static str, pub args: Vec<(&'static str, Value)> }
impl Call {
    /// Encodes through the core: `[pallet.index, variant.index] ++ fields` (encode.rs:532-585).
    pub fn encode(&self, rt: &Runtime) -> Result<CallBytes, CodecError>;
}
```

`CallBytes { data: Vec<u8>, spec_version: u32 }` with a cached `blake2_256` hash mirrors `PY/_transport/contract.py:17-33`
(RES/A1 §6.3). The 10 golden `calls` (transfers, `add_stake`, `remove_stake`, `burned_register`, `set_weights`,
`System.remark`, nested `Sudo.sudo`, `Utility.batch`, `Proxy.proxy`; RES/A2 §6.1) are re-encoded through these typed builders
as a unit test (§11.1), which pins rtao's argument conversions to upstream bytes.

### 3.7 Submission pipeline

`Client::submit(call, signer, SubmitOptions) -> Result<ExtrinsicReport, ChainError>` is a port of
`PY/_transport/interface.py:501-663` + `PY/_substrate.py:625-811` (RES/A1 §4.8-4.10):

1. **Nonce.** `NonceManager` (a `DashMap<AccountId, Mutex<Option<u64>>>`, Python `NonceCache` semantics,
   `PY/_transport/extrinsics.py:55-84`). The first use per account queries `system_accountNextIndex` (which includes
   pool-pending transactions); later uses increment locally. `reserve(n)` hands out consecutive nonces (the shield needs n
   and n+1). Any failure between signing and inclusion clears the entry (`PY/_substrate.py:753-762`). `prepare_call` always
   fetches a fresh nonce (`use_cache=False`, `interface.py:589-590`).
2. **Era.** Default period 128 (`PY/settings.py:22`; extracted), anchored at the **finalized** head (Python parity,
   `interface.py:485-499`; the core's client anchors at best). `TxParams.era = {"period": p, "current": finalized}` makes the
   core compute the phase (`CORE/codec/encode.rs:445-485`), and `era_block_hash = block_hash(era_birth(p, finalized))`
   (`CORE/codec/extrinsic.rs:407-412`). `period = None` means immortal, anchored at genesis.
3. **Metadata hash.** `None` (mode Disabled) unless the signer asks for it (`SignerCaps::METADATA_HASH`, Ledger). Then
   `metadata_digest(runtime.metadata_bytes, ChainInfo{spec_version, "node-subtensor", 42, 9, "TAO"})`
   (`CORE/digest/mod.rs:34-52,102-107`; constants RES/A1 §8.1.2) goes into `TxParams.metadata_hash`. Tip is always 0,
   because tips buy nothing on subtensor: flat priority (RES/C1 §0.5).
4. **Sign.** The core builds the parts (`signature_payload_parts`) and the payload (blake2-256 above 256 bytes). The signer
   receives a `SigningRequest` (§6.3) and returns a `MultiSignature`.
5. **Assemble.** `encode_signed_extrinsic(call, pubkey, sig, version, &params)` gives the bytes and the blake2-256 hash.
6. **Submit.** Without waiting, `author_submitExtrinsic` returns "Submitted (not waiting)" with `success = true`, which
   means *pool acceptance*, exactly the Python gotcha (RES/A1 §10.1). It is reproduced, not "fixed", and documented loudly.
   With waiting, `author_submitAndWatchExtrinsic`: `usurped`/`retracted`/`finalityTimeout`/`dropped`/`invalid` become
   failures named the way `PY/result.py:31-42` (`_WATCH_STATUS_RE`, extracted) classifies them, `inBlock` resolves when only
   inclusion is awaited, and `finalized` resolves otherwise. The subscription is always unsubscribed.
7. **Resolve** (staged `ops::receipt`, Python semantics): `chain_getBlock(hash)`, then locate the extrinsic by blake2-256 of
   each raw extrinsic. If it is absent: `ExtrinsicNotFound`, never a fabricated outcome (`interface.py:691-697`). Then
   `System.Events` at the block, filtered by `extrinsic_idx`, and the outcome:
   - fee = `TransactionPayment.TransactionFeePaid.actual_fee`, else the sum of `Treasury.Deposit`/`Balances.Deposit`
     (`receipt.py:75-96`);
   - success = `ExtrinsicSuccess` present and no failure;
   - failure = `ExtrinsicFailed` (module errors resolved through `Runtime::module_error`), a MevShield failure event, or a
     nested wrapper `Err` (`_NESTED_RESULT_EVENTS`: `Sudo.Sudid`, `Sudo.SudoAsDone`, `Proxy.ProxyExecuted`,
     `Multisig.MultisigExecuted`, `Utility.BatchInterrupted`, `Utility.ItemFailed`: `PY/_transport/utils/receipt.py` [V]).
8. **Result.** `ExtrinsicReport { success, message, block_hash, block_number, extrinsic_index, extrinsic_id: "{block}-{idx:04}",
   fee: Option<Tao>, events: Vec<EventRecord>, error: Option<ChainError> }`. The executor lifts it into `ExtrinsicResult` (§4.4).
9. **Transport failures during submit** follow `_failed_submit` (`PY/_substrate.py:753-772`): clear the nonce, raise
   `RpcPolicyError`/`RpcConnectionError` for policy or exhaustion, turn other pool errors into a failed result with the
   classified error. A pool rejection `InvalidTransaction::Custom(N)` maps through the generated custom-code table (§4.6).
10. **Spec drift between prepare and attach.** `submit_signature` refuses if `spec_version` **or** `transaction_version`
    changed since `prepare_call`. Python checks only spec_version (`interface.py:617-623`); RES/A1 §10.16 recommends both.
    This is a documented, strictly stronger check.

**Fee estimation** (staged `ops::fee`, Python semantics `interface.py:714-731`): nonce from `system_accountNextIndex`
without touching the cache, a zeroed 64-byte signature, an immortal era, and
`state_call("TransactionPaymentApi_query_info", xt ‖ u32_le(len))`. The output is decoded via the runtime-API output type,
reading `partial_fee` and `weight {ref_time, proof_size}` (the latter feeds multisig `max_weight`). Failures are
best-effort warnings in `plan` and hard `PolicyError`s in `submit_shielded` when `max_fee` is set, Python's documented
asymmetry (RES/A1 §10.4).

**Retries** (executor level, §4.3): only pool rejections whose text contains `priority is too low`,
`transaction is outdated` or `stale` (`PY/executor.py` `_TRANSIENT_SUBSTRINGS` [V]; extracted), with a one-block sleep
between attempts and a fresh nonce each attempt. Dispatch failures are never retried. A per-hotkey/per-mechanism
weight-commit `provides` tag means only one pending commit can sit in the pool (RES/C1 §2.2), and the weight setter (§9.2)
takes that into account.

### 3.8 MEV-shielded submission

`Client::submit_shielded(call, signer, ShieldOptions)` ports `PY/executor.py:1016-1214` (RES/A1 §6.5):
1. Read `MevShield.NextKey`, retrying twice one block apart.
2. Reserve nonces `n` and `n + 1` from the `NonceManager`.
3. Sign the inner call at `n + 1` with a **period of 8** (`rtao-core-ext::shield::MAX_SHIELD_ERA_PERIOD`, equal to
   `MONO/runtime/src/check_mortality.rs:19` [V]).
4. `mlkem::seal(next_key, inner_xt, include_key_hash = true)` (`CORE/mlkem/mod.rs:53` [V]). The ciphertext must stay at or
   below 8192 bytes (`_MAX_SHIELDED_CIPHERTEXT_BYTES`, extracted).
5. Compose `MevShield.submit_encrypted(ciphertext)`, sign it at `n` with period 8, and submit it, waiting for inclusion and
   not for finalization (Python defaults, `executor.py:905-907`).
6. Scan blocks from the carrier's inclusion to `included_at + period` for the inner hash. Each remaining block gets at most
   4 polls (`_SHIELDED_POLL_ATTEMPTS_PER_BLOCK`, extracted). When finalization is requested, the inclusion is re-resolved
   after finality and rescanned on reorg. If the inner extrinsic never lands, the result flips to failure with Python's message.
7. The carrier fee is preflighted by pricing `submit_encrypted` with an 8192-byte ciphertext, and the reserve plus carrier
   check runs when the intent reports `required_free` (`executor.py:955-979`). Shielded carriers cannot pay fees in alpha
   (RES/A1 §10.13), so this preflight matters for alpha-only coldkeys.

The executor routes `mev_shield_required` intents (`burned_register`, `add_collateral`) through this path automatically, as
Python's `execute` does (`executor.py:812-824`). `mev_shield_default` is honored only by the CLI, matching Python
(RES/A1 §10.12).

### 3.9 Proxy, batch, and multisig composition

- **Origin wrappers** (port of `nest_origin_wrappers`, `PY/executor.py:99-121`): compose the semantic call to bytes, then
  wrap in `Sudo.sudo(call)` when `origin == root` (never double-wrapping), then in
  `Proxy.proxy(real = proxy_for, force_proxy_type, call)`, then apply the intent's `wrap_call` adapter (saved multisigs).
  Each wrapper is a core `compose_call` with `Value::Bytes(inner)`, which the encoder splices verbatim
  (`CORE/codec/encode.rs:511-525` [V]). `proxy_type` is validated against the **metadata** `ProxyType` enum, cross-checked in
  CI against the extracted Python `PROXY_TYPES` (`PY/intents/proxy.py:21-41`, RES/A1 §3.10).
- **Batch**: `Utility.batch_all` with `Value::List(Value::Bytes(call)…)`. Nesting is refused client-side (the runtime filters
  nested batches anyway: RES/C1 §1.5). Spend, netuid and extra aggregation follow `PY/intents/batch.py:132-164` (RES/A1 §3.13).
- **Multisig**: the account and sorted signatories come from the core's `multisig_account_id` (`CORE/codec/extrinsic.rs:419-437`).
  `other_signatories` is that sorted set minus the signer. The timepoint is read from `Multisig.Multisigs[account, call_hash]`,
  and `max_weight` from the fee probe's `weight`. The opening round uses `as_multi` with the full call; later rounds use
  `approve_as_multi` with the hash. The funding preflight uses `DepositBase + threshold × DepositFactor` plus
  `_FEE_ALLOWANCE_RAO = 5_000_000` (`PY/multisig.py:121-162`, extracted). Exact prior-round call bytes are pinned for
  timelock-bearing calls (`PY/intents/multisig.py:293-299`, RES/A1 §2.4).

### 3.10 Snapshots and block pinning

`client.at(Some(n))` resolves `n → hash` once and returns a `Snapshot` (implements `View`); `client.at(None)` pins the
current head. Block-hash caches store only **finalized** mappings, gated by a finalized height re-probed every 12 s (Python
reorg safety, `interface.py:187-230`). `constant()` uses the view's runtime and is never an RPC. A read spec declares a
pinning class (§4.5). Composite reads always pin, which is a deliberate improvement over the four Python reads that gather
at head without pinning (RES/A2 §1.3). Results only differ when state changes between their RPCs.

### 3.11 Waiting helpers and block streams

- `client.blocks(Finality::{Best, Finalized}) -> impl Stream<Item = Result<BlockHeader, ChainError>>` over
  `chain_subscribe{New,Finalized}Heads`, resubscribing after `SubscriptionLost`. With `fill_gaps(true)` it backfills headers
  missed across a reconnect (Python does not; off by default).
- `wait_for_block(n)`, `wait_for_timestamp(t)` (using `Timestamp.Now`), and `wait_for_epoch(netuid)`, which subscribes to
  heads and polls `LastEpochBlock[netuid]` each head until it advances, as `PY/client.py:418-444` does. All take
  `Option<Duration>`.
- `wait_for_registration(coldkey, hotkey)`: the `register_subnet` completion follow-up (`PY/executor.py:333-568`). It scans
  the receipt for `NetworkAdded` versus `NetworkRegistrationQueued`; on the queued path it rescans the inclusion block, every
  height up to the head, then follows heads, accepting only a `NetworkAdded` whose `SubnetOwner`/`SubnetOwnerHotkey` match
  and which is no longer in `NetworkRegistrationQueue`. Progress events are `Queued`, `Waiting` and `Registered`.

### 3.12 Python-transport parity checklist

Each row is a test in `rtao-testkit` (FakeChain failure injection, modeled on `PYT/harness/fake_node.py:1-15`).

| Python behavior (RES/A1 §4.11, §6) | rtao test |
|---|---|
| idempotent pending frames re-sent after reconnect; submits fail "fate unknown" | `transport_reconnect_resends_reads_not_submits` |
| subscriptions get an error on reconnect | `subscription_lost_on_reconnect` |
| endpoint rotation after 5 consecutive failures; give up after `5 × len(urls)` | `pool_rotation_and_exhaustion` |
| policy refusal is terminal and never rotated (-32004/-32005/1008/403/429) | `policy_refusal_is_terminal` |
| `State already discarded` goes to the archive pool | `state_discarded_uses_archive` |
| finalized-only hash cache | `block_hash_cache_finalized_only` |
| runtime upgrade picked up within about a block | `runtime_upgrade_switches_codec` (localnet `set_code`, §11.4) |
| nonce cache: increment locally, clear on failure, explicit pin | `nonce_manager_semantics` |
| mortal era anchored at finalized head | `era_anchor_finalized` |
| `Submitted (not waiting).` with `success = true` | `no_wait_reports_pool_acceptance` |

---
## 4. Intents and reads parity

### 4.1 Where each piece of the declared surface comes from

| Needed for an intent/read | Source at the pin | How rtao gets it |
|---|---|---|
| op name, summary, description, signer, origin, verify read, wrapped calls, JSON Schema (fields, JSON types, required, descriptions), docs URLs, CLI string, `python_class`, chain `sources` | `CAT/intents.json` (83 entries; keys verified with `jq` [V]) | vendored, then generated into `rtao_catalog::INTENTS` and the intent structs |
| field **defaults** (`slippage_protection=True`, `rate_tolerance=0.05`, `mechid=0`, `period`, …) | Python dataclass `field(default=…)`; **absent from the catalog** (0 properties with `default` [V]) | `tools/extract_py.py` (stdlib `ast`; literal defaults only) → `vendor/upstream/extracted/intent_fields.json`; **PR-U12** adds `default` to `input_schema` |
| `mev_shield_default`, `mev_shield_required`, `all_amount_fields` | class vars, `PY/intents/base.py:160-174` [V] | same extractor (`intent_classvars.json`); PR-U12 adds them to the catalog |
| spend class, touched netuids, preflight reads, build logic | method bodies (`spend()`, `build()`, `effects()`…) | **ported by hand** (§4.2) and checked by the oracle (§11.5) |
| read name, summary, description, params (JSON types), param docs, category, `python` namespace call, chain `hits` | `CAT/reads.json` (102; 81 single-hit, 10 zero-hit, 11 multi-hit [V via `jq`]) | generated `rtao_catalog::READS`, `ReadName`, dispatch, namespaces |
| read return shape, unit tagging | Python dataclasses and dicts in `PY/reads/*.py` | **hand-written models** (§8), checked against the metadata registry and the read oracle |
| 18 codes + remediation; 390 chain error names → code + description | `CAT/errors.json` [V] | generated |
| custom tx codes 0-27,255 → name; invalid-tx needles; dispatch errors; per-code explanations; per-name and substring help overrides; substring fallback; watch-status regex | `PY/error_map.py:488,524,555`; `PY/result.py:27-36,78,116,291,308,475` [V]; runtime `MONO/common/src/transaction_error.rs` [V] | extractor (Python tables) + `syn` extractor (the runtime enum's `From<…> for u8` arms); CI asserts the two agree (Python's own gate only checks `range(28) ∪ {255}`, `codegen/check.py:351`) |
| hyperparameter table (kind, doc, short, signed, bits, min, max) and owner-setter map | `PY/hyperparams.py:91-110` (`Hyperparam(...)` calls) [V]; `PY/intents/hyperparameters.py:31-66` [V] | extractor (handles `Name(literal args)` calls) |
| coldkey-pays-fee calls | `MONO/runtime/src/fee_filters.rs` (`call_filter_group!`) [V]; Python mirror `PY/fee_filters.py:12-32` [V] | `syn` extractor over the **runtime** source (authoritative); CI asserts the Python mirror is equal |
| `SelectiveMetagraphIndex` 0..=76 | `MONO/pallets/subtensor/src/rpc_info/metagraph.rs:488-650` (not in metadata: RES/A2 §0.5) | `syn` extractor |
| network table, era defaults, docs/explorer URL templates, fee headroom, transient-retry substrings, shield bounds, multisig allowance | `PY/settings.py`, `PY/executor.py:45-66` [V], `PY/intents/_money.py`, `PY/multisig.py` | extractor → `rtao_catalog::py::*` constants; rtao code **never hand-copies a Python constant** |
| covered pallets / raw-only calls | `MONO/sdk/python/codegen/check.py:58-72,74-300` [V] | extractor → coverage gate (§4.7) |
| test samples and fake defaults | `PYT/harness/samples.py` (`INTENT_SAMPLES`, `READ_SAMPLES`), `PYT/harness/fake_substrate.py` (`DEFAULT_STORAGE/CONSTANTS/RUNTIME`) [V] | extractor (dev keys resolved to ss58) → oracle inputs and FakeChain defaults |

The extractor fails loudly when a table is no longer a literal of the expected shape. That only happens when upstream
restructures code, and the failure tells the pin-bump PR exactly which table to re-point.

### 4.2 Intent representation

**Generated, per intent** (`rtao-tx/src/generated/intents.rs`, from the catalog plus extracted fields):

```rust
/// Stake TAO to a hotkey on a subnet … (catalog `description`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]                         // input_schema.additionalProperties = false
pub struct AddStake {
    pub hotkey_ss58: AccountId,                       // "*_ss58" → AccountId (serde: ss58 string; Wallet coercion at the executor)
    pub netuid: NetUid,                               // name rule: netuid / *_netuid → NetUid
    pub amount_tao: Amount<Tao>,                      // anyOf[number, decimal-string, "all"] + all_amount_fields → Amount<Tao>
    #[serde(default = "d::add_stake::slippage_protection")] pub slippage_protection: bool,   // default extracted: true
    #[serde(default = "d::add_stake::rate_tolerance")]      pub rate_tolerance: Ratio,       // default extracted: 0.05
}
impl IntentMeta for AddStake { const SPEC: &'static IntentSpec = &rtao_catalog::INTENTS[IntentId::AddStake as usize]; }
```

Field type rules come from `xtask/intent_types.toml`. The defaults are: `*_ss58` → `AccountId`; `*_ss58s` →
`Vec<AccountId>`; `netuid`, `origin_netuid`, `dest_netuid` → `NetUid`; `mechid` → `MechId`; `uids` → `Vec<u16>`; money
fields (the schema's `anyOf` with a decimal-string pattern) → `Amount<Tao>` or `Amount<AlphaFor<..>>` by name suffix
(`_tao`, `_alpha`, `_rao`); `number` → `Ratio` (an exact decimal); other `integer`s → the narrowest type of the wrapped call's
argument ident. Overrides are explicit entries. `IntentSpec` holds `op`, `signer: SignerRole`,
`origin: Origin::{Signed, SubnetOwner, Root}`, `verify`, `wraps: &[(&str, &str)]`, `mev_shield_default`,
`mev_shield_required`, `all_amount_fields`, `schema_json: &str` (verbatim from the catalog) and the docs URLs.

**The trait** (hand-implemented per intent):

```rust
pub trait Intent: IntentMeta + Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    fn summary(&self) -> String;
    fn build(&self, cx: &BuildCx<'_>) -> impl Future<Output = Result<Built, IntentError>> + Send;          // semantic call (pre-wrapper)
    fn preflight(&self, cx: &PreflightCx<'_>) -> impl Future<Output = Result<IntentPreflight, IntentError>> + Send {
        async { Ok(IntentPreflight::effects_only(vec![self.summary()])) }                                // Python default: base.py:204-226
    }
    fn spend(&self) -> Spend { Spend::None }                                        // base.py:300-309
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> { generated_touches(self) }   // default: fields named *netuid (base.py:311-318)
    fn affects_all_subnets(&self) -> bool { false }
    fn wrap_call(&self, _cx: &BuildCx<'_>, call: CallBytes) -> impl Future<Output = Result<CallBytes, IntentError>> + Send { async { Ok(call) } }
    fn origin_view<'w>(&self, wallet: &'w dyn WalletLike) -> OriginView<'w> { OriginView::Wallet(wallet) }
}
pub enum Built { Call(Call), WithExtras(Call, BTreeMap<String, serde_json::Value>) }   // BuiltCall (base.py:414-422)
pub struct BuildCx<'a> { pub view: &'a Snapshot, pub origin: OriginView<'a>, pub runtime: &'a RuntimeHandle, pub clock: &'a dyn Clock }
pub struct IntentPreflight { pub effects: Vec<String>, pub warnings: Vec<String>, pub blocks: Vec<String>,
    pub required_free: Option<Tao>, pub available_free: Option<Tao>, pub estimated_fee: Option<Tao>,
    pub facts: Vec<(String, String)>, pub facts_title: FactsTitle }                   // base.py:109-134
```

`OriginView` implements Python's key-view rules (`hotkey_address`/`coldkey_address`, `PY/intents/base.py:258-298`): an
override wins; otherwise the wallet's key; a bare signer answers only for its own role; anything else is an `IntentError`
with Python's message.

**Generated dispatch.** `pub enum AnyIntent { AddStake(AddStake), …83… }` with `from_tool(op, serde_json::Value)`
(JSON Schema validation with the `jsonschema` crate, then serde), `spec()`, and `match`-based delegation of every trait
method. Every catalog op is a variant, so an intent without an `impl Intent` fails to compile. During the build-out an op can
be listed in `parity/pending.toml`; xtask then emits a stub impl that returns `IntentError::NotYetPorted(op)`. The parity
gate (§4.7) only lets that list shrink, except for ops newly added upstream.

**Pass-through intents.** By a name-normalization heuristic in this session (strip `_ss58`/`_tao`/`_alpha`/`_rao`/`_hex` from
schema properties and compare with the wrapped call's argument names in `RES/_call_index_map.txt`), **46 of 83** intents
have a single wrapped call whose arguments map one-to-one to schema fields, and **37** select between calls, compose several,
or rename or derive arguments [V for the counts; the heuristic is mine]. For the 46, xtask emits `build()` from a generated
field-to-argument map, with unit conversions carried by the field types (e.g. `take: Ratio` → `PerU16`, proportions →
`U64Normalized`). `summary()`, any `preflight` hooks (for example `burned_register`'s burn/lock split) and `"all"` resolution
stay hand-written where Python has them. `xtask/intent_callmap.toml` can also declare renames, such as `swap_basket`'s
`dest_netuid` → `destination_netuid`, to move near-pass-through intents into the generated set. Everything else is ported
line-by-line.

**Hand-ported bodies mirror Python file-for-file** (`parity/source_map.toml` drives the porting report):

```
rtao-tx/src/intents/
  staking.rs (PY/intents/staking.py, 1,925 lines)   weights.rs (weights.py, 563)        registration.rs (registration.py, 595)
  transfer.rs  evm.rs  children.rs  coldkey.rs  collateral.rs  lock.rs  governance.rs  hyperparameters.rs  identity.rs
  root.rs  serving.rs  proxy.rs  multisig.rs (536)  crowdloan.rs  leasing.rs  association.rs  batch.rs
  _money.rs (_money.py, 165)   _root_claim_fee.rs (_root_claim_fee.py, 771)
```

Every function carries a `// py: intents/staking.py:L542-649` provenance comment, and the porting report diffs those
ranges between tags. The 83-intent inventory with wraps, params, spend class and preflight reads is RES/A1 §3. It is the
porting checklist, and the oracle (§11.5) proves each port.

### 4.3 Executor, plan, policy (`rtao-tx::executor`, port of `PY/executor.py`, 1,319 lines)

```rust
pub struct Executor { client: Client, policy: Option<Policy> }
impl Executor {
    pub async fn plan<I: Intent>(&self, intent: &I, wallet: &dyn WalletLike, o: &PlanOptions) -> Result<Plan, TxError>;
    pub async fn preflight<I: Intent>(&self, intent: &I, wallet: &dyn WalletLike, o: &PlanOptions) -> Result<IntentPreflight, TxError>;
    pub async fn execute<I: Intent>(&self, intent: &I, wallet: &dyn WalletLike, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>;
    pub async fn execute_tool(&self, op: &str, args: serde_json::Value, wallet: &dyn WalletLike, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>;
    pub async fn submit_shielded<I: Intent>(&self, intent: &I, wallet: &dyn WalletLike, o: &ShieldOptions) -> Result<ExtrinsicResult, TxError>;
    pub async fn submit_call(&self, call: Call, wallet: &dyn WalletLike, role: SignerRole, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>; // raw: policy.allow_raw_calls
    pub async fn prepare_call(&self, call: Call, address: AccountId, o: &PrepareOptions) -> Result<UnsignedExtrinsic, TxError>;
    pub async fn submit_signature(&self, u: UnsignedExtrinsic, sig: MultiSignature, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>;
    pub fn tools(&self) -> &'static [ToolSpec];                                  // = catalog projection (list_tools, registry.py:43-66)
}
pub struct ExecuteOptions {   // defaults = PY/executor.py:782-788
    pub period: Option<u32> /*Some(128)*/, pub wait_for_inclusion: bool /*true*/, pub wait_for_finalization: bool /*true*/,
    pub retries: u32 /*0*/, pub proxy_for: Option<AccountId>, pub force_proxy_type: Option<ProxyType>,
    pub policy: Option<Policy>, pub wait_for_registration: bool /*true*/, pub registration_timeout: Option<Duration>,
    pub progress: Option<mpsc::Sender<ProgressEvent>>,
}
```

Flow, identical to Python (RES/A1 §4.1-4.7). **`plan`**:
1. coerce addresses;
2. compose (pinned `inner_call_data`, semantic build against the origin view, then sudo, proxy and adapter wrappers; §3.9);
3. compute the accounts (`dispatch_origin` versus `fee_payer`);
4. run the intent preflight;
5. fee: the intent's exact fee, else the fee probe; failure is a warning;
6. add `COLDKEY_FEE_WARNING` when a hotkey signs a coldkey-pays-fee leaf (leaves are found by walking Proxy/Utility/Multisig
   wrappers over the generated runtime-derived set);
7. policy check, with intent `blocks` merged into the violations.

**`execute`**:
1. a `mev_shield_required` intent goes straight to `submit_shielded`, before any plan (`PY/executor.py:812-824`);
2. otherwise run `plan` and refuse with `PolicyError` if it is not `ok`;
3. resolve the signer for the intent's role and submit **exactly** `plan.call`, so the dry run and the signed bytes cannot
   diverge;
4. transient-only retries (§3.7);
5. re-check nested dispatch failures;
6. merge extras: `pure_proxy` data for `create_pure_proxy`, plan extras, and the `register_subnet` completion (§3.11).

`Plan` mirrors `PY/intents/plan.py:89-135` (`op, summary, signer, signer_address, fee, effects, warnings, violations, call
(not serialized), extras, spend, args`) and `to_json()` emits Python's `to_dict()` shape (`fee_tao`, `spend_tao`,
`spend_unbounded`, `ok`, `docs_url`).

`Policy { max_fee: Option<Tao>, max_spend: Option<Tao>, allowed_netuids: Option<BTreeSet<NetUid>>, allow_raw_calls: bool }`
is a port of `PY/intents/plan.py:21-85`, with Python's violation wording. Its semantics:
- `max_fee` with no estimate is a violation;
- an `UNBOUNDED` spend is a violation whenever `max_spend` is set;
- `affects_all_subnets` is a violation whenever `allowed_netuids` is set;
- raw calls are refused unless `allow_raw_calls`.

`tests/policy_equivalence.rs` builds the 22 core trusted constructors (`CORE/transaction.rs:251-826`) and their rtao intent
counterparts and asserts that `bittensor_core::transaction::Policy::check` and rtao's `Policy::check` report the same
violation *set* (wording excluded).

### 4.4 Results and the error model

```rust
pub struct ExtrinsicResult {            // PY/result.py:696-747 (frozen; to_dict)
    pub success: bool, pub message: String, pub block_hash: Option<H256>, pub extrinsic_id: Option<String>,
    pub explorer_url: Option<String>,   // finney/archive only, template extracted from PY/settings.py
    pub fee: Option<Tao>, pub events: Vec<EventRecord>, pub error: Option<ChainError>,
    pub data: BTreeMap<String, serde_json::Value>,
}
impl ExtrinsicResult { pub fn raise_for_failure(self) -> Result<Self, ChainError>; pub fn to_json(&self) -> serde_json::Value; }

pub struct ChainError { pub message: String, pub name: Option<String>, pub code: ErrorCode }   // result.py:501-562
impl ChainError {
    pub fn remediation(&self) -> &str;          // per-name override → substring override → per-code default
    pub fn description(&self) -> Option<&str>;  // catalog description or DISPATCH_ERRORS prose
    pub fn explanation(&self) -> &str;          // EXPLANATIONS[code]
    pub fn docs_url(&self) -> String;           // /docs/errors/chain/<Name> if classified else /docs/errors/<code-with-dashes>
}
#[non_exhaustive] pub enum ErrorCode { InsufficientBalance, InsufficientLiquidity, RateLimited, NotRegistered, NotAuthorized,
    AlreadyExists, NotFound, SubnetNotExists, SubtokenDisabled, Disabled, TooEarly, Expired, LimitExceeded, UnitMismatch,
    InvalidArgument, PolicyViolation, Internal, Unknown }    // GENERATED from CAT/errors.json `codes` (18) [V]
pub enum TxError {
    Policy(PolicyError),                 // code = PolicyViolation (result.py:687-693)
    Chain(ChainError),                   // includes client-side preflight ChainErrors (same codes as chain: weights.py:154-162)
    RpcPolicy(RpcPolicyError),           // status_code, retry_after, policy, reason
    Rpc(RpcConnectionError), ConnectionNotReady, FateUnknown { extrinsic_hash: H256 },
    Intent(IntentError), Signer(SignerError), UnitMismatch(UnitError),
}
```

**Classification** (`classify(message, name) -> ErrorCode`, port of `PY/result.py:483-498`):
1. `NAME_TO_CODE` (generated from `errors.json` `chain_errors`, 390 names);
2. `DISPATCH_ERRORS` (extracted);
3. name ends with `RateLimitExceeded` or contains `RateLimit` → `RateLimited`;
4. no name: the substring fallback (extracted);
5. otherwise `Unknown`.

Pool `Custom(N)` texts go through `_CUSTOM_ERROR_RE` (extracted) and `CUSTOM_TRANSACTION_ERRORS`. Standard
`InvalidTransaction` texts go through the needle table. Watch fatals go through `_WATCH_STATUS_RE`. Module errors resolve
`(pallet_index, error_index)` through `Runtime::module_error` (`CORE/runtime/mod.rs:340`) at the block's runtime, never
through a stale table. The generated `MODULE_ERRORS` table (the analogue of `PY/_generated/errors.py`) is used only for
docs, and the CI check that every chain error name classifies.

### 4.5 Reads

**Generated** (`rtao-reads/src/generated/`):

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, EnumString, IntoStaticStr)] #[strum(serialize_all = "snake_case")]
pub enum ReadName { AcceptsLockedAlpha, AlphaPrice, AlphaPrices, /* … 102 … */ }
pub struct ReadSpec { pub name: &'static str, pub summary: &'static str, pub description: &'static str,
    pub params: &'static [(&'static str, JsonType, &'static str /*doc*/)], pub category: Category,
    pub namespace: &'static str, pub docs_url: &'static str, pub hits: &'static [Hit], pub render: Option<RenderHint>,
    pub pinning: Pinning }   // Pinning::{Single, Pinned} — every multi-RPC read is Pinned (§3.10)
pub async fn dispatch<V: View>(view: &V, name: ReadName, params: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, ReadError> {
    match name {
        ReadName::DelegateTake => { let hotkey: AccountId = param(params, "hotkey_ss58")?;
            to_json(&crate::reads::delegation::delegate_take(view, &hotkey).await?) }
        // … one arm per read; a missing implementation is a compile error (or a pending.toml stub)
    }
}
```

**Hand-written** (`rtao-reads/src/reads/<module>.rs`, mirroring `PY/reads/<module>.py`: accounts, chain, collateral,
delegation, epochs, hyperparameters, identity, leasing, locks, neurons, prices, root, staking, subnets, weights). Each read is
a generic async fn over `View`, with a typed return model (§8). Scalar reads (`scalar_read`, `PY/reads/base.py:156-166`)
are generated whole: one storage item, a typed value, and an integer conversion.

**Namespaces.** 14 categories map to accessor structs named from the catalog's `python` field (`sub.delegation.delegate_take(...)`
becomes `client.delegation().delegate_take(&hotkey)`). Every namespace exists on both `Client` (head) and `Snapshot`
(pinned), and the `block=` keyword of Python's namespace methods (`PY/namespaces.py:52-97`) becomes
`client.at(Some(b)).await?.delegation()…`. The curated extras (`balances.get`, `get_many`, …, `namespaces.py:99-213`) are
hand-written one-liners.

**JSON parity.** `to_json` emits Python field names and follows `btcli query --json`'s `_jsonable` (`PY/cli/query.py:104-116` [V]):
a `Balance` renders as `str(Balance)` (the symbol-tagged decimal string), datetimes as ISO-8601 at second precision,
dataclasses as dicts, and floats as Python renders them (`PerU16` fractions). The read oracle (§11.5) compares rtao's JSON with Python's on the
same seeded state as **parsed values**, not bytes: Python's `json.dumps` and `serde_json` format some floats
differently (`1e-05` versus `1e-5`), but both are shortest round-trip representations, so parsed f64s compare exactly.
The output a CLI user sees for `rtao query X --json` is therefore value-identical to `btcli query X --json`.

### 4.6 Error taxonomy, end to end

Generated `rtao_catalog::errors`: `ErrorCode` (18) with `remediation()`/`explanation()`/`docs_url()`; `CHAIN_ERRORS`
(390 names → code, description, pallets, docs/markdown URLs, chain `sources`); `CUSTOM_TX_ERRORS: [(u8, &str); 29]`
cross-checked against the runtime enum; `INVALID_TX_NEEDLES`, `DISPATCH_ERRORS`, `SUBSTRING_FALLBACK`,
`NAME_HELP_OVERRIDES`, `HELP_OVERRIDES`. These are the same gates Python's `codegen/check.py --names` enforces
(`:354-455`), recast as rtao tests:
- every chain error name in the pinned metadata classifies;
- every classified name still exists in the metadata;
- every code has remediation and an explanation;
- the custom codes equal the runtime enum's codes.

`rtao explain <code|Name>` (§10) renders the same data.

### 4.7 The parity matrix and how CI enforces it

`cargo xtask parity` builds `parity/matrix.json` from the upstream side (at the pin) and the rtao side (from the generated
registries plus `#[parity(...)]` markers on hand-written items):

| Row family | Upstream inventory source | Count at 850f275 |
|---|---|---|
| intents | `CAT/intents.json` | 83 |
| reads | `CAT/reads.json` | 102 |
| namespace methods (curated) | `PY/namespaces.pyi` (generated stub; parse with `ast`) | from file |
| error codes / chain errors / custom codes | `CAT/errors.json`, extracted tables, runtime enum | 18 / 390 / 29 |
| executor and transport features | fixed checklist from RES/A1 §4, §6 (shield, raw, offline, adapters, retries, registration completion, pools, policy refusals, archive fallback, disk cache) | ~25 |
| CLI commands and flags | Python Typer tree, dumped by `tools/oracle/dump_cli.py` (walks the Typer app at the pin) | from dump |
| signers | `PY/signing.py`, `ledger.py`, `vault/`, `extension/` | 5 |
| public package API | `PY/__init__.py` `__all__` | from file |
| hyperparameters | extracted `HYPERPARAMS` / `OWNER_HYPERPARAMETERS` | ~50 / 33 |
| metagraph fields | `PY/metagraph.py` dataclass fields | from file |

Each row has an rtao status (`generated`, `ported`, `pending(issue)`, `excluded(reason)`) and an oracle status (`pass`,
`fail`, `n/a`). Gates:
1. **Compile-time.** Every catalog op or read lacking an implementation fails to build unless listed in `parity/pending.toml`.
2. **`xtask parity --check` on every PR.** Re-derives the upstream inventory from the vendored artifacts and fails on any
   upstream row with no rtao row. It also fails on a new `pending` entry not created by a pin-bump PR, and on an `excluded`
   entry without a reason and issue link. Until M7 the rule is "no regressions"; after M7 it is "zero pending, zero
   unexplained exclusions".
3. **Coverage gate** (port of `codegen/check.py:303-346`). Every call in the 13 covered pallets of the pinned metadata is
   wrapped by an rtao intent, listed in the extracted `RAW_ONLY`, or explicitly in `parity/raw_only_extra.toml`. A new chain
   call therefore forces a decision, as it does in Python. Root-origin intents must name an existing `verify` read.
4. **Oracle gates** (§11.5). Per-intent composed-call, violation and effect equality; per-read JSON equality.
5. **Early warning.** A nightly job diffs the catalogs on monorepo `main` against the pin (catalogs on `main` are kept in sync
   by upstream's `generate.py --check` gate, RES/I §5.1). New or changed ops are filed as issues tagged `upstream-next` days
   before the tag ships.

---
## 5. Money and units (`rtao-types`)

The upstream conventions are fully catalogued in RES/A2 §2. Metadata supplies most unit identities (`TaoBalance`,
`AlphaBalance`, `NetUid`, `MechId`, `NetUidStorageIndex`, `PerU16`; RES/A2 §0.7). The rest comes from the extracted
`HYPERPARAMS` table (blocks, epochs, per-million, u64-normalized). rtao adds no unit knowledge of its own: types come from
metadata idents, and kinds come from the extracted table.

### 5.1 Identity newtypes

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)] #[serde(transparent)]
pub struct NetUid(pub u16);              impl NetUid { pub const ROOT: NetUid = NetUid(0); }
pub struct MechId(pub u8);               // ≤ 16 hard cap (RES/C1 §2.4)
pub struct NetUidStorageIndex(pub u16);  // mechid * 4096 + netuid  (GLOBAL_MAX_SUBNET_COUNT, extracted from PY/settings.py:31)
impl NetUidStorageIndex {
    pub fn new(netuid: NetUid, mech: MechId) -> Result<Self, UnitError>;   // checked arithmetic (workspace lint: arithmetic-side-effects)
    pub fn netuid(self) -> NetUid; pub fn mech(self) -> MechId;            // idx % 4096, idx / 4096 (PY/metagraph.py:384)
}
pub struct Uid(pub u16);
pub struct BlockNumber(pub u64);
pub struct AccountId(pub [u8; 32]);      // Display/FromStr = ss58 prefix 42 via bittensor_core::keys::{ss58_from_public, public_key_from_ss58}
pub enum CryptoType { Ed25519 = 0, Sr25519 = 1 }   // CORE/keys/mod.rs:28-29
```

All of them implement `IntoCoreValue` (`NetUid` → `Value::Int`, `AccountId` → `Value::Bytes`) and `scale_decode::IntoVisitor`
by unwrapping single-field composites, so `NetUid` decodes from the `subtensor_runtime_common::NetUid(u16)` composite,
from `Compact<NetUid>`, and from a bare `u16` key alike.

### 5.2 Balances

Python's proven design is a rao integer tagged with a netuid, where mixing netuids raises (`PY/balance.py:1-25,92-116`;
RES/A2 §2, §8.3). rtao makes the TAO/alpha split static wherever it is statically known, and keeps netuid mismatches as a
runtime error:

```rust
pub const RAO_PER_TAO: u64 = 1_000_000_000;                                   // extracted: PY/settings.py:15
pub struct Tao(u64);                                                           // rao; netuid 0 currency
pub struct Alpha { netuid: NetUid, rao: u64 }                                  // subnet alpha in rao-scale units
pub struct AlphaRao(pub u64);                                                  // wire `AlphaBalance` before a netuid is attached
pub enum Balance { Tao(Tao), Alpha(Alpha) }                                    // reads that return mixed units
impl Tao {
    pub const fn from_rao(rao: u64) -> Self; pub const fn rao(self) -> u64;
    pub fn from_decimal_str(s: &str) -> Result<Self, UnitError>;               // exact, ≤ 9 fractional digits; no floats in the default API
    pub fn from_tao_lossy(x: f64) -> Result<Self, UnitError>;                  // explicit name; rejects negatives/NaN (one-covenant issue #8, RES/E §4.1.8)
    pub fn checked_add(self, o: Tao) -> Option<Tao>; pub fn checked_sub(self, o: Tao) -> Option<Tao>; pub fn saturating_sub(self, o: Tao) -> Tao;
    pub fn to_decimal_string(self) -> String;                                  // exact "1.500000000"
}
impl Alpha {
    pub fn new(netuid: NetUid, rao: u64) -> Self;
    pub fn checked_add(self, o: Alpha) -> Result<Option<Alpha>, UnitError>;    // Err(UnitMismatch) on different netuids (PY UnitMismatchError)
    pub fn tao_equivalent(self, price: AlphaPrice) -> Tao;                     // explicit; never implicit
}
impl AlphaRao { pub fn on(self, netuid: NetUid) -> Alpha; }
pub enum Amount<T> { Exact(T), All }                                           // the "all" sentinel (PY/intents/_money.py:34,51-55)
pub enum Spend { None, Bounded(Tao), Unbounded }                               // PY/intents/_money.py:37-48
```

- `Display` matches Python's `str(Balance)` byte for byte (`PY/balance.py:127-152` [V]). TAO renders as `τ1,234.500000000`
  (thousands separators, 9 decimals). Alpha renders as `1,234.500000000β`, with the subnet's `TokenSymbol` as a suffix, or
  `α` plus a subscript netuid (`α₄₂`) when untagged. Symbols are loaded at connect through the same 7-day disk cache as
  Python (`PY/config.py:22-27`, `PY/client.py:613-633`; path extracted).
- There are no `Add`/`Sub` impls that can panic: this follows the monorepo lint policy (§2.5) and Python's "never silently
  mix units" stance. `Sum` exists only through `try_sum`.
- **Wire-type caveat.** `Metagraph.total_stake` is declared `TaoBalance` but is alpha-equivalent stake weight
  (`alpha + tao × tao_weight`; RES/A2 §4.1, §9.2). rtao models it as `StakeWeight(u64)`, not `Tao`. The model field is
  typed by hand, and `check-models` (§8.1) records the ident override so that a future wire-type change is flagged.

### 5.3 Fixed point and ratios

```rust
pub struct FixedU128<const FRAC: u32> { pub bits: u128 }   pub type U64F64 = FixedU128<64>;
pub struct FixedI128<const FRAC: u32> { pub bits: i128 }   pub type I96F32 = FixedI128<32>; pub type I64F64 = FixedI128<64>;
pub struct FixedI64<const FRAC: u32>  { pub bits: i64 }    pub type I32F32 = FixedI64<32>;
impl<const F: u32> FixedU128<F> {
    pub fn to_f64(self) -> f64;                      // bits / 2^F  (Python's conversions, RES/A2 §2)
    pub fn integer_part(self) -> u128;               // bits >> F   (conviction, root claim threshold)
    pub fn to_decimal_string(self, digits: u32) -> String;   // exact
}
pub struct PerU16(pub u16);    impl PerU16 { pub fn fraction(self) -> f64 { f64::from(self.0) / 65535.0 } }
pub struct Permill(pub u32); pub struct Perbill(pub u32); pub struct Percent(pub u8);
pub struct U64Normalized(pub u64);   // child proportions: u64::MAX = 1.0 (PY/reads/delegation.py:156-166)
pub struct Ratio(/* exact decimal */);   // intent inputs such as rate_tolerance=0.05
```

Decoding goes through a scale-decode visitor over the `{ bits }` composite (`substrate_fixed::FixedU128<…>` in metadata;
RES/F §4). The visitor checks the fractional-bit count from the typenum path when present and otherwise trusts the field
mapping. Alpha sigmoid steepness reads as `I32F32::integer_part` to match the setter (`PY/reads/subnets.py:126-147`).
Hyperparameter human-unit conversion is `rtao_types::hyperparams::{to_raw, from_raw, annotate}`, a port of
`PY/hyperparams.py:581-770` (a decimal point means human units, a plain integer means raw; bit width precedence is explicit
`bits`, then storage ident, then kind default) over the extracted `HYPERPARAMS` table, so a new hyperparameter upstream
needs no Rust change. `tests/hyperparam_units.rs` mirrors Python's `--units` gate (`codegen/check.py:472-507`): the
extracted kind must agree with the metadata ident of the storage item.

---

## 6. Wallets and signing (`rtao-signer`, `Signer` trait in `rtao-types`)

### 6.1 On-disk wallets (btcli-compatible, through the core)

- Layout identical to Python: `<path>/<wallet>/coldkey`, `coldkeypub.txt`, `hotkeys/<hotkey>`, `hotkeys/<hotkey>pub.txt`.
  Default path `~/.bittensor/wallets`, overridable by `BT_WALLET_PATH` (`PY/wallet.py:11,63-68` [V]).
- **Load**: read bytes, detect the format with the core (`$NACL`, `$ANSIBLE_VAULT`, `gAAAAA` legacy Fernet;
  `CORE/keyfiles/mod.rs:29-47`), `decrypt_keyfile_data` if encrypted, then `deserialize_keypair_from_keyfile_data`
  (`:287`), which carries every btwallet quirk: the leading-space `" ss58Address"` key, `cryptoType` as a number or string,
  retry with the other crypto type on an address mismatch, bare seed/mnemonic files, and the polkadot-js export hint
  (RES/B §3.3). rtao reimplements none of it.
- **Create**: `Keypair::generate_mnemonic(12|15|18|21|24)` → `from_mnemonic` → `serialized_keypair_to_keyfile_data` →
  `encrypt_keyfile_data` (NaCl) → atomic temp+fsync+rename with 0o600 files and 0o700 directories (Python's permission
  policy, `PYT/unit/test_keyfiles_golden.py:121-140`, RES/H §1.5). Regenerate from mnemonic, seed or URI uses the same core
  constructors. polkadot-js JSON import uses `Keypair::from_encrypted_json` (`CORE/keys/mod.rs:338`, host).
- **Unlock** happens once per process in `spawn_blocking` (Argon2i, 512 MiB, t=8; RES/F §3.3). The key lives in core
  `Zeroizing` buffers, and the unlocked `Keypair` stays behind an `Arc` inside `WalletSigner` (lazy, as Python's
  `WalletSigner` does, `PY/signing.py:143-224`).
- The coldkey public key comes from `coldkeypub.txt` and never requires unlocking (`PY/intents/base.py:288-298`).

### 6.2 Password resolution (exact order of `PY/wallets.py:62-120` [V])

```rust
pub enum PasswordSource { Explicit(SecretString), EnvAll, EnvPerKeyfile, File(PathBuf), MacKeychain, MacDialog, Prompt }
pub async fn resolve_wallet_password(w: &Wallet, opts: &PasswordOptions) -> Result<Option<SecretString>, SignerError>;
```

1. the explicit argument;
2. `BT_WALLET_PASSWORD`;
3. the per-keyfile variable `BT_PW_<PATH_WITH_/_AND_._AS_UNDERSCORES, UPPERCASED>` (`PY/keyfiles.py` `env_var_name`
   [V]), decoded by the core's `get_password_from_environment` (XOR with the variable name plus base64,
   `CORE/keyfiles/mod.rs:111`);
4. the `password_file` option or `BT_WALLET_PASSWORD_FILE` (first line, trimmed);
5. the macOS Keychain (feature `keychain`): generic password with service `bittensor.coldkey` and the wallet name as
   the account (`PY/macos_password.py:8,62-70` [V]), read with the `security-framework` crate [I: crate choice];
6. the macOS dialog (`osascript`, CLI only);
7. otherwise `None`. The **library never prompts**: it returns `SignerError::PasswordRequired` (RES/E §4.2.9: no prints or
   prompts in library code), and the CLI prompts with `rpassword`, retrying on an empty or mismatched entry like
   `PY/keyfiles.py:42-62`.

`rtao wallet save-password-env` prints `export BT_PW_…=<encoded>` using the staged pure encoder (G11) and never calls
`set_var`. A wrong password maps `CoreError::WrongPassword` to `SignerError::WrongPassword`, keeping Python's message.

### 6.3 Signer trait

```rust
bitflags! { pub struct SignerCaps: u8 {
    const RAW_PAYLOAD   = 1 << 0;   // signs signature_payload bytes (keypairs, remote)
    const METADATA_HASH = 1 << 1;   // wants CheckMetadataHash=Enabled + RFC-0078 proof (MetadataVerifyingSigner; Ledger)
    const PAYLOAD_JSON  = 1 << 2;   // signs Polkadot-JS SignerPayloadJSON (ExtensionPayloadSigner; browser extensions)
    const UNSIGNED_TX   = 1 << 3;   // takes the whole UnsignedExtrinsic (UnsignedExtrinsicSigner; Vault, Ledger clear-signing)
}}
pub trait Signer: Send + Sync {
    fn public_key(&self) -> [u8; 32];
    fn crypto_type(&self) -> CryptoType;
    fn ss58_address(&self) -> AccountId { AccountId(self.public_key()) }
    fn caps(&self) -> SignerCaps { SignerCaps::RAW_PAYLOAD }
    fn sign<'a>(&'a self, req: SigningRequest<'a>) -> BoxFuture<'a, Result<MultiSignature, SignError>>;
    fn sign_raw<'a>(&'a self, msg: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignError>>;   // btauth/1, EVM association
}
pub struct SigningRequest<'a> {
    pub call: &'a [u8],
    pub included_in_extrinsic: &'a [u8],      // core signature_payload_parts().0 (era, nonce, tip, mode)
    pub included_in_signed_data: &'a [u8],    // core signature_payload_parts().1 (spec, tx, genesis, era hash, metadata hash)
    pub payload: &'a [u8],                    // core signature_payload() — blake2-256 if > 256 bytes
    pub payload_json: Option<&'a SignerPayloadJson>,   // PY/_transport/contract.py:87-132 big-endian-hex quirks, built by rtao-chain
    pub context: &'a SigningContext,          // raw V15 metadata bytes, ChainInfo, genesis, ss58 format (contract.py:49-63)
}
pub struct MultiSignature { pub version: CryptoType, pub bytes: [u8; 64] }   // 65-byte replies are split (extrinsics.py:202-213)
```

`dyn Signer` with boxed futures is deliberate here (and only here). Signers are heterogeneous runtime choices made from CLI
flags (`--ledger`, `--signer extension`), signing happens at most a few times per block, so the boxing is noise, and
`sign_raw` lets btauth/1 run on any signer.

| Implementation | Built on | Caps | Notes |
|---|---|---|---|
| `KeypairSigner` | `bittensor_core::keys::Keypair::sign` (`CORE/keys/mod.rs:391`) | RAW | dev URIs, mnemonics, unlocked keyfiles |
| `WalletSigner` | `Wallet` + password resolution + `KeypairSigner` | RAW | lazy unlock, once per process |
| `LedgerSigner` (feature `ledger`) | `LedgerDevice::sign(account, index, unhashed_payload, proof)` (`CORE/signers/ledger.rs:124`); proof = `digest::generate_extrinsic_proof(call, ext, signed, metadata, info)` (`CORE/digest/mod.rs:117`) | METADATA_HASH \| UNSIGNED_TX | ed25519 at `m/44'/354'/account'/0'/index'` (RES/B §3.8); the 65-byte reply is split into version and signature; `spawn_blocking` |
| `VaultSigner` (feature `vault`) | port of `PY/vault/uos.py` (UOS framing: `0x53` ‖ crypto ‖ cmd ‖ pubkey ‖ payload ‖ genesis, multipart envelope [V]) + `prepare_call`/`submit_signature`; serves upstream `PY/vault/static/vault.html` **verbatim** (embedded with `include_bytes!` from vendor) | UNSIGNED_TX | air-gapped QR; the HTTP endpoint shape follows `PY/vault/server.py` |
| `ExtensionSigner` (feature `extension`) | local HTTP+WS bridge on `127.0.0.1:39295`, token in `~/.bittensor/extension_bridge.token`, serving upstream `PY/extension/static/bridge.html` **verbatim** (`PY/extension/bridge.py:17-40` [V]) | PAYLOAD_JSON | wire-compatible with the Python bridge daemon, so either side's bridge can be reused |
| `RemoteSigner` | user-supplied `Arc<dyn Fn(SigningRequest) -> BoxFuture<…>>` or an HTTP signing service | any | HSM, KMS, signing box |

### 6.4 How the executor uses signers

- `resolve_signer(wallet: &dyn WalletLike, role: SignerRole)` follows `PY/signing.py:227-263`: a `Wallet` gives a
  `WalletSigner` for the requested key; a `KeyedWallet{coldkey, hotkey}` gives the matching key; a bare `Arc<dyn Signer>`
  passes through.
- **The signer role is data**: `IntentSpec.signer` is generated from the catalog's `signer` field (`coldkey`/`hotkey`), and
  `raw` calls take the role explicitly.
- `METADATA_HASH` sets `TxParams.metadata_hash = Some(digest)` before the payload is built, because the mode byte is part of
  the signed bytes (`CORE/codec/extrinsic.rs:181-229`). `UNSIGNED_TX` routes through `prepare_call`/`submit_signature`
  internally, so a Vault user runs the same `execute` call.
- Hotkey-signed leaves whose fee goes to the coldkey get the warning from §4.3 (`COLDKEY_PAYS_FEE_CALLS`, runtime-extracted).
- Python's documentation claims hotkey-signed extrinsics are refused for shielding, but no such check exists in the code
  (RES/A1 §6.5). rtao follows the code (no refusal) and records the discrepancy in `parity/exclusions.toml` as "docs drift
  upstream".

---
## 7. Neuron networking (`rtao-btauth`, optional `rtao-transport-quic`)

v11 ships exactly one neuron-to-neuron protocol: btauth/1 (`PY/http_auth.py`, 369 lines; normative
`MONO/docs/guides/signed-requests.mdx`, 321 lines). rtao implements it byte-exactly, reusing the core for the crypto. The
full specification is in RES/D §1; this section covers only the design.

### 7.1 Core (no I/O, no tokio)

```rust
pub const PROTOCOL: &str = "btauth/1"; pub const VERSION: &str = "1";
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(10); pub const DEFAULT_ALLOWED_SKEW: Duration = Duration::from_secs(2);
pub const NO_RECEIVER: &str = "-";                                   // http_auth.py:58-75 (RES/D §1.1)

/// Exactly eight lines joined by '\n', no trailing newline (http_auth.py:175-185).
pub fn build_payload(scheme: CryptoType, method: &Method, target: &str /* path+query, percent-encoded as sent */,
    body: &[u8], nonce_ns: u64, sender: &AccountId, receiver: Option<&AccountId>) -> Vec<u8>;
pub async fn sign(signer: &dyn Signer, method: &Method, target: &str, body: &[u8], receiver: Option<&AccountId>,
    now_ns: impl FnOnce() -> u64) -> Result<HeaderMap, AuthError>;   // fresh nonce every call (signed-requests.mdx:101-102)
pub fn verify(headers: &HeaderMap, body: &[u8], method: &Method, target: &str, own: &AccountId, cfg: &VerifyConfig,
    store: &dyn NonceStore, now_ns: u64) -> Result<Caller, AuthError>;
pub struct VerifyConfig { pub max_age: Duration, pub allowed_skew: Duration, pub require_receiver: bool /*true*/,
    pub accept_bytes_wrapper: bool /*true: Python parity via core keys::verify; false = strict*/ }
pub struct Caller { pub hotkey: AccountId, pub nonce_ns: u64, pub crypto_type: CryptoType }
pub enum AuthError { Malformed(&'static str), WrongReceiver(&'static str), Stale(&'static str), Replayed, BadSignature }
// Display strings are Python's verbatim (http_auth.py:272-341) — they are the de-facto contract (RES/D §8.1).
pub trait NonceStore: Send + Sync {
    fn retention(&self) -> Duration;
    fn check_and_store(&self, hotkey: &AccountId, nonce_ns: u64) -> bool;    // true = first time seen
}
pub struct InMemoryNonceStore { /* (hotkey, nonce) set + FIFO expiry, retention 60 s (http_auth.py:126-151) */ }
```

**Exact payload bytes** (UTF-8; eight lines joined by `\n`; no trailing newline; `http_auth.py:175-185`, RES/D §1.2):

```
btauth/1
<sr25519|ed25519>                      # re-derived from the parsed crypto type, never echoed from the header
<METHOD uppercased>
<path?query exactly as sent, percent-encoded>
<lowercase hex sha256(raw body)>       # empty body: e3b0c442…b855
<nonce: unix time in ns, decimal>
<sender hotkey ss58>
<receiver hotkey ss58, or "-">
```

Headers: `X-Bittensor-Version: 1`, `X-Bittensor-Hotkey`, `X-Bittensor-Receiver` (when bound), `X-Bittensor-Nonce`,
`X-Bittensor-Crypto: ed25519` (omitted for sr25519), and `X-Bittensor-Signature: 0x<64-byte sig hex>`. The
`X-Bittensor-Drand-*` names are reserved and never emitted (RES/D §1.1).

- `verify` follows the normative order (RES/D §1.5):
  1. shape and headers (`X-Bittensor-Version == "1"`, hotkey, decimal nonce, crypto, hex signature; keys case-insensitive);
  2. receiver: absent with `require_receiver` → reject; present and ≠ own → reject;
  3. freshness: older than `max_age` → stale; more than `allowed_skew` in the future → stale (integer nanoseconds, not
     floats: RES/D §1.10);
  4. rebuild the payload from the **received** method, target and body, and the received receiver value (`-` when absent);
     verify under the declared scheme only, through `bittensor_core::keys::verify` (`CORE/keys/mod.rs:457`);
  5. only after the signature verifies, consult the store.

  `retention ≥ max_age + allowed_skew` is checked when a `Verifier` is **constructed** (a misconfiguration error, not a
  per-request auth error).
- **Never produce the `<Bytes>` wrapper.** By default rtao *accepts* wrapped signatures, because that is what Python's
  verifier does (it uses the same core function; RES/D §1.3). The core's `verify` always tries the raw message and then
  the wrapped one (`CORE/keys/mod.rs:402-410,457-469`). So `accept_bytes_wrapper = false` (strict) cannot reuse it: it
  verifies once, with `schnorrkel` (context `b"substrate"`; already in the core's dependency graph) or `ed25519-dalek`.
  That is about 20 rtao lines, and a candidate for a core `verify_strict` (it would join PR-U11).
- **Golden vectors**: `vendor/upstream/btauth_vectors.json`, extracted from `PYT/unit/test_http_auth.py:19-36` [V] and
  checked against `MONO/docs/guides/signed-requests.mdx:252-289`. It holds the sender `//Alice`, receiver `//Bob`, nonce
  `1752076800000000000`, body `{"prompt": "hello"}`, `POST /generate?stream=false`, the exact sr25519 payload, the ed25519
  address `5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu`, and the pinned ed25519 signature `0x1a6b6319…f704`. Every
  Python behavior test is mirrored by name: lowercase headers, crypto omitted for sr25519, `SR25519`/`1` crypto spellings,
  stale at +11 s, future at −3 s, replay on a second verify, `retention = 0` re-accepts, tampered body/method/path →
  `BadSignature`, each missing header → `Malformed`, `Nonce=not-a-number` / `Crypto=ecdsa` / `Signature=0xnothex` /
  `Version=2` → `Malformed` (RES/D §1.8).

### 7.2 Server (`feature = "server"`: tower 0.5 + axum 0.8 + http 1)

```rust
pub struct BtAuthLayer { verifier: Arc<Verifier>, body_limit: usize /* default 4 MiB [I] */, policy: Option<Arc<dyn CallerPolicy>> }
impl<S> Layer<S> for BtAuthLayer { type Service = BtAuthService<S>; … }
pub struct VerifiedCaller(pub Caller);   // axum extractor (from request extensions)
pub trait CallerPolicy: Send + Sync { fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<(), PolicyDenied>>; }
pub struct MetagraphPolicy { mg: watch::Receiver<Arc<Metagraph>>, require_registered: bool, require_permit: bool, min_stake: StakeWeight }
pub struct PerHotkeyRateLimit { … }      // priority/rate limiting keyed by verified hotkey (migration.mdx:455-480)
```

1. The service buffers the body up to `body_limit` **before** any extractor runs, because the hash needs the raw bytes. A
   streaming request body cannot be verified, so btauth/1 requires buffered requests; responses may stream (RES/D §1.9).
2. The target is `req.uri().path_and_query()` verbatim (the percent-encoded wire form). It is never normalized, and a
   trailing `?` is never stripped.
3. `verify` runs, then the `Caller` goes into the request extensions. A `CallerPolicy` denial returns 403. Every `AuthError`
   returns **401** with the JSON body `{"detail": "<python message>"}` (the FastAPI recipe's shape,
   `signed-requests.mdx:134-135,155-156`).
4. The nonce store is `Arc<dyn NonceStore>`. With feature `redis`, a `RedisNonceStore` does `SET key NX EX <retention>`
   (`signed-requests.mdx:97-99`).

### 7.3 Client (`feature = "client"`: reqwest 0.13, rustls)

```rust
pub struct SignedClient { http: reqwest::Client, signer: Arc<dyn Signer> }
impl SignedClient {
    pub async fn send(&self, req: reqwest::Request, receiver: &AccountId) -> Result<reqwest::Response, ClientError>;
    pub async fn post_json<T: Serialize>(&self, url: Url, receiver: &AccountId, body: &T) -> Result<reqwest::Response, ClientError>;
}
pub fn axon_base_url(axon: &AxonInfo) -> Option<Url>;   // "http://ip:port", "[v6]:port"; None when ip == 0 (PY/metagraph.py:563-574)
```

The body must be fully materialized. The target is `url.path()` plus `?` plus `url.query()` (reqwest keeps the
percent-encoded form). **Retries re-sign**: the client exposes `send_with_retry(n)`, which re-signs each attempt with a new
nonce, and documents that a transport-level retry resending an already-signed request is rejected as a replay
(`signed-requests.mdx:101-102,163-167`). Fan-out across miners uses `futures::stream::iter(...).buffer_unordered(k)`.

### 7.4 Legacy v10 and other schemes

- **Legacy v10 bridge** (`feature = "legacy-v10"`): `verify_v10(headers, body_hash: &str) -> Result<AccountId, AuthError>`
  is a line-for-line port of upstream's documented bridge recipe (`MONO/docs/guides/signed-requests.mdx:291-321` [V]). It
  checks the sr25519 signature over `"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"` from the `bt_header_*` headers;
  `body_hash` is supplied by the subnet because it depends on the schema. Like the recipe, rtao adds nothing more (no
  synapse codec, no dendrite). Freshness is the caller's job, as the guide says.
- **Full v10 Synapse/Axon/Dendrite emulation and Epistula**: not provided. Neither is part of the v11 package, which removed
  all networking (`MONO/docs/migration.mdx:455-480`), so under this lens they are outside the parity target. RES/D §2.9 and
  §3.4 specify them should a subnet need them later as separate crates.

### 7.5 QUIC (btlightning)

`rtao-transport-quic` (optional) is an adapter over the published `btlightning` crate (0.2.15, MIT; RES/D §4). rtao does not
reimplement it. It is built **without** btlightning's `subtensor` feature, which would pull subxt 0.50; rtao supplies
validator-permit and allowlist policy from its own `Metagraph`. Two caveats are documented: btlightning reads `protocol == 4`
as "QUIC", while every SDK writes 4 as the default (RES/D §4.7), so discovery must be configured per subnet rather than
inferred; and btlightning authenticates per connection, not per request (RES/D §4.5).

---
## 8. Metagraph and typed models (`rtao-reads::models`)

### 8.1 Principle: hand-written stable models, decoded against the live registry

Read return types are the one piece of the declared surface that upstream does not publish as data. The catalog gives
names and parameters only; Python's results are dataclasses and dicts. rtao therefore writes the models by hand, and this
is the cheapest form of ownership available:

- models mirror the **runtime struct field names** (e.g. `Metagraph<AccountId>`, `pallets/subtensor/src/rpc_info/metagraph.rs:14-113`,
  RES/A2 §4.2) and derive `scale_decode::DecodeAsType`;
- decoding by name tolerates appended fields (§1.6), so a runtime that only *adds* fields needs no rtao change;
- `xtask check-models` walks each model against its registry type in the pinned metadata (every model field exists, with a
  compatible primitive or compact type) and fails the drift PR when a field is removed, renamed or re-typed;
- the runtime's own `#[freeze_struct("…")]` hashes on `rpc_info` structs are extracted with `syn` and diffed per pin bump.
  A hash change is an early signal that a model needs review, even before metadata is fetched (RES/A2 §4.2).

### 8.2 Metagraph

```rust
/// Wire model: SubnetInfoRuntimeApi.get_metagraph / get_mechagraph output (77 fields at spec 477; abridged here).
/// Field names checked against pallets/subtensor/src/rpc_info/metagraph.rs:14-115 [V].
#[derive(Debug, Clone, DecodeAsType)]
pub struct MetagraphWire {
    pub netuid: NetUid,   // for get_mechagraph this carries the NetUidStorageIndex (RES/C2 §0.5)
    pub name: Vec<u8>,    // Vec<Compact<u8>> on the wire -> utf-8 (RES/A2 §2)
    pub symbol: Vec<u8>,
    pub identity: Option<SubnetIdentityV3>,
    pub network_registered_at: u64, pub owner_hotkey: AccountId, pub owner_coldkey: AccountId,
    pub block: u64, pub tempo: u16, pub last_step: u64, pub blocks_since_last_step: u64,
    pub alpha_in: AlphaRao, pub alpha_out: AlphaRao, pub tao_in: Tao, pub moving_price: I96F32, /* … pool, hyperparameter fields … */
    pub hotkeys: Vec<AccountId>, pub coldkeys: Vec<AccountId>, pub identities: Vec<Option<ChainIdentityV2>>,
    pub axons: Vec<AxonInfo>, pub active: Vec<bool>, pub validator_permit: Vec<bool>, pub last_update: Vec<u64>,
    pub emission: Vec<AlphaRao>, pub dividends: Vec<PerU16>, pub incentives: Vec<PerU16>, pub consensus: Vec<PerU16>,
    pub block_at_registration: Vec<u64>, pub alpha_stake: Vec<AlphaRao>, pub tao_stake: Vec<Tao>, pub total_stake: Vec<StakeWeight>,
    pub alpha_dividends_per_hotkey: Vec<(AccountId, AlphaRao)>,
    pub collateral_locked: Vec<AlphaRao>, pub collateral_min: Vec<AlphaRao>, pub collateral_earned: Vec<AlphaRao>,  // spec ≥ 435
    // Deliberately NOT declared: rank, trust, pruning_score, tao_dividends_per_hotkey, subnet_emission, pending_root_emission.
    // They are dead on chain (RES/C2 §0.4), and name-based decoding skips undeclared fields.
}
// Older runtimes (archive reads before spec 435) lack the collateral columns. The typed decode then fails with
// CannotFindField and rtao falls back to the dynamic Value path, which yields `None` collateral exactly as Python does
// (`graph.get("collateral_locked") is not None`, PY/metagraph.py:332-341).
/// v11 projection, field-for-field with PY/metagraph.py:127-235 (RES/A2 §4.1).
pub struct Metagraph {
    pub netuid: NetUid, pub mechid: MechId, pub name: String, pub symbol: String, pub block: BlockNumber,
    pub tempo: u16, pub last_step: u64, pub blocks_since_last_step: u64, pub owner_hotkey: AccountId, pub owner_coldkey: AccountId,
    pub network_registered_at: u64, pub num_uids: u16, pub max_uids: u16, pub price: Option<f64>, pub moving_price: Option<I96F32>,
    pub identity: Option<SubnetIdentity>, pub neurons: Vec<MetagraphNeuron>,
    pub commitments: BTreeMap<Uid, NeuronCommitment>, pub unregistered_commitments: BTreeMap<AccountId, NeuronCommitment>,
    pub raw: Arc<MetagraphWire>,
}
pub struct MetagraphNeuron { pub uid: Uid, pub hotkey: AccountId, pub coldkey: AccountId, pub active: bool,
    pub validator_permit: bool, pub last_update: u64, pub block_at_registration: u64,
    pub consensus: PerU16, pub incentive: PerU16, pub dividends: PerU16,
    #[deprecated(note = "not computed on chain; always 0 (metagraph.rs:782,803-804)")] pub rank: PerU16, /* trust, pruning_score likewise */
    pub emission: Alpha, pub alpha_stake: Alpha, pub tao_stake: Tao, pub total_stake: StakeWeight,
    pub axon: Option<SocketAddr>, pub identity: Option<ChainIdentity>, pub commitment: Option<NeuronCommitment>,
    pub collateral_locked: Option<Alpha>, pub collateral_min: Option<Alpha>, pub collateral_earned: Option<Alpha> }
impl Metagraph {
    pub fn by_uid(&self, uid: Uid) -> Option<&MetagraphNeuron>;
    pub fn by_hotkey(&self, hk: &AccountId) -> Option<&MetagraphNeuron>;   // O(1): index built once, unlike Python's linear scan
    pub fn validators(&self) -> impl Iterator<Item = &MetagraphNeuron>;  // permit holders
    pub fn hotkeys(&self) -> impl Iterator<Item = &AccountId>;
}
```

- **Construction** (port of `PY/metagraph.py:238-291`): `view.pinned()`, one `SubnetInfoRuntimeApi.get_metagraph(netuid)`,
  and (by default, `commitments = true`) two `Commitments` map scans **pinned to the same block**. `None` when the subnet does
  not exist. Columns shorter than `hotkeys` are padded exactly as Python pads (`metagraph.py:311-336`). Commitments render
  with the same status machine (plain, sealed, revealed, failed), and Data fields are decoded through the core `Value` path
  (the custom `Data` SCALE codec with tags 0-137 is metadata-described: RES/C1 §1.3).
- **Mechanisms**: `metagraph_for(netuid, MechId)` uses `get_mechagraph`; `mechid = raw_netuid / 4096`.
- **Selective refresh**: `selective_metagraph(netuid, &[SelectiveMetagraphIndex])` and `selective_mechagraph`, using the enum
  generated from pallet source (§4.1). Preset index sets follow RES/C2 §5.2. The validator loop requests
  `[Block, Tempo, LastStep, BlocksSinceLastStep, WeightsVersion, WeightsRateLimit, ActivityCutoff, CommitRevealWeightsEnabled, CommitRevealPeriod, NumUids, Hotkeys, Axons, Active, ValidatorPermit, LastUpdate, Emission, Dividends, Incentives, BlockAtRegistration, TotalStake]`,
  which is about 55-60% smaller than a full metagraph and needs one stake pass (RES/C2 §5.2). The miner loop requests
  `[Hotkeys, ValidatorPermit, TotalStake, Validators]` for its admission policy. These presets are an rtao product addition:
  Python exposes neither as a read (RES/A2 §3.15). They are additive, outside the parity matrix.
- **Lite and full neurons**: `neurons(netuid, lite)` uses `get_neurons_lite`/`get_neurons`. Weights and bonds come as sparse
  rows (`Vec<(u16, u16)>` per uid, normalized only on request), never as a dense n×n matrix; a 2,500-UID Null subnet would
  otherwise be 6.25M cells (RES/A2 §7).

### 8.3 Other typed models

`NeuronInfoLite`, `NeuronInfo`, `DynamicInfo`, `SubnetInfoV2`, `SubnetHyperparameters` (decoded from
`get_subnet_hyperparams_v3`, a forward-compatible name/value list rendered through the extracted `HYPERPARAMS` kinds;
RES/C2 §1.1), `StakeInfo`, `DelegateInfo`, `DelegatedStake`, `SimSwapResult`, `AxonInfo` (u128 ip, ip_type 4/6, port,
protocol byte kept opaque with default 4, RES/D §5), `NeuronCertificate`, `ChainIdentityV2`, `SubnetIdentityV3`,
`LockState`, `MinerCollateral`, `CommitmentRegistration`, `Lease`, `Crowdloan`, `ProxyDefinition`, `MultisigState`,
`EpochStatus`, `ColdkeySwapAnnouncement`, and the basket/beta models used by the root reads.

Each model has: the registry type path it decodes from (checked by `check-models`), a decode test against `golden.json`
`runtime_calls`/`storage_values` raw bytes where a vector exists (spec 461), and a decode test against rtao's recorded
current-spec fixtures (§11.1). Dead fields documented by RES/C2 §0.4 are `#[deprecated]` and absent from docs.

### 8.4 Sync cadence and persistence

```rust
pub struct MetagraphSync { … }
impl MetagraphSync {
    pub fn spawn(client: Client, netuid: NetUid, mech: Option<MechId>, cadence: Cadence, select: Selection) -> (Self, watch::Receiver<Arc<Metagraph>>);
    pub fn changes(&self) -> broadcast::Receiver<MetagraphChange>;   // UidReplaced{uid, old, new} -> score reset hook; AxonChanged; PermitChanged
}
pub enum Cadence { EveryEpoch { offset_blocks: u32 }, EveryBlocks(u32), Interval(Duration) }   // EveryEpoch keys off LastEpochBlock advancing
pub enum Selection { Full, Selective(&'static [SelectiveMetagraphIndex]) }
```

The defaults follow SN2's tested numbers (600 s for miners, 3600 s for validators; RES/G §7.3), overridden to `EveryEpoch`
when a validator uses the weight setter. After N consecutive failures the sync keeps serving the last good snapshot and
emits a health event, never an empty metagraph (RES/E §4.2.5: no silent defaults).

**Persistence.** Python v11 dropped `save()`/`load()` ("persist `mg.raw` yourself", `MONO/docs/migration.mdx:290-317`).
rtao offers `Metagraph::save(path)` and `load(path, &RuntimeCache)` with format `RTMG` magic, then
`{genesis, block_hash, block_number, spec_version, netuid, mechid}`, then the **raw SCALE** bytes of the runtime-API
response, written atomically. Loading re-decodes with the runtime of `spec_version` from the shared disk cache (§3.3), so
the file outlives model changes. Pickle/torch files from v10 are not read (RES/I §0.6).

---

## 9. Framework for owners, validators, miners (`rtao-neuron`)

v11 deliberately ships no framework (RES/G §6.1), so nothing upstream exists to reuse; this crate is rtao-owned. To keep
it small, it composes intents, reads and btauth, re-uses upstream guidance (`MONO/docs/guides/{validating,mining/index,subnets,null-consensus}.mdx`)
as its specification, and adds no protocol of its own.

### 9.1 Runners

```rust
pub trait Validator: Send + 'static {
    type State: Serialize + DeserializeOwned + Default + Send + Sync;
    fn on_start(&mut self, cx: &NeuronCx, state: &mut Self::State) -> impl Future<Output = anyhow::Result<()>> + Send { async { Ok(()) } }
    /// Score miners; called on the cadence the runner derives from the epoch clock.
    fn step(&mut self, cx: &NeuronCx, mg: &Arc<Metagraph>, state: &mut Self::State) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Relative weights per mechanism; the runner conforms and submits them (§9.2).
    fn weights(&self, cx: &NeuronCx, mg: &Metagraph, state: &Self::State) -> anyhow::Result<MechWeights>;
    fn on_uid_replaced(&mut self, _uid: Uid, _state: &mut Self::State) {}   // reset scores (MetagraphChange::UidReplaced)
}
pub struct ValidatorRunner<V: Validator> { client: Client, wallet: Arc<dyn WalletLike>, cfg: NeuronConfig, v: V, store: StateStore<V::State> }
impl<V: Validator> ValidatorRunner<V> { pub async fn run(self, shutdown: CancellationToken) -> anyhow::Result<()>; }

pub struct MinerRunner { client: Client, wallet: Arc<dyn WalletLike>, cfg: NeuronConfig }
impl MinerRunner {
    /// Serves `router` behind BtAuthLayer + MetagraphPolicy + PerHotkeyRateLimit, keeps the axon published, and drains on shutdown.
    pub async fn serve(self, router: axum::Router, shutdown: CancellationToken) -> anyhow::Result<()>;
}
pub struct OwnerTools { exec: Executor, wallet: Arc<dyn WalletLike> }   // §9.3
```

Each runner is a `JoinSet` of tasks: metagraph sync, epoch clock, the weight setter or axon publisher, the user's step loop,
and a health/metrics ticker. Shutdown comes from SIGTERM or ctrl-c through the `CancellationToken`; in-flight weight
commits get up to one block to land, then state is persisted. State goes through `StateStore<T>` (versioned, netuid-tagged
JSON or bincode, atomic temp+fsync+rename) under `~/.bittensor/<subnet>/<wallet>/<hotkey>/netuid<N>/`, the convention seen
in SN2 and the template (RES/G §7.18). Health is a single key=value log line, matching the user's own `mev-scanner` style
(RES/E §6). With feature `metrics` the runners also export Prometheus metrics.

### 9.2 Weights: conformance, commit-reveal, cadence

**Conformance** is ported from `PY/intents/weights.py:54-114` and runs inside the `SetWeights` intent, so the runner, the
CLI and the oracle share one implementation:
- clip to `MaxWeightsLimit / 65535` of the row total with mass redistribution (`clip_to_max_weight`, identical to v10's
  `normalize_max_weight`: RES/I §0.3);
- max-upscale to u16 and drop zeros (the chain does the same upscale for Yuma, `epoch/math.rs:78-129`, RES/C1 §2.1);
- enforce `min(MinAllowedWeights, SubnetworkN)` unless the row is a single self-weight;
- with `raw_u16`, Null subnets keep exact integers and nothing is scaled (`PYT/unit/test_null_weights.py:13-45`, ported as
  vectors);
- `version_key` defaults to 0 (Python); the runner warns when it is below `WeightsVersionKey`.

**Preflight** reads at one pinned block: `Uids`, `CommitRevealWeightsEnabled`, `WeightsSetRateLimit`,
`LastUpdate[NetUidStorageIndex(netuid, mech)]`, `MinAllowedWeights`, `MaxWeightsLimit`, `SubnetEpochConsensus`, and
`ValidatorPermit` under Null. It raises the same client-side `ChainError` codes Python raises (`weights.py:154-215`).

**Path selection.** If `CommitRevealWeightsEnabled` (default true on chain, RES/C2 §0.7), use
`commit_timelocked_mechanism_weights(netuid, mecid, commit, reveal_round, commit_reveal_version)`. Otherwise use
`set_mechanism_weights`. The legacy `set_weights` call is never used, as in Python (`CAT/intents.json` `set_weights.wraps` [V]).

**Timelock commit**, with every piece of crypto from the core:
1. read `Tempo`, `RevealPeriodEpochs`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex`, `BlocksSinceLastStep` and
   the head, all at one block, plus `block_time` (`Aura.SlotDuration`; 0.25 s on fast-runtime localnets);
2. `rtao_core_ext::epoch_schedule::predict_first_reveal_block` (the pallet-faithful predicate, G5), then
   `target = first_reveal + SECURITY_BLOCK_OFFSET` (3) and
   `round = floor((now + (target − head) × block_time − GENESIS_TIME) / 3)`, minimum 1. Constants come from
   `CORE/timelock/constants.rs`; the clock is injectable;
3. `WeightsTlockPayload { hotkey: pubkey bytes, uids, values, version_key }.encode()` (`CORE/timelock/mod.rs:53`, the
   core's freeze_struct-guarded type) → `encrypt_and_compress(payload, round)` (`:104`);
4. `commit_reveal_version` is read from `CommitRevealWeightsVersion` (currently 4) instead of being hard-coded
   (RES/E §4.1.4). The intent parameter, which defaults to 4 in Python's `commit_weights`, overrides it when given. When
   root bumps the storage value, rtao follows it while Python's default would be rejected (`IncorrectCommitRevealVersion`).
   This is listed as an `rtao-stricter` divergence; with storage at 4, the bytes are identical.

The chain reveals automatically at the first block of epoch `commit_epoch + RevealPeriodEpochs` (RES/C1 §2.3). The
runner watches for `TimelockedWeightsCommitted` and later `TimelockedWeightsRevealed(netuid_index, who)` by name, and
reports reveal failures as health events. The `MaxEpochsPerBlock` deferral is not modeled, upstream or here; the +3 block
offset absorbs small deferrals (RES/C1 §10.4).

**Cadence** (`WeightSetter`, per mechanism `0..MechanismCountCurrent`):
- submit once per tempo, early in the tempo (flat priority, so tips buy nothing; congestion clusters at tempo boundaries:
  RES/C1 §7.3);
- respect `WeightsSetRateLimit` against `LastUpdate[netuid_index]`, since each mechanism has its own slot;
- keep at most one pending commit per (hotkey, mechanism), because the pool `provides` tag rejects a second (RES/C1 §2.2);
- after `FateUnknown` or a lost watch, reconcile by reading `TimelockedWeightCommits[netuid_index][epoch]` / `LastUpdate`
  before resubmitting (the SN2 reconcile pattern, RES/E §4.1.3);
- skip an all-zero row: hold the previous weights rather than submit an empty vector (RES/G §7.4).

### 9.3 Serving, registration, hyperparameters, timelock

- **`AxonPublisher`**: the external IP comes from config (`--axon.external-ip`, env `BT_AXON_EXTERNAL_IP`) or detection. It
  compares with `Axons[netuid, hotkey]` and calls `ServeAxon` only when ip, port, protocol or version differ **and**
  `ServingRateLimit` has elapsed since `AxonInfo.block` (RES/C1 §3). IPv4 and IPv6 are both supported (the chain accepts
  `ip_type` 6; SN2 does not, RES/G §8.9). `ResetAxon` is optional on shutdown. `ServeAxonTls` takes a certificate of at most
  65 bytes, because longer ones are silently dropped on chain (RES/C1 §3); rtao rejects them client-side with an
  explanatory error, a documented strictness addition.
- **Registration**: `ensure_registered(netuid, Mode::Burned { max_cost: Tao } | Mode::Pow { workers, timeout, backend })`.
  - Burned: preview the burn/lock split from `Burn` and `CollateralLockShare` (port of
    `PY/intents/registration.py:27-44`); submit `BurnedRegister`, which is shielded automatically because
    `mev_shield_required`; confirm with the `NeuronRegistered` event and the `uid` read.
  - PoW: build the prefix as `PY/pow_registration.py:19-33` does (`"subtensor-pow-register-v1" ‖ netuid u16 LE ‖ block_hash ‖
    hotkey ‖ coldkey`, 123 bytes; RES/B App. A), call the core's `pow::mine_cpu`/`GpuMiner`, refresh the work block, which
    must be at most 5 blocks old, then submit `PowRegister`.
  - Immunity and pruning awareness through the `immunity_period`, `metagraph` and `BlockAtRegistration` reads.
- **`OwnerTools`**:
  - `register_subnet(hotkey)` with the queued-registration follow-up (§3.11);
  - `start_call(netuid)`, after checking `subnet_start_schedule`;
  - `set_hyperparameter(netuid, name, HyperValue)`, using the extracted owner-setter map. It is admin-window aware: refused
    client-side when `blocks_until_next_epoch < AdminFreezeWindow` (RES/C1 §5.2), with Python's error code;
  - rate-limit aware: `LastRateLimitedBlock` against `OwnerHyperparamRateLimit × Tempo`;
  - identity, symbol, mechanism count and emission split, trims, and a dashboard read that surfaces both "on" flags
    (`subnet_is_active`, `subnet_emission_enabled`: RES/G §1.4).
- **Timelock UX**: `rtao_tx::timelock::{encrypt(data, n_blocks | round), decrypt(envelope)}`. These are the core's
  `encrypt_at_round`/`reveal_round`/`decrypt_with_signature` plus rtao's async drand fetch from the core's
  `DRAND_ENDPOINTS`. They are used for commitments (`Commitments.set_commitment` with a `TimelockEncrypted` field, raw-only in
  Python's coverage list, so exposed as a typed helper plus `submit_call`).

### 9.4 Config and CLI conventions

```rust
#[derive(clap::Args, serde::Deserialize)]
pub struct NeuronConfig {
    #[arg(short = 'n', long = "network", env = "BT_NETWORK", alias = "subtensor.network", default_value = "finney")] pub network: String,
    #[arg(long, env = "BT_CHAIN_ENDPOINT", alias = "subtensor.chain_endpoint")] pub chain_endpoint: Option<Url>,
    #[arg(short = 'w', long = "wallet", env = "BT_WALLET", alias = "wallet.name")] pub wallet: String,
    #[arg(short = 'H', long = "wallet-hotkey", env = "BT_WALLET_HOTKEY", alias = "wallet.hotkey")] pub hotkey: String,
    #[arg(long = "wallet-path", env = "BT_WALLET_PATH", alias = "wallet.path")] pub wallet_path: Option<PathBuf>,
    #[arg(long = "wallet-password-file", env = "BT_WALLET_PASSWORD_FILE")] pub password_file: Option<PathBuf>,
    #[arg(long)] pub netuid: NetUid,
    #[arg(long, value_delimiter = ',')] pub fallback_endpoints: Option<Vec<Url>>,
    #[arg(long, value_delimiter = ',')] pub archive_endpoints: Option<Vec<Url>>,
    #[arg(long)] pub retry_forever: bool,
    #[arg(long = "axon.port", default_value_t = 8091)] pub axon_port: u16,
    #[arg(long = "axon.external-ip", env = "BT_AXON_EXTERNAL_IP")] pub axon_external_ip: Option<IpAddr>,
}
```

- Env names are v11's (`BT_NETWORK`, `BT_CHAIN_ENDPOINT`, `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`,
  `BT_WALLET_PASSWORD`, `BT_WALLET_PASSWORD_FILE`; `MONO/docs/migration.mdx:162-178`, RES/A1 §7). v10 dotted flag names
  are clap aliases, the convention SN2 uses (RES/I §3.4).
- Precedence: flag > env > `~/.bittensor/btcli.json` (or `$BTCLI_CONFIG`) > default. rtao reads Python's config file and
  its keys (`network`, `wallet`, `wallet_hotkey`, `wallet_path`, `fallback_endpoints`, `archive_endpoints`,
  `retry_forever`, `proxy_for`, `mev_shield`; `PY/config.py:12-59`, RES/A1 §7), so one config serves both CLIs.
- Network names map to endpoints through the extracted `NETWORKS` table (`finney`, `test`, `archive`, `devnet`, `local` →
  `$BT_CHAIN_ENDPOINT` or `ws://127.0.0.1:9944`). Raw `ws(s)://` URLs are accepted; unknown names are an error.
- `axon.port` and `axon.external-ip` are rtao neuron flags; v11 has no axon server, so they have no Python equivalent.

---
## 10. CLI (`rtao-cli`, binary `rtao`)

### 10.1 Generated from the same registries the library uses

Python generates `btcli tx` from the intent registry and `btcli query` from the read registry (`PY/cli/tx.py:1-11,486-498`,
`PY/cli/query.py:1-6,254-266` [V]). rtao goes one step further. Its `tx`, `query`, `tools`, `explain` and `call`
subcommands are **built at runtime with clap's builder API from the generated specs**, so they need no generated Rust code
of their own:

```rust
fn tx_command() -> clap::Command {
    let mut cmd = Command::new("tx").about("Submit transactions (generated from intents)").subcommand_required(true);
    for spec in rtao_catalog::INTENTS.iter() {                     // 83 at v477
        cmd = cmd.subcommand(
            Command::new(spec.op.replace('_', "-")).about(spec.summary).long_about(spec.description)
                .next_help_heading(help_panel(spec))            // pallet; "Root (chain sudo)"; "<pallet> — subnet owner" (tx.py:474-484)
                .args(args_from_schema(spec.schema()))           // one --flag per property; required set; anyOf money → accepts "all" when allowed
                .args(global_tx_args()));                        // --dry-run --json --yes --mev-shield/--no-mev-shield --proxy-for --period …
    }
    cmd
}
// execution: matches → serde_json::Value → AnyIntent::from_tool(op, json) → Executor::plan → (review|--yes) → execute
```

- **Address flags.** `*_ss58` properties take ss58 strings, wallet names (`-w name` / `wallet/hotkey`) or the address book
  (`$BTCLI_ADDRESSES_PATH` or `~/.bittensor/btcli_addresses.json`, `PY/config.py:14,212` [V]; proxies and multisigs use
  `btcli_proxies.json` and `btcli_multisigs.json` in the same directory, `:13,15`). This is ported from `PY/cli/context.py` `address_cli_name` and
  related helpers.
- **MEV shield precedence**: required → `--mev-shield`/`--no-mev-shield` → config `mev_shield` → the intent's
  `mev_shield_default`. Shielding is downgraded when free TAO cannot cover the carrier fee (`PY/cli/context.py:260-292,1047-1080`,
  RES/A1 §6.5).
- **Query commands**: `query` builds one subcommand per read, grouped by `category`. Options come from `params`/`param_docs`
  (the vocabulary `string | integer | number | boolean | array`). Tables follow the `render` hints (`Grouped`, `Matrix`), and
  `--json` follows `_jsonable` (§4.5). `--block <n>` (an rtao addition; btcli has none) runs the read on a `Snapshot`.
- **`tools`** prints the catalog projection (`list_tools`, `PY/intents/registry.py:43-66`), plus an rtao `rust` object per
  entry (§13). **`explain <code|ChainErrorName>`** prints the code, description, remediation and docs URL from the
  generated error tables. **`call Pallet.function --args …`** is a raw call: it requires `--allow-raw`, mirroring
  `allow_raw_calls`, and composes through the core with the same wrapper nesting as intents (`PY/executor.py:107`).
- **Global flags** are a port of `PY/cli/globals.py`: `--network/-n`, `--wallet/-w`, `--wallet-hotkey/-H`, `--wallet-path`,
  `--wallet-password-file`, `--json`, `--yes/-y`, `--no-prompt`, `--dry-run`, `--quiet/-q`, `--verbose/-v`, `--proxy-for`,
  `--force-proxy-type`, `--signer`, `--signer-address`, `--signatory`, `--ledger`, `--ledger-account`, `--ledger-index`,
  `--extension-bridge|browser|source`, `--keychain-password`, `--macos-password`, `--fallback-endpoints`,
  `--archive-endpoints`, `--retry-forever` (RES/I §4.2). Precedence: flag > env > btcli.json > default. v9 shorthands are
  hidden aliases (`PY/cli/main.py:118-135`).
- **`doctor`** is an rtao addition with no btcli equivalent. In a downstream workspace it checks that `Cargo.lock`
  resolves `w3f-bls` from the same monorepo tag as rtao's `bittensor-core` pin, and prints the exact `[patch]` line if
  not. It also reports the toolchain against rtao's MSRV, endpoint reachability and genesis per network, and the connected
  spec against `compat.json`.
- **Non-interactive discipline**: without a TTY and without `--yes`, a mutation is declined with exit 1; missing required
  options exit 2. JSON goes to stdout and logs to stderr (RES/G §4.5; `MONO/docs/migration.mdx:670-674`).

### 10.2 Hand-written groups (ported from `PY/cli/commands/*.py`)

`wallet` (1,784 lines in Python: create/new-coldkey/new-hotkey/regen-*/unlock/show/list/balance/overview/registrations/
history/transfer/inspect/sign/verify/encrypt/decrypt/identity/associate-hotkey/swap-hotkey/swap-coldkey/swap-check/
announce-coldkey-swap, keychain save|show|delete), `stake` (aliases of intent commands plus show, list, auto, set-auto,
wizard, child, children), `root`, `subnets` (list, show, hyperparameters, burn-cost, create-cost, price, conviction,
register, create, metagraph), `hparams` (alias `sudo`), `conviction`, `weights` (commit, reveal, set), `axon` (set, reset),
`collateral`, `proxy` (plus `proxy book`), `multisig`, `addr`, `crowd`, `timelock` (encrypt, decrypt, show), `config`
(set, get, unset, clear, path), `extension`, `evm`, `upgrade pending`, `misc` (convert, latency). The command tree at the
pin is dumped from the Python Typer app by `tools/oracle/dump_cli.py`, and the parity matrix tracks every command and flag
(§4.7).

### 10.3 Scope per milestone

| Milestone | CLI scope |
|---|---|
| M2 | `query <read>` for the implemented reads; `tx <op> --dry-run/--json/--yes` for the ~12 core intents; `explain`; `tools`; `wallet create/new-coldkey/new-hotkey/regen-*/list/show/balance`; `config` |
| M4 | all 83 `tx` commands; `call`; `weights`; `axon`; `hparams`; `stake` aliases; `subnets register/create/metagraph` |
| M5 | all 102 `query` commands; `subnets` read views; `root`; `conviction`; `collateral` |
| M6 | `--ledger`, `--signer vault` / `--signer extension`; `extension`; `evm`; `multisig`, `proxy` (incl. book); `timelock` |
| M7 | everything remaining in the dumped Typer tree (`wallet overview/history/inspect/sign/verify/encrypt/decrypt`, `stake wizard`, `upgrade pending`, `misc`); parity gate flips to zero-missing |

Human-readable tables need not be byte-identical to `rich` output. JSON output and exit codes must match, and the
interop lane (§11.6) checks them.

---

## 11. Testing

### 11.1 Unit tests driven by upstream fixtures (vendored verbatim, sha256-pinned)

| Upstream file | What rtao tests with it |
|---|---|
| `PYT/fixtures/golden.json` (1,508,346 B; spec 461; embeds raw V15 and V14 metadata: RES/A2 §6.1) | (a) **argument-conversion tests**: the 10 `calls` re-encoded through rtao's typed call builders (§3.6) must equal `data_hex`; the `storage_keys` through typed descriptors must equal `key_hex`, for the items still present in the pinned metadata (`SubtensorModule.Alpha` was removed after 461: RES/H §0.4); the 2 `signature_payloads` and 2 `extrinsics` through rtao's `TxParams` construction (era dict, nonce, tip) must be byte-equal; the 3 `multisig` derivations and 4 `ss58` vectors through `rtao-types`. (b) **FakeChain replay**: `storage_values`, `query_maps` (`raw_keys` and `raw_changes`), `runtime_calls` (`raw_hex`), `events`, `block` and `receipt` are served by the FakeChain at the golden metadata. rtao's dynamic reads must reproduce `decoded` (via `to_corpus_json`), its receipt resolution must reproduce the outcome and fee, and the typed models must decode the same bytes (`get_neurons_lite`, `get_subnet_hyperparams`, `get_stake_info_for_hotkey_coldkey_netuid`). Loader: strip the `Option<OpaqueMetadata>` wrapper and `Runtime::parse(v15, 461, 1, 42)`, exactly as `CORE/codec/mod.rs:80-109` does (RES/H §1.5) |
| `PYT/fixtures/shape_corpus/corpus.json` (797/802 types, 2,899 samples) | the core replay (`decode_id` + `to_corpus_json`) re-run against the **pinned** core, as a cheap consistency check of the pin; plus rtao's own newtype and fixed-point `DecodeAsType` impls run on every corpus sample whose type path matches (`NetUid`, `AccountId32`, `substrate_fixed::*`, `PerU16`, …), compared against a projection of the core's decode |
| `MONO/sdk/bittensor-core/fixtures/ledger_proof_vector.json` | `LedgerSigner` request construction: the parts, unhashed payload and proof bytes rtao passes to `LedgerDevice::sign` must equal the vector (device mocked at the `LedgerDevice` boundary) |
| `CORE/timelock/epoch_schedule_vectors.rs` (86 lines) | the staged `epoch_schedule` must reproduce all upstream vectors, plus new vectors for the `BlocksSinceLastStep > tempo` case derived from the pallet's tempo-control tests (G5) |
| `PYT/unit/test_http_auth.py:19-36` + `MONO/docs/guides/signed-requests.mdx:252-289` | btauth/1 golden payload, pinned ed25519 signature, sr25519 sign→verify, and every Python negative case by name (§7.1) |
| `PYT/unit/test_keyfiles_golden.py` (`//Alice`, the `abandon…about` mnemonic, legacy Fernet parameters, 0600/0700) | keyfile round-trips through the core, plus **new committed encrypted vectors** produced once by `btcli wallet create` in the oracle container (NaCl with a known password; legacy Fernet via the Python test's parameters). Upstream has none (RES/B §8 gap) |
| `CAT/{intents,reads,errors}.json` | generation inputs, plus a test that every generated spec round-trips to the catalog JSON |
| `PY/_generated/{calls,storage,constants,runtime_apis,errors}.py` | **codegen cross-check**: parse them (extractor) and assert rtao's generated descriptors agree on pallet indices, call names and argument names/idents, storage names and value idents, and the `(pallet_index, error_index) → name` table. Both are derived from metadata at the same tag, so any difference is an rtao codegen bug |
| `PYT/unit/test_null_weights.py`, `test_slippage_protection.py`, `test_root_claim_fee.py`, `test_lock_ownership_gate.py`, `test_hyperparams_units.py`, `test_fee_filters.py`, `test_multisig_safety.py`, `test_prepare_attach.py`, `test_wallet_compat.py`, `test_v446_accounting_and_commitments.py`, `test_balance_properties.py` | ported test-by-test as Rust tests named after the Python test, with `// pyt:` provenance comments; where a test is table-shaped, its tables are extracted rather than copied |

### 11.2 FakeChain (the `FakeSubstrate` equivalent, one layer lower)

Python fakes the `Substrate` protocol (`PYT/harness/fake_substrate.py`), so Python's transport goes untested by it. rtao
fakes the **RPC** layer with `subxt-rpcs`'s `MockRpcClient` (`mock-rpc-client` feature; builder with per-method and
subscription handlers, `REG/subxt-rpcs-0.51.1/src/client/mock_rpc_client.rs:54-171` [V]), so `rtao-chain` (runtime
cache, key building, decoding, submission state machine) runs for real in unit tests:

```rust
let chain = FakeChain::builder()
    .metadata(Metadata::Pinned)                        // vendor/metadata/finney/477.v15.scale (or golden 461)
    .defaults(FakeDefaults::python())                  // extracted DEFAULT_STORAGE / DEFAULT_CONSTANTS / DEFAULT_RUNTIME
    .seed("SubtensorModule", "Uids", (1u16, BOB_HOT), json!(0))      // JSON in Python shapes → core encode_id(value_type) → bytes
    .seed_runtime("StakeInfoRuntimeApi", "get_stake_info_for_coldkey", json!([]))
    .on_submit(Scripted::in_block_then_finalized(events!["System.ExtrinsicSuccess", "TransactionPayment.TransactionFeePaid" => {...}]))
    .fail(Failure::DropAfter(3) | Failure::Refuse(-32004) | Failure::Silence(Duration::from_secs(61)))   // fake_node.py failure modes
    .build();
let client = Client::from_rpc(chain.rpc()).await?;
```

Seeds are written as **Python-shaped JSON** and encoded with the core's lenient encoder (ss58 strings for accounts, variant
names for enums; `CORE/codec/encode.rs:1-5` [V]). Python's fake defaults and samples therefore load without translation.

### 11.3 Property tests and fuzzing

`proptest` covers:
- `Tao`/`Alpha` arithmetic and decimal parsing (no float drift; mixed netuids error);
- fixed-point `to_f64` against bits/2^F, and `integer_part`;
- `NetUidStorageIndex` round-trips;
- weights conformance invariants: sum preserved after clipping, max ≤ limit, upscale maximum 65535, no zeros, raw_u16 left
  untouched;
- btauth payload canonicalization (method case, percent-encoded targets, no trailing newline, nonce bounds exactly at
  `max_age` and `allowed_skew`);
- `era_birth` against the core's `encode_era_value` for random period and current;
- `IntoCoreValue` round-trips through `encode_id`/`decode_id` for every newtype.

`cargo fuzz` targets (nightly smoke, as SN2 runs them, RES/H §3.8): btauth header parsing and verify, RPC JSON response
parsing, `UnsignedExtrinsic` JSON import, keyfile detection, and SelectiveMetagraph decoding. Fuzzing the core's own decoders
is upstream's job, but `decode_extrinsic` and `decode_id` are wrapped in one rtao fuzz target to catch panics at the
boundary.

### 11.4 Localnet e2e (docker)

- **Images.** `ghcr.io/raofoundation/subtensor-localnet:<tag>@<digest>` from `upstream.toml`. Tags `v<spec>` exist for
  mainnet releases (v432, v473, v475, v477); moving tags `mainnet`, `testnet`, `devnet`, `main`/`latest`; per-commit
  `sha-<40hex>` (RES/H §1.3, RES/F §6). `CMD True` gives fast blocks (250 ms, tempo 10); `False` gives 12 s blocks (RES/H §1.3).
- **Harness** (`rtao-testkit::Localnet`), a port of `MONO/sdk/bittensor-core/tests/e2e_support/mod.rs` conventions:
  - `docker run --rm -d -p 127.0.0.1::9944 -p 127.0.0.1::9945 <image> True`, then `docker port`;
  - readiness when `docker inspect` reports healthy (the image healthcheck needs `chain_getHeader` > 0) **or** the logs
    contain `Imported #1` (180 s cap);
  - env contract copied verbatim so upstream docs apply: `E2E_ENDPOINT` (reuse a running node), `LOCALNET_IMAGE`,
    `SKIP_PULL`;
  - unlike upstream, rtao never defaults to a non-existent tag (upstream's default `monorepo-sdk` does not exist: RES/H §6.1).
- **Isolation.** One container per test file, with tests in a file sharing it and using distinct dev accounts. This is
  cheaper than upstream's one container per test (112 boots). Timing-sensitive tests run in a separate `False` lane.
- **Accounts.** Alice, Bob and Charlie have 1,000,000 TAO; Alice is sudo; subnets 0 and 1 exist at genesis; the `faucet`
  call exists only on localnet (RES/H §1.3). Localnet metadata is not mainnet metadata (faucet at call 60, fast constants:
  RES/H §1.1), so e2e never regenerates descriptors from it.
- **Test inventory.** The upstream manifest (`MONO/sdk/bittensor-core/tests/e2e-manifest.json`, 112 names: RES/B §5) is the
  checklist; `parity/e2e_map.toml` maps each upstream e2e name to its rtao test. rtao-specific tests:
  1. CR v4 commit observed as `TimelockedWeightsRevealed` (both lanes);
  2. plain `set_mechanism_weights` with CR disabled through sudo;
  3. mechanism count 2 with per-mechanism rate limits;
  4. `serve_axon` idempotence and rate limit;
  5. metagraph sync across epochs;
  6. btauth/1 between two rtao processes;
  7. shielded `burned_register` (inner receipt resolved) and a shielded `add_collateral`;
  8. `register_subnet`, immediate and queued (queued forced by lowering `SubnetLimit` through sudo);
  9. multisig 2-of-3, pure proxy, announced proxy;
  10. `batch_all` atomic revert;
  11. raw-call policy refusal;
  12. offline prepare → sign → `submit_signature`, refused across a runtime upgrade;
  13. **runtime upgrade mid-session**: take the fast-runtime wasm from a newer localnet image
      (`/target/fast-runtime/release/node_subtensor_runtime.compact.compressed.wasm`, RES/H §1.3) and apply it with sudo
      `System.set_code` on a `:v<pinned>` localnet. rtao must pick up the new runtime and keep signing. Both wasms are
      fast-runtime, so the block time is unchanged [I: assumes the newer spec's migrations run cleanly on fresh localnet
      state];
  14. PoW registration with lowered difficulty (non-fast lane);
  15. differential reads against the core's blocking client (§1.5).

### 11.5 The Python oracle (offline, deterministic, per pin)

The strongest reuse in this design: the Python SDK at the pinned tag is the specification, executed.

- **Build.** `ci/oracle/Dockerfile` checks out the monorepo at `upstream.toml.commit` and runs `uv sync` in `sdk/python`
  (which builds `bittensor-core-py` from the same commit via its `[tool.uv.sources]` path dependency: RES/B §7). The oracle's
  core is byte-identical to rtao's.
- **`tools/oracle/dump_intents.py`** imports `bittensor.intents.REGISTRY`, `PYT/harness/samples.INTENT_SAMPLES` and
  `FakeSubstrate`. For each of the 83 ops it builds the intent from its sample, runs `Client.plan` and `preflight` against a
  fresh `FakeSubstrate` (plus the per-test seeds the Python table test uses: `PYT/unit/test_intents_table.py:28-60` [V]), and
  writes `vendor/oracle/intents.json`: `{op, args, composed_call: (module, function, params) tree, effects, warnings,
  violations, fee, spend, required_free, extras}`. `FakeSubstrate.compose` returns the structural `(module, function,
  params)` tree rather than SCALE bytes (`fake_substrate.py:314-318` [V]), so the comparison is semantic and the core
  separately guarantees encoding.
- **`dump_reads.py`** does the same for the 102 reads with `READ_SAMPLES`, at head and pinned at block 90, as
  `PYT/unit/test_reads_table.py:100-110` does, writing `vendor/oracle/reads.json` (results through `_jsonable`).
- **Rust side.** `rtao-tx/tests/oracle_intents.rs` and `rtao-reads/tests/oracle_reads.rs` seed FakeChain with the same
  defaults and seeds, run rtao's plan and preflight and reads, and compare:
  - composed call trees exactly, with the timelock ciphertext and reveal round excluded, since they are randomized or
    clock-derived;
  - violations, effects and warnings exactly;
  - spend and fee exactly;
  - read JSON as parsed values (§4.5).
  A mismatch prints both sides and the `// py:` source range.
- **Regeneration** happens only in pin-bump PRs, so the diff of `vendor/oracle/*.json` between tags is itself a readable
  changelog of Python behavior changes.

### 11.6 Python interop e2e (docker compose, nightly and on pin bump)

`ci/interop/compose.yml` runs the localnet (`:vNNN` digest), the oracle image (provides `btcli` and `python -m bittensor`)
and the rtao test binary. Scenarios:
1. **btauth/1 both directions × {sr25519, ed25519}**. An rtao client calls a Python FastAPI miner built verbatim from the
   recipe at `signed-requests.mdx:104-157`, and a Python httpx client calls an rtao axum server. Cases: GET with empty body,
   POST JSON, percent-encoded target, unbound receiver with and without opt-out, stale, future, replay, tampered body. Status
   codes and `detail` strings are asserted.
2. **v11 miner ↔ Rust validator**. A Python miner (FastAPI plus `bt.http_auth.verify`) is registered on the localnet subnet;
   the rtao `ValidatorRunner` queries it over btauth/1, scores it and commits timelocked weights. Python's
   `bt.Subtensor(...).subnets.metagraph(netuid)` must then show the incentive. The reverse direction pairs an rtao
   `MinerRunner` with a Python validator script.
3. **Wallet interop**. `btcli wallet create --n-words 12` (with a password via `BT_WALLET_PASSWORD`, and `--no-password`)
   writes keyfiles that rtao loads and signs with. rtao-created wallets are read by `btcli wallet show`/`inspect`. The legacy
   Fernet and Ansible formats are decrypted.
4. **CLI JSON parity**. `btcli query` has no block flag (none in `PY/cli/query.py` or `PY/cli/globals.py` [V]), so pinning
   goes through the library. A Python driver calls `(await client.at(b)).read(name, **params)` for every `READ_SAMPLES`
   entry and renders with btcli's `_jsonable`. rtao runs `rtao query <read> --block b --json` (`--block` is an rtao
   addition). The two must be value-equal. A smoke pass also runs real `btcli query <read> --json` against
   `rtao query <read> --json` at head on the 12 s lane, for reads that are stable within a block, retrying once on mismatch.
   For every state-independent intent sample, the `--dry-run --json` plans must be equal.
5. **Shared caches**. The runtime disk cache and the token-symbol cache written by one tool are read by the other.

### 11.7 Benchmarks (`criterion`)

- `Runtime::parse` on the V15 blob (baseline 3.9 ms measured live; spec target 4 ms);
- `get_metagraph` decode, dynamic `Value` versus typed `DecodeAsType`, on recorded payloads for 256, 1,024 and 2,500 UIDs
  (`xtask record` captures finney/testnet responses read-only);
- `query_map` decode throughput (the core's spec reports 599k entries/s);
- sign plus assemble (spec: 0.03 ms);
- btauth sign and verify throughput;
- keyfile unlock time, a fixed cost reported for operator expectations.

The baselines are the core spec's acceptance table (`MONO/sdk/bittensor-core-spec.md:360-382`) and, side by side,
`sdk/python/scripts/bench_transport.py`/`bench_neurons.py` run in the oracle container against the same endpoint. CI gates
only on gross regressions (>30%).

### 11.8 Extraction tooling, and the zero-Python option

`tools/extract_py.py` is about 300 lines, uses only the stdlib, and runs with `python3 -I`. It parses the listed upstream
files with `ast`, evaluates only literal nodes (and `Name(literal…)` calls for `Hyperparam`), resolves dev-key symbols in
samples to ss58 through a fixed table, and writes `vendor/upstream/extracted/*.json` with the source file and line range of
each table. Rust tables (`transaction_error.rs`, `fee_filters.rs`, `metagraph.rs` index enum, `freeze_struct` hashes) are
extracted in `xtask` with `syn`. If the user wants no Python even in tooling, the Python half can move to a Rust Python
parser (`rustpython-parser`; crate status unverified) without changing the JSON contract. The oracle lanes are Python by
definition.

---
## 12. Upstream tracking

### 12.1 Signals and the job that watches each

| Upstream event | Signal | Job | Cadence |
|---|---|---|---|
| runtime deployed to devnet, testnet or finney | finalized `(spec_version, blake2_256(:code))` changed (the RPC sequence of `MONO/.github/scripts/mainnet-finalized-runtime.sh:19-26`, RES/H §2.1) | `metadata-drift` | every 6 h (`cron: "17 */6 * * *"`, upstream's smoke cadence: RES/H §3.3) |
| a new finalized mainnet tag `v<spec>` | finney runs `<spec>` **and** tag `v<spec>` exists | `pin-bump` | every 30 min |
| Python SDK adds or changes an intent, read or error | `website/.../catalog/*.json` on monorepo `main` differs from the pin | `catalog-early-warning` | nightly |
| Python behavior changes without catalog changes | files under the watched paths changed between the pinned tag and `main` | `upstream-file-watch` | nightly; in pin-bump PRs |
| bittensor-core API change | `core_surface` probe fails against `main` | `core-canary` (allowed failure) | nightly |
| btauth change | `PY/http_auth.py` or `docs/guides/signed-requests.mdx` changed (any change; upstream promises a version bump for incompatible ones: RES/D §1.8) | `upstream-file-watch` (highlighted) | nightly |
| localnet image republished | digest of `:mainnet`/`:testnet`/`:devnet` changed | `e2e-moving-tags` | nightly |
| staged code accepted upstream | pinned core exports a staged symbol | `staged-check` (in `pr-gates`) | every PR |

Watched paths (in `parity/source_map.toml`, each mapped to rtao modules): `sdk/bittensor-core/src/**`,
`sdk/python/bittensor/{intents,reads,_transport,cli}/**`, `sdk/python/bittensor/{executor,result,error_map,hyperparams,settings,fee_filters,metagraph,http_auth,signing,wallet,wallets,keyfiles,multisig,timelock,pow_registration,config}.py`,
`sdk/python/codegen/**`, `sdk/python/tests/{fixtures,harness}/**`, `common/src/{transaction_error,proxy}.rs`,
`runtime/src/{fee_filters,check_mortality,transaction_payment_wrapper}.rs`,
`pallets/subtensor/src/rpc_info/**`, `pallets/subtensor/src/coinbase/{run_coinbase,reveal_commits}.rs`,
`docs/guides/signed-requests.mdx`, `eco-tests/**` (third-party indexers' shape contract: RES/H §1.8).

### 12.2 CI job definitions (GitHub Actions; actions pinned by SHA)

```yaml
# .github/workflows/pr.yml
name: pr-gates
on: [pull_request, push]
jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - uses: actions-rust-lang/setup-rust-toolchain@<sha>   # honors rust-toolchain.toml, which `xtask pin` writes from upstream.toml
        with: { components: "clippy,rustfmt" }
      - run: sudo apt-get install -y perl   # openssl-src (core `host` → fernet → vendored OpenSSL) needs Perl IPC::Cmd (RES/B §6)
      - uses: Swatinem/rust-cache@<sha>     # caches ~/.cargo/git (the monorepo + polkadot-sdk fork clones are large)
      - run: cargo xtask import-upstream --verify-only      # sha256 of every vendored artifact matches upstream.toml
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo xtask lint-upstream                      # monorepo syn lints (subtensor-linting @ pinned tag)
      - run: cargo xtask core-surface                       # compile probe of every bittensor-core symbol rtao uses
      - run: cargo nextest run --workspace --all-features
      - run: cargo test --workspace --doc
      - run: cargo xtask regen --check                      # generated code == regenerated from vendor/
      - run: cargo xtask parity --check                     # §4.7 gates (compile-time gate already enforced by build)
      - run: cargo xtask staged --check                     # delete staged modules upstream has absorbed
      - run: cargo deny check && cargo audit                # same cargo-audit pin as upstream (0.22.2)
  e2e:
    needs: check
    strategy:
      fail-fast: false
      matrix:
        lane: [pinned, mainnet, testnet]      # devnet and main run nightly as allowed-failure
        blocks: [fast]
        include: [{ lane: pinned, blocks: slow }]
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - run: cargo xtask localnet-image --lane ${{ matrix.lane }} >> "$GITHUB_ENV"   # sets LOCALNET_IMAGE=<repo>@<digest>
      - run: cargo nextest run -p rtao-testkit --test e2e --profile e2e-${{ matrix.blocks }}
```

```yaml
# .github/workflows/drift.yml
name: metadata-drift
on: { schedule: [{ cron: "17 */6 * * *" }], workflow_dispatch: {} }
jobs:
  drift:
    strategy: { matrix: { network: [finney, testnet, devnet] } }
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - run: cargo xtask drift --network ${{ matrix.network }} --open-pr
        # 1. finalized head → (spec, :code hash); unchanged vs upstream.toml → exit 0
        # 2. fetch V15 + V16 via Metadata_metadata_at_version(15/16)
        # 3. semantic diff (pallet/call/event/error indices + names + arg idents; storage hashers/keys/value/default;
        #    constants; runtime-API names+versions+signatures; tx-extension identifiers + extra/implicit type shapes;
        #    extrinsic versions). Classify: none | additive | behavioural | breaking.
        #    "spec bumped, surface unchanged" is the common case (478: RES/CORRECTIONS.md #2) → update index only, auto-merge
        # 4. attach `subxt diff old new` verbatim (subxt-cli 0.51.1; it omits events/errors/indices: RES/H §2.3)
        # 5. regen descriptors from the new V15; run check-models; open/refresh PR "drift(<net>): <old>→<new>" labelled
        #    runtime-{additive,behavioural,breaking}; devnet/testnet PRs are "upcoming", finney "now"
```

```yaml
# .github/workflows/pin-bump.yml
name: pin-bump
on: { schedule: [{ cron: "*/30 * * * *" }], workflow_dispatch: { inputs: { tag: { required: false } } } }
jobs:
  bump:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - run: cargo xtask pin --auto     # finalized finney spec S; tag vS exists; S > pinned → proceed, else exit 0
      - run: cargo xtask import-upstream && cargo xtask extract && cargo xtask regen
      - run: cargo xtask oracle         # builds ci/oracle at the new commit; refreshes vendor/oracle/*.json
      - run: cargo xtask record --image ghcr.io/raofoundation/subtensor-localnet:vS   # upstream record_golden.py / record_shape_corpus.py at the tag → vendor/recorded/S/
      - run: cargo xtask porting-report > porting.md   # upstream diff vS_old..vS over watched paths → rtao modules
      - run: cargo nextest run --workspace --all-features
      - run: gh pr create --title "pin: v$OLD → v$NEW" --body-file porting.md --label pin-bump
```

Nightly: `catalog-early-warning`, `upstream-file-watch`, `core-canary` (build against `branch = "main"` with
`:main` localnet, allowed failure), `e2e-moving-tags` (devnet, main), `py-interop` (§11.6), `fuzz-smoke`, and `bench` (§11.7).

### 12.3 Recording current-spec fixtures with upstream's own recorders

Rather than writing a Rust recorder, `xtask record` runs **upstream's** `sdk/python/scripts/record_golden.py` and
`record_shape_corpus.py` (RES/A2 §6.1-6.2) from the oracle image, against `:v<spec>` localnet. The first refuses non-local
endpoints unless explicitly allowed, and submits a real `//Alice` transfer. Output goes to `vendor/recorded/<spec>/`. Since
the formats are upstream's, rtao's golden loaders run unchanged on spec-461 and current-spec files, and both are kept
(cross-spec decoding). A read-only finney pass records large metagraph payloads for benchmarks only.

### 12.4 Compatibility matrix

`compat.json` and `COMPAT.md` are generated by `cargo xtask compat` in every release PR:

| rtao | monorepo tag (core pin) | validated spec range | metadata pins | localnet digests | Python oracle | btauth |
|---|---|---|---|---|---|---|
| 0.1.0 | v477 | 477–478 | finney 477 (code 0x…), devnet 478 | v477@sha256:…, testnet@…, devnet@… | bittensor 11.3.x @ v477 (built from source) | btauth/1 |

"Validated" means the drift classification from the lowest to the highest spec contains no `breaking` entry, **and** the e2e
lanes passed on the listed digests. At runtime, `Client` compares the connected `(spec_version, metadata hash)` with
`compat.json` (compiled in). Outside the range it warns, keeps reads metadata-driven, and keeps signing on behind the
extension guard (§3.3).

### 12.5 Maintenance playbook

| Event | What happens automatically | Human work [I estimates] |
|---|---|---|
| spec bump, surface unchanged (common; 478 changed only `System.Version`: RES/CORRECTIONS.md #2) | drift job updates `upstream.toml` index, auto-merges | none |
| additive surface (new call, event, storage or API) | drift PR with regenerated descriptors; coverage gate fails until each new call is wrapped or declared raw-only | 15 min triage; ½ day if an intent or read is added |
| behavioural surface (event index moved, storage type or default changed, runtime-API version bump) | drift PR; `check-models` pinpoints affected models | ½–1 day |
| breaking (new non-empty tx extension, moved call or pallet index, changed args of a used call, `transaction_version` bump) | drift PR labelled `runtime-breaking`; the extension guard disables signing on the affected network until merged | 1–3 days; usually upstream changed the core too, so the fix is a pin bump |
| Python adds an intent or read | early-warning issue days before the tag; pin-bump PR fails to compile (new enum variant) until ported or listed pending; oracle JSON shows the expected behavior | ½–1 day per intent, 1–2 h per read |
| Python changes behavior only | porting report lists the changed `PY/` ranges and their rtao modules; the oracle diff fails the specific ops | per change |
| bittensor-core API change | `core_surface` names the broken symbol; `core-canary` saw it nightly beforehand | hours |
| btauth/1 change | watch alert; vectors fail | `btauth/2` side by side: sign the configured version, verify both; ½–1 day |
| subxt-rpcs or jsonrpsee breaking release | confined to `rtao-chain::transport` (rule §2.3.2); weekly dependency PR | ½–2 days |
| staged PR merged and tagged | `staged --check` fails; delete the staged module, re-export the core's | 1 h |

### 12.6 Release process

rtao is not published to crates.io. Releases are annotated git tags `rtao-vX.Y.Z`, with a changelog generated by
`release-plz`'s changelog mode or `git-cliff` [I: either works; no publish step]. Versioning follows SemVer on the
`rtao` facade's API. `cargo-semver-checks` runs against the previous tag to catch accidental breaks for downstream git
users. A runtime-breaking drift merge bumps the minor version (pre-1.0) or the major version (post-1.0) of the facade. Each
release PR regenerates `compat.json`. The pin is never bumped inside a patch release unless the drift class is `none` or
`additive`.

---

## 13. Documentation

- **rustdoc**: `#![deny(missing_docs)]` on every crate, and `#![doc = include_str!("../README.md")]` on the facade. Every
  example is a real test exported with docify (`#[docify::export]`) and embedded with `docify::embed_run!` when it runs
  offline (units, keys, btauth vectors, FakeChain reads, intent plans against FakeChain), or `docify::embed!` when it needs a
  node (localnet examples run in the e2e lane). The README is compiled from `README.docify.md` with
  `docify::compile_markdown!` (docify 0.4.1, RES/F §7). The same examples therefore appear on docs.rs-style pages, in the
  book and in the README, and none can rot.
- **mdbook** (`book/`) mirrors the bittensor.com taxonomy (RES/I §5.6a), so an agent that knows upstream's `llms.txt`
  navigates rtao's by analogy:
  - `index`, `quickstart` (cargo deps plus the patch line; connect; typed read; plan and execute);
  - `sdk` (the primitives in Rust: `Client`/`View`, `Wallet`/`Signer`, `Tao`/`Alpha`, intents, `ExtrinsicResult`/`ErrorCode`);
  - `agents` (upstream's eight-step flow with `rtao tools`, `execute_tool`, `Policy`, `--dry-run`, error codes:
    `MONO/docs/agents.mdx`, RES/I §5.5);
  - `cli`;
  - `guides` (validator, miner with btauth, owner tooling, commit-reveal and mechanisms, Null consensus, localnet testing,
    wallets and signers, migration from v10 Python, migration from bittensor-core's blocking client);
  - `concepts`, which **links** to upstream's raw-markdown pages (`https://www.bittensor.com/llms.mdx/docs/concepts/<slug>/content.md`)
    rather than duplicating them.
- **Generated reference** (`cargo xtask regen` writes `book/src/reference/`): `tx/<op>.md` (summary, signer, origin, wraps,
  a parameter table from the schema, the Rust struct and `Executor` usage, the `rtao tx` CLI line, the upstream page link and
  chain `sources`), `query/<read>.md`, `errors/<code>.md`, `errors/chain/<Name>.md` and `hyperparameters/<name>.md`, in
  upstream's page templates (RES/I §5.2) with `## Rust` in place of `## Python`. `regen --check` keeps them current.
- **rtao catalogs**: `catalog/{intents,reads,errors}.json` use upstream's schema verbatim plus a `rust` object per entry,
  e.g. `{"type": "rtao::tx::AddStake", "spec": "rtao_catalog::INTENTS[…]", "cli": "rtao tx add-stake …"}` or
  `{"method": "client.staking().stake_for_coldkey(&coldkey)", "returns": "Vec<StakePosition>"}`. `rtao tools` prints the
  same data. Tools that already consume upstream catalogs read rtao's unchanged.
- **`llms.txt` and `llms-full.txt`** are generated with upstream's conventions (RES/I §5.1): a "Searching these docs"
  preamble (fetch the index, grep the full corpus, fetch single pages as raw markdown, use the catalogs), a "Reference
  catalogs" block, the page tree with one-line descriptions, and a "Mapping to bittensor.com" block pairing every
  `tx/<op>`/`query/<read>`/`errors/<code>` with its upstream URL. `llms-full.txt` is every book page concatenated under
  `# <Title> (/docs/<slug>)` headers, as upstream does. A `SKILL.md`-style quick card for coding agents is generated from
  the CLI tree (the pattern btt and agcli use, RES/E §4.1.12).
- **Upstream changelog in docs**: every pin-bump PR appends to `book/src/upstream-log.md` the porting report summary and
  the oracle diff, giving users a readable record of which upstream changes reached rtao and when.

---
## 14. Milestones

[I] Effort is in person-weeks (pw) for one senior Rust engineer familiar with Substrate. Docs ship with each milestone.
Each milestone ends with a tag `rtao-v0.N.0`.

| # | Milestone | Deliverables | Exit criteria | Effort |
|---|---|---|---|---|
| M0 | **Scaffold and pin** | workspace and crate skeletons; `upstream.toml`; `xtask pin/import-upstream/core-surface/lint-upstream`; vendored artifacts with sha256 manifest; `rtao-types` (newtypes, `Tao`/`Alpha`, `AccountId` via core); `rtao-btauth` core (pure); `pr-gates` CI | golden `calls`/`storage_keys`/`signature_payloads`/`extrinsics`/`multisig`/`ss58` reproduced through rtao conversions plus the core; corpus replay passes on the pinned core; all btauth/1 vectors and negative cases pass; the monorepo lint set is clean | 1.5 |
| M1 | **First typed read against finney** | `rtao-chain` read path (`RpcSession` over subxt-rpcs with pool and reconnect, runtime cache including the shared disk cache, views and snapshots, query/query_map/query_batch/call/constant/events); `rtao-core-ext::ops::{metadata,storage,runtime_api}`; `ir` plus `regen` of chain descriptors; typed `MetagraphWire` and the v11 `Metagraph` projection; `rtao query metagraph`; FakeChain v1; differential test against the core's blocking client | `client.subnets().metagraph(NetUid(1))` on finney returns a typed metagraph that is field-equal to the core's `Value` decode of the same block; localnet e2e lane green on `:v477`; decode benchmarks (typed vs `Value`) recorded for 256/1,024/2,500 UIDs | 2.5 |
| M2 | **Submission and validator primitives** | `Signer` trait; keyfile wallets and password resolution; `NonceManager`; era, fee probe, submit/watch/resolve (`ops::receipt` with Python parity); generated error taxonomy; `Executor` (plan, policy, execute, retries); intents `transfer`, `transfer_all`, `burned_register`, `pow_register` (build only), `serve_axon`, `reset_axon`, `set_weights` (plain plus CR v4 timelock, mechanisms, raw_u16), `commit_weights`, `reveal_weights`, `start_call`, `register_subnet` (with completion), `set_hyperparameter`, `add_stake`, `remove_stake`; the shield path; btauth server and client; oracle infrastructure; Q1 answered | on localnet, an rtao validator commits timelocked weights and observes `TimelockedWeightsRevealed`; a Python v11 miner ↔ rtao validator btauth/1 scenario is green; a shielded `burned_register` resolves its inner receipt; the oracle passes for every implemented intent | 4 |
| M3 | **Framework and template** | `rtao-neuron` (`ValidatorRunner`, `MinerRunner`, `OwnerTools`, `MetagraphSync`, `EpochClock`, `WeightSetter`, `AxonPublisher`, `StateStore`, `NeuronConfig`, metrics); `rtao-subnet-template`; an example subnet | the example validator and miner run 20 epochs on the fast lane and 3 on the 12 s lane with the miner's incentive non-zero; kill and restart resumes from persisted state; Python ↔ rtao interop green in both directions | 3 |
| M4 | **Intent parity** | the remaining intents (46 pass-through generated, 37 ported, including the staking family with slippage, the root-claim fee model, multisig and proxy adapters, crowdloan, leasing, collateral, locks, EVM); raw and offline paths; CLI `tx` (all 83), `call`, `weights`, `axon`, `hparams`, `stake` aliases; coverage gate | oracle 83/83; `pending.toml` has no intents; coverage gate green; every upstream e2e name mapped in `parity/e2e_map.toml` | 5 |
| M5 | **Read parity and typed models** | the 102 reads; 14 namespaces; models plus `check-models`; selective metagraph and mechagraph; commitments; hyperparameters v3 rendering; CLI `query` (all 102); read oracle; CLI JSON interop | read oracle 102/102; the block-pinned interop comparison (§11.6, scenario 4) is value-equal for every `READ_SAMPLES` entry on localnet | 4 |
| M6 | **Signers, transport parity, PoW, EVM, timelock UX** | `LedgerSigner` (metadata hash and proof), `VaultSigner` (UOS plus upstream `vault.html`), `ExtensionSigner` (upstream `bridge.html`, daemon protocol), `RemoteSigner`; the full §3.12 checklist; PoW CPU/GPU; `rtao-evm`; async drand | every row of §3.12 green; Ledger proof vector test green and on-device signing demonstrated; `pow_register` e2e on the 12 s lane; the CLI `--ledger`/`--signer` flows work | 4 |
| M7 | **CLI parity** | all hand-written groups (§10.2); JSON and exit-code parity across the dumped Typer tree | parity matrix has zero missing rows and zero unexplained exclusions; the gate switches to strict mode | 6 |
| M8 | **Upstreaming and docs hardening** (runs alongside M3-M7 from the point each staged module stabilizes) | PR-U1…U12 opened (Appendix A); book, generated reference, `llms.txt`; compat matrix publication; the `sdk/bittensor-rs` proposal | at least PR-U1, U2 and U5 merged and released in a tag, with the matching staged modules deleted (depends on upstream review) | 3 to start, then ongoing |

Total ≈ 33 pw [I]: about 8 months for one engineer, or 4-5 months for two working in parallel after M2 (chain and tx on one
side; reads, CLI and framework on the other). The first usable validator and miner arrive at M3, about 11 pw in.

---

## 15. Risks and open questions

### 15.1 Risks

| # | Risk | Likelihood / impact | Mitigation |
|---|---|---|---|
| R1 | bittensor-core is internal: `publish = false`, no semver, the API follows the Python seam (RES/B §4.4) | high / medium | tag pins only; `core_surface` probe names the broken symbol; nightly `core-canary` against `main`; rtao touches a listed, small surface (§1.2) |
| R2 | Release cadence: 9 finalized runtimes in 20 days (RES/H §0.3) | certain / low after automation | drift and pin bots; "surface unchanged" auto-merges; `compat.json` makes the validated range explicit |
| R3 | Upstream declines the staged PRs (spec §3 lists a Rust transport and intent layer as non-goals) | medium / low | the staged code stays in `rtao-core-ext` (~1.2k lines), isolated and upstream-shaped; the merge-or-keep decision is cheap either way |
| R4 | Build weight and C toolchain from `host` (libsodium, vendored OpenSSL and Perl, secp256k1-sys, the polkadot-sdk fork clone; 330+ crates) | certain / medium | a CI build image, cargo git/registry caches, `cargo-chef` for downstream Docker images; PR-U10 feature split |
| R5 | The Python oracle infrastructure (maturin build of the core, uv) is slow or flaky | medium / medium | runs only in pin-bump and nightly jobs; outputs are committed; the image is cached per commit; fallback is the PyPI rc wheel of the same train (`rcN`, RES/H §2.5) |
| R6 | Extraction breaks when upstream refactors a table | medium / low | the extractor fails loudly with file and line; the tables are few and literal; PR-U3 and PR-U12 move them into catalogs |
| R7 | Parity-versus-correctness divergences: G5 predicate, pinned composite reads, transaction_version check on attach, strict TLS cert length | certain / low | each is an `exclusions.toml` entry of class `rtao-stricter`; the oracle accepts only listed divergences; G5 resolves when PR-U5 lands |
| R8 | jsonrpsee does not expose handshake headers, so `RpcPolicyError` lacks `retry_after`/`policy`/`reason` on HTTP 403/429 | certain / low | documented; a soketto-based transport if needed (~200 lines) |
| R9 | Name-based typed decoding hides a semantic change behind an unchanged field name | low / medium | `freeze_struct` hash watch on `rpc_info` structs; `check-models`; drift classification of storage default and type changes |
| R10 | Reveal-round prediction depends on wall clock and block time; `MaxEpochsPerBlock` deferral is unmodeled (upstream too) | medium / medium | the +3 block security offset (upstream's); reconcile and health events in `WeightSetter`; an e2e on both block-time lanes |
| R11 | Downstream `[patch.crates-io] w3f-bls` friction | certain / low | template repo, `rtao doctor`, Q1 |
| R12 | Typed decode slower than expected on 2,500-UID Null subnets | low / medium | M1 benchmarks before hot paths commit; the dynamic `Value` path (60-77 MB/s) remains the fallback |
| R13 | Public endpoints vary (CombinedBackend failures, storage rate limits, unknown chainHead support) | medium / medium | legacy RPC only; chunked batches; a per-endpoint smoke job (finality lag, spec, metadata hash every 6 h: RES/H §3.10) |
| R14 | License mismatch for an eventual monorepo move (rtao MIT, monorepo Apache-2.0) | certain / medium | dual MIT OR Apache-2.0 from day one; NOTICE for vendored Apache-2.0 fixtures |
| R15 | Python mirrors disagree with runtime sources (fee filters, proxy types) | low / low | CI cross-checks; rtao follows the runtime source and reports the mismatch upstream |

### 15.2 Open questions

1. **Q1.** Is `[patch.crates-io] w3f-bls` needed for std builds? The monorepo's vendored copy only patches no-std
   (RES/F §3.5). Test in M2: build without the patch, then have localnet pallet-drand decrypt rtao's commits. If it works,
   downstream manifests lose the patch line.
2. **Q2.** Will RaoFoundation accept PR-U1…U12, and later `sdk/bittensor-rs`? This needs a conversation with the
   monorepo owners. The spec (§3) argues against a Rust transport and intent layer *in the core*, not against a separate SDK
   directory.
3. **Q3.** Does `sdk/bittensor-core` at `v477` differ from `850f275` in symbols rtao uses? Unanswerable from the shallow
   clone; the first pin PR answers it via `core_surface`.
4. **Q4.** Parity definition for interactive-only CLI flows (`stake wizard`, pickers, prompts). Proposed: identical prompts
   and flags, not identical terminal rendering.
5. **Q5.** btauth `<Bytes>` wrapper: accept by default (parity, chosen here) or strict (RES/D recommends strict)? This is a
   user decision. Either way rtao never produces it.
6. **Q6.** Are archive reads of pre-V15 blocks (Python's frozen `LEGACY_RUNTIME_APIS`) in scope? Deferred here.
7. **Q7.** Should the epoch simulator model `MaxEpochsPerBlock`? This needs upstream agreement so rtao, Python and the
   pallet stay aligned (RES/C1 §10.4).
8. **Q8.** When will upstream sign v5 extrinsics? Metadata advertises `[4, 5]`, and both SDKs sign v4 today
   (RES/A1 §12.2). rtao follows upstream through a pin bump.
9. **Q9.** Will `SelectiveMetagraphIndex` become discoverable (a runtime API or constant), removing the pallet-source
   extraction?
10. **Q10.** Will upstream attach V15/V16 metadata blobs to release assets (RES/H §8.3)? That would shorten the drift job.
11. **Q11.** Is the oracle's CI cost (a maturin build per pin bump) acceptable, or should the PyPI rc wheel be the default?
12. **Q12.** Should Epistula or full v10 Synapse crates exist outside the parity target, given SN8/SN13/SN21 still run
    Synapse (RES/D §6)?

---

## Appendix A — upstream contributions (each staged in `rtao-core-ext` or extracted until merged)

| PR | Target | Scope | rtao-side effect when merged |
|---|---|---|---|
| PR-U1 | `sdk/bittensor-core/src/client/ops.rs` | move the private protocol functions of `client.rs` (`fetch_metadata` :1234, `decode_storage` :1352, runtime-API arg encoding :469-499/:1334, `find_extrinsic` :942, `decode_outcome` :993, `dispatch_error` :1365) into public sans-IO functions the blocking client calls; add the `TransactionPaymentApi_query_info` probe builder | delete `rtao-core-ext::ops`; the blocking client and rtao share one implementation |
| PR-U2 | `client::ops::receipt` | Python-parity outcome decoding (`Sudo.SudoAsDone`, `Utility.BatchInterrupted/ItemFailed`, MevShield failure events, `Treasury`/`Balances.Deposit` fee fallback) | delete the parity additions |
| PR-U3 | `website/.../generate.py` → `errors.json` | export custom transaction codes, invalid-tx needles, dispatch errors, per-code explanations and help overrides | extraction from `error_map.py`/`result.py` disappears |
| PR-U5 | `timelock/epoch_schedule.rs` | `BlocksSinceLastStep > tempo` (pallet-faithful) plus vectors; `generate_commit_v2` takes `now` | delete the staged `epoch_schedule`; the Python SDK gets the fix too |
| PR-U7 | `client.rs` `submit_shielded` | era ≤ 8 (`MAX_SHIELD_ERA_PERIOD`) and the inner-receipt scan | the core's blocking client becomes correct for shielding (rtao never depended on it) |
| PR-U8 | `transaction.rs` | public `PolicyInput` so non-constructor callers can use `Policy::check` | the differential test becomes direct reuse |
| PR-U9 | `runtime/mod.rs` | `Runtime::metadata_ir()` (serde) shared by the Python binding and rtao, extended with key types, hashers, events and runtime-API signatures | delete `rtao-core-ext::ir`; Python codegen and rtao regen consume one IR |
| PR-U10 | `Cargo.toml` features | split `host` into `keyfiles`, `keyfiles-nacl`, `keyfiles-legacy`, `drand-http`, `rpc-client`, `parallel` | validators and miners that load only plaintext hotkeys (the usual btcli layout) stop compiling OpenSSL and libsodium |
| PR-U11 | `keyfiles/mod.rs`, `keys/mod.rs` | pure `encode_password_for_environment`; `keys::verify_strict` (no `<Bytes>` fallback) | delete `rtao-core-ext::keyfile_env` and rtao's strict-verify shim |
| PR-U12 | `website/.../generate.py` → `intents.json` | add `default` to `input_schema` properties; add `mev_shield_default`, `mev_shield_required`, `all_amount_fields` | intent struct generation needs no Python extraction |

Two numbers are deliberately unused. **U4** (teach `Runtime::parse` to accept V16) was dropped because V15 carries
everything the core needs and is what RFC-0078 digests use. **U6** (an async client feature in the core) was dropped
because upstream's spec excludes a Rust transport (`MONO/sdk/bittensor-core-spec.md:87-91`) and rtao gains nothing from it
once PR-U1 exists.
