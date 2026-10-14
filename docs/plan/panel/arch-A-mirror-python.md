# rtao — Architecture A: mirror the Python layering

Design proposal, 2026-10-13. Lens: rtao's crates and modules correspond one-to-one with
`sdk/python/bittensor`, so every upstream Python change has an obvious Rust counterpart and
parity is checked by a tool rather than by reading code.

**Evidence conventions.** `UP/` = `scratchpad/upstream/subtensor/` (RaoFoundation monorepo at
`850f275`); `PY/` = `UP/sdk/python/bittensor/`; `CORE/` = `UP/sdk/bittensor-core/src/`;
`META/` = `scratchpad/meta/`; `R:X §n` = research report X, section n; `FEAS:` = the three
`feasibility-*.md` notes; `CORR:` = `research/CORRECTIONS.md`. **[verified]** = read on disk or
executed on 2026-10-13; **[inferred]** = reasoning not executed; **[proposal]** = a design choice.

Facts established for this document (beyond the research reports):

- **[verified]** Tag `v477` resolves to commit `5c6e83e2f4936f257234f246957f75684c35227c`;
  `gh api repos/RaoFoundation/subtensor/compare/v477...850f2755…` returns 9 commits / 32 files,
  **none under `sdk/`** (the changes are `chain-extensions/`, `docs/`, `pallets/`, `precompiles/`,
  `runtime/`, `ts-tests/`, `website/`). So `bittensor-core` and the Python SDK at `v477` are
  byte-identical to the `850f275` trees that the feasibility builds and every research report used.
- **[verified]** `UP/website/apps/bittensor-website/public/catalog/{intents,reads,errors}.json`
  are byte-identical (`cmp`) to the catalogs fetched from bittensor.com (`scratchpad/docs/`):
  83 intents (75 coldkey / 8 hotkey), 102 reads in 14 categories, 18 error codes, 390 chain errors.
  The catalogs are therefore available at the pinned tag without network access or Python.
- **[verified]** The Python package is 60,461 lines: `cli/` 23,083; top-level modules 13,543;
  `intents/` 8,335; `_transport/` 3,941; `reads/` 3,118; `_generated/` 2,978; `error_descriptions/`
  2,219; `evm/` 1,243; `extension/` 1,226; `vault/` 775. Tests 9,535; `codegen/` 1,144; `scripts/` 1,655.
- **[verified]** jsonrpsee-client-transport 0.26.1 reports a rejected WebSocket handshake as
  `WsHandshakeError::Rejected { status_code }` only (`src/ws/mod.rs:206-210`), while tungstenite
  0.28 returns `Error::Http(Box<Response<Option<Vec<u8>>>>)` with headers and body (`src/error.rs:71`)
  and models close code 1008 as `CloseCode::Policy` (`src/protocol/frame/coding.rs:154,214`).
- **[verified]** scale-decode-derive 0.16.2 decodes named composites by collecting the source
  fields into a map keyed by name and looking each target field up (`CannotFindField` if absent);
  extra source fields are ignored (`scale-decode-derive-0.16.2/src/lib.rs` named-field codegen).
  A `#[derive(DecodeAsType)]` struct therefore tolerates fields appended by a runtime upgrade,
  the same tolerance the Python reads get from dict lookups.

---

## 0. The design in one page

1. **The Python v11 package is already a veneer over bittensor-core.** It imports only the
   chain-defined surface (R:B §3.13: `Runtime`, `era_birth`, `multisig_account_id`, timelock
   functions, `encrypt_mlkem768`, `metadata_digest`, `generate_extrinsic_proof`, `LedgerDevice`,
   `pow_mine_cpu`, keypair/keyfile functions) and keeps transport, registries, executor, results,
   metagraph, wallets, `http_auth` and the CLI in Python. `PY/_transport/codec.py:1-12` is the only
   module that touches the SCALE engine and `PY/_substrate.py:1-12` the only one that touches the
   transport. **rtao is the same veneer in Rust**: the same seam into bittensor-core, the same module
   tree, the same registries, the same catalogs, the same fixtures.
2. **One upstream ref pins everything.** The monorepo tag `v<spec>` is the Cargo git source of
   `bittensor-core` *and* the parity target (`sdk/python/bittensor`, its tests and fixtures, the
   catalogs, and the chain-source tables the Python layer mirrors by hand). Bumping the tag is
   one PR that moves the Rust core, the parity target and the fixtures together.
3. **Parity is enforced by a tool.** `cargo xtask parity` checks five static layers:
   - (a) a file map with the upstream git blob SHA each Rust module was last reconciled against;
   - (b) a Python-AST symbol diff for changed files;
   - (c) equality of rtao's intent/read/error catalogs with the pinned upstream JSON catalogs;
   - (d) chain-source tables parsed with `syn`;
   - (e) transpiled Python data tables.

   Behavioral vectors from a dockerized Python oracle at the same tag complete the picture.
   **Python data tables are transpiled; Python logic is ported by hand.**
4. **The transport mirrors `_transport/`.** rtao has its own JSON-RPC-over-WebSocket session on
   tokio-tungstenite with rpc.py's semantics: idempotent pending frames resent on reconnect,
   `author_submit*` never resent, policy refusals terminal, endpoint pools, archive fallback. Its
   RuntimeManager uses the Python disk-cache file format, so Rust and Python processes share
   `~/.bittensor/runtime-cache`. **There is no subxt in the default dependency graph.**
5. **Decode model.** bittensor-core's `Value` (cyscale shapes) is the return type of the generic
   accessors, as in Python. Typed models decode with scale-decode `DecodeAsType` against the
   `PortableRegistry` that bittensor-core already parsed: one metadata parse, two views. Byte
   payloads never go through `Value`'s lossy utf8-else-hex rendering.
6. **Rust-only code lives outside the mirrored tree**: `rtao-macros`, `rtao-http`, `rtao-neuron`,
   the compat crates, and `rtao/src/ext/`. The parity tool ignores these paths, and reviewers can
   see that they have no upstream counterpart.

---

## 1. Boundary with bittensor-core

### 1.1 The rule

rtao uses from bittensor-core what the Python package uses, at the same seams. The only
additions are the pure `Value` navigation helpers from `client.rs` (§1.2), which use no transport.
The
core spec's own split is the boundary: "Python keeps everything whose right answer is a product
decision; Rust keeps everything whose right answer is defined by the chain"
(`UP/sdk/bittensor-core-spec.md` §1, quoted in R:B §1). rtao replaces the Python half with Rust
and keeps the core half as-is. All bittensor-core imports go through one crate, `rtao-core`. It
plays the role of `UP/sdk/bittensor-core-py` (the PyO3 forwarding layer, R:B §3.13) and of
`PY/sp_core.py`. When the core changes, only `rtao-core` needs editing.

### 1.2 Used as-is

| bittensor-core item | Location | rtao consumer | Python counterpart using the same item |
|---|---|---|---|
| `keys::Keypair` (`new`, `from_mnemonic`, `from_seed`, `from_uri`, `from_private_key`, `generate_mnemonic`, `from_encrypted_json`, `sign`, `verify`, accessors) | `CORE/keys/mod.rs:151-162, 166-410` | `rtao_core::keys` → `rtao::wallet`, `rtao::signing::KeypairSigner`, `rtao::http_auth` | `PY/sp_core.py`, `PY/keyfiles.py` |
| `keys::{verify, public_key_from_ss58, ss58_from_public, CRYPTO_ED25519, CRYPTO_SR25519, DEFAULT_SS58_FORMAT}` | `CORE/keys/mod.rs:28-31, 48-77, 457-469` | `rtao_core::ss58`, `rtao::http_auth::verify` | `PY/_transport/codec.py:64-92`, `PY/http_auth.py` |
| keyfiles: format detectors, `encrypt_keyfile_data`, `decrypt_keyfile_data`, `serialized_keypair_to_keyfile_data`, `deserialize_keypair_from_keyfile_data`, `get_password_from_environment`, `save_password_to_environment` | `CORE/keyfiles/mod.rs:29-47, 86, 111-138, 153, 179, 287` | `rtao::keyfiles` | `PY/keyfiles.py:11-22` |
| `runtime::Runtime::parse` + `pallet`, `storage_entry`, `constant`, `type_id_of`, `type_name_of`, `resolve`, `type_ident`, `module_error`, public fields (`types`, `pallets`, `extrinsic`, `apis`, `outer_event_type`) | `CORE/runtime/mod.rs:137-157, 163-228, 230-366` | `rtao_core::codec::RuntimeCodec` | `PY/_transport/codec.py:147-173` |
| SCALE engine: `compact`, `encode_spec/encode_value/encode_id`, `encode_era_value`, `coerce_account_id`, `compose_call`, `decode_spec/decode_value/decode_id`, `decode_call_value`, `storage_key`, `decode_storage_key_params`, `decode_batch`, `decode_map_page`, `decode_map_changes` | `CORE/codec/encode.rs:65-89, 176, 445, 491, 532`; `decode.rs:170-296, 658`; `storage.rs:21-164`; `batch.rs:55-135` | `RuntimeCodec` (rtao-core) | `PY/_transport/codec.py:181-323` |
| Extrinsics: `TxParams`, `PAYLOAD_FIELDS` (metadata-order extension encoding, fails closed on unknown non-zero-sized extensions), `signature_payload_parts`, `signature_payload`, `encode_signed_extrinsic` (v4), `decode_extrinsic`, `era_birth`, `multisig_account_id`, `multisig_ss58` | `CORE/codec/extrinsic.rs:19-55, 89-118, 217-316, 321, 407-442` | `rtao_transport::extrinsics` | `PY/_transport/codec.py:349-467`, `PY/_transport/extrinsics.py:268-318` |
| RFC-0078: `ChainInfo`, `metadata_digest`, `generate_extrinsic_proof` | `CORE/digest/mod.rs:34-52, 102-142` | `rtao::ledger`, `rtao::vault` (`MetadataVerifyingSigner`) | `PY/ledger.py:115`, `PY/vault/signer.py:121` |
| MEV shield: `mlkem::seal`, `twox_128`, `KDF_ID` | `CORE/mlkem/mod.rs:31-107` | `rtao::executor` shielded path | `PY/executor.py:1041` (`encrypt_mlkem768`) |
| Timelock: `WeightsTlockPayload`, `UserData`, `encrypt_and_compress`, `generate_commit_v2`, `encrypt_at_round`, `inner_ciphertext`, `reveal_round`, `decrypt_with_signature`, `constants::*`, `epoch_schedule::*` | `CORE/timelock/mod.rs:53-65, 87-195, 230-238, 319-325`; `constants.rs:6-51`; `epoch_schedule.rs:17-142` | `rtao::timelock`, `rtao::intents::weights` | `PY/timelock.py`, `PY/intents/weights.py:268-333` |
| PoW: `pow::{seal, mine_cpu, gpu_devices, GpuDevice, GpuMiner}` (OpenCL types re-exported at `CORE/pow/mod.rs:9`) | `CORE/pow/mod.rs:11-66`, `CORE/pow/opencl.rs:181, 501-540` | `rtao::pow_registration` | `PY/pow_registration.py` |
| Ledger: `LedgerDevice::{open, app_version, address, sign}` (feature `ledger`) | `CORE/signers/ledger.rs:46-156` | `rtao::ledger::LedgerSigner` | `PY/ledger.py:54-177` |
| `Value` navigation helpers `field`, `variant`, `variant_name`, `as_str`, `as_u128`, `as_bool`, `value_bytes` | `CORE/client.rs:1515-1570` | `rtao_core::value` | dict access in `PY/reads/*` |
| `CoreError` | `CORE/error.rs:11-47` | converted into `rtao::Error` | `UP/sdk/bittensor-core-py/src/errors.rs:29-39` |

### 1.3 Wrapped or extended

- **`RuntimeCodec`** (rtao-core) holds an `Arc<Runtime>` and adds:
  - `metadata_ir()` and `runtime_api_map()`. These are Rust ports of the two PyO3 methods in
    `UP/sdk/bittensor-core-py/src/runtime.rs:554-660`. They live in the binding crate, not the
    core, and read only public `Runtime` fields, so the port is about 120 lines. Codegen and the
    drift gate need them (§3.7).
  - `decode_as::<T: DecodeAsType>(type_id, bytes)` and `encode_as::<T: EncodeAsType>(type_id, &T)`,
    resolving against `runtime.types` (a `scale_info::PortableRegistry`, scale-info 2.11.6, the same
    copy scale-decode 0.16.2 uses; FEAS: veneer-coexistence).
  - Storage miss semantics, a mirror of `PY/_transport/storage.py:8-27`: a `Default` modifier
    decodes `default_bytes`; `Optional` yields `None`.
- **`KeypairSigner`** implements the rtao `Signer` trait (§6.3) over `Keypair`.
- **Async drand.** `rtao::timelock::fetch_round_signature` uses async reqwest and feeds
  `decrypt_with_signature`. It replaces the blocking `get_round_info`/`decrypt`
  (`CORE/timelock/mod.rs:242-315`), which use `reqwest::blocking`.
- **CPU-heavy core calls run on the blocking pool**, called through `tokio::task::spawn_blocking`:
  `Runtime::parse` (~4 ms per spec, R:B §1; 3.9 ms measured in FEAS), the Argon2i keyfile KDF
  (512 MiB, t=8; R:F §3.3), decodes larger than 64 KiB, and PoW mining.

### 1.4 Not used at runtime, and why

`client::Client`, `client::{Snapshot, BlockStream}` and `transaction::{IntentCall, Policy, Plan,
Executor}` are not used at runtime. They are product logic (R:B §3.14 classifies `client.rs` and
`transaction.rs` as PRODUCT). The parity target is Python's product logic, and the two disagree
on observable behavior:

| Concern | bittensor-core native SDK | Python v11 (rtao's target) |
|---|---|---|
| Transport | blocking `reqwest` HTTP POST, `ws://` rewritten to `http://`, polling (`CORE/client.rs:150-179, 1300-1308, 338-355, 897-962`) | websocket with subscriptions, batch, reconnect, pools (`PY/_transport/rpc.py`) |
| Default era | 64 (`CORE/client.rs:32`; `transaction.rs:1098`) | 128 (`PY/settings.py:22`) |
| Era anchor | best head (`CORE/client.rs:515-519`) | finalized head (`PY/_transport/interface.py:485-499`) |
| Shielded era | 64 for carrier and inner (`CORE/client.rs:633-646`). The runtime rejects eras above 8 for `submit_encrypted` (`UP/runtime/src/check_mortality.rs:19, 94-100`): an [inferred] bug | 8 (`PY/settings.py:29`) |
| Shielded inner receipt | not resolved; carrier result returned (R:A1 §9) | scanned until era end (`PY/executor.py:1073-1214`) |
| Fee estimate | deprecated `payment_queryInfo` RPC (`CORE/client.rs:551-576`) | `TransactionPaymentApi_query_info` via `state_call` with a zero signature and immortal era (`PY/_transport/interface.py:714-731`) |
| Error codes | `snake_case(name)` plus a few aliases (`CORE/client.rs:1396-1411`) | 18 `ErrorCode`s, 390 classified names (`PY/error_map.py:23-476`) |
| Intents | 22 trusted constructors; `IntentCall::trusted` is private, so any other intent is classified raw, unbounded, all-subnet (`CORE/transaction.rs:103-248`) | 83 intents with preflights, adapters, spend/netuid semantics |
| Nonce | none | `NonceCache` (`PY/_transport/extrinsics.py:55-84`) |

Re-implementing Python's executor in Rust does not duplicate chain-defined logic. Every byte the
executor produces still comes from the core (§1.2).

**bittensor-core's client as a test oracle [proposal].** `rtao-testkit` takes `bittensor-core`
(feature `host`) as a dev-dependency. The `core-oracle` e2e lane compares rtao's async results
with `bittensor_core::Client` on the same localnet block, for every case where the two should
agree:

- storage values (`Client::at(block).query`);
- runtime-API results;
- `compose_call` bytes;
- `partial_fee`, for byte-identical extrinsics. The core's `estimate_fee` signs with a 64-block
  mortal era, while Python and rtao use an immortal one; the era encodings differ by a byte, and
  the length fee is 0.5 rao/byte (R:C1 §7.1). The lane therefore passes rtao-built bytes to the
  core's `rpc_value("payment_queryInfo", …)` escape hatch (`CORE/client.rs:255`).

This puts the native client to work without coupling rtao's runtime behavior to it.

### 1.5 What bittensor-core lacks, and where rtao supplies it

| Missing in the core | rtao module (mirror of) |
|---|---|
| async WS JSON-RPC, subscriptions, reconnect, endpoint pool, policy refusals, archive fallback | `rtao-transport::rpc` (`PY/_transport/rpc.py`) |
| per-spec codec cache, block→spec resolution, disk cache | `rtao-transport::runtime` (`PY/_transport/runtime.py`) |
| nonce cache, `SignerPayloadJSON`, unsigned/attach split, watch/outcome resolution, nested-wrapper failure, MevShield events | `rtao-transport::{extrinsics, utils::receipt, interface}` |
| the `Substrate` seam, `Snapshot`, view-based reads | `rtao::{substrate, snapshot, client}` |
| generated name-based descriptors + drift/coverage/names/units gates | `rtao-generated` + `xtask codegen` (`UP/sdk/python/codegen`) |
| 83 intents, 102 reads, registries, JSON schemas, tool catalogs | `rtao::{intents, reads}` + `rtao-macros` |
| executor (compose → preflight → fee → policy → sign → submit → resolve; shield; raw; offline) | `rtao::executor` (`PY/executor.py`) |
| 18-code taxonomy, remediation, descriptions | `rtao::{result, error_map, error_descriptions}` |
| typed decode (no lossy byte rendering) | `rtao_core::codec` + `rtao/src/ext/types` |
| wallet directory model, password resolution, Keychain, Vault, extension bridge, EVM keystores | `rtao::{wallet, wallets, keyfiles, macos_password, vault, extension, evm}` |
| btauth/1 | `rtao::http_auth` (+ Rust-only `rtao-http`) |
| metagraph model, commitments | `rtao::metagraph` |
| CLI | `rtao-cli` (`PY/cli`) |

### 1.6 Proposed upstream contributions (none blocking)

| # | Change | Where | Why it matters to rtao (and to Python) |
|---|---|---|---|
| U1 | Use an 8-block era for the shielded carrier and inner extrinsic | `CORE/client.rs:633-646` | [inferred] the core's `submit_shielded` is rejected as `Stale` by `CheckMortality`; its e2e test only asserts a non-empty message (`UP/sdk/bittensor-core/tests/e2e.rs:313-339`) |
| U2 | `blocks_since_last_step > tempo` instead of `> MAX_TEMPO` | `CORE/timelock/epoch_schedule.rs:54` vs `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:1232` [verified] | Python and rtao both call `generate_commit_v2`. Fixing it once upstream fixes both SDKs; rtao carries a regression test that flips when the fix lands |
| U3 | `generate_commit_v2_at(now_secs, …)` with an injectable clock | `CORE/timelock/mod.rs:160-195` (`now_unix()` at :177) | deterministic reveal-round tests |
| U4 | Move `metadata_ir()` and `runtime_api_map()` from `bittensor-core-py` into `Runtime` (serde structs) | `UP/sdk/bittensor-core-py/src/runtime.rs:554-660` | one IR for Python codegen and rtao codegen; rtao drops its port |
| U5 | Split `host` into `keyfiles`, `pow`, `drand-fetch`, `client` | `UP/sdk/bittensor-core/Cargo.toml:105-126` | rtao could drop blocking reqwest and its internal runtime (a hazard when dropped in async contexts, R:B §6) |
| U6 | Decode option rendering `Vec<u8>` as `Value::Bytes` | `CORE/codec/decode.rs:812-817` (`bytes_value`) | removes the ambiguity where a UTF-8 payload that starts with `0x` is indistinguishable from hex |
| U7 | Public policy-semantics constructor (`Policy::check_semantics(spend, netuids, all, raw, fee)`) | `CORE/transaction.rs:56-90, 929-976` | lets any intent model reuse the core's fail-closed predicate |
| U8 | Commit encrypted keyfile vectors (NaCl, Fernet, Ansible) with passwords | `UP/sdk/python/tests/unit/test_keyfiles_golden.py` (round-trip only today, R:B §8) | cross-implementation proof |
| U9 | Accept V16 in `Runtime::parse` (project it to the V15 view) | `CORE/runtime/mod.rs:193-199` | removes the V15-only constraint |
| U10 | Publish `metadata-v15.scale` / `metadata-v16.scale` as release assets | `UP/.github/workflows/watch-mainnet-release.yml:283-341` (R:H §0.1) | lets drift detection skip live fetches |

### 1.7 Pinning and bumping

```toml
# rtao/Cargo.toml (workspace)
[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false, features = ["host"] }

[patch.crates-io]
# Cargo ignores [patch] tables of dependencies; the monorepo applies this one at UP/Cargo.toml:335
# so tle (the timelock IBE crate, UP/Cargo.toml:310) resolves the same w3f-bls copy pallet-drand uses.
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

- **[verified]** This resolves and builds with `rev = "850f2755…"` (FEAS gitdep and coexistence
  notes: 207 crates without `host`, 330 with). Cargo honors workspace inheritance inside the git
  checkout, and the vendored `w3f-bls` is found inside the same checkout.
- **[inferred]** `tag = "v477"` resolves identically. The `v477...850f275` diff touches neither
  `sdk/` nor the root `Cargo.toml`/`Cargo.lock`, so the crate sources and the workspace pins that
  Cargo inherits are the same. Re-verify in M0 with `cargo tree`.
- **Default features off, `host` on.** `host` is required for keyfiles and PoW
  (`CORE/lib.rs:10-25`). `ledger` is enabled through `rtao-core/ledger`.
- **Identity.** bittensor-core's version is never stamped (`0.1.0`, `publish = false`; R:B §7),
  so the git commit is the identity. `xtask sync-upstream` writes
  `crates/rtao-core/src/upstream.rs` (`pub const UPSTREAM: UpstreamPin = UpstreamPin { tag: "v477",
  commit: "5c6e83e2…", python_version: "11.3.1.dev0" }`) for diagnostics and `rtao --version`.
- **When to bump.** On each mainnet-finalized runtime. The signal is the drift job seeing finney's
  `(spec_version, :code hash)` change (§12.2), corroborated by a final (not pre-release) GitHub
  release `v<spec>`; tags can be abandoned (v470/471/476, R:H §1.2).
- **`cargo xtask bump-upstream --tag v<N>`**:
  1. checks that release `v<N>` is final and that finney's spec equals N;
  2. rewrites both `tag =` fields and runs `cargo update -p bittensor-core -p w3f-bls`;
  3. runs `xtask sync-upstream` (copies fixtures, catalogs and static assets to
     `upstream/v<N>/`, records blob SHAs);
  4. runs `xtask parity report --from v<old> --to v<N>`, which covers the Python-tree diffs, the
     `sdk/bittensor-core` diff and the catalog diffs;
  5. pins finney metadata at spec N and runs `xtask codegen`;
  6. opens a PR whose body is the parity report.

  The PR cannot merge until `xtask parity check` is green (§4.6).
- **Preview lanes.** CI jobs `upstream-main` and `upstream-testnet` rewrite the dependency to
  `branch = "main"` / `branch = "testnet"` (sed + `cargo update`) and run the offline suite plus
  the parity report as allowed-failure jobs. This gives hours to days of warning before mainnet
  moves (R:H §3.3).

### 1.8 The blocking client and the dynamic codec

- **Blocking reqwest client: not on any runtime path** (§1.4). Its only cost is compile time,
  because `host` still compiles `client.rs`/`transaction.rs`. U5 would remove that. rtao never
  calls it outside the `core-oracle` test lane.
- **Dynamic `Value` codec.**
  - **Kept** as the currency of the generic accessors (`query`, `query_map`, `runtime`,
    `constant`, `decode_scale`). This is exactly Python's contract (R:A2 §1.1), and it keeps the
    2,899-sample shape corpus usable as a regression oracle for decoding (§11.1).
  - **Not used** for typed models. `Value` renders `AccountId32` as an ss58 `String`, `Vec<u8>` as
    utf8-else-`0x`-hex `Str` (`CORE/codec/decode.rs:5, 812-817`), and enums as `{"Variant": …}`.
    Round-tripping typed data through it costs base58 encode/decode per account and is ambiguous
    for byte strings.
  - **Typed path.** Typed models (`Metagraph`, `NeuronInfoLite`, `StakeInfo`, `DelegateInfo`,
    `SimSwapResult`, `DynamicInfo`, `HyperparamEntry`, commitments) are decoded by `DecodeAsType`
    from the raw bytes and the type id the core resolved.
  - **Guard.** A differential test decodes every recorded payload through both paths and compares
    after projecting typed → `Value` (§11.1).
- **V15 vs V16.** Like Python, rtao fetches V15 with `state_call("Metadata_metadata_at_version",
  0x0f000000)` and falls back to `state_getMetadata` (V14) (`PY/_transport/runtime.py:292-310`),
  because `Runtime::parse` rejects V16 (`CORE/runtime/mod.rs:193-199`). The drift tooling also
  fetches V16 and reads it with `frame-metadata 23` directly, for `deprecation_info` and
  `transaction_extensions_by_version` (R:H §1.1).

---

## 2. Workspace and crate layout

### 2.1 The mirror rule

| Rule | Statement |
|---|---|
| R1 paths | `PY/<dir>/<file>.py` → `crates/rtao/src/<dir>/<file>.rs`. Three Python packages become their own crates: `PY/_transport/` → `crates/rtao-transport/src/`, `PY/_generated/` → `crates/rtao-generated/src/`, `PY/cli/` → `crates/rtao-cli/src/`. Leading underscores are dropped (`_substrate.py` → `substrate.rs`, `intents/_money.py` → `intents/money.rs`); `__init__.py` → `mod.rs`/`lib.rs`; `.pyi` stubs have no counterpart. |
| R2 names | A Python class `X` becomes the Rust type `X`; functions and methods keep their snake_case name; constants keep SCREAMING_CASE. Private Python helpers (`_conform`) become private Rust functions without the underscore (`conform`). |
| R3 aliases | Every mirrored public item carries `#[doc(alias = "bittensor.<dotted.path>")]`, e.g. `#[doc(alias = "bittensor.intents.serving.ServeAxon")]`. Rustdoc search then finds Rust items by their Python names, and `xtask` collects the aliases with `syn` (§4.6). |
| R4 headers | Every mirrored file starts with `//! Mirrors `sdk/python/bittensor/<path>` at the pinned upstream (see parity/upstream.toml).` |
| R5 Rust-only | Code with no upstream counterpart lives in Rust-only crates or under `crates/rtao/src/ext/`, never in a mirrored path. |
| R6 docs | Intent, read, field and parameter docstrings are copied verbatim, in their catalog form (single backticks), into doc comments. Catalog descriptions are then equal by construction (§4.6). |
| R7 tables | Python modules that are pure literal data (or modules with literal tables) are transpiled into `crates/rtao/src/py_tables/<python path>.rs` by `xtask transpile`; the mirrored module `include!`s them (§4.6). |

### 2.2 Crates

| Crate | Mirrors | Purpose and public surface | Features | Depends on |
|---|---|---|---|---|
| `rtao-core` | `PY/sp_core.py` + the role of `UP/sdk/bittensor-core-py` | The only importer of `bittensor_core`. Re-exports `keys`, `keyfiles`, `mlkem`, `digest`, `timelock`, `pow`, `CoreError`. Defines `codec::{RuntimeCodec, Value, TypeRef}`, `ss58::AccountId32`, `units::{NetUid, MechId, NetUidStorageIndex, TaoBalance, AlphaBalance, PerU16, U64F64, I96F32, I64F64, I32F32}`, `value::*`, `UPSTREAM` | `ledger` | bittensor-core (git tag), scale-decode 0.16, scale-encode 0.10, parity-scale-codec 3.7, serde(_json) |
| `rtao-transport` | `PY/_transport/` | `RpcSession`, `SubstrateConnection`, `RuntimeManager`, `NonceCache`, contract types (`CallBytes`, `SignedExtrinsic`, `UnsignedExtrinsic`, `SigningContext`, `InclusionReport`, `BlockData`, `MultisigAccount`, `MetadataIr`), signer capability traits. Documented as internal, as in Python (`PY/_transport/__init__.py:1-5`) | `rustls` (default) | rtao-core, tokio, tokio-tungstenite, futures, serde_json, tracing, hex, lru |
| `rtao-generated` | `PY/_generated/` | Generated `calls`, `storage`, `constants`, `runtime_apis`, `errors` | — | rtao-core |
| `rtao-macros` | — (Rust-only) | `#[intent(...)]`, `#[read(...)]`: registry entries, JSON schema, defaults, CLI metadata, namespace methods | — | syn, quote, proc-macro2 |
| `rtao` | the rest of `PY/` | Facade (`PY/__init__.py`), `Client`, `Snapshot`, `Substrate`, executor, intents, reads, results, wallets, metagraph, http_auth, timelock, pow, evm/vault/extension/ledger behind features | `blocking`, `ledger`, `vault`, `extension`, `evm`, `keychain` | rtao-core, rtao-transport, rtao-generated, rtao-macros, linkme, serde(_json), async-trait, tokio, reqwest (drand/evm rpc), rust_decimal, secrecy, chrono |
| `rtao-cli` | `PY/cli/` | binary `rtao` (btcli-equivalent) | `ledger`, `vault`, `extension`, `evm` (default all on) | rtao, clap 4, comfy-table, anstream, indicatif, rpassword, qrcode |
| `rtao-testkit` | `UP/sdk/python/tests/harness/` | `FakeSubstrate`, `FakeNode`, samples, golden/corpus loaders, localnet docker harness, Python-oracle runner | `docker` | rtao, rtao-transport, tokio-tungstenite (server) |
| `rtao-http` | — (Rust-only) | btauth/1 for tower/axum (server) and reqwest (client); `NonceStore` impls | `server`, `client`, `redis` | rtao, axum 0.8, tower 0.5, reqwest 0.13 |
| `rtao-neuron` | — (Rust-only; v11 removed Axon/Dendrite, R:G §6.1) | validator/miner/owner lifecycles, `WeightSetter`, `AxonPublisher`, `MetagraphSync`, `EpochClock`, config | `validator`, `miner`, `owner`, `metrics`, `auto-update` | rtao, rtao-http |
| `rtao-compat-synapse`, `rtao-compat-epistula`, `rtao-quic` | — (Rust-only, optional) | legacy v10 Synapse headers, Epistula dialects, btlightning adapter (§7.4) | — | rtao-http, btlightning (quic) |
| `xtask` | `UP/sdk/python/codegen/`, `UP/sdk/python/scripts/`, plus the parity tooling | `codegen`, `transpile`, `parity`, `sync-upstream`, `bump-upstream`, `record-golden`, `record-corpus`, `oracle`, `drift`, `docs`, `image`, `bench-live`, `compat-matrix` | — | rtao-core, rtao, syn, ruff_python_parser (git pin), toml, serde_json, frame-metadata 23 |

### 2.3 Dependency direction

```
                        bittensor-core  (git, tag v<spec>, features=["host"])
                              │
                          rtao-core
                 ┌────────────┼──────────────┐
          rtao-transport  rtao-generated   rtao-macros (proc-macro)
                 └────────────┼──────────────┘
                             rtao  ─────────────────────────────┐
             ┌───────────────┼──────────────┬──────────────┐    │
          rtao-cli       rtao-http     rtao-testkit (dev)   xtask
                             │
                        rtao-neuron         rtao-compat-* / rtao-quic → rtao-http
```

No cycles. Only `rtao-core` names `bittensor_core`; only `rtao-transport` opens sockets
(mirroring `PY/_substrate.py:5-8`).

### 2.4 Module map

The map is machine-readable in `parity/map.toml` (§4.6, App. B). "kind" is one of `hand`,
`transpiled`, `mixed` (hand logic plus transpiled tables), `generated` (from metadata), or `n/a`
(with a reason).

**Top-level modules (`PY/*.py`)**

| Python | Lines | Rust | Kind |
|---|---:|---|---|
| `__init__.py` | 276 | `rtao/src/lib.rs` (re-exports; every intent struct re-exported at the crate root, as `PY/__init__.py` does with `_INTENT_EXPORTS`) | hand |
| `_live.py` | 81 | `rtao/src/live.rs` (a `LiveDisplay` hook the CLI installs; `keyfiles` pauses it while prompting) | hand |
| `_substrate.py` | 851 | `rtao/src/substrate.rs` (`trait Substrate`, `RpcSubstrate`) | hand |
| `_subtensor.py` | 205 | `rtao/src/subtensor.rs` (`Subtensor` = blocking facade, `set_weights` helper) | hand, `blocking` |
| `balance.py` | 222 | `rtao/src/balance.rs` | hand |
| `basket_index.py` | 361 | `rtao/src/basket_index.rs` | hand |
| `basket_index_data.py` | 975 | `rtao/src/py_tables/basket_index_data.rs` | transpiled |
| `client.py` | 688 | `rtao/src/client.rs` | hand |
| `config.py` | 480 | `rtao/src/config.rs` (same `~/.bittensor/btcli.json` schema) | hand |
| `error_map.py` | 795 | `rtao/src/error_map.rs` + `py_tables/error_map.rs` | mixed |
| `executor.py` | 1,319 | `rtao/src/executor.rs` | hand |
| `fee_filters.py` | 98 | `rtao/src/fee_filters.rs` + `py_tables/fee_filters.rs` | mixed |
| `http_auth.py` | 369 | `rtao/src/http_auth.rs` | hand |
| `hyperparams.py` | 769 | `rtao/src/hyperparams.rs` + `py_tables/hyperparams.rs` | mixed |
| `keyfiles.py` | 217 | `rtao/src/keyfiles.rs` | hand |
| `ledger.py` | 181 | `rtao/src/ledger.rs` | hand, `ledger` |
| `macos_password.py` | 143 | `rtao/src/macos_password.rs` | hand, cfg(macos) + `keychain` |
| `masked_input.py` | 147 | `rtao/src/masked_input.rs` (rpassword) | hand |
| `metagraph.py` | 573 | `rtao/src/metagraph.rs` | hand |
| `multisig.py` | 319 | `rtao/src/multisig.rs` | hand |
| `namespaces.py` | 294 | `rtao/src/namespaces.rs` (14 namespace types; methods emitted by `#[read]`) | hand |
| `namespaces.pyi` | 738 | — | n/a: Rust types are the stub (Python emits it with `UP/sdk/python/codegen/emit_namespaces.py`) |
| `pow_registration.py` | 255 | `rtao/src/pow_registration.rs` | hand |
| `result.py` | 750 | `rtao/src/result.rs` + `py_tables/result.rs` | mixed |
| `settings.py` | 220 | `rtao/src/settings.rs` + `py_tables/settings.rs` | mixed |
| `signing.py` | 283 | `rtao/src/signing.rs` | hand |
| `snapshot.py` | 115 | `rtao/src/snapshot.rs` | hand |
| `sp_core.py` | 40 | `rtao-core/src/lib.rs` | hand |
| `sync.py` | 479 | `rtao/src/sync.rs` (`SyncClient`) | hand, `blocking` |
| `timelock.py` | 392 | `rtao/src/timelock.rs` | hand |
| `wallet.py` | 366 | `rtao/src/wallet.rs` | hand |
| `wallets.py` | 542 | `rtao/src/wallets.rs` | hand |

**Subpackages**

| Python | Lines | Rust | Kind |
|---|---:|---|---|
| `_transport/{__init__, codec, const, contract, errors, extrinsics, interface, lru, protocols, rpc, runtime_api, runtime, storage, utils/__init__, utils/receipt}.py` | 3,941 | `rtao-transport/src/{lib, codec, const, contract, errors, extrinsics, interface, lru, protocols, rpc, runtime_api, runtime, storage, utils/mod, utils/receipt}.rs` | hand |
| `_generated/{calls, constants, errors, runtime_apis, storage}.py` | 2,978 | `rtao-generated/src/{calls, constants, errors, runtime_apis, storage}.rs` | generated |
| `intents/{__init__, _money, _root_claim_fee, association, base, batch, children, coldkey, collateral, crowdloan, evm, governance, hyperparameters, identity, leasing, lock, multisig, plan, proxy, registration, registry, root, serving, staking, transfer, weights}.py` | 8,335 | `rtao/src/intents/{mod, money, root_claim_fee, association, base, batch, children, coldkey, collateral, crowdloan, evm, governance, hyperparameters, identity, leasing, lock, multisig, plan, proxy, registration, registry, root, serving, staking, transfer, weights}.rs` | hand; transpiled tables for `hyperparameters` (`OWNER_HYPERPARAMETERS`, `_ALPHA_PAIR`), `proxy` (`PROXY_TYPES`), `root_claim_fee` (fee-model constants) |
| `reads/{__init__, base, accounts, chain, collateral, delegation, epochs, hyperparameters, identity, leasing, locks, neurons, prices, root, staking, subnets, weights}.py` | 3,118 | `rtao/src/reads/{mod, base, accounts, …, weights}.rs` | hand |
| `error_descriptions/*.py` (24 files) | 2,219 | `rtao/src/py_tables/error_descriptions/*.rs` | transpiled |
| `evm/{__init__, addresses, contracts, keys, networks, precompiles, rpc, transactions}.py` + `abi/*.json` (21) | 1,243 | `rtao/src/evm/*.rs`; ABIs via `include_str!` from `upstream/<tag>/static/evm/abi/` | hand (`networks` transpiled), `evm` |
| `extension/{__init__, bridge, browser, client, daemon, picker, session, signer, tokens}.py` + `static/bridge.html` | 1,226 | `rtao/src/extension/*.rs`; HTML copied by `sync-upstream` | hand, `extension` |
| `vault/{__init__, qr, scan, server, signer, uos}.py` + `static/*` | 775 | `rtao/src/vault/*.rs` | hand, `vault` |
| `cli/**` (main, globals, context, tx, query, call, call_names, output, prompt, helpers, intent_prompts, stake_picker, stake_exit, metagraph_view, hyperparams_view, multisig_helpers, null_pruning, root_helpers, upgrade_helpers, secrets, logs, help_theme, commands/*) | 23,083 | `rtao-cli/src/**` (same names) | hand (`tx`, `query` driven by registries) |

**Tooling and tests (outside `PY/`)**

| Python | Rust | Kind |
|---|---|---|
| `UP/sdk/python/codegen/{__main__, metadata, emit_python, check}.py` | `xtask/src/codegen/{main, metadata, emit_rust, check}.rs` | hand |
| `UP/sdk/python/codegen/emit_namespaces.py` | — | n/a (namespace methods are emitted at compile time) |
| `UP/sdk/python/scripts/record_golden.py`, `record_shape_corpus.py` | `xtask record-golden`, `xtask record-corpus` | hand |
| `UP/sdk/python/scripts/bench_transport.py`, `bench_neurons.py` | `crates/rtao/benches/*.rs` + `xtask bench-live` | hand |
| `UP/sdk/python/scripts/{build_basket_index, export_beta_baselines_rs, verify_finney_evm_association_cap, prepare_clone_for_e2e}.py` | — / `xtask clone-prep` (M6) | n/a: upstream maintenance scripts |
| `UP/sdk/python/tests/harness/{fake_substrate, fake_node, samples}.py` | `rtao-testkit/src/{fake_substrate, fake_node, samples}.rs` | hand |
| `UP/sdk/python/tests/unit/test_*.py` (33 files) | `crates/<owning crate>/tests/<same name>.rs` | hand |
| `UP/sdk/python/tests/e2e/{test_null_consensus, test_pow_registration, test_timelock_commitment}.py` | `crates/rtao/tests/e2e_*.rs` (docker) | hand |
| `UP/sdk/python/tests/fixtures/{golden.json, shape_corpus/corpus.json}` | `upstream/v<N>/fixtures/` (synced copies with SHA-256 manifest) | synced |

**Watched upstream files that are not Python** (part of the parity surface, §4.6):

- `common/src/transaction_error.rs`: custom transaction error codes;
- `runtime/src/fee_filters.rs`: the coldkey-pays-fee list;
- `pallets/subtensor/src/rpc_info/metagraph.rs`: `SelectiveMetagraphIndex`;
- `docs/guides/signed-requests.mdx`: the btauth/1 specification and vectors;
- `sdk/bittensor-core/**`;
- `website/apps/bittensor-website/public/catalog/*.json`.

### 2.5 Features, MSRV, edition

- **Edition and MSRV.** Edition 2024 for all rtao crates. MSRV **1.89**, the channel of
  `UP/rust-toolchain.toml` at the pinned tag, where upstream CI builds bittensor-core and its
  fork deps (`UP/.github/workflows/check-rust.yml:261-264`, R:B §2.3). Every direct dependency
  floor is at or below it (tokio 1.71, axum 0.8 1.80, clap 4.6 1.85, proptest 1.12 1.88;
  R:F §2, §5, §8).
  - Policy: MSRV = max(upstream toolchain at the pinned tag, the dependency floors), checked by
    an `msrv` CI job.
  - Development uses current stable (1.99).
- **Default features.** `rtao`: none beyond `rustls`; `rtao-cli`: everything. Heavy or
  platform-specific pieces (`ledger` → hidapi, `vault` → qrcode + local web page, `extension` →
  bridge daemon, `evm` → alloy keystore/signing) are opt-in, mirroring the Python package's
  former extras (`UP/sdk/python/pyproject.toml` `evm = []`, `cli = []`).
- **Edition-2024 hazard.** `std::env::set_var` is `unsafe` in edition 2024, and bittensor-core's
  `save_password_to_environment` calls it (`CORE/keyfiles/mod.rs:126-138`, with a "not
  thread-safe" note). rtao exposes that only from the single-threaded CLI path, before the runtime
  starts.

### 2.6 What a subnet owner writes

Validator:

```toml
[package]
name = "sn-x-validator"
edition = "2024"

[dependencies]
rtao        = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0" }
rtao-neuron = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0", features = ["validator", "metrics"] }
rtao-http   = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0", features = ["client"] }
tokio  = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }
anyhow = "1"

# Needed in every top-level workspace that (transitively) depends on bittensor-core (§1.7).
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Miner:

```toml
[dependencies]
rtao        = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0" }
rtao-neuron = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0", features = ["miner"] }
rtao-http   = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.4.0", features = ["server"] }
axum   = "0.8"
tokio  = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

- **Template.** A `cargo generate` template (`rtao-subnet-template`) ships both binaries, the patch
  table, a `rust-toolchain.toml` and a docker-compose localnet.
- **Release tags.** rtao tags are `rtao-vX.Y.Z`. Every release records its upstream pin in
  `COMPAT.md` (§12.5).
- **Optional M0 experiment.** Build without the `w3f-bls` patch (crates.io `w3f-bls =0.1.3`, std)
  and prove timelock ciphertexts still decrypt on a localnet (`pallet-drand`). If they do, the
  patch requirement can be dropped (R:F §3.5 says the vendored fork only patches no-std).

---

## 3. Chain client

The layering is `PY/_transport/__init__.py:7-16`, one Rust module per Python module:
`interface` → `runtime` → `storage` → `extrinsics` → `runtime_api` → `codec` → `rpc`.
Each module knows only the ones below it, and `rpc` has no SCALE knowledge. Above the
transport, `rtao::substrate::RpcSubstrate` is the only consumer of `SubstrateConnection`, as in
Python.

### 3.1 Async model

- **The application owns the tokio runtime (1.53).** rtao spawns tasks but never builds a runtime.
  The one exception is `rtao::sync::SyncClient` (feature `blocking`), which mirrors `PY/sync.py`'s
  private loop thread: a dedicated thread runs a current-thread runtime, and calls are proxied over
  channels.
- **Object-safe async traits** (`Substrate`, `View`, `Signer`, `Intent`, `NonceStore`) use
  `async-trait`. Boxing one future per RPC is noise next to the RPC itself. Generic hot paths take
  `impl View` to avoid the box.
- **CPU work runs in `spawn_blocking`** (§1.3). Large map pages use the core's rayon fan-out
  (`decode_map_page`, `CORE/codec/batch.rs:86`, parallel above 64 items).
- **Bounded fan-out.** A per-connection `Semaphore` (default 4 permits, configurable) limits
  concurrent paged scans. Public endpoints are rate limited (about 1 req/s/IP per
  `UP/docs/guides/local-development.mdx:160-163`, R:G §4.1) and refuse with JSON-RPC
  `-32004 "storage work rate limit exceeded"` (R:A2 §7).

### 3.2 `rtao_transport::rpc` — the session (mirror of `PY/_transport/rpc.py`)

```rust
pub struct SessionConfig {
    pub urls: Vec<url::Url>,           // [primary] + fallbacks; ws:// or wss:// only (PY/settings.py:93-107)
    pub handshake_timeout: Duration,   // 10 s     rpc.py:60, 86-91
    pub response_timeout: Duration,    // 60 s     rpc.py:323, 733-741 (silence watchdog while requests pend)
    pub max_retries: u32,              // 5        rpc.py:322, 633-634 (timeouts per endpoint before rotating)
    pub retry_forever: bool,           // false    rpc.py:635-644 (else give up after max_retries × urls.len())
    pub subscription_buffer: usize,    // 2048     rpc.py:70, 280-291 (drop oldest)
    pub early_notification_ids: usize, // 64       rpc.py:297-300, 775-793
    pub extra_headers: http::HeaderMap,// Rust-only: e.g. Authorization for private nodes (R:E §4.1 #10)
}

pub struct RpcSession { /* Arc<Inner>; one supervisor task owns the socket */ }

impl RpcSession {
    pub async fn connect(cfg: SessionConfig) -> Result<Self, RpcError>;
    pub async fn request<R: DeserializeOwned>(&self, method: &str, params: impl Serialize) -> Result<R, RpcError>;
    pub async fn request_batch(&self, calls: &[(&str, serde_json::Value)]) -> Result<Vec<Result<serde_json::Value, RpcError>>, RpcError>;
    pub async fn subscribe(&self, method: &str, params: impl Serialize, unsubscribe: &'static str) -> Result<Subscription, RpcError>;
    pub async fn close(&self);
}

pub enum RpcError {
    Rpc { code: i64, message: String, data: Option<serde_json::Value> },  // JSON-RPC error object
    PolicyRejection { status_code: Option<u16>, retry_after: Option<Duration>, policy: Option<String>, reason: Option<String> },
    StateDiscarded { block_hash: H256 },                                  // "State already discarded" → archive retry
    FateUnknown { method: &'static str },                                 // in-flight author_submit* across a drop
    SubscriptionLost,                                                     // pushed into every open Subscription on reconnect
    MaxRetriesExceeded, ConnectionClosed, Timeout, Decode(String),
}
```

Behaviour, line for line with rpc.py:

- **Socket ownership.** A supervisor task owns the split socket. Callers enqueue frames and await
  a oneshot. The pending map keeps each request's frame so it can be resent.
- **Reconnect.** Every unanswered idempotent frame is resent *before* the "connected" gate
  reopens. Pending `author_submitExtrinsic`/`author_submitAndWatchExtrinsic` frames fail with
  `FateUnknown` instead (rpc.py:62-66, 686-725), so a duplicate never hits "already imported" and
  clears the nonce cache under a live transaction (R:A1 §4.8).
- **Subscriptions do not survive reconnects.** Each gets `SubscriptionLost`, mirroring the error
  text "verify balances before retrying" (rpc.py:582-602). Notifications that race the subscribe
  response are buffered; late ones for closed ids are dropped.
- **Policy refusals are terminal**: never retried, never rotated (rpc.py:53-59, 92-124, 190-241).
  They are:
  - an HTTP 403/429 handshake response, with `X-RateLimit-Policy`, `X-RateLimit-Reason` and
    `Retry-After` read from the response;
  - JSON-RPC codes `-32004`/`-32005`, with gateway data;
  - WebSocket close code 1008.
- **Rotation and backoff.** Rotate on connect failure or every `max_retries` consecutive timeouts.
  Back off `min(0.25·n, 5.0)` s within a cycle and `min(2·cycle, 30)` s between full-pool cycles
  when `retry_forever` (rpc.py:648, 676-678).
- **Sanitized diagnostics.** URL userinfo is stripped, control characters removed, length bounded
  (rpc.py:136-187). Logging uses `tracing` targets `rtao::transport` and
  `rtao::transport::raw_websocket`, mirroring Python's `bittensor.transport` loggers
  (`PY/__init__.py` docstring).

**Why tokio-tungstenite rather than jsonrpsee or subxt-rpcs.**

- The policy-refusal contract needs the handshake response headers and the close code.
  tungstenite exposes both (`Error::Http(Box<Response<…>>)`, `CloseCode::Policy`); jsonrpsee 0.26
  reports only `Rejected { status_code }` [verified, header list].
- The resubmission rules above are specific to this SDK.
- The Python transport is a raw `websockets` session with its own correlation (R:B §3.13, R:F §2),
  so a raw tungstenite session is the faithful mirror.

The cost is about 1.2k lines [inferred; rpc.py itself is 812], tested against `rtao-testkit::FakeNode` (a mirror of
`UP/sdk/python/tests/harness/fake_node.py`, which injects drop/refuse/silence) and a port of
`tests/unit/test_rpc.py`. TLS is rustls with the `ring` provider, the same backend reqwest and
axum use (R:F §2).

### 3.3 Networks, pools, archive fallback, rate limits, backend selection

- **Networks.** `rtao::settings` transpiles `NETWORKS`, `FALLBACK_ENDPOINTS`, `ARCHIVE_ENDPOINTS`,
  `FINNEY_GENESIS_HASH` and the explorer URL tables from `PY/settings.py:33-77, 149-221`.
  `resolve_endpoint(network)` accepts the names `finney | test | archive | devnet | local` or a raw
  `ws(s)://` URL; `local` honours `BT_CHAIN_ENDPOINT` (`PY/settings.py:33-41, 93-107`).
- **Genesis check (Rust-only addition).** On connect, `chain_getBlockHash(0)` is compared with the
  pinned genesis for named networks (finney `0x2f0555cc…`, testnet `0x8f9cf856…`, devnet
  `0x07789904…`; R:H §1.1). A mismatch refuses signing. This is recorded as divergence D-004 (§4.6).
- **Archive fallback.** `RpcSubstrate` lazily opens the archive pool and retries a read that hit
  `StateDiscarded` there (`PY/_substrate.py:366-400`).
- **No backend selection.** rtao speaks only the legacy JSON-RPC namespace the v11 SDK uses
  (`state_call`, `state_getStorageAt`, `state_queryStorageAt`, `state_getKeysPaged`,
  `state_getMetadata`, `state_getRuntimeVersion`, `chain_get{Head,FinalizedHead,BlockHash,Header,Block}`,
  `chain_{subscribe,unsubscribe}{NewHeads,FinalizedHeads}`, `author_submitExtrinsic`,
  `author_submitAndWatchExtrinsic`/`author_unwatchExtrinsic`, `account_nextIndex`; R:C2 §7,
  `PY/_transport/extrinsics.py:69-77`). That namespace works on the public finney WS endpoint
  (FEAS: subxt, `LegacyBackend` row), whereas the default subxt `CombinedBackend` fails there with
  `-32601`. Custom `subnetInfo_*`/`neuronInfo_*`/`swap_*` RPCs are never used (R:C2 §0.1, §9.1).
- **Capability probe.** `rpc_methods` is called once per connection and only logged at debug
  level.
- **Page size.** `query_map` defaults to `state_getKeysPaged` pages of 100
  (`PY/_transport/interface.py:304`), the Python default, which suits rate-limited public nodes;
  configurable. The core's 1,000 (`CORE/client.rs:33`) is not used.

### 3.4 `rtao_transport::runtime` — codecs per spec (mirror of `PY/_transport/runtime.py`)

```rust
pub struct RuntimeManager { /* session, genesis, caches */ }
impl RuntimeManager {
    pub async fn head_codec(&self) -> Result<Arc<RuntimeCodec>, TransportError>;            // revalidated ≤ every 12 s (runtime.py:142-176)
    pub async fn codec_at(&self, block_hash: H256) -> Result<Arc<RuntimeCodec>, TransportError>; // via the parent's runtime version (runtime.py:178-207)
}
```

- **Memory caches.** An LRU of 4 codecs keyed by `spec_version` and an LRU of 512
  `block_hash → spec_version` entries (runtime.py:123-143). Concurrent fetches of one spec share
  a `futures::future::Shared` (runtime.py:209-238).
- **Metadata fetch.** V15 via `state_call("Metadata_metadata_at_version", "0x0f000000")`, with
  the `Option<OpaqueMetadata>` stripped (`PY/_transport/codec.py:43-58`); V14 via
  `state_getMetadata` when the method is missing. A missing `transactionVersion` is a hard error
  (runtime.py:42-54).
- **Disk cache, byte-compatible with Python** (runtime.py:36-121):
  - directory `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache/`;
  - file `md1-<genesis 16 hex>-<spec>.bin`, containing magic `STMD`, then `u32 LE
    transaction_version`, then `u8 is_v15`, then the raw `MetadataVersioned` bytes;
  - atomic temp-file + rename writes; newest 8 files kept.

  Rust and Python processes on one host share the cache. An interop test asserts that each reads
  the other's files (§11.6).
- **Block-hash caches.** Only number↔hash mappings at or below the finalized head are cached,
  with the finalized height re-probed at most every 12 s (`PY/_transport/interface.py:45,
  187-230`), so a reorg cannot serve a stale fork hash.

### 3.5 The seam: `trait Substrate` and `trait View`

`rtao::substrate::Substrate` mirrors `PY/_substrate.py:76-302` method for method. Reads return
`Decoded`, which keeps the bytes and their type, instead of Python's eagerly decoded objects.
Callers choose `Value` (Python shapes) or a typed decode, and fakes can seed either.

```rust
/// Raw SCALE plus the codec/type that decodes it (miss semantics already applied).
pub struct Decoded { codec: Arc<RuntimeCodec>, ty: u32, bytes: Option<Bytes> }
impl Decoded {
    pub fn is_none(&self) -> bool;
    pub fn value(&self) -> Result<Value, Error>;                       // cyscale shape, == Python
    pub fn decode<T: scale_decode::DecodeAsType>(&self) -> Result<T, Error>;
    pub fn decode_opt<T: scale_decode::DecodeAsType>(&self) -> Result<Option<T>, Error>;
}

#[async_trait::async_trait]
pub trait Substrate: Send + Sync + 'static {
    // display metadata (PY/_substrate.py:85-102)
    fn token_symbols(&self) -> TokenSymbols;
    fn balance(&self, rao: u64, netuid: NetUid) -> Balance;
    // lifecycle
    async fn connect(&self) -> Result<(), Error>;
    async fn close(&self) -> Result<(), Error>;
    // chain position (115-135)
    async fn block_hash(&self, block: Option<u64>) -> Result<H256, Error>;
    async fn block_number(&self) -> Result<u64, Error>;
    async fn finalized_block_number(&self) -> Result<u64, Error>;
    async fn block_time(&self) -> Result<Duration, Error>;      // Aura.SlotDuration / 1000 (PY/_substrate.py:423-433)
    async fn spec_version(&self) -> Result<u32, Error>;
    async fn get_block(&self, at: BlockRef) -> Result<Option<BlockData>, Error>;
    // reads (141-201)
    async fn query(&self, item: &StorageItem, keys: &[Value], at: Option<H256>) -> Result<Decoded, Error>;
    async fn map_has_entries(&self, item: &StorageItem, keys: &[Value], at: Option<H256>) -> Result<bool, Error>;
    async fn query_map(&self, item: &StorageItem, keys: &[Value], at: Option<H256>) -> Result<Vec<(Vec<Value>, Decoded)>, Error>;
    async fn query_batch(&self, item: &StorageItem, key_sets: &[Vec<Value>], at: Option<H256>) -> Result<Vec<Decoded>, Error>;
    async fn runtime_call(&self, method: &RuntimeApiMethod, params: &[Value], at: Option<H256>) -> Result<Decoded, Error>;
    async fn constant(&self, item: &ConstantItem) -> Result<Decoded, Error>;
    async fn decode_scale(&self, type_string: &str, data: &[u8]) -> Result<Value, Error>;
    fn blocks(&self, finalized: bool) -> BoxStream<'static, Result<BlockHeader, Error>>;
    async fn events(&self, at: Option<H256>) -> Result<Vec<EventRecord>, Error>;
    // writes (203-302)
    async fn compose(&self, call: &Call) -> Result<CallBytes, Error>;
    async fn estimate_fee(&self, call: &CallBytes, signer: &dyn PublicKeyed) -> Result<Balance, Error>;
    async fn estimate_weight(&self, call: &CallBytes, signer: &dyn PublicKeyed) -> Result<Weight, Error>;
    async fn account_next_index(&self, who: &AccountId32) -> Result<u32, Error>;
    async fn mev_next_key(&self) -> Result<Option<Vec<u8>>, Error>;
    async fn sign_extrinsic(&self, call: &CallBytes, signer: &dyn Signer, nonce: u32, period: Option<u64>) -> Result<SignedExtrinsic, Error>;
    async fn find_extrinsic(&self, extrinsic_hash: H256, block_hash: H256) -> Result<Option<ExtrinsicResult>, Error>;
    async fn prepare(&self, call: &CallBytes, address: &AccountId32, opts: PrepareOptions) -> Result<UnsignedExtrinsic, Error>;
    async fn submit_signature(&self, unsigned: &UnsignedExtrinsic, signature: &[u8], opts: SubmitOptions) -> Result<ExtrinsicResult, Error>;
    async fn submit(&self, call: &CallBytes, signer: &dyn Signer, opts: SubmitOptions) -> Result<ExtrinsicResult, Error>;
    async fn submit_signed(&self, ext: &SignedExtrinsic, opts: SubmitOptions) -> Result<ExtrinsicResult, Error>;
    fn multisig_account(&self, signatories: &[AccountId32], threshold: u16) -> MultisigAccount;
    async fn submit_multisig(&self, call: &CallBytes, signer: &dyn Signer, ms: &MultisigAccount, opts: SubmitOptions) -> Result<ExtrinsicResult, Error>;
}
```

`PublicKeyed` is the public half of a signer (address, public key, crypto type): what
`public_view` returns (`PY/signing.py:266-283`). Fee and weight estimation never unlock a key.

`rtao::reads::base::View` is Python's "view": anything exposing the generic read surface
(`PY/reads/base.py:1-20`). Both `Client` (head) and `Snapshot` (pinned) implement it.

```rust
#[async_trait::async_trait]
pub trait View: Send + Sync {
    fn pinned_block(&self) -> Option<u64>;                                    // Snapshot.block
    async fn at(&self, block: Option<u64>) -> Result<Snapshot, Error>;       // pin (no-op on a Snapshot)
    async fn query(&self, item: &StorageItem, keys: &[Value]) -> Result<Decoded, Error>;
    async fn query_map(&self, item: &StorageItem, keys: &[Value]) -> Result<Vec<(Vec<Value>, Decoded)>, Error>;
    async fn query_batch(&self, item: &StorageItem, key_sets: &[Vec<Value>]) -> Result<Vec<Decoded>, Error>;
    async fn runtime(&self, method: &RuntimeApiMethod, params: &[Value]) -> Result<Decoded, Error>;
    async fn constant(&self, item: &ConstantItem) -> Result<Decoded, Error>;  // never pinned (PY/snapshot.py:66-68)
    fn balance(&self, rao: u64, netuid: NetUid) -> Balance;
    async fn timestamp(&self) -> Result<DateTime<Utc>, Error>;
    async fn block_time(&self) -> Result<Duration, Error>;
}
```

`Client::new(network, ClientOptions { policy, fallback_endpoints, archive_endpoints,
retry_forever, substrate: Option<Arc<dyn Substrate>> })` mirrors `PY/client.py:139-205`,
including the "pass `[]` to pin to one endpoint" semantics and injection of any `Substrate` for
tests.

### 3.6 Reading state: storage, maps, batches, runtime APIs, constants, events

| Accessor | RPC (as Python) | Codec work (bittensor-core) |
|---|---|---|
| `query` | `state_getStorageAt(key, hash)` | `storage_key` (`CORE/codec/storage.rs:69`); miss semantics from `StorageInfo.{modifier, default_bytes}` (`CORE/runtime/mod.rs:29-40`) |
| `query_batch` | one `state_queryStorageAt(keys, hash)`; head pinned when no hash (`PY/_transport/interface.py:256-278`) | batch key build; `decode_batch` |
| `query_map` | `state_getKeysPaged(prefix, 100, start, hash)` + `state_queryStorageAt(page, hash)`; keys and values pinned to one head hash per call (interface.py:347-351) | `decode_storage_key_params` (reversible hashers), `decode_map_page` |
| `runtime` | `state_call("{Api}_{method}", scale(params), hash)` | inputs encoded positionally or by name from the V15 API descriptors (`CORE/runtime/mod.rs:82-100`); output type id from the method info. No method on spec 477 returns `Vec<u8>` (R:C2 §1.0), so the frozen legacy decoders of `PY/_transport/runtime_api.py:24-171` matter only for archive reads of pre-V15 runtimes. They are ported anyway, for parity |
| `constant` | none | metadata of the head codec |
| `events` | `state_getStorageAt(System.Events)` | `decode_id(outer event)` → records `{phase, extrinsic_idx, event_index, module_id, event_id, attributes, topics}` (`CORE/codec/decode.rs:723-737`) |

- **Events are matched by `(pallet name, event name)`, never by index.** Event indices moved
  between spec 461 and 477 (SubtensorModule 149 changed meaning; R:H §2.2).
- **Typed event access [proposal].** A scale-decode visitor over `System.Events` records each
  event's `(pallet, variant)` and the byte span of its fields, so
  `EventRecord::attributes_as::<T: DecodeAsType>()` can decode one event's fields typed. Until that
  lands, events are consumed as `Value`, exactly like Python's dict records.

### 3.7 Generated descriptors and per-release regeneration (mirror of `PY/_generated` + `codegen/`)

Following R:A2 §8 decision 1, the generated layer is **name-based and carries no type ids or
hashers**; those are resolved at runtime from the live metadata per spec, which is exactly how
bittensor-core is designed (`UP/sdk/bittensor-core-spec.md:129-137`).

- `xtask codegen --metadata upstream/metadata/finney/<spec>.v15.scale` parses with
  `bittensor_core::runtime::Runtime::parse` and builds the IR with the `metadata_ir()` port
  (§1.3). The IR has the same fields as Python's `MetadataIR`
  (`PY/_transport/contract.py:182-246`): pallets with `name`, `index`, `calls[name, args[name,
  type_ident], docs]`, `errors[index, name, docs]`, `storage[name, value_type_ident]` and
  `constants`, plus runtime APIs.
- **Rust-only IR additions:** runtime-API input/output idents and `api_version`, which V15
  carries and Python's IR omits (R:A2 §5.7), and events (`name`, field idents).
- `emit_rust` writes:

```rust
// rtao-generated/src/calls.rs  (excerpt; Python: _generated/calls.py:429-431)
pub mod subtensor_module {
    use super::*;
    /// Serves or updates axon /prometheus information for the neuron … (docs copied from metadata)
    #[doc(alias = "bittensor.calls.SubtensorModule.serve_axon")]
    pub fn serve_axon(netuid: NetUid, version: u32, ip: u128, port: u16, ip_type: u8,
                      protocol: u8, placeholder1: u8, placeholder2: u8) -> Call {
        Call::new("SubtensorModule", "serve_axon", [
            ("netuid", netuid.into()), ("version", version.into()), ("ip", ip.into()),
            ("port", port.into()), ("ip_type", ip_type.into()), ("protocol", protocol.into()),
            ("placeholder1", placeholder1.into()), ("placeholder2", placeholder2.into()),
        ])
    }
}
// rtao-generated/src/storage.rs (Python: _generated/storage.py)
pub mod subtensor_module {
    pub const TEMPO: StorageItem = StorageItem::new("SubtensorModule", "Tempo", "u16");
}
// runtime_apis.rs, constants.rs, errors.rs (ERRORS: &[((u8, u8), ErrorInfo)]) likewise.
```

- **Argument types.** Primitive idents map to Rust primitives. Known newtype idents map to
  `rtao_core::units` types: `NetUid`, `MechId`, `NetUidStorageIndex`, `TaoBalance`,
  `AlphaBalance`, `PerU16`, `AccountId32`, `H160`, `H256`. Every other ident becomes `Value`,
  just as Python aliases it to `Any` (`PY/_generated/calls.py:19-40`). Rust is stricter wherever
  that is cheap and as permissive as Python elsewhere.
- **Output rules copied from `emit_python.py`.**
  - Sorted, deterministic output.
  - Headers omit `spec_version`, so a spec bump without a surface change produces no diff
    (`UP/sdk/python/codegen/emit_python.py:22-30`, CORR #2).
  - Keyword-collision suffixing (`:43-45`); generated files are excluded from rustfmt.
- **Per-release regeneration.**
  - A metadata pin is committed per (network, spec), with an `upstream/metadata/index.toml` row
    (R:H §1.1).
  - `xtask codegen --check` runs in PR CI: offline regeneration from the pinned blob, then a
    diff. This is the offline analogue of `codegen.check --drift`.
  - The nightly drift job regenerates from live finney, testnet and devnet (§12.2).
  - Runtime guard: at connect, every descriptor rtao uses is validated against the live
    `RuntimeCodec`, checking that the name exists, the key arity matches and the value ident
    matches (R:A2 §5.7 item 3, R:E §4.1 #1). A mismatch logs a warning for reads and refuses for
    signing.

### 3.8 Typed decode

- **Typed models** live in `crates/rtao/src/ext/types/` (Rust-only, R5). They are hand-written
  `#[derive(DecodeAsType, EncodeAsType)]` structs whose field names equal the runtime's: the
  `freeze_struct`-pinned rpc_info structs (R:A2 §4.2). Because scale-decode matches fields by name
  and ignores extras [verified], an appended runtime field does not break decoding. A renamed or
  removed field fails with `CannotFindField`, which the nightly drift job catches on recorded
  payloads.
- **Generic accessors** return `Decoded`. Each read (§4.2) chooses `.value()` where its Python
  body pattern-matches dicts and `.decode::<T>()` where a typed model exists. Because the read
  body is hand-ported, every such choice is visible in review.
- **Differential test.** For each typed model, golden and recorded payloads decode both ways, and
  the typed result projected to `Value` must equal the core's `Value` (§11.1).

### 3.9 Writes: nonce, era, fee, signing, submission, outcome

| Step | Python (cite) | rtao |
|---|---|---|
| Nonce | `NonceCache`: first `account_nextIndex`, then local increment; `pin`; `clear` on any failure between sign and inclusion (`PY/_transport/extrinsics.py:55-84`; `PY/_substrate.py:753-772`) | `rtao_transport::extrinsics::NonceCache` (per-account `tokio::sync::Mutex<HashMap<AccountId32, u32>>`); identical rules; `prepare` always fetches fresh (`interface.py:589-590`) |
| Era | default 128 (`PY/settings.py:22`); mortal era anchored at the **finalized** head with `era_block_hash = block_hash(era_birth(period, current))`; immortal anchors at genesis (`interface.py:485-499`) | same, using `bittensor_core::codec::extrinsic::era_birth` |
| Payload | `call ‖ extra ‖ additional` in metadata extension order; blake2-256 if > 256 bytes; `CheckMetadataHash` Disabled unless a `MetadataVerifyingSigner` supplies the digest (R:A1 §8.1) | `Runtime::signature_payload_parts`/`signature_payload` with `TxParams { metadata_hash }`; digest from `digest::metadata_digest` when the signer asks |
| Extension guard | — | Rust-only, at connect and in the drift job: fail if the runtime declares an extension identifier outside the core's `PAYLOAD_FIELDS` table with a non-zero-sized type. Already true inside the core (`CORE/codec/extrinsic.rs:89-118`); surfacing it at connect gives an early, clear error. Note CORR #1: the 478 fee wrapper keeps the `ChargeTransactionPayment` identifier |
| Assembly | v4 only (`CORE/codec/extrinsic.rs:260-316`); finney also allows v5 (`META/finney.json`), unused by Python | same; v5 out of scope (R:A1 §12 Q2) |
| Fee | `TransactionPaymentApi_query_info` via `state_call` on an immortal, zero-signature extrinsic built with `account_nextIndex` without touching the cache; read `partial_fee` (`interface.py:714-731`; `PY/_substrate.py:564-577`) | same; returns `Balance` and `Weight { ref_time, proof_size }` |
| Submit | no wait → `author_submitExtrinsic` → result `success=true, "Submitted (not waiting)."` (`PY/_substrate.py:776-777`); else `author_submitAndWatchExtrinsic`: `usurped/retracted/finalitytimeout/dropped/invalid` → error; `inBlock`/`finalized` per wait flags; always unsubscribe (`interface.py:626-663`; `extrinsics.py:352-374`) | same state machine; `SubmitOptions { period: Option<u64>, wait_for_inclusion: bool, wait_for_finalization: bool, nonce: Option<u32> }` with Python defaults (128/true/true) |
| Resolve | `chain_getBlock`; locate by blake2-256 of each raw extrinsic; `ExtrinsicNotFound` rather than fabricate; events filtered by `extrinsic_idx`; fee = `TransactionFeePaid.actual_fee` else deposit sum; failure = `ExtrinsicFailed` or MevShield failure events; nested wrapper `Err` (`Sudid`, `SudoAsDone`, `ProxyExecuted`, `MultisigExecuted`, `BatchInterrupted`, `ItemFailed`) is failure (`interface.py:677-712`; `PY/_transport/utils/receipt.py:8-237`) | `rtao_transport::utils::receipt` port; module errors resolved by `Runtime::module_error` |
| Result | `ExtrinsicResult { success, message, block_hash, extrinsic_id "{block}-{idx:04}", explorer_url (finney/archive), fee, events, error, data }` (`PY/result.py:697-749`) | `rtao::result::ExtrinsicResult` (same fields; `raise_for_failure() -> Result<Self, ChainError>`; `to_json()` = Python `to_dict()`) |
| Transport failure | clear nonce; `RpcPolicyRejection`/`MaxRetriesExceeded` → raise; other request errors → failed result via `chain_error_from_substrate_request` (`PY/_substrate.py:753-772`) | identical mapping: `Err(Error::RpcPolicy{..})` vs `Ok(ExtrinsicResult{success:false, error:Some(..)})` |
| Retry | executor-level only, on `"priority is too low"`, `"transaction is outdated"`, `"stale"`; fresh nonce each time; dispatch failures never retried (`PY/executor.py:52-56, 795-800`) | same (rtao-neuron adds `LastUpdate`-based reconciliation on top, §9.4) |
| Spec change | `submit_signature` refuses if `spec_version` changed since `prepare` (`interface.py:617-623`) | also refuses on a `transaction_version` change (R:A1 §10 #16) |

### 3.10 MEV-shielded submission (mirror of `PY/executor.py:897-1214`)

1. Read `MevShield.NextKey`, retrying twice one block apart.
2. Fetch the nonce n. Sign the inner extrinsic at **n+1 with an 8-block era**; seal it with
   `mlkem::seal(pk, inner, include_key_hash = true)`.
3. Compose `MevShield.submit_encrypted(ciphertext)` and submit the carrier at **n with an 8-block
   era**.
4. Resolve the inner extrinsic by scanning blocks up to `included_at + period`, with bounded waits
   per block. Re-resolve after finality and rescan on reorg. Flip the result to failure if the
   inner extrinsic never lands.

The ciphertext is bounded at 8,192 bytes (`PY/executor.py:61`). The carrier fee is estimated by
pricing `submit_encrypted` with an 8,192-byte ciphertext.

Policy and defaults:

- **Missing fee.** `max_fee_tao` set without a fee estimate is a hard `PolicyError` here, unlike
  in `plan` (R:A1 §10 #4).
- **Free-TAO check.** Execution is blocked if free TAO covers the inner reserve but not reserve +
  carrier.
- **Defaults.** `period = 8`, `wait_for_inclusion = true`, `wait_for_finalization = false`.
- **Shield precedence.** `execute` redirects only `mev_shield_required` intents
  (`burned_register`, `add_collateral`). The CLI precedence is required → flag → config → intent
  default (`PY/cli/context.py:260-292`), mirrored in `rtao-cli`.

### 3.11 Composition: sudo, proxy, multisig, batch

- **Composition order**, mirroring `_compose_intent_call` and `nest_origin_wrappers`
  (`PY/executor.py:92-121, 175-219`):
  1. take pinned inner call data if present;
  2. `semantic = intent.semantic_intent()` with addresses coerced;
  3. build against `origin_view` (or the `proxy_for` view);
  4. wrap in `Sudo.sudo` if `origin == "root"` (never double-wrapped);
  5. wrap in `Proxy.proxy(real, force_proxy_type, call)` if proxying;
  6. apply `intent.wrap_call(...)` (saved-multisig adapter).

  `dispatch_origin` and `fee_payer` follow `_intent_accounts` (`:234-246`).
- **Splicing.** Nested calls are a byte splice: `Value::Bytes(inner)` params are spliced verbatim
  by `compose_call` (`CORE/codec/encode.rs:528-585`).
- **Multisig.** `Multisig.as_multi` gets `max_weight` from `estimate_weight`, the existing
  timepoint from `Multisig.Multisigs[account, call_hash]`, and `other_signatories` sorted by raw
  bytes minus the signer (`PY/_transport/interface.py:746-787`). The account comes from
  `multisig_account_id` (`CORE/codec/extrinsic.rs:419-437`). Funds preflight and
  `_FEE_ALLOWANCE_RAO = 5_000_000` come from `PY/multisig.py:121-162`.
- **Batch.** `Utility.batch_all` only, via the `Batch` intent: no nesting, aggregated spend and
  netuids, namespaced extras (`PY/intents/batch.py:23-167`).

### 3.12 Snapshots and block pinning

- **`Client::at(block)`** resolves the head number once and returns `Snapshot { client, block,
  hash }` (`PY/client.py:650-658`; `PY/snapshot.py:25-116`). Every accessor forwards the pinned
  hash; `Snapshot::at(None)` returns itself.
- **The Python idiom `view = await view.at()`** at the top of multi-call reads becomes
  `let view = view.at(None).await?;` in the ported read bodies, line for line.
- **Four reads that Python gathers unpinned** (`delegate_take`, `coldkey_swap_announcement`,
  `hotkey_identities`, `miner_collateral`; R:A2 §1.3) are pinned in rtao. This is divergence
  D-002: results come from one block instead of possibly straddling two. The `FakeSubstrate`
  records the `at` hash of every call, and a table test asserts that every read's calls share one
  hash (§11.2).

### 3.13 Block streams and waiting helpers

- `Client::blocks(finalized) -> impl Stream<Item = Result<BlockHeader>>` subscribes to
  `chain_subscribe{NewHeads,FinalizedHeads}` (`PY/_transport/interface.py:439-455`).
- Waiting helpers (`PY/client.py:376-444`):
  - `wait_for_block(block, timeout)`;
  - `wait_for_timestamp(when, timeout)`, which sleeps through long gaps on the chain's clock;
  - `wait_for_epoch(netuid, timeout)`, which watches `LastEpochBlock` advance, the only
    trustworthy signal because the schedule is stateful (R:C2 §0.6).
- On `SubscriptionLost` the helpers resubscribe and re-check chain state rather than replaying
  stale headers.

### 3.14 Offline and external signing

- `prepare_call(call, address, crypto_type, nonce, period, tip, metadata_hash) -> UnsignedExtrinsic`
  and `submit_signature(unsigned, signature)` mirror `PY/executor.py:1258-1316`.
- `UnsignedExtrinsic` round-trips through JSON, as `to_dict`/`from_dict` does
  (`PY/_transport/contract.py:120-134`). It carries `SignerPayloadJSON`, with numeric fields as
  big-endian hex at SCALE width (`PY/_transport/extrinsics.py:87-132`), so Polkadot-JS-style
  signers and the extension bridge interoperate.

---

## 4. Intents and reads parity

Upstream counts: **83 intents** (`intents.json`; 83 `@register` decorators, R:A1 §2.3) and
**102 reads** (`reads.json`; R:A2 §3). The brief's "~70" and "~100" are approximations. rtao
mirrors all of them, under the same names, files and categories.

### 4.1 Intent representation

An intent is a plain struct (Python: a dataclass) with an attribute macro carrying the class
variables, plus an `impl Intent` with the hand-ported behavior. Example, mirroring
`PY/intents/serving.py:26-66`:

```rust
//! Mirrors `sdk/python/bittensor/intents/serving.py` at the pinned upstream.

/// Publish this hotkey's axon endpoint (ip:port) for a subnet.
///
/// Writes the connection info to chain storage so validators on the subnet
/// know where to reach this miner or validator; it does not start any server —
/// … (docstring copied verbatim in its catalog form)
#[rtao::intent(op = "serve_axon", signer = "hotkey", wraps = [("SubtensorModule", "serve_axon")])]
#[doc(alias = "bittensor.intents.serving.ServeAxon")]
#[derive(Clone, Debug, PartialEq)]
pub struct ServeAxon {
    /// Subnet on which to publish the endpoint.
    pub netuid: NetUid,
    /// Public IPv4 or IPv6 address of the endpoint, in standard dotted or colon notation.
    pub ip: String,                       // parsed in build(), as Python does with ipaddress
    /// TCP port the endpoint listens on.
    pub port: u16,
    /// Application protocol tag stored alongside the endpoint; its meaning is subnet-defined.
    #[intent(default = 4)]
    pub protocol: u8,
    /// Version number of the serving neuron's software, stored with the endpoint.
    #[intent(default = 1)]
    pub version: u32,
}

#[async_trait::async_trait]
impl Intent for ServeAxon {
    async fn build(&self, sub: &dyn Substrate, _wallet: &dyn KeyView) -> Result<Built, Error> {
        let addr: std::net::IpAddr = self.ip.parse().map_err(|e| Error::invalid_argument(e))?;
        let (ip, ip_type) = ip_to_u128(addr);          // int(ipaddress) + version, PY serving.py:50-63
        let call = calls::subtensor_module::serve_axon(self.netuid, self.version, ip, self.port,
                                                       ip_type, self.protocol, 0, 0);
        Ok(sub.compose(&call).await?.into())
    }
    fn summary(&self) -> String { format!("serve axon {}:{} on netuid {}", self.ip, self.port, self.netuid) }
}
```

The trait mirrors the `Intent` ABC (`PY/intents/base.py:138-399`) hook for hook:

```rust
#[async_trait::async_trait]
pub trait Intent: Send + Sync + std::fmt::Debug + 'static {
    fn info(&self) -> &'static IntentInfo;                                   // macro: op/signer/origin/verify/wraps/…
    async fn build(&self, sub: &dyn Substrate, wallet: &dyn KeyView) -> Result<Built, Error>;
    fn summary(&self) -> String;
    async fn effects(&self, sub: &dyn Substrate, signer: &AccountId32) -> Result<Vec<String>, Error> { Ok(vec![self.summary()]) }
    async fn warnings(&self, sub: &dyn Substrate, signer: &AccountId32) -> Result<Vec<String>, Error> { Ok(vec![]) }
    async fn blocks(&self, sub: &dyn Substrate, signer: &AccountId32) -> Result<Vec<String>, Error> { Ok(vec![]) }
    async fn preflight(&self, sub: &dyn Substrate, dispatch_origin: &AccountId32, fee_payer: &AccountId32,
                       call: Option<&CallBytes>) -> Result<IntentPreflight, Error>;   // default composes the three hooks
    async fn wrap_call(&self, sub: &dyn Substrate, wallet: &dyn KeyView, call: CallBytes) -> Result<CallBytes, Error> { Ok(call) }
    fn origin_view<'a>(&'a self, sub: &dyn Substrate, wallet: &'a dyn KeyView) -> OriginView<'a> { OriginView::Wallet(wallet) }
    fn spend(&self) -> Spend { Spend::None }
    fn touches_netuids(&self) -> Vec<NetUid>;                                // macro default: the `netuid` field, if any
    fn affects_all_subnets(&self) -> bool { false }
    fn semantic_intent(&self) -> &dyn Intent;                                // adapters return the inner intent
    fn to_args(&self) -> serde_json::Map<String, serde_json::Value>;         // macro: money → exact decimal strings
}

pub struct IntentInfo {
    pub op: &'static str, pub class: &'static str, pub module: &'static str,
    pub signer: Role,               // Coldkey | Hotkey
    pub origin: Origin,             // Signed | SubnetOwner | Root
    pub verify: Option<&'static str>,
    pub wraps: &'static [(&'static str, &'static str)],
    pub all_amount_fields: &'static [&'static str],
    pub mev_shield_default: bool, pub mev_shield_required: bool,
    pub description: &'static str,  // from the doc comment
    pub fields: &'static [FieldInfo],   // name, py_schema, help, default (JSON), required
    pub from_args: fn(serde_json::Map<String, serde_json::Value>) -> Result<Box<dyn Intent>, Error>,
}
#[linkme::distributed_slice] pub static INTENTS: [&'static IntentInfo];
```

- **Key views.** `KeyView` is the Rust shape of "wallet-like": a wallet, keyed wallet or bare
  signer. It provides `hotkey_address(override)`, `hotkey_public_key()` and `coldkey_address()`
  with the exact fallback rules of `PY/intents/base.py:248-298` (a bare signer *is* the key when
  the intent's signer role matches). Intents never touch private keys.
- **Schema is Python's, not schemars'.**
  - The macro builds `json_schema()` with a `PySchema` trait that mirrors `_json_type` and
    `money_schema` (`PY/intents/base.py:58-104`). Integers, including `NetUid`/`u16`/`u64`, emit
    `{"type":"integer"}`; `Money` emits the three-way `anyOf` with `"all"` only for
    `all_amount_fields`; `Option<T>` is non-required `T`; lists map to `array`.
  - The output is byte-comparable with the catalog's `input_schema`. schemars would add
    `format`/`minimum` keys and break equality.
- **Validation semantics.** `from_args` rejects unknown keys (`PY/intents/base.py:366-372`),
  parses money as number | decimal string | `"all"`, and runs the same construction-time
  validation as the Python `__post_init__` hooks. Examples: `_as_pairs` for weights; the
  children proportion sum ≤ `u64::MAX` (`PY/intents/children.py:71-87`); `swap_basket_many` legs
  1..128.
- **Registry** (`rtao::intents::registry`, mirror of `PY/intents/registry.py`):
  - `REGISTRY` is the sorted view of `INTENTS`;
  - `build(op, args)`;
  - `list_tools()` emits `{name, summary, description, signer, origin, verify, wraps, docs_url,
    input_schema}` (`registry.py:43-66`);
  - a startup-free unit test asserts op uniqueness, which Python checks at import.
- **Execution adapters** (unregistered, `PY/intents/multisig.py:276-398`):
  `MultisigIntentAdapter` and `MultisigThreshold1IntentAdapter` implement `Intent` with
  `semantic_intent()` returning the inner intent, `wrap_call` adding `as_multi` /
  `as_multi_threshold_1`, and `inner_call_data` pinning prior-round bytes.

**Rejected alternative.** Generating intent structs from the Python AST and the catalogs. It
would need a Python→Rust type-mapping table, for example `weights: Optional[list[float]]`, which
accepts either a list or a `{uid: w}` map. Generated code with hand-written impls attached makes
ownership confusing. Detection stays mechanical through §4.6 (schema, defaults and metadata are
compared), while humans own the Rust code.

### 4.2 Read representation

Reads stay **functions of a view**, as in Python; the macro adds the registry entry, the
JSON-in/JSON-out dispatcher, the catalog record and the namespace method. Example, mirroring
`PY/reads/neurons.py:31-66`:

```rust
/// Every neuron on a subnet in ONE runtime-API call (the metagraph fast path).
///
/// `lite=true` (the default) omits per-neuron weights/bonds, which is far
/// smaller and what almost every caller wants.
#[rtao::read(name = "neurons", category = "Neurons & registration", namespace = Neurons,
             targets = [runtime(neuron_info_runtime_api::GET_NEURONS_LITE),
                        runtime(neuron_info_runtime_api::GET_NEURONS)])]
#[doc(alias = "bittensor.reads.neurons.neurons")]
pub async fn neurons(
    view: &dyn View,
    #[param(doc = "Subnet whose neurons to list.")] netuid: NetUid,
    #[param(doc = "Omit per-neuron weights/bonds (much smaller; what most callers want).", default = true)] lite: bool,
) -> Result<Vec<Neuron>, Error> {
    let method = if lite { &neuron_info_runtime_api::GET_NEURONS_LITE } else { &neuron_info_runtime_api::GET_NEURONS };
    let records: Vec<ext::types::NeuronInfoLite> = view.runtime(method, &[netuid.into()]).await?.decode()?;
    // … same post-processing as Python: total_stake = Σ stake list, sorted by uid
}
```

- **Parameter attributes.** The outer attribute macro strips `#[param(...)]` from the parameters
  before emitting the function; async-graphql uses the same pattern for resolver arguments.
- **`targets`.** These are compile-time references to generated descriptors, so a renamed
  storage item fails to compile against the regenerated layer. They also replace the website
  generator's source scraping for `hits` (R:A2 §5.6), and the catalog check compares them with
  upstream `hits`. Python has 10 reads without hits; rtao may declare targets for them and the
  checker accepts a superset.
- **Generated namespace methods.** `#[read(namespace = Neurons)]` emits an inherent method
  `impl<'v> Neurons<'v> { pub async fn neurons(&self, netuid: NetUid, lite: bool) -> … }`.
  Inherent impls may live anywhere in the crate, so no aggregation step is needed. The 14
  namespace types and the curated methods (`balances.get/get_many/existential_deposit`,
  `staking.get/positions`, `subnets.burn/commit_reveal_enabled/metagraph/commitments/info/all`,
  `neurons.all`; `PY/namespaces.py:99-213`) are hand-written in `rtao/src/namespaces.rs`. A
  category without a namespace is a compile error, which replaces the `--namespaces` gate.
- **Helpers.** `scalar_read!` mirrors `scalar_read` (`PY/reads/base.py:156-166`); `utf8_text`
  is ported for `Value` inputs.
- **Address params.** Params typed `Address` accept a wallet, keypair or signer through
  `impl Into<Address>`, the analogue of `coerce_address` (`PY/reads/base.py:108-128`).
- **Render hints** `Grouped`/`Matrix` are carried in `ReadInfo` for the CLI.

### 4.3 One registry, several projections

| Projection | Python | rtao |
|---|---|---|
| dispatch by name | `client.read(name, **params)`, `execute_tool(op, args)` | `client.read(name, json!({...}))`, `executor.execute_tool(op, args, wallet)` |
| agent catalog | `list_tools()`, `list_reads()` | `rtao::intents::registry::list_tools()`, `rtao::reads::list_reads()`; `rtao tools`, `rtao reads` |
| CLI | `btcli tx <op>`, `btcli query <name>` generated (`PY/cli/tx.py:1-10`, `PY/cli/query.py`) | `rtao tx <op>`, `rtao query <name>` generated at runtime with the clap builder API (§10) |
| docs | website `generate.py` reference pages | `xtask docs` reference pages (§13) |
| parity | — | `xtask parity` compares projections with the upstream catalogs (§4.6) |

### 4.4 Executor, plan, policy, results: type and function map

| Python (file:line) | Rust |
|---|---|
| `IntentPreflight` (`PY/intents/base.py:109-134`) | `struct IntentPreflight { effects, warnings, blocks, required_free: Option<Balance>, available_free: Option<Balance>, estimated_fee: Option<Balance>, facts: Vec<(String, String)>, facts_title: &'static str }` |
| `BuiltCall(call, extras)` (`:414-422`) | `struct Built { call: CallBytes, extras: serde_json::Map<String, Value> }` |
| `Spend = Balance \| UNBOUNDED \| None` (`PY/intents/_money.py:37-48`) | `enum Spend { None, Bounded(Balance), Unbounded }` |
| `Policy(max_fee_tao, max_spend_tao, allowed_netuids, allow_raw_calls)`; `check` (`PY/intents/plan.py:21-85`) | `struct Policy { max_fee: Option<Balance>, max_spend: Option<Balance>, allowed_netuids: Option<BTreeSet<NetUid>>, allow_raw_calls: bool }`; `fn check(&self, intent: &dyn Intent, fee: Option<Balance>) -> Vec<String>`. Fail-closed rules identical, including "max fee set and fee unknown ⇒ violation" and checks run on `semantic_intent()` |
| `Plan` + `to_dict` (`plan.py:89-135`) | `struct Plan { op, summary, signer, signer_address, fee, effects, warnings, violations, #[serde(skip)] call, extras, spend, args }`; `ok()`; `to_json()` adds `fee_tao`, `spend_tao`, `spend_unbounded`, `ok`, `docs_url` |
| `Executor.plan/execute/execute_tool/submit_shielded/submit_call/prepare_call/submit_signature/preflight/tools` (`PY/executor.py:596-1319`) | `impl Executor` with the same methods; `ExecuteOptions { proxy_for, force_proxy_type, period: Option<u64> = Some(128), wait_for_inclusion = true, wait_for_finalization = true, retries = 0, wait_for_registration = true, registration_timeout: Option<Duration>, policy: Option<Policy>, progress: Option<Arc<dyn Fn(Progress) + Send + Sync>> }` |
| `_compose_intent_call`, `nest_origin_wrappers`, `_intent_accounts`, `_coerce_addresses` (`:99-246`) | private fns of the same names (minus underscore) |
| `_complete_subnet_registration` (`:333-568`) | `complete_subnet_registration`: receipt → rescan inclusion block → scan to head → follow new heads; accepts only a matching `NetworkAdded` not still queued; progress `queued`/`waiting`/`registered` |
| `_pure_created_data` (`:571-593`) | same; `create_pure_proxy` extras |
| `ExtrinsicResult` (`PY/result.py:697-749`) | `struct ExtrinsicResult { success, message, block_hash, extrinsic_id, explorer_url, fee, events, error: Option<ChainError>, data }`; `raise_for_failure`; `to_json` |
| `ChainError(message, name, code)` with `remediation`, `description`, `docs_url`, `to_dict` (`:501-562`) | `struct ChainError { message, name: Option<String>, code: ErrorCode }` with the same methods |
| `PolicyError` (`:687-693`) | `PolicyError { violations }` (code `policy_violation`) |
| `BittensorError` ← `ConnectionNotReady`, `RpcConnectionError` ← `RpcPolicyError`, `ChainError`, `PolicyError`; `UnitMismatchError` (`:45-73`; `PY/balance.py:40`) | `enum Error { ConnectionNotReady, RpcConnection(String), RpcPolicy { status_code, retry_after, policy, reason }, Chain(ChainError), Policy(PolicyError), UnitMismatch(UnitMismatchError), Auth(AuthError), Timelock(TimelockError), Keyfile(String), WrongPassword, Ledger(String), Vault(String), Bridge(String), InvalidArgument(String), Core(CoreError) }` plus `Error::code() -> ErrorCode` |

### 4.5 Error taxonomy

- **`ErrorCode`.** 18 variants (`PY/error_map.py:23-41`); `as_str()` yields the snake_case
  value; `#[non_exhaustive]`.
- **Tables (transpiled)** from `error_map.py`:
  - `NAME_TO_CODE` (390 names);
  - `CUSTOM_TRANSACTION_ERRORS` (codes 0–27 and 255 → catalog names, `:488-518`);
  - `INVALID_TRANSACTION_ERRORS` (16 needles, `:524-541`);
  - `DISPATCH_ERRORS` (`:555-795`).

  From `result.py`: the substring fallback (`_SUBSTRING_FALLBACK`, `:291-304`) and the
  remediation override tables. Descriptions come from `error_descriptions/*.py`.
- **`classify_error`** keeps the precedence of `PY/result.py:483-498`: name table → dispatch
  table → `*RateLimit*` → substring fallback → `unknown`.
- **`chain_error_from_substrate_request`** parses `"Custom error: N"` and `{"Custom": N}`
  payloads, standard `InvalidTransaction` texts, and watch-status fatals (`:574-634`).
- **Gates**, the mirror of `UP/sdk/python/codegen/check.py --names` (`:349-455`), run by
  `xtask codegen check --names` against `rtao-generated::errors::ERRORS`:
  - every chain error name classifies, and every classified name still exists;
  - every classified name has a non-empty description;
  - **the custom-code key set equals the `u8` codes parsed with `syn` from
    `common/src/transaction_error.rs` at the pinned tag** (the `impl From<CustomTransactionError>
    for u8` match arms). Python's gate hardcodes `frozenset(range(28)) | {255}`
    (`check.py:349-351`).

### 4.6 The parity matrix and `cargo xtask parity`

Parity data lives in `parity/` (schemas in App. B):

- `parity/upstream.toml`: repo, tag, commit, Python package version (`11.3.1.dev0`).
- `parity/map.toml`: one row per watched upstream file, with `py`, `rs` (one or more paths),
  `kind`, `synced_blob` (git blob SHA of the upstream file when the Rust counterpart was last
  reconciled), `synced_tag`, `notes`.
- `parity/divergences.toml`: intentional behavior differences. Each has an id, the upstream
  symbol, the rtao behavior, a rationale, a test that pins it, and an optional upstream
  issue/PR. Initial entries:

  | Id | Divergence |
  |---|---|
  | D-001 | `hotkey_owner`/`miner_collateral` treat the all-zero `Owner` default as "none" (R:A2 §9 Q9). [inferred] behavior of `Default`-modifier maps; verify on a localnet in M1 before adopting |
  | D-002 | gather reads are pinned (§3.12) |
  | D-003 | the library's default `PromptPolicy::Never` (§6.2) |
  | D-004 | genesis check before signing (§3.3) |
  | D-005 | NonceStore is async (§7.1) |
- `parity/allow.toml`: temporary gaps, each with a milestone. It must be empty at M6.

`cargo xtask parity check` runs five layers and fails on any unexplained difference.

1. **File layer.**
   - List the upstream tree at the pinned commit for the watched roots
     (`sdk/python/bittensor/**`, `sdk/python/codegen/**`, `sdk/python/tests/**`,
     `sdk/bittensor-core/**`, the three chain-source files of layer 4, the catalogs,
     `docs/guides/signed-requests.mdx`). It uses a managed partial clone at `target/upstream.git`
     (`git clone --filter=blob:none`; `git ls-tree -r <commit>`).
   - A new upstream file without a map row fails. So does a row whose `py` no longer exists.
   - A row whose `synced_blob` differs from the current blob fails, and the report includes
     `git diff <synced_blob> <current_blob>`.
2. **Symbol layer.** For every changed `.py`, parse old and new blobs with `ruff_python_parser`
   (pure Rust, git-pinned, tooling only) and diff:
   - module-level names; classes, their dataclass fields (name, annotation, default, `help`) and
     `ClassVar`s (`op`, `signer`, `origin`, `verify`, `wraps`, `all_amount_fields`, mev flags);
   - functions and methods with parameters and defaults;
   - decorator arguments (`@read(...)`, `@register`).

   Each changed symbol is joined to its Rust item through the `#[doc(alias = "bittensor.…")]`
   index, which `syn` builds from the rtao tree. The report says "Python `X` changed; Rust
   counterpart `crates/rtao/src/…:line`" or "no Rust counterpart".
3. **Catalog layer.**
   - rtao's projections (`list_tools()`, `list_reads()` and the error catalog, dumped by an
     `xtask` binary linking `rtao`) are compared with the upstream catalogs at the tag.
   - Intents: name, `python_class`, signer, origin, verify, wraps, normalized `input_schema`
     (field set, types, required set, descriptions).
   - Reads: name, params, param_docs, category, docs text, and hits ⊆ targets.
   - Errors: code set, remediation, chain name → code, description.
   - Defaults are not in the schema. They are compared between the Python-AST field defaults
     (layer 2) and `IntentInfo.fields[*].default`.
4. **Chain-source layer.** `syn` parses `common/src/transaction_error.rs` (the custom-code set),
   `runtime/src/fee_filters.rs` (the `call_filter_group!(ColdkeyPaysFeeCalls, [...])` list versus
   rtao's `COLDKEY_PAYS_FEE_CALLS`), and `pallets/subtensor/src/rpc_info/metagraph.rs`
   (`SelectiveMetagraphIndex::from_index` arms 0..=76, which generate
   `rtao::ext::types::SelectiveMetagraphIndex`). It also records the `freeze_struct` hashes of
   `rpc_info/*.rs` as layout tripwires (R:A2 §4.2).
5. **Transpile layer.** `xtask transpile --check` regenerates `crates/rtao/src/py_tables/**` from
   the Python literals at the tag and diffs.
   - Supported forms: `str/int/float/bool/None`, implicit string concatenation, tuples, lists,
     dicts, sets, `frozenset`, enum attribute references (`_C.INVALID_ARGUMENT`), and simple
     constructor calls with literal arguments (`Hyperparam(kind, doc, short=…)`).
   - Anything else is an error, which forces a hand port.
   - Targets: `error_map` tables, `error_descriptions`, `hyperparams.HYPERPARAMS`,
     `intents/hyperparameters.OWNER_HYPERPARAMETERS`, `intents/proxy.PROXY_TYPES`,
     `fee_filters.COLDKEY_PAYS_FEE_CALLS`, the `settings` tables, `basket_index_data`,
     `evm/networks`, `codegen/check.{COVERED_PALLETS, RAW_ONLY}`, and the `result.py`
     remediation/substring tables.

`cargo xtask parity bless --file <py>…` records the current blob SHA after a human has ported
the change. The PR diff then shows both the Rust edit and the blessed SHA, which is the
mechanical audit trail. `cargo xtask parity report --from vA --to vB` prints the same analysis
as Markdown for bump PRs.

**Behavioral vectors** (`parity/vectors/`) come from `parity/oracle/*.py`, run by `xtask oracle`
in Docker. The image is built from the pinned tag: `sdk/bittensor-core-py` + `sdk/python` via
maturin and uv. The PyPI wheel is used when its version maps exactly to the tag (R:H §2.5). The
oracle dumps, as committed JSON:

- every intent sample from `tests/harness/samples.py`, with its `to_dict`, `json_schema`, built
  call `(module, function, params)` under `FakeSubstrate` defaults, `summary`, `spend`,
  `touches_netuids`, `affects_all_subnets`, and the `plan()` JSON;
- every read sample's output under `FakeSubstrate`, at head and pinned;
- pure-function vectors: weights `_conform`, `normalize`, `clip_to_max_weight` over seeded
  inputs; `hyperparams.to_raw/normalized/annotate`; `Balance` parse/format; `classify_error` over
  a message corpus; `chain_error_from_substrate_request` over recorded RPC error payloads;
  `fee_filters.charges_coldkey_fee`; `timelock.round_at/reveal_time/parse_duration`;
  `pow_registration.registration_prefix/seal/effective_difficulty`; `metagraph._build` over
  recorded `get_metagraph` payloads; `utf8_text`.

Rust tests replay them through `rtao-testkit::FakeSubstrate` seeded from the same exported
defaults (`DEFAULT_STORAGE`, `DEFAULT_CONSTANTS` and `DEFAULT_RUNTIME`,
`UP/sdk/python/tests/harness/fake_substrate.py:34-73`). The vectors are regenerated only on
upstream bumps, in the bump PR.

### 4.7 How an upstream addition is detected in CI

| Upstream event | Detector | Failure message |
|---|---|---|
| New intent (new `@register` class) | catalog layer (name missing from `list_tools()`), file/symbol layers (changed `intents/*.py`), `codegen check --coverage` (the new chain call is neither wrapped nor raw-only) | `parity: intent 'foo' (FooIntent, intents/x.py) has no Rust counterpart` |
| New read | catalog layer; symbol layer | `parity: read 'bar' missing` |
| New error name or reclassification | `codegen check --names`; transpile layer | `names: chain error 'Baz' does not classify` |
| New custom tx code | chain-source layer (syn) | `custom codes differ: +28` |
| Changed default or param | symbol layer; catalog layer (schema) | `ServeAxon.protocol default 4 → 5` |
| New `_transport` behavior | file layer (blob changed) + symbol diff | file diff listed for porting |
| New Python unit test | file layer (`tests/unit/*.py` blob changed) | test diff listed; port the case into the mirrored Rust test |

The job definition is `parity` in App. A. It runs on every PR (offline, against the pinned tag
via the cached partial clone) and nightly against `main`, `testnet` and `mainnet` branch heads
as a non-blocking report.

---

## 5. Money and units

### 5.1 `Balance` (mirror of `PY/balance.py`)

```rust
#[doc(alias = "bittensor.balance.Balance")]
#[derive(Clone, Debug)]
pub struct Balance { rao: u64, netuid: NetUid, symbol: Option<Arc<str>> }

impl Balance {
    pub const fn from_rao(rao: u64, netuid: NetUid) -> Self;
    pub fn from_tao(tao: impl Into<DecimalInput>) -> Result<Self, Error>;               // exact: str/Decimal; floats only via explicit f64 ctor
    pub fn from_alpha(alpha: impl Into<DecimalInput>, netuid: NetUid) -> Result<Self, Error>;
    pub fn from_amount(amount: impl Into<DecimalInput>, netuid: NetUid) -> Result<Self, Error>;
    pub fn rao(&self) -> u64;  pub fn netuid(&self) -> NetUid;
    pub fn tao(&self) -> Result<f64, UnitMismatchError>;     // errors on alpha, as Python raises
    pub fn alpha(&self) -> Result<f64, UnitMismatchError>;   // errors on TAO
    pub fn amount(&self) -> f64;                              // unit-agnostic
    pub fn decimal(&self) -> rust_decimal::Decimal;           // exact (PY/balance.py:118-125)
    pub fn unit(&self) -> &str;                               // τ, chain symbol, or α + subscript netuid
    pub fn checked_add(&self, o: &Balance) -> Result<Balance, Error>;   // UnitMismatch if netuids differ
    pub fn checked_sub(&self, o: &Balance) -> Result<Balance, Error>;
}
// Display == Python __repr__ formatting; PartialEq/Ord compare within one unit, else error via try_cmp.
pub fn tao(x: f64) -> Balance; pub fn alpha(x: f64, netuid: NetUid) -> Balance; pub fn rao(x: u64, netuid: NetUid) -> Balance;
```

- **Representation is `u64` rao, unlike Python's unbounded `int`**, because the chain types are
  `TaoBalance(u64)`/`AlphaBalance(u64)` (R:C1 §1). Arithmetic is checked: overflow returns an
  error instead of wrapping. A float is never held internally (`PY/intents/_money.py:1-31`).
- **Netuid tagging is a runtime property, not a type-level one.** R:A2 §8 decision 3 calls
  `Alpha<const N>` impractical. The netuid-tagged value is the proven design. `UnitMismatchError`
  mirrors `PY/balance.py:40-145`.
- **Symbols.** Token symbols come from the connection: `Substrate::balance(rao, netuid)` tags
  with the `TokenSymbol` map loaded at connect through a 7-day disk cache
  (`PY/client.py:613-633`, `PY/config.py:22-27`), shared with btcli's
  `btcli_token_symbols.json`.

### 5.2 Intent money inputs

- **`Money`.** `pub enum Money { Amount(Balance), All }`. Serde accepts a number, a decimal
  string matching `^\d+(\.\d+)?$`, or `"all"` (the latter only for `all_amount_fields`),
  mirroring `money_schema` and `_money.py:29-69`. Negative amounts are rejected.
- **`call_amount` and unit enforcement** (`PY/intents/_money.py:89-163`). The parameter's
  runtime type identity (`TaoBalance` or `AlphaBalance`) comes from the generated call args
  (`rtao-generated::calls::ARG_IDENTS`, a static table emitted next to the builders). A wrong
  netuid tag is an error that names the call and parameter. The `_FALLBACK_AMOUNT_UNITS` hand
  table (`:89-128`) covers only pre-newtype metadata and is transpiled for parity.
- **`"all"` resolution** reproduces Python exactly:
  - free − `ExistentialDeposit` − 500,000 rao headroom (`PY/intents/staking.py:248-267`);
  - stake info for alpha;
  - `u64::MAX` claim-all where Python uses it (`:296-313`);
  - `amount − 1 rao` for `move_swap_stake` (`:184-205`).

### 5.3 Chain newtypes (rtao-core `units`)

`NetUid(u16)`, `MechId(u8)`, `NetUidStorageIndex(u16)` (`= mechid * 4096 + netuid`,
`PY/settings.py:31`; `UP/pallets/subtensor/src/subnets/mechanism.rs:30-43`), `TaoBalance(u64)`,
`AlphaBalance(u64)` and `PerU16(u16)` are transparent SCALE newtypes with
`EncodeAsType`/`DecodeAsType` and `Into<Value>`. They encode as the bare integer, matching the
chain (R:C1 §1).

- `NetUidStorageIndex::new(netuid, mechid)` and `.split() -> (NetUid, MechId)`, since
  `get_mechagraph` returns the index in its `netuid` field (R:C2 §0.5).
- `PerU16::fraction() -> f64` (raw/65535).
- Ratio denominators `PerU16=65535`, `Percent=100`, `Permill=1e6`, `Perbill=1e9` come from
  `RATIO_TYPE_DENOMINATORS` (`PY/hyperparams.py:32-52`).

### 5.4 Fixed point

`substrate_fixed` values arrive as composites `{bits}` (R:F §4). Each kind is a newtype that is
exact on the wire, with float views only for display or Python-parity outputs:

| Type | Repr | Python conversion (cite) | Rust API |
|---|---|---|---|
| `U64F64` | `u128` bits | bits / 2^64 (`PY/metagraph.py`, `PY/reads/root.py:79-94`) | `to_f64()`, `int_part() = bits >> 64`, `to_decimal_string()` (exact) |
| `I96F32` | `i128` bits | bits / 2^32; `RootClaimableThreshold` uses `bits >> 32` whole rao (`PY/reads/root.py:54-77, 437-451`) | `to_f64()`, `int_part()` |
| `I64F64` | `i128` bits | bits / 2^64 (`SubnetEmaTaoFlow`, `PY/reads/subnets.py:85-111`) | `to_f64()` |
| `I32F32` | `i64` bits | integer part `bits // 2^32` for `alpha_sigmoid_steepness` (`PY/reads/subnets.py:126-147`) | `int_part()` (floor division semantics, as Python `//`) |

- **Float equality.** Python-parity outputs that are floats (e.g. `drain_ratio`) are compared in
  the vectors with exact bit equality, because both sides compute `bits / 2.0f64.powi(n)` in
  IEEE double. Any formula that differs from Python's is flagged in the transpile/symbol diff.
- **Weight conformance is the one place Python does float math that ends up on chain.**
  `normalize` and `clip_to_max_weight` (`PY/intents/weights.py:54-114`) work on `float` and
  quantize with `round()` at the end. The port uses the same left-to-right summation order and
  **`f64::round_ties_even`**, because Python's `round` rounds half to even while Rust's
  `f64::round` rounds half away from zero. The oracle's weight vectors pin u16 outputs that are
  bit-identical, including `.5` ties.

### 5.5 Hyperparameter units

`rtao::hyperparams` mirrors `PY/hyperparams.py`.

- **Transpiled:** the `HYPERPARAMS` table (kind, doc, short, signed, bits, minimum, maximum),
  `KINDS`, and the codec and ident maps (`_IDENT_KINDS`, `_IDENT_CODEC`, `_KIND_CODEC`).
- **Ported:** `kind_of`, `metadata_kind`, `normalized`, `annotate`, `value_forms`,
  `proportion_to_raw`, `to_raw` (a decimal point means human form; a plain integer means raw;
  `PY/hyperparams.py:581-770`).
- **`--units` gate.** `xtask codegen check --units` asserts that the hand table never contradicts
  the metadata identity of each storage value (`UP/sdk/python/codegen/check.py:472-507`).

---

## 6. Wallets and signing

### 6.1 Keyfiles and the wallet directory (mirror of `PY/wallet.py`, `PY/wallets.py`, `PY/keyfiles.py`)

- **Layout.** `~/.bittensor/wallets/<name>/coldkey` (encrypted), `coldkeypub.txt`,
  `hotkeys/<hotkey>` (plaintext by default), `hotkeys/<hotkey>pub.txt`. `Wallet::new("cold/hot")`
  parses the compound form with the same validation messages (`PY/wallet.py:29-70`).
- **Lazy loading.** `Wallet::{coldkey, coldkeypub, hotkey, hotkeypub}` load keyfiles lazily
  (`OnceCell`) through bittensor-core:
  - `keyfile_data_is_encrypted*`, then `decrypt_keyfile_data` (NaCl argon2i13 with the fixed
    salt, Ansible Vault, or legacy Fernet with PBKDF2 at 10M iterations), then
    `deserialize_keypair_from_keyfile_data`;
  - `deserialize_keypair_from_keyfile_data` handles every btwallet quirk: the leading-space
    `" ss58Address"` key, numeric-string `cryptoType`, the secret precedence
    phrase → seed → private key → ss58, and a retry with the other crypto type on address
    mismatch (`CORE/keyfiles/mod.rs:213-364`).
- **No reimplementation.** rtao implements none of these formats; byte compatibility with btcli
  is the core's responsibility. That is the main benefit of the veneer (R:F §3.3 lists the
  pure-Rust alternative rtao does *not* need).
- **Creation.** `create_new_coldkey`, `create_new_hotkey`, `regenerate_*`, `regen_*_pub`
  (`PY/wallet.py:169-360`, `PY/wallets.py:214-386`) use `Keypair::generate_mnemonic` and
  `from_mnemonic`/`from_seed`, then `serialized_keypair_to_keyfile_data` and
  `encrypt_keyfile_data` for coldkeys. Writes follow `PY/keyfiles.py:128-163`: temp file +
  rename, files `0o600`, directories `0o700`, refusal to overwrite without `overwrite = true`.
  Hotkeys are created unencrypted, as in Python (`PY/wallets.py:239-245`).
- **Unlock off the reactor.** Unlock runs in `spawn_blocking` (Argon2i, 512 MiB); one unlock per
  process per role is cached, mirroring `WalletSigner` (`PY/signing.py:143-224`).

### 6.2 Password resolution (exact order of `PY/wallets.py:62-118`)

1. explicit `password`;
2. `BT_WALLET_PASSWORD`;
3. the per-keyfile variable `BT_PW_<keyfile path with separators and dots → "_", uppercased>`
   (`PY/keyfiles.py:102-104`), decoded with `bittensor_core::keyfiles::get_password_from_environment`
   (base64 XOR with the variable name, `CORE/keyfiles/mod.rs:111-125`);
4. `password_file` argument or `BT_WALLET_PASSWORD_FILE` (one line, trimmed);
5. macOS Keychain, service `bittensor.coldkey`, account = wallet name (`PY/macos_password.py:8,
   58-82`), feature `keychain`. It is read through the `security` CLI exactly as Python does, so
   `btcli wallet keychain save` entries are visible to rtao and vice versa;
6. the native macOS dialog (osascript, `--macos-password`);
7. an interactive prompt (`rpassword`), **only if `PromptPolicy` allows it**.

```rust
pub enum PromptPolicy { Never, IfTty, Always }
pub struct PasswordSources { pub password: Option<SecretString>, pub password_file: Option<PathBuf>,
                             pub keychain: bool, pub macos_dialog: bool, pub prompt: PromptPolicy }
pub fn resolve_wallet_password(wallet: &Wallet, src: &PasswordSources) -> Result<Option<SecretString>, Error>;
```

- **Prompting (divergence D-003).** The library defaults to `PromptPolicy::Never`; `rtao-cli`
  uses `IfTty`. The CLI behavior therefore equals btcli: without a TTY it refuses to prompt
  (`PY/keyfiles.py:42-62`). The library never blocks on stdin unless asked (R:E §4.2 #9).
- **Secrets** are held in `secrecy::SecretString` and zeroized on drop.

### 6.3 Signer trait design (mirror of `PY/signing.py` + `PY/_transport/protocols.py`)

```rust
/// Anything that can sign an extrinsic payload (PY/signing.py:49-70; protocols.py:29-50).
#[async_trait::async_trait]
pub trait Signer: Send + Sync {
    fn account(&self) -> AccountId32;            // ss58 via rtao-core
    fn public_key(&self) -> [u8; 32];
    fn crypto_type(&self) -> CryptoType;         // Ed25519 = 0, Sr25519 = 1 (CORE/keys/mod.rs:28-29)
    async fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, Error>;
    /// Optional capabilities (protocols.py:54-91); default None.
    fn metadata_verifying(&self) -> Option<&dyn MetadataVerifyingSigner> { None }      // CheckMetadataHash digest
    fn unsigned_extrinsic(&self) -> Option<&dyn UnsignedExtrinsicSigner> { None }      // whole prepared tx (Ledger/Vault)
    fn extension_payload(&self) -> Option<&dyn ExtensionPayloadSigner> { None }        // SignerPayloadJSON (browser)
    fn sync_signer(&self) -> Option<&dyn SyncSigner> { None }                          // needed by http_auth::sign
}
pub trait SyncSigner: Send + Sync { fn sign_sync(&self, payload: &[u8]) -> Result<Vec<u8>, Error>; }
```

- **Dispatch.** `sign_unsigned` dispatches on capability in the order of
  `PY/_transport/extrinsics.py:321-347`: unsigned-extrinsic signer, then extension payload
  signer, then plain `sign`.
- **Signature normalization.** `attach_signature` normalizes a 65-byte MultiSignature and checks
  the variant byte against `crypto_type` (`:202-239`).

| Implementation | Python counterpart | Notes |
|---|---|---|
| `KeypairSigner` | sp_core `Keypair` | sync + async; ed25519/sr25519 |
| `WalletSigner` | `PY/signing.py:143-224` | public data from pub files (never unlocks for addressing or fees); lazy unlock through §6.2 |
| `LedgerSigner` (feature `ledger`) | `PY/ledger.py:54-177` | `LedgerDevice` behind `spawn_blocking`; `metadata_digest` → `digest::metadata_digest` with `ChainInfo{spec_name: "node-subtensor", base58_prefix: 42, decimals: 9, token_symbol: "TAO"}`; `sign_unsigned_extrinsic` sends `payload ‖ generate_extrinsic_proof(...)`; refuses raw `sign(bytes)` (no blind signing, `UP/docs/guides/ledger.mdx:81-85` per R:G §4.4) |
| `VaultSigner` (feature `vault`) | `PY/vault/*` | UOS QR encoding, a local axum page serving the same `vault.html`, two scans within 8 blocks for shielded submissions |
| `ExtensionSigner` (feature `extension`) | `PY/extension/*` | bridge daemon on `127.0.0.1:39295` with token file `~/.bittensor/extension_bridge.token`, same `bridge.html` and the same wire messages, so it interoperates with the Python daemon (R:G §4.4) |
| `RemoteSigner` (trait example) | none | Rust-only pattern for KMS/HSM: implement `Signer`; the signing payload is exposed by `prepare_call` (§3.14) |

### 6.4 Signer role per intent, and `resolve_signer`

- **Declaration.** Each intent declares `signer = "coldkey" | "hotkey"` (`IntentInfo.signer`).
  The catalog has 75 coldkey and 8 hotkey intents: `associate_evm_key`, `commit_weights`,
  `reset_axon`, `reveal_weights`, `serve_axon`, `serve_axon_tls`, `serve_prometheus`,
  `set_weights` (R:I §5.3).
- **Wallet input.** `enum WalletLike { Name(String), Wallet(Wallet), Keyed(Arc<dyn KeyedWallet>),
  Signer(Arc<dyn Signer>) }` mirrors `PY/signing.py:93-140`, where a string becomes
  `Wallet("cold")` or `Wallet("cold/hot")`.
- **Resolution.** `resolve_signer(wallet, role)` (`:227-263`) returns a `WalletSigner` for a
  `Wallet`, passes a `Signer` through, and picks `coldkey`/`hotkey` from a `KeyedWallet`.
  `public_view(wallet, role)` never unlocks and is used for fee estimation and preflights
  (`:266-283`).
- **Address coercion.** Before composition, the executor replaces every `*_ss58`/`*_ss58s`
  field that holds a wallet-like value by the matching key's address (hotkey fields get the
  hotkey, others the coldkey; `PY/executor.py:142-157`). In Rust the field type `Address` holds
  either an ss58 string or a `KeyRef`, resolved at that step.
- **Hotkey-signed calls are fee-free.** They are `Pays::No`, so a zero-balance hotkey can submit
  them; fee-bearing hotkey calls on the allow-list are charged to the owning coldkey
  (`UP/runtime/src/fee_filters.rs:16-34`). The executor emits `COLDKEY_FEE_WARNING` exactly as
  `plan` does (`PY/executor.py:744-745`, `PY/fee_filters.py:68-98`).

---

## 7. Neuron networking

v11 ships no networking stack: no `Axon`, `Dendrite` or `Synapse`. Identity is `http_auth`
sign/verify, blacklisting is a metagraph lookup, and priority is framework rate limiting keyed by
the verified hotkey (`UP/docs/migration.mdx:455-480`, R:G §6.1). rtao mirrors `http_auth.py`
exactly in `rtao::http_auth` and adds the HTTP integrations as the Rust-only `rtao-http` crate.

### 7.1 `rtao::http_auth` (mirror of `PY/http_auth.py`, pure)

```rust
pub const PROTOCOL: &str = "btauth/1";  pub const VERSION: &str = "1";
pub const HEADER_VERSION: &str = "X-Bittensor-Version";   pub const HEADER_HOTKEY: &str = "X-Bittensor-Hotkey";
pub const HEADER_RECEIVER: &str = "X-Bittensor-Receiver"; pub const HEADER_NONCE: &str = "X-Bittensor-Nonce";
pub const HEADER_CRYPTO: &str = "X-Bittensor-Crypto";     pub const HEADER_SIGNATURE: &str = "X-Bittensor-Signature";
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(10);
pub const DEFAULT_ALLOWED_SKEW: Duration = Duration::from_secs(2);   // PY/http_auth.py:58-75

pub fn build_payload(scheme: Scheme, method: &str, path: &str, body: &[u8], nonce_ns: u64,
                     sender_ss58: &str, receiver_ss58: Option<&str>) -> Vec<u8>;   // :157-185
pub fn sign(signer: &dyn Signer, method: &str, path: &str, body: &[u8],          // needs signer.sync_signer(), as Python
            receiver_ss58: Option<&str>, nonce_ns: Option<u64>) -> Result<Headers, Error>;   // requires a synchronous signer (:188-239)
pub async fn verify(headers: &http::HeaderMap, body: &[u8], method: &str, path: &str,
                    self_hotkey_ss58: &str, opts: &VerifyOptions, store: &dyn NonceStore)
                    -> Result<Caller, AuthError>;                                       // :242-343
pub struct Caller { pub hotkey_ss58: String, pub nonce_ns: u64, pub crypto_type: CryptoType }
pub enum AuthError { MalformedAuth(String), WrongReceiver(String), StaleRequest(String),
                     ReplayedRequest(String), BadSignature(String) }               // messages == Python's
#[async_trait] pub trait NonceStore: Send + Sync { async fn check_and_store(&self, hotkey: &str, nonce_ns: u64) -> Result<bool, Error>; fn retention(&self) -> Duration; }
pub struct InMemoryNonceStore { /* Mutex<(HashSet<(String,u64)>, VecDeque<(Instant,(String,u64))>)>; retention 60 s */ }
```

**Exact payload.** UTF-8, eight lines joined by `\n`, **no trailing newline**:

```
btauth/1
<sr25519|ed25519>
<METHOD uppercased>
<request target: path + "?query", percent-encoded, as sent>
<sha256(raw body) lowercase hex>
<nonce ns, decimal>
<sender ss58>
<receiver ss58 or "-">
```

**Verification order** (normative, `UP/docs/guides/signed-requests.mdx:78-99`, R:D §1.5):

1. **shape**: version `== "1"`, hotkey, nonce (decimal), crypto (default sr25519, parsed
   case-insensitively or from the digits), signature (hex after an optional `0x`);
2. **receiver**: rejected if absent while `require_receiver`, or if it is not our hotkey;
3. **freshness**: `now − nonce > max_age` is stale; `nonce − now > skew` is in the future. This
   uses `i128` nanoseconds; Python compares floats (R:D §1.10);
4. **signature**: the payload is rebuilt with the *received* receiver value, and the signature
   verified **under the declared scheme only** with `bittensor_core::keys::verify`;
5. **store sanity**: `max_age + skew ≤ store.retention()`, checked at construction (Python raises
   `ValueError`);
6. **replay**: `check_and_store` runs only after the signature has verified, so unauthenticated
   traffic cannot poison the store.

The signature scheme is re-derived from the parsed type, never echoed from the header.

- **Crypto.** Signing uses sr25519 with the `b"substrate"` context, or ed25519, through the core
  `Keypair`. Python's verifier, through `keys::verify`, also accepts a signature over
  `<Bytes>payload</Bytes>` (`CORE/keys/mod.rs:402-410, 457-469`). rtao inherits this tolerance
  for parity, because it calls the same function, and **never produces** the wrapped form.
  - **Opt-in strict mode [proposal].** Rejecting the wrapped form needs a direct verifier, because
    the core's `verify` tries both forms internally. `VerifyOptions::reject_wrapped` would use
    `schnorrkel` 0.11.5 (already in the graph through the core) for sr25519, and `ed25519-dalek`
    2.2 for ed25519.
- **NonceStore is async (divergence D-005).** It allows Redis without blocking the reactor;
  Python's protocol is synchronous.
- **Golden vectors**, pinned in `crates/rtao/tests/test_http_auth.rs`, the mirror of
  `UP/sdk/python/tests/unit/test_http_auth.py:19-36` and `signed-requests.mdx:252-289`:
  - sender `//Alice`, receiver `//Bob` (`5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty`),
    nonce `1752076800000000000`, body `{"prompt": "hello"}` (19 bytes),
    `POST /generate?stream=false`, `sha256 = 341c5744…c3cc`;
  - the exact 8-line payload;
  - the ed25519 `//Alice` address `5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu` with the
    pinned signature `0x1a6b6319…f704`;
  - sr25519 sign→verify only, because its signatures are randomized;
  - every negative case in the Python file: lowercased headers, the crypto header omitted for
    sr25519, stale at +11 s, future at −3 s, replay, `retention = 0`, tampered body, method and
    path, the missing-header matrix and malformed values.

  The governing policy is quoted from the test file: a break "needs a protocol version bump, not
  a fixture update".

### 7.2 `rtao-http` (Rust-only)

- **Server** (feature `server`): `BtAuthLayer<S: NonceStore>`, a tower layer, and the axum
  extractor `VerifiedRequest { caller: Caller, body: Bytes }`.
  - The raw body is buffered up to a configurable limit **before** any JSON extractor runs.
  - The request target is `uri.path_and_query()` verbatim, which keeps the percent-encoded wire
    form (R:D §1.2).
  - On success `Caller` is inserted into the request extensions. Every `AuthError` becomes 401
    with body `{"detail": msg}`, mirroring the FastAPI recipe (`signed-requests.mdx:134-135`).
  - Streaming request bodies cannot be verified and are documented as such; responses may stream.
  - A separate `MetagraphPolicy` layer returns 403 for verified-but-unwelcome callers:
    registered, `validator_permit`, and a stake floor (`signed-requests.mdx:147-156`). It reads a
    shared `Arc<ArcSwap<Metagraph>>` that `rtao-neuron::MetagraphSync` keeps fresh.
- **Client** (feature `client`): `BtAuthClient` wraps `reqwest::Client`. It signs
  `method + url.path() + "?" + url.query() + body` with a **fresh nonce per attempt**, and
  retries happen above the signing step (`signed-requests.mdx:101-102, 163-167`). Helpers
  `post_json`, `get` and `fan_out(miners, req)` are bounded by a semaphore.
- **Store**: `RedisNonceStore` (feature `redis`) implements `SET hotkey:nonce 1 NX EX retention`
  (`signed-requests.mdx:97-99`).

### 7.3 Legacy v10 Synapse and Epistula (optional crates)

Three verifiable subnets still run legacy Synapse, three run Epistula dialects, and none has
adopted btauth/1 yet (R:D §6). Both are needed during migration but are not part of v11 parity.

- **`rtao-compat-synapse`.**
  - the `bt_header_*` codec;
  - a sha3-256 body hash over `required_hash_fields`, with a per-schema `PythonStr` rendering;
  - the dendrite signer for `"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"`;
  - an axon verifier with the 4 s + `timeout` window and a per-`hotkey:uuid` monotonic table;
  - status semantics and SSE streaming (R:D §2.9);
  - the v11 "bridge" verifier described in `signed-requests.mdx:291-321`.
- **`rtao-compat-epistula`.** The spec dialect `{sha256}.{uuid}.{ts_ms}.{signed_for}` with knobs:
  `allowed_delta_ms` (8000, or 15000 for BitMind), `reject_future`, `require_uuid` (Apex dialect),
  and `emit_secret_signatures`. It reuses the `NonceStore` trait for UUIDs (R:D §3.4).

### 7.4 QUIC (btlightning)

`rtao-quic` is an optional thin adapter over the `btlightning` 0.2.15 crate (quinn 0.11, rustls
0.23, MessagePack framing, sr25519 handshake bound to the certificate fingerprint; R:D §4).

- It gives wire compatibility with SN2, including the Python bindings for interop tests, behind
  an rtao `Transport` trait.
- It is not core: btlightning is single-vendor, has no per-request signatures, and its
  `protocol == 4` means "QUIC", which collides with every SDK's default (R:D §4.7, §4.9).
- rtao does not enable btlightning's `subtensor` feature, which pulls subxt 0.50.

---

## 8. Metagraph and typed models

### 8.1 Public model (mirror of `PY/metagraph.py`)

```rust
#[doc(alias = "bittensor.metagraph.Metagraph")]
pub struct Metagraph {
    pub netuid: NetUid, pub mechid: MechId,                 // mechid = raw netuid field / 4096 (PY/metagraph.py:384)
    pub name: String, pub symbol: String, pub block: u64,
    pub tempo: u16, pub last_step: u64, pub blocks_since_last_step: u64,
    pub owner_hotkey: AccountId32, pub owner_coldkey: AccountId32, pub network_registered_at: u64,
    pub num_uids: u16, pub max_uids: u16,
    pub price: Option<f64>,                                  // tao_in / alpha_in
    pub moving_price: Option<I96F32>,
    pub identity: Option<SubnetIdentity>,
    pub neurons: Vec<MetagraphNeuron>,                       // ordered by uid
    pub commitments: BTreeMap<u16, Option<NeuronCommitment>>,
    pub unregistered_commitments: BTreeMap<AccountId32, NeuronCommitment>,
    pub raw: ext::types::MetagraphInfo,                      // full typed wire struct (Python: raw dict)
}
#[doc(alias = "bittensor.metagraph.MetagraphNeuron")]
pub struct MetagraphNeuron {
    pub uid: u16, pub hotkey: AccountId32, pub coldkey: AccountId32, pub active: bool, pub validator_permit: bool,
    pub last_update: u64, pub block_at_registration: u64,
    pub rank: f64, pub trust: f64, pub consensus: f64, pub incentive: f64, pub dividends: f64, pub pruning_score: f64, // PerU16/65535; deprecated three are 0.0
    pub emission: Balance, pub alpha_stake: Balance, pub tao_stake: Balance, pub total_stake: Balance,  // total tagged alpha (PY:366)
    pub axon: Option<String>,                                // "ip:port", "[v6]:port", None when ip == 0 (PY:563-573)
    pub identity: Option<ChainIdentity>, pub commitment: Option<NeuronCommitment>,
    pub collateral_locked: Option<Balance>, pub collateral_min: Option<Balance>, pub collateral_earned: Option<Balance>, // None before spec 435
}
impl Metagraph { pub fn hotkeys(&self); pub fn coldkeys(&self); pub fn validators(&self); pub fn neuron(&self, uid: u16) -> Result<&MetagraphNeuron>; pub fn by_hotkey(&self, hk: &AccountId32) -> Option<&MetagraphNeuron>; }
```

`NeuronCommitment` mirrors `PY/metagraph.py:45-124`: fields, plus the derived `status ∈ {plain,
sealed, revealed, failed}`, `decrypted`, `age_blocks`, `duration` (with Python's hardcoded
`BLOCKTIME` for parity), `value` and `reveals_at` (drand).

### 8.2 Construction (mirror of `fetch`, `PY/metagraph.py:238-291`)

1. `let view = view.at(None).await?;` (pin).
2. One `SubnetInfoRuntimeApi.get_metagraph(netuid)` call, decoded **typed** into
   `ext::types::MetagraphInfo`: the 77-field wire struct
   (`UP/pallets/subtensor/src/rpc_info/metagraph.rs:14-113`) as a `DecodeAsType` struct.
3. When `commitments` is set, concurrently: `query_map(Commitments.CommitmentOf, [netuid])` and
   `query_map(Commitments.RevealedCommitments, [netuid])`.
4. `build(netuid, info, commitments)` is a line-by-line port of `_build`
   (`PY/metagraph.py:297-403`): `column()` padding, `PerU16 → f64`, `I96F32` price, and
   collateral `None` detection.

**Payload size and speed.**

- R:C2 §5.1 estimates about 205 B/UID: roughly 53 KB at 256 UIDs and about 510 KB for 2,500-UID
  Null subnets.
- Decoding runs in `spawn_blocking` above 64 KiB. The typed path avoids about 2 × n ss58
  renderings per refresh compared with the `Value` path.
- The differential test of §3.8 keeps both paths honest.

### 8.3 Beyond Python (Rust-only, `rtao/src/ext/`)

- **`SelectiveMetagraphIndex`.** Generated from the pallet's `from_index` match (0..=76) by the
  chain-source layer (§4.6); it is not in metadata (R:A2 §4.3).
  `selective_metagraph(view, netuid, &[idx])` returns an `ext::types::SelectiveMetagraphInfo`,
  all `Option` fields. Preset `VALIDATOR_SYNC` = indices
  `[7, 8, 9, 10, 26, 27, 28, 46, 47, 30, 52, 55, 56, 57, 59, 60, 61, 62, 66, 69]` (about 90 B/UID,
  one stake pass; R:C2 §5.2). Preset `MINER_SYNC` = `[52, 57, 69, 72]`.
- **`mechagraph(view, netuid, mechid)`** decodes `get_mechagraph` (`metagraph.rs:838-863`) and
  splits the `NetUidStorageIndex`.
- **Dead fields.** `subnet_emission`, `pending_root_emission`, `tao_dividends_per_hotkey`, `rank`,
  `trust` and `pruning_score` are `#[deprecated]` accessors, always zero on chain
  (`metagraph.rs:676-680, 782, 803-804`; R:C2 §0.4), kept for parity.
- **Persistence.** Python v11 has none: "persist `mg.raw` yourself" (`UP/docs/migration.mdx:308-317`).
  `rtao-neuron` offers `MetagraphStore`, which writes the raw SCALE bytes of the `get_metagraph`
  response with the header `MGRF ‖ u32 spec_version ‖ [u8;32] block_hash ‖ u64 block ‖ u16 netuid`.
  Reloading decodes through the codec of that spec, fetched or disk-cached (§3.4). The v10
  pickle format is not reproduced (R:A2 §4.4).
- **Other typed runtime-API models**, all by-name `DecodeAsType` with fields equal to the
  pallet's: `NeuronInfoLite`, `NeuronInfo`, `StakeInfo`, `DelegateInfo`, `DynamicInfo`,
  `SubnetState`, `SimSwapResult`, `HyperparamEntry`/`HyperparamValue`, `BasketSummary`, `AxonInfo`,
  `PrometheusInfo`, `ChainIdentityV2`, `SubnetIdentityV3`, `LockState`, `MinerCollateralState`
  (R:A2 §5.7 #6, R:C2 §1).

### 8.4 Read-side record types and sync cadence

- **Record types.** The records the reads return keep the Python names, fields and modules, as
  `PY/reads/__init__.py` exports them:
  - `SubnetInfo { netuid, tempo, burn: Balance, neuron_count }` (`reads/subnets.rs`);
  - `Neuron` (`reads/neurons.rs`);
  - `StakePosition` and `StakeValuation` (`reads/staking.rs`);
  - `DelegateInfo` and `DelegatedStake` (`reads/delegation.rs`);
  - `Commitment` (`reads/identity.rs`);
  - `SwapQuote` (`reads/prices.rs`).

  They are built from the typed models of §8.3 or from `Value`, mirroring each read body.
- **Sync cadence.** Python's answer is "refetch" (`UP/docs/migration.mdx:290-317`). rtao keeps that
  as the library primitive: `client.subnets().metagraph(netuid)` or
  `client.at(b).await?.subnets().metagraph(netuid)`.
- **`rtao-neuron::MetagraphSync` (§9.1)** adds the cadence policies observed in the wild
  (R:G §2.4): every N seconds (validators 3,600 s, miners 600 s, as in SN2), on each observed
  epoch (`LastEpochBlock` advances), or on events (`NeuronRegistered`, `AxonServed`,
  `HotkeySwapped`). It also forces a reconnect after N consecutive failures (SN2's threshold
  is 3).

---

## 9. Framework for owners, validators and miners (`rtao-neuron`, Rust-only)

v11 deliberately has no neuron framework (R:G §6.1), so this crate is the one large piece with
no Python mirror. It is built **only on rtao's public API**: intents, reads, the `Client`, and
`rtao-http`. Every chain interaction therefore inherits the parity guarantees of §4, and the
framework adds only orchestration.

### 9.1 Modules and lifecycle abstractions

```rust
pub mod config;     // NeuronArgs (clap), env + btcli.json layering
pub mod runtime;    // NeuronRuntime: JoinSet supervisor, SIGTERM/ctrl-c, graceful drain, health line
pub mod epoch;      // EpochClock (epoch_status read at one block), next_epoch(), wait_for_epoch()
pub mod sync;       // MetagraphSync: periodic/per-epoch refresh → ArcSwap<Metagraph>, hotkey-change events
pub mod weights;    // WeightSetter: cadence, mechanisms, commit-reveal via SetWeights, reconciliation
pub mod axon;       // AxonPublisher: external IP, idempotent serve_axon, periodic re-check
pub mod register;   // Registrar: burned / PoW / root registration + confirmation
pub mod owner;      // OwnerOps: register_subnet + wait, start_call, hyperparameters w/ admin window, mechanisms
pub mod validator;  // trait Validator + ValidatorLoop
pub mod miner;      // MinerServer (axum + rtao-http + MetagraphPolicy)
pub mod persist;    // atomic state files, MetagraphStore
#[cfg(feature = "metrics")] pub mod metrics;          // metrics + prometheus exporter
#[cfg(feature = "auto-update")] pub mod auto_update;  // release check + checksum/attestation + restart signal

#[async_trait::async_trait]
pub trait Validator: Send + 'static {
    type State: serde::Serialize + serde::de::DeserializeOwned + Default + Send;
    /// One scoring step against the current metagraph; returns per-uid scores to fold into the EMA.
    async fn step(&mut self, ctx: &ValidatorCtx, state: &mut Self::State) -> Result<StepScores, anyhow::Error>;
    /// Weights to submit (defaults to normalized EMA scores); return None to hold previous weights.
    fn weights(&self, ctx: &ValidatorCtx, state: &Self::State) -> Option<WeightsInput> { None }
    fn on_hotkey_changed(&mut self, uid: u16, state: &mut Self::State) {}          // reset per-uid state (R:G §2.4 #13)
}
pub struct ValidatorCtx { pub client: rtao::Client, pub metagraph: Arc<Metagraph>, pub http: rtao_http::BtAuthClient,
                          pub netuid: NetUid, pub uid: u16, pub epoch: EpochStatus }
```

**`ValidatorLoop::new(args).validator(v).run().await`** composes the pieces:
1. `MetagraphSync`: per epoch, or every 3,600 s (the SN2 default), with a full or
   `VALIDATOR_SYNC` selective fetch (§8.3).
2. Calls `Validator::step` on a tick, typically 100 ms (SN2 `LOOP_DELAY_SECONDS 0.1`, R:G §2.5).
3. `WeightSetter` submits on cadence.
4. State persists every 300 s and on shutdown.
5. Optionally, `AxonPublisher` serves an axon so miners can allowlist the validator IP (SN2
   pattern, R:G §2.3).

**`MinerServer::new(args).router(axum::Router).policy(MetagraphPolicy::validators_only(min_stake)).run().await`**
mounts `BtAuthLayer` + `MetagraphPolicy`, then starts `AxonPublisher` and `MetagraphSync`
(600 s; SN2 miner default, minimum 30 s).

### 9.2 Config and CLI conventions

- **Flags and env.** `NeuronArgs` uses the btcli v11 spellings and environment names:
  `-n/--network` (`BT_NETWORK`; `local` honours `BT_CHAIN_ENDPOINT`), `-w/--wallet`
  (`BT_WALLET`), `-H/--wallet-hotkey` (`BT_WALLET_HOTKEY`), `--wallet-path` (`BT_WALLET_PATH`),
  `--wallet-password-file` (`BT_WALLET_PASSWORD_FILE`; `BT_WALLET_PASSWORD` is honoured by §6.2),
  `--fallback-endpoints`, `--archive-endpoints` (`none` pins), `--retry-forever`, `--netuid`.
- **v10 aliases.** Dotted spellings are hidden aliases: `--subtensor.network`,
  `--subtensor.chain_endpoint`, `--wallet.name`, `--wallet.hotkey`, `--wallet.path`,
  `--axon.port`, `--axon.external_ip`, `--logging.level`. This is the SN2 precedent
  (`inference-labs-inc_subnet-2/crates/sn2-validator/src/cli.rs:9-24`, R:G §4.2).
- **Precedence.** flag > env > `~/.bittensor/btcli.json` > default, the btcli rule
  (`UP/docs/cli.mdx:21-22`); rtao reads the same file with the same keys (§10.3).
- **Framework-only knobs** use an `RTAO_` prefix (`RTAO_AXON_EXTERNAL_IP`, `RTAO_AXON_PORT`,
  `RTAO_STATE_DIR`), because v11 removed `BT_AXON_*` (`UP/docs/migration.mdx:166-180`).
- **Logging.** `tracing` with a compact key=value stderr formatter by default, data on stdout;
  `RUST_LOG` or `BITTENSOR_LOG`-style levels (R:E §6; `PY/cli/logs.py:32-41`).

### 9.3 Epoch clock and cadence

`EpochClock` reads `epoch_status` (the read of `PY/reads/epochs.py:84-128`: `Tempo`,
`LastEpochBlock`, `BlocksSinceLastStep`, `PendingEpochAt`, `SubnetEpochIndex`,
`get_next_epoch_start_block`, block time) at one block. It never uses the modular formula
(R:C2 §0.6). `wait_for_epoch` watches `LastEpochBlock`.

| Cadence | Rule | Notes |
|---|---|---|
| `EveryEpoch` | one submission per observed epoch | |
| `RateLimited { margin }` | submit when `blocks_since_last_update ≥ WeightsSetRateLimit + margin` | with commit-reveal on, the 100-block limit applies to commits and `LastUpdate` moves at commit time (R:G §2.6 #19) |
| `Interval(Duration)` | fixed wall-clock interval | |

### 9.4 Weight setting

`WeightSetter::submit(scores)` builds a `SetWeights` intent and runs it through `Executor` with
the validator's hotkey signer. Normalization, preflight and path selection are therefore exactly
Python's (`PY/intents/weights.py:54-440`):

- **Conformance.** Accept a `{uid: w}` map or parallel lists. Reject duplicates, negatives and
  non-finite values. Clip to `MaxWeightsLimit` by cutoff search unless the row is a pure
  self-weight. Max-upscale `round(w / max × 65535)`, using half-to-even rounding as in §5.4, and
  drop zeros. Then require `len ≥ MinAllowedWeights` (self-weight exempt). `raw_u16` is allowed
  only on Null subnets (no scaling).
- **Preflight at one block.** Reads `Uids`, `CommitRevealWeightsEnabled`, `WeightsSetRateLimit`,
  `LastUpdate[mechid·4096 + netuid]`, `MinAllowedWeights`, `MaxWeightsLimit`,
  `SubnetEpochConsensus` (plus `ValidatorPermit` under Null). Failures map to `not_registered`,
  `rate_limited` (with blocks remaining) or `not_authorized`.
- **Path.** `set_mechanism_weights` when commit-reveal is off. Otherwise
  `commit_timelocked_mechanism_weights(netuid, mecid, ciphertext, reveal_round, 4)`, with
  `(ciphertext, reveal_round)` from `bittensor_core::timelock::generate_commit_v2`; this is the
  same function Python calls through `get_encrypted_commit_v2`. The schedule state is read at one
  block, and the block time comes from `Aura.SlotDuration` (0.25 s on fast-blocks localnets,
  R:G §5.3).
- **Framework-only additions:**
  - **Version guard.** Read `CommitRevealWeightsVersion` and refuse early if it is not the
    intent's constant 4. Python hardcodes 4, and root can bump the version (R:C1 §7.5).
  - **Mechanisms.** Loop over `mecid < MechanismCountCurrent`; each mechanism has its own
    rate-limit slot (R:C1 §2.3).
  - **Hold previous** when every miner weight would be zero (SN2 pattern, R:G §2.6 #21).
  - **Bounded watch, then reconcile.** If the watch is lost (`FateUnknown`), poll
    `LastUpdate[uid]` or the commit queue for up to 60 s before declaring failure (SN2
    `weights.rs:104-180`, R:E §4.1 #3).
  - **No blind resubmission.** Only one pending commit per `(hotkey, mechanism)` may sit in the
    pool (provides-tag, R:C1 §2.2).
  - **`version_key`** is configurable, default 0, as in v11 (`UP/docs/migration.mdx:400-404`).

### 9.5 Serving, registration, hyperparameters, timelock

- **`AxonPublisher`.**
  - Detects the external IPv4 (configurable resolver, override `RTAO_AXON_EXTERNAL_IP`) and
    rejects non-global addresses (SN2 `registration.rs:22-98`).
  - Compares `Axons[netuid, hotkey]` first and calls the `ServeAxon` intent only on change,
    respecting `ServingRateLimit` (R:C1 §3).
  - Re-checks every 300 s; optional `ResetAxon` on decommission.
  - IPv6 encoding is supported (`ip_type = 6`); detection is IPv4-only by default (R:G §8.9).
- **`Registrar`.**
  - `BurnedRegister`, MEV-shield required (`PY/intents/registration.py:114-173`).
  - PoW through `rtao::pow_registration::mine_registration`, a port of
    `PY/pow_registration.py:85-253`: a fresh challenge every 12 s and a 5-block work window, with
    the core's CPU or OpenCL miners. It then calls `PowRegister`.
  - `RootRegister`.
  - Confirmation through the `uid` read and the `NeuronRegistered` event.
- **`OwnerOps`.**
  - `RegisterSubnet` with the queued-completion wait (§4.4).
  - `StartCall` gated by the `subnet_start_schedule` read.
  - `SetHyperparameter(name, value)` using the transpiled hyperparameter table and units (§5.5).
    It adds an admin-window check, `get_next_epoch_start_block − now ≥ AdminFreezeWindow`
    (default 10; R:C1 §5.2, R:C2 §4.2), and the rate limit `OwnerHyperparamRateLimit × tempo`.
  - `SetSubnetIdentity`, `UpdateSymbol`, `SetMechanismCount`, `TrimSubnet`.
  - Raw calls with no v11 intent (`trigger_epoch`, `register_network_with_identity`,
    `sudo_set_mechanism_emission_split`; R:G §8.11) go through `submit_call` with an explicit
    `Policy { allow_raw_calls: true }`.
- **Timelock.** `rtao::timelock::{Timelocked, encrypt, decrypt, round_at, reveal_time}` mirrors
  `PY/timelock.py` for data-plane sealing composed with btauth (`signed-requests.mdx:199-218`) and
  for `Commitments.set_commitment` timelocked fields.

---

## 10. CLI (`rtao-cli`, binary `rtao`)

### 10.1 Generated from the registries

`PY/cli/tx.py` and `PY/cli/query.py` turn every registered intent and read into a subcommand
whose options come from the dataclass fields or read params. `rtao-cli/src/tx.rs` and `query.rs`
do the same at startup with the clap **builder** API over `INTENTS`/`READS`. Adding an intent to
`rtao` therefore adds `rtao tx <op>` with no CLI edit.

```rust
pub fn build_tx_command() -> clap::Command {
    let mut tx = clap::Command::new("tx").about("Submit transactions (generated from intents).");
    for info in rtao::intents::registry::sorted_by_panel() {             // (help panel = pallet/privilege, op) as PY/cli/tx.py:471-495
        let mut cmd = clap::Command::new(info.op.replace('_', "-"))
            .about(first_line(info.description)).long_about(info.description);
        for field in info.fields { cmd = cmd.arg(arg_for(info, field)); } // --netuid <int>, money "<amount|all>", lists comma-separated,
                                                                         // *_ss58 accept ss58 | address-book | wallet "W/H" names
        tx = tx.subcommand(cmd.args(tx_globals()));                      // --yes/-y, --dry-run, --mev-shield/--no-mev-shield, --proxy-for,
    }                                                                    // --force-proxy-type, --signatory, signer flags
    tx
}
```

**Command body.** It mirrors `_make_command` (`PY/cli/tx.py:255-460`):
1. resolve addresses and fill missing values through prompts (TTY only);
2. `IntentInfo.from_args`, then `executor.plan`, then a review card from `IntentPreflight.facts`;
3. confirm, unless `--yes`. Without a TTY or with `--json`, a mutation without `--yes` is
   declined with exit 1, and missing required options exit 2 (`PY/cli/context.py:2088-2103`);
4. shield precedence (§3.10), `execute`, then output: JSON (Python's `to_dict` keys) or a table.

`query` mirrors `PY/cli/query.py:135-260`: comma-separated arrays, `*_ss58` fallback to the
configured wallet only when a param is required, and the `Grouped`/`Matrix` render hints.
`tools`, `explain <code|ChainError>` (`--chain`) and `call Pallet.function --args '{…}' [--sudo]
[--multisig]` (`PY/cli/call.py`) are generated from the same tables.

### 10.2 Hand-written groups (1:1 with `PY/cli/commands/*.py`)

| Group | Python file | Notes |
|---|---|---|
| `wallet` | `commands/wallet.py` (1,784) | create, new-coldkey, new-hotkey, regen-*, unlock, show, list, balance, overview, registrations, history, transfer, inspect, sign, verify, encrypt, decrypt, set/get-identity, associate-hotkey, swap-*, announce-coldkey-swap, keychain save/show/delete |
| `stake` | `commands/stake.py`, `stake_exit.py`, `stake_picker.py` | aliases of intent commands + show/list/auto/set-auto/wizard + child get/set/revoke/take |
| `root` | `commands/root*.py`, `root_helpers.py` | list, claim, allocate, register |
| `subnets` | `commands/subnets.py`, `metagraph_view.py`, `hyperparams_view.py` | list, show, hyperparameters, burn-cost, create-cost, price, conviction, register, create, metagraph |
| `conviction` (alias `lock`) | `commands/lock.py` | |
| `hparams` (alias `sudo`, `su`) | `commands/sudo.py`, `null_pruning.py` | set/get/get-take/set-take/check-start/start/set-symbol/identity/trim/trim-null(-batch)/stake-burn, mechanisms |
| `addr`, `proxy`, `multisig` | `commands/{addresses,proxy,multisig}.py`, `multisig_helpers.py` (1,455) | address book, proxy book, saved multisigs |
| `evm` | `commands/evm/*` | feature `evm` |
| `config` | `commands/config.py` | same keys as `PY/config.py:30-55` |
| `misc` | `commands/misc.py` + `weights`, `axon`, `collateral`, `timelock`, `crowd`, `extension`, `upgrade` | hidden top-level aliases as `PY/cli/main.py:118-135` |

### 10.3 Shared state with btcli

- **Same files, same schemas.** `rtao` reads and writes the same files btcli does: `$BTCLI_CONFIG`
  or `~/.bittensor/btcli.json` (settable keys `network`, `wallet`, `wallet_hotkey`, `wallet_path`,
  `json`, `quiet`, `signer_address`, `extension_browser`, `fallback_endpoints`,
  `archive_endpoints`, `retry_forever`, `proxy_for`, `mev_shield`; `PY/config.py:12-59`), the
  proxy, address and multisig books, the multisig cache, and the subnet-name and token-symbol
  caches with their TTLs (24 h and 7 d).
- **Same write discipline.** Atomic writes, unknown keys ignored, a corrupt file renamed
  `.corrupt.<ts>`.
- **Result.** A user can alternate between `btcli` and `rtao` on one machine, and the interop
  lane proves it (§11.6).

### 10.4 Scope per milestone

| Milestone | CLI surface |
|---|---|
| M2 | `query`, `tx`, `tools`, `explain`, `call`, `config` |
| M3 | `wallet` core (create, regen, list, balance, overview, sign, verify), `subnets` (list, show, metagraph, hyperparameters), `weights`, `axon` |
| M5 | everything else in §10.2, signer flags (`--signer wallet\|ledger\|vault\|extension`, `--ledger-account/index`, `--signer-address`), shell completions |

**CLI parity check.** The oracle dumps btcli's Typer command tree (names, options, aliases,
hidden flags) into `parity/vectors/cli_tree.json`. A test asserts that the `rtao` tree matches,
apart from listed divergences: the binary name, and `--version` reporting both versions.

---

## 11. Testing

The test tree mirrors `UP/sdk/python/tests/`: `tests/unit/test_X.py` becomes
`crates/<owner>/tests/test_X.rs`, and the harness becomes `rtao-testkit`. Upstream tests are the
executable spec. When a Python test file changes, the parity file layer lists it (§4.7), and the
new cases are ported into the mirrored Rust file.

### 11.1 Unit tests on upstream golden fixtures

Copies are synced by `xtask sync-upstream` into `upstream/v477/fixtures/`, with a SHA-256
manifest and Apache-2.0 provenance.

| Fixture (upstream path) | What it pins | rtao test (mirror of) |
|---|---|---|
| `sdk/python/tests/fixtures/golden.json` (1.5 MB, spec 461, raw V15+V14 metadata embedded) | `calls[10]` (incl. nested Sudo/Utility/Proxy), `signature_payloads[2]`, `extrinsics[2]` (fake sig `1..=64`, version 1), `multisig[3]`, `ss58[4]`, `constants[3]`, `storage_keys[10]`, `storage_values[8]` (incl. miss), `query_maps[4]`, `runtime_calls[4]`, `events`, `block`, `receipt`, `payment_info` (R:A2 §6.1) | `rtao-transport/tests/test_codec_golden.rs` and `test_storage_golden.rs` (mirrors of the same-named Python files). The vectors are replayed **through rtao's wrappers** (`RuntimeCodec`, `extrinsics::prepare/attach`, `utils::receipt`), so the seam is tested and not only the core. The core replays the same file internally (`CORE/codec/mod.rs:115-492`) |
| `sdk/python/tests/fixtures/shape_corpus/corpus.json` (797/802 types, 2,899 samples) | decoded-value shapes | `test_shape_corpus.rs`: `Decoded::value()` + `codec::value::to_corpus_json` (`CORE/codec/value.rs:58`) must equal each sample; coverage ≥ 95% (`test_shape_corpus.py:41`) |
| `docs/guides/signed-requests.mdx:252-289` + `tests/unit/test_http_auth.py:19-36` | btauth/1 payload, ed25519 signature, negatives | `rtao/tests/test_http_auth.rs` |
| RFC-0078 digest `affbcc25…11c0` for golden V15 + `ChainInfo(461, "node-subtensor", 42, 9, "TAO")` (`CORE/digest/mod.rs:184-201`) | `CheckMetadataHash` digest | `rtao/tests/test_ledger_signer.rs` (digest path) |
| `sdk/bittensor-core/fixtures/ledger_proof_vector.json` | Ledger proof bytes | same, feature `ledger` |
| `tests/unit/test_keyfiles_golden.py:25-140` | `//Alice` address, test mnemonic, NaCl round-trip, legacy Fernet parameters, JSON fields, `0600`/`0700` | `rtao/tests/test_keyfiles_golden.rs`, plus **encrypted** vectors made by btcli in the oracle (U8 until upstream commits them) |
| `timelock/epoch_schedule_vectors.rs` (core) | reveal-block prediction | exercised by the core's own tests; rtao adds the U2 regression test (expected to flip when fixed upstream) |
| Python-oracle vectors (`parity/vectors/*.json`, §4.6) | intent build/plan, reads under the fake, pure functions | `rtao/tests/test_oracle_*.rs` |
| Typed-vs-`Value` differential | typed models agree with the core decoder | `rtao/tests/test_typed_models.rs` over golden `runtime_calls` + rtao-recorded payloads (`get_metagraph`, selective, `get_dynamic_info`, `get_delegates`, stake info) |

**Spec-current fixtures.** The upstream goldens are frozen at spec 461 but remain valid for
offline decoding forever, because the metadata is embedded. Spec-current fixtures come from
`xtask record-golden` and `xtask record-corpus`, Rust ports of `record_golden.py` and
`record_shape_corpus.py` with the same JSON schema (R:H §5). They are recorded from the
`:v<spec>` localnet image and, read-only, from finney. Both generations are kept.

### 11.2 Registry tables (mirror of `test_intents_table.py`, `test_reads_table.py`)

- **Intents.** For every intent in `INTENTS`:
  - it has a sample (samples exported by the oracle from `tests/harness/samples.py`);
  - its schema is well-formed and equal to the catalog's;
  - `to_args → from_args` round-trips exactly, and unknown arguments are rejected;
  - it composes and plans offline against `FakeSubstrate`;
  - it has the right spend shape (`UNBOUNDED` set per R:A1 §3.14);
  - the tools catalog matches the registry.
- **Reads.** For every read in `READS`:
  - it has a sample and runs both at head and pinned to block 90;
  - **all chain calls of one invocation use one block hash** (§3.12).

### 11.3 Fake transports

- **`FakeSubstrate`** (mirror of `tests/harness/fake_substrate.py`) implements `Substrate` over
  maps.
  - It owns a `RuntimeCodec` built from the pinned metadata, so seeded values (`Value` or
    `EncodeAsType`) become real SCALE and decode through the production path.
  - Seeds may be partial dicts, as Python's are; for example `System.Account` is seeded as
    `{"data": {"free": 0, …}}`. `testkit::fill_defaults(type_id, Value)` walks the
    `PortableRegistry` and fills absent composite fields with the type's zero value (ints 0,
    `false`, empty sequences, `None`) before the core's `encode_id`. Account ids may be given as
    ss58, which the core's encoder accepts (`CORE/codec/encode.rs:1-5`). The Python seeds exported
    by the oracle therefore work unchanged.
  - Its API mirrors Python's: `seed`, `seed_default`, `seed_map`, `seed_constant`, `seed_runtime`
    (a value or a closure over params), `queue_result`, `seed_events`, plus the `submissions` and
    `inner_results` queues.
  - Its defaults (`DEFAULT_STORAGE`, `DEFAULT_CONSTANTS`, `DEFAULT_RUNTIME`) are exported by the
    oracle.
  - It records the `at` hash of every call.
- **`FakeNode`** (mirror of `tests/harness/fake_node.py`) is an in-process tokio-tungstenite
  JSON-RPC server.
  - It serves metadata, runtime version, scripted heads, storage from a golden file, and
    `author_submitAndWatchExtrinsic` with scripted `inBlock`/`finalized` and `golden.receipt`
    events.
  - Fault injection: drop, refuse (403/429 with `X-RateLimit-*`/`Retry-After`), silence, close
    1008, `-32004`/`-32005` errors, and "State already discarded".
  - It drives `test_rpc.rs` (mirror of `tests/unit/test_rpc.py`): `FateUnknown`, nonce-cache
    clearing, resubmission, `SubscriptionLost`, archive retry, pool rotation and terminal policy
    refusals.

### 11.4 Localnet e2e (docker)

- **Harness.** `rtao-testkit::localnet` adopts bittensor-core's e2e conventions
  (`UP/sdk/bittensor-core/tests/e2e_support/mod.rs:23-147`):
  - `E2E_ENDPOINT` reuses a running node; `LOCALNET_IMAGE` and `SKIP_PULL` are honoured;
  - ephemeral ports via `docker run -d -p 127.0.0.1::9944 -p 127.0.0.1::9945 <image> [True|False]`;
  - readiness is the image healthcheck turning `healthy` or `Imported #1` in the logs (180 s cap);
  - `docker rm -f` on drop.

  This is plain `docker` CLI (about 100 lines; R:H §7 #4).
- **Images.** Pinned by digest in `upstream/images.toml`. The default is
  `ghcr.io/raofoundation/subtensor-localnet:v477`, never `:monorepo-sdk`, which does not exist
  (R:H §6.1).

  | Lane | Image | Status |
  |---|---|---|
  | `pinned` | `:v<pin>` | required |
  | `mainnet` | `:mainnet` | required |
  | `testnet` | `:testnet` | required; failures open a "pre-break" issue |
  | `devnet` | `:devnet` | allowed-failure |
  | `main` | `:main` | weekly, allowed-failure |
  | `wasm-override` | latest node + released wasm via `SUBTENSOR_LOCALNET_RUNTIME_WASM` (`UP/node/src/chain_spec/localnet.rs:6-26`) | optional |
- **Block times.** Fast blocks (`True`, 250 ms) by default. A `False` lane (12 s) runs the
  timing tests: an observed timelocked reveal, metagraph sync at tempo boundaries, `wait_for_epoch`.
- **Localnet pitfalls** (R:H §3.5):
  - the faucet call exists only there;
  - constants are fast values;
  - the genesis is regenerated per container;
  - sudo is Alice; Alice, Bob and Charlie hold 1M TAO.
- **Inventory:**
  - **(a)** ports of the three Python e2e files (`test_null_consensus`, `test_pow_registration`,
    `test_timelock_commitment`);
  - **(b)** the behavior tests of bittensor-core's 112-test manifest (R:B §5): transfer
    exactness, staking, delegation, subnet register + `start_call`, serving, hyperparameters,
    identities, auto-stake, batch atomicity, policy caps, **MEV-shield pipeline**, crowdloans,
    nested sudo, timelocked commitment reveal, coldkey-swap announcement, EVM key association,
    planned vs live fee, multisig m-of-n, proxies, block subscription, metagraph fast path vs
    runtime API, snapshot-pinned reads, `quote_stake` slippage. The shield test proves the
    8-block era is accepted, which U1 says the core client fails; the 73 per-intent plan tests
    collapse into one table-driven test over `INTENTS`;
  - **(c)** rtao-specific: direct and timelocked weights (with `TimelockedWeightsRevealed`
    observed), mechanism weights, btauth between two rtao processes, a node restart mid-session
    (`FateUnknown`, nonce cleared, resubscribe), and a runtime upgrade mid-session (sudo
    `setCode` with the next release's wasm; the codec refreshes within about one block).
- **Execution.** nextest partitions; one container per test binary; isolation through fresh
  subnets and random wallets.

### 11.5 `core-oracle` lane (§1.4)

The same localnet block is read through rtao and through `bittensor_core::Client`
(`spawn_blocking`). Storage values, runtime calls, `compose_call` bytes and `partial_fee` are
asserted equal. Fees are compared on the same extrinsic bytes, as §1.4 describes.

### 11.6 Python-interop e2e (docker compose; Python only in test infrastructure)

**Services.**
- localnet `:v<pin>`;
- `oracle`: an image built from the pinned tag (`sdk/bittensor-core-py` via maturin +
  `sdk/python` via uv), or the PyPI wheel when its version maps exactly;
- the rtao test binaries.

**Scenarios.**
1. **btauth/1 across languages.**
   - The Python FastAPI miner recipe and httpx `HotkeyAuth` client, verbatim from
     `signed-requests.mdx:104-197`, against rtao's `BtAuthClient` and axum `BtAuthLayer`.
   - Cases: both schemes; GET with an empty body, POST JSON, percent-encoded path+query, unbound
     receiver; stale, future, replay and tampered requests.
   - Status codes and `detail` strings are asserted. rtao never emits the `<Bytes>` wrap.
2. **v11 miner ↔ Rust validator, and Python validator ↔ Rust miner** on a localnet subnet.
   Weights are set and observed on chain; `bt.Metagraph` equals `rtao::Metagraph` field by field.
3. **Wallet interop.** `btcli wallet create/new-coldkey/new-hotkey/regen-coldkey --seed` with known
   passwords; rtao loads the files and signs. Wallets created by rtao are read by
   `btcli wallet show/sign/verify`. Keychain entries are tested on the macOS runner, and EVM V3
   keystores both ways.
4. **Read parity on chain.** The oracle dumps `(await client.at(B)).read(name, **sample)` for all
   102 reads at a pinned block B; rtao dumps the same; the JSON must be equal (integers exact,
   floats bit-exact; divergences D-001/D-002 excluded by id).
5. **Plan parity on chain.** For every intent sample, the composed call bytes are equal (except
   timelocked ciphertexts, which are compared structurally), and fee, effects, warnings and
   violations are equal.
6. **Shared state.** The runtime disk cache, `btcli.json` and the address, proxy and multisig
   books written by one side are read by the other.
7. **CLI.** `btcli tools` vs `rtao tools` (upstream fields), `btcli query X --json` vs
   `rtao query X --json`, and the `cli_tree.json` comparison.

### 11.7 Property tests, fuzzing, benchmarks

- **Properties (proptest).**
  - SCALE `Value` round-trips for every registry type, using a Rust port of the corpus byte
    generator (`record_shape_corpus.py:47-130`);
  - `Balance` parsing and arithmetic (mirror of `test_balance_properties.py`);
  - weights conformance invariants (sum, `max ≤ limit`, `len ≥ min`, zero-drop);
  - btauth payload canonicalization;
  - era encode and `era_birth`; ss58 round-trip; `Money` serde; `NetUidStorageIndex` split/join;
  - hyperparameter `to_raw`/`normalized` inverses (mirror of `test_codec_properties.py`,
    `test_hyperparams_units.py`).
- **Fuzzing** (cargo-fuzz, a short nightly cron as SN2 does, R:H §3.8) targets
  `RuntimeCodec::parse`, extrinsic and event decoding, storage-value decode by type id, JSON-RPC
  frame parsing in `rpc.rs`, `http_auth::verify`, keyfile deserialization and ss58 decoding.
- **Benchmarks (criterion)**, compared against the core spec's acceptance numbers
  (`UP/sdk/bittensor-core-spec.md:360-382`: codec build 4 ms, decode 60–77 MB/s, `query_map`
  decode 599k entries/s, compose < 50 µs, sign+assemble 0.03 ms):
  - metadata parse;
  - typed vs `Value` metagraph decode at 256, 1,024 and 2,500 UIDs;
  - `query_map` page decode, compose and sign+assemble;
  - btauth sign/verify throughput;
  - `RpcSession` throughput against `FakeNode`.

  `xtask bench-live` ports `bench_transport.py`/`bench_neurons.py` for manual runs on finney. CI
  fails only on regressions above 30% (R:H §3.8).

---

## 12. Upstream tracking

### 12.1 Three kinds of drift, three feeds

| Drift | Feed | Tool |
|---|---|---|
| **Structural** (chain surface) | live finney/testnet/devnet metadata, and the `:v<spec>` localnet image for behavior | `xtask drift`: semantic metadata diff |
| **Semantic** (Python SDK behavior) | the monorepo at the next tag or branch head: `sdk/python/**`, catalogs, chain-source tables | `xtask parity report` (§4.6) |
| **Core** (chain-defined bytes) | `sdk/bittensor-core/**` diff between tags | golden/corpus tests, plus the parity file layer over `sdk/bittensor-core` |

One tag bump moves all three together (§1.7).

### 12.2 CI jobs

The full YAML is in App. A.

| Job | Trigger | Steps | Gate |
|---|---|---|---|
| `pr` | every PR | fmt; clippy `-D warnings`; nextest; doc tests; `xtask codegen --check`; `xtask transpile --check`; `xtask codegen check --coverage --names --units`; `xtask parity check`; cargo-deny; cargo-audit (0.22.2, the upstream pin); `msrv` (1.89); `cargo hack --feature-powerset --depth 2 check` | blocking |
| `e2e-localnet` | PRs touching transport/executor/intents/reads (path filter); nightly | matrix of §11.4 | `pinned` and `mainnet` blocking |
| `interop` | nightly; every bump PR | §11.6 compose lanes | blocking on bump PRs |
| `metadata-drift` | cron `17 */6 * * *` | per network: `chain_getFinalizedHead` → `state_getRuntimeVersion` → `state_getStorageHash(":code")` (the method of `UP/.github/scripts/mainnet-finalized-runtime.sh:19-26`); on change, fetch V15 and V16 and run the semantic diff (pallet, call, event and error **indices**; call args and type idents; storage hashers, key/value types and defaults; constants; runtime-API names, versions and signatures; transaction extensions with their extra and implicit types; extrinsic versions; V16 `deprecation_info`), attaching `subxt diff` output as a second opinion (the CLI is installed; it does not report events, errors or indices, R:H §2.3); run `xtask codegen`; open or refresh PR `drift: <net> <old>→<new>`, labelled `runtime-breaking` / `runtime-behavioral` / `runtime-additive` | PR review |
| `upstream-watch` | daily | `xtask parity report --from <pin> --to origin/{main,testnet,mainnet}`, catalog diff, bittensor-core diff; updates one issue per branch. Also watches PRs touching `eco-tests/**` (indexer storage contract, R:H §1.8), `sdk/python/bittensor/http_auth.py` and `docs/guides/signed-requests.mdx` | informational |
| `core-pin-bump` | finney finalized spec > pin **and** release `v<N>` is final | `xtask bump-upstream --tag v<N>` → PR with the parity report, regenerated vectors and code | merge needs green `pr` + `interop` + `e2e` on `:v<N>` |
| `upstream-preview` | nightly | swap the dependency to `branch = "main"`, then build and offline tests | allowed-failure |
| `live-probe` | cron every 6 h (`UP/.github/workflows/scheduled-smoke-tests.yml:7-9` cadence) | read-only: finality lag < 10 min, spec, `:code` hash, V15 digest vs pin, metagraph decode for netuid 1 and the largest netuid on finney | alert |
| `fuzz`, `bench` | nightly | §11.7 | bench > 30% regression fails |
| `release` | manual | `xtask compat-matrix`, changelog, tag `rtao-vX.Y.Z` | — |

The drift tool's extra guards (R:H §3.3) alert when:
- `Metadata_metadata_versions` no longer lists 15;
- `spec_name ≠ "node-subtensor"`;
- `transaction_version ≠ 1`;
- the extension list changes;
- any pallet or call index moves.

### 12.3 Metadata pins

- **Layout.** `upstream/metadata/<network>/<spec>.v15.scale` and `.v16.scale`, plus
  `upstream/metadata/index.toml` with one row per pin: `{network, spec_version, block_hash,
  code_hash, sha256_v15, sha256_v16, recorded_at, source}`. Rows are keyed like the runtime disk
  cache (`(genesis[..16], spec)`).
- **Source.** Pins come **only from live networks**. Localnet metadata differs: it has the faucet
  call at index 60 and `fast-runtime` constants (R:H §1.1, §6.3).
- **Timing.** Testnet runs the same bytes as finney about a day earlier. `META/finney.scale` and
  `META/test.scale` are byte-identical today (R:H §1.1).
- **CORR #2.** "spec bumped, surface unchanged" is the common case. Generated files omit the spec,
  so such a bump produces only an `index.toml` row and an "additive: none" drift PR that can be
  auto-merged.

### 12.4 Maintenance playbook

| Event | What fires | Human work | Typical effort [inferred, after R:H §4] |
|---|---|---|---|
| **(a) New runtime** (devnet → testnet → mainnet) | `metadata-drift` PRs per network; then `core-pin-bump` when finney finalizes | Read the classified diff. *Additive*: merge; optionally expose new calls or events. *Behavioral* (event index moves, storage types, API versions): check typed models and per-spec tables, run e2e. *Breaking* (extension change, moved indices, changed args of a used call, `transaction_version`): fix the seam in `rtao-core`/`rtao-transport`; major release | additive ≈ 1 h; behavioral ≈ ½ day; breaking 1–3 days |
| **(b) The Python SDK adds or changes an intent or read** | `pr`/`upstream-watch` parity failures listing the op and the changed blobs; catalog diff; coverage gate | Port the struct and impl or the read body; copy the docstrings; refresh oracle vectors (bump PR); `parity bless` | ½–1 day per intent; 1–2 h per read |
| **(c) Python changes transport or executor semantics** (e.g. retry substrings, era defaults) | file and symbol layers on `_transport/*.py` / `executor.py`; changed `tests/unit/test_rpc.py` | Port the diff; port the new test cases | proportional to the diff |
| **(d) bittensor-core changes** (API or bytes) | bump PR shows the `sdk/bittensor-core` diff; golden/corpus tests | Adapt `rtao-core` only. If U1/U2 land, flip the regression tests | usually < ½ day |
| **(e) btauth/1 → btauth/2** | watched spec, implementation and tests change; vectors fail | Implement v2 alongside v1 (verify both, sign the configured one); keep v1 for upstream's deprecation window (`test_http_auth.py:3-6` policy) | ½–1 day |
| **(f) Catalog schema change** (it happened once: `chain_errors` values became objects, R:I §5.3) | catalog parser error | Update the parser; it tolerates added fields | hours |
| **(g) Metadata version or extrinsic format shift** (V17, v5 default) | drift guards | rtao requests V15 explicitly and signs v4; plan a migration | days |
| **(h) Dependency majors** (tokio-tungstenite, axum, reqwest, clap) | Renovate weekly | Bump, contained in single modules | hours |
| **(i) New upstream test vectors** | file layer on `tests/**` | Port into the mirrored Rust test | hours |

**Cadence.** 9 finalized runtimes in 20 days (v466–v477, R:H §0.3) means the `pr`/drift
automation must carry most of the load. Budget a standing maintenance share of about
0.25–0.5 engineer per week once parity is reached [inferred].

### 12.5 Compatibility matrix and releases

`COMPAT.md` and `compat.json` hold one row per rtao release:

| rtao | upstream tag (commit) | Python SDK mirrored | spec range validated | metadata pins | localnet images (digests) | btauth | interop oracle |
|---|---|---|---|---|---|---|---|
| `rtao-v0.4.0` | `v477` (`5c6e83e2`) | `11.3.1.dev0` | 477–478 | finney 477, testnet 477, devnet 478 | `v477@sha256:…`, `testnet@…`, `devnet@…` | btauth/1 | `bittensor` @ `v477` |

- **"Validated"** means the drift diff across the range has no *breaking* class (§12.2), and the
  e2e and interop lanes passed on the listed digests (R:H §3.9).
- **Runtime guard.** On connect, rtao compares the chain's spec with the validated range. Outside
  it, reads still work (descriptors are validated dynamically, §3.7). Signing still works too,
  because payloads are metadata-driven, but rtao logs a warning; the warning can be configured to
  refuse for calls whose arguments changed.
- **Upstream has no SDK↔spec compatibility table** (R:H §2.5), so rtao's matrix is the only one
  its users get.

---

## 13. Documentation

- **rustdoc.**
  - `#![deny(missing_docs)]` on `rtao`, `rtao-http` and `rtao-neuron`. Mirrored items carry the
    verbatim Python docstrings (R6) and `#[doc(alias = "bittensor.…")]` (R3), so searching
    docs.rs-style output for `SetWeights` or `bittensor.reads.neurons` lands on the Rust item.
  - Crate roots use `#![doc = include_str!("../README.md")]`.
  - docs.rs metadata (`all-features`, `--cfg docsrs`) is kept even though rtao is not published,
    so `cargo doc` output matches.
- **docify.** Every example is a real item in `crates/*/examples/` or `tests/`, marked
  `#[docify::export]`.
  - Offline-runnable examples (`FakeSubstrate`-backed: compose, plan, btauth sign/verify, Balance
    math) are embedded with `docify::embed_run!`, so they run as doctests.
  - Node-dependent examples use `docify::embed!`.
  - The README is compiled from `README.docify.md` with `docify::compile_markdown!`.
- **mdBook (`docs/`).** Mirrors the bittensor.com taxonomy (`UP/docs/meta.json`): `index`,
  `quickstart`, `sdk` (the five primitives in Rust: `Client`, `Wallet`/`Signer`, `Balance`,
  intents, `ExtrinsicResult`), `agents`, `cli`, and:
  - `concepts/`: links to upstream raw Markdown at
    `https://www.bittensor.com/llms.mdx/docs/concepts/<slug>/content.md`, not copies;
  - `guides/`: Rust-specific, covering the validator loop, a miner with btauth, owner tooling,
    local development, upgrading pins;
  - `migration`: three tables, (1) Python v11 → rtao (generated from `parity/map.toml` plus the
    alias index, so it is always complete), (2) v10 Python → rtao (R:I §1), (3) the
    bittensor-core native SDK → rtao;
  - generated reference pages `tx/`, `query/`, `errors/`, `hyperparameters/`.
- **Generated reference.** `xtask docs` is the Rust analogue of
  `UP/website/apps/bittensor-website/scripts/generate.py`. It renders one page per intent and per
  read with the same sections (R:I §5.2), with `## Rust` replacing `## Python`. "On-chain
  implementation" links reuse upstream `sources`/`hits` URLs (`https://www.bittensor.com/code/…`)
  rather than hosting chain source.
- **Catalogs.** `docs/catalog/{intents,reads,errors}.json` use the upstream schema verbatim plus a
  `rust` object per entry:
  `{"crate": "rtao", "path": "rtao::intents::weights::SetWeights", "cli": "rtao tx set-weights …"}` for
  intents and `{"method": "client.neurons().neurons(netuid, lite)", "returns": "Vec<Neuron>"}` for
  reads (R:I §5.6b). `xtask docs --check` is a CI drift gate.
- **`llms.txt` / `llms-full.txt`.**
  - Same preamble conventions as bittensor.com's (`scratchpad/docs/bittensor.com_llms.txt:3-27`):
    raw-Markdown links, "grep the full corpus", catalog URLs.
  - A **mapping block**: `docs/tx/<op>` ↔ `https://www.bittensor.com/llms.mdx/docs/tx/<op>/content.md`
    and `query/<name>` likewise, so an agent can read the upstream semantics and the Rust
    signature side by side.
  - Links to the `rtao tools` and `rtao reads` JSON.
  - Both files are generated by `xtask docs --llms` and drift-gated.

---

## 14. Milestones

Effort is in engineer-weeks [inferred]. Sizing basis (§0 facts):
- the Python library is about 34.4k lines excluding `cli/` and `_generated/`, or about 30.5k
  hand-port lines after removing the transpiled data (`error_descriptions/`,
  `basket_index_data.py`) and the `.pyi` stub;
- the CLI is 23k lines;
- the tests are 9.5k lines.

Milestones overlap after M1: the CLI, intents and framework tracks can run in parallel.

| # | Deliverables | Exit criteria | Effort |
|---|---|---|---|
| **M0 — first typed read against finney** | Workspace and crates skeleton; `pr` CI; the bittensor-core `v477` pin + patch; `rtao-core::RuntimeCodec` (+ `metadata_ir` port, typed decode); `rtao-transport` basics (connect, request, subscribe, V15 fetch, disk cache); `Client::new/query/runtime`; `ext::types::MetagraphInfo`; `xtask sync-upstream`; parity file layer with every upstream file mapped (most `planned`); golden calls, storage keys, payloads and extrinsics replayed through the wrappers; the w3f-bls-patch experiment (§2.6) | `cargo run --example metagraph -- finney 1` prints a typed metagraph decoded from `get_metagraph`, together with `Tempo(1)` from storage. Golden vectors are byte-equal. Corpus replay passes | 2–3 |
| **M1 — read-side parity** | Full rpc.rs semantics + `FakeNode`; RuntimeManager (block→spec, Python-compatible disk cache, finalized-only hash cache); `rtao-generated` + `xtask codegen` and its gates (`--check`, `--names` minus the tables still to come); `Substrate`, `RpcSubstrate`, `Snapshot`, `View`; `#[read]` macro; **all 102 reads** + 14 namespaces; `Metagraph`/commitments; `FakeSubstrate`; read tables; the oracle (read vectors) | Catalog layer green for reads (102/102). Oracle read vectors equal. Localnet read parity equal except D-001/D-002 | 4–6 |
| **M2 — write path + validator/miner MVP** | `Signer`, `KeypairSigner`, `WalletSigner`; wallets, keyfiles and password resolution; extrinsics pipeline (nonce, era, fee, sign, watch, receipt); results and the error taxonomy (transpiled tables + names gate); executor (plan, execute, policy, retries, sudo/proxy); `#[intent]`; intents `transfer`, `transfer_all`, `serve_axon` (+tls/prometheus/reset), `set_weights` (+timelock), `commit_weights`, `reveal_weights`, `burned_register`, `root_register`, `register_subnet` (+completion), `start_call`, `set_hyperparameter`, `add_stake`, `remove_stake`, `batch`; `http_auth` + `rtao-http`; CLI `tx`/`query`/`tools`/`explain`/`call`/`config` | Localnet e2e: create a subnet, start the call, register a validator and a miner, serve an axon, set weights directly and timelocked (reveal observed on 12 s blocks), btauth both directions against Python. `tools` equal for the implemented ops | 6–8 |
| **M3 — framework, harness, interop** | `rtao-neuron` (validator loop, miner server, metagraph sync, weight setter, axon publisher, registrar, owner ops, config); the full localnet harness and matrix; Python interop lanes 1–3 and 6; CLI `wallet` core, `subnets`, `weights`, `axon`; `rtao-subnet-template` | A sample subnet (Rust validator + Rust miner) runs on localnet, and also against a Python v11 miner and validator. The template builds from `cargo generate` | 4–6 |
| **M4 — remaining intents, advanced transport** | The remaining ~65 intents: staking family with slippage, limits and `"all"`; children/take; coldkey swap; collateral; locks; crowdloan; leasing; governance/root (sudo + verify); identity; proxy (pure, announced); multisig + adapters + funds preflight; EVM intents; association; basket. Also: root-claim fee model; full MEV-shield pipeline; `prepare_call`/`submit_signature`; the complete hyperparameter table; the timelock module; PoW CPU/GPU; the `evm` feature | Intent catalog 83/83. Plan-parity vectors equal for every sample. The e2e behavior inventory (§11.4b) passes | 8–10 |
| **M5 — CLI parity and external signers** | Every hand-written CLI group (§10.2) with JSON-output parity; shared btcli state; Ledger, Vault and extension signers; completions | `cli_tree` parity. Interop lane 7. Ledger proof vector, plus one manual device run | 8–12 |
| **M6 — full parity and hardening** | `parity/allow.toml` empty; the divergence register final, with upstream PRs filed; docs, mdBook, `llms.txt`; compat matrix and release automation; fuzz and bench baselines; optional `rtao-compat-synapse`, `rtao-compat-epistula`, `rtao-quic` | `xtask parity check` green with zero allowances at the then-current tag, **and** one complete upstream bump executed through §12.4 | 3–4 |

**Total: about 35–49 engineer-weeks to full parity.** The subnet-building path (M0–M3) is about
16–23 weeks. After that, M4 and M5 can run in parallel tracks.

---

## 15. Risks and open questions

### 15.1 Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **Parity long tail**: the CLI (23k lines with interactive prompts, pickers, Rich output) and 83 intents with intricate preflights (`_root_claim_fee.py` alone is 771 lines). | Generated `tx`/`query` early. JSON-output parity before human formatting. The parity tool makes the remaining gap visible as a number, the rows of `allow.toml`. |
| 2 | **Upstream churn** (about 2–4 runtimes per week in late Sept–Oct 2026; R:H §4). SDK changes ride on the same tags. | Automated drift and bump PRs; a mechanical diff per file; budgeted maintenance (§12.4). |
| 3 | **bittensor-core is an unversioned internal crate.** Its API changes when Python needs it to (R:B §9.2). | A single seam (`rtao-core`); the bump PR shows the core diff; upstream proposals U1–U10. |
| 4 | **The `[patch.crates-io] w3f-bls` requirement leaks** into every downstream workspace (Cargo ignores dependency patches). | Template repo; a loud build-time doc note; the M0 experiment to drop it. |
| 5 | **Heavy build.** About 330 crates with `host`, with C and system dependencies: libsodium, vendored OpenSSL (needs Perl), `secp256k1-sys`, hidapi (R:B §6). The first fetch clones the monorepo (236 MB) and the polkadot-sdk fork. | CI caches `~/.cargo/git`; a documented toolchain (Perl, a C compiler); U5 to make the C-backed pieces optional. |
| 6 | **Reimplementing rpc.py's semantics** risks subtle reconnect or nonce bugs. | `FakeNode` fault injection, a port of `test_rpc.py`, a node-restart e2e, and soak runs against finney in `live-probe`. |
| 7 | **Typed-model drift**: a runtime renames a field of an rpc_info struct. | `DecodeAsType` by name; `freeze_struct` tripwires; drift-job decode of recorded payloads; the differential test vs `Value`. |
| 8 | **Divergence creep**: "fixing" Python behavior (D-001…D-005) dilutes parity. | Every divergence needs a register entry, a pinning test and an upstream issue or PR. Default to bug-compatibility for chain-facing bytes. |
| 9 | **Timelock correctness inherits the core's epoch-predicate divergence** (U2). Both SDKs share it, and the +3-block offset absorbs small deferrals (R:C1 §2.3). | The U2 PR upstream; a `False`-block reveal e2e; a framework guard that checks `CommitRevealWeightsVersion`. |
| 10 | **Python-oracle CI cost**: building the bittensor-core wheel with maturin, OpenSSL and Perl. | Cache the oracle image per tag; use PyPI wheels when versions map exactly. |
| 11 | **Tooling dependencies**: `ruff_python_parser` is git-only. | Pin a rev and restrict use to `xtask`. Fallbacks: `rustpython-parser` (crates.io) or `tree-sitter-python`. |
| 12 | **Licensing**: rtao is MIT; transpiled tables, copied docstrings, fixtures and static HTML are Apache-2.0 upstream material. | Keep Apache-2.0 headers and a `NOTICE` in `py_tables/`, `upstream/` and copied assets [proposal; licensing advice may be needed]. |
| 13 | **Edition-2024 `set_var` unsafety** in the core's env-password helper (`CORE/keyfiles/mod.rs:126-138`). | Expose it only on the single-threaded CLI path before the runtime starts. |
| 14 | **Public endpoint variability**: rate limits, `-32601` for the newer RPC families, lite nodes keeping about 300 blocks. | Legacy namespace only; conservative page sizes; archive fallback; endpoint pools (§3.3). |

### 15.2 Open questions

1. **Bug compatibility.** Should D-001 (zero-account `Owner` default treated as none;
   [inferred] behavior, to be verified on a localnet first) and D-002 (pinning gather reads) be
   divergences or exact copies of Python? Recommendation: divergences plus upstream PRs, so the
   Python SDK converges.
2. **Python in rtao's CI** (the oracle and interop lanes). The 100%-Rust requirement applies to
   what subnet owners build, not to rtao's test infrastructure. Please confirm.
3. **CLI binary name.** Keep `rtao`, or allow an opt-in `btcli` symlink for drop-in use? With
   shared config (§10.3) that could surprise users who have both installed.
4. **Upstream appetite.** Will RaoFoundation accept U1–U10, in particular moving `metadata_ir`
   into the core (U4), splitting `host` (U5) and publishing metadata release assets (U10)?
5. **Tag reliability.** Does every finalized runtime get both a final release tag and a
   `:v<spec>` localnet image? That held for 473, 475 and 477 (R:H §8 Q1).
6. **External signers.** Are Vault and the extension bridge wanted in Rust for "full parity", or
   is Ledger plus prepare/submit-signature enough? The plan includes them in M5 behind features.
7. **subxt interop.** A later `subxt` bridge (an `RpcClientT` over `RpcSession`) would let users
   build an `OnlineClient` on rtao's connection. Not planned; demand-driven.
8. **Weight-commit version source.** Should `SetWeights` read `CommitRevealWeightsVersion` from
   storage instead of Python's constant 4? Recommendation: keep the constant for parity, add the
   framework-level guard (§9.4), and propose the change upstream.

---

## Appendix A — CI workflow sketches

```yaml
# .github/workflows/pr.yml
name: pr
on: [pull_request, push]
jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@<sha>      # also caches ~/.cargo/git (monorepo + polkadot-sdk fork)
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - run: cargo nextest run --workspace --all-features
      - run: cargo test --workspace --doc --all-features
      - run: cargo xtask codegen --check            # offline regen from upstream/metadata pins
      - run: cargo xtask transpile --check          # py_tables/** == Python literals at the pinned tag
      - run: cargo xtask codegen check --coverage --names --units
      - run: cargo xtask parity check               # file + symbol + catalog + chain-source layers
      - run: cargo deny check
      - run: cargo install cargo-audit --version 0.22.2 --locked && cargo audit
  msrv:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@1.89
      - run: cargo check --workspace --all-features --locked
  features:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install cargo-hack --locked && cargo hack --feature-powerset --depth 2 check -p rtao -p rtao-http -p rtao-neuron
```

```yaml
# .github/workflows/e2e.yml
name: e2e-localnet
on:
  pull_request: { paths: ['crates/rtao-transport/**', 'crates/rtao/src/executor.rs', 'crates/rtao/src/intents/**', 'crates/rtao/src/reads/**', 'upstream/**'] }
  schedule: [{ cron: '0 3 * * *' }]
jobs:
  localnet:
    runs-on: ubuntu-24.04
    strategy:
      fail-fast: false
      matrix:
        lane: [pinned, mainnet, testnet, devnet]
        blocks: ['True']
        include: [{ lane: pinned, blocks: 'False' }]   # 12 s blocks for timing tests
    continue-on-error: ${{ matrix.lane == 'devnet' }}
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@stable
      - run: echo "LOCALNET_IMAGE=$(cargo xtask image --lane ${{ matrix.lane }})" >> $GITHUB_ENV   # digest from upstream/images.toml
      - run: docker pull "$LOCALNET_IMAGE"
      - run: LOCALNET_BLOCKS=${{ matrix.blocks }} cargo nextest run -p rtao -p rtao-neuron --features e2e --test-threads 4
```

```yaml
# .github/workflows/drift.yml
name: metadata-drift
on: { schedule: [{ cron: '17 */6 * * *' }], workflow_dispatch: {} }
jobs:
  drift:
    runs-on: ubuntu-24.04
    strategy: { matrix: { network: [finney, testnet, devnet] } }
    steps:
      - uses: actions/checkout@<sha>
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo install subxt-cli --version 0.51.1 --locked
      - run: cargo xtask drift --network ${{ matrix.network }} --out target/drift   # exit 0 = unchanged
      - if: hashFiles('target/drift/changed') != ''
        run: |
          cargo xtask codegen --metadata "$(cat target/drift/v15_path)"
          gh pr create --title "drift: ${{ matrix.network }} $(cat target/drift/old)→$(cat target/drift/new)" \
            --body-file target/drift/report.md --label "$(cat target/drift/class)" || gh pr edit …
```

```yaml
# .github/workflows/interop.yml  (Python only inside containers)
name: interop
on: { schedule: [{ cron: '0 4 * * *' }], pull_request: { paths: ['parity/**', 'upstream/**', 'Cargo.toml'] } }
jobs:
  interop:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@<sha>
      - run: cargo xtask oracle build          # docker build -f parity/oracle/Dockerfile --build-arg TAG=$(cargo xtask upstream tag)
      - run: docker compose -f parity/oracle/compose.yml up -d localnet
      - run: cargo xtask oracle run --scenarios btauth,wallets,reads,plans,state,cli
      - run: cargo nextest run -p rtao-testkit --features interop
```

## Appendix B — parity manifest schemas

```toml
# parity/upstream.toml
repo = "https://github.com/RaoFoundation/subtensor.git"
tag = "v477"
commit = "5c6e83e2f4936f257234f246957f75684c35227c"
python_package_version = "11.3.1.dev0"        # sdk/python/pyproject.toml at the tag
watched_roots = ["sdk/python/bittensor", "sdk/python/codegen", "sdk/python/tests", "sdk/bittensor-core",
                 "common/src/transaction_error.rs", "runtime/src/fee_filters.rs",
                 "pallets/subtensor/src/rpc_info/metagraph.rs", "docs/guides/signed-requests.mdx",
                 "website/apps/bittensor-website/public/catalog"]

# parity/map.toml — one row per watched upstream file
[[file]]
py = "sdk/python/bittensor/intents/serving.py"
rs = ["crates/rtao/src/intents/serving.rs"]
kind = "hand"                                  # hand | transpiled | mixed | generated | synced | n/a | planned
synced_blob = "<git blob sha1 of the file when last reconciled>"
synced_tag = "v477"
notes = ""

[[file]]
py = "sdk/python/bittensor/namespaces.pyi"
rs = []
kind = "n/a"
notes = "Rust namespace methods are emitted at compile time by #[read]; no stub needed."

# parity/divergences.toml
[[divergence]]
id = "D-002"
upstream = "bittensor.reads.delegation.delegate_take (and 3 other gather reads)"
python = "issues 2-3 head reads that may straddle blocks"
rtao = "pins the view first; all calls share one block hash"
rationale = "consistent snapshot; no observable cost"
test = "crates/rtao/tests/test_reads_table.rs::every_read_uses_one_block"
upstream_pr = ""                                # to be filed

# parity/allow.toml — temporary gaps, must be empty at M6
[[gap]]
upstream = "intent:swap_basket_many"
milestone = "M4"

# upstream/images.toml — localnet image digests per lane
[lanes]
pinned  = "ghcr.io/raofoundation/subtensor-localnet:v477@sha256:<digest>"
mainnet = "ghcr.io/raofoundation/subtensor-localnet:mainnet@sha256:<digest>"
testnet = "ghcr.io/raofoundation/subtensor-localnet:testnet@sha256:<digest>"
devnet  = "ghcr.io/raofoundation/subtensor-localnet:devnet@sha256:<digest>"
```
