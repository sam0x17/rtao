# A2 — v11 Python SDK read side, Metagraph model, and code generation

Research report for the **rtao** plan (Rust-only Bittensor subnet SDK). Written 2026-10-13 against the on-disk monorepo snapshot `RaoFoundation/subtensor@850f275` (runtime spec_version 478 in source; finney/testnet metadata at spec 477).

## 0. Path conventions, scope, headline findings

**Path conventions used in citations**

- Unprefixed paths are relative to the monorepo root `SP/upstream/subtensor/`.
- `SDK/` = `sdk/python/bittensor/` (so `SDK/reads/base.py` = `sdk/python/bittensor/reads/base.py`).
- `CORE/` = `sdk/bittensor-core/src/` (the Rust core crate).
- `META/` = `SP/meta/` (finney V16 metadata dumps, spec 477).
- `LEGACY/` = `SP/upstream/bittensor/` (archived v10.5 Python SDK).
- `DOCSCAT/` = `SP/docs/` (reads.json etc.).

Everything below was read directly from those files unless marked **(inferred)**. No cargo/python was run inside the untrusted trees; the only tool run against them was `subxt explore --file=META/finney.scale`.

**Headline findings (the ten things that change rtao's design the most)**

1. The v11 read side is a **registry of 102 named async reads** (`SDK/reads/*.py`, 15 modules) written against a tiny "view" surface — `query` / `query_map` / `query_batch` / `runtime` / `constant` / `balance` / `at` / `timestamp` / `block_time` — so the same function runs at the chain head (`Client`) or pinned to a block (`Snapshot`) with zero per-read plumbing (`SDK/reads/base.py:1-20`, `SDK/snapshot.py:25-116`). The inventory in §3 covers all 102 (counted from source and matched 1:1 against `DOCSCAT/reads.json`).
2. **Generated descriptors are name-based and carry no type ids, hashers or key types** — a storage descriptor is `Item('SubtensorModule','Tempo','u16')` (container, name, value-type *ident*), a runtime API descriptor is `Method('SubnetInfoRuntimeApi','get_metagraph')` (`SDK/_generated/storage.py:10-21`, `SDK/_generated/runtime_apis.py:10-15`). Hashers, key types, value type ids, default bytes and runtime-API in/out types are resolved **at runtime from the live metadata** by the Rust core (`CORE/runtime/mod.rs:26-40,82-100,369-408,483-504`). This is the explicit design decision of the core spec §4.2: "ids are not stable across spec versions, so they never leave the process" (`sdk/bittensor-core-spec.md:129-137`).
3. The **drift gate is byte-exact regeneration**: `codegen.check --drift <ws>` re-dumps the IR from a node and compares every file in `_generated/` as text, plus flags orphan files (`sdk/python/codegen/check.py:32-53`). Generated headers deliberately omit `spec_version` so a spec bump with no surface change does not fail the gate (`sdk/python/codegen/emit_python.py:22-30`). CI runs the offline gates on every PR and the drift gate against a sudo-upgraded mainnet clone (`.github/workflows/runtime-checks.yml:529-540`, `.github/workflows/mainnet-clone-preview.yml:166`, `docs/internals/sdk-tests.mdx:85-113`).
4. The **Metagraph** is one runtime-API call (`SubnetInfoRuntimeApi.get_metagraph`) plus, by default, two Commitments-pallet map scans, all pinned to one block (`SDK/metagraph.py:238-255`). It has **no weights/bonds, no persistence, no incremental sync** — "refetch" is the sync strategy and `save()/load()` are gone (`docs/migration.mdx:290-317`). `rank`, `trust` and `pruning_score` are **deprecated on chain and always empty** in the runtime response (`pallets/subtensor/src/rpc_info/metagraph.rs:782,803-804`), so v11 renders them as `0.0`.
5. The runtime's wire struct `Metagraph<AccountId>` has 77 selectable fields (`SelectiveMetagraphIndex` 0..=76, `pallets/subtensor/src/rpc_info/metagraph.rs:488-650`) and the metadata gives you only the struct shape, not the index numbering — a Rust port must copy that enum from the pallet source (or from `LEGACY/bittensor/core/chain_data/metagraph_info.py:439-514`, which stops at 73).
6. **Unit conventions are the thing metadata cannot express** and the SDK keeps them in exactly three places: `Balance` (rao ints tagged by netuid; `SDK/balance.py`), `hyperparams.py` (hand table of unit kinds + `RATIO_TYPE_DENOMINATORS` keyed on the metadata's `PerU16`/`Percent`/`Permill`/`Perbill` identities), and inline fixed-point decoders (`U64F64` = bits/2^64, `I96F32` = bits/2^32, `I64F64` = bits/2^64, `I32F32` integer part = bits//2^32). §2 collects every convention found, with citations.
7. The post-newtype runtime **declares most units as type paths** (`TaoBalance`, `AlphaBalance`, `NetUid`, `MechId`, `NetUidStorageIndex`, `PerU16`) — visible in the storage descriptors (`SDK/_generated/storage.py:91-98,105-107,238-241`) and in `subxt explore` output — so a typed Rust SDK can get unit safety from metadata-driven codegen for free where the Python SDK had to hand-table it; the residual hand table is blocks / epochs / per-million / u64-normalized / bool kinds (`SDK/hyperparams.py:463-471`).
8. **Test fixtures a Rust port can reuse today**: `sdk/python/tests/fixtures/golden.json` (1.5 MB, recorded on a spec-461 localnet; embeds the raw V15 and V14 metadata blobs so everything replays offline) and `sdk/python/tests/fixtures/shape_corpus/corpus.json` (1.08 MB; 2,899 `(type id, SCALE bytes, decoded value)` samples over 797 of 802 registry types). The Rust core already replays both natively (`CORE/codec/mod.rs:115,209,284,313,370,429,476,492`), which is the pattern rtao should copy.
9. **Performance baselines exist**: the core spec's acceptance table records codec build 337 ms → 4 ms, SCALE decode of metagraph/neurons payloads 3–6 MB/s → 60–77 MB/s, `query_map` decode 92k → 599k entries/s, warm connect ~1.1 s → ~0.6 s (`sdk/bittensor-core-spec.md:360-382`). Scripts to re-measure: `sdk/python/scripts/bench_transport.py`, `bench_neurons.py`.
10. **A native Rust client already exists in the monorepo** (`CORE/client.rs`, 1,770 lines; blocking `reqwest`, HTTP JSON-RPC, `publish = false`) with `query`/`query_batch`/`query_map`/`runtime_call`/`at()`/`metagraph()`/`neurons()`/`subnet_hyperparameters()` and a 20-entry `read_catalog` (`CORE/client.rs:138-260,357-500,656-780,1073-1110`). It is sync and reqwest-only (no tokio), so it is a reference, not a base, for a tokio-first rtao.

---

## 1. The three levels of reading chain state, views, and block pinning

### 1.1 Levels (as documented and implemented)

`docs/concepts/client.mdx:36-75` names three levels; the code backs each:

| Level | Surface | Implementation |
|---|---|---|
| Typed namespaces | `client.subnets.metagraph(1)`, `client.balances.get(addr)`, ... one namespace per read category (14 of them) | `SDK/namespaces.py:52-97` (`_ReadNamespace.__getattr__` resolves any registry read in its category and appends a keyword-only `block=` pin unless the read already has a `block` param), curated methods at `:99-213`, `NAMESPACES` map at `:279-294`; type stub `SDK/namespaces.pyi` is generated (`sdk/python/codegen/emit_namespaces.py`) |
| Named reads | `await client.read("delegate_take", hotkey_ss58=...)`, `client.reads()` catalog | `SDK/reads/base.py:131-153` (`dispatch`, `list_reads`), `SDK/client.py:273-283` |
| Generic accessors | `client.query(st.SubtensorModule.Tempo, [1])`, `query_map`, `query_batch`, `runtime`, `constant`, `decode_scale` | `SDK/client.py:209-271`; descriptors are any `(container, name)` 2-tuple, generated or hand-written (`SDK/client.py:63-66,212-217`) |

The CLI's `btcli query <name>` is generated from the same registry: one subcommand per read, options derived from `params`, table rendering driven by `render` hints (`SDK/cli/query.py:1-6,135-260`).

### 1.2 The read registry

- `@read(name, params, *, category, param_docs, render)` registers a `ReadSpec(name, doc, params, fetch, category, param_docs, render)`; duplicate names and `param_docs` for unknown params raise at import (`SDK/reads/base.py:57-105`).
- `params` is a `{name: json-type}` dict with the vocabulary `string | integer | number | boolean | array` (used by the CLI option builder `SDK/cli/query.py:27` and by the agent catalog).
- `*_ss58` params are auto-coerced from wallet/keypair/signer objects via `signing.coerce_address` (`SDK/reads/base.py:108-128`).
- Render hints: `Grouped(key)` for `{group: [record]}` results and `Matrix(row, col, value)` for `{row: {col: value}}` (`SDK/reads/base.py:33-54`); used only by `weights`, `bonds`, `timelocked_weight_commits`.
- `scalar_read(name, item, per_netuid, doc, category)` is a one-liner factory for "one storage value + `int()`" reads (`SDK/reads/base.py:156-166`); `hyperparameters.py` and two chain reads use it.
- `utf8_text()` decodes chain byte strings (hex / bytes / list-of-ints such as compact-u8 vectors) to text, falling back to `0x`-hex (`SDK/reads/base.py:169-185`).
- `list_reads()` emits `{name, summary, description, params, param_docs, category, docs_url}` per read (`SDK/reads/base.py:140-153`); the docs generator enriches it into `DOCSCAT/reads.json` with `markdown_url`, `python` (namespace call form) and `hits` (`website/apps/bittensor-website/scripts/generate.py:1240-1250`).

### 1.3 Views and block pinning semantics

- A **view** is anything exposing `query / query_map / query_batch / runtime / constant / balance / at / timestamp / block_time` (`SDK/reads/base.py:4-9`). `Client` (head) and `Snapshot` (pinned) are both views.
- `client.at(block=None)` resolves the head number once and returns `Snapshot(client, block)`; every snapshot accessor forwards with `block=self.block` (`SDK/client.py:650-658`, `SDK/snapshot.py:50-64`). `Snapshot.at()` returns itself (or a sibling for another block) (`SDK/snapshot.py:108-112`).
- **The idiom** `view = await view.at()` at the top of a read pins all subsequent calls to one block when the read is invoked on a `Client`, and is a no-op on a `Snapshot` (`SDK/reads/base.py:11-13`). Reads that do this: `timestamp`, `blocks_until_next_epoch`, `blocks_since_last_update`, `epoch_status`, `subnet_collateral`, `collateral_policy`, `subnet_convictions`, `stake_value_for_coldkey(s)`, `validator_basket_summary`, `root_baskets`, `subnet`, `subnets`, `subnet_start_schedule`, `metagraph.fetch`, `metagraph.fetch_commitments` (cited per row in §3). Reads that gather several head calls **without** pinning (so they can straddle a block): `delegate_take` (3 RPCs), `coldkey_swap_announcement` (2), `hotkey_identities` (N+M sequential), `miner_collateral` (Owner then MinerCollateral).
- Block number → hash: `Client._block_hash` calls `substrate.block_hash(block)`; the transport caches number↔hash only for blocks at or below the finalized head (re-probed at most every 12 s) so a reorg cannot serve a stale fork hash (`SDK/_transport/interface.py:187-230`).
- `constant()` is never pinned — constants are read from the head codec's metadata (`SDK/snapshot.py:66-68`, `SDK/_transport/interface.py:393-395`).
- `query_map` without a block pins keys and values to one `chain_getHead` hash per call (so a key deleted between `state_getKeysPaged` and `state_queryStorageAt` cannot yield `None`), but two separate `query_map` calls are not mutually pinned unless the read used `at()` (`SDK/_transport/interface.py:347-351`). `query_batch` likewise pins to head when no hash is given (`:266-267`).
- Archive fallback: a read that hits `StateDiscardedError` ("State already discarded for ...") is transparently retried on an archive pool (`SDK/_substrate.py:383-400`, `SDK/_transport/rpc.py:202-209`, `SDK/settings.py:69-77`).

### 1.4 Transport mechanics behind the generic accessors

| Accessor | RPC(s) | Decode path |
|---|---|---|
| `query(module, item, params, block_hash)` | `state_getStorageAt(key, hash)` — note the `At` variant, with `None` hash meaning head | key = `twox128(prefix) ++ twox128(name) ++ hasher(param)...` built in the core (`CORE/codec/storage.rs:59-94`); miss semantics: `Default` modifier → decode `default_bytes`, `Optional` → `Option<T>` → `None` (`SDK/_transport/storage.py:8-27`, `codec.py:140-144`) |
| `query_batch(module, item, params_list, hash)` | one `state_queryStorageAt(keys, hash)` | keys built in one core crossing (`storage_key_batch`), values bulk-decoded (`SDK/_transport/interface.py:256-278`) |
| `query_map(module, item, params, hash, page_size=100)` | `state_getKeysPaged(prefix, page_size, start_key, hash)` + `state_queryStorageAt(keys, hash)` per page, iterated lazily by `QueryMapResult` | map-key recovery from the key's trailing bytes for reversible hashers (`Blake2_128Concat` 16-byte prefix, `Twox64Concat` 8, `Identity` 0) (`CORE/codec/storage.rs:46-57,96-164`); `RpcSubstrate.query_map` materializes the whole map into a list and unwraps 1-tuples (`SDK/_substrate.py:485-498,847-851`) |
| `runtime(api, method, params, hash)` | `state_call("Api_method", hex(params), hash)` | V15 `runtime_api_map` gives input `(name, type_string)` pairs and output type; params may be a list (positional) or dict (by name); legacy path for V14 runtimes or methods whose declared output is `Vec<u8>` (double-encoded results; frozen table `LEGACY_RUNTIME_APIS`) (`SDK/_transport/runtime_api.py:95-243`) |
| `constant(module, name)` | none | decoded from metadata (`SDK/_transport/codec.py:323-325`) |
| `decode_scale(type_string, data)` | none | type-string grammar: `scale_info::N`, registry names, `Vec<T>`, `Option<T>`, `[T; N]`, tuples, `Compact<T>`, `Bytes`, `AccountId`, `Era`, `Call` (`CORE/runtime/type_string.rs:1-153`) |

Runtime/codec resolution (`SDK/_transport/runtime.py`): block hash → `chain_getHeader` parent hash → `state_getRuntimeVersion(parent)` → spec version → `RuntimeCodec` (LRU of 4 codecs, 512 block→spec entries) (`:123-198`). Head codec is re-validated against `state_getRuntimeVersion` at most every 12 s (`:142-176`). Metadata is fetched as **V15** via `state_call("Metadata_metadata_at_version", 0x0f000000)` with `state_getMetadata` (V14) fallback (`:292-310`), and cached on disk at `~/.bittensor/runtime-cache/md1-<genesis16>-<spec>.bin` (magic `STMD` + `<IB` header; max 8 files) (`:36-121`). `transactionVersion` missing is a hard error (`:42-54`).

Connection resilience: single websocket with request correlation, auto-resubmit of pending idempotent frames on reconnect (never `author_submit*`), subscription failure push, endpoint pool rotation, policy-refusal (HTTP 403 / JSON-RPC -32004/-32005 / WS 1008) treated as terminal (`SDK/_transport/rpc.py:1-70,190-240`). Default fallback and archive pools are in `SDK/settings.py:56-77`.

Connect-time side effect relevant to units: `Client.connect()` loads the `TokenSymbol` map (one `query_map`) through a 7-day disk cache and installs it as `substrate.token_symbols`, so `view.balance(rao, netuid)` tags alpha balances with the subnet's real symbol (`SDK/client.py:613-633`, `SDK/config.py:22-27`, `SDK/_substrate.py:326-335`).

### 1.5 Sync facade and the dual-mode entry point

`bt.Subtensor` is a `SyncClient` (private event loop on a daemon thread, every coroutine proxied) that also implements `__await__` / `async with` to hand back the async `Client`; the first use locks the mode (`SDK/_subtensor.py:44-118`, `SDK/sync.py:1-11`). `bt.set_weights(...)` is a module-level blocking helper that reuses one shared client per endpoint (`SDK/_subtensor.py:121-205`). For rtao this is informational only (tokio-first means no such facade), but the fact that **there is exactly one async implementation** and the sync layer is a mechanical proxy is the pattern to keep.

---

## 2. Unit and scale conventions (every one found)

| Quantity | Wire type / storage | Convention | Citation |
|---|---|---|---|
| TAO | `TaoBalance(u64)` | rao integers; `RAO_PER_TAO = 10**9`; `Balance(rao, netuid=0)` | `SDK/settings.py:15`, `SDK/balance.py:44-57` |
| Subnet alpha | `AlphaBalance(u64)` | rao-scale integers in the subnet's own currency; `Balance(rao, netuid)`; mixing netuids raises `UnitMismatchError`; `.tao` raises on alpha, `.alpha` raises on TAO, `.amount` is unit-agnostic | `SDK/balance.py:1-25,92-116,139-145` |
| Display symbol | `TokenSymbol[netuid]` (`Vec<u8>`) | tagged at decode time via `view.balance(rao, netuid)`; fallback `α` + subscript netuid | `SDK/balance.py:127-137`, `SDK/_substrate.py:326-335` |
| Exact serialization | — | `Balance.decimal` (Decimal) never loses precision; `from_tao` accepts str/Decimal for >~9M TAO | `SDK/balance.py:59-69,118-125` |
| Scores: incentive, dividends, consensus, (validator_)trust | `PerU16(u16)` | fraction = raw/65535; `RATIO_TYPE_DENOMINATORS = {PerU16: 65535, Percent: 100, Permill: 1e6, Perbill: 1e9}`; `ratio_fraction(ident, raw)` | `SDK/hyperparams.py:27-52`, `SDK/metagraph.py:343-346` |
| rank, pruning_score | `Compact<u16>` | deprecated: runtime emits empty vectors (metagraph) / `rank=0`, `pruning_score=u16::MAX` (NeuronInfo); SDK renders `0.0` via `column()` padding | `pallets/subtensor/src/rpc_info/metagraph.rs:782,803-804`, `neuron_info.rs:131-142`, `SDK/metagraph.py:311-313,357-362` |
| Delegate / childkey take | storage `Delegates`, `MinDelegateTake`, `MaxDelegateTake`, `ChildkeyTake`: `PerU16` | fraction via the descriptor's `value_type_ident`; `take_u16` raw also exposed | `SDK/reads/delegation.py:16-28,104-121`, `SDK/_generated/storage.py:91-99` |
| Weights matrix | `Weights[(NetUidStorageIndex, u16)] -> Vec<(u16,u16)>` | "max-upscaled u16 values whose absolute scale carries no meaning" → each row normalized to fractions summing to 1 | `SDK/reads/weights.py:22-46`; hasher table `META/explore/_storage_table.tsv` row `Weights` |
| Bonds matrix | `Bonds[...] -> Vec<(u16,u16)>` | magnitude meaningful → value/65535 (PerU16 semantics) | `SDK/reads/weights.py:49-76` |
| `max_weight_limit` | `MaxWeightsLimit` u16 | cap on any single *normalized* weight: `max_weight_limit/65535` of the row total, not a raw ceiling | `SDK/reads/hyperparameters.py:30-36` |
| Mechanism storage index | `NetUidStorageIndex(u16)` | `mechid * GLOBAL_MAX_SUBNET_COUNT(4096) + netuid`; equals `netuid` for mechanism 0; used as first key of `Weights`, `Bonds`, `LastUpdate`, `Incentive`, `TimelockedWeightCommits`; `Metagraph.mechid = raw netuid // 4096` | `SDK/reads/weights.py:12-16`, `SDK/settings.py:31`, `pallets/subtensor/src/subnets/mechanism.rs:30-43`, `SDK/metagraph.py:384` |
| Child/parent proportions | `Vec<(u64, AccountId32)>` | u64-normalized: `u64::MAX` = 100% | `SDK/reads/delegation.py:156-166` |
| `tao_weight` (root stake weighting) | `TaoWeight` u64 | normalized by u64::MAX in the runtime; `total_stake = alpha + tao * tao_weight` | `pallets/subtensor/src/staking/stake_utils.rs:196-204,269-320` |
| Hyperparameter kinds (hand table) | storage ints | `u16` (65535=1.0), `u64` (u64::MAX=1.0), `per_million` (1,000,000=1.0, e.g. `bonds_moving_avg`), `rao`, `blocks` (12 s annotated), `epochs`, `difficulty` (u64::MAX = disabled), `fixed128` (U64F64 bits/2^64), `int`, `bool` | `SDK/hyperparams.py:55-80,110-408` |
| Metadata-dictated kinds | `value_type_ident` | `PerU16 → u16`, `Permill → per_million`, `TaoBalance → rao`; metadata wins over the hand table; `codegen.check --units` asserts agreement | `SDK/hyperparams.py:463-499`, `sdk/python/codegen/check.py:472-507` |
| `burn_increase_mult` | `BurnIncreaseMult` `FixedU128` (U64F64) | bits/2^64; bounds 1.0..3.0 | `SDK/hyperparams.py:323-332` |
| `collateral_lock_share` / `collateral_drain_ratio` | u16 / U64F64 | share/65535 capped at 62258 (95%); ratio bits/2^64, (0, 10] | `SDK/hyperparams.py:338-354`, `SDK/reads/collateral.py:22-26,146-157` |
| `alpha_sigmoid_steepness` | storage `i16`; hyperparams v3 returns `I32F32` | read flattens `I32F32` to its **integer part** `bits // 2^32` so it matches the setter | `SDK/reads/subnets.py:126-147` |
| `moving_price` | `SubnetMovingPrice` `FixedI128` = `I96F32` | bits / 2^32 | `SDK/metagraph.py:33-34,396-397` |
| Spot price | `tao_in / alpha_in` (SubnetTAO / SubnetAlphaIn) | τ per alpha, float | `SDK/metagraph.py:377-396`, `metagraph.rs:722-724` |
| `current_alpha_price(_all)` | `u64` | rao per alpha → `/1e9` = TAO per alpha | `SDK/reads/prices.py:75-95` |
| `SubnetEmaTaoFlow` | `(u64, FixedI128)` = `(last_update, I64F64)` | bits/2^64, rao per block, signed | `SDK/reads/subnets.py:85-111` |
| Conviction (`get_hotkey_conviction`) | `FixedU128` = `U64F64` of alpha rao | integer part `bits >> 64` is the rao amount | `SDK/reads/locks.py:150-154` |
| `LockState.conviction` | U64F64 | bits/2^64 as float rao in client-side projections | `SDK/reads/locks.py:171-185` |
| `UnlockRate` / `MaturityRate` | u64 blocks | default `934_866` when storage read returns empty | `SDK/reads/locks.py:13-15` |
| Ownership gate | — | `conviction * 100 > eligible_alpha * 18` strictly; subnet age ≥ `7200*365+1800 = 2,629,800` blocks | `SDK/reads/locks.py:17-18,242-245` |
| `RootClaimableThreshold` / `BasketRate` | `FixedI128` = `I96F32` | threshold: `bits >> 32` whole rao; rate: bits/2^32 float (small ratio, truncation would zero it) | `SDK/reads/root.py:54-77,437-451` |
| Beta (basket) tokens | `beta: u64` raw | `_RAW_PER_BETA = 10**9`; par τ1 at inception; `beta_price_tao = nav_rao / beta_total_raw` | `SDK/reads/root.py:46-51,212-235` |
| `BetaPricing` fields | `U64F64` | bits/2^64 floats (`spot_price`, `display_price`, `stake_price`, `staker_yield`, `staker_twr`, `bag_index`, `stake_index`, `display_shares`) | `SDK/reads/root.py:79-94` |
| Lease `emissions_share` | u8/int | percent 0..100 | `SDK/reads/leasing.py:19` |
| `AccountFlags` | u128 | bit 0 = accepts locked alpha | `SDK/reads/locks.py:20-22,118-132` |
| Swap fee | `Swap.FeeRate` u16 | amount × FeeRate / 65535 | `docs/concepts/money.mdx:83` |
| Block time | `Aura.SlotDuration` ms | `/1000` → 12.0 mainnet, 0.25 fast-blocks; **never hardcode** | `SDK/_substrate.py:423-433` (but `BLOCKTIME = 12.0` is still used for commitment ages and hyperparam annotations: `SDK/metagraph.py:103-105`, `SDK/hyperparams.py:521-535`) |
| Drand round ↔ time | quicknet | `QUICKNET_GENESIS = 1_692_803_367`, period 3 s; `reveal_time(r) = genesis + (r-1)*3` | `SDK/timelock.py:50-54,175-178` |
| `name` / `symbol` on metagraph & dynamic info | `Vec<Compact<u8>>` | list of ints → utf-8 (unlike every other text field, which is `Vec<u8>`) | `SDK/reads/subnets.py:176-190`, `metagraph.rs:20-22` |
| Axon endpoint | `AxonInfo{ip: u128, port: u16, ip_type: u8,...}` | `ip==0` → None; v6 rendered `[addr]:port` | `SDK/metagraph.py:563-574` |
| Commitment `Data` fields | enum `None \| Raw0..RawN([u8;N]) \| Sha256/... \| TimelockEncrypted{encrypted, reveal_round} \| TimelockRevealFailed{...}` | Raw* concatenated → utf-8 else 0x-hex; hashes `sha256:0x…`; status machine plain/sealed/revealed/failed | `SDK/metagraph.py:475-533`; `subxt explore pallet Commitments storage CommitmentOf` |
| Revealed commitment bytes | `Vec<(Vec<u8>, u64)>` | payload is SCALE bytes with a compact length prefix the SDK strips by hand | `SDK/metagraph.py:535-560` |
| Null consensus capacity | — | `NULL_UID_BUDGET = 2_500`; Yuma 256; capacity `floor(budget / mechanism_count)` | `pallets/subtensor/src/subnets/mechanism.rs:34-35`, `docs/guides/null-consensus.mdx:42-43`, `SDK/hyperparams.py:316-321` |

Hyperparameter input coercion rules (reusable for an rtao `set_hyperparameter` CLI): a plain integer is always raw; a value with a decimal point is human (0..1 fraction / TAO / multiplier); codec width precedence is explicit `Hyperparam.bits` → storage ident → kind default (`SDK/hyperparams.py:581-770`).

---

## 3. Complete inventory of the 102 registered reads

Counting method: multi-line-aware scan of `@read("...")`, `scalar_read("...")` and `_hyperparam("...")` in `SDK/reads/*.py` gives 102 names; `DOCSCAT/reads.json` has 102 entries and `docs/query/` has 102 pages + `index.mdx` + `meta.json`; the two name sets are identical (verified with `comm`). Per-file counts: accounts 6, chain 6, collateral 3, delegation 9, epochs 5, hyperparameters 7, identity 5, leasing 5, locks 7, neurons 6, prices 4, root 11, staking 11, subnets 14, weights 3.

Column key — **Hits**: `S:` storage item with key params in order, `R:` runtime API method, `C:` constant, `M:` map scan (`query_map`), `B:` batch (`query_batch`). **Pin**: `pinned` = read calls `view.at()` first (all sub-calls at one block); `single` = one RPC (pinned iff called via a Snapshot); `gather` = several RPCs not mutually pinned on a Client. Key hashers are from `META/explore/_storage_table.tsv` (spec 477) where noted. Return "units" use the Balance tagging described in §2.

### 3.1 Accounts & keys — namespace `balances` (`SDK/reads/accounts.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `balance` | `S: System.Account[coldkey]` (Blake2_128Concat) | `coldkey_ss58: string` | `Balance` TAO from `data.free` | single (`:14-23`) |
| `balances` | `B: System.Account[[a]…]` one RPC | `coldkey_ss58s: array` | `{addr: Balance}` (0 for missing) | single (`:26-38`) |
| `existential_deposit` | `C: Balances.ExistentialDeposit` | — | `Balance` TAO | n/a (`:41-45`) |
| `proxies` | `S: Proxy.Proxies[coldkey]` → `(BoundedVec<{delegate, proxy_type, delay}>, deposit)` | `coldkey_ss58` | `{proxies: [{delegate, proxy_type: str, delay: int}], deposit: Balance}` | single (`:48-68`) |
| `coldkey_swap_announcement` | `S: SubtensorModule.ColdkeySwapAnnouncements[coldkey]` `(u32, H256)` Optional, Twox64Concat; `S: ColdkeySwapDisputes[coldkey]` u32 Optional | `coldkey_ss58` | `{execute_block, new_coldkey_hash, disputed: bool, dispute_block\|None}` or `None` | gather (`:71-95`) |
| `multisig` | `S: Multisig.Multisigs[account, call_hash]` | `account_ss58`, `call_hash` (hex, `0x` optional) | `{timepoint: {height, index}, deposit: Balance, depositor, approvals: [ss58]}` or `None` | single (`:98-119`) |

Curated namespace extras: `balances.get(address, block)`, `balances.get_many(addresses, block)`, `balances.existential_deposit()` (`SDK/namespaces.py:99-117`).

### 3.2 Chain — namespace `chain` (`SDK/reads/chain.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `timestamp` | `S: Timestamp.Now` (ms) via `view.timestamp()` | — | `{block, unix: float, iso}` | pinned (`:11-16`; `SDK/client.py:332-335`) |
| `block_time` | `C: Aura.SlotDuration` /1000, cached per connection | — | `float` seconds | n/a (`:19-22`; `SDK/_substrate.py:423-433`) |
| `is_fast_blocks` | derived: `block_time() == 0.25` | — | `bool` | n/a (`:25-28`; `SDK/client.py:345-347`) |
| `block_info` | `chain_getBlock` + extrinsic decode; timestamp from the `Timestamp.set` inherent (saves a storage read) | `block: integer` | `{number, hash, timestamp iso, extrinsics: ["Module.function"\|None], explorer_url}` or `None` | takes its own block; not pinnable by `block=` (`:31-59`; `SDK/client.py:349-372,675-688`) |
| `mev_shield_next_key` | `S: MevShield.NextKey` (BoundedVec) | — | `0x`-hex ML-KEM-768 public key or `None` | single (`:62-68`) |
| `tx_rate_limit` | `S: SubtensorModule.TxRateLimit` (Plain u64) | — | `int` blocks | single (`:71-77`) |

### 3.3 Delegation — namespace `delegation` (`SDK/reads/delegation.py`)

`DelegateInfo` dataclass: `hotkey, owner, take: float (0..1), nominators: int (count), registrations: [netuid], validator_permits: [netuid], return_per_1000: Balance TAO, total_daily_return: Balance TAO` (`:31-64`). Wire struct `DelegateInfo<AccountId32>{delegate_ss58, take: Compact<PerU16>, nominators: Vec<(AccountId32, Vec<(Compact<NetUid>, Compact<u64>)>)>, owner_ss58, registrations: Vec<Compact<NetUid>>, validator_permits, return_per_1000: Compact<u64>, total_daily_return: Compact<u64>}` (`pallets/subtensor/src/rpc_info/delegate_info.rs:12-23`).

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `delegates` | `R: DelegateInfoRuntimeApi.get_delegates()` | — | `[DelegateInfo]` | single (`:67-71`) |
| `delegate` | `R: get_delegate(delegate_account)` → `Option<DelegateInfo>` | `hotkey_ss58` | `DelegateInfo\|None` | single (`:74-83`) |
| `is_delegate` | same call | `hotkey_ss58` | `bool` | single (`:86-95`) |
| `delegate_take` | `S: Delegates[hotkey]` (PerU16, Blake2_128Concat), `S: MinDelegateTake`, `S: MaxDelegateTake` (Plain PerU16) | `hotkey_ss58` | `{hotkey, take, take_u16, min, max}` fractions keyed on `value_type_ident` | gather (`:98-121`) |
| `delegated` | `R: get_delegated(delegatee_account)` → `Vec<(DelegateInfo, (Compact<NetUid>, Compact<AlphaBalance>))>` | `coldkey_ss58` | `[DelegatedStake{delegate, netuid, stake: Balance(netuid)}]` | single (`:124-144`) |
| `children` | `S: ChildKeys[hotkey, netuid]` (Blake2_128Concat, Identity) | `hotkey_ss58, netuid` | `[(proportion u64-normalized, child_ss58)]` | single (`:147-166`) |
| `parents` | `S: ParentKeys[hotkey, netuid]` | same | `[(proportion, parent_ss58)]` | single (`:169-188`) |
| `pending_children` | `S: PendingChildKeys[netuid, hotkey]` (**key order reversed** vs ChildKeys; Identity, Blake2_128Concat) → `(Vec<(u64, AccountId32)>, u64)` | same | `{children: [(prop, child)], cooldown_block}` | single (`:191-215`) |
| `childkey_threshold_suspended` | `S: ChildkeyThresholdSuspended[hotkey]` (Optional unit `()`) | `hotkey_ss58` | `bool` = key present | single (`:218-234`) |

### 3.4 Epochs & timing — namespace `epochs` (`SDK/reads/epochs.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `next_epoch_start_block` | `R: SubnetInfoRuntimeApi.get_next_epoch_start_block(netuid)` → `Option<u64>` | `netuid` | `int\|None` (None when tempo 0); chain-computed `LastEpochBlock + tempo`, pulled earlier by `PendingEpochAt` | single (`:13-28`) |
| `blocks_until_next_epoch` | same | `netuid` | `max(0, next - view.block)\|None` | pinned (`:31-47`) |
| `blocks_since_last_step` | `S: BlocksSinceLastStep[netuid]` u64 | `netuid` | `int` | single (`:50-56`) |
| `blocks_since_last_update` | `S: LastUpdate[netuid]` (`Vec<u64>`, key `NetUidStorageIndex` → mechanism 0 only) | `netuid, uid` | `view.block - last[uid]` or `None` | pinned (`:59-81`) |
| `epoch_status` | `S: Tempo, LastEpochBlock, BlocksSinceLastStep, PendingEpochAt, SubnetEpochIndex [netuid]`; `R: get_next_epoch_start_block`; `block_time()` | `netuid` | `{netuid, block, tempo, epoch_index, last_epoch_block, blocks_since_last_step, pending_epoch_at\|None, next_epoch_start_block\|None, blocks_remaining\|None, seconds_remaining\|None}` | pinned (`:84-128`) |

Related client helper (not a read): `client.wait_for_epoch(netuid)` subscribes to heads and polls `LastEpochBlock[netuid]` at each new block until it advances (`SDK/client.py:418-444`).

### 3.5 Hyperparameters — namespace `hyperparameters` (`SDK/reads/hyperparameters.py:13-54`)

All seven are `scalar_read`s: `S: <item>[netuid]` → `int`, single.

| Read | Storage item (type) | Unit |
|---|---|---|
| `weights_rate_limit` | `WeightsSetRateLimit` (u64) | blocks |
| `difficulty` | `Difficulty` (u64) | PoW difficulty |
| `min_allowed_weights` | `MinAllowedWeights` (u16) | count (pure self-weight bypasses) |
| `max_weight_limit` | `MaxWeightsLimit` (u16) | normalized cap `x/65535` |
| `immunity_period` | `ImmunityPeriod` (u16) | blocks |
| `reveal_period` | `RevealPeriodEpochs` (u64) | epochs |
| `subnet_emission_enabled` | `SubnetEmissionEnabled` (bool → int 0/1) | flag |

The full hyperparameter set is served by `subnet_hyperparameters` (§3.13) and the unit table in `SDK/hyperparams.py:110-408` (50 entries incl. owner-settable ones absent from the v3 listing: `max_allowed_uids`, `burn_increase_mult`, `burn_half_life`, `collateral_*`, `yuma3_enabled`, `subnet_is_active`, `subnet_emission_enabled`, `transfers_enabled`, `owner_cut_*`, `bonds_reset_enabled`, `user_liquidity_enabled`).

### 3.6 Identity & commitments — namespace `identity` (`SDK/reads/identity.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `identity` | `S: IdentitiesV2[coldkey]` (`ChainIdentityV2`, Optional, Blake2_128Concat) | `coldkey_ss58` | `{field: utf8 text}` or `None` | single (`:25-34`) |
| `hotkey_identities` | `S: Owner[hotkey]` ×N then `S: IdentitiesV2[coldkey]` ×M (gathered, not batched) | `hotkey_ss58s: array` | `{hotkey: identity}` for hotkeys whose owner has one | gather (`:37-65`) |
| `commitment` | `S: Commitments.CommitmentOf[netuid, hotkey]` → `Registration{deposit: TaoBalance, block: u32, info{fields: BoundedVec<Data>}}` | `netuid, hotkey_ss58` | `Commitment{block, deposit: Balance, data: str, fields: raw}` or `None` | single (`:81-101`) |
| `revealed_commitment` | `S: Commitments.RevealedCommitments[netuid, hotkey]` → `Vec<(Vec<u8>, u64)>` | same | raw decoded value | single (`:104-115`) |
| `commitments` | `M: CommitmentOf[netuid]`, `M: RevealedCommitments[netuid]`, `M: Uids[netuid]` (for uid join) | `netuid` | `[{hotkey, uid\|None, commitment: value\|None, block, duration: "3d 4h", is_revealed, status: plain\|sealed\|revealed\|failed, reveals_at: iso\|None}]` newest first | pinned via `metagraph.fetch_commitments` (`:133-161`, `SDK/metagraph.py:258-291`) |

### 3.7 Leases & crowdloans — namespace `leasing` (`SDK/reads/leasing.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `lease` | `S: SubtensorModule.SubnetLeases[lease_id]` (u32 key, Twox64Concat, Optional) | `lease_id: integer` | `{lease_id, beneficiary, coldkey, hotkey, emissions_share: 0..100, end_block\|None, netuid, cost: Balance TAO}` or `None` | single (`:12-39`) |
| `leases` | `M: SubnetLeases` | — | `[lease record]` | single (`:42-46`) |
| `crowdloan` | `S: Crowdloan.Crowdloans[id]` | `crowdloan_id` | `{creator, deposit, min_contribution, cap, raised: Balance TAO, end: block, finalized, target_address\|None, funds_account\|None}` or `None` | single (`:49-74`) |
| `crowdloans` | `M: Crowdloan.Crowdloans` | — | `[{id, creator, raised_tao: float, cap_tao: float, finalized}]` sorted by id | single (`:77-94`) |
| `crowdloan_contributors` | `M: Crowdloan.Contributions[id]` | `crowdloan_id` | `[{contributor, amount_tao: float}]` | single (`:97-111`) |

### 3.8 Locks & conviction — namespace `locks` (`SDK/reads/locks.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `coldkey_lock` | `M: Lock[coldkey]` (NMap `(coldkey, netuid, hotkey) -> LockState`, Blake2_128Concat/Identity/Blake2_128Concat) to find the hotkey, then `R: StakeInfoRuntimeApi.get_coldkey_lock(coldkey, netuid)` → `Option<LockState{locked_mass: AlphaBalance, conviction: U64F64, last_update: u64}>` and `S: DecayingLock[coldkey, netuid]` (bool Optional) | `coldkey_ss58, netuid` | `{hotkey, netuid, locked_alpha: Balance(netuid) rolled forward, is_perpetual}` or `None` | gather (`:33-71`) |
| `locks_for_coldkey` | `M: Lock[coldkey]` + per-pair `get_coldkey_lock` + `DecayingLock` | `coldkey_ss58` | `[record]` sorted by (netuid, hotkey) | gather (`:74-87`) |
| `locks_for_hotkey` | `M: LockingColdkeys[netuid, hotkey]` (unit values; reverse index) + per-coldkey records | `hotkey_ss58, netuid` | `[{coldkey, hotkey, netuid, locked_alpha, is_perpetual}]` | gather (`:90-115`) |
| `accepts_locked_alpha` | `S: AccountFlags[coldkey]` u128 | `coldkey_ss58` | `bool` (bit 0) | single (`:118-132`) |
| `hotkey_conviction` | `R: get_hotkey_conviction(hotkey, netuid)` → `U64F64` | `hotkey_ss58, netuid` | `{conviction_alpha: Balance(netuid)}` (`bits >> 64`) | single (`:135-154`) |
| `most_convicted_hotkey` | `R: get_most_convicted_hotkey_on_subnet(netuid)` → `Option<AccountId32>` | `netuid` | `str\|None` | single (`:157-168`) |
| `subnet_convictions` | `M: HotkeyLock[netuid]`, `M: DecayingHotkeyLock[netuid]`, `S: OwnerLock[netuid]`, `S: DecayingOwnerLock[netuid]`, `S: SubnetOwnerHotkey[netuid]`, `S: SubnetAlphaOut[netuid]`, `S: SubnetProtocolAlpha[netuid]`, `S: AlphaAssets.AlphaBurned[netuid]`, `S: UnlockRate`, `S: MaturityRate`, `S: NetworkRegisteredAt[netuid]`; client-side float mirror of the pallet's `roll_forward_lock` | `netuid` | `{netuid, block, alpha_out, protocol_alpha, alpha_burned, eligible_alpha, threshold_alpha, total_locked_alpha, total_conviction_alpha, leader_hotkey, leader_blocks_to_threshold, unlock_rate, maturity_rate, owner_hotkey, registered_at, ownership_changeable_at_block, hotkeys: [{hotkey, is_owner, locked_alpha, conviction_alpha, pct_of_threshold, blocks_to_threshold\|None}]}` | pinned (`:289-419`) |

Note for rtao: `subnet_convictions` contains real domain math (exponential decay projection, bisection for the crossing block: `:188-286`) that a Rust port should implement with the pallet's own fixed-point arithmetic rather than the Python float mirror.

### 3.9 Mining & collateral — namespace `collateral` (`SDK/reads/collateral.py`)

These three use **hand-built descriptors** `Item("SubtensorModule","MinerCollateral","MinerCollateralState")`, `Item(...,"CollateralLockShare","u16")`, `Item(...,"CollateralDrainRatio","U64F64")` with a stale `TODO(codegen)` comment (`:8-17`) — the generated catalog already contains all three (`SDK/_generated/storage.py:347-349`). Because the docs scraper only recognizes `st.X.Y` references, `collateral_policy` shows no hits in `reads.json`.

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `miner_collateral` | `S: Owner[hotkey]` (when coldkey omitted) then `S: MinerCollateral[netuid, hotkey, coldkey]` (NMap Identity/Blake2_128Concat/Blake2_128Concat, Optional) → `MinerCollateralState{locked, min_locked, earned, drain_ratio: U64F64}` | `netuid, hotkey_ss58, coldkey_ss58?` | `{hotkey, coldkey, netuid, locked_alpha, min_locked_alpha, earned_alpha, drain_ratio: float, headroom_alpha, shortfall_alpha, releasable_work_alpha}` or `None` | gather (`:57-93`) |
| `subnet_collateral` | `M: MinerCollateral[netuid]` + `B: Uids[netuid, hotkey]` | `netuid` | `[record + uid\|None]` sorted by locked desc | pinned (`:96-129`) |
| `collateral_policy` | `S: CollateralLockShare[netuid]` (u16), `S: CollateralDrainRatio[netuid]` (U64F64; 1.0 default when absent) | `netuid` | `{netuid, lock_share: x/65535, drain_ratio: float}` | pinned (`:132-157`) |

### 3.10 Neurons & registration — namespace `neurons` (`SDK/reads/neurons.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `neurons` | `R: NeuronInfoRuntimeApi.get_neurons_lite(netuid)` or `get_neurons(netuid)` (full, with `weights`/`bonds` as `Vec<(Compact<u16>,Compact<u16>)>` filtered to non-zero) | `netuid, lite: boolean = true` | `[Neuron{uid, hotkey, coldkey, active, validator_permit, last_update, total_stake: Balance(netuid) = Σ stake list, raw: dict}]` sorted by uid | single (`:32-66`; wire structs `pallets/subtensor/src/rpc_info/neuron_info.rs:8-55`) |
| `hotkey_owner` | `S: Owner[hotkey]` — **`Default` modifier with an all-zero `AccountId32` default** (`META/explore/_storage_table.tsv` row `Owner`), so a miss decodes to the all-zero account's ss58 string; the SDK's `str(value) if value else None` only yields `None` for a literal falsy value | `hotkey_ss58` | `str\|None` (in practice the zero account for unowned hotkeys — **inferred** from the default bytes; same applies to `Keys` and `SubnetOwnerHotkey`) | single (`:69-78`) |
| `owned_hotkeys` | `S: OwnedHotkeys[coldkey]` | `coldkey_ss58` | `[ss58]` | single (`:81-90`) |
| `netuids_for_hotkey` | `M: IsNetworkMember[hotkey]` (Blake2_128Concat, Identity) | `hotkey_ss58` | sorted `[netuid]` where true | single (`:93-102`) |
| `uid` | `S: Uids[netuid, hotkey]` (Identity, Blake2_128Concat; Optional u16) | `hotkey_ss58, netuid` | `int\|None` | single (`:105-117`) |
| `associated_evm_key` | `S: AssociatedEvmAddress[netuid, uid]` (Twox64Concat×2, Optional `(H160, u64)`) | `netuid, uid` | `{evm_address, block}` or `None` | single (`:120-135`) |

Curated: `neurons.all(netuid, block=None, *, lite=True)` (`SDK/namespaces.py:200-213`).

### 3.11 Prices & swaps — namespace `prices` (`SDK/reads/prices.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `quote_stake` | `R: SwapRuntimeApi.sim_swap_tao_for_alpha{netuid, tao: rao}` (dict params by name) → `SimSwapResult{tao_amount, alpha_amount, tao_fee, alpha_fee, tao_slippage, alpha_slippage}` | `netuid, amount_tao: number` | `SwapQuote{tao, alpha(netuid), tao_fee, alpha_fee, tao_slippage, alpha_slippage}` | single (`:36-54`) |
| `quote_unstake` | `R: sim_swap_alpha_for_tao{netuid, alpha: rao}` | `netuid, amount_alpha: number` | `SwapQuote` | single (`:57-72`) |
| `alpha_prices` | `R: current_alpha_price_all()` → `Vec<SubnetPrice{netuid, price: u64}>` | — | `{netuid: tao_per_alpha float}` | single (`:75-79`) |
| `alpha_price` | `R: current_alpha_price(netuid)` → u64 | `netuid` | `{netuid, tao_per_alpha, price_rao}` | single (`:82-95`) |

### 3.12 Staking — namespace `staking` (`SDK/reads/staking.py` and `SDK/reads/root.py`)

`StakePosition{hotkey, coldkey, netuid, stake: Balance(netuid), is_registered}` keeps only 5 of the 9 wire fields of `StakeInfo{hotkey, coldkey, netuid, stake: Compact<AlphaBalance>, locked: Compact<u64>, emission: Compact<AlphaBalance>, tao_emission: Compact<TaoBalance>, drain: Compact<u64>, is_registered}` (`:16-62`; `pallets/subtensor/src/rpc_info/stake_info.rs:11-23`).

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `stake` | `R: StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid(hotkey, coldkey, netuid)` → `Option<StakeInfo>` | `coldkey_ss58, hotkey_ss58, netuid` | `Balance(netuid)` (0 when None) | single (`:65-81`) |
| `stake_for_coldkey` | `R: get_stake_info_for_coldkey(coldkey)` | `coldkey_ss58` | `[StakePosition]` | single (`:84-98`) |
| `stake_for_coldkeys` | `R: get_stake_info_for_coldkeys([coldkeys])` → `Vec<(AccountId32, Vec<StakeInfo>)>` | `coldkey_ss58s: array` | `{coldkey: [StakePosition]}` | single (`:101-119`) |
| `stake_value_for_coldkey` | `R: get_stake_info_for_coldkey` + `alpha_prices` | `coldkey_ss58` | `StakeValuation{coldkey, block, positions, stake_value: Balance TAO (spot, no slippage), tao_per_alpha: {netuid: float}}` | pinned (`:133-153`) |
| `stake_value_for_coldkeys` | batch variant | `coldkey_ss58s` | `{coldkey: StakeValuation}` | pinned (`:156-182`) |
| `stake_availability` | `R: get_stake_availability_for_coldkeys([coldkey], Some([netuid]))` → `BTreeMap<AccountId32, BTreeMap<NetUid, StakeAvailability{total, locked, available}>>` | `coldkey_ss58, netuid` | `{netuid, total, locked, available: Balance(netuid)}` | single (`:185-215`) |
| `stake_availability_for_coldkey` | same, many netuids | `coldkey_ss58, netuids: array` | `[rows]` | single (`:218-248`) |
| `total_alpha_staked` | `S: TotalAlphaStaked[netuid]` (AlphaBalance; O(1) aggregate, backfilled since v448) | `netuid` | `Balance(netuid)` | single (`:251-268`) |
| `staking_hotkeys` | `S: StakingHotkeys[coldkey]` (`Vec<AccountId32>`) | `coldkey_ss58` | `[ss58]` | single (`:271-288`) |
| `auto_stake` | `S: AutoStakeDestination[coldkey, netuid]` (Blake2_128Concat, Identity; Optional) | `coldkey_ss58, netuid` | `str\|None` | single (`:291-303`) |
| `auto_stake_all` | `M: AutoStakeDestination[coldkey]` | `coldkey_ss58` | `[{netuid, hotkey}]` sorted | single (`:306-323`) |
| `root_basket_owed` | `R: BetaBasketRuntimeApi.get_root_basket_owed(coldkey)` | `coldkey_ss58` | `Balance` TAO | single (`root.py:166-184`) |
| `root_basket_owed_breakdown` | `R: get_root_basket_positions(coldkey)` → `Vec<(AccountId32, u64 beta, TaoBalance payout)>` | `coldkey_ss58` | `[{hotkey, owed_tao}]` desc, zero-owed omitted | single (`:187-209`) |
| `basket_position` | `R: get_basket_position{hotkey, coldkey}` (hand-built `Method`; v2) → `Option<BasketPosition{hotkey, beta, beta_total, nav_tao, value_tao, spot_value_tao}>` | `hotkey_ss58, coldkey_ss58` | `{hotkey, beta_raw, beta_total_raw, beta, fund_fraction, beta_price_tao, nav_tao, value_tao, spot_value_tao}` or `None` | single (`:212-264`) |
| `root_basket_portfolio` | `R: get_root_basket_portfolio(coldkey)` (hand-built) | `coldkey_ss58` | `[position record]` sorted by value desc | single (`:267-286`) |
| `validator_basket` | `R: get_validator_basket(hotkey)` → `Vec<(NetUid, AlphaBalance, TaoBalance)>` | `hotkey_ss58` | `[{netuid, alpha: Balance(netuid), value_tao}]` (netuid 0 = TAO cash slot) | single (`:289-310`) |
| `validator_basket_summary` | `R: get_validator_basket_summary(hotkey)` → `BasketSummary{hotkey, nav_tao, spot_nav_tao, shares: u64, deposited_tao, redeemed_tao, holdings: [{netuid, alpha, spot_tao, realizable_tao}]}`; `S: BasketRate[hotkey]` (I96F32); `R: get_beta_pricing(hotkey)` (hand-built, v3+; exceptions swallowed) | `hotkey_ss58` | summary record + `basket_rate` + optional `chain_pricing{spot_price, display_price, stake_price, staker_yield, staker_twr, bag_index, stake_index, first_block, provisional, display_shares}` | pinned (`:313-344`) |
| `root_baskets` | `R: get_all_validator_baskets()`; `B: BasketRate[[hotkey]…]`; paged `R: get_all_beta_pricing(start_after, limit=0)` loop until `next` is None | — | `[summary record]` sorted by nav desc | pinned (`:347-374`) |
| `validator_basket_nav` | `R: get_validator_basket_nav(hotkey)` | `hotkey_ss58` | `Balance` TAO | single (`:377-386`) |
| `basket_trading_status` | `R: get_basket_trading_status(hotkey)` (hand-built, v4) → `{enabled, frozen, refill_blocks, tao_available, budget_tao}` | `hotkey_ss58` | `{enabled, frozen, refill_blocks, refill_per_block_tao, used_tao, budget_tao, remaining_tao}` | single (`:389-420`) |
| `root_basket_total_nav` | `R: get_root_basket_total_nav()` | — | `Balance` TAO | single (`:423-434`) |
| `root_claim_threshold` | `S: RootClaimableThreshold[0]` (hand-built `Item`; I96F32 → `bits >> 32`) | — | `Balance` TAO | single (`:437-451`) |

`root.py` hand-builds five `Method`/`Item` descriptors with stale TODOs (`:28-44`) even though `SDK/_generated/runtime_apis.py:32-50` and `storage.py:314` already list them — the five reads consequently have no `hits` in `reads.json` (`basket_position`, `basket_trading_status`, `root_basket_portfolio`, `root_claim_threshold`; plus `collateral_policy`, `commitments`, `timestamp`, `block_time`, `is_fast_blocks`, `block_info` for other reasons). Curated: `staking.get(coldkey, hotkey, netuid, block)`, `staking.positions(coldkey, block)` (`SDK/namespaces.py:119-141`).

Display-layer pricing for baskets (`SDK/basket_index.py`, frozen series in `basket_index_data.py` rebuilt by `scripts/build_basket_index.py`) is a **fallback** for pre-v3 nodes; on v3+ nodes `chain_pricing` passes through the runtime's numbers (`SDK/basket_index.py:26-38,191-216,286-315`). rtao should treat the chain-side `BetaBasketRuntimeApi` v3+ as canonical and skip the local index math.

### 3.13 Subnets — namespace `subnets` (`SDK/reads/subnets.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `subnet` | `S: Tempo[netuid]`, `S: Burn[netuid]`, `S: SubnetworkN[netuid]` (all `Map<Identity>`) | `netuid` | `SubnetInfo{netuid, tempo, burn: Balance TAO, neuron_count}` | pinned (`:24-43`) |
| `subnets` | `M: NetworksAdded`, `M: Tempo`, `M: Burn`, `M: SubnetworkN` (4 map scans) | — | `[SubnetInfo]` for netuids with `NetworksAdded == true` | pinned (`:46-70`) |
| `burn` | `S: Burn[netuid]` (TaoBalance) | `netuid` | `Balance` TAO | single (`:73-82`) |
| `subnet_tao_flows` | `M: SubnetEmaTaoFlow` → `(u64, I64F64)` Optional | — | `{netuid: ema float rao/block}` | single (`:92-111`) |
| `commit_reveal_enabled` | `S: CommitRevealWeightsEnabled[netuid]` | `netuid` | `bool` | single (`:114-123`) |
| `subnet_hyperparameters` | `R: SubnetInfoRuntimeApi.get_subnet_hyperparams_v3(netuid)` → `Option<Vec<HyperparamEntry{name: Vec<u8>, value: HyperparamValue}>>`, `HyperparamValue = Bool \| U16(Compact) \| U32 \| U64 \| U128 \| TaoBalance(Compact) \| I32F32 \| U64F64` | `netuid` | `{name: raw payload}`; U64F64 → raw bits; I32F32 → `bits // 2^32`; `None` if subnet missing | single (`:126-167`; `pallets/subtensor/src/rpc_info/subnet_info.rs:126-170`) |
| `metagraph` | `R: get_metagraph(netuid)` → `Option<Metagraph<AccountId32>>` | `netuid` | raw dict with `name`/`symbol` decoded to text (compact-u8 vectors) | single (`:170-190`) |
| `subnet_identity` | `S: SubnetIdentitiesV3[netuid]` (Blake2_128Concat, Optional) → `{subnet_name, github_repo, subnet_contact, subnet_url, discord, description, logo_url, additional}` | `netuid` | utf-8 dict or `None` | single (`:193-202`) |
| `subnet_names` | `M: SubnetIdentitiesV3` | — | `{netuid: subnet_name}` | single (`:205-214`) |
| `token_symbols` | `M: TokenSymbol` (`Vec<u8>`) | — | `{netuid: symbol}`; also run at connect and disk-cached 7 days | single (`:217-226`) |
| `subnet_start_schedule` | `S: NetworkRegisteredAt[netuid]`; `C: SubtensorModule.InitialStartCallDelay` (the **constant**, not the `StartCallDelay` storage item at `storage.py:172`) | `netuid` | `{netuid, registered_at, start_call_delay, earliest_start_block, current_block}` | pinned (`:229-251`) |
| `mechanism_count` | `S: MechanismCountCurrent[netuid]` (`MechId` u8, Twox64Concat) | `netuid` | `int` | single (`:254-263`) |
| `mechanism_emission_split` | `S: MechanismEmissionSplit[netuid]` (Optional `Vec<u16>`) | `netuid` | `[u16 raw shares]` (empty = no explicit split) | single (`:266-279`) |
| `subnet_registration_cost` | `R: SubnetRegistrationRuntimeApi.get_network_registration_cost()` → `TaoBalance` | — | `Balance` TAO | single (`:282-290`) |

Curated: `subnets.burn`, `subnets.commit_reveal_enabled`, `subnets.metagraph(netuid, block=None, *, commitments=True) -> Optional[Metagraph]` (shadows the raw `metagraph` read), `subnets.commitments(netuid, block)`, `subnets.info(netuid, block)` (= `subnet`), `subnets.all(block)` (= `subnets`) (`SDK/namespaces.py:143-197`).

### 3.14 Weights & bonds — namespace `weights` (`SDK/reads/weights.py`)

| Read | Hits | Params | Returns | Pin |
|---|---|---|---|---|
| `weights` | `M: Weights[mechid*4096+netuid]` (`Map<Identity,Identity>` keyed `(NetUidStorageIndex, u16 uid)` → `Vec<(u16,u16)>`) | `netuid, mechid=0` | `{validator_uid: {miner_uid: fraction}}` rows normalized to sum 1; `Matrix("validator","miner","weight")` | single (`:22-46`) |
| `bonds` | `M: Bonds[index]` | same | `{uid: {target: value/65535}}` | single (`:49-76`) |
| `timelocked_weight_commits` | `M: TimelockedWeightCommits[index]` (`Map<Twox64Concat,Twox64Concat>` keyed `(NetUidStorageIndex, u64 epoch)` → `Vec<(AccountId32, u64 commit_block, BoundedVec<u8> ciphertext, u64 reveal_round)>`) | same | `{epoch: [{hotkey, commit_block, reveal_round, reveals_at: datetime (drand), ciphertext}]}`; `Grouped("epoch")` | single (`:79-113`) |

### 3.15 Runtime-API and storage surface the reads do **not** wrap (scope candidates for rtao)

From `META/explore/api_*.txt` and `SDK/_generated/runtime_apis.py` vs. the hits above:

- `SubnetInfoRuntimeApi`: `get_selective_metagraph(netuid, Vec<u16>)`, `get_mechagraph(netuid, MechId)`, `get_selective_mechagraph(netuid, MechId, Vec<u16>)`, `get_all_metagraphs()`, `get_all_mechagraphs()`, `get_dynamic_info(netuid)` / `get_all_dynamic_info()` (→ `DynamicInfo`, `pallets/subtensor/src/rpc_info/dynamic_info.rs:9-33`), `get_subnet_state(netuid)` (per-uid vectors incl. `emission_history: Vec<Vec<Compact<AlphaBalance>>>` — the legacy SDK's stake source), `get_subnet_info(_v2)`, `get_subnets_info(_v2)`, `get_subnet_hyperparams(_v2)`, `get_block_emission()`, `get_subnet_to_prune()`, `get_subnet_account_id(netuid)`, `get_coldkey_auto_stake_hotkey(coldkey, netuid)`.
- `NeuronInfoRuntimeApi.get_neuron(netuid, uid)`, `get_neuron_lite(netuid, uid)`.
- `StakeInfoRuntimeApi.get_stake_fee(origin: Option<(AccountId32, NetUid)>, origin_coldkey, destination, destination_coldkey, amount: u64) -> u64`.
- `BetaBasketRuntimeApi`: `get_basket_payout`, `get_beta_index`, `get_beta_position`, `get_beta_portfolio`, `get_basket_claim_preview`, `get_root_basket_claim_previews`.
- `ProxyFilterRuntimeApi.get_proxy_types/get_proxy_filters`, `ShieldApi.*`, `TransactionPaymentApi.query_info` (used internally for fees: `SDK/_transport/interface.py:714-731`).
- Storage: `ValidatorTrust`, `Consensus`, `Incentive`, `Dividends`, `Emission`, `Active`, `ValidatorPermit`, `Keys`, `Axons`, `NeuronCertificates`, `Prometheus`, `WeightCommits`, `CRV3WeightCommits(V2)`, `StakeWeight`, `TotalHotkeyAlpha`, `SubnetTAO`, `SubnetAlphaIn/Out`, `SubnetMovingPrice`, `MaxAllowedUids`, `SubnetEpochConsensus`, `NullPruningTarget`, `SavedYumaMaxAllowedValidators`, `LoadedEmission`, the `Basket*`/`Beta*` and `Drand.Pulses` families — all reachable only via the generic accessors today (the CLI's Null-pruning helper reads several directly: `SDK/cli/null_pruning.py:46-66`).

---

## 4. The Metagraph model

### 4.1 v11 `Metagraph` (SDK/metagraph.py)

**Construction** — `metagraph.fetch(view, netuid, *, commitments=True)`: `view = await view.at()`; one `SubnetInfoRuntimeApi.get_metagraph(netuid)`; when `commitments` is true, concurrently `_fetch_commitment_map` = `query_map(Commitments.CommitmentOf, [netuid])` + `query_map(Commitments.RevealedCommitments, [netuid])` (+ `query_map(SubtensorModule.Uids, [netuid])` only for the standalone `fetch_commitments`); returns `None` when the subnet does not exist (`SDK/metagraph.py:238-291`). Weights/bonds are deliberately excluded ("they dominate the payload size and almost no caller wants them"; `:12-15`).

**`MetagraphNeuron` fields** (`:127-161`): `uid, hotkey, coldkey, active, validator_permit, last_update, block_at_registration, rank, trust, consensus, incentive, dividends, pruning_score` (all six scores `PerU16 → raw/65535`, `:343-346`), `emission: Balance(netuid)`, `alpha_stake: Balance(netuid)`, `tao_stake: Balance(0)`, `total_stake: Balance(netuid)`, `axon: "ip:port"|None`, `identity: dict|None` (ChainIdentityV2 of the coldkey), `commitment: NeuronCommitment|None`, `collateral_locked/min/earned: Balance(netuid)|None` (None on runtimes < spec 435, detected by `graph.get("collateral_locked") is not None`, `:332-341`).

**`Metagraph` fields** (`:164-235`): `netuid, mechid (= raw netuid // 4096), name, symbol, block, tempo, last_step, blocks_since_last_step, owner_hotkey, owner_coldkey, network_registered_at, num_uids, max_uids, price (tao_in/alpha_in | None), moving_price (I96F32 bits/2^32 | None), identity (SubnetIdentityV3 | None), neurons (ordered by uid), commitments: {uid: NeuronCommitment} (missing → None), unregistered_commitments: {hotkey: ...}, raw`. Helpers: `hotkeys`, `coldkeys`, `validators` (permit holders), `neuron(uid)` (KeyError), `by_hotkey(hk)` (linear scan), `__len__`, `__iter__`.

**Columns pulled from `raw` and padded with `None` to `len(hotkeys)`** (`:311-336`): `coldkeys, identities, axons, active, validator_permit, last_update, block_at_registration, rank, trust, consensus, incentives, dividends, pruning_score, emission, alpha_stake, tao_stake, total_stake, collateral_locked, collateral_min, collateral_earned`. Fields of the raw record **not lifted** (available via `mg.raw`): `subnet_emission (deprecated 0), alpha_in, alpha_out, tao_in, alpha_out_emission, alpha_in_emission, tao_in_emission, pending_alpha_emission, pending_root_emission (0), subnet_volume, rho, kappa, min_allowed_weights, max_weights_limit, weights_version, weights_rate_limit, activity_cutoff, max_validators, burn, difficulty, registration_allowed, pow_registration_allowed, immunity_period, min/max_difficulty, min/max_burn, adjustment_alpha, adjustment_interval, target_regs_per_interval, max_regs_per_block, serving_rate_limit, commit_reveal_weights_enabled, commit_reveal_period, liquid_alpha_enabled, alpha_high, alpha_low, bonds_moving_avg, tao_dividends_per_hotkey (always 0), alpha_dividends_per_hotkey`.

**`NeuronCommitment`** (`:45-124`): `hotkey, netuid, uid|None, block, queried_block, deposit: Balance, data: str, encrypted: bool, reveal_round|None, revealed: [(reveal_block, plaintext)], fields`; derived `decrypted`, `status ∈ {plain, sealed, revealed, failed}`, `is_revealed`, `age_blocks`, `duration` (uses the hardcoded `BLOCKTIME`), `value`, `reveals_at` (drand). Fully-revealed commitments whose storage entry the chain dropped are reconstructed from `RevealedCommitments` with `block = latest reveal block` (`:437-455`).

**Unit tagging caveat**: `total_stake` is tagged with `netuid`/alpha symbol (`:366`) although the runtime declares it `Vec<Compact<TaoBalance>>` (`metagraph.rs:109`); the value is `alpha + tao·tao_weight` in alpha-equivalent units (`stake_utils.rs:315`), so the SDK's tagging is semantically the better one but the wire type name is misleading — a typed Rust decoder must not trust `TaoBalance` here.

### 4.2 How the runtime builds `get_metagraph` (`pallets/subtensor/src/rpc_info/metagraph.rs:653-828`)

Per uid 0..`SubnetworkN`: `Keys[netuid, uid]` → hotkey; `Owner[hotkey]` → coldkey; `BlockAtRegistration[netuid, uid]`; `IdentitiesV2[coldkey]`; `get_axon_info(netuid, hotkey)`. Per hotkey: `AlphaDividendsPerSubnet[netuid, hotkey]` (tao dividends are hard-coded zero). Vectors: `Active[netuid]`, `ValidatorPermit[netuid]`, `LastUpdate[NetUidStorageIndex(netuid)]`, `Emission[netuid]`, `Dividends[netuid]`, `Incentive[NetUidStorageIndex(netuid)]`, `Consensus[netuid]`; `pruning_score`, `trust`, `rank` = `Vec::new()` ("Deprecated: no longer computed"). Stakes from `get_stake_weights_for_network` (alpha = inherited alpha stake, tao = inherited root stake, total = alpha + tao·tao_weight; `stake_utils.rs:269-320`) converted `I64F64 → u64`. Collateral vectors from `MinerCollateral[(netuid, hotkey, Owner[hotkey])]` with zero defaults (`:1493-1516`). Pool fields: `SubnetAlphaIn`, `SubnetAlphaOut`, `SubnetTAO`, `SubnetAlphaOutEmission`, `SubnetAlphaInEmission`, `SubnetTaoInEmission`, `PendingValidatorEmission + PendingServerEmission`, `SubnetVolume`, `SubnetMovingPrice`; `last_step = LastMechansimStepBlock[netuid]`; `name = get_name_for_subnet`, `symbol = TokenSymbol`, `identity = SubnetIdentitiesV3`, owners `SubnetOwnerHotkey`/`SubnetOwner`. Cost is O(num_uids) storage reads plus one per hotkey — on a 2,500-UID Null subnet that is thousands of trie reads inside one `state_call`.

Wire struct (77 fields, `#[freeze_struct("93460b9b3cbf6d4e")]`, `metagraph.rs:14-113`) — compact-encoded scalars (`Compact<NetUid>`, `Compact<u64>`, ...), `Vec<Compact<u8>>` name/symbol, `Option<SubnetIdentityV3>`, `I96F32 moving_price`, per-uid `Vec<Compact<...>>` columns, `Vec<(AccountId, Compact<Balance>)>` dividend breakdowns. The `freeze_struct` hash is the runtime's own guard that the struct layout did not silently change — rtao can use the same hashes as a drift signal (grep `freeze_struct` across `rpc_info/*.rs`).

### 4.3 Selective metagraph and mechagraph

- `get_selective_metagraph(netuid, metagraph_indexes: Vec<u16>) -> Option<SelectiveMetagraph>` where every field is `Option<...>`; `SelectiveMetagraphIndex` maps 0..=76 to fields (`metagraph.rs:488-650`): 0 Netuid, 1 Name, 2 Symbol, 3 Identity, 4 NetworkRegisteredAt, 5 OwnerHotkey, 6 OwnerColdkey, 7 Block, 8 Tempo, 9 LastStep, 10 BlocksSinceLastStep, 11 SubnetEmission, 12 AlphaIn, 13 AlphaOut, 14 TaoIn, 15 AlphaOutEmission, 16 AlphaInEmission, 17 TaoInEmission, 18 PendingAlphaEmission, 19 PendingRootEmission, 20 SubnetVolume, 21 MovingPrice, 22 Rho, 23 Kappa, 24 MinAllowedWeights, 25 MaxWeightsLimit, 26 WeightsVersion, 27 WeightsRateLimit, 28 ActivityCutoff, 29 MaxValidators, 30 NumUids, 31 MaxUids, 32 Burn, 33 Difficulty, 34 RegistrationAllowed, 35 PowRegistrationAllowed, 36 ImmunityPeriod, 37 MinDifficulty, 38 MaxDifficulty, 39 MinBurn, 40 MaxBurn, 41 AdjustmentAlpha, 42 AdjustmentInterval, 43 TargetRegsPerInterval, 44 MaxRegsPerBlock, 45 ServingRateLimit, 46 CommitRevealWeightsEnabled, 47 CommitRevealPeriod, 48 LiquidAlphaEnabled, 49 AlphaHigh, 50 AlphaLow, 51 BondsMovingAvg, 52 Hotkeys, 53 Coldkeys, 54 Identities, 55 Axons, 56 Active, 57 ValidatorPermit, 58 PruningScore, 59 LastUpdate, 60 Emission, 61 Dividends, 62 Incentives, 63 Consensus, 64 Trust, 65 Rank, 66 BlockAtRegistration, 67 AlphaStake, 68 TaoStake, 69 TotalStake, 70 TaoDividendsPerHotkey, 71 AlphaDividendsPerHotkey, 72 Validators (uids sorted by stake, filtered above the stake threshold; `:1558-1595`), 73 Commitments (`Vec<(AccountId, Vec<Compact<u8>>)>` via the Commitments pallet's `GetCommitments`; `:1597-1614`), 74 CollateralLocked, 75 CollateralMin, 76 CollateralEarned. Unknown indexes are ignored. **This numbering is not in the metadata** (the metadata only shows the struct), so rtao must vendor the enum.
- `get_mechagraph(netuid, mecid)` = `get_metagraph` with `netuid` replaced by the storage index and `last_update`/`incentives` re-read from `LastUpdate[index]`/`Incentive[index]` (`:838-863`); `get_selective_mechagraph` similar (`:893-912`). The v11 SDK exposes neither as a read; `Metagraph.mechid` is derived from the raw netuid field for callers who use the generic accessor.

### 4.4 Lite vs full, sync, persistence

- **v11**: "lite vs full" applies only to `neurons(lite=True|False)` (`get_neurons_lite` vs `get_neurons`; full adds per-neuron sparse `weights`/`bonds`; `SDK/reads/neurons.py:32-66`). Matrices are separate reads (`weights`, `bonds`). Sync = refetch (`client.subnets.metagraph(netuid, block=...)`); persistence = none ("persist `mg.raw` yourself"; `docs/migration.mdx:308-317`). Block streaming via `client.blocks(finalized=...)` (`chain_subscribeNewHeads`/`FinalizedHeads`; `SDK/_transport/interface.py:439-455`), `wait_for_block`, `wait_for_timestamp`, `wait_for_epoch` (`SDK/client.py:376-444`).
- **Legacy v10** (`LEGACY/bittensor/core/metagraph.py`): `sync(block, lite, subtensor)` → `neurons_lite`/`neurons` runtime call (`:1758-1786`), `_set_metagraph_attributes` builds numpy/torch arrays (`:705-760`), full mode densifies weights/bonds into n×n matrices via `_process_weights_or_bonds` (`:644-703`) and root weights (`:1815-1869`), stakes from `SubnetInfoRuntimeApi.get_subnet_state` (`:1871-1915`), extra info from `get_metagraph_info` = `get_selective_metagraph` with `field_indices` (`LEGACY/bittensor/core/subtensor.py:2643-2725`), then mechanism count/split (`:1917-1930`). Persistence: `save()` writes `~/.bittensor/metagraphs/network-{network}/netuid-{netuid}/block-{block}.pt` — a pickle (or `torch.save`) of `state_dict()` = `{netuid, network, version, n, block, consensus, validator_trust, incentive, emission, dividends, active, last_update, validator_permit, weights, bonds, uids, axons, neurons, alpha_stake, tao_stake, stake}`; `load()` picks the highest `block-N` file (`:94-145,595-618,762-805`). The lite-node note "only the previous 300 blocks" is in the sync docstring (`:1676-1686`). **(inferred)** rtao should not reproduce the pickle format; if persistence is wanted, the natural v11-compatible artifact is the raw SCALE bytes of the `get_metagraph` result plus `(block_hash, spec_version)`.

### 4.5 CLI rendering of the metagraph (what humans expect to see)

`btcli subnets metagraph` shows pool/emission sections and a per-neuron list with `uid, hotkey, coldkey, stake (total_stake), emission (alpha), incentive (u16/65535), dividends (u16/65535), active, validator_permit, last_update, axon`, sorted by stake desc; `--json` carries every per-uid field (`SDK/cli/metagraph_view.py:28-46,154-199`).

---

## 5. The codegen pipeline

### 5.1 Input

`python -m codegen [ws-endpoint]` (default `ws://127.0.0.1:9944`) connects, calls `SubstrateConnection.metadata_ir()` and writes `SDK/_generated/` (`sdk/python/codegen/__main__.py:17-28`, `codegen/metadata.py:40-51`). The IR comes from the Rust core's `Runtime::metadata_ir()` over the **V15** `MetadataVersioned` bytes the transport fetched (`Metadata_metadata_at_version(0x0f000000)`, V14 fallback) (`SDK/_transport/codec.py:479-513`, `runtime.py:292-310`). Note: the on-disk `META/finney.scale` is V16 (per the task brief) while the SDK pins V15 — rtao should decide which version to standardize on; V15 suffices for runtime APIs, V16 adds per-extension metadata used for transaction extensions.

### 5.2 The IR (`SDK/_transport/contract.py:182-246`)

```
MetadataIR{spec_version, pallets: [PalletIR{name, index, calls: [CallIR{name, args: [CallArgIR{name, type_ident}], docs}],
          errors: [ErrorIR{index, name, docs}], storage: [StorageIR{name, value_type_ident}], constants: [name]}],
          runtime_apis: [RuntimeApiIR{name, methods: [name]}]}
```

`type_ident` / `value_type_ident` is the last path segment of the registry type (`TaoBalance`, `NetUid`, `AccountId32`, `PerU16`), `Option<...>` kept with its payload, structural names for path-less types (`u64`, `Vec<u16>`, `(u16, u16)`, `[u8; 32]`, `Compact<u64>`), recursion bounded at depth 8 (`CORE/runtime/mod.rs:263-328`). The IR is "deliberately small and JSON-serializable: it captures only what the emitters currently use" (`codegen/metadata.py:1-9`). **No type ids, no hashers, no key types, no runtime-API in/out types, no events** are emitted.

### 5.3 Emitters (`sdk/python/codegen/emit_python.py`)

| File | Shape | Notes |
|---|---|---|
| `errors.py` | `ERRORS: dict[(pallet_index, error_index) -> ErrorInfo(pallet, name, docs)]` | 420 lines; backs `--names` gate and agent error reference (`:97-113`) |
| `calls.py` | `class <Pallet>: @staticmethod def <call>(arg: 'TypeIdent', ...) -> Call(module, function, {params})`; module-level aliases `TaoBalance = Any`, `u64 = int`, ... | string annotations survive to `__annotations__` so the intents layer can key unit enforcement on them (`:116-169`); 1,704 lines |
| `storage.py` | `class <Pallet>: <Item> = Item('<Pallet>', '<Item>', '<value_type_ident>')`; `Item.__iter__` yields only `(container, name)` | 477 lines, 277 SubtensorModule entries at spec 477 (`:227-240`) |
| `constants.py` | `Item('<Pallet>', '<Name>')` | (`:243-250`) |
| `runtime_apis.py` | `class <Api>: <method> = Method('<Api>', '<method>')` | sorted by API name (`:253-259`) |

Properties: deterministic (sorted) output; every emitted file is `compile()`d before write (`:262-275`); collision checks for pallet classes, item names and alias/preamble names (`:86-95`); keyword-named identifiers get a `_` suffix (`:43-45`); headers intentionally lack `spec_version` (`:22-30`). `ruff` is configured not to format `_generated/` so the drift comparison stays literal (`sdk/python/pyproject.toml:73-79`).

`emit_namespaces.py` generates `SDK/namespaces.pyi` from the live read registry (curated methods introspected + every registry read with a keyword-only `block: Optional[int] = None` unless the read already has a `block` param) and fails if a return annotation names a type not exported by one of eight listed modules, or if a read category has no namespace class (`sdk/python/codegen/emit_namespaces.py:38-50,159-226`).

### 5.4 Gates (`sdk/python/codegen/check.py`)

- `--drift <endpoint>`: regenerate from the node and compare each artifact's text; also fail on files in `_generated/` the emitter did not produce (`:32-53`).
- `--coverage`: every call in `COVERED_PALLETS` (SubtensorModule, Balances, Proxy, Utility, Multisig, Crowdloan, Swap, LimitOrders, MevShield, EVM, Commitments, AdminUtils, Contracts) must be wrapped by an intent (`cls.wraps`) or listed in `RAW_ONLY`; stale raw-only names and double listings fail; intents with `origin == "root"` must declare a `verify` read that exists in the read registry (`:56-346`).
- `--names`: every chain error name classifies to a semantic `ErrorCode`, every classified name still exists, descriptions exist and are non-empty, `CUSTOM_TRANSACTION_ERRORS` codes are in lockstep with `common/src/transaction_error.rs` (`frozenset(range(28)) | {255}`) (`:349-455`).
- `--units`: hyperparameter hand table agrees with the metadata-derived unit kind and every entry has a ≤38-char `short` (`:472-507`).
- `--namespaces`: `namespaces.pyi` matches the registry (`:458-469`).
- Commands: `just codegen-static`, `just regen` (codegen + `record_golden.py`), `just drift`, `just check-docs` (docs pages vs registries) (`sdk/python/justfile`). CI: offline gates in `runtime-checks.yml:508-540`; drift against the mainnet clone (`mainnet-clone-preview.yml:166`); AGENTS.md forbids hand-editing `_generated/` (`AGENTS.md:63`).

### 5.5 How runtime APIs and storage keys are actually described (at runtime, not in codegen)

- Storage: `Runtime::storage_entry(pallet, name) -> StorageInfo{name, prefix, modifier: "Default"|"Optional", hashers: ["Blake2_128Concat",...], key_types: [type id per hasher], value_type: id, default_bytes}`; a multi-hasher map's key tuple is split into its components (`CORE/runtime/mod.rs:26-40,369-408`). Python mirrors it as `StorageEntry` with `scale_info::N` strings (`SDK/_transport/codec.py:123-144,257-271`).
- Runtime APIs: `Runtime::apis: [RuntimeApiInfo{name, methods: [RuntimeApiMethodInfo{name, inputs: [{name, ty}], output, docs}]}]` from V15 (`CORE/runtime/mod.rs:82-100,483-504`); Python's `runtime_api_map` is `{api: {method: {name, inputs: [(name, type_string)], output: type_string, docs}}}` (`SDK/_transport/codec.py:471-475`); encoding walks `inputs` positionally or by name (`SDK/_transport/runtime_api.py:175-205`).
- Hasher table for the storage items the reads touch (from `META/explore/_storage_table.tsv`, columns name/modifier/kind/keys/value): `Weights`, `Bonds` `Map<Identity,Identity>` `(NetUidStorageIndex,u16)`; `Uids` `Map<Identity,Blake2_128Concat>` `(NetUid,AccountId32)` Optional; `Keys` `Map<Identity,Identity>` `(NetUid,u16)`; `Axons` `Map<Identity,Blake2_128Concat>` Optional; `LastUpdate`/`Incentive` `Map<Identity>` `NetUidStorageIndex`; `Emission`/`Dividends`/`Consensus`/`ValidatorTrust`/`ValidatorPermit`/`Active` `Map<Identity>` `NetUid`; `Lock` `Map<Blake2_128Concat,Identity,Blake2_128Concat>` `(AccountId32,NetUid,AccountId32)` Optional; `LockingColdkeys` `Map<Identity,Blake2_128Concat,Blake2_128Concat>` `()` value; `MinerCollateral` `Map<Identity,Blake2_128Concat,Blake2_128Concat>` Optional; `ChildKeys`/`ParentKeys` `Map<Blake2_128Concat,Identity>` `(AccountId32,NetUid)`; `PendingChildKeys` `Map<Identity,Blake2_128Concat>` `(NetUid,AccountId32)`; `TotalHotkeyAlpha`, `IsNetworkMember`, `AutoStakeDestination`, `DecayingLock` `Map<Blake2_128Concat,Identity>`; `TimelockedWeightCommits` `Map<Twox64Concat,Twox64Concat>` `(NetUidStorageIndex,u64)`; `AssociatedEvmAddress` `Map<Twox64Concat,Twox64Concat>` `(NetUid,u16)`; `MechanismCountCurrent`/`MechanismEmissionSplit`/`SubnetLeases`/`ColdkeySwapAnnouncements`/`ColdkeySwapDisputes` `Map<Twox64Concat>`; `Delegates`/`Owner`/`OwnedHotkeys`/`StakingHotkeys`/`AccountFlags`/`IdentitiesV2`/`BasketRate`/`RootClaimableThreshold`/`ChildkeyThresholdSuspended`/`SubnetIdentitiesV3` `Map<Blake2_128Concat>`; `Tempo`/`Burn`/`SubnetworkN`/`NetworksAdded`/`TokenSymbol`/`TotalAlphaStaked`/`MaxAllowedUids`/`CollateralLockShare`/`CollateralDrainRatio`/`SubnetMovingPrice`/`SubnetEmaTaoFlow`/`LastEpochBlock`/`PendingEpochAt`/`SubnetEpochIndex`/`BlocksSinceLastStep`/`OwnerLock` `Map<Identity>` `NetUid`; `TxRateLimit`/`MinDelegateTake`/`MaxDelegateTake` Plain. All `Identity`-hashed `NetUid` keys mean map iteration over subnets is ordered and cheap to recover.

### 5.6 How the docs/catalog layer attributes reads to chain surface

`read_targets(spec)` regex-scans the read's **source** for `st.<Pallet>.<Item>`, `api.<Api>.<method>`, `constants.<Pallet>.<Name>` and adds descriptors captured in the fetch closure (for `scalar_read`), then resolves each to a source line in `pallets/` for `/code` links (`website/apps/bittensor-website/scripts/generate.py:405-487`). Resulting `hits` kinds over `reads.json`: 86 storage, 32 runtime, 2 constant; 10 reads have no hits (listed in §3.12). Lesson for rtao: declare a read's chain targets **explicitly in its descriptor** (a static `targets: &[Target]` slice) rather than scraping source — a Rust registry can make this a compile-time constant, and the drift gate can then assert every target still exists in the metadata.

### 5.7 What an equivalent Rust codegen needs to emit

Derived from the above plus the Rust core's `Runtime` model (**(inferred)** recommendations marked):

1. **Name-based descriptor constants** per pallet for storage (`container`, `name`, key arity, hasher list, key type idents, value type ident, modifier), constants (name, type ident), calls (name, args with type idents, docs), errors (`(pallet_index, error_index) -> name, docs`), **events** (absent in the Python IR; needed for inclusion resolution and for subnet owner tooling), and runtime APIs (api, method, input `(name, type ident)` list, output type ident, docs, and the api_version from the runtime-api crate — e.g. `BetaBasketRuntimeApi` methods carry `#[api_version(2..5)]`, `pallets/subtensor/runtime-api/src/lib.rs:112-157`). The Python IR omits runtime-API signatures; rtao's IR should include them because the V15 metadata has them (`CORE/runtime/mod.rs:483-504`).
2. **Typed newtype vocabulary** from the registry paths: `TaoBalance(u64)`, `AlphaBalance(u64)`, `NetUid(u16)`, `MechId(u8)`, `NetUidStorageIndex(u16)`, `PerU16(u16)`, `AccountId32` — these exist as named types in metadata (`subxt explore` output above) and map directly onto Rust newtypes with unit semantics, replacing the Python `value_type_ident` strings + `RATIO_TYPE_DENOMINATORS` hand table.
3. **A runtime (not codegen) resolution path** for hashers/key types/value types keyed by spec version (exactly `CORE/runtime/mod.rs`), so archive-block reads and runtime upgrades mid-session keep working without regenerating; generated descriptors should be validated against the live `Runtime` at connect (name exists, key arity matches, value ident matches) — this is the Rust analogue of `--drift`, runnable continuously.
4. **Vendored chain-source enums that metadata cannot express**: `SelectiveMetagraphIndex` (0..=76), `HyperparamValue` variant semantics (metadata has the enum; the *unit* per hyperparameter name is a hand table), `NULL_UID_BUDGET`/`256` capacity rules, `GLOBAL_MAX_SUBNET_COUNT = 4096`, the collateral/lock constants, drand quicknet genesis/period, the `freeze_struct` hashes of the rpc_info structs as a layout-drift tripwire.
5. **Fixture emission for tests**: the golden/shape corpora (§6) plus a generated "every descriptor resolves" test against the checked-in metadata blob (`META/finney.scale` or the metadata embedded in `golden.json`).
6. **Optional typed decode via subxt codegen** (`META/finney_codegen.rs` exists, ~60k lines) for hot structs (`Metagraph`, `NeuronInfoLite`, `StakeInfo`, `DelegateInfo`, `SimSwapResult`, `BasketSummary`), **but** gated by a byte-exact drift test, because subxt-generated types are per-spec; keep a dynamic `Value` path (like `CORE/codec/value.rs`) for everything else and for cross-spec archive reads. (**inferred**)

---

## 6. Golden fixtures and the shape corpus — what is recorded and how rtao can reuse it

### 6.1 `sdk/python/tests/fixtures/golden.json` (1,508,346 bytes)

Recorded by `sdk/python/scripts/record_golden.py` against a localnet (refuses non-local endpoints unless `RECORD_GOLDEN_ALLOW_REMOTE=1`; it submits a real `//Alice` transfer) (`:17-50`). Contents (`jq` over the file; recorder sections cited):

| Key | Count / shape | What it pins |
|---|---|---|
| `network` | `{genesis_hash: 0x1dc00e76…ce452, spec_version: 461, transaction_version: 1, ss58_format: 42}` | the runtime the vectors were recorded under (`:153-160`) |
| `metadata` | `{block_hash, v15_hex (773,478 hex chars ≈ 387 KB), v14_hex (688,536)}` | raw `MetadataVersioned` blobs — enables fully offline replay; `conftest.golden_codec()` rebuilds the codec from `v15_hex` (`sdk/python/tests/conftest.py:25-36`) |
| `storage_keys` | 10 cases: `System.Account[Alice]`, `System.Events`, `Timestamp.Now`, `SubtensorModule.Tempo[1]`, `NetworksAdded[3]`, `Bonds[1,0]`, `Keys[1,0]`, `Alpha[hot,cold,1]`, `BlocksSinceLastStep[7]`, `Multisig.Multisigs[Alice, 0xab…]` → `key_hex`, `value_scale_type` | byte-exact key construction incl. multi-hasher NMaps (`:77-88,169-183`) |
| `calls` | 10: transfers, `add_stake`, `remove_stake`, `burned_register`, `set_weights`, `System.remark`, nested `Sudo.sudo`, `Utility.batch`, `Proxy.proxy` → `data_hex` | call composition incl. nested call splicing (`:90-110,185-237`) |
| `signature_payloads` | immortal + mortal (`period 64`) | (`:239-284`) |
| `extrinsics` | 2 with a fixed fake 64-byte signature | deterministic signed-extrinsic encoding + hash (`:286-311`) |
| `multisig` | 3 signer sets | `MultiAccountId` derivation (`:313-328`) |
| `ss58` | 4 dev keys | (`:330-334`) |
| `constants` | `Balances.ExistentialDeposit`, `Aura.SlotDuration`, `System.SS58Prefix` decoded | (`:112-116,336-340`) |
| `receipt`, `events`, `block` | the inclusion block of the live transfer: raw `chain_getBlock`, decoded header/extrinsics, raw+decoded `System.Events` | (`:342-358,380-387,438-446`) |
| `storage_values` | 8 cases incl. a map miss (`NetworksAdded[9999]`) → `raw_hex` (nullable) + `decoded` | Default/Optional miss semantics (`:118-127,360-378`) |
| `query_maps` | 4: `NetworksAdded`, `Tempo`, `Keys[1]` (double map, first key fixed), `System.Account` → `prefix_hex`, `raw_keys` (≤25), `raw_changes`, decoded `pairs` | map-key recovery (`:129-134,389-417`) |
| `runtime_calls` | 4: `NeuronInfoRuntimeApi.get_neurons_lite[1]`, `SubnetInfoRuntimeApi.get_subnet_hyperparams[1]`, `StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid[hot,cold,1]`, `AccountNonceApi.account_nonce[Alice]` → `encoded_params_hex`, `raw_hex`, `decoded` | runtime-API param encoding + result decoding (`:136-145,419-436`) |
| `payment_info` | keys + weight shape | (`:448-450`) |

Consumers: `tests/unit/test_codec_golden.py` (calls, payloads, parts concatenation, extrinsic encode/decode, multisig, ss58, constants, storage values, events, runtime_api_map presence, IR shape — including the assertions that `add_stake` args are `AccountId32/NetUid/TaoBalance` and `Delegates`'s value ident is `PerU16`, `:184-216`) and `tests/unit/test_storage_golden.py` (keys, batch keys, miss semantics, bulk == single decode, map pairs). The Rust core replays the same file natively: `CORE/codec/mod.rs:209` (`composed_calls_match_the_golden_vectors`), `:284` (storage keys), `:313` (map keys), `:370` (signature payloads), `:429` (signed extrinsics), `:476` (block extrinsics), `:492` (multisig).

### 6.2 `sdk/python/tests/fixtures/shape_corpus/corpus.json` (1,076,611 bytes)

Recorded offline by `sdk/python/scripts/record_shape_corpus.py` from the golden metadata's portable registry: for every type id, SCALE bytes are **generated** from the type definition with a deterministic RNG (`random.Random(type_id * 1000 + i)`), decoded once, and stored as `{id, name (path), samples: [{scale_hex, decoded}]}`; `SAMPLES_PER_TYPE = 3`, `MAX_DEPTH = 8`, `MAX_SEQ_LEN = 3`, enum sample count = `max(3, min(n_variants, 16))` so every variant up to 16 is visited; skipped types record a reason (`:36-41,151-195`). Current file: `spec_version 461`, `codec "bittensor-core"`, `types_total 802`, `types_covered 797`, `skipped 5` (all "uninhabited variant type"), 2,899 samples. `tests/unit/test_shape_corpus.py` asserts >95% coverage and **exact** equality of decoded shapes after the same `jsonable()` scrub (`:39-75`); the core replays it in Rust (`CORE/codec/mod.rs:115 decoder_reproduces_the_shape_corpus`) through `to_corpus_json` (`CORE/codec/value.rs:55-102`).

The shape conventions the corpus pins (and that rtao's dynamic decoder must choose to honour or deliberately diverge from): AccountId32 → ss58 string; `Vec<u8>` → utf-8 string if valid else `0x`-hex; `[u8; N]` → `0x`-hex (`[u8;0]` → `[]`); enums → `{"Variant": payload}` with unit variants as bare strings; `Option` unwraps (`None` → null); newtype composites unwrap; `BTreeMap` → dict with native keys; `Era` → `"00"` / `(period, phase)`; `Compact` unwraps to int; u128 above i128 kept exact; u256 as decimal string in JSON (`CORE/codec/decode.rs:1-8,355-400`, `value.rs:10-33`). Fixed-point types decode as `{"bits": int}` composites (seen throughout the reads: `_fixed_to_float`, `_i96f32_rao`, etc.).

### 6.3 Behavioural test tables (also reusable)

- `tests/harness/samples.py` — `READ_SAMPLES`: sample params for every registered read (meta-test fails if any read lacks one; `tests/unit/test_reads_table.py:76-98`).
- `tests/harness/fake_substrate.py` — `FakeSubstrate` over dicts: `seed`, `seed_default`, `seed_map`, `seed_constant`, `seed_runtime`; permissive `DEFAULT_STORAGE` / `DEFAULT_CONSTANTS` / `DEFAULT_RUNTIME` chosen so preflights pass (`:34-77`). Every read is dispatched both at head and pinned to block 90 (`test_reads_table.py:100-110`), proving reads only use the view surface. Spot semantic checks: `balance` is netuid-0 rao, `uid` None when unregistered, `block_time` 0.25 from `SlotDuration 250` (`:113-137`).
- Pure-function unit tests worth porting as vectors: commitment lifecycle (`tests/unit/test_v446_accounting_and_commitments.py`), lock ownership gate (`test_lock_ownership_gate.py`), Null raw-weight conformance with 2,500-entry rows (`test_null_weights.py:13-45`), hyperparameter unit resolution (`test_hyperparams_units.py`).

### 6.4 Reuse plan for rtao (**inferred** from the above)

1. Vendor `golden.json` and `corpus.json` into rtao's test tree as-is; build the decoder from `golden.json.metadata.v15_hex` exactly like `golden_codec()`; assert storage keys, call bytes, payloads, extrinsic bytes and map-key recovery byte-for-byte (shape-independent — these are the highest-value, zero-cost vectors).
2. For decoded-value vectors, implement a `to_corpus_json` projection of rtao's dynamic value type and compare against `corpus.json` / `golden.json.{storage_values,query_maps,runtime_calls,events,block}.decoded`. Where rtao deliberately diverges (e.g. keeping AccountId32 as bytes, enums as tagged structs), keep the projection layer so the corpus still runs.
3. Re-record both fixtures against the spec-477/478 node with rtao's own recorder (the Python scripts are the spec), and keep *both* spec-461 and current-spec fixtures to exercise cross-spec decoding (archive reads).
4. Port `READ_SAMPLES` + `FakeSubstrate` as a trait-object fake for a Rust `Substrate`/`View` trait so every read is exercised offline at head and pinned.

---

## 7. Performance notes, payload sizes, bulk decode, large subnets

- **Bench scripts**: `sdk/python/scripts/bench_transport.py` measures cold/warm connect (metadata download + codec build), `chain_getHeader` RTT ×10, single `System.Account` query (key build / network / decode), `get_metagraph` per netuid (payload MB, `state_call` time, hex→bytes, SCALE decode median of 3, decode share and MB/s), `query_map System.Account` paged (`--pages 5 --page-size 1000`; entries/s), sign+assemble ×50, ss58 ×10k (`:88-271`). `bench_neurons.py` probes `get_neurons_lite` and `get_neurons` (weights/bonds included) across `--netuids 1,3,8,19,64` and prints MB, item count, RPC vs decode ms and MB/s (`:1-67`). Neither script's output is committed; the spec table is the only recorded result.
- **Recorded results** (`sdk/bittensor-core-spec.md:360-382`, finney, 2026-07-09/10): codec build 337 ms → **4 ms**; metagraph/neurons SCALE decode 3–6 MB/s → **60–77 MB/s**; `compose_call` 1.1 ms → **<50 µs**; sign+assemble 0.1 ms → **0.03 ms** (29k/s); `query_map` decode 92k → **599k entries/s**; warm connect ~1.1 s → **~0.6 s** ("network floor"). What paid: one FFI crossing per page, SCALE+ss58 off-GIL, rayon fan-out above `PARALLEL_THRESHOLD = 64` items (`CORE/codec/batch.rs:22-49`), per-call repeated-object cache (`sdk/bittensor-core-py/src/values.rs:9-55`), limb-based base58 (~6× faster).
- **Bulk decode entry points** in the core: `decode_batch(type_strings, datas)` (dedups consecutive type strings), `decode_map_page(entry, raw_keys, raw_values, fixed)`, `decode_map_changes(entry, changes, fixed)` (hex parsing inside the parallel section) (`CORE/codec/batch.rs:51-135`); `pyo3` wrappers release the GIL around them (`sdk/bittensor-core-py/src/runtime.rs:191,364,385`).
- **Paging**: Python `query_map` page size 100 (`SDK/_transport/interface.py:304`); the native Rust client uses `STORAGE_PAGE_SIZE = 1_000` and a cursor-repeat guard (`CORE/client.rs:33,804-845`). Public endpoints enforce storage work rate limits (JSON-RPC `-32004 "storage work rate limit exceeded"` is classified as a terminal policy refusal; `SDK/_transport/rpc.py:52-57,190-231`), which argues for conservative page sizes and for preferring runtime APIs (`get_metagraph`, `get_neurons_lite`) over map scans on hot paths.
- **Payload shape**: a `get_metagraph` response is O(num_uids) compact ints per column plus 32-byte keys ×2 per uid, `AxonInfo` (≈ 16+ bytes) and `Option<ChainIdentityV2>` per uid; `get_neurons` adds every validator's sparse weight and bond row. The `weights` read builds a dict-of-dicts (`SDK/reads/weights.py:41-46`): on a 2,500-UID Null subnet with dense rows that is up to 6.25M entries — rtao should use CSR/sparse rows and avoid `f64` normalization in the storage layer.
- **Large subnets / Null consensus**: `NULL_UID_BUDGET = 2_500` shared across mechanisms, Yuma 256; capacity `floor(budget / mechanism_count)` (`pallets/subtensor/src/subnets/mechanism.rs:34-35`; `docs/guides/null-consensus.mdx:42-43`; `docs/internals/null-consensus-testnet.mdx:32`); weight rows up to 2,500 entries via `setMechanismWeightsV2`, timelocked ciphertexts up to 32 KiB; released selectors keep 4,096-entry / 5,000-byte bounds (`docs/guides/null-consensus.mdx:209-212`). The SDK's raw-u16 weight path refuses to rescale and preserves `[65534, 1, 1, ...]` exactly (`tests/unit/test_null_weights.py:13-28`). Owner pruning back to Yuma is a restartable batch flow reading `NetworksAdded, SubnetOwner, SubnetworkN, NullPruningTarget, MechanismCountCurrent, MinAllowedUids, MaxAllowedUids, SubnetEpochConsensus, Tempo, LastEpochBlock, PendingEpochAt, AdminFreezeWindow` at one pinned block (`SDK/cli/null_pruning.py:46-80`).
- **Null stake election** uses a cached stake path on chain (`get_null_stake_weights_for_network`, `stake_utils.rs:326-345`), so `get_metagraph` on Null subnets is still the general O(n) path — expect the heaviest `state_call` payloads there.
- **Archive depth**: lite nodes keep ~300 blocks of state (legacy sync note, `LEGACY/bittensor/core/metagraph.py:1676-1686`); v11 handles this by archive fallback (§1.3).

---

## 8. Decisions for the rtao architects

1. **Descriptor model**: adopt the v11 split — name-based generated descriptors (for discoverability and the drift gate) + runtime metadata resolution (hashers/types per spec version). Do **not** bake scale-info type ids or hashers into generated code (spec §4.2 rationale; `sdk/bittensor-core-spec.md:129-137`). Recommended: generated `const` descriptors validated at connect against the live `Runtime`, plus an optional subxt-typed fast path for ~6 hot structs guarded by a byte-exact fixture test.
2. **Read registry shape**: keep the "view" abstraction (one trait with `query/query_map/query_batch/runtime_call/constant/at/...`) so each read is written once and runs at head or pinned; make pinning explicit and auditable (the Python convention `view = await view.at()` is easy to forget — see the unpinned gathers in §1.3). Recommended: reads declare `Pinning::Required` and the dispatcher pins for them.
3. **Units**: encode `TaoBalance`/`AlphaBalance`/`PerU16`/`NetUid`/`MechId`/`NetUidStorageIndex` as newtypes generated from metadata paths; keep a small hand table only for blocks/epochs/per-million/u64-normalized kinds and fixed-point widths (U64F64, I96F32, I64F64, I32F32), with a `--units` style test that the hand table never contradicts a metadata identity. Decide whether `Balance` carries the netuid at the type level (`Alpha<const N>` is impractical; a runtime netuid field with checked arithmetic as in `SDK/balance.py` is the proven design).
4. **Metagraph**: ship a typed `Metagraph` decoded from `get_metagraph` (not from storage scans), drop `rank/trust/pruning_score` or mark them deprecated-zero, expose `mechid`, add `get_selective_metagraph` with a vendored index enum for cheap partial refreshes, keep commitments as an opt-in second fetch pinned to the same block. Persistence: raw SCALE + `(block_hash, spec_version)` if at all.
5. **Fixtures**: vendor `golden.json` + `corpus.json`, re-record at current spec with a Rust recorder that mirrors `record_golden.py`/`record_shape_corpus.py`, and run them in `cargo test` exactly as `CORE/codec/mod.rs` does.
6. **Transport**: tokio + `jsonrpsee`/`subxt`-style ws with the Python session's semantics worth copying — resubmit idempotent pending frames on reconnect, never resubmit `author_submit*`, treat policy refusals as terminal, pool rotation, archive fallback on `State already discarded`, finalized-only block-hash cache, head-codec TTL ≈ one block, V15 metadata via `Metadata_metadata_at_version` with disk cache keyed by `(genesis, spec)`.
7. **Docs/catalog**: generate the read catalog (`name, summary, params, param_docs, category, hits`) from the Rust registry at build time; make `hits` declarative.

## 9. Open questions

1. Metadata version: SDK pins V15; the fetched `META/finney.scale` is V16. Which does rtao standardize on, and does the drift gate compare V15 or V16 IR?
2. `total_stake` is declared `TaoBalance` on the wire but is alpha-equivalent (`alpha + tao·tao_weight`); confirm the intended unit with the runtime team before typing it.
3. Should rtao expose the deprecated `rank`/`trust`/`pruning_score`/`subnet_emission`/`tao_dividends_per_hotkey`/`pending_root_emission` fields at all (all hard-zero in the runtime today), or drop them and rely on `raw`?
4. `SelectiveMetagraphIndex` numbering lives only in pallet source; is there (or should there be) a runtime constant/API exposing it, so clients do not have to vendor it?
5. `get_all_beta_pricing` is paged with a chain-side cap ("limit 0 = full page"); the exact cap is not in the SDK — needed for rtao's paging loop bounds.
6. The Python reads still hand-build five descriptors (`collateral.py:15-17`, `root.py:30-44`) despite the generated catalog containing them; confirm whether that is only staleness (so rtao can treat the generated catalog as complete).
7. `hyperparams.py` hand-tables 50 names but `get_subnet_hyperparams_v3` returns a runtime-defined, forward-compatible list; decide whether rtao's unit table is keyed by hyperparameter name (SDK approach) or by the `HyperparamValue` variant plus storage identity (more automatic, loses blocks/epochs semantics).
8. No committed benchmark output exists beyond the spec table; rtao should establish its own baselines with the same methodology (`bench_transport.py` sections) on the same endpoints.
9. `Owner`, `Keys` and `SubnetOwnerHotkey` are `Default`-modifier maps whose default is 32 zero bytes (`META/explore/_storage_table.tsv`), so "unowned"/"unregistered" decodes to the all-zero account rather than `None`; the v11 reads `hotkey_owner` (`SDK/reads/neurons.py:75-78`) and `miner_collateral` (`SDK/reads/collateral.py:88-92`) test for `None` and therefore cannot detect that case (**inferred**; not verified against a live node). rtao should represent these as `Option<AccountId32>` by comparing against the zero account explicitly, and decide whether to treat that as a documented divergence from v11.
