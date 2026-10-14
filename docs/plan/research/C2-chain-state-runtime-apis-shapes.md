# C2 — Subtensor chain read surface and data shapes (spec 477 live / 478 at `850f275`)

Research report for the **rtao** planning effort. Audience: expert Rust/Substrate engineer. Every claim below is tied to an on-disk source; items that are estimates or inferences are labelled as such.

Path shorthands used in citations:

- `SUB/` = `SP/upstream/subtensor/` (RaoFoundation/subtensor monorepo @ 850f275, runtime `spec_version: 478` — `SUB/runtime/src/lib.rs:241`)
- `META/` = `SP/meta/` (live finney metadata V16, spec 477; `finney.scale`, `finney.json`, `finney_codegen.rs`, `explore/`)
- `DOCS/` = `SP/docs/`
- `PY/` = `SUB/sdk/python/bittensor/`

Derived artifacts I generated during this research (all under `META/explore/`): `_all_api_methods.txt` (every runtime-API method with exact SCALE in/out shapes from `subxt explore`), `_api_v2v3.txt` (the `get_subnet_hyperparams_v2/v3`, `get_subnet_info_v2`, `get_subnets_info_v2` shapes), `_all_events.txt` (every SubtensorModule event shape), `_all_constants.txt` (every SubtensorModule constant with value), `_storage_table.tsv` (all 277 SubtensorModule storage entries: modifier, hashers, key type, value type, default bytes, docs).

---

## 0. Executive summary (what matters most for rtao)

1. **Everything a neuron needs is reachable through `state_call` runtime APIs + plain storage reads; the custom `subnetInfo_*`/`neuronInfo_*` JSON-RPC methods are a legacy veneer over the same runtime APIs and lag behind them** (no `get_subnet_hyperparams_v3`, no `get_next_epoch_start_block` custom RPC exists: `SUB/pallets/subtensor/rpc/src/lib.rs:21-176`). The v11 Python SDK never calls them; it uses `state_call` with `"{Api}_{method}"` (`PY/_transport/runtime_api.py:229-242`). rtao should do the same.
2. **No runtime API on spec 477 returns `Vec<u8>` (legacy double-encoding is gone)**: every output in `META/finney.json` is a typed SCALE value (verified by walking the `apis[].methods[].output` type ids; see §1.0). The Python "legacy registry" exists only for historical blocks (`PY/_transport/runtime_api.py:1-13,167-171`).
3. **API versions on finney**: `SubnetInfoRuntimeApi` v2, `BetaBasketRuntimeApi` v5, all others v1 (`META/finney.json` `apis[].version`; §1.0). `get_subnet_hyperparams` and `_v2` are `#[deprecated]` in favour of `_v3` (`SUB/pallets/subtensor/runtime-api/src/lib.rs:49-54`).
4. **Several struct fields are dead but still on the wire**: `Metagraph.subnet_emission=0`, `pending_root_emission=0`, `tao_dividends_per_hotkey` all zero, `pruning_score/trust/rank` empty (`SUB/pallets/subtensor/src/rpc_info/metagraph.rs:676-680,715,724,786-787,802-803`); `NeuronInfo.rank=0,trust=0,pruning_score=65535` (`neuron_info.rs:131,135,142`); `StakeInfo.locked=0,tao_emission=0,drain=0` (`stake_info.rs:70-82`); `SubnetInfo.emission_values=0,network_modality=0,network_connect=[]` (`subnet_info.rs:194-217`); `DynamicInfo.emission=0,pending_root_emission=0` (`dynamic_info.rs:58,68`). Storage items `Rank`, `Trust`, `PruningScore` no longer exist (absent from `META/explore/SubtensorModule_storage.txt`).
5. **Mechanisms ("mechagraphs") change the storage keys**: `Weights`, `Bonds`, `LastUpdate`, `Incentive`, `ConsensusByMechanism`, `WeightCommits`, `TimelockedWeightCommits`, `CRV3WeightCommits(V2)` are keyed by `NetUidStorageIndex = netuid + mecid*4096` (`SUB/pallets/subtensor/src/subnets/mechanism.rs:28-43`), while `Keys`, `Uids`, `Consensus`, `Dividends`, `Emission`, `Active`, `ValidatorPermit`, `ValidatorTrust`, `Tempo`, … are keyed by `NetUid` (§2). `get_mechagraph` returns a `Metagraph` whose `netuid` field carries the *storage index* (`metagraph.rs:838-862`); decode `mecid = idx / 4096`, `netuid = idx % 4096` (the Python does: `PY/metagraph.py:384`).
6. **Epoch scheduling is stateful, not modular**: next epoch = `LastEpochBlock + tempo`, pulled earlier by `PendingEpochAt` (owner `trigger_epoch`), deferred by the per-block cap `MaxEpochsPerBlock` (default 2), and skipped (slot still consumed) on inconsistent state (`SUB/pallets/subtensor/src/coinbase/run_coinbase.rs:1217-1276,373-486`). The legacy `(block + netuid + 1) % (tempo + 1)` formula is retained only for a one-shot migration and is explicitly "NOT used by live commit-reveal logic" (`SUB/pallets/subtensor/src/subnets/weights.rs:1510-1522`). Use `SubnetInfoRuntimeApi::get_next_epoch_start_block` and watch `LastEpochBlock`.
7. **Commit-reveal timing is in epochs via `SubnetEpochIndex`**: commits are tagged with `current_epoch_with_lookahead` and reveal exactly at `commit_epoch + RevealPeriodEpochs` (`weights.rs:1451-1488`); CRv3/timelocked commits are auto-revealed by the chain in `on_initialize` → `block_step` → `reveal_crv3_commits` using drand pulses (`coinbase/block_step.rs:7-42,87-105`; `coinbase/reveal_commits.rs:39-66`). `CommitRevealWeightsEnabled` defaults to **true** (`META/explore/_storage_table.tsv`, default `[1]`) and `CommitRevealWeightsVersion` is **4**.
8. **Block time**: `MILLISECS_PER_BLOCK = prod_or_fast!(12000, 250)` (`SUB/common/src/lib.rs:183`); the `fast-runtime` cargo feature (`SUB/common/Cargo.toml:37`, `SUB/pallets/subtensor/Cargo.toml:168`, `SUB/scripts/localnet.sh:47`) selects 250 ms. Live finney `Aura.SlotDuration = 12000`, `Timestamp.MinimumPeriod = 6000` (subxt explore of `META/finney.scale`). Python detects block time from `Aura.SlotDuration` (`PY/_substrate.py:423-433`), and `INITIAL_SUBNET_TEMPO = prod_or_fast!(360, 10)` (`SUB/runtime/src/lib.rs:904`).
9. **Payload sizes (estimated, §5)**: `get_metagraph` ≈ 0.4 KB + ~205 B/UID → ~53 KB @256, ~210 KB @1024, ~510 KB @2500; `get_neurons_lite` ≈ 195 B/UID; `get_neurons` (full) adds the weight+bond matrices (~12 B per nonzero (validator,miner) pair) and is dominated by them. `get_subnet_state.emission_history` does `S×n` storage reads (`show_subnet.rs:55-67`) — avoid. Yuma subnets cap at `max_allowed_uids ≤ 256` (`epoch_uid_budget` → `DefaultMaxAllowedUids` = `InitialMaxAllowedUids` = 256); Null-consensus subnets may reach 2 500 (`mechanism.rs:35,116-121`).

---

## 1. Runtime API inventory (complete)

### 1.0 Versions, output typing, and discovery

From `META/finney.json` (`apis[]`): `DelegateInfoRuntimeApi v1`, `NeuronInfoRuntimeApi v1`, `SubnetInfoRuntimeApi v2`, `StakeInfoRuntimeApi v1`, `SubnetRegistrationRuntimeApi v1`, `BetaBasketRuntimeApi v5`, `ProxyFilterRuntimeApi v1`, `SwapRuntimeApi v1`, `ShieldApi v1`, plus standard `AccountNonceApi`, `TransactionPaymentApi`, `TransactionPaymentCallApi`, `Core`, `Metadata`, `BlockBuilder`, `TaggedTransactionQueue`, `OffchainWorkerApi`, `SessionKeys`, `GrandpaApi`, `AuraApi`, `GenesisBuilder`, `EthereumRuntimeRPCApi`, `ConvertTransactionRuntimeApi`, `ContractsApi` (`META/explore/_runtime_apis.txt`). (`BabeApi` appears in that list too but there is no Babe pallet — `subxt explore pallet Babe` fails — so it is an artefact of the Core/consensus trait set; the chain is Aura+Grandpa.)

The runtime trait declarations: `SUB/pallets/subtensor/runtime-api/src/lib.rs:30-160`; implementations: `SUB/runtime/src/lib.rs:2429-2800` (SubnetInfo impl at 2462-2550; StakeInfo 2552-2584; SubnetRegistration 2586-2590; BetaBasket `#[api_version(5)]` 2592; ProxyFilter 2650; Swap 2715; Shield 2795; AccountNonce 1972; TransactionPayment 1978; TransactionPaymentCall 2003).

**Output typing check**: walking `apis[].methods[].output` in `META/finney.json` shows no method whose output resolves to `Vec<u8>`; outputs are `Option<Struct>`, `Vec<Struct>`, `Vec<Option<Struct>>`, newtypes (`TaoBalance`), `u64`, `bool`, `BTreeMap`, `FixedU128`, tuple. The Python legacy path triggers only when "the method's declared output is `Vec<u8>`" or the runtime predates V15 metadata (`PY/_transport/runtime_api.py:165-171`) — i.e. never on current finney. **rtao can rely on metadata-driven encoding/decoding for every call; no hand-written legacy decoders are needed unless reading archive blocks older than the V15 switch.**

Call wire format: `state_call("{Api}_{method}", scale(params) hex, block_hash?)` (`PY/_transport/runtime_api.py:239-242`). With subxt: `api.runtime_api().at(hash).call(...)` or `call_raw("SubnetInfoRuntimeApi_get_metagraph", Some(&bytes))` — the ecosystem crates do exactly this (`ErgodicLabs_btt` uses `subxt::dynamic::runtime_api_call`, `inference-labs-inc_btlightning` uses `call_raw`; §8).

### 1.1 `SubnetInfoRuntimeApi` (v2) — `META/explore/_all_api_methods.txt:1-1636`, `_api_v2v3.txt`

| Method | Input (SCALE) | Output | Notes / source |
|---|---|---|---|
| `get_subnet_info(netuid: NetUid)` | `NetUid(u16)` | `Option<SubnetInfo<AccountId32>>` | legacy v1 shape, no identity; `subnet_info.rs:176-220` |
| `get_subnets_info()` | – | `Vec<Option<SubnetInfo>>` | iterates `NetworksAdded`; `subnet_info.rs:222-242` |
| `get_subnet_info_v2(netuid)` | `NetUid` | `Option<SubnetInfov2>` | adds `identity: Option<SubnetIdentityV3>`; `subnet_info.rs:244-291` |
| `get_subnets_info_v2()` | – | `Vec<Option<SubnetInfov2>>` | `subnet_info.rs:293-313` |
| `get_subnet_hyperparams(netuid)` **deprecated** | `NetUid` | `Option<SubnetHyperparams>` | `runtime-api/src/lib.rs:49-50`; `subnet_info.rs:315-376` |
| `get_subnet_hyperparams_v2(netuid)` **deprecated** | `NetUid` | `Option<SubnetHyperparamsV2>` | `runtime-api/src/lib.rs:51-52`; `subnet_info.rs:378-453` |
| `get_subnet_hyperparams_v3(netuid)` `#[api_version(2)]` | `NetUid` | `Option<Vec<HyperparamEntry>>` | forward-compatible name/value list; `subnet_info.rs:126-173,468-649` |
| `get_all_dynamic_info()` | – | `Vec<Option<DynamicInfo>>` | `dynamic_info.rs:75-82` |
| `get_dynamic_info(netuid)` | `NetUid` | `Option<DynamicInfo>` | `dynamic_info.rs:36-74` |
| `get_all_metagraphs()` | – | `Vec<Option<Metagraph>>` | one full metagraph per subnet — very heavy; `metagraph.rs:829-836` |
| `get_metagraph(netuid)` | `NetUid` | `Option<Metagraph>` | `metagraph.rs:653-827` |
| `get_all_mechagraphs()` | – | `Vec<Option<Metagraph>>` | one per (subnet, mecid); `metagraph.rs:865-875` |
| `get_mechagraph(netuid, mecid: MechId)` | `(NetUid, MechId(u8))` | `Option<Metagraph>` | same struct; `netuid` field = storage index; only `last_update` and `incentives` re-read per mechanism; `metagraph.rs:838-863` |
| `get_subnet_state(netuid)` | `NetUid` | `Option<SubnetState>` | includes `emission_history: Vec<Vec<Compact<AlphaBalance>>>` (S×n reads); `show_subnet.rs:84-165` |
| `get_selective_metagraph(netuid, metagraph_indexes: Vec<u16>)` | `(NetUid, Vec<u16>)` | `Option<SelectiveMetagraph>` | §1.1.3 index table; `metagraph.rs:877-891` |
| `get_selective_mechagraph(netuid, subid: MechId, metagraph_indexes: Vec<u16>)` | `(NetUid, MechId, Vec<u16>)` | `Option<SelectiveMetagraph>` | `netuid` field forced to the storage index; `metagraph.rs:893-912,1518-1556` |
| `get_coldkey_auto_stake_hotkey(coldkey, netuid)` | `(AccountId32, NetUid)` | `Option<AccountId32>` | reads `AutoStakeDestination`; `subnet_info.rs:455-460` |
| `get_subnet_to_prune()` | – | `Option<NetUid>` | lowest moving price among non-immune subnets; `coinbase/root.rs:344+` |
| `get_subnet_account_id(netuid)` | `NetUid` | `Option<AccountId32>` | `SubtensorPalletId.into_sub_account_truncating(netuid)`; `subnets/subnet.rs:643-652` |
| `get_next_epoch_start_block(netuid)` | `NetUid` | `Option<u64>` | `min(LastEpochBlock+tempo, PendingEpochAt)`; `None` if tempo==0; `run_coinbase.rs:1262-1276` |
| `get_block_emission()` | – | `TaoBalance(u64)` | `calculate_block_emission()`; `runtime/src/lib.rs:2543-2548` |

Example `subxt explore` input encodings (from the tool): `get_metagraph` → `((55210))` i.e. a single `NetUid(u16)`; `get_selective_metagraph` → `((31387), (31387, 9154))`.

#### 1.1.1 `Metagraph<AccountId32>` — field-by-field (`metagraph.rs:14-118`; built at `653-827`; identical SCALE shape confirmed in `META/explore/_all_api_methods.txt:679-850` and `META/finney_codegen.rs:56883`)

Units legend: **rao** = 1e-9 TAO (`TaoBalance(u64)`); **alpha-rao** = 1e-9 alpha of that subnet (`AlphaBalance(u64)`); **PerU16** = `u16` parts-per-65535 (`sp_arithmetic::PerU16`); **Compact<…>** = SCALE compact; **I96F32** = `substrate_fixed::FixedI128<U32>` (`bits: i128`, value = bits / 2^32).

Subnet-level header:

| Field | Wire type | Source / semantics |
|---|---|---|
| `netuid` | `Compact<NetUid>` | subnet id; for mechagraphs the storage index (`netuid + 4096*mecid`) |
| `name` | `Vec<Compact<u8>>` | UTF-8 bytes, **each byte compact-encoded** (bytes ≥ 64 take 2 bytes); `get_name_for_subnet` |
| `symbol` | `Vec<Compact<u8>>` | same encoding quirk; `TokenSymbol` storage |
| `identity` | `Option<SubnetIdentityV3>` | `SubnetIdentitiesV3` storage |
| `network_registered_at` | `Compact<u64>` | block; `NetworkRegisteredAt` |
| `owner_hotkey` / `owner_coldkey` | `AccountId32` | `SubnetOwnerHotkey` / `SubnetOwner` |
| `block` | `Compact<u64>` | block at which the call executed |
| `tempo` | `Compact<u16>` | `Tempo` |
| `last_step` | `Compact<u64>` | `LastMechansimStepBlock` (block of last *successful* epoch distribution; `run_coinbase.rs:508`) |
| `blocks_since_last_step` | `Compact<u64>` | `current_block - last_step` computed at call time (`metagraph.rs:683-685`), **not** the `BlocksSinceLastStep` storage item (which is capped at `tempo+1`) |
| `subnet_emission` | `Compact<u64>` | **always 0, "DEPRECATED"** (`metagraph.rs:715`) |
| `alpha_in` | `Compact<AlphaBalance>` | pool alpha reserve `SubnetAlphaIn` (alpha-rao) |
| `alpha_out` | `Compact<AlphaBalance>` | alpha outstanding `SubnetAlphaOut` |
| `tao_in` | `Compact<TaoBalance>` | pool TAO reserve `SubnetTAO` (rao) — despite the source comment "tao injected per block" |
| `alpha_out_emission` / `alpha_in_emission` / `tao_in_emission` | compacts | per-block injection terms `SubnetAlphaOutEmission` / `SubnetAlphaInEmission` / `SubnetTaoInEmission` |
| `pending_alpha_emission` | `Compact<AlphaBalance>` | `PendingValidatorEmission + PendingServerEmission` |
| `pending_root_emission` | `Compact<TaoBalance>` | **always 0** (`metagraph.rs:724`) |
| `subnet_volume` | `Compact<u128>` | cumulative TAO volume `SubnetVolume` |
| `moving_price` | `I96F32` (`FixedI128<U32>`, `bits:i128`) | EMA price in TAO/alpha; value = bits/2^32 (Python: `PY/metagraph.py:31-32,398`) |
| `rho`, `kappa` | `Compact<u16>` | `Rho`, `Kappa` (kappa is PerU16-scaled: 32767 ≈ 0.5) |
| `min_allowed_weights`, `max_weights_limit` | `Compact<u16>` | `MinAllowedWeights`, `MaxWeightsLimit` |
| `weights_version` | `Compact<u64>` | `WeightsVersionKey` |
| `weights_rate_limit` | `Compact<u64>` | `WeightsSetRateLimit` (blocks) |
| `activity_cutoff` | `Compact<u64>` | **effective blocks** = `ActivityCutoffFactorMilli × tempo / 1000`, min 1 (`utils/misc.rs:639-647`) |
| `max_validators` | `Compact<u16>` | `MaxAllowedValidators` |
| `num_uids`, `max_uids` | `Compact<u16>` | `SubnetworkN`, `MaxAllowedUids` |
| `burn` | `Compact<TaoBalance>` | current registration burn (rao) |
| `difficulty`, `min_difficulty`, `max_difficulty` | `Compact<u64>` | PoW |
| `registration_allowed`, `pow_registration_allowed` | `bool` | |
| `immunity_period` | `Compact<u16>` | blocks |
| `min_burn`, `max_burn` | `Compact<TaoBalance>` | rao |
| `adjustment_alpha` | `Compact<u64>` | u64-scaled EMA factor |
| `adjustment_interval`, `target_regs_per_interval`, `max_regs_per_block` | `Compact<u16>` | |
| `serving_rate_limit` | `Compact<u64>` | blocks between `serve_axon` |
| `commit_reveal_weights_enabled` | `bool` | |
| `commit_reveal_period` | `Compact<u64>` | `RevealPeriodEpochs` (**epochs**) |
| `liquid_alpha_enabled` | `bool` | |
| `alpha_high`, `alpha_low` | `Compact<u16>` | `AlphaValues` (u16-scaled 0..65535) |
| `bonds_moving_avg` | `Compact<u64>` | parts-per-1e6 (default 900000 → EMA alpha 0.1) |

Per-UID columns (all `Vec` of length `n = SubnetworkN`, index = uid; except where noted):

| Field | Wire type | Source / semantics |
|---|---|---|
| `hotkeys` | `Vec<AccountId32>` | `Keys[netuid][uid]` |
| `coldkeys` | `Vec<AccountId32>` | `Owner[hotkey]` |
| `identities` | `Vec<Option<ChainIdentityV2>>` | `IdentitiesV2[coldkey]` (coldkey identity, not hotkey) |
| `axons` | `Vec<AxonInfo>` | `Axons[netuid][hotkey]`, zeroed `AxonInfo` when unset (`subnets/serving.rs:185-200`) |
| `active` | `Vec<bool>` | `Active` (written by epoch) |
| `validator_permit` | `Vec<bool>` | `ValidatorPermit` |
| `pruning_score` | `Vec<Compact<u16>>` | **always empty** (deprecated) |
| `last_update` | `Vec<Compact<u64>>` | `LastUpdate[NetUidStorageIndex]` — block of last set_weights/commit |
| `emission` | `Vec<Compact<AlphaBalance>>` | `Emission` — **per-tempo** alpha-rao paid at the last epoch (docs: `SUB/docs/concepts/emissions.mdx` "Epochs" para) |
| `dividends`, `incentives`, `consensus`, `trust` | `Vec<Compact<PerU16>>` | `Dividends`, `Incentive[storage index]`, `Consensus`; `trust` **always empty** |
| `rank` | `Vec<Compact<u16>>` | **always empty** |
| `block_at_registration` | `Vec<Compact<u64>>` | `BlockAtRegistration` |
| `alpha_stake` | `Vec<Compact<AlphaBalance>>` | inherited alpha stake per UID (child/parent adjusted), `get_stake_weights_for_network` (`staking/stake_utils.rs:269-320`) |
| `tao_stake` | `Vec<Compact<TaoBalance>>` | inherited root-TAO stake per UID |
| `total_stake` | `Vec<Compact<TaoBalance>>` | `alpha + tao × tao_weight` — **typed TaoBalance but it is a stake-weight scalar in alpha units**, not TAO (`stake_utils.rs:312-316`) |
| `tao_dividends_per_hotkey` | `Vec<(AccountId32, Compact<TaoBalance>)>` | **all zero** ("Tao dividends were removed", `metagraph.rs:676-677`) |
| `alpha_dividends_per_hotkey` | `Vec<(AccountId32, Compact<AlphaBalance>)>` | `AlphaDividendsPerSubnet[netuid][hotkey]` (last tempo) |
| `collateral_locked`, `collateral_min`, `collateral_earned` | `Vec<Compact<AlphaBalance>>` | miner collateral per UID (`collateral_vectors`); present since spec ≥ 435 per `PY/reads/subnets.py:181-183` |

Runtime cost of `get_metagraph` (inferred from code): per UID 5 storage reads (`Keys`, `Owner`, `BlockAtRegistration`, `IdentitiesV2`, `Axons`) + 1 (`AlphaDividendsPerSubnet`) + the stake-weight pass (`get_inherited_for_hotkey_on_subnet`/`get_tao_inherited…` per UID, which walks `ChildKeys`/`ParentKeys`) — O(n × (parents+children)).

#### 1.1.2 `SelectiveMetagraph<AccountId32>` (`metagraph.rs:119-229`, codegen `META/finney_codegen.rs:57110`)

Same fields as `Metagraph`, each wrapped in `Option<…>` (`identity` becomes `Option<Option<SubnetIdentityV3>>`), **plus two extra fields** not in `Metagraph`:

- `validators: Option<Vec<Compact<u16>>>` — UIDs with validator permit whose stake weight exceeds `StakeThreshold`, sorted ascending by stake (`metagraph.rs:1558-1596`).
- `commitments: Option<Vec<(AccountId32, Vec<Compact<u8>>)>>` — Commitments-pallet payloads per hotkey (`metagraph.rs:1598-1614`).

`netuid` is always populated (`Compact<NetUid>`, default `NetUid::ROOT` in the empty struct, overwritten by the merge; `metagraph.rs:233-236,893-912`).

#### 1.1.3 Selective index scheme (`SelectiveMetagraphIndex::from_index`, `metagraph.rs:488-650`)

Indices are `u16`; unknown indices are ignored (`from_index → None`, test at `metagraph.rs:1659-1672`). Pass any subset in any order; each index is built independently then merged (`merge_value`, `metagraph.rs:233-385`).

```
 0 Netuid              1 Name                 2 Symbol               3 Identity
 4 NetworkRegisteredAt 5 OwnerHotkey          6 OwnerColdkey         7 Block
 8 Tempo               9 LastStep            10 BlocksSinceLastStep 11 SubnetEmission(=0)
12 AlphaIn            13 AlphaOut            14 TaoIn               15 AlphaOutEmission
16 AlphaInEmission    17 TaoInEmission       18 PendingAlphaEmission19 PendingRootEmission(=0)
20 SubnetVolume       21 MovingPrice         22 Rho                 23 Kappa
24 MinAllowedWeights  25 MaxWeightsLimit     26 WeightsVersion      27 WeightsRateLimit
28 ActivityCutoff     29 MaxValidators       30 NumUids             31 MaxUids
32 Burn               33 Difficulty          34 RegistrationAllowed 35 PowRegistrationAllowed
36 ImmunityPeriod     37 MinDifficulty       38 MaxDifficulty       39 MinBurn
40 MaxBurn            41 AdjustmentAlpha     42 AdjustmentInterval  43 TargetRegsPerInterval
44 MaxRegsPerBlock    45 ServingRateLimit    46 CommitRevealWeightsEnabled 47 CommitRevealPeriod
48 LiquidAlphaEnabled 49 AlphaHigh           50 AlphaLow            51 BondsMovingAvg
52 Hotkeys            53 Coldkeys            54 Identities          55 Axons
56 Active             57 ValidatorPermit     58 PruningScore(empty) 59 LastUpdate
60 Emission           61 Dividends           62 Incentives          63 Consensus
64 Trust(empty)       65 Rank(empty)         66 BlockAtRegistration 67 AlphaStake
68 TaoStake           69 TotalStake          70 TaoDividendsPerHotkey(zeros) 71 AlphaDividendsPerHotkey
72 Validators         73 Commitments         74 CollateralLocked    75 CollateralMin
76 CollateralEarned
```

Cost notes (verified in builder code): indices 52/53/54/55/66/70/71 each loop `0..n` re-reading `Keys` (`metagraph.rs:1417-1456` for 70/71); **67, 68, 69 each call `get_stake_weights_for_network` independently** (`metagraph.rs:1373-1403`) — requesting all three triples the most expensive computation; 72 iterates `Keys` prefix and calls `get_stake_weights_for_hotkey_on_subnet` per permitted UID (`metagraph.rs:1558-1596`). For mechagraphs only 62 (`Incentives`) and 59 (`LastUpdate`) are mechanism-specific (`metagraph.rs:1518-1556`).

#### 1.1.4 `DynamicInfo<AccountId32>` (`dynamic_info.rs:9-33,36-74`)

`netuid: Compact<NetUid>`, `owner_hotkey`, `owner_coldkey: AccountId32`, `subnet_name: Vec<Compact<u8>>`, `token_symbol: Vec<Compact<u8>>`, `tempo: Compact<u16>`, `last_step: Compact<u64>` (=`LastMechansimStepBlock`), `blocks_since_last_step: Compact<u64>` (computed), `emission: Compact<u64>` (**always 0**), `alpha_in`, `alpha_out: Compact<AlphaBalance>`, `tao_in: Compact<TaoBalance>`, `alpha_out_emission`, `alpha_in_emission`, `tao_in_emission`, `pending_alpha_emission` (= validator + server pending), `pending_root_emission` (**0**), `subnet_volume: Compact<u128>`, `network_registered_at: Compact<u64>`, `subnet_identity: Option<SubnetIdentityV3>`, `moving_price: I96F32`. This is the cheap "subnet header" read — no per-UID data — use it for price/pool dashboards instead of `get_metagraph`.

#### 1.1.5 `SubnetState<AccountId32>` (`show_subnet.rs:10-39,84-165`)

`netuid`, `hotkeys`, `coldkeys`, `active`, `validator_permit`, `pruning_score` (empty), `last_update`, `emission`, `dividends`, `incentives`, `consensus`, `trust` (empty), `rank` (empty), `block_at_registration`, `alpha_stake`, `tao_stake`, `total_stake` (same semantics as the metagraph columns), `emission_history: Vec<Vec<Compact<AlphaBalance>>>` = for every subnet S (outer) × every hotkey of *this* subnet (inner): `LastHotkeyEmissionOnNetuid[hotkey][S]` (`show_subnet.rs:55-67`). Cost is `|subnets| × n` reads; unless you want cross-subnet emission history, prefer `get_metagraph`/`get_selective_metagraph`.

#### 1.1.6 `SubnetInfo` / `SubnetInfov2` (`subnet_info.rs:9-54`)

Both: `netuid: Compact<NetUid>`, `rho`, `kappa: Compact<u16>`, `difficulty: Compact<u64>`, `immunity_period`, `max_allowed_validators`, `min_allowed_weights`, `max_weights_limit`, `scaling_law_power`, `subnetwork_n`, `max_allowed_uids: Compact<u16>`, `blocks_since_last_step: Compact<u64>` (**here it is the `BlocksSinceLastStep` storage value**, capped at tempo+1), `tempo: Compact<u16>`, `network_modality: Compact<u16>` (**0**), `network_connect: Vec<[u16;2]>` (**empty**), `emission_values`/`emission_value: Compact<u64>` (**0**), `burn: Compact<TaoBalance>`, `owner: AccountId32`; v2 appends `identity: Option<SubnetIdentityV3>`. Legacy; nothing here that `DynamicInfo` + `get_subnet_hyperparams_v3` do not provide better.

#### 1.1.7 `SubnetHyperparams` (v1), `SubnetHyperparamsV2`, `SubnetHyperparamsV3` (`subnet_info.rs:56-173`)

v1 (27 fields): `rho, kappa, immunity_period, min_allowed_weights, max_weights_limit, tempo: Compact<u16>`, `min_difficulty, max_difficulty, weights_version, weights_rate_limit: Compact<u64>`, `adjustment_interval: Compact<u16>`, `activity_cutoff: Compact<u64>` (effective blocks), `registration_allowed: bool`, `target_regs_per_interval: Compact<u16>`, `min_burn, max_burn: Compact<TaoBalance>`, `bonds_moving_avg: Compact<u64>`, `max_regs_per_block: Compact<u16>`, `serving_rate_limit: Compact<u64>`, `max_validators: Compact<u16>`, `adjustment_alpha, difficulty, commit_reveal_period: Compact<u64>`, `commit_reveal_weights_enabled: bool`, `alpha_high, alpha_low: Compact<u16>`, `liquid_alpha_enabled: bool`.

v2 = v1 + `alpha_sigmoid_steepness: I32F32` (`FixedI64<U32>`, `bits: i64`; storage holds a plain `i16`, so clients divide bits by 2^32 to recover the setter value — `PY/reads/subnets.py:126-147`), `yuma_version: Compact<u16>` (3 if `Yuma3On` else 2), `subnet_is_active: bool` (= `SubtokenEnabled`), `transfers_enabled: bool` (`TransferToggle`), `bonds_reset_enabled: bool`, `user_liquidity_enabled: bool` (**hard-coded false**, `subnet_info.rs:451`).

v3 = `Vec<HyperparamEntry { name: Vec<u8>, value: HyperparamValue }>` with `enum HyperparamValue { Bool(bool), U16(Compact<u16>), U32(Compact<u32>), U64(Compact<u64>), U128(Compact<u128>), TaoBalance(Compact<TaoBalance>), I32F32(FixedI64<U32>), U64F64(FixedU128<U64>) }` (variant indices 0..7 in declaration order; `subnet_info.rs:133-143`). Names emitted at `850f275` in order (`subnet_info.rs:480-648`): `epoch_consensus`(U16: 0=Yuma,1=Null), `kappa`, `immunity_period`, `min_allowed_weights`, `max_weights_limit`, `tempo`, `weights_version`, `weights_rate_limit`, `activity_cutoff`(U64 blocks), `activity_cutoff_factor`(U32 per-mille), `registration_allowed`, `target_regs_per_interval`, `min_burn`, `max_burn`(TaoBalance), `burn_half_life`(U16), `burn_increase_mult`(U64F64), `bonds_moving_avg`, `max_regs_per_block`, `serving_rate_limit`, `max_validators`, `commit_reveal_period`, `commit_reveal_weights_enabled`, `alpha_high`, `alpha_low`, `liquid_alpha_enabled`, `alpha_sigmoid_steepness`(I32F32), `yuma_version`, `subnet_is_active`, `transfers_enabled`, `bonds_reset_enabled`, `user_liquidity_enabled`(false), `owner_cut_enabled`, `owner_cut_auto_lock_enabled`, `min_childkey_take`(U16), `collateral_lock_share`(U16), `collateral_drain_ratio`(U64F64). **Note: v3 at this commit does not emit `rho`, `difficulty`, `min/max_difficulty`, `adjustment_interval`, `adjustment_alpha`** — read those from v2 or storage if needed. The on-chain 477 list may differ slightly (the shape is self-describing, so decode by name; `subnet_info.rs:145-149` says "treat unknown names as forward-compatible additions"). The wire shape of v3 never changes when params are added (`subnet_info.rs:166-172`).

### 1.2 `NeuronInfoRuntimeApi` (v1) — `META/explore/_all_api_methods.txt:1637-1873`

| Method | Input | Output |
|---|---|---|
| `get_neurons(netuid)` | `NetUid` | `Vec<NeuronInfo>` (stops at first missing uid; `neuron_info.rs:58-74`) |
| `get_neuron(netuid, uid: u16)` | `(NetUid, u16)` | `Option<NeuronInfo>` |
| `get_neurons_lite(netuid)` | `NetUid` | `Vec<NeuronInfoLite>` |
| `get_neuron_lite(netuid, uid)` | `(NetUid, u16)` | `Option<NeuronInfoLite>` |

`NeuronInfo<AccountId32>` (`neuron_info.rs:8-31`, built `76-146`): `hotkey`, `coldkey: AccountId32`; `uid: Compact<u16>`; `netuid: Compact<NetUid>`; `active: bool`; `axon_info: AxonInfo`; `prometheus_info: PrometheusInfo`; `stake: Vec<(AccountId32, Compact<AlphaBalance>)>` — **exactly one entry** `(owner coldkey, TotalHotkeyAlpha[hotkey][netuid])` (`neuron_info.rs:118-121`; `TotalHotkeyAlpha` is the hotkey's total alpha across all nominators, `stake_utils.rs:661-665`), not a per-coldkey breakdown; `rank: Compact<u16>` (**0**); `emission: Compact<AlphaBalance>` (last-epoch alpha-rao); `incentive`, `consensus`, `trust`(**0**), `validator_trust`, `dividends: Compact<PerU16>`; `last_update: Compact<u64>`; `validator_permit: bool`; `weights: Vec<(Compact<u16>, Compact<u16>)>` (uid, weight) nonzero only, from `Weights[storage index][uid]`; `bonds: Vec<(Compact<u16>, Compact<u16>)>` nonzero only; `pruning_score: Compact<u16>` (**65535**). `NeuronInfoLite` = same minus `weights`/`bonds` (`neuron_info.rs:33-55`).

### 1.3 `StakeInfoRuntimeApi` (v1) — `_all_api_methods.txt:1874-2139`

| Method | Input | Output |
|---|---|---|
| `get_stake_info_for_coldkey(coldkey)` | `AccountId32` | `Vec<StakeInfo>` |
| `get_stake_info_for_coldkeys(coldkeys)` | `Vec<AccountId32>` | `Vec<(AccountId32, Vec<StakeInfo>)>` |
| `get_stake_info_for_hotkey_coldkey_netuid(hotkey, coldkey, netuid)` | `(AccountId32, AccountId32, NetUid)` | `Option<StakeInfo>` (always `Some`, `stake_info.rs:118-144`) |
| `get_stake_availability_for_coldkeys(coldkeys, netuids: Option<Vec<NetUid>>)` | | `BTreeMap<AccountId32, BTreeMap<NetUid, StakeAvailability>>` (SCALE as sorted `Vec<(k,v)>`) |
| `get_stake_fee(origin: Option<(AccountId32,NetUid)>, origin_coldkey, destination: Option<(AccountId32,NetUid)>, destination_coldkey, amount: u64)` | | `u64` (rao fee; 0 if origin==destination; `stake_info.rs:209-222`) |
| `get_coldkey_lock(coldkey, netuid)` | `(AccountId32, NetUid)` | `Option<LockState>` |
| `get_hotkey_conviction(hotkey, netuid)` | `(AccountId32, NetUid)` | `U64F64` (`FixedU128<U64>`, `bits: u128`, value = bits/2^64) |
| `get_most_convicted_hotkey_on_subnet(netuid)` | `NetUid` | `Option<AccountId32>` (`subnet_king`) |

`StakeInfo<AccountId32>` (`stake_info.rs:11-23`): `hotkey`, `coldkey`, `netuid: Compact<NetUid>`, `stake: Compact<AlphaBalance>` (alpha-rao; TAO-rao when netuid 0), `locked: Compact<u64>` (**0**), `emission: Compact<AlphaBalance>` (= `AlphaDividendsPerSubnet[netuid][hotkey]`, hotkey-level, not per coldkey), `tao_emission: Compact<TaoBalance>` (**0**), `drain: Compact<u64>` (**0**), `is_registered: bool`. `get_stake_info_for_coldkey(s)` iterates `StakingHotkeys[coldkey] × all netuids` and skips zero positions (`stake_info.rs:49-90`). `StakeAvailability { total, locked, available: Compact<AlphaBalance> }` (`stake_info.rs:25-31`). `LockState { locked_mass: AlphaBalance, conviction: U64F64, last_update: u64 }` (`META/finney_codegen.rs:57609`).

Why rtao should read stake via this API and not storage: per-(hotkey,coldkey) stake is a **share-pool** value (`AlphaV2: NMap(hot,cold,netuid) → SafeFloat{mantissa:u128, exponent:i64}` and `TotalHotkeySharesV2`; `SUB/primitives/share-pool/src/lib.rs:27-30`) that must be converted through `get_alpha_share_pool(...).try_get_value(coldkey)` (`stake_utils.rs:638-648`). Hotkey totals, however, are a plain `TotalHotkeyAlpha` read (`stake_utils.rs:661-665`).

### 1.4 `DelegateInfoRuntimeApi` (v1) — `_all_api_methods.txt:2140-2251`

`get_delegates() -> Vec<DelegateInfo>`; `get_delegate(delegate: AccountId32) -> Option<DelegateInfo>`; `get_delegated(delegatee: AccountId32) -> Vec<(DelegateInfo, (Compact<NetUid>, Compact<AlphaBalance>))>`.

`DelegateInfo<AccountId32>` (`delegate_info.rs:12-23`): `delegate_ss58`, `take: Compact<PerU16>`, `nominators: Vec<(AccountId32, Vec<(Compact<NetUid>, Compact<u64>)>)>` (built by iterating the whole `AlphaV2` prefix of the hotkey and valuing through share pools — expensive; `delegate_info.rs:63-89`), `owner_ss58`, `registrations: Vec<Compact<NetUid>>`, `validator_permits: Vec<Compact<NetUid>>`, `return_per_1000: Compact<u64>`, `total_daily_return: Compact<u64>` (`emission × 7200/tempo` summed over registrations — rao/day). `get_delegates` iterates every `Delegates` key — avoid on hot paths.

### 1.5 `SwapRuntimeApi` (v1) — `_all_api_methods.txt:2252-2327`; impl `SUB/runtime/src/lib.rs:2715-2793`

- `current_alpha_price(netuid) -> u64` = `current_price × 1e9` (rao of TAO per alpha, i.e. price scaled by 1e9).
- `current_alpha_price_all() -> Vec<SubnetPrice { netuid: NetUid, price: u64 }>`.
- `sim_swap_tao_for_alpha(netuid, tao: TaoBalance) -> SimSwapResult`; `sim_swap_alpha_for_tao(netuid, alpha: AlphaBalance) -> SimSwapResult` with `SimSwapResult { tao_amount, alpha_amount, tao_fee, alpha_fee, tao_slippage, alpha_slippage }` (non-compact `TaoBalance(u64)`/`AlphaBalance(u64)`); on failure all zeros. Slippage is "no-slippage amount at spot − actual" (`lib.rs:2735-2745`).

### 1.6 `SubnetRegistrationRuntimeApi` (v1): `get_network_registration_cost() -> TaoBalance` (= `get_network_lock_cost`; `runtime/src/lib.rs:2586-2590`). Custom RPC alias `subnetInfo_getLockCost` (`rpc/src/lib.rs:89-90,534-538`).

### 1.7 `ProxyFilterRuntimeApi` (v1) — `_all_api_methods.txt:2338-2406`

`get_proxy_types() -> Vec<ProxyTypeInfo { name: Vec<u8>, index: u8, deprecated: bool }>`; `get_proxy_filters(proxy_types: Option<Vec<u8>>) -> Vec<ProxyFilterInfo { proxy_type: u8, name, deprecated, filter_mode: FilterMode::{AllowAll, Allow(Vec<CallInfo{pallet_name, pallet_index: u8, call_name, call_index: u8, constraint: Option<CallConstraint::{ParamLessThan{param_name, limit: u128}, NestedCallMustBe{param_name, pallet_name, call_name}}>}>)} }>`. Useful for rtao's proxy-aware signing (which calls a proxy type may dispatch).

### 1.8 `ShieldApi` (v1) — `_all_api_methods.txt:2407-2608`

`is_shielded_using_current_key(key_hash: [u8;16]) -> bool`; `try_decode_shielded_tx(uxt: UncheckedExtrinsic) -> Option<ShieldedTransaction { key_hash: [u8;16], kem_ct: Vec<u8>, aead_ct: Vec<u8>, nonce: [u8;24] }>`; `try_unshield_tx(dec_key_bytes: Vec<u8>, shielded_tx) -> Option<UncheckedExtrinsic>`. The extrinsic type dump confirms the signed-extension set: `((CheckNonZeroSender, CheckSpecVersion, CheckTxVersion, CheckGenesis, CheckMortality, CheckNonce, CheckWeight), (ChargeTransactionPaymentWrapper, SudoTransactionExtension, CheckShieldedTxValidity, SubtensorTransactionExtension, DrandPriority), CheckMetadataHash)` (`_all_api_methods.txt:2440-2470`). Related storage: `MevShield.NextKey: BoundedVec<u8>` (ML-KEM-768 public key; `PY/reads/chain.py:62-68`).

### 1.9 Standard APIs

- `AccountNonceApi::account_nonce(account: AccountId32) -> u32` (`runtime/src/lib.rs:1972-1976`).
- `TransactionPaymentApi::query_info(uxt, len: u32) -> RuntimeDispatchInfo { weight: Weight{ref_time: Compact<u64>, proof_size: Compact<u64>}, class: DispatchClass, partial_fee: TaoBalance }`; `query_fee_details(uxt, len) -> FeeDetails { inclusion_fee: Option<InclusionFee{base_fee, len_fee, adjusted_weight_fee}>, tip }`; `query_weight_to_fee(Weight) -> TaoBalance`; `query_length_to_fee(u32) -> TaoBalance`. **Subtensor overrides fee computation** via `staking_fee::query_info/query_fee_details` (`runtime/src/lib.rs:1978-2001`, `runtime/src/staking_fee.rs`), so `partial_fee` for staking calls includes subtensor-specific logic — always ask the chain rather than computing locally. `TransactionPaymentCallApi::query_call_info(call, len)` / `query_call_fee_details` take a bare `RuntimeCall` (no signature needed; `lib.rs:2003-2026`). The Python SDK estimates fees by signing with a zero signature and calling `TransactionPaymentApi_query_info` (`PY/_transport/interface.py:714-731`); rtao can use `TransactionPaymentCallApi_query_call_info` to avoid the fake signature.
- `Metadata::metadata_versions() -> Vec<u32>`, `metadata_at_version(u32) -> Option<OpaqueMetadata>`; finney served V16 (`META/finney.json` top-level key `V16`).

### 1.10 `BetaBasketRuntimeApi` (v5) — `_all_api_methods.txt:7248-8134`, decl `runtime-api/src/lib.rs:91-159`

18 methods (`get_root_basket_owed`, `get_basket_payout`, `get_validator_basket_nav`, `get_validator_basket`, `get_root_basket_total_nav`, `get_validator_basket_summary`, `get_all_validator_baskets`, `get_root_basket_positions`, `get_basket_position`(v2), `get_root_basket_portfolio`(v2), `get_beta_pricing`(v3), `get_all_beta_pricing(start_after: Option<AccountId32>, limit: u32)`(v3, paginated), `get_beta_index`(v3) `-> (U64F64, U64F64)`, `get_beta_position`(v3), `get_beta_portfolio`(v3), `get_basket_trading_status`(v4), `get_basket_claim_preview`(v5), `get_root_basket_claim_previews`(v5)). Staker/explorer-facing; not needed for subnet validator/miner loops; the struct shapes (`BasketSummary`, `BasketPosition`, `BetaPricing`, …) are in `SUB/pallets/subtensor/src/rpc_info/basket_info.rs` and `META/finney_codegen.rs:56694+`. Note `#[api_version(N)]` per method means a client must check the advertised API version before calling v2+ methods on older nodes.

### 1.11 Shared leaf types

- `NetUid(u16)` (`SUB/common/src/lib.rs:77`, `ROOT = 0`), `NetUidStorageIndex(u16)` (`lib.rs:291`, `CompactAs<u16>`), `MechId(u8)` (`lib.rs:216`, `MAIN = 0`), `AlphaBalance(u64)`, `TaoBalance(u64)` (`SUB/common/src/currency.rs:51,73`), `PerU16(u16)`. All are `CompactAs` newtypes; subxt codegen emits `#[codec(compact)] pub netuid: NetUid` (`META/finney_codegen.rs:56884-56885`) and derives only `DecodeAsType/EncodeAsType` (not parity-codec `Encode/Decode`) by default (`finney_codegen.rs:60396-60417`) — rtao should pass `--derive Encode,Decode,Clone,PartialEq` (or use its own hand-written types) if it wants raw SCALE.
- `AxonInfo { block: u64, version: u32, ip: u128, port: u16, ip_type: u8 (4|6), protocol: u8, placeholder1: u8, placeholder2: u8 }` — fixed 34 bytes (`SUB/pallets/subtensor/src/lib.rs:305-324`). `PrometheusInfo { block: u64, version: u32, ip: u128, port: u16, ip_type: u8 }` — 31 bytes (`lib.rs:361-372`). `NeuronCertificate { public_key: BoundedVec<u8, 64>, algorithm: u8 }` (`lib.rs:332-337`) — TLS cert pubkey set by `serve_axon_tls(…, certificate: Vec<u8>)`.
- `ChainIdentityV2 { name, url, github_repo, image, discord, description, additional: Vec<u8> }` (`lib.rs:403-419`; storage `IdentitiesV2: coldkey → ChainIdentityV2`). `SubnetIdentityV3 { subnet_name, github_repo, subnet_contact, subnet_url, discord, description, logo_url, additional: Vec<u8> }` (`lib.rs:463-481`; storage `SubnetIdentitiesV3: netuid → …`).
- Fixed-point encodings on the wire: `I96F32` = `FixedI128<U32>` struct `{bits: i128}` (16 bytes); `I32F32` = `FixedI64<U32>` `{bits: i64}`; `U64F64` = `FixedU128<U64>` `{bits: u128}`; `I64F64` appears in `SubnetEmaTaoFlow: (u64, FixedI128<U64>)`. Decode as `bits / 2^frac`.

---

## 2. Storage entries that matter to a neuron / subnet owner

Source of truth for key/hasher/value/default: `META/explore/_storage_table.tsv` (generated from `META/finney.json`, spec 477) — 277 entries in `SubtensorModule`. Pallet index **7**; other relevant pallets: `System` 0, `Timestamp` 2, `Aura` 3, `Balances` 5, `TransactionPayment` 6, `Commitments` 18, `AdminUtils` 19, `Drand` 26, `Swap` 28, `MevShield` 30 (`META/finney.json` pallet indices).

Storage key construction: `twox128("SubtensorModule") ++ twox128(entry) ++ hasher(k1) [++ hasher(k2) …]`. **Hashers**: most `NetUid`-keyed maps are `Identity` (raw `u16` LE), AccountId keys are `Blake2_128Concat`, and several newer maps use `Twox64Concat` (noted). `Default` modifier = the default bytes below are returned when the key is absent; `Optional` = `None` when absent. Line numbers are in `SUB/pallets/subtensor/src/lib.rs` unless stated.

### 2.1 Subnet identity, ownership, existence

| Entry (lib.rs line) | Key → Value | Hashers | Default |
|---|---|---|---|
| `NetworksAdded` (2385) | `NetUid → bool` | Identity | false |
| `TotalNetworks` (2243) | plain `u16` | – | 0 |
| `SubnetLimit` | plain `u16` | – | 128 |
| `NetworkRegisteredAt` (2419) | `NetUid → u64` | Identity | 0 |
| `SubnetOwner` (2492) / `SubnetOwnerHotkey` (2497) | `NetUid → AccountId32` | Identity | zero account |
| `SubnetIdentitiesV3` (2986) | `NetUid → SubnetIdentityV3` | Blake2_128Concat | Optional |
| `IdentitiesV2` (2981) | `AccountId32(coldkey) → ChainIdentityV2` | Blake2_128Concat | Optional |
| `TokenSymbol` (2118) | `NetUid → Vec<u8>` | Identity | `"τ"`-like default bytes `[16,240,157,156,143]` (compact-len 4 + U+1D71B… i.e. the default symbol glyph) |
| `SubtokenEnabled` (2732) | `NetUid → bool` ("subnet_is_active") | Identity | false |
| `FirstEmissionBlockNumber` (2371) | `NetUid → u64` | Identity | Optional (set by `start_call`) |
| `SubnetEmissionEnabled` | `NetUid → bool` | Identity | true |
| `SubnetMechanism` (2376) | `NetUid → u16` (legacy "mechanism type", **not** mechanism count) | Identity | 0 |
| `MechanismCountCurrent` (3581) | `NetUid → MechId(u8)` | **Twox64Concat** | 1 |
| `MaxMechanismCount` | plain `MechId` | – | 2 |
| `MechanismEmissionSplit` (3586) | `NetUid → Vec<u16>` (u16-normalised split per mechanism) | Twox64Concat | Optional |
| `SubnetEpochConsensus` (2728) | `NetUid → EpochConsensus{Yuma=0, Null=1}` | Identity | Yuma |
| `NetworkRegistrationAllowed` (2403) / `NetworkPowRegistrationAllowed` | `NetUid → bool` | Identity | true / false |

### 2.2 Neuron registry

| Entry | Key → Value | Hashers | Default |
|---|---|---|---|
| `SubnetworkN` (2381) | `NetUid → u16` | Identity | 0 |
| `MaxAllowedUids` (2545) | `NetUid → u16` | Identity | 256 |
| `MinAllowedUids` | `NetUid → u16` | Identity | 64 |
| `Keys` (2844) | `(NetUid, uid u16) → AccountId32` | Identity, Identity | zero account (**Default**, so a missing uid decodes as all-zero hotkey — bound iteration by `SubnetworkN`) |
| `Uids` (2839) | `(NetUid, hotkey) → u16` | Identity, Blake2_128Concat | **Optional** (`None` = not registered) |
| `IsNetworkMember` (2390) | `(hotkey, NetUid) → bool` | Blake2_128Concat, Identity | false (used by `netuids_for_hotkey` via prefix query_map on hotkey) |
| `Owner` (1611) | `hotkey → coldkey` | Blake2_128Concat | zero account |
| `OwnedHotkeys` (1881) | `coldkey → Vec<hotkey>` | Blake2_128Concat | `[]` |
| `BlockAtRegistration` (2932) | `(NetUid, uid) → u64` | Identity, Identity | 0 |
| `Axons` (2945) | `(NetUid, hotkey) → AxonInfo` | Identity, Blake2_128Concat | Optional |
| `Prometheus` (2969) | `(NetUid, hotkey) → PrometheusInfo` | Identity, Blake2_128Concat | Optional |
| `NeuronCertificates` (2957) | `(NetUid, hotkey) → NeuronCertificate` | Identity, Blake2_128Concat | Optional |
| `AssociatedEvmAddress` | `(NetUid, uid) → (H160, u64 block)` | Twox64Concat ×2 | Optional |
| `ImmunityPeriod` (2550) | `NetUid → u16` | Identity | 4096 |
| `Burn` (2636) / `MinBurn` / `MaxBurn` | `NetUid → TaoBalance` | Identity | 0.1 TAO (100 000 000 rao) / 500 000 rao / 100 TAO |
| `Difficulty` (2640) / `MinDifficulty` / `MaxDifficulty` | `NetUid → u64` | Identity | 10 000 000 / 10 000 000 / 2^62−1 |
| `RegistrationsThisInterval` (2525), `RegistrationsThisBlock`, `BurnRegistrationsThisInterval`, `POWRegistrationsThisInterval` | `NetUid → u16` | Identity | 0 |
| `TargetRegistrationsPerInterval` / `AdjustmentInterval` / `MaxRegistrationsPerBlock` | `NetUid → u16` | Identity | 2 / 100 / 1 |
| `AdjustmentAlpha` | `NetUid → u64` | Identity | 0 |
| `BurnHalfLife` / `BurnIncreaseMult` | `NetUid → u16` / `U64F64` | Identity | 360 / 1.5 (bits `0x1_8000_0000_0000_0000`… see tsv) |
| `MinerCollateral` (3625) | `(NetUid, hotkey, coldkey) → MinerCollateralState{locked: AlphaBalance, drain_ratio: U64F64, min_locked: AlphaBalance, earned: …}` (`lib.rs:550-566`) | Identity, Blake2 ×2 | Optional |
| `CollateralLockShare` / `CollateralDrainRatio` | `NetUid → u16` / `U64F64` | Identity | 0 / 1.0 |

### 2.3 Weights, bonds, commit-reveal (mechanism-indexed)

| Entry | Key → Value | Hashers | Default |
|---|---|---|---|
| `Weights` (2906) | `(NetUidStorageIndex, uid) → Vec<(u16 uid, u16 weight)>` | Identity, Identity | `[]` |
| `Bonds` (2919) | `(NetUidStorageIndex, uid) → Vec<(u16, u16)>` | Identity, Identity | `[]` |
| `LastUpdate` (2891) | `NetUidStorageIndex → Vec<u64>` (per uid, block of last set/commit) | Identity | `[]` |
| `WeightsSetRateLimit` (2606) | `NetUid → u64` blocks | Identity | 100 |
| `WeightsVersionKey` (2571) | `NetUid → u64` | Identity | 0 |
| `MinAllowedWeights` (2576) / `MaxWeightsLimit` (2566) | `NetUid → u16` | Identity | 1024 / 65535 |
| `CommitRevealWeightsEnabled` (2631) | `NetUid → bool` | Identity | **true** |
| `CommitRevealWeightsVersion` (3531) | plain `u16` | – | **4** |
| `RevealPeriodEpochs` (3094) | `NetUid → u64` epochs | **Twox64Concat** | 1 (bounds 1..=100, `lib.rs:287-289`) |
| `WeightCommits` (3029) | `(NetUidStorageIndex, hotkey) → Vec<(H256 hash, u64 commit_epoch, u64 commit_block, u64 unused)>` | Twox64Concat ×2 | Optional (CRv2 hash commits; max 10 unrevealed, `weights.rs:211`) |
| `TimelockedWeightCommits` (3042) | `(NetUidStorageIndex, u64 epoch) → Vec<(hotkey, u64 commit_block, BoundedVec<u8, 32 KiB> ciphertext, u64 reveal_round)>` | Twox64Concat ×2 | `[]` (CRv3/4 live queue; Yuma commits ≤ 5000 B, `lib.rs:63-64`, `weights.rs:450-453`) |
| `CRV3WeightCommits` (3060) / `CRV3WeightCommitsV2` (3077) | same key shape; **deprecated**, only cleared by dissolution (`subnets/dissolution.rs:240-242`) | Twox64Concat ×2 | `[]` |
| `StakeThreshold` (3024) | plain `u64` ("weights_min_stake") | – | 0 on chain default (docs say ~1000 tokens' worth live: `SUB/docs/concepts/emissions.mdx` "Stake weight") |

### 2.4 Epoch outputs (Yuma) — written at `SUB/pallets/subtensor/src/epoch/run_epoch.rs:565-625`

| Entry | Key → Value | Hashers | Written by |
|---|---|---|---|
| `Active` (2862) | `NetUid → Vec<bool>` | Identity | epoch |
| `ValidatorPermit` (2901) | `NetUid → Vec<bool>` | Identity | epoch (top-K by stake weight, K=`MaxAllowedValidators`) |
| `ValidatorTrust` (2896) | `NetUid → Vec<PerU16>` | Identity | epoch |
| `Consensus` (2867) | `NetUid → Vec<PerU16>` | Identity | epoch |
| `ConsensusByMechanism` | `NetUidStorageIndex → Vec<PerU16>` | Identity | epoch |
| `Incentive` (2877) | `NetUidStorageIndex → Vec<PerU16>` | Identity | epoch (**mechanism-indexed**) |
| `Dividends` (2882) | `NetUid → Vec<PerU16>` | Identity | epoch |
| `Emission` (2887) | `NetUid → Vec<AlphaBalance>` (per-tempo payout per uid) | Identity | epoch |
| `StakeWeight` | `NetUid → Vec<u16>` | Identity | epoch |
| `AlphaDividendsPerSubnet` (1691) | `(NetUid, hotkey) → AlphaBalance` | Identity, Blake2 | coinbase distribution (`run_coinbase.rs:862`) |
| `TotalHotkeyAlphaLastEpoch` | `(hotkey, NetUid) → AlphaBalance` | Blake2, Identity | coinbase |
| `LastHotkeyEmissionOnNetuid` | `(hotkey, NetUid) → AlphaBalance` | Blake2, Identity | coinbase |

### 2.5 Epoch scheduling and pending emission

| Entry | Key → Value | Default | Semantics |
|---|---|---|---|
| `Tempo` (2322) | `NetUid → u16` | 360 | owner-settable in `[MinTempo=360, MaxTempo=50400]` (constants); root any |
| `LastEpochBlock` (2347) | `NetUid → u64` | 0 | "last epoch attempt block (consumed slot)"; set every time a slot fires, even if skipped (`run_coinbase.rs:481`) |
| `PendingEpochAt` (2353) | `NetUid → u64` | 0 | block of an owner-triggered epoch; 0 = none |
| `SubnetEpochIndex` (2359) | `NetUid → u64` | 0 | monotonic counter, +1 per consumed slot (`run_coinbase.rs:483`) — **the epoch id used by commit-reveal** |
| `BlocksSinceLastStep` (2470) | `NetUid → u64` | 0 | incremented per block, **capped at `tempo+1`** (`run_coinbase.rs:412-416`), reset to 0 on a successful run |
| `LastMechansimStepBlock` (2475) | `NetUid → u64` | 0 | block of last *successful* distribution (`run_coinbase.rs:508`) |
| `LastYumaStepBlock` | `NetUid → u64` | Optional | last epoch that updated bonds (frozen under Null) |
| `MaxEpochsPerBlock` (2690) | plain `u8` | 2 | per-block epoch cap |
| `AdminFreezeWindow` (1476) | plain `u16` | 10 | blocks before an epoch during which owner admin ops are rejected (`utils/misc.rs:59-81`) |
| `OwnerHyperparamRateLimit` (1481) | plain `u16` | 2 | epochs between owner hyperparam updates |
| `PendingServerEmission` (2436) / `PendingValidatorEmission` (2441) / `PendingRootAlphaDivs` / `PendingOwnerCut` | `NetUid → AlphaBalance` | 0 | accrued alpha drained at epoch |
| `ActivityCutoff` (2556) | `NetUid → u16` | 5000 | **legacy raw value**; effective cutoff uses `ActivityCutoffFactorMilli` (2365, `u32`, default 13889 ≈ 13.889 × tempo/1000 → 5000 blocks at tempo 360) |
| `Kappa` (2521) / `Rho` (2512) | `NetUid → u16` | 32767 / 10 | |
| `BondsMovingAverage` (2591) / `BondsPenalty` | `NetUid → u64` / `u16` | 900000 / 65535 | |
| `AlphaValues` (2718) | `NetUid → (u16 low, u16 high)` | (45875, 58982) | liquid alpha |
| `LiquidAlphaOn` (2708) / `Yuma3On` (2713) | `NetUid → bool` | false / false | **Blake2_128Concat** hashers (unlike most NetUid maps) |
| `MaxAllowedValidators` (2581) | `NetUid → u16` | 128 | |
| `ServingRateLimit` (2507) | `NetUid → u64` | 50 | blocks between `serve_axon` per hotkey |

### 2.6 Pool / economics

| Entry | Key → Value | Default |
|---|---|---|
| `SubnetTAO` (1820) | `NetUid → TaoBalance` (pool TAO reserve) | 0 |
| `SubnetAlphaIn` (1862) / `SubnetAlphaOut` (1867) | `NetUid → AlphaBalance` | 0 |
| `SubnetAlphaInEmission` / `SubnetAlphaOutEmission` / `SubnetTaoInEmission` | per-block injection | 0 |
| `SubnetMovingPrice` (1793) | `NetUid → I96F32` | 0 |
| `SubnetVolume` | `NetUid → u128` | 0 |
| `TotalHotkeyAlpha` (1932) | `(hotkey, NetUid) → AlphaBalance` (Blake2, Identity) | 0 |
| `TotalAlphaStaked` (1945) | `NetUid → AlphaBalance` | 0 |
| `AlphaV2` (1975) / `TotalHotkeySharesV2` (1963) | share-pool `SafeFloat` (do not interpret directly) | |
| `StakingHotkeys` (1876) | `coldkey → Vec<hotkey>` | `[]` |
| `TaoWeight` (1582) | plain `u64` (U64F64 fraction bits) | `InitialTaoWeight = 971718665099567868` ≈ 0.0527 (docs say the live governance-set value is 0.18: `SUB/docs/concepts/network.mdx` "stake weight" para — **storage default ≠ live value**) |
| `BlockEmission` (1751) | plain `u64` | 1 TAO (1 000 000 000) |
| `TotalIssuance` (1781) / `TotalStake` | plain `TaoBalance` | 0 |
| `SubnetOwnerCut` | plain `u16` | 11796 (18%) |
| `Delegates` (1621) | `hotkey → PerU16 take` | 11796 |
| `ChildKeys` (1665) | `(parent hotkey, NetUid) → Vec<(u64 proportion, child)>` (Blake2, Identity) | `[]` |
| `ParentKeys` (1678) | `(child hotkey, NetUid) → Vec<(u64 proportion, parent)>` | `[]` |
| `PendingChildKeys` | `(NetUid, parent) → (Vec<(u64, child)>, u64 cooldown_block)` | |
| `TxRateLimit` | plain `u64` | 1000 |
| `LastRateLimitedBlock` | `RateLimitKey<AccountId32> → u64` (Identity) | 0 |

### 2.7 Non-subtensor pallets a neuron reads

- `System.Account: AccountId32 → AccountInfo { nonce: u32, consumers, providers, sufficients: u32, data: AccountData { free, reserved, frozen: TaoBalance, flags: ExtraFlags(u128) } }`; `System.Number: u32`; `System.Events: Vec<EventRecord { phase: Phase, event: RuntimeEvent, topics: Vec<H256> }>`; `Timestamp.Now: u64` ms (subxt explore dumps above).
- `Commitments.CommitmentOf: (NetUid, hotkey) → Registration { deposit: TaoBalance, block: u32, info: CommitmentInfo { fields: BoundedVec<Data> } }` with `enum Data { None, Raw0..Raw128([u8;N]), BlakeTwo256([u8;32]), Sha256, Keccak256, ShaThree256, TimelockEncrypted { encrypted: BoundedVec<u8>, reveal_round: u64 }, ResetBondsFlag, BigRaw(BoundedVec<u8>), TimelockRevealFailed { encrypted, reveal_round } }`; `Commitments.RevealedCommitments: (NetUid, hotkey) → Vec<(Vec<u8> plaintext, u64 reveal_block)>` (subxt explore of `META/finney.scale`). Python's typed metagraph joins these two maps to the metagraph at one block (`PY/metagraph.py:238-288`).
- `Drand.Pulses: round → Pulse` (used by the chain to reveal timelocked commits; `coinbase/reveal_commits.rs:74`).

### 2.8 Constants worth caching (`META/explore/_all_constants.txt`)

`InitialTempo 360`, `MinTempo 360`, `MaxTempo 50400`, `InitialImmunityPeriod 4096`, `InitialMaxAllowedUids 256`, `InitialMaxAllowedValidators 128`, `InitialMinAllowedWeights 1024`, `InitialServingRateLimit 50`, `InitialWeightsVersionKey 0`, `InitialActivityCutoff 5000`, `InitialKappa 32767`, `InitialRho 10`, `InitialBondsMovingAverage 900000`, `InitialDefaultDelegateTake 11796`, `InitialSubnetOwnerCut 11796`, `InitialBurn 100000000`, `InitialMinBurn 500000`, `InitialMaxBurn 100000000000`, `InitialDifficulty 10000000`, `InitialMinStake 2000000`, `InitialMinTransfer 100000`, `InitialNetworkMinLockCost 1000000000000`, `InitialNetworkRateLimit 7200`, `InitialTxRateLimit 1000`, `InitialTxDelegateTakeRateLimit 216000`, `InitialTaoWeight 971718665099567868`, `InitialEmaPriceHalvingPeriod 201600`, `InitialMaxEpochsPerBlock 2`, `AlphaHigh 58982`, `AlphaLow 45875`, `KeySwapCost 100000000`, `HotkeySwapOnSubnetInterval 7200`, `MaxImmuneUidsPercentage 80`, `SubtensorPalletId "subtensr"`, `BurnAccountId "burntnsr"`, `LiquidAlphaOn false`, `Yuma3On false`. Also `Aura.SlotDuration 12000`, `Timestamp.MinimumPeriod 6000`, `Balances.ExistentialDeposit` (docs: 500 rao, `SUB/docs/concepts/money.mdx` "Minimums").

---

## 3. Events a neuron / owner cares about

All in `SubtensorModule` (pallet index 7). Variant indices from `META/finney.json`; shapes from `META/explore/_all_events.txt`; definitions `SUB/pallets/subtensor/src/macros/events.rs`.

| idx | Event | Fields (positional unless named) | Notes |
|---|---|---|---|
| 0 | `NetworkAdded` | `(NetUid, u16 modality)` | `events.rs:13` |
| 2 | `StakeAdded` | `(coldkey, hotkey, TaoBalance tao_in, AlphaBalance alpha_out, NetUid, u64 fee_rao)` | field order from emission site `staking/stake_utils.rs:1267-1274` |
| 3 | `StakeRemoved` | `(coldkey, hotkey, TaoBalance tao_out, AlphaBalance alpha_in, NetUid, u64 fee_rao)` | `stake_utils.rs:1154-1161` |
| 4 | `StakeMoved` | `(coldkey, origin_hotkey, origin_netuid, dest_hotkey, dest_netuid, TaoBalance)` | |
| 5 | `WeightsSet` | `(NetUidStorageIndex, u16 uid)` | **mechanism storage index**, not netuid |
| 7 | `NeuronRegistered` | `(NetUid, u16 uid, AccountId32 hotkey)` | |
| 29 | `AxonServed` | `(NetUid, AccountId32 hotkey)` | also `PrometheusServed` |
| 53 | `TempoSet` | `(NetUid, u16)` | |
| 85 | `CRV3WeightsCommitted` | `(hotkey, NetUidStorageIndex, H256 commit_hash)` | legacy CRv3 path |
| 86 | `WeightsCommitted` | `(hotkey, NetUidStorageIndex, H256)` | CRv2 hash commit |
| 87 | `WeightsRevealed` | `(hotkey, NetUidStorageIndex, H256)` | |
| 100 | `CRV3WeightsRevealed` | `(NetUid, hotkey)` | |
| 101 | `CommitRevealPeriodsSet` | `(NetUid, u64)` | |
| 102 | `CommitRevealEnabled` | `(NetUid, bool)` | |
| 108 | `TimelockedWeightsCommitted` | `(hotkey, NetUidStorageIndex, H256 blake2(ciphertext), u64 reveal_round)` | emitted by `commit_timelocked_weights` (`weights.rs:441-446,476-481`) |
| 109 | `TimelockedWeightsRevealed` | `(NetUidStorageIndex, hotkey)` | emitted in `on_initialize` (Initialization phase) |
| 111 | `IncentiveAlphaEmittedToMiners` | `{ netuid: NetUidStorageIndex, emissions: Vec<AlphaBalance> }` (index = uid) | per-mechanism epoch payout |
| 136 | `EpochTriggered` | `{ netuid, by: AccountId32, fires_at: u64 }` | owner trigger |
| 137 | `EpochDeferred` | `{ netuid, from_block, to_block }` | per-block cap |
| 138 | `EpochSkipped` | `{ netuid, block }` | inconsistent input state; slot consumed |
| 144 | `CollateralLocked` | `{ netuid, hotkey, locked, total_locked: AlphaBalance }` | |
| – | `WeightsSetRateLimitSet (NetUid,u64)` 23, `ImmunityPeriodSet (NetUid,u16)`, `ServingRateLimitSet (NetUid,u64)`, `BurnSet (NetUid,TaoBalance)`, `DifficultySet (NetUid,u64)`, `RegistrationAllowed (NetUid,bool)`, `MinAllowedWeightSet`, `MaxAllowedUidsSet`, `MaxAllowedValidatorsSet`, `KappaSet`, `RhoSet`, `BondsMovingAverageSet`, `WeightsVersionKeySet (NetUid,u64)`, `ActivityCutoffFactorMilliSet {netuid, factor_milli}`, `EpochConsensusSet (NetUid, EpochConsensus)`, `LiquidAlphaConsensusModeSet`, `SubnetOwnerHotkeySet (NetUid, AccountId32)`, `SubnetIdentitySet (NetUid)`, `ChainIdentitySet (coldkey)`, `FirstEmissionBlockNumberSet (NetUid,u64)`, `HotkeySwapped {coldkey, old_hotkey, new_hotkey}`, `HotkeySwappedOnSubnet {…, netuid}`, `SetChildren (parent, NetUid, Vec<(u64, child)>)`, `AlphaRecycled/AlphaBurned (coldkey, hotkey, AlphaBalance, NetUid)`, `TransactionFeePaidWithAlpha {who, netuid, alpha_fee, tao_amount}`, `SubnetOwnerChanged {netuid, old_coldkey, new_coldkey}`, `NullUidsPruningProgress {netuid, target, remaining, removed, cleared_commits}` | hyperparam/owner events | all shapes in `META/explore/_all_events.txt` |

`AdminUtils` (pallet 19) also emits: `SubnetEmissionEnabledSet`, `BurnHalfLifeSet`, `BurnIncreaseMultSet`, `CollateralDrainRatioSet`, `CollateralLockShareSet`, `BondsResetToggled`, `PrecompileUpdated`, basket-related (`META/explore/AdminUtils_events.txt`); most owner hyperparam calls are `AdminUtils::sudo_set_*` (`META/explore/AdminUtils_calls.txt`, e.g. `sudo_set_tempo`, `sudo_set_weights_set_rate_limit`, `sudo_set_commit_reveal_weights_enabled`, `sudo_set_commit_reveal_weights_interval`, `sudo_set_max_allowed_uids`, `sudo_set_activity_cutoff_factor`, `sudo_set_epoch_consensus`, `sudo_set_mechanism_count`, `sudo_set_mechanism_emission_split`) which dispatch into `pallet_subtensor` setters that emit the `SubtensorModule` events above.

Retired-but-present variants (never emitted): `RootWeightsSet(u16)` (`events.rs:45-47`), `MaxWeightLimitSet` ("deprecated: this event is unused"), `BulkBalancesSet` ("placeholder"). Keep them in the enum for index stability.

Decoding note: `WeightsSet`, `WeightsCommitted`, `WeightsRevealed`, `TimelockedWeightsCommitted/Revealed`, `IncentiveAlphaEmittedToMiners`, `CRV3WeightsCommitted` carry `NetUidStorageIndex` — filter by `idx % 4096 == netuid` and derive `mecid = idx / 4096`.

---

## 4. Epoch / tempo mechanics as seen by a client

### 4.1 Block ordering (`on_initialize` → `block_step`, `SUB/pallets/subtensor/src/coinbase/block_step.rs:7-42`)

1. `update_registration_prices_for_networks()` (burn/difficulty adjustment)
2. `get_block_emission()`
3. **`reveal_crv3_commits()`** — for every non-root subnet not deferred this block: drop queues older than `reveal_epoch`, then decrypt and apply `TimelockedWeightCommits[idx][cur_epoch − reveal_period]` using drand pulses; entries whose pulse is missing are re-queued and retried every block of the reveal epoch (`coinbase/reveal_commits.rs:39-89`).
4. **`run_coinbase(block_emission)`** — `drain_pending` decides which epochs fire (`run_coinbase.rs:394-486`): bump `BlocksSinceLastStep` (cap tempo+1); if `should_run_epoch` and under the cap and input state consistent → drain pending emissions, run epoch, `BlocksSinceLastStep=0`; if over the cap → `PendingEpochAt = block+1` + `EpochDeferred`; if inconsistent → `EpochSkipped`; in all fired cases `LastEpochBlock = block`, `PendingEpochAt = 0`, `SubnetEpochIndex += 1`. Then `distribute_emissions_to_subnets` sets `LastMechansimStepBlock = block` (`:508`) and, for Yuma, `LastYumaStepBlock`.
5. `flush_pending_basket_deposits_block`, `update_moving_prices`, `update_root_prop`, `try_set_pending_children` (children apply on the block an epoch fired, `block_step.rs:44-53`), root coldkey map population.

`should_run_epoch(netuid, block)` (`run_coinbase.rs:1217-1238`): `tempo == 0 → false`; `PendingEpochAt > 0 && block >= PendingEpochAt → true`; `BlocksSinceLastStep > tempo → true` (safety net); else `block − LastEpochBlock >= tempo`.

### 4.2 Client-facing predictions

- **Next epoch block**: `SubnetInfoRuntimeApi::get_next_epoch_start_block(netuid)` = `min(LastEpochBlock + tempo, PendingEpochAt if >0)`; `None` if `tempo == 0` (`run_coinbase.rs:1262-1276`). It does *not* account for cap deferral or the safety net. Python `epoch_status` bundles `Tempo`, `LastEpochBlock`, `BlocksSinceLastStep`, `PendingEpochAt`, `SubnetEpochIndex`, this API, and block time at one pinned block (`PY/reads/epochs.py:84-128`).
- **Observing the epoch**: Python `wait_for_epoch` subscribes to new heads and polls `LastEpochBlock` until it exceeds the baseline (`PY/client.py:418-443`). An rtao equivalent: subscribe finalized heads, read `LastEpochBlock` (or watch for `IncentiveAlphaEmittedToMiners`/`EpochSkipped` events for the subnet in each block's `System.Events`).
- **Owner `trigger_epoch`** (`coinbase/tempo_control.rs:80-125`): rejected if commit-reveal enabled (`DynamicTempoBlockedByCommitReveal`), if a trigger is pending, if the auto epoch is within `AdminFreezeWindow` blocks, or rate-limited; sets `PendingEpochAt = now + AdminFreezeWindow` and emits `EpochTriggered{fires_at}`.
- **Admin freeze window** (`utils/misc.rs:59-81`): owner hyperparam calls fail with `AdminActionProhibitedDuringWeightsWindow` when `remaining_blocks_to_auto_epoch < AdminFreezeWindow` (default 10) or a trigger is pending. Owner tooling should check `get_next_epoch_start_block − now >= AdminFreezeWindow` before submitting.
- **`set_tempo`** resets the cycle (`LastEpochBlock = now`), bounded `[360, 50400]` for owners, rate-limited by `TransactionType::TempoUpdate` (`tempo_control.rs:9-39`).

### 4.3 Weights and commit-reveal windows (what a validator must do)

- `set_weights` path (`subnets/weights.rs:980-1075`): root forbidden; `uids.len()==values.len()`; mechanism must exist; `uids.len() <= SubnetworkN`; hotkey registered; `check_weights_min_stake` (≥ `StakeThreshold` stake weight); `version_key >= WeightsVersionKey` (`check_version_key`); **rate limit only when commit-reveal is disabled**: `block − LastUpdate[idx][uid] >= WeightsSetRateLimit` (first-ever set bypasses since `LastUpdate==0`; `weights.rs:1295-1315`); validator permit required unless self-weight or owner uid (`:1350-1368`); no duplicate / invalid uids; `weights.len() >= min(MinAllowedWeights, SubnetworkN)` unless self-weight (`:1370-1390`); max-upscale then `max_weight_limited` (max ≤ `MaxWeightsLimit` after normalisation unless `MaxWeightsLimit==65535`; `:1408-1420`). Under `EpochConsensus::Null` the self-weight/permit rules differ (`:1058-1074,1351-1353`).
- `commit_weights` (CRv2, `weights.rs:150-230`): requires `CommitRevealWeightsEnabled`; rate-limited via `check_rate_limit` (bumps `LastUpdate` at commit time — so "blocks since last update" measures commits, `PY/reads/epochs.py:72-75`); commit tagged with `commit_epoch = current_epoch_with_lookahead` (`SubnetEpochIndex`, +1 if an epoch fires this block; `:1471-1479`); max 10 unrevealed. `reveal_weights` must land when `current_epoch == commit_epoch + RevealPeriodEpochs` exactly (`is_reveal_block_range`, `:1451-1456`; `RevealTooEarly` otherwise), and expires once `current_epoch > commit_epoch + reveal_period` (`ExpiredWeightCommit`, `:1483-1488,719-737`). Hash = `BlakeTwo256(who, netuid_index, uids, values, salt, version_key)` (`:1524-1533`).
- `commit_timelocked_weights` (CRv3/4, `weights.rs:379-490`): requires `commit_reveal_version == CommitRevealWeightsVersion` (**4** on finney; `IncorrectCommitRevealVersion` otherwise); payload is a tlock ciphertext (`TLECiphertext<TinyBLS381>` over drand quicknet, `reveal_commits.rs:7-12`) of SCALE `WeightsTlockPayload { hotkey: Vec<u8>, uids: Vec<u16>, values: Vec<u16>, version_key: u64 }` (`reveal_commits.rs:19-26`); ≤ 5000 bytes for Yuma subnets; queued under `TimelockedWeightCommits[idx][cur_epoch]`; auto-revealed by the chain at the first block of epoch `cur_epoch + reveal_period` provided `Drand.Pulses[reveal_round]` exists — so the client must choose `reveal_round` ≤ the drand round at that block (Python computes "reveals_at" from the round: `PY/timelock.py`; `PY/reads/weights.py:103-112`). `LastUpdate` is bumped at commit (`weights.rs:447,484`).
- `EpochConsensus::Null` subnets route `commit_timelocked_weights` to `enqueue_null_timelock` (`weights.rs:430-449`) — different queue, same event.

### 4.4 Block time, tempo, and localnet

`MILLISECS_PER_BLOCK = prod_or_fast!(12000, 250)`; `MINUTES = 60_000 / MILLISECS_PER_BLOCK` (`SUB/common/src/lib.rs:183-193`); `INITIAL_SUBNET_TEMPO = prod_or_fast!(360, 10)` (`SUB/runtime/src/lib.rs:904`); `MaxUidsTrimmingRateLimit = prod_or_fast!(30*7200, 1)` (`pallets/subtensor/src/lib.rs:1131`). The `fast-runtime` feature is passed by `scripts/localnet.sh:47` (`FEATURES="pow-faucet metadata-hash fast-runtime"`) and both fast and non-fast binaries are built into `Dockerfile-localnet:20-21`. Live finney advertises `Aura.SlotDuration = 12000`. Detect block time from that constant (as Python does) rather than hard-coding; tempo 360 × 12 s ≈ 72 min on mainnet, 10 × 0.25 s = 2.5 s on fast localnet.

### 4.5 Yuma terms the metagraph exposes (for interpretation)

Per `SUB/docs/concepts/emissions.mdx` "Yuma Consensus" and `SUB/docs/internals/consensus.mdx:110-121,125-143`: stake weight = `alpha + tao×tao_weight`; validators below `StakeThreshold` zeroed; neurons with `LastUpdate` older than the effective activity cutoff are inactive; permits = top-K by stake weight; consensus = stake-weighted median at `kappa`; rank = stake-weighted clipped weights → **incentive** (normalised, PerU16); **validator_trust** = row-sum of clipped weights; bonds EMA with `alpha = 1 − bonds_moving_avg/1e6` (Yuma3/liquid-alpha variants per `Yuma3On`, `LiquidAlphaOn`); **dividends** = bonds × incentive (PerU16). Per-tempo alpha split: 18% owner (`SubnetOwnerCut`), 41% miners, 41% validators+stakers (`emissions.mdx` "The per-tempo split"). Per-miner `trust`, `rank`, `pruning_score` are no longer computed (docs + code).

---

## 5. Payload sizes, performance, and what to cache

### 5.1 SCALE size model (estimates — not measured against a live node)

Fixed leaf sizes: `AccountId32` 32 B; `AxonInfo` 34 B; `PrometheusInfo` 31 B; `bool` 1 B; `Compact<u16>` 1 B (<64), 2 B (<16 384), 4 B (≥16 384, e.g. PerU16 near 65535); `Compact<u64>` 4 B for block numbers (< 2^30), 5–9 B for rao amounts ≥ 2^30 (e.g. 1 TAO = 1e9 rao → 5 B; 1 000 TAO → 7 B); zero → 1 B.

**`Metagraph` per UID** (typical mixed subnet; identities mostly `None`): hotkeys 32 + coldkeys 32 + identities ~1 + axons 34 + active 1 + validator_permit 1 + last_update 4 + emission ~4 + dividends/incentives/consensus ~6 + block_at_registration 4 + alpha/tao/total_stake ~12 + tao_dividends_per_hotkey 33 (32 + zero) + alpha_dividends_per_hotkey ~35 + collateral ×3 ~6 ≈ **~205 B/UID**, plus a ~0.3–0.5 KB header (name/symbol/identity/hyperparams). Identities add ~50–300 B for each coldkey that set one.

| n UIDs | `get_metagraph` | `get_neurons_lite` (~195 B/UID) | `get_neurons` (full) | `get_selective_metagraph` validator subset (§5.2, ~90 B/UID) |
|---|---|---|---|---|
| 256 | ~53 KB | ~50 KB | lite + weights/bonds: with V active validators each holding ~n nonzero weights, ≈ V×n×12 B → V=64: +197 KB ≈ **250 KB** | ~23 KB |
| 1024 (Null-consensus or legacy only; Yuma cap is 256) | ~210 KB | ~200 KB | V=64: +786 KB ≈ **1 MB** | ~92 KB |
| 2500 (Null budget) | ~510 KB | ~490 KB | V=64: +1.9 MB ≈ **2.4 MB** | ~225 KB |

Over legacy JSON-RPC the SCALE bytes are hex-encoded (2× bytes + JSON framing), so a 256-UID metagraph is ~110 KB on the wire; with subxt's `jsonrpsee` WS backend this is a single response message — ensure the client's max response size is set generously (subxt's default WS max response is 10 MiB in recent versions; verify for the chosen subxt). `get_all_metagraphs` (≈ 128 subnets × 50–200 KB) is multi-megabyte and executes every per-UID stake computation for every subnet in one runtime call — avoid.

Runtime-side costs that dominate latency (inferred from code): the per-UID stake-weight pass with child/parent inheritance (`stake_utils.rs:269-320` → `get_inherited_for_hotkey_on_subnet` / `get_tao_inherited…` at `:444-520`), `get_subnet_state.emission_history` (`|subnets| × n` reads), `get_delegates` (iterates all delegates × full `AlphaV2` prefix each), and `SelectiveMetagraph` indices 67/68/69 (each recomputes the full stake pass).

### 5.2 What `get_selective_metagraph` saves

A validator loop needs: `[7 block, 8 tempo, 9 last_step, 10 blocks_since_last_step, 26 weights_version, 27 weights_rate_limit, 28 activity_cutoff, 46/47 CR settings, 30 num_uids, 52 hotkeys, 55 axons, 56 active, 57 validator_permit, 59 last_update, 60 emission, 61 dividends, 62 incentives, 66 block_at_registration, 69 total_stake]` → ~90 B/UID (skipping coldkeys 32, identities, tao/alpha dividend lists 68, alpha/tao stake 8, collateral 6) ≈ **55–60 % smaller** than the full metagraph, and only one stake pass instead of one (full) — but requesting 67+68+69 together costs three passes, so request only `69 TotalStake` (and derive nothing else) or accept one extra pass for `67`. A miner loop (needs validator hotkeys/permits/stake to verify btauth signatures) needs `[52, 57, 69]` (+`72 validators`) ≈ 40 B/UID. For mechanisms, `get_selective_mechagraph` with `[59, 62]` fetches only the mechanism-specific columns.

Alternative cheap reads when only one column changes: `ValidatorPermit`, `Active`, `Incentive`, `Dividends`, `Consensus`, `Emission`, `LastUpdate` are each one `Vec` storage value (`state_getStorage`), ~1–4 B/UID; `Keys` prefix via `state_getKeysPaged`+`state_queryStorageAt` (or subxt `storage().iter()`) gives the UID→hotkey map in one paged scan.

### 5.3 Caching plan for a Rust client (recommendation)

- **Per runtime (`spec_version`)**: metadata + type registry + storage key prefixes + runtime-API ids and `api_version`s; refresh on `state_getRuntimeVersion` change (Python validates with a TTL against `state_getRuntimeVersion`: `PY/_substrate.py:435-441`). Keep `Aura.SlotDuration`, `ExistentialDeposit`, pallet/call/event indices here.
- **Per subnet, refresh on epoch boundary (`LastEpochBlock` change) or on `*Set` events**: hyperparams (`get_subnet_hyperparams_v3` or `DynamicInfo` + storage), `TokenSymbol`, `SubnetOwnerHotkey`, `MechanismCountCurrent`, `MechanismEmissionSplit`, `SubnetEpochConsensus`, the epoch output vectors (`ValidatorPermit`, `Active`, `Incentive`, `Dividends`, `Consensus`, `Emission`, `ValidatorTrust`), `Bonds`.
- **Per subnet, refresh on `NeuronRegistered`/`HotkeySwapped*`/`AxonServed` events (or every few blocks)**: `Keys` (uid→hotkey), `Uids` for own hotkey, `Axons`/`NeuronCertificates` of peers, `BlockAtRegistration`.
- **Per block (cheap scalars)**: `System.Number`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex`, own `LastUpdate[uid]`, own `WeightCommits`/`TimelockedWeightCommits` queue, own `System.Account.nonce`.
- **Never cache across blocks without a block hash**: prices (`current_alpha_price_all`), stake positions, `Burn`/`Difficulty` (adjust every `AdjustmentInterval`).
- Pin multi-read snapshots to one block hash (Python's `view.at()` pattern, `PY/reads/base.py:11-13`) — pass `at` to every `state_call`/storage read.

---

## 6. How the v11 Python reads layer chooses storage vs runtime API (so rtao can mirror it)

Registry/dispatch: `PY/reads/base.py` (`@read(name, params, category, render)`, `scalar_read` for one-storage-value reads, `utf8_text` for `Vec<Compact<u8>>` text). Catalog with the exact chain "hits" per read: `DOCS/reads.json` (102 entries; `hits[].kind ∈ {storage, runtime, constant}`). Summary of the rule set observed in the code:

1. **Per-UID vectors / whole-subnet views → runtime API** (`get_metagraph`, `get_neurons_lite`/`get_neurons`, `get_subnet_hyperparams_v3`, `get_next_epoch_start_block`), because the runtime does the joins (Keys→Owner→Identity→Axon, stake inheritance) in one call (`PY/reads/neurons.py:41-66`, `subnets.py:150-190`, `epochs.py:13-47`, `metagraph.py:238-256`).
2. **Anything involving stake amounts per coldkey → runtime API** (`StakeInfoRuntimeApi::*`, `DelegateInfoRuntimeApi::*`, `SwapRuntimeApi::*`, `BetaBasketRuntimeApi::*`) because of share-pool math and swap simulation (`staking.py:65-233`, `delegation.py`, `prices.py`, `root.py`, `locks.py` hits in `DOCS/reads.json`).
3. **Single scalars / single-key maps → storage** (`Tempo`, `Burn`, `SubnetworkN`, `Difficulty`, `ImmunityPeriod`, `WeightsSetRateLimit`, `MinAllowedWeights`, `MaxWeightsLimit`, `RevealPeriodEpochs`, `CommitRevealWeightsEnabled`, `MechanismCountCurrent`, `MechanismEmissionSplit`, `SubnetEmissionEnabled`, `TxRateLimit`, `Uids`, `Owner`, `OwnedHotkeys`, `StakingHotkeys`, `ChildKeys`, `ParentKeys`, `PendingChildKeys`, `Delegates`+`Min/MaxDelegateTake`, `IdentitiesV2`, `SubnetIdentitiesV3`, `TokenSymbol`, `AutoStakeDestination`, `AccountFlags`, `AssociatedEvmAddress`, `System.Account`, `Proxy.Proxies`, `Multisig.Multisigs`, `MevShield.NextKey`) — `hyperparameters.py`, `subnets.py:24-123`, `neurons.py:69-135`, `chain.py`, `accounts.py`.
4. **Matrices → storage prefix scans (`query_map`) keyed by the mechanism index**: `Weights`, `Bonds`, `TimelockedWeightCommits` (`weights.py:12-16,41,67,98-100`); `IsNetworkMember` prefix by hotkey for `netuids_for_hotkey`; `SubnetIdentitiesV3`/`TokenSymbol`/`NetworksAdded`/`Tempo`/`Burn`/`SubnetworkN` full-map scans for listings (`subnets.py:46-70`, four batched `query_map`s instead of per-subnet calls).
5. **Composite reads pin a block** (`view = await view.at()`), then `asyncio.gather` the parts (`subnets.py:30-37`, `epochs.py:98-115`, `metagraph.py:245-250`).
6. **Unit tagging at the boundary**: `Balance(rao, netuid)` where netuid 0 = TAO; `PerU16` → fraction via `ratio_fraction("PerU16", v)`; `I96F32`/`I32F32`/`U64F64` → `bits / 2^frac` (`metagraph.py:31-32,343-345,398`; `subnets.py:85-89,126-147`). `Vec<Compact<u8>>` text fields decoded with `utf8_text` (`base.py:169-185`).
7. **Fee estimation**: `TransactionPaymentApi_query_info` via `state_call` with a zero-signature extrinsic (`_transport/interface.py:714-731`).
8. **Block time**: `Aura.SlotDuration` constant / 1000 (`_substrate.py:423-433`); `is_fast_blocks = block_time == 0.25` (`client.py:71,345-347`).

Full per-read hit table (from `DOCS/reads.json`; `S:`=storage, `R:`=runtime API, `C:`=constant):

```
accepts_locked_alpha S:AccountFlags | alpha_price R:Swap.current_alpha_price | alpha_prices R:Swap.current_alpha_price_all
associated_evm_key S:AssociatedEvmAddress | auto_stake(_all) S:AutoStakeDestination | balance(s) S:System.Account
blocks_since_last_step S:BlocksSinceLastStep | blocks_since_last_update S:LastUpdate | blocks_until_next_epoch R:SubnetInfo.get_next_epoch_start_block
bonds S:Bonds | burn S:Burn | childkey_threshold_suspended S:ChildkeyThresholdSuspended | children S:ChildKeys | parents S:ParentKeys | pending_children S:PendingChildKeys
coldkey_lock S:Lock (+R:StakeInfo.get_coldkey_lock, S:DecayingLock) | coldkey_swap_announcement S:ColdkeySwapAnnouncements,ColdkeySwapDisputes
commit_reveal_enabled S:CommitRevealWeightsEnabled | commitment S:Commitments.CommitmentOf | revealed_commitment S:Commitments.RevealedCommitments
crowdloan(s) S:Crowdloan.Crowdloans | crowdloan_contributors S:Crowdloan.Contributions
delegate / is_delegate R:DelegateInfo.get_delegate | delegates R:get_delegates | delegated R:get_delegated | delegate_take S:Delegates,MinDelegateTake,MaxDelegateTake
difficulty S:Difficulty | immunity_period S:ImmunityPeriod | min_allowed_weights S:MinAllowedWeights | max_weight_limit S:MaxWeightsLimit | reveal_period S:RevealPeriodEpochs | weights_rate_limit S:WeightsSetRateLimit | subnet_emission_enabled S:SubnetEmissionEnabled
epoch_status S:Tempo,LastEpochBlock,BlocksSinceLastStep,PendingEpochAt,SubnetEpochIndex + R:get_next_epoch_start_block | next_epoch_start_block R:same
existential_deposit C:Balances.ExistentialDeposit | hotkey_conviction R:StakeInfo.get_hotkey_conviction | most_convicted_hotkey R:get_most_convicted_hotkey_on_subnet
hotkey_identities S:Owner,IdentitiesV2 | identity S:IdentitiesV2 | hotkey_owner S:Owner | owned_hotkeys S:OwnedHotkeys | staking_hotkeys S:StakingHotkeys | uid S:Uids | netuids_for_hotkey S:IsNetworkMember
lease(s) S:SubnetLeases | locks_for_coldkey S:Lock | locks_for_hotkey S:LockingColdkeys
mechanism_count S:MechanismCountCurrent | mechanism_emission_split S:MechanismEmissionSplit | metagraph R:SubnetInfo.get_metagraph | mev_shield_next_key S:MevShield.NextKey
miner_collateral S:Owner,MinerCollateral | subnet_collateral S:Uids,MinerCollateral | collateral_policy S:CollateralLockShare,CollateralDrainRatio
multisig S:Multisig.Multisigs | proxies S:Proxy.Proxies | neurons R:NeuronInfo.get_neurons_lite|get_neurons
quote_stake R:Swap.sim_swap_tao_for_alpha | quote_unstake R:Swap.sim_swap_alpha_for_tao
root_basket_* / validator_basket_* / basket_* R:BetaBasket.* (+S:BasketRate) | root_claim_threshold S:RootClaimableThreshold
stake R:StakeInfo.get_stake_info_for_hotkey_coldkey_netuid | stake_for_coldkey(s) / stake_value_for_coldkey(s) R:get_stake_info_for_coldkey(s) | stake_availability(_for_coldkey) R:get_stake_availability_for_coldkeys | total_alpha_staked S:TotalAlphaStaked
subnet S:Tempo,Burn,SubnetworkN | subnets S:NetworksAdded,Tempo,Burn,SubnetworkN | subnet_hyperparameters R:get_subnet_hyperparams_v3 | subnet_identity / subnet_names S:SubnetIdentitiesV3 | subnet_registration_cost R:SubnetRegistration.get_network_registration_cost
subnet_start_schedule S:NetworkRegisteredAt + C:InitialStartCallDelay | subnet_tao_flows S:SubnetEmaTaoFlow | subnet_convictions S:HotkeyLock,DecayingHotkeyLock,OwnerLock,DecayingOwnerLock,SubnetOwnerHotkey,SubnetAlphaOut,SubnetProtocolAlpha,AlphaAssets.AlphaBurned,UnlockRate,MaturityRate,NetworkRegisteredAt
timelocked_weight_commits S:TimelockedWeightCommits | token_symbols S:TokenSymbol | tx_rate_limit S:TxRateLimit | weights S:Weights
timestamp / block_time / is_fast_blocks / block_info: client-level (Timestamp.Now, Aura.SlotDuration, chain_getBlock)
```

Contrast with the archived v10 SDK: its `Metagraph.sync` chained `_assign_neurons` (neurons_lite/neurons runtime API), `_set_weights_and_bonds`, `_get_all_stakes_from_chain` and `_apply_extra_info` (`get_metagraph`) and warned about non-archive nodes beyond ~300 blocks (`/…/upstream/bittensor/bittensor/core/metagraph.py:1356-1385`); v11 collapses this into one `get_metagraph` + two Commitments map scans. The "~300 blocks on lite nodes" limit is restated in `SUB/docs/concepts/network.mdx` ("Networks and endpoints").

---

## 7. JSON-RPC methods still in use (non-runtime-API)

Verified from the v11 transport (`PY/_transport/*.py`, grep of string literals) and the node's RPC wiring (`SUB/node/src/rpc.rs:121-172`):

**Used by the v11 SDK (legacy `state_*`/`chain_*`/`author_*` namespace):**
`state_call` (runtime APIs; `runtime_api.py:239`, `interface.py:724`, `runtime.py:296`), `state_getStorageAt` (`interface.py:252`), `state_queryStorageAt` (batched values; `interface.py:272,368`), `state_getKeysPaged` (prefix scans; `interface.py:294,363`), `state_getMetadata` (`runtime.py:307`), `state_getRuntimeVersion` (`runtime.py:162,185`), `chain_getHead`, `chain_getFinalizedHead`, `chain_getBlockHash`, `chain_getHeader`, `chain_getBlock` (`interface.py:179-223,416,682`), `chain_subscribeNewHeads` / `chain_subscribeFinalizedHeads` with matching `chain_unsubscribe*Heads` (`interface.py:439-455`), `author_submitExtrinsic`, `author_submitAndWatchExtrinsic` / `author_unwatchExtrinsic` (`interface.py:635-656`, `rpc.py:66`). No `system_*` or `payment_*` RPC is called by the SDK (nonce via storage `System.Account`/`AccountNonceApi`; fees via `TransactionPaymentApi_query_info` over `state_call`).

**Served by the node but not used by v11** (`node/src/rpc.rs:138-144`): `substrate_frame_rpc_system` (`system_accountNextIndex`, `system_dryRun`), `pallet_transaction_payment_rpc` (`payment_queryInfo`, `payment_queryFeeDetails`), **custom subtensor RPCs** (`SUB/pallets/subtensor/rpc/src/lib.rs:21-176`): `delegateInfo_getDelegates/getDelegate/getDelegated`, `neuronInfo_getNeurons(Lite)/getNeuron(Lite)`, `subnetInfo_getSubnetInfo(_v2)/getSubnetsInfo(_v2)/getSubnetHyperparams(V2)/getAllDynamicInfo/getDynamicInfo/getAllMetagraphs/getMetagraph/getAllMechagraphs/getMechagraph/getSubnetState/getBlockEmission/getLockCost/getSelectiveMetagraph/getSelectiveMechagraph/getColdkeyAutoStakeHotkey/getSubnetToPrune/getSubnetAccountId`, `stakeInfo_getColdkeyLock`, `betaBasket_*` (8 methods), and **swap RPCs** (`SUB/pallets/swap/rpc/src/lib.rs:20-32`): `swap_currentAlphaPrice`, `swap_currentAlphaPriceAll`, `swap_simSwapTaoForAlpha`, `swap_simSwapAlphaForTao`. These return raw SCALE `Vec<u8>` (JSON arrays of bytes) or scalars; they are strictly a subset of the runtime APIs (no `getSubnetHyperparamsV3`, no `getNextEpochStartBlock`, no `stakeInfo_getStakeInfoForColdkey`). Treat as legacy/deprecated for rtao; `ManualSeal` RPCs (`engine_createBlock`) exist only on dev nodes (`rpc.rs:150-157`). Frontier `eth_*`/`net_*`/`web3_*` RPCs are mounted (`rpc.rs:165-171`; `PY/evm/rpc.py` uses `eth_*`).

**Not observed anywhere in the SDK**: the new JSON-RPC-spec-v2 `chainHead_v1_*` / `transaction_v1_*` / `archive_*` methods. The node is built on polkadot-sdk (RaoFoundation fork, `SUB/Cargo.toml:256`), so `sc-rpc-spec-v2` endpoints are *probably* exposed, but whether public finney/lite endpoints pass them through is **unverified** — rtao should default to the legacy namespace (subxt `LegacyBackend`) and treat `chainHead` as optional.

---

## 8. Observations from the Rust ecosystem (what others already do)

Grep over the cloned crates: `CortexLM_bittensor-rs` (subxt 0.50; heavy `storage()` use, `get_metagraph` ×19, `get_stake_info_for_coldkey`, `get_selective_metagraph`, `subscribe_finalized`), `one-covenant_bittensor-rs` (subxt 0.44; `get_metagraph`, `get_selective_metagraph` ×20, `get_subnet_hyperparams(_v2)`, `get_neurons_lite`, `get_subnet_state`), `unarbos_agcli` (subxt 0.38; `get_neurons_lite` ×85, `get_dynamic_info`, `get_all_dynamic_info`, `get_subnet_hyperparams`, dynamic storage), `ErgodicLabs_btt` (subxt 0.50; `subxt::dynamic::runtime_api_call` + `dynamic::storage`), `inference-labs-inc_subnet-2` (subxt 0.50 with `reconnecting-rpc-client`; `get_neurons_lite`), `inference-labs-inc_btlightning` (`call_raw` + `scale_decode`), `timehash13_subtensor-api-rs` (subxt 0.41), `womboai_rusttensor` (subxt 0.38 + `substrate-compat`). Takeaways: (a) everyone goes through runtime APIs for the metagraph/neurons and through storage for scalars and matrices, exactly like the Python layer; (b) subxt versions are fragmented (0.38–0.50) and pinned to a static metadata snapshot, which is the maintenance pain rtao's design goals target; (c) nobody uses `get_subnet_hyperparams_v3` yet (it is `api_version(2)`), and only CortexLM/one-covenant use the selective metagraph.

---

## 9. Recommendations / decisions surfaced for the rtao architects

1. **Transport**: legacy JSON-RPC namespace (`state_call`, `state_getStorage*`, `chain_subscribe*Heads`, `author_submitAndWatchExtrinsic`) via subxt's `LegacyBackend`; make `chainHead_v1` opt-in. Do not implement the custom `subnetInfo_*`/`neuronInfo_*`/`swap_*` RPCs.
2. **Type strategy**: generate from metadata at build time (subxt codegen with `--derive Encode,Decode,Clone,PartialEq,Debug`) *and* keep a thin, hand-curated "stable" layer (`Metagraph`, `NeuronInfoLite`, `DynamicInfo`, `StakeInfo`, hyperparams-by-name) with `From` impls, so a runtime upgrade only touches the generated layer. Use `get_subnet_hyperparams_v3` (self-describing) as the canonical hyperparam read and fall back to v2/storage for `rho`/difficulty/adjustment fields it omits.
3. **Units**: newtype `Rao`/`AlphaRao(netuid)`/`PerU16`/`I96F32`/`U64F64`/`I32F32` with explicit conversions; never `f64` on the wire. Treat `Metagraph.total_stake` as a stake-weight scalar, not TAO.
4. **Mechanisms**: model `NetUidStorageIndex` explicitly (`netuid + 4096*mecid`) for `Weights/Bonds/LastUpdate/Incentive/WeightCommits/TimelockedWeightCommits` and for event filtering.
5. **Epoch clock**: implement `EpochClock` from `LastEpochBlock`, `Tempo`, `PendingEpochAt`, `SubnetEpochIndex`, `MaxEpochsPerBlock` + `get_next_epoch_start_block`; never the modular formula; expose `wait_for_epoch` by watching `LastEpochBlock`.
6. **Commit-reveal**: ship CRv4 timelock (tlock over drand quicknet, `WeightsTlockPayload`) as the default weight-setting path since `CommitRevealWeightsEnabled` defaults to true and version is 4; keep plain `set_weights` for subnets with CR off; expose `is_reveal_block_range`/expiry logic client-side mirroring `weights.rs:1451-1488`.
7. **Read budget**: default validator sync = `get_selective_metagraph` with the §5.2 index set (one stake pass), `get_dynamic_info` for the header, storage scalars for epoch state; full `get_metagraph` only on demand; never `get_all_metagraphs`/`get_subnet_state`/`get_delegates` in loops.
8. **Dead fields**: mark `subnet_emission`, `pending_root_emission`, `tao_dividends_per_hotkey`, `pruning_score`, `trust`, `rank` (metagraph), `rank/trust/pruning_score` (NeuronInfo), `locked/drain/tao_emission` (StakeInfo), `emission_values/network_modality/network_connect` (SubnetInfo) as `#[deprecated]` in the stable layer and do not surface them in docs.
9. **Upgrade detection**: compare `state_getRuntimeVersion.spec_version` and the runtime-API `api_version` list on connect and on every finalized head; gate v2+/v5 methods behind the advertised version.
10. **Open validation before 1.0**: measure real `get_metagraph`/`get_selective_metagraph` latency and byte sizes on finney for a 256-UID subnet (estimates in §5 are analytic); confirm whether public endpoints expose `chainHead_v1`; confirm the live `StakeThreshold`/`TaoWeight` values (storage defaults differ from the documented live values).
