# rtao, architecture C: an operator-first veneer over `bittensor-core`

Date: 2026-10-13. Lens: *subnet-operator first*: start from what a validator, a miner and a subnet-owner
binary do minute by minute, design the SDK surface backwards from those loops, put the test harness and
operational safety at the centre, and still reach full parity with the v11 Python package.

Target repo: the rtao repository (today an empty `rtao` crate: edition 2024, MIT, author sam0x17;
this design does not modify it).

User decisions treated as fixed: (1) crates.io publishability is not a goal, git dependencies and a replicated
`[patch.crates-io] w3f-bls` are fine; (2) rtao is a veneer over `sdk/bittensor-core` and reuses the official Rust core for everything chain-defined; (3) the scope is parity with the whole v11 Python layer, phased but complete.

## How to read this document

Path shorthands (all absolute roots):

| Shorthand | Path |
|---|---|
| `SUB:` | `SP/upstream/subtensor/` (RaoFoundation monorepo @ `850f2755`, spec 478 on `main`; finney/testnet run 477) |
| `CORE:` | `SUB:sdk/bittensor-core/src/` |
| `PY:` | `SUB:sdk/python/bittensor/` (v11 Python layer) |
| `RES:` | `SP/research/` (reports A1, A2, B, C1, C2, D, E, F, G, H, I, CORRECTIONS.md, feasibility-*.md) |
| `META:` | `SP/meta/` (finney/test/devnet metadata, `explore/` dumps) |
| `CAT:` | `SP/docs/` (bittensor.com `intents.json`, `reads.json`, `errors.json`, `index.json`, `llms*.txt`) |
| `UP:` | `SP/upstream/` (ecosystem clones; `UP:inference-labs-inc_subnet-2/crates/` is abbreviated `SN2:`) |
| `FEAS:` | `SP/feasibility/` |
| `REG:` | `REG/` (unpacked crates.io sources) |

Evidence markers: **[V]** read in source while writing this design; **[R]** stated (with a citation) by a research report, whose section is given; **[I]** inferred: an argument or an estimate rather than an observation. `RES:CORRECTIONS.md` overrides the reports: in particular the spec-478 fee-extension identifier did
**not** change on the wire and spec 478 changes no client-visible metadata (`RES:CORRECTIONS.md` items 1-2), so
C1 §0 finding 2 is not relied on anywhere below.

---

## 0. Executive summary

The design in fifteen decisions (each is argued in the section named in brackets):

1. **Split by who defines the bytes** (§1). Every byte the chain or an external ecosystem defines (keys and ss58, btwallet keyfiles, SCALE encode/decode over the runtime's own metadata, signature payload and
   v4 extrinsic assembly, era math, multisig ids, RFC-0078 digest, ML-KEM shield blob, drand tlock, PoW seal, Ledger APDU) comes from `bittensor-core` unchanged. rtao owns everything the core's own classification
   calls PRODUCT (`RES:B-bittensor-core-rust.md` §3.14): transport, typing, policy, the v11 product layer, the
   neuron framework, the CLI.
2. **One anti-corruption crate** (§1.8, §2). `rtao-btcore` is the only crate that names `bittensor_core`; it is
   pinned to the monorepo tag `v<spec_version>` of the runtime finney runs (today `v477`) with the matching
   `w3f-bls` patch. A pin bump touches one crate plus fixtures.
3. **The core's blocking `Client` and `Executor` are not on any runtime path** (§1.4, §1.7). They are
   `reqwest::blocking` over HTTP with polling receipts, no subscriptions and no nonce cache
   (`CORE:client.rs:30-33,150-179` [V], `RES:B` §4.1), and `Executor` is hard-wired to that client
   (`CORE:transaction.rs:998-1116` [V]). rtao keeps them as *differential oracles* in tests, and a
   `clippy.toml` `disallowed-methods` list makes accidental use a CI failure.
4. **Async transport = `subxt-rpcs` + jsonrpsee 0.26 behind rtao's own session supervisor** (§3.2). rtao
   implements `subxt_rpcs::client::RpcClientT` (`REG:subxt-rpcs-0.51.1/src/client/rpc_client_t.rs` [V]) with the
   v11 Python session semantics (pool rotation, terminal policy refusals, idempotent resend, never resubmitting
   `author_submit*`, archive fallback) and talks the legacy RPC namespace only; the default subxt
   `CombinedBackend` fails on the finney WS endpoint (`RES:feasibility-subxt.md` table).
5. **Typed where operators live, dynamic everywhere else** (§3.5, §8). The core's `Runtime` (V15) is the per-spec
   metadata object; hot structures (metagraph, neuron info, hyperparameters, stake info, events used by
   receipts) decode with `scale-decode`'s `DecodeAsType` directly against `Runtime.types`, the same `scale-info 2.11.6` registry type, verified to unify (`RES:feasibility-veneer-coexistence.md`). The core's
   cyscale-shaped `Value` remains the encode input for `compose_call` and the dynamic/CLI/agent output, which
   keeps rtao's JSON close to Python's.
6. **Name-based generated descriptors validated against the live runtime** (§3.6). `cargo xtask codegen`
   emits typed call-argument structs, storage/runtime-API/constant/event descriptors and error tables from
   pinned V16 metadata (committed, reviewable); every connect and every runtime upgrade re-verifies them and
   disables, fail-closed, any intent whose wrapped call changed shape.
7. **Operator loops are first-class library objects** (§0.1, §9). `MetagraphSync`, `EpochClock`,
   `WeightScheduler` (CRv4 timelocked commits, mechanisms, Null raw-u16, version-key preflight, bounded watch +
   reconciliation), `AxonPublisher` (idempotent `serve_axon`), `BtAuthLayer` (btauth/1 + metagraph policy),
   `StateStore`, metrics and graceful shutdown, composed by `rtao-neuron` into `Validator`/`Miner` runtimes.
8. **Safety rails are on by default** (§9.12): neuron runtimes never load a coldkey, a neuron's
   `Policy` allows only its own netuid and zero spend, every mutation has a dry-run plan, preflights check rate
   limits and admin windows from chain state at one pinned block, clock skew and genesis are checked at boot.
9. **The harness comes first** (§11, §14 M0). Upstream goldens (`golden.json`, shape corpus, Ledger proof,
   btauth vectors, epoch-schedule vectors) are vendored and replayed; an in-process `FakeChain` serves pinned
   metadata and scripted extrinsic lifecycles; a docker localnet harness reuses upstream's env contract
   (`E2E_ENDPOINT`, `LOCALNET_IMAGE`, `SKIP_PULL`); a Python-interop compose lane is a CI oracle only (rtao
   itself stays 100% Rust).
10. **Parity is enforced by a failing test** (§4.7). Vendored bittensor.com catalogs (83 intents, 102
    reads, 18 codes, 390 chain errors; counted [V] in `CAT:`) are compared with rtao's registries and a
    `parity/manifest.toml`; any upstream name with no implementation and no manifest decision fails CI.
11. **btauth/1 is native and byte-exact; legacy protocols are optional crates** (§7). Python is the
    normative reference (`PY:http_auth.py`, `SUB:docs/guides/signed-requests.mdx:252-289`); v10 Synapse and
    Epistula live in `rtao-compat-*`; btlightning QUIC is a feature-gated adapter.
12. **Money is unit-tagged integers** (§5). `Rao`, `AlphaRao`, `Balance{rao, unit}`, `NetUid`, `MechId`,
    `NetUidStorageIndex`, `PerU16`, `U64F64`/`I96F32`/`I64F64`/`I32F32`; no floats on any wire path.
13. **The CLI is generated from the same registries** (§10). `rtao tx <op>` and `rtao query <read>` are derived
    from intent/read definitions exactly like v11's `btcli tx`/`btcli query` (`PY:cli/tx.py:1-11` [V]).
14. **Upstream tracking is automated** (§12): metadata drift (finney/testnet/devnet), catalog drift, core-pin
    bumps, watched-file hashes, live probes, a published compat matrix. Behaviour-only runtime bumps (the common case, `RES:CORRECTIONS.md` item 2) cost about an hour of review (§12.5).
15. **Sequencing by operator value** (§14): M0 harness + first typed read, M1 validator, M2 miner, M3 owner +
    operational safety + CLI v0, then reads, intents, signers, CLI and legacy networking to full parity
    (~42 engineer-weeks [I]).

### 0.1 The three operator loops (the design is derived from them)

#### Validator (Yuma subnet with commit-reveal on, the chain default per `RES:C2` §0 item 7)

| When | What the binary does | Chain surface | rtao API (crate) | Evidence |
|---|---|---|---|---|
| boot | load config (flag > env > file > default); load **hotkey only** (plaintext keyfile) | - | `NeuronConfig`, `Wallet::hotkey()` (`rtao-neuron`, `rtao-wallet`) | `RES:G` §4.2, §2.9 item 26 |
| boot | connect to endpoint pool, assert genesis, fetch V15 metadata for the finalized spec, verify generated surface, check clock skew | `chain_getBlockHash(0)`, `state_getRuntimeVersion`, `state_call Metadata_metadata_at_version(15)`, `Timestamp.Now` | `Subtensor::connect` (`rtao-client`) | `RES:H` §1.1 (genesis table), `RES:A1` §6.2 |
| boot | registration, permit, stake threshold, version-key, CR version checks at one block | `Uids`, `ValidatorPermit`, `StakeThreshold`, `WeightsVersionKey`, `CommitRevealWeightsVersion` | `NeuronCx::registration()`, `WeightsPreflight` | `RES:C1` §2.1 steps 6-10, §2.3 |
| boot | restore scores, keyed by hotkey | - | `StateStore` | `RES:G` §2.4 item 13, §2.5 |
| every finalized block | head + epoch clock tick | `chain_subscribeFinalizedHeads` | `HeadTracker`, `EpochClock` | `RES:C2` §4.2 |
| every epoch (or N blocks) | metagraph sync pinned to one block, diff, reset scores of replaced hotkeys | `SubnetInfoRuntimeApi.get_selective_metagraph` | `MetagraphSync` (`rtao-metagraph`) | `RES:C2` §5.2; `RES:G` §2.4 |
| continuously | query miners over btauth/1 with bounded fan-out, score, EMA | HTTP | `BtAuthClient::fan_out`, user `Validator::step` | `RES:D` §1.4, §1.9 |
| once per epoch per mechanism | conform weights -> CRv4 timelocked commit (direct set when CR is off) -> bounded watch -> reconcile | `commit_timelocked_mechanism_weights` (118), `set_mechanism_weights` (119) | `WeightScheduler` (`rtao-neuron`) | `RES:C1` §2.1-2.4; `RES:G` §2.6 |
| reveal epoch | confirm the chain revealed our commit | event `TimelockedWeightsRevealed(idx, hotkey)` | `WeightScheduler` reveal watcher | `RES:C1` §2.3; `RES:C2` §3 |
| every 300 s and at exit | persist state atomically | - | `StateStore` | `RES:G` §2.5 item 15 (SN2) |
| runtime upgrade | refetch metadata, re-verify surface, stop signing calls whose shape changed | `state_subscribeRuntimeVersion` | `RuntimeWatcher` (`rtao-client`) | `RES:H` §2.2 |
| SIGTERM | stop scoring, drain the in-flight commit (deadline), persist, exit | - | `NeuronRuntime` | `RES:G` §7 MUST 10 |

#### Miner

| When | What the binary does | Chain surface | rtao API | Evidence |
|---|---|---|---|---|
| boot | hotkey, connect, check registration and immunity window | `Uids`, `BlockAtRegistration`, `ImmunityPeriod` | `NeuronCx::registration()` | `RES:G` §3.2 |
| boot | detect public IP (override flag), publish the axon **only if the chain record differs** and the serving rate limit allows | storage `Axons[netuid, hotkey]`, `serve_axon` (4, `Pays::No`) | `AxonPublisher` | `RES:G` §3.3; `RES:C1` §3 |
| per request | btauth/1 verify (shape -> receiver -> freshness -> signature -> replay), then metagraph policy (registered + permit + stake floor) -> 401/403 | - | `BtAuthLayer`, `MetagraphPolicy` (`rtao-http`) | `RES:D` §1.5-1.9 |
| per request | per-hotkey rate limit and priority by stake | - | `PerHotkeyLimiter` | `RES:G` §3.4 item 7 |
| every 600 s (min 30 s) or on events | refresh metagraph used by the policy (lock-free swap) | `get_selective_metagraph` miner preset | `MetagraphSync` | `RES:G` §3.4 item 7 (SN2 600 s) |
| every 300 s | re-check public IP / axon record; re-serve idempotently | `Axons` | `AxonPublisher` | `RES:G` §3.3 item 5 |
| continuously | prune-risk and immunity metrics | metagraph emission ranks | `PruneRisk` | `RES:G` §3.2; `RES:C1` §4.1 |
| SIGTERM | stop accepting, drain HTTP, optional `reset_axon` (opt-in) | `serve_axon` ip 0 | `NeuronRuntime` | `RES:G` §3.3 item 5 |

#### Subnet owner (mostly operator tooling, run interactively or from cron)

| Step | Chain surface | rtao API | Evidence |
|---|---|---|---|
| budget and create: cost, rate limit, queued vs immediate completion | `get_network_registration_cost`, `register_network`, events `NetworkAdded`/`NetworkRegistrationQueued` | `RegisterSubnet` intent with completion wait | `RES:G` §1.1-1.2; `RES:A1` §4.13 |
| secure the owner coldkey: multisig + `Owner` proxy for ops | `Multisig.as_multi`, `Proxy.add_proxy` | proxy/multisig adapters (`rtao-tx`) | `RES:G` §1.3 |
| activate | `subnet_start_schedule`, `start_call` (92) | `StartCall` + schedule check | `RES:G` §1.4 |
| tune hyperparameters inside the admin window and per-parameter rate limit | `AdminUtils.sudo_set_*`, `LastRateLimitedBlock`, `AdminFreezeWindow`, `get_next_epoch_start_block` | `HyperparamPlanner` + `SetHyperparameter` | `RES:G` §1.5 items 14-18; `RES:C1` §5.2 |
| identity, symbol, mechanisms, trims, Null trim batches | `set_subnet_identity`, `update_symbol`, `sudo_set_mechanism_count`, `sudo_trim_*` | owner intents | `RES:G` §1.7-1.9; `RES:A1` §3.8 |
| monitor: both "on" flags, owner changes, hyperparameter events, conviction | reads + event stream | `OwnerMonitor` | `RES:G` §1.4 item 13, §1.11 |

Everything in these tables is delivered by milestone M3 (§14); the rest of the parity scope (staking,
baskets, crowdloans, EVM, Vault, …) follows in M4-M8.

---

## 1. Boundary with `bittensor-core`

### 1.1 Principle

`bittensor-core` classifies its own modules as CHAIN (bytes the chain defines), COMPAT (bytes an external
ecosystem defines: btwallet, polkadot-js, Ledger) or PRODUCT (`RES:B` §3.14 table). rtao uses every CHAIN
and COMPAT module as-is; it re-implements no byte format. PRODUCT modules (`client.rs`, `transaction.rs`) are
replaced because they are blocking and incomplete, and they are kept as test oracles.

### 1.2 Used as-is (through `rtao-btcore` re-exports)

| bittensor-core item | Location [V] | rtao consumer |
|---|---|---|
| `keys::{CRYPTO_ED25519=0, CRYPTO_SR25519=1, DEFAULT_SS58_FORMAT=42}` | `CORE:keys/mod.rs:28-31` | `rtao-primitives::AccountId`, `rtao-signer` |
| `keys::{public_key_from_ss58, ss58_from_public}` | `CORE:keys/mod.rs:48,57` | `AccountId` ss58 codec |
| `keys::Keypair` (`from_mnemonic`, `from_seed`, `from_uri`, `from_private_key`, `generate_mnemonic`, `from_encrypted_json`, `sign`, `verify`) | `CORE:keys/mod.rs:151-162,166-410` | `KeypairSigner`, wallet creation/regen, btauth signing |
| `keys::verify(message, sig, ss58, crypto_type)` (raw, then `<Bytes>` wrapped) | `CORE:keys/mod.rs:457-469` | btauth/1 verification (parity default, §7.1) |
| `keyfiles::{keyfile_data_is_encrypted*, keyfile_data_encryption_method, encrypt_keyfile_data, decrypt_keyfile_data, serialized_keypair_to_keyfile_data, deserialize_keypair_from_keyfile_data, get_password_from_environment}` | `CORE:keyfiles/mod.rs:29-47,86,111,153,179,287` | `rtao-wallet` |
| `runtime::Runtime::parse` + lookups (`pallet`, `storage_entry`, `constant`, `resolve`, `type_ident`, `module_error`, `apis`, `types`) | `CORE:runtime/mod.rs:137-157,163,230-366` | `rtao-client::RuntimeHandle` |
| `codec`: `Runtime::compose_call`, `encode_id`, `coerce_account_id` | `CORE:codec/encode.rs:532,176,491` | typed call builders -> bytes |
| `codec`: `Runtime::decode_id`, `decode_call_value`, `Value`, `to_corpus_json` | `CORE:codec/decode.rs:296,658`; `CORE:codec/value.rs:12,58` | dynamic reads, CLI/agent JSON, corpus tests |
| `codec`: `Runtime::storage_key`, `decode_storage_key_params`, `storage_prefix`, `hash_param` | `CORE:codec/storage.rs:21,60,69,101` | storage reads and map iteration |
| `codec::batch::{decode_map_page, decode_map_changes}` | `CORE:codec/batch.rs:86,111` | bulk map decode (run under `spawn_blocking`: rayon fans out above 64 items, `:23`) |
| `codec::extrinsic::{TxParams, signature_payload_parts, signature_payload, encode_signed_extrinsic, decode_extrinsic, era_birth, multisig_account_id}` | `CORE:codec/extrinsic.rs:19-30,217,240,260,321,407,419` | the transaction pipeline (§3.7) |
| `mlkem::{seal, twox_128, KDF_ID}` | `CORE:mlkem/mod.rs:34-53` | shielded submission (§3.8) |
| `digest::{ChainInfo, metadata_digest, generate_extrinsic_proof}` | `CORE:digest/mod.rs:34,102,117` | `CheckMetadataHash = Enabled`, Ledger proofs |
| `timelock::{WeightsTlockPayload, UserData, encrypt_and_compress, decrypt_and_decompress, encrypt_at_round, inner_ciphertext, reveal_round, decrypt_with_signature, constants::*}` | `CORE:timelock/mod.rs:53-104,143,230,319`; `CORE:timelock/constants.rs:6-51` | CRv4 weight commits, timelocked commitments, decrypt UX |
| `pow::{seal, mine_cpu, GpuMiner, gpu_devices}` | `CORE:pow/mod.rs:9-51` | `pow_register` (§9.6) |
| `signers::ledger::LedgerDevice` (feature `ledger`) | `CORE:signers/ledger.rs:52-156` | `LedgerSigner` |
| `error::CoreError` | `CORE:error.rs:11` | mapped into rtao error enums |

### 1.3 Wrapped (rtao adds an async or typed facade; behaviour is the core's)

| Wrapper | Wraps | Why |
|---|---|---|
| `rtao_client::RuntimeHandle` (`Arc`) | `Runtime` (V15) + lazily decoded V16 (`frame-metadata` 23) + the surface report | add V16 deprecation info and the extension-version table, which `Runtime::parse` rejects (`CORE:runtime/mod.rs:194-199` [V]); cache per `(genesis, spec)` |
| typed decode (`rtao-runtime` models) | `Runtime.types: PortableRegistry` | `DecodeAsType` decodes named composites by field name into a map and errors only on a missing field (`REG:scale-decode-derive-0.16.2/src/lib.rs:222-236,345-370` [V]), so additions and reorders upstream do not break typed models. The `TypeResolver` impl for `PortableRegistry` comes from `scale-type-resolver` with its `scale-info` feature (`REG:scale-type-resolver-0.2.0/src/portable_registry.rs:59` [V]), which rtao enables explicitly because `scale-decode` depends on it with default features off (`REG:scale-decode-0.16.2/Cargo.toml` [V]) |
| `KeypairSigner` | `Keypair::sign` | async `Signer` trait, role tagging |
| `LedgerSigner` | `LedgerDevice` + `generate_extrinsic_proof` | blocking HID -> `spawn_blocking`; needs metadata hash Enabled |
| `rtao_wallet::Wallet` | keyfile functions | layout, password resolution order, permissions, creation |
| `rtao_intents::weights::TimelockCommitter` | `encrypt_and_compress` + `WeightsTlockPayload` | injectable clock and the corrected epoch predicate (§1.6) |
| `rtao_intents::pow::PowSolver` | `mine_cpu` / `GpuMiner` | `spawn_blocking`, cancellation, 5-block work window refresh |
| `rtao_intents::timelock::DrandClient` | `decrypt_with_signature` | async drand HTTP instead of the core's blocking fetch |

### 1.4 Deliberately not used on runtime paths

| Item | Why | What rtao does instead |
|---|---|---|
| `client::Client` | blocking `reqwest`, `ws://`->`http://` rewrite, polling receipts, no subscriptions/reconnect/nonce cache (`RES:B` §4.1; `CORE:client.rs:30-33` [V]) | `rtao-rpc` + `rtao-client`; `Client` is a differential oracle in e2e (§11.4 scenario 12) |
| `client::Client::submit_shielded` | signs inner and carrier with `DEFAULT_ERA_PERIOD = 64` (`CORE:client.rs:32,633,644` [V]) while the runtime rejects `submit_encrypted` with an era above 8 (`SUB:runtime/src/check_mortality.rs:19,94-100` [V]); no inner-receipt resolution | §3.8 (era 8, inner scan) |
| `transaction::{IntentCall, Policy, Plan, Executor}` | `Executor` holds `&Client` and `IntentCall::encode` takes `&Client` (`CORE:transaction.rs:828-830,998-1116` [V]); only 22 trusted constructors (`:251-826`) and the trusted semantics constructor is private, so the other 61 intents could only exist as raw/unbounded calls | `rtao-tx` + `rtao-intents`; the 22 constructors and `Policy::check` are differential oracles (call bytes via the public accessors `pallet()/function()/params()` at `:191-213` + `compose_call`) |
| `timelock::generate_commit_v2` | reads the wall clock inside (`now_unix`, `CORE:timelock/mod.rs:45,177`) and uses the core epoch simulator (§1.6) | `TimelockCommitter` composes the same public pieces with an injected clock |
| `timelock::{decrypt, get_round_info, get_reveal_round_signature}` | blocking drand fetch over `reqwest` (`RES:B` §3.4) | async drand client + `decrypt_with_signature` |
| `keyfiles::save_password_to_environment` | `std::env::set_var`, documented as not thread-safe (`CORE:keyfiles/mod.rs:126-137` [V]) | never set process env from a tokio process |
| `Runtime` built from V14 | V14 yields `apis = []` (`CORE:runtime/mod.rs:172-181` [V]), so runtime-API params cannot be encoded | rtao always fetches V15 (`Metadata_metadata_at_version(15)`). On finney today `state_getMetadata` returns V14 (357,499 bytes, version byte 14) and the runtime API returns V15 (400,688 bytes) [V: both fetched from `https://entrypoint-finney.opentensor.ai` while writing this design]; the coexistence run parsed the V14 blob |

### 1.5 What `bittensor-core` lacks and how rtao supplies it

| Gap | Evidence | rtao supplies |
|---|---|---|
| async transport, subscriptions, reconnect, endpoint pools, policy refusals, archive fallback | `RES:B` §4.3 table; `RES:A1` §6.1 | `rtao-rpc` session (§3.2) |
| nonce cache, in-flight tracking, "fate unknown" reconciliation | `RES:B` §4.3 | `rtao-client::NonceManager`, reconciliation (§3.7) |
| receipts by subscription + nested-wrapper failure decoding + MevShield failure events | `CORE:client.rs` polls (`RES:B` §4.1) | `rtao-client::outcome` |
| V16 metadata | `CORE:runtime/mod.rs:194-199` | `RuntimeHandle::v16()` via `frame-metadata` 23 |
| typed results | codec returns cyscale `Value` (`RES:B` §3.10) | `rtao-runtime` models (DecodeAsType) |
| 83 intents, preflights, completion waits | 22 constructors (`CORE:transaction.rs:251-826`) | `rtao-intents` |
| 102 reads | 20-name catalog (`RES:B` §4.1) | `rtao-reads` |
| 18-code error taxonomy, pool custom codes, remediation | core uses `snake_case(name)` + aliases (`RES:A1` §9) | `rtao-tx::errors` (generated, §4.5) |
| `Signer` abstraction; external signers | `Keypair` only (`RES:B` §4.1) | `rtao-signer` |
| btauth/1 | Python-only (`PY:http_auth.py`) | `rtao-auth`, `rtao-http` |
| wallet layout, password resolution order | Python (`PY:wallets.py:60-118` [V]) | `rtao-wallet` |
| metagraph, framework, CLI, test harness | - | `rtao-metagraph`, `rtao-neuron`, `rtao-cli`, `rtao-testkit` |

### 1.6 Known divergences in the core and how rtao handles them

1. **Epoch safety-net predicate.** The core ports `should_run_epoch` with
   `blocks_since_last_step > MAX_TEMPO_U64` (`CORE:timelock/epoch_schedule.rs:54` [V]); the pallet uses
   `BlocksSinceLastStep > tempo` (`SUB:pallets/subtensor/src/coinbase/run_coinbase.rs:1232` [V]). The paths
   differ only after deferred or skipped epochs, which is exactly when reveal-round prediction is fragile.
   rtao ships `rtao_btcore::epoch`, a port of `epoch_schedule.rs` (260 lines, Apache-2.0, attribution kept) with the pallet predicate, plus the upstream vectors and one new safety-net vector, and proposes the
   one-line fix upstream (§1.9). A test asserts the shim and the core agree on every vector except the
   safety-net one; when a pin bump contains the fix the shim is deleted.
2. **Deferral by `MaxEpochsPerBlock` is not modelled** by either simulator (`CORE:timelock/epoch_schedule.rs`
   doc at `:76-88` per `RES:B` §3.4; `RES:C1` §10 item 4). rtao keeps the `SECURITY_BLOCK_OFFSET = 3` margin
   (`CORE:timelock/constants.rs:39` [V]) and adds a reveal watcher metric so a missed reveal is visible.
3. **Wall clock in reveal-round math.** Same formula as `CORE:timelock/mod.rs:169-187` [V], but `now` comes from
   an injected `Clock` (system by default). At boot the framework compares system time with `Timestamp.Now` of
   the best block and refuses to start weight setting beyond a configurable skew (default: warn > 2 s, refuse
   > 10 s) [I: thresholds chosen to match btauth's 2 s skew and 10 s freshness, `RES:D` §1.1].
4. **Extrinsic v4 only** (`CORE:codec/extrinsic.rs:268-273` [V]) while finney V16 advertises `[4, 5]`
   (`RES:B` §3.10). rtao signs v4, like Python and the goldens; v5 is a tracked open question (§15).
5. **Fail-closed extension encoding is good and kept**: an unknown extension that contributes bytes aborts
   signing (`CORE:codec/extrinsic.rs:89-118` [V]); rtao surfaces the same condition at connect time (§3.4).

### 1.7 What rtao does about "blocking" and "dynamic `Value`"

**Blocking.** Only pure, bounded functions of the core are called from async code. CPU-heavy calls run on
`tokio::task::spawn_blocking`: NaCl keyfile decrypt (Argon2i, 512 MiB, t=8; `RES:F` §3.3), PoW mining,
Ledger HID exchanges, and large `decode_map_page` batches. `Runtime::parse` (3.9 ms on live finney metadata,
`RES:feasibility-veneer-coexistence.md`) runs once per spec, also off the reactor. The `host` feature still
compiles `reqwest::blocking` into the graph (it gates keyfiles and PoW, which rtao needs; `SUB:sdk/bittensor-core/Cargo.toml` `[features] host` [V]); rtao never calls it, enforced by:

```toml
# clippy.toml (workspace)
disallowed-methods = [
  { path = "bittensor_core::client::Client::connect", reason = "blocking reqwest client; use rtao_client::Subtensor" },
  { path = "bittensor_core::transaction::Executor::new", reason = "bound to the blocking Client; use rtao_tx::Executor" },
  { path = "bittensor_core::transaction::Executor::with_policy", reason = "bound to the blocking Client" },
  { path = "bittensor_core::timelock::generate_commit_v2", reason = "reads the wall clock; use rtao_intents::weights::TimelockCommitter" },
  { path = "bittensor_core::timelock::decrypt", reason = "blocking drand fetch" },
  { path = "bittensor_core::timelock::get_round_info", reason = "blocking drand fetch" },
  { path = "bittensor_core::timelock::get_reveal_round_signature", reason = "blocking drand fetch" },
  { path = "bittensor_core::timelock::epoch_schedule::predict_first_reveal_block", reason = "safety-net predicate differs from the pallet; use rtao_btcore::epoch" },
  { path = "bittensor_core::keyfiles::save_password_to_environment", reason = "std::env::set_var is not thread-safe" },
]
```

(`rtao-btcore`'s oracle tests opt out with `#[allow(clippy::disallowed_methods)]`.)

**Dynamic `Value`.** Kept in three places and removed from the operator hot path:

- *Encoding*: generated call structs convert to `Value` (`impl ToCoreValue`) and go through
  `Runtime::compose_call`, so call bytes are produced by the same encoder Python uses and the golden vectors pin
  (`RES:A2` §6.1). Nested calls splice as `Value::Bytes` (`CORE:codec/encode.rs:528-585` per `RES:B` §3.10).
- *Dynamic output*: `rtao query`, `rtao call`, `read_dynamic`, archive reads outside the validated spec range,
  and agent tooling return `Value` rendered with the core's corpus conventions (`CORE:codec/value.rs:58`).
- *Oracles*: the shape corpus and the core's `Client` reads in differential tests.

Typed decode is `scale-decode` over `Runtime.types`; no `Value` tree is built for the metagraph, events or
hyperparameters. One registry type, one SCALE stack (`RES:feasibility-veneer-coexistence.md` lists the unified
versions; `deny.toml` bans duplicates, §12.2).

### 1.8 Pinning and bumping

```toml
# rtao/Cargo.toml (workspace)
[workspace.dependencies]
# Pin = the monorepo tag of the runtime finney runs (immutable lightweight tag at the train commit,
# RES:H §1.2). host is required: keyfiles and PoW are host-only (CORE:lib.rs:10-25 [V]).
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }

[patch.crates-io]
# Cargo ignores [patch] tables of dependencies; tle (workspace pin) must resolve the vendored fork, the same
# copy pallet-drand decrypts with (RES:feasibility-bittensor-core-gitdep.md; RES:B §2.2).
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

- `rtao-btcore` exports `pub const BITTENSOR_CORE_TAG: &str = "v477";`. A unit test parses `Cargo.lock` and
  asserts that `bittensor-core` and `w3f-bls` resolve to the same `?tag=…#<sha>` and that the tag equals the
  constant.
- Downstream binaries must copy the `[patch.crates-io]` line (§2.6); `rtao doctor` checks their `Cargo.lock`.
- Only tags whose mainnet upgrade finalized are eligible: pre-release tags can be abandoned (v470, v471, v476; `RES:H` §1.2, §6 item 9).

Bump procedure (automated by `core-pin.yml`, §12.2):

1. Trigger: the drift job sees finney's finalized `spec_version` become N and `gh release view vN` is final.
2. `cargo xtask core bump --tag vN` edits both entries, runs `cargo update -p bittensor-core -p w3f-bls`, and
   renders `cargo xtask upstream-diff v<old>..vN` over the chain-defined paths (`sdk/bittensor-core/src/{keys,
   keyfiles,timelock,mlkem,digest,pow,signers,runtime,codec}`, `sdk/bittensor-core/Cargo.toml`,
   `sdk/bittensor-core/fixtures`, `sdk/python/tests/fixtures`) into the PR body.
3. The PR runs: SCALE-stack duplicate bans, MSRV versus the monorepo `rust-toolchain.toml` at vN (1.89 today,
   `SUB:rust-toolchain.toml` [V]), all golden/corpus/oracle tests, e2e on `subtensor-localnet:vN`.
4. Merge -> rtao minor release with a `compat.json` row (§12.3).

A weekly `core-next` lane builds against the monorepo `main` (allowed to fail) so core changes are seen before
they are tagged.

### 1.9 Proposed upstream contributions (non-blocking; rtao works without them)

| # | Change | Benefit to rtao | Size [I] |
|---|---|---|---|
| U1 | `should_run_epoch`: compare `blocks_since_last_step` with `tempo`, add a safety-net vector | delete `rtao_btcore::epoch` shim | 1 line + test |
| U2 | `generate_commit_v2_at(now_unix: f64, …)` and make `generate_commit_v2` a wrapper | deterministic commits, use the core function directly | ~10 lines |
| U3 | `submit_shielded`: era 8 for carrier and inner (`MAX_SHIELD_ERA_PERIOD`), inner receipt scan | fixes the core's own e2e path | small |
| U4 | split `host` into `keyfiles` (pure RustCrypto NaCl: `argon2` + `crypto_secretbox`, `RES:F` §3.3), `keyfiles-legacy` (fernet/OpenSSL, ansible-vault), `net` (reqwest), `parallel` (rayon), `pow-gpu` (libloading) | removes libsodium and vendored OpenSSL (Perl) from operator builds | medium |
| U5 | `Runtime::parse` accepts V16 | one metadata fetch instead of two | small |
| U6 | committed encrypted keyfile vectors ($NACL, Fernet, Ansible) | closes the gap noted in `RES:B` §8 | small |
| U7 | publish `metadata-v15.scale`/`metadata-v16.scale` as release assets | drift job no longer needs a live node for finney (`RES:H` §8 Q3) | small |
| U8 | a public `IntentCall` semantics constructor | lets third parties reuse core policy semantics | small |

---

## 2. Workspace and crate layout

### 2.1 Repository tree

```
rtao/
├─ Cargo.toml              # [workspace] members, [workspace.dependencies], [patch.crates-io] w3f-bls (§1.8)
├─ rust-toolchain.toml     # dev toolchain (current stable); MSRV checked separately (§2.5)
├─ clippy.toml             # disallowed-methods (§1.7)
├─ deny.toml               # duplicate bans for the SCALE stack, license allowlist, advisories
├─ metadata/               # pinned runtime metadata: <network>/<spec>.v15.scale, .v16.scale, index.toml (§12.1)
├─ fixtures/
│  ├─ upstream/            # vendored verbatim + MANIFEST.toml {upstream_commit, sha256}: golden.json,
│  │                       #   corpus.json, ledger_proof_vector.json, btauth-vectors.json, epoch-schedule.json
│  └─ rtao/                # rtao-recorded goldens/corpus per spec (cargo xtask record / corpus)
├─ parity/
│  ├─ upstream/            # intents.json, reads.json, errors.json, index.json + UPSTREAM.toml {fetched_at, index.commit}
│  ├─ manifest.toml        # per-name status for every upstream intent/read/code/CLI command/setting (§4.7)
│  └─ raw_only.toml        # chain calls deliberately not wrapped by an intent (coverage gate)
├─ crates/
│  ├─ rtao-btcore/  rtao-primitives/  rtao-runtime/  rtao-rpc/  rtao-signer/  rtao-client/
│  ├─ rtao-wallet/  rtao-tx/  rtao-reads/  rtao-intents/  rtao-metagraph/  rtao-auth/  rtao-http/
│  ├─ rtao-neuron/  rtao-cli/  rtao-testkit/  rtao-macros/  rtao/            (facade)
│  └─ rtao-evm/  rtao-compat-synapse/  rtao-compat-epistula/  rtao-transport-quic/   (M7/M8, optional)
├─ examples/               # validator/, miner/, owner/ templates, also the docify sources for the book
├─ interop/                # docker compose + Python scenarios: CI oracle only, never a runtime dependency
├─ book/                   # mdbook; reference pages generated by xtask (§13)
└─ xtask/                  # codegen, metadata fetch, drift, catalog sync, parity, core bump, record, docs
```

### 2.2 Crates

Dependency direction is strictly downward in this table: a crate depends only on crates listed above it, plus third-party crates, with one exception: the facade `rtao` re-exports everything, including the optional crates
listed after it.

| Crate | Purpose | Public surface (main items) | Features | Depends on |
|---|---|---|---|---|
| `rtao-btcore` | the only crate that names `bittensor_core`; re-exports the items in §1.2, maps `CoreError`, hosts the epoch shim and oracle tests | `keys`, `keyfiles`, `codec`, `runtime`, `extrinsic`, `shield`, `digest`, `timelock`, `pow`, `epoch`, `ledger`, `BITTENSOR_CORE_TAG` | `ledger`, `oracle` (dev: differential tests against `Client`/`Policy`) | `bittensor-core` (git tag) |
| `rtao-primitives` | units and identities | `AccountId`, `H256`, `NetUid`, `MechId`, `Uid`, `NetUidStorageIndex`, `Rao`, `AlphaRao`, `Balance`, `Unit`, `PerU16`, `U64F64`, `I96F32`, `I64F64`, `I32F32`, `ProportionU64`, `StakeWeight`, `BlockNumber`, `AxonTarget` (uid, hotkey, socket address), `hparams::{HyperparamKind, HYPERPARAMS}` | `serde` (default), `schema` (schemars) | `rtao-btcore`, `parity-scale-codec`, `scale-encode`, `scale-decode` |
| `rtao-runtime` | the subtensor surface: generated descriptors + hand-written typed models + surface verification | `calls::*`, `storage::*`, `apis::*`, `constants::*`, `events::*`, `errors::ERRORS`, `models::*`, `Call`, `StorageItem`, `RuntimeApiMethod`, `StaticEvent`, `verify_surface()`, `SelectiveMetagraphIndex` | - | `rtao-primitives`, `rtao-btcore` |
| `rtao-rpc` | tokio JSON-RPC session implementing `subxt_rpcs::client::RpcClientT` | `Session`, `SessionConfig`, `SessionEvent`, `RpcError`, `Network`, `EndpointPool`, `RtaoRpcConfig` (for `LegacyRpcMethods`) | `rate-limit` (default), `test-util` (scripted transport with fault injection, the base of `rtao-testkit::FakeChain`) | `subxt-rpcs`, `jsonrpsee`, `tokio`, `governor` |
| `rtao-signer` | the signing contract and in-process/hardware/remote signers | `Signer`, `SigningRequest`, `SigningContext`, `SignerCaps`, `Signature`, `SignerPayloadJson`, `KeypairSigner`, `LedgerSigner`, `RemoteSigner`, `ExtensionSigner`, `VaultSigner` | `ledger`, `remote`, `extension`, `vault` | `rtao-primitives`, `rtao-btcore` |
| `rtao-client` | the chain client | `Subtensor`, `Snapshot`, `BlockRef`, `HeadPolicy`, `RuntimeHandle`, `Events`, `RawEvent`, `TxRequest`, `TxReceipt`, `Inclusion`, `Mortality`, `NonceManager`, `CallBytes`, `compose::{sudo, proxy, batch_all, multisig}`, `shield::submit_shielded`, `EpochClock`, `EpochStatus`, waits | `shield` (default), `metadata-hash`, `subxt` (escape hatch), `test-util` | `rtao-rpc`, `rtao-runtime`, `rtao-signer`, `rtao-btcore` |
| `rtao-wallet` | btwallet/btcli-compatible wallets | `Wallet`, `WalletRef`, `Keyfile`, `PasswordSource`, `resolve_password`, `create_*`, `regen_*`, `list_wallets` | `prompt` (default), `keychain-macos`, `keyring` | `rtao-signer`, `rtao-btcore` |
| `rtao-macros` | proc macros | `#[derive(Intent)]`, `#[rtao::read]` | - | `syn`, `quote` |
| `rtao-tx` | v11 product layer for mutations | `Intent`, `DynIntent`, `IntentMeta`, `Spend`, `Touches`, `Policy`, `Plan`, `Preflight`, `Executor`, `ExecOpts`, `ExtrinsicResult`, `ChainError`, `ErrorCode`, `TxError`, `UnsignedExtrinsic`, `ProxyType`, `MultisigAdapter`, `IntentRegistry`, `tools()` | `cli` (clap derives), `schema` (default) | `rtao-client`, `rtao-macros` |
| `rtao-reads` | the 102 named reads and 14 namespaces | `ReadSpec`, `ReadRegistry`, `Category`, namespace structs (`Subnets`, `Staking`, `Epochs`, …), typed result structs | `cli` | `rtao-client`, `rtao-macros` |
| `rtao-intents` | the 83 intents (+ weights conform, timelock committer, PoW solver, hyperparameter encoding) | `staking::*`, `weights::*`, `registration::*`, `owner::*`, `serving::*`, `proxy::*`, `multisig::*`, `crowdloan::*`, `leasing::*`, `collateral::*`, `locks::*`, `coldkey::*`, `children::*`, `identity::*`, `evm::*`, `basket::*`, `root::*`, `batch::Batch`, `REGISTRY` | per group + `all` (default), `pow-gpu` | `rtao-tx`, `rtao-reads` |
| `rtao-metagraph` | the operator's view of a subnet | `Metagraph`, `NeuronRef`, `MetagraphNeuron`, `MetagraphDiff`, `MetagraphSync`, `MetagraphHandle`, `SyncPolicy`, `Preset`, `snapshot::{save, load}` | `commitments` | `rtao-reads` |
| `rtao-auth` | btauth/1, no I/O | `payload()`, `sign()`, `Verifier`, `VerifyConfig`, `Caller`, `AuthError`, `NonceStore`, `InMemoryNonceStore` | - | `rtao-signer`, `rtao-btcore`, `sha2` |
| `rtao-http` | btauth/1 for axum/tower and reqwest | `BtAuthLayer`, `VerifiedCaller`, `CallerPolicy`, `MetagraphPolicy`, `PerHotkeyLimiter`, `BtAuthClient`, `FanOut`, `RedisNonceStore` | `server`, `client`, `metagraph-policy`, `redis` | `rtao-auth`, (opt) `rtao-metagraph` |
| `rtao-neuron` | the framework | `NeuronConfig`, `NeuronRuntime`, `Validator`, `Miner`, `NeuronCx`, `WeightScheduler`, `Cadence`, `AxonPublisher`, `StateStore`, `EmaScores`, `PruneRisk`, `OwnerOps`, `HyperparamPlanner`, `OwnerMonitor`, `telemetry::init` | `validator`, `miner`, `owner`, `auto-update`, `prometheus` (default) | `rtao-intents`, `rtao-metagraph`, `rtao-http`, `rtao-wallet` |
| `rtao-cli` | the `rtao` binary (btcli-equivalent) | - | per command group | everything above |
| `rtao-testkit` | harness | `Localnet`, `LocalnetProfile`, `FakeChain`, `Scenario`, fixture loaders, `interop::*` | `docker` (default), `interop` | `rtao-client`, `rtao-rpc/test-util`, `rtao-wallet` |
| `rtao` | facade + prelude | re-exports | `validator`, `miner`, `owner`, `full`, `ledger`, `blocking`, `subxt`, … (§2.4) | all |
| `rtao-evm` (M7) | EVM helpers parity (`PY:evm/`) | H160 mirror, EIP-191 association signature, `eth_*` RPC, precompile ABIs | - | `rtao-client`, `alloy-primitives`, `alloy-sol-types`, `k256` [I: crate choice] |
| `rtao-compat-synapse` (M8) | v10 Synapse/Axon/Dendrite wire format | header codec, sha3 body hash with `PythonStr`, dendrite signer, axon verifier | - | `rtao-signer`, `axum`, `reqwest` |
| `rtao-compat-epistula` (M8) | Epistula v2 dialects | signer/verifier with dialect knobs | - | `rtao-signer` |
| `rtao-transport-quic` (M8) | btlightning adapter | `QuicClient`, `QuicServer` glue to `Signer` and `MetagraphPolicy` | - | `rtao-http` (policy types), `rtao-signer`, `btlightning` (default features off) |

Two crates are worth justifying. `rtao-btcore` exists so that a core API change is absorbed in one crate and so
the `disallowed-methods` list has one place to be relaxed. `rtao-metagraph` is separate from `rtao-reads`
because the metagraph is the one structure every role and the CLI share, and its sync service is useful
without the framework.

### 2.3 Dependency layers

A crate may depend only on crates in lower layers (the table in §2.2 lists the exact edges):

| Layer | Crates |
|---|---|
| 0 | `bittensor-core` (git tag `v<spec>`; brings `tle`, patched `w3f-bls`, the RaoFoundation `sp-core` fork, `frame-metadata` 23) |
| 1 | `rtao-btcore` |
| 2 | `rtao-primitives` |
| 3 | `rtao-runtime`, `rtao-signer`, `rtao-rpc` (third-party only), `rtao-macros` (proc-macro) |
| 4 | `rtao-client`, `rtao-auth`, `rtao-wallet` |
| 5 | `rtao-tx`, `rtao-reads`, `rtao-evm` |
| 6 | `rtao-intents`, `rtao-metagraph` |
| 7 | `rtao-http` (optionally `rtao-metagraph`), `rtao-compat-synapse`, `rtao-compat-epistula` |
| 8 | `rtao-neuron`, `rtao-testkit`, `rtao-transport-quic` |
| 9 | `rtao-cli`, `rtao` (facade) |

### 2.4 Feature flags (facade `rtao`)

| Feature | Enables | Typical user |
|---|---|---|
| `validator` | `rtao-neuron/validator`, `rtao-metagraph`, `rtao-http/client`, `rtao-intents/{weights,serving,registration}` | validator binary |
| `miner` | `rtao-neuron/miner`, `rtao-http/{server,metagraph-policy}`, `rtao-intents/{serving,registration,collateral}` | miner binary |
| `owner` | `rtao-neuron/owner`, `rtao-intents/{owner,proxy,multisig,identity}` | owner tooling |
| `full` | every intent group and read, `blocking`, `ledger`, `mev-shield`, `pow` | wallets/CLIs/agents |
| `blocking` | `rtao::blocking::Subtensor` (owns a current-thread runtime; parity with `bt.Subtensor`'s sync facade, `RES:A2` §1.5) | scripts |
| `ledger`, `remote-signer`, `extension-signer`, `vault-signer` | the corresponding `rtao-signer` features | owners and CLI users |
| `keychain-macos`, `keyring` | password sources (§6.2) | CLI users |
| `pow-gpu` | OpenCL miner via the core's `GpuMiner` (`libloading`, no link-time dependency, `RES:B` §3.7) | PoW registrants |
| `metadata-hash` | `CheckMetadataHash = Enabled` path (needed by Ledger) | owners |
| `redis` | shared btauth nonce store | multi-process miners |
| `subxt` | `Subtensor::subxt()` -> `subxt::OnlineClient` over the same `RpcClient` with `LegacyBackend` | explorers, power users |
| `compat-synapse`, `compat-epistula`, `quic` | optional networking crates | migrating subnets |

`bittensor-core/host` is always on (keyfiles and PoW require it); this is a documented cost (§15).

### 2.5 Edition, MSRV, toolchain, lints

- Edition 2024 for every rtao crate (the core stays 2021, which Cargo handles per crate).
- **MSRV = max(subxt-rpcs MSRV, monorepo `rust-toolchain.toml` at the pinned tag)** = max(1.88, 1.89) = **1.89**
  today (`RES:F` §8 for 1.88; `SUB:rust-toolchain.toml` [V] for 1.89). The core-pin job raises it when the
  monorepo does. CI builds once on the MSRV and everything else on current stable (1.99, `RES:F` §8).
- Workspace lints: `missing_docs = "deny"` on library crates, `unsafe_code = "forbid"` outside `rtao-btcore`
  (which needs none either), clippy `-D warnings`, the `disallowed-methods` list of §1.7.
- Errors: `thiserror` enums per crate with context strings; `anyhow` only in binaries, examples and tests
  (matches the maintainer's existing style, `RES:E` §6).

### 2.6 What a subnet owner's `Cargo.toml` looks like

The rtao repository URL and tag below are illustrative (the repository is not published yet).

Validator binary:

```toml
[package]
name = "sn42-validator"
version = "0.1.0"
edition = "2024"

[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.3.0", default-features = false, features = ["validator"] }
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }
serde = { version = "1", features = ["derive"] }
anyhow = "1"

[patch.crates-io]
# Required by rtao's bittensor-core dependency; must match rtao's BITTENSOR_CORE_TAG (`rtao doctor` checks).
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Miner binary:

```toml
[package]
name = "sn42-miner"
version = "0.1.0"
edition = "2024"

[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v0.3.0", default-features = false, features = ["miner"] }
axum = "0.8"
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }
serde = { version = "1", features = ["derive"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Owner tool: `features = ["owner", "ledger"]`, same patch. The examples directory ships all three as templates
(§13).

### 2.7 Third-party stack (versions verified by `RES:F` §0.1 and the coexistence build)

`tokio 1.53`, `subxt-rpcs 0.51.1` (`reconnecting-rpc-client` not used because rtao owns reconnect; `mock-rpc-client`
in tests), `jsonrpsee 0.26.1` (`ws-client`, `async-client`, `client-ws-transport-tls`), `parity-scale-codec
3.7.5`, `scale-info 2.11.6`, `scale-decode 0.16.2`, `scale-encode 0.10.1`, `scale-value 0.18.2`, `frame-metadata
23.0.1` (all unified with the core, `RES:feasibility-veneer-coexistence.md`), `scale-type-resolver 0.2` with
feature `scale-info` (§1.3), `axum 0.8.9`, `tower 0.5.3`,
`tower-http 0.7.1`, `reqwest 0.13.5` (rustls), `sha2 0.10`, `governor` (rate limits) [I: crate choice], `arc-swap`
[I], `tracing 0.1.44`, `tracing-subscriber 0.3.23`, `metrics 0.24.6`, `metrics-exporter-prometheus 0.18.3`,
`clap 4.6.7`, `serde_json`, `schemars` [I], `proptest 1.12`, `insta 1.49`, `wiremock 0.6.5`, `criterion 0.8.2`,
`docify 0.4.1`, `mdbook 0.5.4`. subxt itself is optional (feature `subxt`, 0.51.1).

---

## 3. Chain client (`rtao-rpc`, `rtao-client`, `rtao-runtime`)

### 3.1 Async model

- Everything is `async` on tokio's multi-thread runtime; no global state. `Subtensor` is a cheap `Clone`
  (`Arc` inner) and `Send + Sync`.
- Background tasks owned by a client (all cancelled when the last clone drops): the session supervisor
  (`rtao-rpc`), the head tracker (best + finalized subscriptions with gap backfill), the runtime-version
  watcher. Neuron services (metagraph sync, weight scheduler) live in `rtao-neuron`, not in the client.
- Every read happens at a `Snapshot` (a block hash + its `RuntimeHandle`). There is no unpinned read API; the
  client's `HeadPolicy` (`Best` by default for parity with Python's head reads, `Finalized` in neuron runtimes)
  decides which head `client.head()` returns. This removes the v11 class of "gather" reads that straddle
  blocks (`RES:A2` §1.3 lists `delegate_take`, `coldkey_swap_announcement`, `hotkey_identities`,
  `miner_collateral`).

### 3.2 Transport: `rtao-rpc::Session`

`Session` implements `subxt_rpcs::client::RpcClientT` (`request_raw`, `subscribe_raw`;
`REG:subxt-rpcs-0.51.1/src/client/rpc_client_t.rs` [V]), so the typed method wrappers in
`subxt_rpcs::methods::LegacyRpcMethods` (`state_call`, `state_get_storage`, `state_query_storage_at`,
`state_get_keys_paged`, `chain_*`, `author_submit_and_watch_extrinsic`, `state_subscribe_runtime_version`, …,
`REG:subxt-rpcs-0.51.1/src/methods/legacy.rs:34-388` [V]) work unchanged on top of it, and tests can swap in
`subxt_rpcs`'s mock client or rtao's `FakeChain`.

```rust
pub struct SessionConfig {
    pub endpoints: Vec<Url>,               // primary first, then fallbacks (PY:settings.py:56-77 per RES:A1 §7)
    pub archive_endpoints: Vec<Url>,       // used only for "State already discarded" (RES:A1 §6.1)
    pub retry_forever: bool,               // BT_RETRY_FOREVER / --retry-forever
    pub handshake_timeout: Duration,       // 10 s   (PY:_transport/rpc.py:L60, RES:A1 §4.11)
    pub silence_timeout: Duration,         // 60 s with requests pending -> presumed dead (rpc.py:L323)
    pub max_failures_per_endpoint: u32,    // 5 consecutive, then rotate (rpc.py:L322)
    pub ping: Option<PingConfig>,          // 20 s ping / 60 s inactivity (SN2:sn2-chain/src/lib.rs:29-32 [V])
    pub rate_limit: Option<Quota>,         // token bucket per endpoint (§3.2.4)
    pub max_response_bytes: u32,           // metagraph of 2,500 uids is ~0.5 MB SCALE, ~1 MB hex (RES:C2 §5.1)
    pub subscription_buffer: usize,        // 2048, oldest dropped (rpc.py:L70)
    pub headers: Vec<(HeaderName, HeaderValue)>, // e.g. bearer token for private nodes (RES:E §4.1 item 10)
    pub plaintext_ws: PlaintextPolicy,     // ws:// allowed for loopback/private nets only by default
}
pub enum SessionEvent { Connected { url: Url }, Disconnected { url: Url, cause: String },
                        Rotated { from: Url, to: Url }, PolicyRefused { url: Url, retry_after: Option<Duration> } }
```

#### 3.2.1 Reconnect semantics (port of the v11 `RpcSession`, `RES:A1` §6.1)

| Situation | Behaviour |
|---|---|
| idempotent request (`state_*`, `chain_*`, `system_*`, `rpc_methods`) in flight when the socket dies | re-sent on the next socket before callers see an error; bounded by `silence_timeout` |
| `author_submitExtrinsic` / `author_submitAndWatchExtrinsic` in flight | **never re-sent**; the future fails with `RpcError::FateUnknown { extrinsic_hash }` and the transaction layer reconciles (§3.7.6); resending could hit "already imported" and clear the nonce cache under a live transaction (`RES:A1` §4.8) |
| head / runtime-version subscriptions | transparently re-subscribed; the head tracker detects number gaps and backfills with `chain_getBlockHash(n)` so event scanners see every block |
| extrinsic watch subscription | terminal `FateUnknown` (never re-subscribed) |
| connect failure or `max_failures_per_endpoint` consecutive timeouts | rotate to the next endpoint; backoff `min(0.25*n, 5) s`, full-pool cycles `min(2*cycle, 30) s` with `retry_forever` (`RES:A1` §4.11) |
| gateway policy refusal: handshake HTTP 403/429, JSON-RPC `-32004`/`-32005`, WS close 1008 | **terminal**, never retried or rotated (to avoid amplifying traffic), surfaced as `RpcError::PolicyRefused { retry_after }` (`RES:A1` §6.1, `PY:_transport/rpc.py:L53-59,L92-124,L190-241` per A1) |
| `"State already discarded"` | the read is retried once on the lazily opened archive pool (`RES:A2` §1.3) |

rtao does **not** use subxt's `ReconnectingRpcClient`: it "doesn't retain subscriptions and pending method
calls" (`RES:F` §1.2), failing in-flight calls with `DisconnectedWillReconnect` (`SN2:sn2-chain/src/lib.rs` doc
comment [V]), so rtao would still need its own per-method policy (resend idempotent reads, `FateUnknown` for
submissions, resubscribe heads), which is the whole supervisor. rtao implements it once over jsonrpsee's
`WsClient` (one per socket); the supervisor owns the swap.

#### 3.2.2 Why the legacy RPC namespace and not a subxt backend

The v11 SDK uses only the legacy namespace (`state_call`, `state_getStorageAt`, `state_queryStorageAt`,
`state_getKeysPaged`, `chain_*`, `author_*`; `RES:C2` §7). subxt 0.51's default `CombinedBackend` fails with
`-32601` on the finney WS endpoint while `LegacyBackend` and `ChainHeadBackend` work (`RES:feasibility-subxt.md`);
SN2 assembles a backend without the archive backend for the same reason (`SN2:sn2-chain/src/lib.rs:47-62` [V]).
Since rtao builds extrinsics with the core and decodes with scale-decode, it needs no backend at all and calls the legacy methods directly. The optional `subxt` feature builds an `OnlineClient` from the same
`RpcClient` with an explicit `LegacyBackend` for users who want subxt's API.

#### 3.2.3 Custom subtensor RPCs

Not used. `subnetInfo_*`, `neuronInfo_*`, `swap_*` are a lagging subset of the runtime APIs (no
`getSubnetHyperparamsV3`, no `getNextEpochStartBlock`; `RES:C2` §0 item 1, §7). All runtime APIs go through
`state_call("{Api}_{method}")`.

#### 3.2.4 Rate limits

Public OTF endpoints are rate-limited to roughly one request per second per IP
(`SUB:docs/guides/local-development.mdx:160` [V]). `SessionConfig::rate_limit` is a token bucket applied in
`request_raw`: off for `local` and custom endpoints, on for the public pools [I: default quota is a tuning
decision; start at 1 r/s with burst 10 and measure]. Storage map scans use page size 100 on public endpoints
(Python's choice, `RES:A2` §7) and 1,000 on own nodes (the core's, `CORE:client.rs:33` [V]); neuron runtimes
prefer runtime APIs over scans for exactly this reason (`RES:A2` §7). Operators are told to run their own lite
node for production (`SUB:docs/concepts/network.mdx` "run your own node" [V]).

#### 3.2.5 Telemetry

`rtao_rpc_requests_total{method,endpoint,outcome}`, `rtao_rpc_request_seconds{method}` (histogram),
`rtao_rpc_reconnects_total{endpoint}`, `rtao_rpc_policy_refusals_total`, `rtao_rpc_inflight`. URLs are
sanitised in logs (userinfo stripped), as the Python transport does (`RES:A1` §6.1).

### 3.3 Networks and the connect sequence

| Name | Primary | Fallbacks | Archive pool | Genesis |
|---|---|---|---|---|
| `finney` (default) | `wss://entrypoint-finney.opentensor.ai:443` | `wss://lite.chain.opentensor.ai:443`, `wss://lite.sub.latent.to:443` | `wss://archive.chain.opentensor.ai:443`, `wss://archive.sub.latent.to:443` | `0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03` |
| `test` | `wss://test.finney.opentensor.ai:443` | - | - | `0x8f9cf856bf558a14440e75569c9e58594757048d7b3a84b5d25f6bd978263105` |
| `devnet` | `wss://dev.chain.opentensor.ai:443` | - | - | `0x077899043eb684c5277b6814a39161f4ce072b45e782e12c81a521c63fb4f3e5` |
| `archive` | `wss://archive.chain.opentensor.ai:443` | `wss://archive.sub.latent.to:443` | - | finney's |
| `local` | `$BT_CHAIN_ENDPOINT` or `ws://127.0.0.1:9944` | - | - | not pinned (regenerated per localnet container, `RES:H` §1.3) |
| any `ws(s)://` URL | as given | - | - | not pinned |

(Endpoints: `RES:A1` §7, `RES:G` §4.1; genesis hashes: `RES:H` §1.1.)

`Subtensor::connect(network)`:

1. Build the session; connect with a startup deadline (30 s, the SN2 bound, `SN2:sn2-chain/src/lib.rs:29` [V]).
2. `chain_getBlockHash(0)` -> refuse to continue when a named network's genesis does not match.
3. `system_properties` -> ss58 format must be 42.
4. `chain_getFinalizedHead` + `state_getRuntimeVersion(at)`; `transactionVersion` missing is a hard error
   (Python refuses the same way, `PY:_transport/runtime.py:42-54` [V]).
5. Resolve the `RuntimeHandle` for `(genesis, spec)` (§3.4).
6. Verify the generated surface against it (§3.6.4); start the head tracker and runtime watcher.
7. Load token symbols for display (7-day disk cache, as Python, `RES:A2` §1.4).

### 3.4 Runtime metadata, caching, upgrades

- **Fetch**: `state_call("Metadata_metadata_at_version", 0x0f000000)` -> strip the SCALE `Option<Vec<u8>>`
  wrapper -> `Runtime::parse(bytes, spec, tx_version, 42)`. V16 (`0x10000000`) is fetched lazily for the
  extension-version table, deprecation info and drift checks, decoded with `frame-metadata` 23 directly
  (`RES:H` §1.1 explains why digests need V15 and codegen prefers V16).
- **Memory cache**: LRU of four `RuntimeHandle`s keyed by `(genesis, spec_version)`, plus 512 `block_hash ->
  spec` entries; a block's runtime is resolved from its parent's runtime version (same rules as
  `PY:_transport/runtime.py`, `RES:A2` §1.4).
- **Disk cache shared with Python**: rtao reads and writes the v11 format exactly: directory
  `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache/`, file `md1-<genesis[..16]>-<spec>.bin`,
  content `b"STMD" || u32 LE transaction_version || u8 is_v15 || raw metadata`, atomic temp-file + rename, eight
  newest kept (`PY:_transport/runtime.py:36-112` [V]). A Python validator and an rtao miner on the same host
  share one cache. V16 blobs go to `rtao-md16-<genesis16>-<spec>.scale`, outside Python's `md*.bin` prune glob.
- **Upgrades**: `state_subscribeRuntimeVersion` plus a check on every finalized head. On a new spec: fetch,
  parse, verify the surface, swap the head runtime. Any prepared-but-unsigned transaction from the old spec is
  refused (`PY:_transport/interface.py:617-623` per `RES:H` §2.2), also on `transaction_version` change
  (`RES:A1` §10 item 16).
- **Extension guard at connect**: every identifier in the extension list must be one the core encodes
  (`PAYLOAD_FIELDS`, `CORE:codec/extrinsic.rs:38-55` [V]) or zero-sized in both slots; otherwise signing is
  disabled with an error naming the extension (the same rule the core enforces at signing time, `:89-118` [V]), moved to boot so an operator learns at deploy time rather than at the first weight commit. Today all four custom extensions
  are empty (`RES:C1` §0 item 3; `RES:F` §1.5).

### 3.5 Reading chain state

```rust
impl Subtensor {
    pub async fn head(&self) -> Result<Snapshot>;                     // per HeadPolicy
    pub async fn at(&self, block: BlockRef) -> Result<Snapshot>;      // Best | Finalized | Number(u32) | Hash(H256)
}
impl Snapshot {
    pub fn number(&self) -> u32;  pub fn hash(&self) -> H256;  pub fn runtime(&self) -> &RuntimeHandle;
    // typed
    pub async fn storage<S: StorageItem>(&self, key: &S::Key) -> Result<S::Output>;          // Default -> value, Optional -> Option
    pub async fn storage_batch<S: StorageItem>(&self, keys: &[S::Key]) -> Result<Vec<S::Output>>;   // one state_queryStorageAt
    pub fn storage_iter<S: StorageItem>(&self, prefix: S::Prefix) -> impl Stream<Item = Result<(S::Key, S::Value)>>;
    pub async fn call<M: RuntimeApiMethod>(&self, method: &M) -> Result<M::Output>;          // state_call at this hash
    pub fn constant<C: ConstantItem>(&self) -> Result<C::Value>;                              // from this block's runtime
    pub async fn events(&self) -> Result<Events>;                                             // lazy, see below
    // dynamic
    pub async fn storage_value(&self, pallet: &str, item: &str, keys: &[Value]) -> Result<Option<Value>>;
    pub async fn call_dynamic(&self, api: &str, method: &str, params: &[Value]) -> Result<Value>;
    pub async fn timestamp(&self) -> Result<SystemTime>;  pub fn block_time(&self) -> Duration; // Aura.SlotDuration
}
```

- **Storage**: keys are built by `Runtime::storage_key` from the live entry (hashers and key types come from the
  runtime, never from generated code, so archive reads at older specs work). Values decode with `DecodeAsType`
  against the entry's value type id. Miss semantics follow the modifier: `Default` decodes the metadata default
  bytes, `Optional` yields `None` (`RES:A2` §1.4). `Owner`, `Keys`, `SubnetOwnerHotkey` default to the zero
  account; their typed descriptors map that to `None` (`RES:A2` §9 item 9; a deliberate, documented divergence from v11).
- **Map iteration**: `state_getKeysPaged` + `state_queryStorageAt` per page, keys and values pinned to the
  snapshot hash; key components recovered with `decode_storage_key_params`.
- **Runtime APIs**: params are encoded per input type id from `Runtime.apis` (V15), outputs decode typed; no
  runtime API on spec 477 returns double-encoded `Vec<u8>` (`RES:C2` §0 item 2), so no legacy decoders are
  needed in the supported range. Methods gated by `api_version` (`SubnetInfoRuntimeApi` v2,
  `BetaBasketRuntimeApi` v5; `RES:C2` §1.0) are checked against the runtime's advertised versions before the
  call.
- **Constants**: read from the snapshot's runtime (Python reads them from the head codec,
  `RES:A2` §1.3; per-snapshot is more correct across upgrades and is a documented divergence).
- **Events**: `Events` decodes `System.Events` lazily: the outer structure is walked once with a
  scale-decode visitor that records `(phase, pallet name, event name, field byte range)` and skips field
  decoding (`Variant::name()`/`skip_decoding()`, `REG:scale-decode-0.16.2/src/visitor/types/variant.rs:70-91`
  [V]); `RawEvent::decode_as::<E: StaticEvent>()` decodes the fields on demand. Events are always matched by
  `(pallet, name)`, never by index: event indices are not stable across specs (SubtensorModule 149 changed
  meaning 461->477, `RES:H` §0 item 4).

### 3.6 Generated surface (`rtao-runtime`) and how it is regenerated

#### 3.6.1 Inputs and outputs

`cargo xtask codegen` reads `metadata/finney/<spec>.v16.scale` (the canonical pin) with `frame-metadata` 23 and
writes `crates/rtao-runtime/src/generated/{calls,storage,apis,constants,events,errors}.rs` and `surface.json`
(the IR). Rules, mostly copied from the Python generator (`RES:A2` §5):

- deterministic and sorted; the header names the metadata content hash computed **without** `System.Version`, so
  a spec bump with no surface change yields no diff (`RES:H` §1.7; `RES:CORRECTIONS.md` item 2);
- only covered pallets get typed call structs (the Python `COVERED_PALLETS` 13 plus `System`, `Timestamp`,
  `Aura`, `Drand`, `Sudo`), every pallet gets name tables;
- type identities map through `codegen/types.toml` (`NetUid -> rtao_primitives::NetUid`, `MechId`,
  `NetUidStorageIndex`, `TaoBalance -> Rao`, `AlphaBalance -> AlphaRao`, `PerU16`, `AccountId32 -> AccountId`,
  `H256`, `substrate_fixed::FixedU128<U64> -> U64F64`, `FixedI128<U32> -> I96F32`, `BoundedVec<T,_> -> Vec<T>`,
  `Compact<T> -> T`); anything unmapped becomes `rtao_btcore::codec::Value` unless a hand-written model is
  registered for it in `codegen/models.toml`;
- V16 `deprecation_info` becomes `#[deprecated(note = …)]` (`RES:H` §1.1 lists deprecated calls 70, 73, 139,
  140), and calls rtao must never emit (the fee-charging no-op `set_tempo`/`set_activity_cutoff_factor`,
  `RES:G` §1.5 item 16) are `#[doc(hidden)]` and excluded from intents.

Excerpt of generated output:

```rust
// @generated by `cargo xtask codegen` from metadata/finney/477.v16.scale
// surface hash 9c41…e2 (System.Version excluded). Do not edit.
pub mod subtensor_module {
    pub mod calls {
        /// `SubtensorModule::set_mechanism_weights`: call index 119 at generation time (informational:
        /// composition resolves by name against the live runtime).
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub struct SetMechanismWeights {
            pub netuid: NetUid, pub mecid: MechId, pub dests: Vec<u16>, pub weights: Vec<u16>, pub version_key: u64,
        }
        impl crate::Call for SetMechanismWeights {
            const PALLET: &'static str = "SubtensorModule";
            const NAME: &'static str = "set_mechanism_weights";
            const ARGS: &'static [(&'static str, &'static str)] =
                &[("netuid", "NetUid"), ("mecid", "MechId"), ("dests", "Vec<u16>"), ("weights", "Vec<u16>"), ("version_key", "u64")];
            fn to_value(&self) -> rtao_btcore::codec::Value { /* generated record */ }
        }
    }
    pub mod storage {
        /// `Tempo`: Map<Identity>(NetUid) -> u16, Default.
        pub struct Tempo;
        impl crate::StorageItem for Tempo {
            const PALLET: &'static str = "SubtensorModule";
            const NAME: &'static str = "Tempo";
            const HASHERS: &'static [Hasher] = &[Hasher::Identity];
            const KEY_IDENTS: &'static [&'static str] = &["NetUid"];
            const VALUE_IDENT: &'static str = "u16";
            type Key = (NetUid,); type Value = u16; type Output = u16;
        }
    }
}
```

#### 3.6.2 Hand-written typed models

`rtao-runtime/src/models/` holds the structures operators touch, written once with `#[derive(DecodeAsType)]`
and registered in `codegen/models.toml` so generated runtime-API descriptors return them: `MetagraphWire`
(77 fields, `RES:C2` §1.1.1), `SelectiveMetagraphWire`, `NeuronInfoLite`, `NeuronInfo`, `DynamicInfo`,
`SubnetInfoV2`, `HyperparamEntry`/`HyperparamValue` (v3), `StakeInfo`, `StakeAvailability`, `DelegateInfo`,
`SimSwapResult`, `SubnetPrice`, `LockState`, `MinerCollateralState`, `AxonInfo`, `PrometheusInfo`,
`NeuronCertificate`, `ChainIdentityV2`, `SubnetIdentityV3`, `CommitmentRegistration` + `Data`, `SubnetLease`,
`CrowdloanInfo`, `ProxyDefinition`, `MultisigInfo`, `BasketSummary`, `BasketPosition`, `BetaPricing`. Because
`DecodeAsType` matches named fields by name and ignores extras (`REG:scale-decode-derive-0.16.2/src/lib.rs:229-236`
[V]), runtime additions do not break them; removals and renames fail loudly with `CannotFindField`.

`SelectiveMetagraphIndex` (0..=76) is vendored from `SUB:pallets/subtensor/src/rpc_info/metagraph.rs:488-650`
(`RES:C2` §1.1.3) because the metadata only carries the struct; a CI test regex-parses that file at the pinned
tag and asserts equality.

#### 3.6.3 Regeneration per runtime release

The drift job (§12.2) fetches V15/V16 on any `(spec, :code)` change on finney/testnet/devnet, runs the semantic
diff, regenerates, and opens a PR. For the common behaviour-only bump the generated code does not change and the
PR only adds the metadata files and an `index.toml` row. Generated-code changes bump `rtao-runtime`'s version
(minor for additive, major for removed/changed items that rtao exposes), checked by `cargo-semver-checks` against
the previous rtao tag.

#### 3.6.4 Surface verification (the runtime analogue of Python's `--drift` gate)

At connect and on every new runtime, `verify_surface(&RuntimeHandle) -> SurfaceReport` checks every generated
item that rtao code uses: pallet and call exist, argument names and type identities equal
(`Runtime::type_ident`, `CORE:runtime/mod.rs:272` [V]); storage hashers, key idents and value idents; runtime-API
input names/idents, output ident and advertised `api_version`; events and errors used by receipts; each model's
declared field set. Classes follow `RES:H` §3.3 step 4 (breaking / behavioural / additive). Policy
`SurfacePolicy::FailClosed` (default): an intent whose wrapped call changed shape returns
`TxError::SurfaceChanged` at plan time; a typed read whose model no longer matches returns
`ReadError::SurfaceChanged` and its `read_dynamic` twin still works. Gauge `rtao_surface_breaking_items`.

### 3.7 Transactions

#### 3.7.1 Pipeline

```rust
pub struct TxRequest {
    pub call: CallBytes,                   // bytes + the spec they were composed under
    pub signer: Arc<dyn Signer>,
    pub nonce: NoncePolicy,                // Auto | Explicit(u32)
    pub mortality: Mortality,              // Mortal { period } (default 128) | Immortal
    pub metadata_hash: MetadataHashMode,   // Disabled (default) | Enabled (Ledger, opt-in)
    pub wait: WaitFor,                     // Pool | InBlock | Finalized (default Finalized, PY:executor.py L782-788 per RES:A1 §4.7)
    pub deadline: Option<Duration>,
}
impl Subtensor { pub async fn submit(&self, req: TxRequest) -> Result<TxReceipt, SubmitError>; }
```

1. Refuse if `req.call.spec_version` differs from the head runtime's spec.
2. Nonce: `NonceManager::reserve(account)` (§3.7.2).
3. Era: `Mortal{period}` -> `birth = era_birth(period, finalized_number)` (`CORE:codec/extrinsic.rs:407` [V]),
   anchored at the **finalized** head like Python (`RES:A1` §4.9); `era_block_hash = chain_getBlockHash(birth)`;
   immortal anchors at genesis. When finality lags by more than `period/2` the request still proceeds
   but `rtao_finality_lag_blocks` and a warning make the shortened validity visible.
4. `TxParams { era, nonce, tip: 0, tip_asset_id: None, genesis, era_block_hash, metadata_hash }`
   (`CORE:codec/extrinsic.rs:19-30` [V]); tips are always zero because priority is flat and tips buy nothing
   (`RES:C1` §0 item 5).
5. `(extra, additional) = runtime.signature_payload_parts(&params)`; `payload = signature_payload(call, &params)`
   (blake2-256 above 256 bytes, `:240-253`).
6. `signature = signer.sign_extrinsic(&SigningRequest{…})` (§6.3).
7. `(bytes, hash) = runtime.encode_signed_extrinsic(call, pubkey, &sig, scheme_byte, &params)` (`:260-316`).
8. Submit: `WaitFor::Pool` -> `author_submitExtrinsic` (the result means *accepted by the pool*; rtao's `Inclusion::PoolAccepted` makes this v11 gotcha explicit, `RES:A1` §10 item 1); otherwise
   `author_submitAndWatchExtrinsic` until `InBlock`/`Finalized`, with `usurped`, `retracted`,
   `finalityTimeout`, `dropped`, `invalid` as terminal statuses (`RES:A1` §4.10).
9. Outcome (§3.7.5); nonce commit or release.

#### 3.7.2 Nonces

Per account: first use reads `system_accountNextIndex` (pool-aware, used by both Python and the core; `RES:A1` §4.8, §9), later uses increment locally so several transactions can pipeline; a failure between signing
and inclusion clears the entry ("a failed submit does not consume the nonce on-chain", `RES:A1` §4.8); an
`FateUnknown` submission keeps the nonce reserved until reconciliation resolves it. Hotkeys that ever signed have
`sufficients = 1` and never reset their nonce (`RES:C1` §7.4), so never assume zero.

#### 3.7.3 Fees

`state_call("TransactionPaymentApi_query_info", extrinsic || u32 LE len)` over an extrinsic signed with a
zeroed 64-byte signature and an immortal era; `partial_fee` is the estimate (`RES:A1` §4.4). The fee payer is
the signer, except hotkey calls in the coldkey-pays list (`PY:fee_filters.py:12-32` [V] mirrors
`SUB:runtime/src/fee_filters.rs`) and proxied calls with `RealPaysFee` (478 wrapper, `RES:C1` §7.1); the plan
reports the payer. `TaoBalance`/`AlphaBalance` are `u64`; rtao never uses subxt's `u128` fee helpers
(`RES:E` §4.1 item 9).

#### 3.7.4 Pool rejections and retries

JSON-RPC 1010 with `Custom error: N` maps through the `CustomTransactionError` table (codes 0-27 and 255,
`SUB:common/src/transaction_error.rs:4-73` per `RES:A1` §5.3; table reproduced in `RES:C1` §7.5) to a chain error
name, then to an `ErrorCode`. Standard pool errors are matched by code first (substrate author errors 1010-1016
[I: standard Substrate codes, not verified on disk]) and by the v11 text needles second (`RES:A1` §5.3). Retries
happen only for transient pool rejections ("priority is too low", "transaction is outdated", "stale") with a
fresh nonce, never for dispatch errors (`RES:A1` §4.7). A weight commit is never blindly resubmitted: the pool
allows one pending commit per `(hotkey, netuid_index)` (`RES:C1` §2.2 provides-tag).

#### 3.7.5 Outcome resolution

`chain_getBlock(h)` -> locate the extrinsic by blake2-256 of each raw extrinsic (never fabricate an outcome when
absent, `RES:A1` §4.10) -> decode `System.Events` at `h` -> keep `ApplyExtrinsic(idx)` -> success iff
`System.ExtrinsicSuccess` and no nested failure: `Sudo.Sudid`/`SudoAsDone`, `Proxy.ProxyExecuted`,
`Multisig.MultisigExecuted` with `Err`, `Utility.BatchInterrupted`/`ItemFailed`, or a MevShield failure event
(`RES:A1` §4.10) -> module errors resolved with `Runtime::module_error(pallet, error[0])`
(`CORE:runtime/mod.rs:340` [V]) -> fee from `TransactionPayment.TransactionFeePaid.actual_fee`.

```rust
pub struct TxReceipt {
    pub extrinsic_hash: H256, pub status: Inclusion,              // PoolAccepted | InBlock | Finalized
    pub block: Option<(u32, H256)>, pub index: Option<u32>,
    pub dispatch: Option<DispatchOutcome>,                        // Success | Failed(DispatchError)
    pub fee: Option<Rao>, pub events: Vec<RawEvent>,
}
```

#### 3.7.6 Reconciliation after a lost watch

On `FateUnknown` (socket drop, timeout, reconnect): scan finalized blocks from the submission height to
`birth + period` for the extrinsic hash (backfilled by number, so gaps are impossible). Found -> resolve
normally. Not found once the finalized head passes `birth + period` -> `Expired` (definitively not included),
nonce released. This generalises SN2's effect-polling reconciliation (`LastUpdate` poll,
`SN2:sn2-chain/src/weights.rs:104-180` [V]); for weight commits the scheduler additionally uses the `LastUpdate`
effect as a fast path. Short eras make this cheap, which is why the weight scheduler uses period 32 (§9.3).

### 3.8 Shielded submission (ML-KEM-768)

Port of `PY:executor.py` `_submit_encrypted_call` (`RES:A1` §6.5) on core primitives:

1. Read `MevShield.NextKey` (retry twice, one block apart).
2. `n = system_accountNextIndex(signer)`; sign the inner call at nonce `n+1` with era **8**.
3. `ciphertext = mlkem::seal(next_key, inner_bytes, include_key_hash = true)` (`CORE:mlkem/mod.rs:53` [V]);
   reject above the 8,192-byte bound (`RES:A1` §6.5).
4. Compose `MevShield.submit_encrypted { ciphertext }`, sign the carrier at nonce `n` with era **8**; the runtime rejects longer eras as `Stale` (`SUB:runtime/src/check_mortality.rs:19,94-100` [V]).
5. Resolve the inner extrinsic by hash in blocks `[carrier_inclusion, inclusion + 8]`, re-checking after
   finality; if it never lands the result is a failure ("not included before its era expired").
6. Policy: with `max_fee` set and no estimate, shielding is a hard `PolicyError`; the carrier fee is estimated by
   pricing an 8,192-byte ciphertext and blocked if free TAO cannot cover reserve + carrier (`RES:A1` §6.5).
   Shielded carriers cannot pay fees in alpha (`RES:A1` §10 item 13).

`mev_shield_required` intents (`burned_register`, `add_collateral`) always take this path; the CLI applies
`mev_shield_default` for stake-trading intents (precedence: required -> flag -> config -> intent default,
`RES:A1` §6.5); library `execute` does not auto-shield otherwise (parity, `RES:A1` §10 item 12).

### 3.9 Composing proxy, sudo, multisig and batch

`CallBytes` nest by splicing (`Value::Bytes`), so every wrapper is a `compose_call` on the core:

```rust
pub mod compose {
    pub fn sudo(rt: &RuntimeHandle, inner: CallBytes) -> Result<CallBytes>;                 // not double-wrapped
    pub fn proxy(rt: &RuntimeHandle, real: AccountId, force: Option<ProxyType>, inner: CallBytes) -> Result<CallBytes>;
    pub fn batch_all(rt: &RuntimeHandle, calls: Vec<CallBytes>) -> Result<CallBytes>;         // batch/force_batch: raw only
    pub async fn as_multi(s: &Snapshot, signer: AccountId, others: Vec<AccountId>, threshold: u16,
                          inner: CallBytes) -> Result<CallBytes>; // max_weight from query_info; timepoint from Multisig.Multisigs
    pub fn as_multi_threshold_1(rt: &RuntimeHandle, others: Vec<AccountId>, inner: CallBytes) -> Result<CallBytes>;
}
```

Order is fixed: semantic call -> `Sudo.sudo` (root-origin intents only) -> `Proxy.proxy` -> multisig adapter
(`PY:executor.py:L175-219` per `RES:A1` §4.2). Multisig accounts derive with
`multisig_account_id` (`CORE:codec/extrinsic.rs:419` [V]); `other_signatories` are sorted by raw key bytes and
exclude the signer. Proxy-type capabilities are runtime-defined and deny-by-default (`RES:G` §1.3 item 10), so a
plan through a proxy consults `ProxyFilterRuntimeApi.get_proxy_filters` (`RES:C2` §1.7) and warns when the call
is outside the proxy type's filter [I: exact filter semantics to be confirmed against the runtime].

### 3.10 Waiting helpers and the epoch clock

```rust
impl Subtensor {
    pub async fn wait_for_block(&self, n: u32, finalized: bool) -> Result<Snapshot>;
    pub async fn wait_for_timestamp(&self, t: SystemTime) -> Result<Snapshot>;
    pub async fn wait_for_epoch(&self, netuid: NetUid) -> Result<EpochEvent>;   // LastEpochBlock advances (PY:client.py:418-444 per RES:A2)
    pub async fn wait_for_event<E: StaticEvent>(&self, filter: impl Fn(&E) -> bool, until: BlockDeadline) -> Result<(Snapshot, E)>;
    pub fn finalized_heads(&self) -> impl Stream<Item = Snapshot>;              // gap-free
    pub fn best_heads(&self) -> impl Stream<Item = Snapshot>;
    pub fn epoch_clock(&self) -> EpochClock;
}
pub struct EpochStatus {
    pub netuid: NetUid, pub block: u32, pub tempo: u16, pub epoch_index: u64, pub last_epoch_block: u64,
    pub blocks_since_last_step: u64, pub pending_epoch_at: Option<u64>, pub next_epoch_start_block: Option<u64>,
    pub blocks_remaining: Option<u64>, pub seconds_remaining: Option<f64>,
    pub admin_freeze_window: u64, pub admin_window_open: bool,                  // owner tooling (RES:C2 §4.2)
}
```

`EpochClock::status` reads `Tempo`, `LastEpochBlock`, `BlocksSinceLastStep`, `PendingEpochAt`,
`SubnetEpochIndex`, `AdminFreezeWindow` and `get_next_epoch_start_block` at one snapshot (`RES:A2` §3.4; the
modular legacy formula is never used, `RES:C2` §0 item 6). `EpochClock::predict_reveal_block` uses the
`rtao_btcore::epoch` simulator (§1.6). Block time comes from `Aura.SlotDuration` (12 s on finney, 0.25 s on
fast localnets) and is never hard-coded (`RES:C2` §4.4).

---

## 4. Intents and reads parity (`rtao-tx`, `rtao-intents`, `rtao-reads`)

### 4.1 The upstream surface (counted [V] in `CAT:`)

- `intents.json`: 83 intents; signer coldkey 75 / hotkey 8 (`associate_evm_key, commit_weights, reset_axon,
  reveal_weights, serve_axon, serve_axon_tls, serve_prometheus, set_weights`); origin signed 74 /
  subnet_owner 7 / root 2; 139 wrapped `(pallet, call)` pairs.
- `reads.json`: 102 reads in 14 categories (Accounts & keys 6, Chain 6, Delegation 9, Epochs & timing 5,
  Hyperparameters 7, Identity & commitments 5, Leases & crowdloans 5, Locks & conviction 7, Mining & collateral 3,
  Neurons & registration 6, Prices & swaps 4, Staking 22, Subnets 14, Weights & bonds 3).
- `errors.json`: 18 semantic codes, 390 chain error names.
- The Python registry builds a JSON schema per intent at import (`RES:A1` §2.3) and the CLI generates one
  command per intent/read from the registries (`PY:cli/tx.py:1-11`, `PY:cli/query.py:1-6` [V]).

### 4.2 Intent representation

```rust
pub enum SignerRole { Coldkey, Hotkey }
pub enum Origin { Signed, SubnetOwner, Root }            // Root => executor wraps Sudo.sudo
pub enum ShieldPolicy { Off, Default, Required }         // mev_shield_default / mev_shield_required
pub enum Spend { None, Bounded(Rao), Unbounded }         // "TAO leaving the signer" (RES:A1 §2.2, §3.14)
pub enum Touches { None, Netuids(SmallVec<[NetUid; 2]>), All }

pub struct IntentMeta {
    pub op: &'static str,                 // "set_weights"; CLI `rtao tx set-weights`; docs /tx/set-weights
    pub class: &'static str,              // "SetWeights" (= catalog python_class)
    pub signer: SignerRole, pub origin: Origin, pub shield: ShieldPolicy,
    pub verify: Option<&'static str>,     // read confirming a root-origin effect (required when origin = Root)
    pub wraps: &'static [(&'static str, &'static str)],
    pub all_amount_fields: &'static [&'static str],
    pub group: &'static str,              // CLI help panel / docs index
}

pub trait Intent: Send + Sync + Debug + Serialize + DeserializeOwned + JsonSchema + 'static {
    const META: IntentMeta;
    fn summary(&self) -> String;
    fn spend(&self) -> Spend { Spend::None }
    fn touches(&self) -> Touches;
    fn build(&self, cx: &BuildCx<'_>) -> impl Future<Output = Result<Built, TxError>> + Send;
    fn preflight(&self, cx: &PreflightCx<'_>) -> impl Future<Output = Result<Preflight, TxError>> + Send {
        default_preflight(self, cx)       // effects = [summary]
    }
}
pub struct BuildCx<'a> { pub at: &'a Snapshot, pub origin: AccountView, pub signer: AccountId }
pub struct Built { pub call: CallBytes, pub extras: BTreeMap<String, serde_json::Value> }   // e.g. reveal_round
pub struct Preflight {
    pub effects: Vec<String>, pub warnings: Vec<String>, pub blocks: Vec<String>,
    pub required_free: Option<Rao>, pub available_free: Option<Rao>, pub estimated_fee: Option<Rao>,
    pub facts: Vec<(String, String)>, pub facts_title: FactsTitle,           // RES:A1 §2.1 IntentPreflight
}
/// Object-safe twin used by the registry, `execute_tool` and the CLI (boxed futures, JSON args).
pub trait DynIntent: Send + Sync { fn meta(&self) -> &'static IntentMeta; /* … */ }
```

Definitions use one derive; the macro emits `META`, a schemars schema whose property names, `required` list and
money encoding (`anyOf [number, decimal string, "all"]`, `RES:I` §5.3) match the catalog's `input_schema`, a
clap `Args` struct under the `cli` feature, and the `DynIntent` glue:

```rust
/// Set validator weights for one subnet mechanism (plain or timelocked by the subnet's commit-reveal setting).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, rtao::Intent)]
#[intent(op = "set_weights", class = "SetWeights", signer = Hotkey, origin = Signed,
         wraps(("SubtensorModule", "set_mechanism_weights"), ("SubtensorModule", "commit_timelocked_mechanism_weights")))]
pub struct SetWeights {
    pub netuid: NetUid,
    #[serde(default)] pub uids: Option<Vec<u16>>,
    #[serde(default)] pub weights: Option<WeightsArg>,      // map uid->w or list parallel to uids
    #[serde(default)] pub mechid: MechId,
    #[serde(default)] pub version_key: u64,                 // v11 default 0 (RES:G §2.6 item 21)
    #[serde(default)] pub raw_u16: bool,                    // Null consensus only
}
```

The 83 intents are grouped by module exactly as the Python files are (`staking`, `weights`, `registration`,
`transfer`/`evm`, `children`, `coldkey`, `collateral`/`lock`, `governance`/`hyperparameters`/`identity`/`root`,
`serving`, `proxy`, `multisig`, `crowdloan`/`leasing`, `association`, `batch`; `RES:A1` §3), so a diff of
`PY:intents/<group>.py` maps to one Rust module. Per-intent behaviours that matter for parity are reproduced
literally, including: slippage limits `price * (1 +/- tol)` with `allow_partial=false` and default tolerance 5%;
`"all"` = free - existential deposit - 500,000 rao; the claim batches on netuid 0; `move_swap_stake` swapping
`amount - 1 rao`; `set_take` choosing increase/decrease from `Delegates`; `alpha_low`/`alpha_high` sharing one
setter; fixed128 values encoded as `{bits}`; `register_subnet` waiting for the matching `NetworkAdded`
(`RES:A1` §3.1-3.13, §4.13, §10). The root-claim fee model (`PY:intents/_root_claim_fee.py`, 771 lines) is a preview only (the chain charges what it charges, `RES:A1` §8.2) and is ported last within M5.

Intents and reads are hand-written Rust; only their skeletons are generated. The catalogs describe parameters, signer,
origin and wrapped calls, but the behaviour (preflight reads, `all` resolution, limit math, completion waits)
exists only in the Python source, which stays the specification. `cargo xtask scaffold intent <op>` and
`cargo xtask scaffold read <name>` generate the starting module from the catalog entry (fields and docs from
`input_schema`/`params`, `wraps`, signer, origin, category, targets from `hits`), and the parity tests of §4.7 keep
the hand-written result equal to the catalog.

### 4.3 Registry and tool manifest

`rtao_intents::REGISTRY: &[&dyn DynIntent]` is an explicit list (no linker tricks), so completeness is a plain
test. `Executor::tools()` emits the catalog schema verbatim (`name, summary, description, signer, origin,
verify, wraps, docs_url, input_schema`) plus a `rust` object
(`{"crate":"rtao-intents","type":"rtao::intents::weights::SetWeights","cli":"rtao tx set-weights …"}`, the
extension proposed in `RES:I` §5.6), so agents written against bittensor.com's catalog work unchanged.
`Executor::execute_tool(op, json, wallet)` is name dispatch for agents (`SUB:docs/agents.mdx` per `RES:I` §5.5).

### 4.4 Plan / execute / policy / results

`rtao_tx::Executor` reproduces `PY:executor.py` (1,319 lines) step for step (`RES:A1` §4):

1. Normalise: address arguments are already typed `AccountId`s; the CLI and `execute_tool` resolve names
   (wallet, `wallet/hotkey`, address book, proxy book, saved multisig) through an `AddressResolver`.
2. Compose: `semantic.build(origin view)` -> `Sudo.sudo` if `Origin::Root` -> `Proxy.proxy` if `proxy_for` ->
   multisig adapter (§3.9). With `proxy_for`, the build runs against the proxied account's view.
3. Accounts: `fee_payer` = public key of the role's signer (never unlocks); `dispatch_origin` = `proxy_for` or
   the origin view (`RES:A1` §4.3).
4. Preflight: `intent.preflight(dispatch_origin, fee_payer, &call)`; the coldkey-pays-fee warning when a
   hotkey signs a listed call (`PY:fee_filters.py` [V]).
5. Fee: the intent's exact estimate if supplied, else `query_info`; failure is a warning in `plan`.
6. Policy: `Policy::check(semantic, fee)` + intent `blocks` -> `violations`.
7. `plan` returns `Plan`; `execute` re-plans, refuses on violations, resolves the signer for the role, submits
   **exactly** `plan.call` (`RES:A1` §4.6), retries only transient pool rejections, post-processes
   (`register_subnet` completion with progress callbacks `queued -> waiting -> registered`, `create_pure_proxy`
   data from `Proxy.PureCreated`, build extras).

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Policy {                         // PY:intents/plan.py:21-85 (RES:A1 §4.5)
    pub max_fee: Option<Rao>,               // set + no estimate => violation ("must not fail open")
    pub max_spend: Option<Rao>,             // Unbounded spend => violation; Bounded above cap => violation
    pub allowed_netuids: Option<BTreeSet<NetUid>>,  // Touches::All => violation; outside set => violation
    pub allow_raw_calls: bool,              // submit_call / submit_signature refused unless true
}
pub struct ExecOpts {
    pub proxy_for: Option<AccountId>, pub force_proxy_type: Option<ProxyType>, pub multisig: Option<MultisigAdapter>,
    pub mortality: Mortality /* 128 */, pub wait: WaitFor /* Finalized */, pub retries: u8 /* 0 */,
    pub shield: Option<bool>, pub wait_for_registration: bool /* true */, pub policy: Option<Policy>,
}
impl Executor {
    pub async fn plan(&self, i: &dyn DynIntent, w: &WalletSigners, o: &ExecOpts) -> Result<Plan, TxError>;
    pub async fn execute(&self, i: &dyn DynIntent, w: &WalletSigners, o: &ExecOpts) -> Result<ExtrinsicResult, TxError>;
    pub async fn submit_shielded(&self, i: &dyn DynIntent, w: &WalletSigners, o: &ExecOpts) -> Result<ExtrinsicResult, TxError>;
    pub async fn submit_call(&self, call: CallBytes, s: Arc<dyn Signer>, o: &ExecOpts) -> Result<ExtrinsicResult, TxError>;
    pub async fn prepare_call(&self, call: CallBytes, address: AccountId, o: &ExecOpts) -> Result<UnsignedExtrinsic, TxError>;
    pub async fn submit_signature(&self, u: UnsignedExtrinsic, sig: Signature, o: &ExecOpts) -> Result<ExtrinsicResult, TxError>;
    pub fn tools(&self) -> Vec<ToolSpec>;
}
```

`UnsignedExtrinsic` serialises to the same JSON as Python's `to_dict()/from_dict()` (`RES:A1` §4.12), so a call
prepared by Python can be signed by an rtao signer and vice versa; `submit_signature` refuses when
`spec_version` or `transaction_version` changed since preparation.

`ExtrinsicResult` keeps the v11 fields and `to_json()` keys (`success, message, fee_tao, block_hash,
extrinsic_id = "{block}-{idx:04}", events, explorer_url, data, error`; `RES:A1` §4.10) and adds
`status: Inclusion` (pool-accepted vs included vs finalized) so "success" is never ambiguous.
`raise_for_failure()` returns the embedded `ChainError`.

### 4.5 Error taxonomy

```rust
pub enum ErrorCode { InsufficientBalance, InsufficientLiquidity, RateLimited, NotRegistered, NotAuthorized,
    AlreadyExists, NotFound, SubnetNotExists, SubtokenDisabled, Disabled, TooEarly, Expired, LimitExceeded,
    UnitMismatch, InvalidArgument, PolicyViolation, Internal, Unknown }          // PY:error_map.py:23-41 (RES:A1 §5.1)
pub struct ChainError { pub message: String, pub name: Option<String>, pub pallet: Option<String>,
    pub code: ErrorCode, pub description: Option<&'static str>, pub remediation: &'static str, pub docs_url: String }
pub enum TxError { Policy(Vec<String>), Chain(ChainError), Rpc(RpcError), FateUnknown { hash: H256, until_block: u32 },
    Signer(SignerError), SignerUnavailable(SignerRole), SurfaceChanged { pallet: String, call: String, detail: String },
    Unit(UnitMismatch), InvalidArgument(String), Codec(String) }
```

| Source on the wire | Resolution | Table (generated or vendored) |
|---|---|---|
| `Module { index, error }` in `ExtrinsicFailed` or nested wrapper events | `Runtime::module_error` -> name -> `NAME_TO_CODE` | `NAME_TO_CODE` (390 names) and descriptions generated from `parity/upstream/errors.json` |
| non-module `DispatchError` (`BadOrigin`, `Token(..)`, `Arithmetic(..)`, …) | variant name -> `DISPATCH_ERRORS` | vendored from `PY:error_map.py:555-795` (`RES:A1` §5.2); CI regex-checks the key set |
| pool `Custom(N)` (RPC 1010) | `CUSTOM_TRANSACTION_ERRORS[N]` -> name -> code | 0-27 and 255 from `SUB:common/src/transaction_error.rs`; CI asserts the discriminant set equals `range(28)` plus `{255}` like `check.py --names` (`RES:A1` §5.5) |
| standard `InvalidTransaction`/`UnknownTransaction` | code, then text needles (`"inability to pay some fees"` -> Payment, …) | `INVALID_TRANSACTION_ERRORS` (`RES:A1` §5.3) |
| watch fatals (`usurped`, `retracted`, `finalityTimeout`, `dropped`, `invalid`) | name -> code | fixed |
| MevShield inclusion events | name -> code | fixed |
| no name | substring fallback (`PY:result.py:291-304`) | fixed |

Precedence equals `classify_error` (`PY:result.py:L483-498`: name table -> dispatch table -> `*RateLimit*` -> message
substrings -> `Unknown`). Remediation prose per code and docs URLs come from `errors.json` (`codes[*]`).
`rtao explain <code|Name>` prints them (§10).

### 4.6 Read representation

```rust
/// Epoch timing for a subnet at one block.
#[rtao::read(name = "epoch_status", category = EpochsTiming,
    targets(storage("SubtensorModule", "Tempo"), storage("SubtensorModule", "LastEpochBlock"),
            storage("SubtensorModule", "BlocksSinceLastStep"), storage("SubtensorModule", "PendingEpochAt"),
            storage("SubtensorModule", "SubnetEpochIndex"),
            runtime_api("SubnetInfoRuntimeApi", "get_next_epoch_start_block"), constant("Aura", "SlotDuration")))]
pub async fn epoch_status(at: &Snapshot, netuid: NetUid) -> Result<EpochStatus, ReadError> { … }
```

The macro emits a `ReadSpec` (name, summary, description, param schema from the argument types, param docs,
category, **declared** chain targets) into `rtao_reads::REGISTRY`, a JSON dispatcher (`client.read(name, json)`
for agents and the CLI), and a typed method on the category namespace:

```rust
let st = client.epochs().epoch_status(NetUid(1)).await?;              // pinned to client.head()
let mg = client.at(BlockRef::Number(4_200_000)).await?.subnets().metagraph(NetUid(1)).await?;
```

The 14 namespaces mirror `PY:namespaces.py` (`balances, chain, delegation, epochs, hyperparameters, identity,
leasing, locks, collateral, neurons, prices, staking, subnets, weights`; `RES:A2` §1.1) including the curated
helpers (`balances().get_many`, `staking().positions`, `subnets().metagraph(commitments)`, `neurons().all(lite)`).
Declared targets make the catalog's `hits` exact instead of regex-scraped (`RES:A2` §5.6, `RES:I` §5.4), and the
surface verifier checks them. Every read returns a typed struct that also implements `Serialize`; reads with real
domain math (`subnet_convictions`' decay projection and bisection, `RES:A2` §3.8) use the pallet's fixed-point
arithmetic rather than the Python float mirror. Paged runtime APIs (`get_all_beta_pricing`) loop until `next` is
`None`.

### 4.7 Parity tracking enforced in CI

`parity/manifest.toml` holds one row per upstream name the tests below did not find implemented:

```toml
[intents.claim_root]
status = "planned"            # implemented | planned | omitted | rtao-only
milestone = "M5"
[reads.root_basket_portfolio]
status = "planned"
milestone = "M4"
[python_modules."basket_index.py"]
status = "omitted"
reason = "display fallback for pre-v3 BetaBasket nodes; finney exposes BetaBasketRuntimeApi v5 (RES:A2 §3.12, RES:C2 §1.0)"
```

Tests (fast, offline, in the PR gate):

| Test | Asserts |
|---|---|
| `every_upstream_intent_is_accounted_for` | each name in `parity/upstream/intents.json` is in `rtao_intents::REGISTRY` with equal `signer`, `origin`, `verify`, `wraps` (as sets) and schema property names/`required`, or has a manifest row; reverse direction marks rtao-only intents |
| `every_upstream_read_is_accounted_for` | name, param names/types, category |
| `error_tables_match_catalog` | 18 codes and 390 `chain_errors` names/codes equal the generated tables |
| `every_covered_call_has_a_decision` | each call of the covered pallets in the pinned metadata is wrapped by some intent or listed in `parity/raw_only.toml`; this is the `check.py --coverage` forcing function (`RES:A1` §5.5) |
| `root_intents_declare_verify` | every `Origin::Root` intent names an existing read |
| `cli_tree_matches` | `parity/upstream/cli-tree.json` (dumped from `btcli` by the interop lane): every command exists in rtao or has a manifest row |
| `settings_match` | `btcli.json` keys (`PY:config.py` `SETTABLE` [V]) and `BT_*` env names are honoured or listed |
| `samples_cover_every_read` | the `READ_SAMPLES` port (`RES:A2` §6.3) has params for every read; each read runs at head and pinned on `FakeChain` |

The nightly catalog job (§12.2) refreshes `parity/upstream/` from bittensor.com and from regex scans of the
monorepo's `testnet`/`main` branches (`@register` classes, `@read(` names), and opens a PR in which every new
upstream name appears with `status = "planned"`. The PR cannot merge without a triage commit, so a Python addition breaks no build and still forces a decision.

### 4.8 Deliberate divergences from v11 (each documented in the migration guide)

1. Reads are always pinned to one block (§3.1).
2. Constants come from the snapshot's runtime, not the head runtime.
3. `Owner`/`Keys`/`SubnetOwnerHotkey` misses are `None`, not the zero account (`RES:A2` §9 item 9).
4. `ExtrinsicResult.status` distinguishes pool acceptance from inclusion.
5. `SetWeights` preflight also checks the stored `WeightsVersionKey`, stake threshold and permit, which v11 leaves
   to the chain (`RES:G` §2.6 item 18); the same codes are returned, with remediation.
6. `total_stake` is typed `StakeWeight`, not TAO (`RES:A2` §4.1).
7. Deprecated always-zero fields (`rank`, `trust`, `pruning_score`, `subnet_emission`, `pending_root_emission`,
   `tao_dividends_per_hotkey`, …; `RES:C2` §0 item 4) are `#[deprecated]` on the typed models and omitted from
   rtao's own rendering, while the dynamic JSON still contains them.

---

## 5. Money and units (`rtao-primitives`)

```rust
// Every newtype below carries the same derives (shown once):
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, EncodeAsType, DecodeAsType)]
pub struct Rao(pub u64);                      // TaoBalance (SCALE: bare u64)
pub struct AlphaRao(pub u64);                 // AlphaBalance: raw alpha base units of *some* subnet
pub struct NetUid(pub u16);  pub struct MechId(pub u8);  pub struct Uid(pub u16);
pub struct NetUidStorageIndex(pub u16);       // mechid * 4096 + netuid (RES:C1 §2.1; RES:A2 §2)
impl NetUidStorageIndex { pub const STRIDE: u16 = 4096; pub fn new(n: NetUid, m: MechId) -> Self; pub fn split(self) -> (NetUid, MechId); }
pub struct PerU16(pub u16);                   // fraction = raw / 65535
pub struct ProportionU64(pub u64);            // child-key proportions, u64::MAX = 100%
pub struct StakeWeight(pub u64);              // alpha + tao * TaoWeight, alpha-equivalent
pub struct U64F64 { pub bits: u128 }  pub struct I96F32 { pub bits: i128 }   // substrate_fixed layouts:
pub struct I64F64 { pub bits: i128 }  pub struct I32F32 { pub bits: i64 }    //   value = bits / 2^frac

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Unit { Tao, Alpha(NetUid) }         // Unit::of(NetUid(0)) == Tao (both SDKs tag netuid 0 as TAO)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Balance { rao: u64, unit: Unit }
impl Balance {
    pub const fn tao(r: Rao) -> Self;                pub const fn alpha(n: NetUid, a: AlphaRao) -> Self;
    pub fn parse_tao(s: &str) -> Result<Self, AmountError>;              // exact decimal, <= 9 places
    pub fn parse_alpha(n: NetUid, s: &str) -> Result<Self, AmountError>;
    pub fn checked_add(self, o: Self) -> Result<Self, UnitMismatch>;     // same unit only; also sub/mul_ratio
    pub fn as_tao(self) -> Result<Rao, UnitMismatch>;                    // alpha -> error (v11 `.tao` raises)
    pub fn as_alpha(self, n: NetUid) -> Result<AlphaRao, UnitMismatch>;
    pub fn to_decimal_string(self) -> String;                            // exact
    pub fn to_f64_lossy(self) -> f64;                                    // the only float exit, explicitly named
}
```

Rules, all carried over from v11 (`RES:I` §2.6, `RES:A2` §2):

- Amounts are integers in rao everywhere on the wire; 1 TAO = 10^9 rao; existential deposit 500 rao; minimum
  stake 2,000,000 rao (`RES:G` §4.6). No float constructor exists in the default API (v10 truncated with
  `int(balance * 1e9)`, `RES:I` §2.6).
- Intent money fields are unit-named (`amount_tao`, `amount_alpha`, `limit_price_rao`) and typed
  `MoneyArg = Amount(Decimal string | number) | All`; `All` is resolved at build time from chain state.
  Each money parameter's unit is checked against the call argument's runtime type identity
  (`TaoBalance`/`AlphaBalance`) from the generated `ARGS` table (the Rust equivalent of `call_amount`, `RES:A1` §2.2), so an alpha amount can never be passed where TAO is expected.
- Fixed-point newtypes decode from the runtime's `{bits}` composite by field name, independent of the typenum
  generic (`RES:F` §4); they render exactly (`Display` with full precision) and offer `to_f64()` for display
  math only. Consensus-relevant math (weights) is u16-based anyway.
- Display uses the chain-registered token symbol (`TokenSymbol`, 7-day cache) like v11, with `τ` for TAO.
- Hyperparameter units: `HyperparamKind { U16Fraction, U64Fraction, PerMillion, Rao, Blocks, Epochs, Difficulty,
  Fixed128, Int, Bool }` and `HYPERPARAMS: &[HyperparamSpec { name, kind, doc, short, signed, bits, min, max,
  setter: Option<&str>, owner_settable, root_only }]`, data vendored from `PY:hyperparams.py` (`Hyperparam` at
  `:91-107` [V], 50 entries per `RES:A2` §3.5) and the owner setter map from `PY:intents/hyperparameters.py:31-66`
  (`RES:A1` §3.8). Input rule: a plain integer is the raw value, a value with a decimal point is the human form
  (`RES:G` §1.5 item 15). Two CI checks keep it honest: name/kind/bounds equal the Python tables at the pinned tag
  (regex extraction), and every kind agrees with the storage value-type identity in the pinned metadata (the
  `check.py --units` gate, `RES:A2` §5.4).

---

## 6. Wallets and signing (`rtao-wallet`, `rtao-signer`)

### 6.1 Layout, loading, creation

```rust
pub struct WalletRef { pub name: String, pub hotkey: String, pub path: PathBuf }
// "cold" | "cold/hot"; defaults default/default and ~/.bittensor/wallets; env BT_WALLET, BT_WALLET_HOTKEY, BT_WALLET_PATH
pub struct Wallet { /* WalletRef + unlock cache */ }
impl Wallet {
    pub fn open(r: WalletRef) -> Self;
    pub fn coldkeypub(&self) -> Result<AccountId, WalletError>;            // coldkeypub.txt; never unlocks
    pub fn hotkey(&self) -> Result<KeypairSigner, WalletError>;            // hotkeys/<hk> (plaintext JSON)
    pub async fn coldkey(&self, pw: &PasswordSources) -> Result<KeypairSigner, WalletError>;  // spawn_blocking
    pub fn create_coldkey(&self, words: u8, password: Option<&SecretString>, ow: Overwrite) -> Result<Mnemonic, WalletError>;
    pub fn create_hotkey(&self, words: u8, ow: Overwrite) -> Result<Mnemonic, WalletError>;
    pub fn regen_coldkey(&self, src: KeySource, password: Option<&SecretString>, ow: Overwrite) -> Result<(), WalletError>;
    // regen_hotkey, regen_coldkeypub, regen_hotkeypub, inspect, sign/verify/encrypt/decrypt message
}
pub fn list_wallets(path: &Path) -> Result<Vec<WalletListing>, WalletError>;
```

- Layout `~/.bittensor/wallets/<name>/{coldkey, coldkeypub.txt, hotkeys/<hk>, hotkeys/<hk>pub.txt}`
  (`RES:G` §4.3).
- Parsing uses `deserialize_keypair_from_keyfile_data`, which already handles every on-disk quirk: the legacy
  `" ss58Address"` key, `cryptoType` as number or string, derivation order, cross-check with retry under the
  other crypto type, bare hex/mnemonic files, and an error pointing polkadot-js exports at `btcli wallet regen-coldkey --json-path` (`RES:B` §3.3).
  Encrypted formats ($NACL, Ansible Vault, legacy Fernet) decrypt through `decrypt_keyfile_data`.
- Writing: `serialized_keypair_to_keyfile_data` + `encrypt_keyfile_data` (writes $NACL); files 0o600 via
  temp-file + rename, directories 0o700; never overwrite without `Overwrite::Yes`; never prompt when stdin is not
  a TTY (`RES:G` §4.3). Mnemonics are `secrecy::SecretString`; the core zeroizes retained material
  (`RES:B` §3.2).
- The NaCl KDF costs 512 MiB and eight Argon2i passes (`RES:F` §3.3): unlock once per process, cache the
  `Keypair` (not the password).

### 6.2 Password resolution order (exactly `PY:wallets.py:69-77` [V])

1. explicit password (API / tests);
2. `BT_WALLET_PASSWORD`;
3. per-keyfile `BT_PW_<path with separators and dots replaced by _, uppercased>` (`PY:keyfiles.py:102-104` [V]),
   whose value is base64(XOR(password, var name)) decoded by the core's `get_password_from_environment`
   (`CORE:keyfiles/mod.rs:111-124` [V]);
4. `--wallet-password-file` / `BT_WALLET_PASSWORD_FILE` (one line, trimmed);
5. macOS Keychain, only with `--keychain-password` (service `bittensor.coldkey`, account `<wallet>`, so entries
   saved by `btcli wallet keychain save` are found; `RES:G` §4.3), behind feature `keychain-macos`;
6. native macOS password dialog, only with `--macos-password`;
7. interactive prompt, only on a TTY (the CLI re-prompts twice on a wrong password when it prompted).

`keyring` (Linux Secret Service) is an rtao-only extra source between 4 and 5, off by default. Neuron runtimes
never run this resolution: they do not load coldkeys (§6.5).

### 6.3 The `Signer` contract

```rust
pub trait Signer: Send + Sync + 'static {
    fn account(&self) -> AccountId;
    fn scheme(&self) -> CryptoScheme;        // Ed25519 = 0, Sr25519 = 1; also the MultiSignature variant byte
    fn caps(&self) -> SignerCaps;
    fn sign_extrinsic<'a>(&'a self, req: &'a SigningRequest<'a>) -> BoxFuture<'a, Result<Signature, SignerError>>;
    fn sign_raw<'a>(&'a self, _msg: &'a [u8]) -> BoxFuture<'a, Result<Signature, SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })      // Ledger/Vault refuse blind signing
    }
}
pub struct SignerCaps { pub raw_bytes: bool, pub requires_metadata_hash: bool, pub interactive: bool, pub two_stage: bool }
pub struct SigningRequest<'a> {
    pub call: &'a [u8], pub extra: &'a [u8], pub additional: &'a [u8], pub payload: &'a [u8],  // payload hashed if > 256 B
    pub json: &'a SignerPayloadJson,     // polkadot-js SignerPayloadJSON: BE hex at SCALE width (RES:A1 §6.4)
    pub ctx: SigningContext<'a>,
}
pub struct SigningContext<'a> { pub metadata_v15: &'a [u8], pub chain_info: &'a ChainInfo, pub genesis: H256,
                                pub spec_version: u32, pub transaction_version: u32, pub ss58_format: u16 }
pub enum Signature { Ed25519([u8; 64]), Sr25519([u8; 64]) }
```

The trait is object-safe (`Arc<dyn Signer>` flows through executors and the btauth client) and async because
hardware, browser and remote signers are. When `caps().requires_metadata_hash`, the transaction pipeline computes
`metadata_digest(v15, ChainInfo { spec, "node-subtensor", 42, 9, "TAO" })` and signs with
`CheckMetadataHash = Enabled` (`RES:A1` §8.1 item 2; `RES:H` §1.5 digest vector).

### 6.4 Implementations

| Signer | Feature | Mechanics | Evidence |
|---|---|---|---|
| `KeypairSigner` | always | core `Keypair::sign` (sp-core sr25519 `substrate` context, ed25519 RFC 8032); from wallet, URI, mnemonic, seed | `CORE:keys/mod.rs:391-399` |
| `LedgerSigner` | `ledger` | core `LedgerDevice` on `spawn_blocking`; path `m/44'/354'/account'/0'/index'`; proof from `generate_extrinsic_proof`; clear-signing only; Polkadot generic app >= 100.0.5 | `CORE:signers/ledger.rs:52-156`; `RES:B` §3.8; `RES:G` §4.4 |
| `VaultSigner` | `vault` | UOS payload frames (port of `PY:vault/uos.py`), one-shot loopback page server reusing the Python static assets (Apache-2.0) for camera scanning; shielded submissions need two scans inside the 8-block era | `PY:vault/server.py:1-15` [V]; `RES:G` §4.4 |
| `ExtensionSigner` | `extension` | the v11 bridge protocol on `127.0.0.1:39295` with `~/.bittensor/extension_bridge.token`, so the same browser page works; signs `SignerPayloadJson` | `RES:G` §4.4; `PY:extension/bridge.py` |
| `RemoteSigner` | `remote` | rtao addition: `POST /sign {payload_json, payload_hex, call_hex, account}` -> `{signature}`, request authenticated with btauth/1 by a separate client key; lets coldkeys live in an HSM/KMS service | [I] |

Offline signing for any external signer also works through `prepare_call`/`submit_signature` (§4.4).

### 6.5 Signer role per intent and key hygiene

`IntentMeta.signer` picks the key; `WalletSigners { coldkey: Option<Arc<dyn Signer>>, hotkey: Option<Arc<dyn
Signer>> }` supplies them; a missing role is `TxError::SignerUnavailable`. Neuron runtimes construct
`WalletSigners { hotkey: Some(..), coldkey: None }` ("hotkey on the box, coldkey off it", `SUB:docs/guides/validating.mdx` per `RES:G` §2.9), so every coldkey operation (registration, staking, owner
calls) runs from the CLI or owner tooling, ideally through an `Owner` or `Staking` proxy (`RES:G` §1.3). The
eight hotkey intents are fee-free (`Pays::No`) or charge the owning coldkey (`RES:C1` §0 item 4), so a hotkey with
zero balance can run a validator.

---

## 7. Neuron networking (`rtao-auth`, `rtao-http`, optional compat crates)

v11 removed the Axon/Dendrite/Synapse stack; identity is btauth/1 signed HTTP, blacklisting is a metagraph
lookup, priority is framework rate limiting keyed by the verified hotkey (`RES:G` §6 item 1). rtao's networking
layer is therefore the *replacement* for v10's stack, built on axum/tower and reqwest.

### 7.1 `rtao-auth`: btauth/1, byte-exact and I/O-free

Wire contract (normative `SUB:docs/guides/signed-requests.mdx:35-102`, reference `PY:http_auth.py:58-343`;
summarised in `RES:D` §1):

- Headers: `X-Bittensor-Version: 1`, `X-Bittensor-Hotkey` (ss58), optional `X-Bittensor-Receiver`,
  `X-Bittensor-Nonce` (unix **nanoseconds**, decimal), `X-Bittensor-Crypto: ed25519` (omitted for sr25519),
  `X-Bittensor-Signature: 0x<lowercase hex of 64 bytes>`. `X-Bittensor-Drand-*` are reserved: never emitted or
  interpreted.
- Signed payload: UTF-8 of eight lines joined by `\n`, no trailing newline:
  `btauth/1`, scheme, uppercase method, request target in percent-encoded wire form (path + `?query` exactly as
  sent), `sha256(raw body)` lowercase hex, nonce, sender ss58, receiver ss58 or `-`.

```rust
pub fn payload(scheme: Scheme, method: &str, target: &str, body: &[u8], nonce_ns: u64,
               sender: &str, receiver: Option<&str>) -> Vec<u8>;
pub async fn sign(signer: &dyn Signer, method: &Method, target: &str, body: &[u8],
                  receiver: Option<&AccountId>, now_ns: u64) -> Result<AuthHeaders, AuthError>;   // fresh nonce per attempt
pub struct VerifyConfig {
    pub max_age: Duration,                  // 10 s
    pub allowed_skew: Duration,             // 2 s
    pub require_receiver: bool,             // true
    pub accept_polkadotjs_wrapped: bool,    // true = Python parity (see below)
}
pub struct Verifier<S: NonceStore> { /* cfg, own hotkey, store */ }
impl<S: NonceStore> Verifier<S> {
    pub fn new(own: AccountId, cfg: VerifyConfig, store: S) -> Result<Self, ConfigError>;  // store.retention() >= max_age + skew, else error
    pub async fn verify(&self, h: &HeaderMap, method: &Method, target: &str, body: &[u8], now_ns: u64) -> Result<Caller, AuthError>;
}
pub struct Caller { pub hotkey: AccountId, pub nonce_ns: u64, pub scheme: Scheme }
pub enum AuthError { Malformed(&'static str), WrongReceiver(&'static str), Stale(&'static str), Replayed, BadSignature }
pub trait NonceStore: Send + Sync {
    fn retention(&self) -> Duration;
    fn check_and_store<'a>(&'a self, hotkey: &'a AccountId, nonce_ns: u64) -> BoxFuture<'a, Result<bool, StoreError>>;
}
```

Verification order is the normative one: shape -> receiver -> freshness (integer nanoseconds, not floats) ->
signature under the **declared** scheme only (never try both) -> replay store, consulted only after the
signature verified so unauthenticated traffic cannot poison it (`RES:D` §1.5). Error messages are the Python
strings verbatim (they are the de-facto contract, `RES:D` §8.1). The scheme line is re-derived from the parsed
crypto type, so `X-Bittensor-Crypto: SR25519` verifies (`RES:D` §1.2).

Crypto: signing uses `Signer::sign_raw` (in-process `KeypairSigner` or `RemoteSigner`; hardware signers refuse
raw signing). Verification calls the core's `keys::verify` (`CORE:keys/mod.rs:457-469` [V]), which also accepts a
signature over `<Bytes>payload</Bytes>`. Python accepts it through the same function, so rtao does too by default, which guarantees that an rtao miner accepts every request a Python miner accepts. `RES:D` §1.3 recommends rejecting the
wrapped form by default; this design keeps parity and offers `accept_polkadotjs_wrapped = false` (strict:
schnorrkel `substrate` context and ed25519 verified directly) [I: strict path crate choice]. rtao never produces
the wrapped form.

Nonce stores: `InMemoryNonceStore` (sharded map + expiry wheel, retention 60 s like `PY:http_auth.py:126-151`
per `RES:D` §1.6) and `RedisNonceStore` (`SET btauth:<hotkey>:<nonce> 1 NX PX <retention>`, feature `redis`) for
multi-process miners.

### 7.2 `rtao-http` server side

```rust
let app = miner.router(&cx)
    .layer(BtAuthLayer::new(verifier)
        .policy(MetagraphPolicy::validators(cx.metagraph.clone()).min_stake(StakeWeight::from_tao(1_000)))
        .rate_limit(PerHotkeyLimiter::per_second(20).burst(40))
        .max_body(4 * 1024 * 1024));
async fn generate(VerifiedCaller(caller): VerifiedCaller, Json(req): Json<Prompt>) -> impl IntoResponse { … }
```

- The layer buffers the body up to `max_body` (413 above) **before** any JSON extractor runs, hashes the raw
  bytes, and uses `req.uri().path_and_query()` verbatim as the target (`RES:D` §1.9-1.10).
- Auth failures -> 401 with FastAPI's body shape `{"detail": "<message>"}` (`SUB:docs/guides/signed-requests.mdx`
  recipe per `RES:D` §1.7) plus an `X-Bittensor-Auth-Error: <class>` diagnostic header [I: rtao addition].
- `CallerPolicy::admit(&Caller) -> Admission { Allow { priority: f64 }, Deny(reason) }`; `MetagraphPolicy`
  checks registered + validator permit + stake floor against the lock-free `MetagraphHandle` (`by_hotkey`
  hash index; v11 does a linear scan, `RES:A2` §4.1) -> 403 when verified but unwelcome (`RES:G` §3.4 item 7).
- `PerHotkeyLimiter` (keyed token buckets) -> 429; optional stake-weighted priority admission under contention
  (legacy template priority = caller stake, `RES:G` §3.4 item 7).
- Response streaming (SSE) is unrestricted; request streaming is impossible by construction (the hash covers the
  whole body) and documented as such.
- `v10-bridge` feature: `verify_v10_dendrite` accepting `bt_header_dendrite_*` callers during migration
  (`RES:G` §3.4 item 8).

### 7.3 `rtao-http` client side (the dendrite replacement)

```rust
pub struct BtAuthClient { /* reqwest::Client, Arc<dyn Signer>, Arc<dyn Clock> */ }
impl BtAuthClient {
    pub fn request(&self, method: Method, url: Url, receiver: Option<AccountId>) -> SignedRequestBuilder; // .json/.body/.timeout/.send
    pub async fn fan_out<Q: Serialize, R: DeserializeOwned>(&self, targets: impl IntoIterator<Item = AxonTarget>,
        make: impl Fn(&AxonTarget) -> (Method, String, Q), cfg: FanOut) -> Vec<(Uid, Result<R, FanOutError>)>;
}
pub struct FanOut { pub concurrency: usize, pub timeout: Duration /* 12 s, v10 dendrite default (RES:D §2.1) */, pub retries: u8 }
```

Every attempt re-signs with a fresh nonce; retries live above the signing step, never in a transport-level
retry policy (transport retries are rejected as replays, `RES:D` §1.4). Targets come from
`Metagraph::axon_targets()` (`ip == 0` means not serving; IPv6 renders `[addr]:port`, `RES:D` §5).

### 7.4 Tests that pin the protocol

The two vectors of `SUB:docs/guides/signed-requests.mdx:252-289` (sr25519 sign->verify; ed25519 signature
`0x1a6b6319…f704` byte-exact), every negative case of `PY:tests/unit/test_http_auth.py` (`RES:D` §1.8 list),
proptests for payload canonicalisation and boundary nonces (exactly `max_age`/`allowed_skew`), a fuzz target on
`verify(headers, body)`, and the Python cross-language lane (§11.5). The upstream rule applies: a change that
breaks the vectors needs `btauth/2`, not a fixture update (`RES:H` §1.5).

### 7.5 Legacy protocols (optional crates, M8)

None of the 13 subnets surveyed speaks btauth/1 yet; three run v10 Synapse, three an Epistula dialect, one fiber,
one btlightning (`RES:D` §6). Migration therefore needs bridges:

- `rtao-compat-synapse`: `bt_header_*` codec, sha3-256 body hash over Python `str()` renderings of
  `required_hash_fields` (a `PythonStr` trait implemented per field type and tested against Python), dendrite
  signer `"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"`, axon verifier with the `4 s + timeout` window and
  monotonic per `hotkey:uuid`, v10 status codes, raw-chunk streaming (`RES:D` §2.9).
- `rtao-compat-epistula`: the spec dialect with knobs for BitMind (future skew, 15 s), gobt and the Apex no-uuid
  variant (`RES:D` §3.4).

### 7.6 QUIC via btlightning (optional, M8)

`rtao-transport-quic` adapts `btlightning` 0.2.15 (sr25519 handshake bound to the certificate fingerprint,
MessagePack framing, no per-request signatures; `RES:D` §4) to rtao's `Signer` and `MetagraphPolicy`, with
default features off to avoid its `btwallet` and subxt-0.50 dependencies. It still depends on crates.io
`sp-core = "34.0"` unconditionally (`UP:inference-labs-inc_btlightning/crates/btlightning/Cargo.toml` [V]), next to
the core's forked `sp-core 37`; whether both coexist cleanly is unverified and is the M8 entry check (§15). The
`protocol = 4` collision (btlightning reads 4 as QUIC, every SDK writes 4 by default) is documented and left as is (`RES:D` §4.7).

---

## 8. Metagraph and typed models (`rtao-metagraph`, `rtao-runtime::models`)

### 8.1 Types

```rust
pub struct Metagraph {
    pub netuid: NetUid, pub mechid: MechId, pub block: (u32, H256), pub spec_version: u32,
    pub name: String, pub symbol: String, pub identity: Option<SubnetIdentity>,       // name/symbol: Vec<Compact<u8>> on the wire
    pub owner_hotkey: Option<AccountId>, pub owner_coldkey: Option<AccountId>, pub network_registered_at: u64,
    pub tempo: u16, pub last_step: u64, pub blocks_since_last_step: u64, pub num_uids: u16, pub max_uids: u16,
    pub pool: PoolState,            // alpha_in/out, tao_in, emissions, volume, moving_price: I96F32, spot price
    pub params: MetagraphParams,    // rho, kappa, min_allowed_weights, weights_version, weights_rate_limit, …
    pub commitments: Option<BTreeMap<Uid, NeuronCommitment>>,
    /* private: columnar per-uid vectors exactly as decoded; by_hotkey: HashMap<AccountId, Uid>; raw bytes */
}
pub struct NeuronRef<'a> { /* borrows one row */ }
impl<'a> NeuronRef<'a> {
    pub fn uid(&self) -> Uid;  pub fn hotkey(&self) -> &AccountId;  pub fn coldkey(&self) -> Option<&AccountId>;
    pub fn active(&self) -> Option<bool>;  pub fn validator_permit(&self) -> Option<bool>;  pub fn last_update(&self) -> Option<u64>;
    pub fn block_at_registration(&self) -> Option<u64>;  pub fn incentive(&self) -> Option<PerU16>;  pub fn dividends(&self) -> Option<PerU16>;
    pub fn consensus(&self) -> Option<PerU16>;  pub fn emission(&self) -> Option<AlphaRao>;  pub fn stake_weight(&self) -> Option<StakeWeight>;
    pub fn alpha_stake(&self) -> Option<AlphaRao>;  pub fn tao_stake(&self) -> Option<Rao>;  pub fn axon(&self) -> Option<AxonEndpoint>;
    pub fn identity(&self) -> Option<&ChainIdentity>;  pub fn collateral(&self) -> Option<Collateral>;   // locked/min/earned
}
impl Metagraph {
    pub fn len(&self) -> usize;  pub fn neuron(&self, uid: Uid) -> Option<NeuronRef<'_>>;
    pub fn by_hotkey(&self, hk: &AccountId) -> Option<NeuronRef<'_>>;  pub fn neurons(&self) -> impl Iterator<Item = NeuronRef<'_>>;
    pub fn validators(&self) -> impl Iterator<Item = NeuronRef<'_>>;  pub fn axon_targets(&self) -> impl Iterator<Item = AxonTarget> + '_;
    pub fn to_owned_neurons(&self) -> Vec<MetagraphNeuron>;  pub fn to_json(&self) -> serde_json::Value;   // v11 --json shape
}
```

Columns are kept as decoded (no per-neuron allocation; a 256-uid metagraph is ~53 KB of SCALE, 2,500 uids
~510 KB, `RES:C2` §5.1 estimates). Accessors return `Option` because selective fetches omit columns. Weights
and bonds are separate reads returning a sparse matrix (`SparseRows { row_ptr, cols, vals }`), avoiding the
dict-of-dicts blow-up at 2,500 uids (`RES:A2` §7); rows are normalised as in v11 (weights to fractions summing to
1, bonds as value/65535; `RES:A2` §2).

### 8.2 Construction

| Fetch | Runtime API | Use |
|---|---|---|
| `Preset::Full` | `SubnetInfoRuntimeApi.get_metagraph(netuid)` | owner dashboards, CLI, one-off tools |
| `Preset::Validator` | `get_selective_metagraph(netuid, [7,8,9,10,26,27,28,30,46,47,52,55,56,57,59,60,61,62,66,69])` | validator sync: ~90 B/uid; of the stake columns 67-69, each of which costs a full stake pass, only 69 is requested (`RES:C2` §5.2) |
| `Preset::Miner` | `get_selective_metagraph(netuid, [52,57,69,72])` | miner caller policy (hotkeys, permits, stake, validators) |
| `Preset::Custom(Vec<SelectiveMetagraphIndex>)` | same | anything else |
| mechanisms | `get_mechagraph(netuid, mecid)` / `get_selective_mechagraph` | the returned `netuid` field is the storage index; `mecid = idx / 4096` (`RES:C2` §0 item 5) |
| commitments (opt-in) | `query_map(Commitments.CommitmentOf, [netuid])` + `RevealedCommitments` at the same block | v11 `fetch(commitments=True)` (`RES:A2` §4.1) |

Rules: one block per fetch; `None` when the subnet does not exist; never `get_all_metagraphs`,
`get_subnet_state` or `get_delegates` in loops (`RES:C2` §9 item 7). A storage fan-out fallback with bounded
concurrency (SN2 style, `RES:E` §4.1 item 11) exists only for runtimes where the API is unavailable.

### 8.3 Sync service

```rust
pub struct SyncPolicy {
    pub cadence: SyncCadence,               // EveryEpoch { fallback_blocks } | EveryBlocks(u32) | Interval(Duration)
    pub preset: Preset,
    pub early_on_events: bool,              // NeuronRegistered / AxonServed / HotkeySwapped* for this netuid -> debounced refresh
    pub reconnect_after_failures: u32,      // 3 (SN2, RES:G §2.4 item 12)
}
impl MetagraphSync {
    pub fn spawn(client: Subtensor, netuid: NetUid, mech: MechId, p: SyncPolicy, cancel: CancellationToken)
        -> (MetagraphHandle, broadcast::Receiver<MetagraphDiff>);
}
pub struct MetagraphHandle(Arc<ArcSwap<Metagraph>>);       // lock-free load() on every request
pub struct MetagraphDiff {
    pub from_block: u32, pub to_block: u32,
    pub registered: Vec<(Uid, AccountId)>, pub replaced: Vec<(Uid, AccountId, AccountId)>, pub deregistered: Vec<(Uid, AccountId)>,
    pub permits_gained: Vec<Uid>, pub permits_lost: Vec<Uid>, pub axons_changed: Vec<Uid>,
}
```

Defaults: validators `EveryEpoch { fallback_blocks: 100 }` with `Preset::Validator`; miners `Interval(600 s)`
(minimum 30 s) with `Preset::Miner` (SN2's values, `RES:G` §3.4 item 7); owner monitor `EveryEpoch` with `Full`.
`MetagraphDiff::replaced` drives score resets (template and SN2 reset per-uid state when the hotkey at a uid
changes, `RES:G` §2.4 item 13); `hotkey_root(hk)` follows `HotkeyRoot`/`HotkeySuccessor` lineage for scoring
continuity across hotkey swaps (collateral guide, same reference).

### 8.4 Persistence

`snapshot::save` writes `metagraph-<netuid>-<mechid>-<block>.scale` = header (magic `RTMG`, format version,
genesis, `spec_version`, block hash, netuid, preset) + the raw runtime-API response bytes; `load` decodes them
with the cached runtime of that spec. This is deterministic, independent of rtao's struct layout, and matches the
recommendation of `RES:A2` §4.4. `to_json` exports the v11 `--json` shape. v10 `.pt` pickles are not read
(`RES:I` §3.3); importing the legacy template's `state.npz` scores is an M8 option (`import-npz`).

---

## 9. The framework for owners, validators and miners (`rtao-neuron`)

### 9.1 Runtime architecture

`NeuronRuntime` is a supervisor over a `tokio::task::JoinSet` with one `CancellationToken`; services
communicate through `watch` (head, metagraph version) and `broadcast` (diffs, events) channels:

```
            ┌────────────── HeadTracker (finalized heads, gap-free) ─────────────┐
            │                                                                     │
   MetagraphSync ──► MetagraphHandle (ArcSwap) ──► BtAuthLayer policy (miner)     │
            │              │                                                      ▼
            │              └──► Validator::step (user, scoring, BtAuthClient fan-out)
            ▼                                                                     │
      MetagraphDiff ──► Validator::on_metagraph (score resets)                   │
                                                                                  ▼
   WeightScheduler (per mechanism) ◄── EpochClock ◄────────────────────── head watch
   AxonPublisher (miner; validator optional), StateStore (300 s + exit), RuntimeWatcher
   Telemetry (/metrics, /healthz, /readyz), AutoUpdate (opt-in), signal handler -> cancel
```

```rust
pub struct NeuronCx {
    pub client: Subtensor, pub netuid: NetUid, pub hotkey: Arc<dyn Signer>,
    pub metagraph: MetagraphHandle, pub epochs: EpochClock, pub http: BtAuthClient,
    pub shutdown: CancellationToken, pub metrics: Metrics, pub state_dir: PathBuf,
}
pub struct StateCell<S>(Arc<tokio::sync::RwLock<S>>);   // never hold the write guard across an await
```

### 9.2 Validator

```rust
pub trait Validator: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    fn on_start(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send { async { Ok(()) } }
    /// One unit of scoring work; called in a loop, paced and cancelled by the runtime.
    fn step(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Pure: the weight row for one mechanism from the current state.
    fn weights(&self, cx: &NeuronCx, s: &Self::State, mech: MechId) -> WeightsProposal;
    fn on_metagraph(&self, _diff: &MetagraphDiff, _s: &mut Self::State) {}
}
pub enum WeightsProposal { Set(WeightsInput), HoldPrevious, Skip(&'static str) }
pub enum WeightsInput { Map(BTreeMap<Uid, f64>), Parallel { uids: Vec<Uid>, weights: Vec<f64> }, RawU16 { uids: Vec<Uid>, values: Vec<u16> } }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = rtao::neuron::NeuronConfig::load()?;          // flags > env > file > defaults
    rtao::neuron::NeuronRuntime::validator(cfg, MyValidator::default()).run().await
}
```

`EmaScores { alpha (0.1, template default), by_hotkey }` with `reset(hotkey)` and `to_weights(&Metagraph)` covers
the common scoring pattern (`RES:I` §2.5); state is keyed by hotkey, never by uid alone.

### 9.3 `WeightScheduler`: the validator's critical path

Per finalized head, per mechanism `mech < MechanismCountCurrent[netuid]`:

1. `cadence.due(&EpochStatus, last_attempt[mech])`. Default `OncePerEpoch { offset_blocks: 2 + (blake2(hotkey)
   mod max(1, tempo / 10)) }`: one commit per epoch is sufficient (commits are tagged per epoch and revealed per
   epoch, `RES:C1` §2.3), early in the tempo because inclusion is first-come and tips buy nothing
   (`RES:C1` §0 item 5, §7.3), with deterministic per-hotkey jitter so validators do not all land in the same
   block [I]. CR-off subnets use `RateLimited` (submit once `blocks_since_last_update >= WeightsSetRateLimit`).
   An activity alarm fires at 80% of the effective activity cutoff (factor * tempo / 1000; `RES:G` §2.2 item 6).
2. `WeightsPreflight::read(at, hotkey, netuid, mech)` at **one block**: `Uids`, `LastUpdate[idx][uid]`,
   `WeightsSetRateLimit`, `CommitRevealWeightsEnabled`, `CommitRevealWeightsVersion`, `RevealPeriodEpochs`,
   `WeightsVersionKey`, `MinAllowedWeights`, `MaxWeightsLimit`, `SubnetworkN`, `SubnetEpochConsensus`,
   `ValidatorPermit`, `StakeThreshold` + own stake weight, and the epoch schedule state (`LastEpochBlock`,
   `PendingEpochAt`, `SubnetEpochIndex`, `Tempo`, `BlocksSinceLastStep`). The v11 preflight subset is
   `PY:intents/weights.py:154-215` (`RES:G` §2.6 item 18); rtao adds the version-key, permit and threshold checks
   (§4.8 item 5). Codes returned match the chain's (`NOT_REGISTERED`, `RATE_LIMITED`, `NOT_AUTHORIZED`,
   `INVALID_ARGUMENT`) plus remediation ("subnet requires version_key >= N; upgrade the validator").
3. `validator.weights(state, mech)`; `HoldPrevious` and `Skip` leave the chain's current row in place.
4. `conform(input, &rules)` runs the v11 pipeline (`PY:intents/weights.py:54-114,218-258`; `RES:I` §2.4): reject
   duplicates/negatives/non-finite; if `MaxWeightsLimit < 65535` and not a pure self-weight, clip with the
   mass-redistributing cutoff (identical to v10 `normalize_max_weight`, epsilon 1e-7); max-upscale
   `round(w / max * 65535)`; drop zeros; enforce `len >= min(MinAllowedWeights, SubnetworkN)` unless self-weight.
   `RawU16` requires Null consensus and the sole permit holder, values taken verbatim (`RES:G` §2.6 item 17).
   The chain's `MaxWeightExceeded` check is dead code today (`get_max_weight_limit` is `u16::MAX`, `RES:C1` §0
   item 7); rtao clips anyway for parity. v10's `process_weights` quantile/uniform fallbacks are available only
   behind `legacy-weights` with a warning (`RES:I` §2.4).
5. Hold-previous guard: a row with no positive miner weight (only self/owner) is not submitted (SN2's guard,
   `RES:G` §2.6 item 21).
6. Path: CR on -> `commit_timelocked_mechanism_weights(netuid, mech, ct, reveal_round, CommitRevealWeightsVersion)`
   (index 118); CR off -> `set_mechanism_weights` (119); `mecid = 0` by default like v11 (`RES:G` §2.6 item 16).
   The ciphertext is built by `TimelockCommitter`:

   ```text
   first  = rtao_btcore::epoch::predict_first_reveal_block(&schedule, reveal_period)   // pallet predicate (§1.6)
   target = first + SECURITY_BLOCK_OFFSET (3)
   round  = max(1, floor((clock.now_unix() + (target - current_block) * block_time - GENESIS_TIME) / DRAND_PERIOD))
   ct     = encrypt_and_compress(WeightsTlockPayload { hotkey: pubkey, uids, values, version_key }.encode(), round)
   ```

   (formula `CORE:timelock/mod.rs:169-187` [V]; `block_time` from `Aura.SlotDuration`, so fast-block localnets
   work, `RES:G` §2.6 item 20). Size limits: <= 5,000 bytes on Yuma, <= floor(32 KiB / mechanism count) on Null
   (`RES:C1` §2.3). A pool rejection `InvalidRevealRound` (custom code 15) triggers one recomputation.
7. Submit with era **32**, `WaitFor::InBlock`, deadlines 30 s to submit and 180 s to inclusion (SN2's bounds,
   `SN2:sn2-chain/src/weights.rs:14-17` [V]). Era 32 is deliberate: the pool keeps at most one pending commit per
   `(hotkey, netuid_index)` (`RES:C1` §2.2) and the default rate limit is 100 blocks, so a lost commit expires
   well before the next one is allowed, so reconciliation (§3.7.6) always reaches a definite answer [I].
8. Reveal watcher: during the predicted reveal epoch, look for `TimelockedWeightsRevealed(idx, hotkey)`; if the
   epoch ends without it, count `rtao_weights_reveal_missed_total` and log the inputs (round, predicted block,
   observed `Drand.LastStoredRound`); the most likely causes are clock skew and deferred epochs (§1.6).

### 9.4 Miner

```rust
pub trait Miner: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    fn router(&self, cx: &NeuronCx) -> axum::Router;          // the runtime wraps it in BtAuthLayer + policy + limits
    fn caller_policy(&self, cx: &NeuronCx) -> MetagraphPolicy { MetagraphPolicy::validators(cx.metagraph.clone()) }
    fn on_start(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send { async { Ok(()) } }
    fn on_metagraph(&self, _diff: &MetagraphDiff, _s: &mut Self::State) {}
}
```

The runtime adds: HTTP server with graceful shutdown, `AxonPublisher`, `MetagraphSync` (miner preset),
`PruneRisk` monitor, `StateStore`, telemetry.

### 9.5 `AxonPublisher`

```rust
pub struct AxonConfig {
    pub port: u16, pub external_ip: Option<IpAddr>, pub external_port: Option<u16>,
    pub protocol: u8 /* 4 */, pub version: u32 /* 1 */, pub certificate: Option<NeuronCertificate>,
    pub ip_sources: Vec<Url> /* https://api4.ipify.org, 5 s timeout (SN2) */, pub allow_non_global: bool /* false; true for local */,
    pub recheck: Duration /* 300 s */, pub reset_on_shutdown: bool /* false */,
}
```

Algorithm (chain rules `SUB:pallets/subtensor/src/subnets/serving.rs` per `RES:C1` §3, conventions per
`RES:G` §3.3): resolve the IP (override first; reject loopback/RFC1918/CGNAT/link-local IPv4 unless allowed;
IPv6 is encoded correctly but not auto-detected, `RES:G` §8 item 9); read `Axons[netuid, hotkey]`; if
`(ip, port, ip_type, protocol, version)` already match, do nothing; if `now - prev.block < ServingRateLimit`
(default 50), schedule for the first allowed block; else submit `serve_axon` (or `serve_axon_tls` with a
certificate) signed by the hotkey (`Pays::No`, fee-free). Pool rejections 11/12/13 map to
`InvalidIpAddress`/`ServingRateLimitExceeded`/`InvalidPort`. Moving hosts follows the docs: publish the new
endpoint first, drain, then stop the old process; `reset_axon` on shutdown is opt-in.

### 9.6 Registration, immunity, PoW

- `RegistrationStatus { uid, registered_at, immunity_period, immune_until, subnet_full, prune_rank, at_risk }`
  from the metagraph plus `ImmunityPeriod`/`MaxAllowedUids`, using the pallet's pruning order (lowest emission
  among non-immune, ties to older registration then lower uid; owner hotkeys immortal up to
  `ImmuneOwnerUidsLimit`; immune ones pruned only below `MinNonImmuneUids`; `RES:C1` §4.1). Gauges
  `rtao_immunity_blocks_remaining`, `rtao_prune_risk`.
- Runtimes refuse to start unregistered unless `--wait-for-registration` (then they wait for
  `NeuronRegistered`). Registering is a coldkey operation done from the CLI or owner tools: `burned_register`
  (always MEV-shielded; preview of burn vs collateral split from `Burn` and `CollateralLockShare`; unbounded spend
  so any spend cap blocks it unless the operator passes an explicit maximum, `RES:G` §2.1 item 1),
  `pow_register` (`PowSolver`: prefix `b"subtensor-pow-register-v1" || netuid LE || block_hash || hotkey || coldkey`,
  core `pow::seal`, difficulty clamp, 5-block work age with refresh, optional GPU; `RES:B` §3.7, `RES:C1` §4.2),
  `root_register`.

### 9.7 Owner operations

```rust
impl OwnerOps {
    pub async fn create_subnet(&self, hotkey: AccountId, wait: RegistrationWait) -> Result<SubnetCreated, TxError>; // queued path handled
    pub async fn start(&self) -> Result<ExtrinsicResult, TxError>;                    // after subnet_start_schedule
    pub async fn set_hparam(&self, name: &str, value: HparamArg, wait: WindowWait) -> Result<ExtrinsicResult, TxError>;
    pub async fn set_identity(&self, id: SubnetIdentity) -> Result<ExtrinsicResult, TxError>;
    pub async fn update_symbol(&self, symbol: &str) -> Result<ExtrinsicResult, TxError>;
    pub async fn set_mechanism_count(&self, n: u8) -> Result<ExtrinsicResult, TxError>;
    pub async fn trim(&self, max_n: u16) -> Result<ExtrinsicResult, TxError>;
    pub async fn trim_null(&self, target: u16, recovery: &Path) -> Result<TrimReport, TxError>;   // resumable batches <= 64 uids
}
impl HyperparamPlanner { pub async fn plan(&self, at: &Snapshot, netuid: NetUid, name: &str, v: HparamArg) -> Result<HparamPlan, TxError>; }
pub struct HparamPlan { pub current: Value, pub raw: Value, pub call: CallBytes, pub earliest_block: u64, pub reasons: Vec<String> }
```

The planner encodes the owner rules of `RES:G` §1.5 and `RES:C1` §5.2: owner-settable versus root-only, unit
conversion (§5), bounds (tempo 360-50,400, activity factor 1,000-50,000, collateral share <= 62,258, …), the admin
freeze window (`blocks_until_next_auto_epoch >= AdminFreezeWindow` and no pending trigger), per-parameter rate
limits from `LastRateLimitedBlock` (`Tempo * OwnerHyperparamRateLimit`; version key 5 tempos; mechanism count
7,200 blocks; trims 216,000; owner hotkey 50,400), the `immunity_period > commit_reveal_period * tempo` warning,
the shared `alpha_low`/`alpha_high` setter, `epoch_consensus` as an enum, fixed128 as `{bits}`, and never the
deprecated no-op `set_tempo`/`set_activity_cutoff_factor`. `WindowWait::UntilOpen { deadline }` waits for the
computed block instead of failing.

`OwnerMonitor` streams the owner events listed in `RES:G` §1.11 and exports both "on" flags (`SubtokenEnabled` and `SubnetEmissionEnabled`; a subnet can run with zero pool-side emission, `RES:G` §1.4 item 13), conviction
leader versus the 18% threshold, pending delayed-proxy announcements, and the subnet's pruning exposure
(`get_subnet_to_prune`).

### 9.8 Timelock helpers beyond weights

`timelock::seal(data, Reveal::AtRound(r) | Reveal::InBlocks(n)) -> Timelocked` (the `UserData` envelope, so `btcli timelock` can read rtao output; `CORE:timelock/mod.rs:62,230`); `timelock::open(&Timelocked, &DrandClient)`
(async fetch + `decrypt_with_signature`); `commitments::set_timelocked(netuid, fields)` composing
`Commitments.set_commitment` with `TimelockEncrypted { encrypted: inner_ciphertext, reveal_round }` and the
plain/sealed/revealed/failed status machine (`RES:A2` §2; `RES:G` §3.4 item 9); reveal time =
`GENESIS_TIME + (round - 1) * 3 s` (`RES:A2` §2).

### 9.9 Configuration and CLI conventions

- Precedence: flag > env > config file > default (`SUB:docs/cli.mdx:21-22` per `RES:G` §4.2). Neurons read an
  optional `--config rtao.toml`; the CLI reads `~/.bittensor/btcli.json` (§10).
- Flags: v11 spellings primary, v10 dotted spellings as clap aliases (SN2 precedent, `RES:I` §3.4): `--netuid`,
  `-n/--network` (`subtensor.network`), `--chain-endpoint` (`subtensor.chain_endpoint`), `-w/--wallet`
  (`wallet.name`), `-H/--wallet-hotkey` (`wallet.hotkey`), `--wallet-path` (`wallet.path`), `--axon-port`
  (`axon.port`), `--external-ip` (`axon.external_ip`), `--log-level` (`logging.level`), `--state-dir`,
  `--metrics-addr`, `--fallback-endpoints`, `--archive-endpoints`, `--retry-forever`.
- Env: `BT_NETWORK`, `BT_CHAIN_ENDPOINT` (overrides `local`), `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`,
  `BT_FALLBACK_ENDPOINTS`, `BT_ARCHIVE_ENDPOINTS`, `BT_RETRY_FOREVER` (`RES:I` §3.2); v10 names
  (`BT_SUBTENSOR_NETWORK`, `BT_SUBTENSOR_CHAIN_ENDPOINT`, `BT_WALLET_NAME`) accepted with a deprecation warning;
  `BT_AXON_EXTERNAL_IP` accepted (SN2). Libraries never read `argv` (v11 rule, `RES:I` §3.1).

### 9.10 Observability

- `tracing` everywhere; `telemetry::init(LogFormat::Compact | Json | Pretty)` logs to stderr (`Compact` is single-line `key=value`, the style of the maintainer's own tools, `RES:E` §6); data goes to stdout (CLI).
- Prometheus exporter on `--metrics-addr` (off by default). Core metrics: `rtao_chain_head{kind}`,
  `rtao_finality_lag_blocks`, `rtao_spec_version`, `rtao_surface_breaking_items`, `rtao_rpc_*` (§3.2.5),
  `rtao_metagraph_sync_seconds`, `rtao_metagraph_age_blocks`, `rtao_weights_submissions_total{mech,path,outcome}`,
  `rtao_blocks_since_last_update{mech}`, `rtao_weights_reveal_missed_total`, `rtao_axon_published{ip,port}`,
  `rtao_btauth_requests_total{outcome}`, `rtao_btauth_rejections_total{class}`, `rtao_immunity_blocks_remaining`,
  `rtao_prune_risk`, `rtao_clock_skew_seconds`, `rtao_state_save_seconds`.
- `/healthz` (process alive) and `/readyz` (connected, runtime verified, registered, metagraph younger than twice
  its cadence, last weight attempt not failed) on the metrics listener.

### 9.11 Persistence, shutdown, upgrades, deployment

- `StateStore`: default directory `~/.bittensor/miners/<wallet>/<hotkey>/netuid<N>/<name>/` (template layout,
  `RES:I` §3.1); envelope `{schema_version, rtao_version, netuid, hotkey, block, saved_at, state}` as JSON;
  atomic write (temp file in the same directory, fsync, rename, fsync directory); three generations kept; save
  every 300 s and at shutdown (`RES:G` §2.5 item 15).
- Shutdown: first SIGTERM/SIGINT cancels the token -> stop accepting work -> wait for an in-flight weight
  submission up to 60 s -> final save -> optional `reset_axon` -> graceful HTTP close -> exit 0; a second signal
  exits immediately (130).
- Runtime upgrades: §3.4; on `SurfaceChanged` for the weight calls the scheduler pauses, alerts and keeps
  everything else running.
- Binary upgrades: `auto-update` feature (off by default) implementing SN2's pattern: poll releases every 300 s,
  verify `SHA256SUMS` and optionally a Sigstore attestation, atomically replace the binary, request a graceful
  restart from the supervisor (`RES:G` §2.8 item 25).
- Deployment templates: multi-stage Dockerfile (builder needs a C toolchain and Perl because the core's `host`
  feature builds libsodium and vendored OpenSSL, `RES:B` §6), non-root runtime image, read-only hotkey mount,
  state volume; systemd unit (`Restart=on-failure`, `KillSignal=SIGTERM`, `TimeoutStopSec=90`); readiness probe
  on `/readyz`.

### 9.12 Safety rails (defaults)

1. Neuron runtimes load the hotkey only; coldkey operations require the CLI/owner tools (§6.5).
2. A neuron's `Policy` is `{ allowed_netuids: {netuid}, max_spend: 0, allow_raw_calls: false }`; every
   mutation has a dry-run `Plan`; the CLI declines mutations without `--yes` when not on a TTY (§10).
3. Genesis check at connect; refusal to sign across spec or transaction-version changes.
4. Surface verification at connect and on upgrades, fail-closed per call (§3.6.4); extension guard (§3.4).
5. Clock-skew check against `Timestamp.Now` (btauth freshness and reveal rounds depend on it).
6. Preflights at one pinned block: registration, rate limits, version key, permit, stake threshold, admin window,
   per-parameter rate limits.
7. Idempotent axon publishing; bounded watches with definitive reconciliation; no blind resubmission.
8. Hold-previous weights guard; reveal-miss and activity-cutoff alarms; prune-risk gauge.
9. Spend caps block unbounded-spend intents (`register_subnet`, `burned_register`, `transfer_all`, …;
   `RES:A1` §3.14) unless the operator states a maximum.
10. `burned_register` and `add_collateral` are always MEV-shielded (`mev_shield_required`, `RES:A1` §3.3, §3.7).

---

## 10. CLI (`rtao-cli`, binary `rtao`)

### 10.1 Generated from the registries

v11 generates `btcli tx <op>` from the intent registry and `btcli query <read>` from the read registry so
library and CLI cannot drift (`PY:cli/tx.py:1-11`, `PY:cli/query.py:1-6` [V]). rtao does the same with derives:

- `rtao tx <op-with-dashes>`: one subcommand per `IntentMeta.op`. The `#[derive(Intent)]` macro (feature `cli`)
  derives a clap `Args` struct from the intent's fields: names become `--kebab-case` (`--amount-tao`,
  `--hotkey-ss58` printed as `--hotkey` like v11's address options), doc comments become help, money fields parse
  exact decimals and `all`, list/map fields accept JSON or comma lists, address fields accept ss58, address-book,
  proxy-book or saved-multisig names, or `WALLET[/HOTKEY]` (`RES:G` §4.5). The command body runs the single
  choke point: build -> `plan` -> review card (effects, warnings, fee, payer, spend, violations) -> confirmation or
  `--yes` -> `execute` -> `ExtrinsicResult` rendering. `--dry-run` stops after the plan.
- `rtao query <read-with-dashes>`: options from the read's params, output table or `--json`, `Grouped`/`Matrix`
  render hints (`RES:A2` §1.2).
- `rtao call Pallet.function --args '{…}' [--sudo] [--multisig NAME]`: dynamic composition through the core's
  `Value` encoder; requires `allow_raw_calls`.
- `rtao tools` (catalog-schema manifest, §4.3), `rtao explain <code|ChainErrorName>` (§4.5).

### 10.2 Command tree, globals, behaviour

The tree mirrors v11 (`PY:cli/main.py` per `RES:I` §4.2): `wallet, stake, root, subnets, conviction,
hparams` (alias `sudo`), `addr, evm, proxy, multisig, query, tx, call, tools, explain, config, misc`
(`weights, axon, collateral, timelock, crowd, extension, upgrade`), with v11's hidden v9 aliases (`w`, `st`, `s`,
`su`, `wt`, `cr`, …). rtao adds `doctor` (toolchain, `[patch]` check, connectivity, genesis/spec versus the compat
range, wallet permissions, clock skew, docker) and `localnet up|down|fund|subnet` (the documented local loop,
`RES:G` §5 item 5).

Globals are v11's (`PY:cli/globals.py` per `RES:G` §4.5): `-n/--network`, `-w/--wallet`, `-H/--wallet-hotkey`,
`--wallet-path`, `--json`, `-q`, `-v/-vv/-vvv`, `-y/--yes` (`--no-prompt`), `--dry-run`,
`--mev-shield/--no-mev-shield`, `--proxy-for`, `--force-proxy-type`, `--signatory`, `--signer
wallet|ledger|vault|extension|remote`, `--ledger*`, `--signer-address`, `--extension-*`,
`--wallet-password-file`, `--keychain-password`, `--macos-password`, `--fallback-endpoints`,
`--archive-endpoints`, `--retry-forever`.

Non-interactive discipline (`RES:G` §4.5): without `--yes`, in `--json` mode or with no TTY, a mutation is
declined with exit code 1; missing required options exit 2; JSON goes to stdout, diagnostics to stderr
(`-v` INFO, `-vv` DEBUG, `-vvv` raw frames; `BITTENSOR_LOG` overrides). Config is `~/.bittensor/btcli.json`
(`BTCLI_CONFIG`) with v11's settable keys (`PY:config.py:31-56` [V]), unknown keys ignored, corrupt files renamed
`.corrupt.<ts>`, atomic writes; the address/proxy/multisig books and caches use the same files and formats, so a
user can alternate between `btcli` and `rtao` (`RES:G` §4.2). Explorer links for finney/archive only. Shell
completions via `clap_complete`.

### 10.3 Scope per milestone

| Milestone | CLI scope |
|---|---|
| M3 | globals, config, `wallet list/balance/new-coldkey/new-hotkey/regen-*/inspect`, `subnets metagraph/hyperparameters/list/show/burn-cost/create-cost`, `hparams get/set/start/check-start/set-identity/set-symbol/trim/trim-null-batch/mechanisms`, `weights set/commit/reveal`, `axon set/reset`, `tx`/`query` for every implemented item, `call`, `tools`, `explain`, `doctor`, `localnet` |
| M4 | `query` complete |
| M5 | `tx` complete, `stake add/remove/move/transfer/swap/unstake-all`, `proxy`, `multisig`, `crowd` reads |
| M6 | `wallet sign/verify/encrypt/decrypt/set-identity/associate-hotkey/swap-*`, `wallet keychain save/show/delete`, `--signer ledger|vault|extension|remote` |
| M7 | everything else in the v11 tree (`stake wizard/show/list/auto/set-auto`, `root`, `conviction`, `addr`, `evm`, `misc collateral/timelock/crowd/extension/upgrade`), `--json` output parity, completions |

---

## 11. Testing

### 11.1 Layers

| Layer | Runs | Needs |
|---|---|---|
| unit + property | every PR | nothing |
| golden conformance (upstream + rtao vectors) | every PR | nothing (metadata embedded in fixtures) |
| `FakeChain` integration (executor, scheduler, reconnect, reconciliation, every read) | every PR | nothing |
| localnet e2e | `e2e` label, main, nightly matrix | docker |
| Python interop | nightly, `interop` label | docker (Python only inside CI) |
| live probes (read-only finney/testnet) | every 6 h | network |
| benchmarks, fuzz | nightly | nothing |

### 11.2 Upstream golden fixtures reused (vendored under `fixtures/upstream/` with commit and sha256)

| File | What rtao asserts |
|---|---|
| `SUB:sdk/python/tests/fixtures/golden.json` -> `storage_keys` (10, incl. double/triple maps and `Multisig.Multisigs`) | typed descriptors + `Runtime::storage_key` give `key_hex` |
| -> `calls` (10, incl. nested `Sudo.sudo`, `Utility.batch`, `Proxy.proxy`) | generated call structs -> `compose_call` give `data_hex` |
| -> `signature_payloads` (immortal; mortal period 64) | rtao pipeline `TxParams` -> payload bytes |
| -> `extrinsics` (fake signature bytes 1..=64) | full pipeline with a fixed-signature test signer -> `extrinsic_hex`, `extrinsic_hash` |
| -> `multisig` (3), `ss58` (4), `constants` (3) | derivations, codec, constant decode |
| -> `storage_values` (8, incl. a miss), `query_maps` (4), `runtime_calls` (4), `events`, `block`, `payment_info`, `receipt` | typed decode incl. Default/Optional semantics, key recovery, runtime-API param encoding and typed outputs (`get_neurons_lite`, `get_subnet_hyperparams`, stake info, `account_nonce`), lazy events, block decode, fee decode, outcome resolution from recorded events |
| `SUB:sdk/python/tests/fixtures/shape_corpus/corpus.json` (797/802 types, 2,899 samples, spec 461) | (a) core decoder regression at every pin bump: `to_corpus_json(decode_id(...)) == decoded`; (b) every typed model decodes every sample of the type ids it covers, and its projection equals `decoded` (`RES:H` §3.4 item 2) |
| `SUB:sdk/bittensor-core/fixtures/ledger_proof_vector.json` | `LedgerSigner` proof assembly byte-exact |
| `CORE:timelock/epoch_schedule_vectors.rs` (transcribed to JSON by `xtask import-upstream`) | `rtao_btcore::epoch` shim, plus one new safety-net vector (§1.6) |
| digest `affbcc25…11c0` (`CORE:digest/mod.rs:191-203` per `RES:B` §8) | `metadata_digest` with `ChainInfo` built from the runtime |
| PoW seals (`CORE:pow/mod.rs:88-105`), `twox_128("System")` (`CORE:mlkem/mod.rs:118-124`) | `PowSolver`, shield key hash |
| btauth vectors (`SUB:docs/guides/signed-requests.mdx:252-289`) + negatives (`PY:tests/unit/test_http_auth.py`) | §7.4 |
| `PY:tests/unit/test_keyfiles_golden.py:66-84` (Fernet parameters, JSON fields, 0600/0700) + rtao cross vectors produced by Python in the interop lane (`fixtures/rtao/keyfiles/`) | wallet loading of every format; closes the "no encrypted vectors" gap (`RES:B` §8) |
| `PY:tests/unit/test_null_weights.py:13-45` and the conform tables | `conform` / raw-u16 vectors |
| `PY:tests/harness/samples.py` (`READ_SAMPLES`) | one sample per read; head and pinned dispatch |

The upstream goldens are frozen at spec 461 and stay valid because the metadata is embedded (`RES:H` §1.5).
`cargo xtask record --image ghcr.io/raofoundation/subtensor-localnet:v<spec>` re-records rtao's own
`fixtures/rtao/golden-<spec>.json` in the upstream schema plus V16 hex, the RFC-0078 digest, a weights call set,
a `get_metagraph` response, a btauth self-check and a recorded-not-submitted v5 encoding; `--endpoint finney
--read-only` adds mainnet-only vectors; `cargo xtask corpus` writes `corpus-<spec>.json` (`RES:H` §5).

### 11.3 `FakeChain` (offline, in-process)

`rtao-testkit::FakeChain` implements `RpcClientT`: serves pinned metadata (from `metadata/` or a golden file),
runtime versions, scripted heads (`advance(n)`, forks for reorg tests), storage seeded by typed descriptor
(`seed::<storage::subtensor_module::Tempo>((NetUid(1),), 360)`), runtime-API responses, a scripted extrinsic
lifecycle (`on_submit(|xt| Script::in_block(events).then_finalized())`), and fault injection (socket drop,
`-32004`/`-32005`/close 1008 refusals, silence, reconnect). With tokio's paused clock it drives the executor,
retries, reconnect semantics, reconciliation, the weight scheduler and every read without docker. It is the Rust counterpart of v11's `FakeSubstrate` and `FakeNode` (`RES:H` §5.4; `RES:A2` §6.3).

### 11.4 Localnet e2e (docker)

- Harness: plain `docker` CLI, as upstream (`RES:H` §7 recommendation 4): `docker run -d --rm -p 127.0.0.1::9944
  <image> [True|False]`, readiness by the image healthcheck or the `Imported #1` log line (180 s), port from
  `docker port`; the env contract is upstream's: `E2E_ENDPOINT` reuses a running node, `LOCALNET_IMAGE` /
  `LOCALNET_IMAGE_NAME`, `SKIP_PULL` (`SUB:sdk/bittensor-core/tests/e2e_support/mod.rs:63-80` [V]). The default
  image is `subtensor-localnet:v<pinned spec>` by digest (upstream's default tag `monorepo-sdk` does not exist,
  `RES:H` §6 item 1).
- Profiles: `Fast` (`True`: 250 ms blocks, tempo 10) and `RealTime` (`False`: 12 s blocks) for timing-sensitive
  tests such as reveal rounds (`RES:G` §5 items 3, 6).
- Isolation: one container per test binary (nextest test group) and, inside it, a fresh subnet plus freshly funded
  accounts per test, so tests run in parallel. The subnet lock cost doubles per registration (`RES:G` §1.1 item 2)
  and would exhaust the dev balances, so the harness lowers it with Alice-sudo calls present in the metadata
  (`sudo_set_network_min_lock_cost`, `sudo_set_lock_reduction_interval`; also `sudo_set_weights_set_rate_limit`,
  `sudo_set_serving_rate_limit`, `sudo_set_tx_rate_limit` per test; `META:explore/AdminUtils_calls.txt` [V]); the
  `pow-faucet` build already zeroes the network registration rate limit (`RES:G` §1.1 item 2). Tests that mutate
  global state get a dedicated container. (Upstream starts one container per test in 32 shards, `RES:B` §5;
  per-subnet isolation is cheaper [I].)
- Scenario builder: `Scenario::new(&ln).owner().subnet().start().validator().miner().stake(tao(1_000)).build().await`.
- Image matrix (`RES:H` §3.5): pinned `v<spec>` (required), `:mainnet` (required), `:testnet` (required, may open a
  pre-break issue instead of failing `main`), `:devnet` (allowed failure), `:main` (weekly, allowed failure),
  optional wasm-override lane via `SUBTENSOR_LOCALNET_RUNTIME_WASM` (`RES:H` §1.3).
- Inventory: the upstream manifest (72 intent plans + 40 behaviour tests, `RES:B` §5) mirrored as M5 completes,
  plus the operator scenarios that define M1-M3:
  1. validator, CR on: 20 epochs of commits, every reveal observed, weights read back, incentives move;
  2. validator, CR off: direct sets respect `WeightsSetRateLimit`;
  3. two mechanisms: per-mechanism commits and `LastUpdate[idx]`;
  4. Null consensus: raw u16, sole permit holder, null trim batches;
  5. owner bumps `WeightsVersionKey` mid-run -> validator preflight error with remediation -> reconfigure -> resume;
  6. lost watch: the session test hook drops the socket mid-commit -> definitive reconciliation, nonce reuse safe;
  7. runtime upgrade (wasm-override lane): surface re-verified, no signature across the spec change;
  8. miner: idempotent `serve_axon`, rate-limit scheduling, 401/403/429 paths;
  9. owner: create (immediate path), start, hyperparameter planner waits for the admin window, mechanisms,
     identity, symbol, trim;
  10. shielded `burned_register` resolved via the inner extrinsic;
  11. `RealTime`: reveal happens in the predicted epoch with 12 s blocks;
  12. differential: rtao reads versus the core `Client` at the same block (metagraph, neurons, hyperparameters,
      stake, quote) and rtao call bytes versus the core's 22 `IntentCall` constructors.

### 11.5 Python interop (docker compose; CI oracle only)

Services: `chain` (localnet image by digest), `py` (`python:3.12-slim` + `uv pip install bittensor==<stable
matching the spec>`, 11.3.x today, `RES:H` §2.5), `rtao` (test binaries). Scenarios:

1. Wallets: Python creates an encrypted coldkey and a plaintext hotkey (`PY:wallet.py:169,200` [V]); rtao unlocks
   it (`BT_WALLET_PASSWORD`) and signs a transfer; and the reverse. Produced blobs are committed as cross vectors.
2. btauth/1 both directions, both schemes, every negative case, with the FastAPI and httpx recipes verbatim from
   `signed-requests.mdx` (`RES:D` §8.3).
3. Live subnet: Python v11 miner (FastAPI + `bt.http_auth.verify` + metagraph policy) <-> rtao validator; rtao miner <->
   Python validator (`bt.set_weights`); assertions on weights read back and request logs.
4. Wire parity at the current spec: Python emits call data, signature payloads and fake-signed extrinsics for a
   fixed call set (the `record_golden.py` mode); rtao byte-compares (`RES:H` §3.6 item 3).
5. Metagraph JSON equality at a pinned block; shared runtime-cache files in both directions.
6. CLI parity: `btcli tx <op> --dry-run --json` versus `rtao tx <op> --dry-run --json` (ok, violations, payer, fee
   within tolerance, call bytes) and `btcli query --json` key sets; `btcli` command-tree dump -> `parity/upstream/cli-tree.json`.

### 11.6 Property tests, fuzzing, benchmarks

- `proptest`: `Balance` parsing and same-unit arithmetic; fixed-point rendering versus `bits / 2^frac`;
  `NetUidStorageIndex` round trip; era encoding versus the core's `era_birth`; `conform` invariants (max = 65535,
  no zeros, order preserved, clipped max/sum <= limit, self-weight exemption); btauth payload canonicalisation;
  ss58 round trip; SCALE round trips of models through the registry; scheduler never double-submits inside a rate
  limit under random head sequences.
- `cargo fuzz` (nightly, short runs, SN2's pattern `RES:H` §3.8): btauth `verify`, keyfile parsing, metadata
  decode, extrinsic decode, events decode, selective-metagraph decode.
- `criterion` (baselines committed, nightly, alarm on > 30% regression, `RES:H` §3.8): `Runtime::parse` (V15),
  metagraph typed decode versus `Value` decode at 256/1,024/2,500 uids from recorded payloads, `conform` at 2,500
  weights, btauth sign/verify and layer overhead, 10k storage-key derivations, map-page decode, sign+assemble
  (the core reports 0.03 ms, `RES:A2` §7).

---

## 12. Upstream tracking

### 12.1 Pins and fixtures

- `metadata/<network>/<spec>.v15.scale` and `.v16.scale`, plus `metadata/index.toml` rows
  `{network, spec_version, block_hash, code_hash, sha256_v15, sha256_v16, recorded_at, source}` keyed by
  `(genesis[..16], spec)` like the Python disk cache (`RES:H` §1.1, §3.1). Localnet metadata is stored separately and
  never used for finney bindings (faucet call 60, fast constants; `RES:H` §1.1).
- `fixtures/upstream/MANIFEST.toml` (`upstream_commit`, sha256 per file), `parity/upstream/UPSTREAM.toml`
  (catalog fetch time and `index.json` commit), `BITTENSOR_CORE_TAG` (§1.8), `rtao-testkit/images.toml`
  (localnet image digests).

### 12.2 CI workflows

```yaml
# .github/workflows/pr.yml  (every PR and push to main; offline)
jobs:
  lint:     cargo fmt --check; cargo clippy --workspace --all-targets --all-features -- -D warnings   # includes disallowed-methods
  test:     matrix [ubuntu-latest, ubuntu-24.04-arm, macos-14]: cargo nextest run --workspace --all-features; cargo test --doc --workspace
  msrv:     cargo +1.89 check --workspace --all-features            # = max(subxt-rpcs, monorepo toolchain at the pin)
  features: cargo hack check --workspace --feature-powerset --depth 2
  supply:   cargo deny check (duplicate bans: parity-scale-codec, scale-info, scale-decode, scale-encode, scale-value,
            frame-metadata, frame-decode, jsonrpsee); cargo audit (documented ignores, like upstream's cargo-audit.yml)
  codegen:  cargo xtask codegen --check; cargo xtask docs --check
  parity:   cargo nextest run -p rtao-parity-tests                  # §4.7
  semver:   cargo semver-checks --baseline-rev "$(git describe --tags --abbrev=0 --match 'rtao-v*')"
  cache:    actions/cache on ~/.cargo/registry, ~/.cargo/git (monorepo + polkadot-sdk fork), target/
```

```yaml
# .github/workflows/drift.yml
on: { schedule: [{ cron: "17 */6 * * *" }], workflow_dispatch: {} }    # upstream smoke cadence (RES:H §3.3)
jobs:
  drift:
    strategy: { fail-fast: false, matrix: { network: [finney, testnet, devnet] } }
    steps:
      - cargo xtask drift --network ${{ matrix.network }} --open-pr
        # finalized head -> state_getRuntimeVersion -> state_getStorageHash(":code") vs metadata/index.toml;
        # on change: fetch V15+V16 -> semantic diff (pallet/call/event/error indices and names, call arg idents,
        # storage hashers/types/defaults, constant values, runtime-API versions and signatures, extension
        # identifiers and types, extrinsic versions) + `subxt diff` attached -> codegen -> offline tests ->
        # PR drift/<network> labelled none|additive|behavioural|breaking; for finney also verify the release
        # subtensor-digest.json blake2_256 against the :code hash (RES:H §3.3 step 3)
```

| Workflow | Trigger | Does |
|---|---|---|
| `catalog.yml` | nightly | `cargo xtask catalog sync`: fetch bittensor.com `catalog/{intents,reads,errors}.json` and `code/index.json`; regex-scan monorepo `testnet`/`main` Python registries; PR updating `parity/upstream/` with new names as `planned` rows (§4.7) |
| `core-pin.yml` | finney drift or weekly | when finney's finalized spec N > pin and release vN is final: `cargo xtask core bump --tag vN` -> PR with `upstream-diff` of chain-defined core files; `core-next` lane builds against monorepo `main` weekly (allowed failure) |
| `watch.yml` | daily | sha256 of watched upstream files on `main` and `testnet` versus `fixtures/upstream/WATCH.toml`; change -> issue with diff. Watched: `PY:http_auth.py`, `docs/guides/signed-requests.mdx`, `PY:error_map.py`, `PY:hyperparams.py`, `PY:intents/hyperparameters.py`, `PY:fee_filters.py`, `PY:settings.py`, `PY:_transport/{runtime,rpc}.py`, `common/src/transaction_error.rs`, `runtime/src/{fee_filters,check_mortality}.rs`, `pallets/subtensor/src/rpc_info/metagraph.rs`, `pallets/subtensor/src/coinbase/{reveal_commits,run_coinbase}.rs`, `eco-tests/**` (indexer contract, early warning for metagraph shapes, `RES:H` §1.8), `scripts/localnet.sh`, `Dockerfile-localnet` |
| `e2e.yml` | nightly + `e2e` label | localnet matrix (§11.4) |
| `interop.yml` | nightly + `interop` label | compose lane (§11.5) |
| `probe.yml` | every 6 h | read-only finney/testnet: finality lag < 10 min, spec, `:code` hash, V15 digest versus pin, metagraph decode of netuid 1 and the largest subnet, events of the last 10 blocks (`RES:H` §3.10) |
| `release.yml` | merge to `main` / tag `rtao-v*` | release-plz PR (changelog, versions); on tag: `rtao` CLI binaries via cargo-dist (linux x86_64/aarch64, macOS arm64), `compat.json`, book deploy |

### 12.3 Compat matrix and runtime guard

`COMPAT.md` and `compat.json`, one row per rtao release: validated spec range, metadata pins, bittensor-core tag,
localnet image digests, `subxt-rpcs`/`jsonrpsee` versions, Python `bittensor` version interop-tested, btauth
version. "Validated" means the drift from lowest to highest pinned spec has no breaking class and e2e passed on
the listed digests (`RES:H` §3.9). At runtime a spec outside the validated range only logs a warning; surface verification (§3.6.4) decides per call, because most bumps are behaviour-only (`RES:CORRECTIONS.md` item 2).

### 12.4 Release process

Git tags `rtao-v<semver>` (no crates.io); per-crate versions and changelogs by release-plz; `cargo-semver-checks`
against the previous tag; a release whenever a drift or core-pin PR merges or features land; GitHub release notes
carry the compat row and CLI binaries.

### 12.5 Maintenance playbook

| Upstream event | Detected by | Action | Effort [I] |
|---|---|---|---|
| runtime bump, surface unchanged (the common case) | `drift.yml` | PR adds metadata files + index row; e2e on the new image; merge | ~1 h |
| runtime bump, additive | `drift.yml` | regenerate; new calls/reads become `planned` rows | 0.5 day |
| runtime bump, behavioural (event index move, storage type/default change, API version) | `drift.yml` | adjust models/tests (events are name-matched, so usually nothing) | 0.5-1 day |
| runtime bump, breaking (used call's args, extension with payload, transaction version) | `drift.yml` on devnet, days before finney (`RES:H` §2.1); connect guard | fix generated surface; extension payloads arrive with the core tag bump; major bump of `rtao-runtime` | 1-3 days |
| new bittensor-core tag | `core-pin.yml` | review `upstream-diff`, run oracles and corpus, bump | 0.5-2 days |
| Python adds an intent/read/error | `catalog.yml` | implement from the upstream source and catalog schema | 0.5-1 day per intent, 1-2 h per read (`RES:H` §4(b)) |
| btauth change | `watch.yml` | implement `btauth/2` side by side, keep v1 through the deprecation window | 0.5-1 day |
| `subxt-rpcs`/`jsonrpsee` breaking release | Renovate/Dependabot weekly | confined to `rtao-rpc` | 0.5-2 days |
| metadata V17 appears | `drift.yml` alert | nothing until adopted (rtao requests 15 and 16 explicitly) | - |
| localnet image policy change | `e2e.yml` failures | update `images.toml` | minutes |
| polkadot-sdk fork bump | arrives with a core tag | `deny.toml` duplicate bans catch SCALE-stack splits | varies |

---

## 13. Documentation

- **rustdoc**: `#![deny(missing_docs)]`, crate docs from `README.md`, feature-gated items marked with
  `doc(cfg)`; `cargo doc` for the whole workspace published to GitHub Pages (no docs.rs without crates.io).
- **docify** (`docify 0.4.1`): every example in rustdoc, README and the book is an `#[docify::export]`ed function in
  `examples/` or a test; `docify::embed_run!` for examples that run offline (keys, SCALE, btauth vectors, `conform`,
  `Balance`), `docify::embed!` for ones that need a node; README and book pages are compiled from `*.docify.md`
  sources with `docify::compile_markdown!`, so examples cannot rot (`RES:F` §7).
- **Book** (mdbook 0.5.4), mirroring bittensor.com's taxonomy so agents can navigate by analogy (`RES:I` §5.6):
  `quickstart`, `sdk`, `agents`, `cli`, `concepts` (links to upstream raw markdown, not copies), `guides`
  (validator, miner, owner, local development, signed requests, timelock, wallets and signers, deployment,
  observability, upgrades and compatibility, testing your subnet), `migration` (Python v10 -> rtao, Python v11 ->
  rtao, bittensor-core -> rtao, including §4.8's divergences), `reference` (generated `tx/<op>`, `query/<read>`,
  `errors/<code>`, `errors/chain/<Name>`, `hyperparameters/<name>`, using bittensor.com's page sections with `## Rust` in place of `## Python`, `RES:I` §5.2), regenerated by `cargo xtask docs` with a `--check` CI gate.
- **Catalogs and llms.txt**: rtao publishes `catalog/{intents,reads,errors}.json` in the upstream schema plus a
  `rust` object per entry, and `llms.txt`/`llms-full.txt` with bittensor.com's conventions (grep instructions,
  raw-markdown URLs, catalog URLs) plus `rtao tools`, the `cargo doc` site, and a "mapping to bittensor.com" block
  pairing each `tx/<op>` with its upstream page (`RES:I` §5.1, §5.6). The naming map is mechanical: catalog `name`
  -> `IntentMeta.op`, `python_class` -> the same Rust struct name, read `name` -> namespace method, codes ->
  `ErrorCode` variants (`RES:I` §5.6 (d)).
- **Templates**: `examples/validator` (EMA scoring + weight scheduler), `examples/miner` (axum + btauth + policy),
  `examples/owner` (create, start, hyperparameters through the planner), `examples/scripts` (blocking facade).

---

## 14. Milestones

Effort is in engineer-weeks (EW) for an engineer fluent in Substrate and Rust [I: estimates]. Every milestone
ends with green CI on all lanes it introduces.

| # | Milestone | Deliverables | Exit criteria | EW |
|---|---|---|---|---|
| M0 | Foundations, harness, first typed read | workspace, pins, `clippy.toml`/`deny.toml`; `rtao-btcore` (re-exports, epoch shim + vectors); `rtao-primitives` core types; `rtao-rpc` session; `rtao-client` connect, runtime cache (Python-compatible disk cache), snapshots, typed storage/API/constants, lazy events; xtask `metadata fetch`, `codegen` (subset used by M1), `drift` prototype; models `MetagraphWire`, `SelectiveMetagraphWire`, `HyperparamEntry`; golden conformance; `FakeChain` v0; `Localnet` v0; `pr.yml` | `cargo run --example metagraph -- --network finney --netuid 1` prints a typed metagraph at a pinned block; golden tests green; nightly drift runs on finney/testnet/devnet with an empty report | 3 |
| M1 | Validator | `KeypairSigner`; wallet hotkey + coldkey unlock (password sources 1-4, 7); tx pipeline (nonce, era, fee, watch, outcome, reconciliation); `conform`, `TimelockCommitter`, mechanisms, Null raw u16; `EpochClock`; `MetagraphSync`; `rtao-auth`; `BtAuthClient`; `rtao-neuron` validator runtime (scheduler, state, telemetry, shutdown) | sample validator: 50 fast-localnet epochs with a commit per epoch and every reveal observed; 24 h on a testnet subnet with no missed reveal attributable to rtao; SIGTERM during a commit reconciles definitively (scenario 6) | 5 |
| M2 | Miner | `BtAuthLayer`, `MetagraphPolicy`, `PerHotkeyLimiter`, Redis store; `AxonPublisher`; `PruneRisk`; miner runtime; Python interop lane (btauth, wallets, live subnet) | Python validator <-> rtao miner and rtao validator <-> Python miner pass in CI; `serve_axon` never resubmitted when unchanged | 3 |
| M3 | Operational safety, owner tooling, CLI v0 | `rtao-tx` (macro, executor, policy, plan, results, error tables + gates); intents: registration (burned/PoW/root), owner group, serving, weights, transfer; shielded submission; `HyperparamPlanner`, `OwnerMonitor`; CLI v0 (§10.3); parity manifest seeded with every upstream name | owner scenario 9 and shielded scenario 10 green; `rtao tx … --dry-run --json` matches `btcli` for the M3 op set | 5 |
| M4 | Reads parity | all 102 reads, 14 namespaces, `READ_SAMPLES`, remaining models (stake, delegation, baskets, locks/conviction in fixed point, collateral, leases, crowdloans, commitments); `blocking` facade; CLI `query` complete | zero `planned` reads; interop read/metagraph JSON key-set equality | 4 |
| M5 | Intents parity | staking (limits, `all`, claim batches), children/take, coldkey swap, collateral, locks, proxy (pure, announced), multisig (adapters, funds preflight), crowdloan, leasing, association, EVM key association/withdraw, basket, root claim (+ fee preview), batch; `prepare_call`/`submit_signature`; CLI `tx` complete | zero `planned` intents; the upstream intent-plan inventory mirrored and green | 6 |
| M6 | Signers and wallets parity | Ledger (metadata-hash path, proof vector), macOS keychain and dialog, keyring, Vault, extension bridge, remote signer; wallet create/regen/sign/verify/encrypt/decrypt/associate | Ledger signs on localnet (hardware-in-the-loop checklist), Vault and extension flows demonstrated; wallet CLI parity | 4 |
| M7 | CLI hand-written groups, EVM | remaining v11 command groups, `--json` output parity, completions, `rtao-evm` | `cli_tree_matches` with zero `planned` rows | 6 |
| M8 | Legacy networking and extras | `rtao-compat-synapse`, `rtao-compat-epistula`, `rtao-transport-quic` (after the dependency check), v10 bridge, `import-npz` | interop with a `bittensor==10.5.0` Synapse peer and with btlightning's Python bindings | 3 |
| M9 | Hardening, 1.0 | fuzz in nightly, benchmark baselines, weekly mainnet-clone read-only lane (`RES:H` §1.4, §3.5), complete book/llms, compat automation, security review of signers and keyfiles | `rtao-v1.0.0` with a compat row; book published | 3 |
| | **Total** | | | **~42** |

Ordering: M0, then M1, then M2 and M3 in parallel, M4 and M5 in parallel, M6 and M7 in parallel, then M8 and M9. With two engineers the critical path is
M0+M1+M3+M5+M7+M9, about 28 EW (6-7 months) [I]. Operator value arrives early: a working validator at the end of M1, a miner at M2, owner tooling with safety rails at M3.

Parity scale check: the hand-written v11 Python layer is about 52k lines: 12,805 in top-level modules plus
39,865 in `intents/`, `reads/`, `_transport/`, `cli/`, `evm/`, `vault/`, `extension/` [V: `wc -l` over
`PY:`], of which the CLI alone is the largest share. Generated `tx`/`query` commands and the shared registries are
what keep M4-M7 inside the estimate.

---

## 15. Risks and open questions

### 15.1 Risks

| # | Risk | Likelihood / impact [I] | Mitigation |
|---|---|---|---|
| R1 | bittensor-core is internal and unversioned (`publish = false`, version never stamped, API shaped by Python needs; its spec lists a Rust transport and intent layer as non-goals, `RES:B` §1, §7), so tag bumps can break rtao | medium / medium | one bridge crate; `upstream-diff` in every bump PR; oracle tests; proposals U1-U8; fallback of vendoring individual modules with their fixtures |
| R2 | heavy build: 392 crates with `host`, C toolchain (libsodium, vendored OpenSSL needing Perl, secp256k1-sys, hidapi), git fetch of the monorepo and the polkadot-sdk fork (`RES:feasibility-veneer-coexistence.md`, `RES:B` §6) | high / low-medium | CI caches incl. `~/.cargo/git`, `net.git-fetch-with-cli`, builder image, upstream U4 |
| R3 | a core bump splits the SCALE stack, so `DecodeAsType` over `Runtime.types` stops type-checking | low / high | `deny.toml` duplicate bans; `core-next` lane; temporary fallback to `Value` decode for affected models |
| R4 | reveal prediction errors (safety-net predicate, unmodelled `MaxEpochsPerBlock` deferral, local clock skew) cause missed reveals | medium / high for validators | epoch shim with the pallet predicate; skew check; reveal watcher metric; `RealTime` e2e; U1/U2 |
| R5 | a custom transaction extension gains a payload | low / high (no signing) | connect guard; devnet drift gives days of lead; extension support arrives through the core tag |
| R6 | event indices move | certain / none | name-based matching only |
| R7 | public endpoint quotas (~1 r/s) and per-endpoint RPC differences | high / medium | token bucket, runtime APIs over scans, own-node guidance, per-endpoint probes |
| R8 | parity long tail (~52k lines of Python) overruns | medium / medium | generation from registries; manifest makes the gap explicit; operator-critical scope first |
| R9 | btauth/1 adoption is zero among surveyed subnets (`RES:D` §6) | high / medium | compat crates in M8; v10 bridge feature |
| R10 | btlightning pulls crates.io `sp-core 34` next to the fork's `sp-core 37` | unknown / low | M8 entry check; fallback: reimplement its MIT-licensed framing without sp-core |
| R11 | localnet differs from mainnet (faucet call, fast constants, patched rate limits; `RES:H` §1.1, §6) | certain / medium | never pin finney bindings from localnet; `RealTime` profile; weekly mainnet-clone read-only lane |
| R12 | licensing: rtao is MIT, ported/vendored upstream material is Apache-2.0 (`SUB:LICENSE` [V]) | low / low | NOTICE file, per-file headers on the epoch shim, vendored Vault/extension assets and fixtures |
| R13 | accepting `<Bytes>`-wrapped btauth signatures by default (parity) disagrees with `RES:D` §1.3 | low / low | configurable; revisit if Python drops the fallback |
| R14 | keyfile crypto inside the core relies on unmaintained `sodiumoxide` (`RES:E` §4.2 item 10) | low / medium | upstream U4 (pure RustCrypto NaCl); rtao's cross vectors detect any regression |

### 15.2 Open questions

1. Extrinsic v5: will the runtime or the Python SDK move to v5 general transactions (finney already advertises
   `[4, 5]`)? rtao signs v4 because the core does (`RES:A1` Q2, `RES:H` §8 Q5).
2. Real public-endpoint quotas, to set the default rate limiter from measurement rather than the documented "about
   one request per second".
3. Will RaoFoundation accept U1-U8, especially the `host` split (U4) and metadata release assets (U7)?
4. Binary name: `rtao` (default) or optionally `btcli`, which collides with Python's `btcli` on `PATH`?
5. Should the validator runtime offer "follow the chain's `WeightsVersionKey`" (convenient, but it defeats the
   owner's forced-upgrade lever)? This design says no.
6. Will `subtensor-localnet:v<spec>` images keep appearing for every finalized release (true since v473, `RES:H`
   §8 Q1)?
7. `SelectiveMetagraphIndex` is only in pallet source (`RES:A2` Q4); would upstream expose it through a constant or
   runtime API?
8. The intended unit of `total_stake` (declared `TaoBalance`, alpha-equivalent in practice; `RES:A2` Q2).
9. Ledger in CI: self-hosted hardware runner or a manual release checklist?
10. Can the queued `register_network` path be exercised on localnet (it needs `SubnetLimit` reached or lowered; a
    root setter for it was not verified in this design)?
11. State-file stability across rtao major versions: is a JSON envelope with `schema_version` enough, or do
    validators need migration hooks?

---

## Appendix A. Evidence consulted for this design

Read in this session [V]: `RES:CORRECTIONS.md`; `RES:feasibility-{veneer-coexistence,subxt,bittensor-core-gitdep}.md`;
of the eleven research reports, A1, A2, B, D, G and H in full, C1 (§0, §2-§4, §6-§10), C2 (§0, §1.0-§1.1,
§3-§9), E (§0, §1, §4-§7), F (§0-§11) and I (§0, §2-§5, §7); `CORE:lib.rs`, `CORE:../Cargo.toml`, `CORE:keys/mod.rs` (public API),
`CORE:keyfiles/mod.rs:29-140`, `CORE:timelock/{mod.rs:40-200, epoch_schedule.rs:1-145, constants.rs}`,
`CORE:mlkem/mod.rs:30-110`, `CORE:digest/mod.rs` (API), `CORE:pow/mod.rs` (API), `CORE:signers/ledger.rs` (API),
`CORE:runtime/mod.rs:130-200`, `CORE:codec/{encode.rs:525-590, extrinsic.rs:15-125,210-320,400-442}`,
`CORE:client.rs` (API, `:30-33`, `:618-656`), `CORE:transaction.rs:1-250,660-720,820-1130`,
`SUB:sdk/bittensor-core/tests/e2e_support/mod.rs:15-140`, `SUB:pallets/subtensor/src/coinbase/run_coinbase.rs:1217-1277`,
`SUB:runtime/src/check_mortality.rs:15-25,88-104`, `SUB:rust-toolchain.toml`, `SUB:LICENSE` (header),
`SUB:docs/concepts/network.mdx:20-40`, `SUB:docs/guides/local-development.mdx` (rate-limit line 160),
`PY:intents/base.py:100-250`, `PY:wallets.py:60-120`, `PY:keyfiles.py:65-110`, `PY:hyperparams.py:55-135`,
`PY:fee_filters.py:1-40`, `PY:config.py:28-60`, `PY:_transport/runtime.py:30-125`, `PY:cli/tx.py:1-60`,
`PY:cli/query.py:1-40`, `PY:client.py` (method list), `PY:namespaces.py` (class list), `PY:vault/server.py:1-30`,
`PY:_live.py`, `PY:basket_index.py` (header), the package tree and line counts; `CAT:intents.json`,
`CAT:reads.json`, `CAT:errors.json` (counts, distributions, `set_weights` schema); `META:explore/AdminUtils_calls.txt`;
`FEAS:core-plus-subxt/{Cargo.toml,src/main.rs}`; `SN2:sn2-chain/src/{lib.rs:25-120, weights.rs:1-185}`;
`UP:inference-labs-inc_btlightning/crates/btlightning/Cargo.toml`;
`REG:subxt-rpcs-0.51.1/{Cargo.toml, src/client/rpc_client_t.rs, src/client/round_robin_rpc_client.rs, src/methods/legacy.rs}`,
`REG:scale-decode-derive-0.16.2/src/lib.rs`, `REG:scale-decode-0.16.2/src/{impls/mod.rs:990-1060, visitor/types/{variant,composite}.rs}`,
`REG:scale-type-resolver-0.2.0/src/portable_registry.rs:59`, `REG:scale-decode-0.16.2/Cargo.toml`; the target repo
the rtao repository (`Cargo.toml`, tree); live, read-only: `state_getMetadata` and
`state_call Metadata_metadata_at_version(0x0f000000)` against `https://entrypoint-finney.opentensor.ai` (version
bytes and sizes quoted in §1.4).
