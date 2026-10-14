# Report I — Legacy SDK/CLI surface (v10 / v9) and the bittensor.com docs corpus

Research input for the rtao plan. Date: 2026-10-13. Reader: rtao architects (expert Rust/Substrate).

Path abbreviations used in citations (all under `SP/`):

| Abbrev | Path | What |
|---|---|---|
| `$BT` | `upstream/bittensor/bittensor` | archived Python SDK v10.5 package |
| `$BTCLI` | `upstream/btcli/bittensor_cli` | archived `bittensor-cli` 9.23.2 (`upstream/btcli/pyproject.toml:7`) |
| `$SDK11` | `upstream/subtensor/sdk/python/bittensor` | unified v11 package in the monorepo |
| `$DOCS` | `upstream/subtensor/docs` | bittensor.com docs source (.mdx) |
| `$WEB` | `upstream/subtensor/website/apps/bittensor-website` | docs site + catalog generator |
| `$CAT` | `docs` | fetched `bittensor.com_llms.txt`, `bittensor.com_llms-full.txt`, `intents.json`, `reads.json`, `errors.json`, `index.json` |
| `$META` | `meta` | finney metadata + subxt explore dumps |
| `$TPL` | `upstream/bittensor-subnet-template` | archived subnet template (v10-era neuron code) |
| `$ASI` | `upstream/async-substrate-interface` | archived substrate client used by v10 (2.2.1, `pyproject.toml:3`) |
| `$SN2` | `upstream/inference-labs-inc_subnet-2` | native Rust subnet (clap conventions precedent) |

Verification legend: every claim below cites a file:line. Items I could not verify on disk are explicitly marked **(inferred)**. Mapping rows marked **†** are my own pairing (based on identical storage/runtime targets) rather than a row in `$DOCS/migration.mdx`.

---

## 0. Executive summary (what matters for rtao)

1. **v10 `Subtensor` has 235 methods** (`grep -c '^    (async )?def ' $BT/core/subtensor.py` → 235; `AsyncSubtensor` has 236: the only differences are `__enter__/__exit__` vs `__aenter__/__aexit__/initialize`). v11 replaced them with 102 named reads in 14 namespaces + 83 intents + 4 generic accessors (`$CAT/reads.json` has 102 entries, `$CAT/intents.json` has 83; namespaces at `$SDK11/namespaces.py:279`). Migration page: "the `Subtensor` class and its ~165 methods are gone" (`$DOCS/migration.mdx:9`). Section 1 maps all 235.
2. **The v10 code that subnets in the wild actually depend on is small**: the archived subnet template calls `subtensor.metagraph`, `get_current_block`, `is_hotkey_registered`, `subnet_exists`, `set_weights`, `serve_axon`, `neurons`, `min_allowed_weights`, `max_weight_limit` plus the metagraph arrays (`axons`, `n`, `hotkeys`, `S`, `uids`, `validator_permit`, `last_update`) and `weight_utils.process_weights_for_netuid` / `convert_weights_and_uids_for_emit` (call-site counts in §1.5). SN1 and Chi use the same handful (§1.5).
3. **Weight semantics changed subtly between v10 and v11** (§2): v10 `convert_weights_and_uids_for_emit` max-upscales then rounds to u16 and drops zeros (`$BT/utils/weight_utils.py:165-210`); v11 `normalize` does the same (`$SDK11/intents/weights.py:54-81`) but v11 additionally clips to `max_weight_limit` with a mass-redistributing cutoff identical to v10's `normalize_max_weight` (`$SDK11/intents/weights.py:83-114` vs `$BT/utils/weight_utils.py:26-71`), enforces `min_allowed_weights` as a hard error rather than the v10 "all ones" fallback, exempts a single self-weight, and defaults `version_key` to 0 instead of the SDK version (`$DOCS/migration.mdx:392-404`). The chain's enforced max-weight limit is pinned to `u16::MAX` (`$CAT/bittensor.com_llms-full.txt:11757-11765`).
4. **Balance**: v10 `Balance(int)`=rao, `Balance(float)`=tao, unit by `netuid`, cross-unit arithmetic raises (`$BT/utils/balance.py:88-103, 12-36`). v11 keeps the netuid-tag, makes `.tao` on alpha raise, drops `from_tao(netuid=)` in favour of `from_alpha`, uses `Decimal` for exact amounts (`$SDK11/balance.py:44-125`; `$DOCS/migration.mdx:414-432`).
5. **Config**: v10 argparse keys are dotted (`--subtensor.network`, `--wallet.name`, `--axon.port`, `--logging.debug`, `--neuron.*`) with `BT_*` env defaults and a `--config` YAML (`$BT/core/settings.py:116-154`; `$BT/core/config.py:195-207`). v11 deletes all of it (`$DOCS/migration.mdx:162-180`). SN2 (native Rust) already uses clap `alias = "wallet.name"` etc. (`$SN2/crates/sn2-validator/src/cli.rs:9-25`) — the natural rtao convention.
6. **Metagraph persistence**: v10 `save()` writes `~/.bittensor/metagraphs/network-<net>/netuid-<n>/block-<b>.pt` which is a **pickle** (non-torch) or `torch.save` (torch) of `state_dict()` (`$BT/core/metagraph.py:94-116, 595-617, 762-804`). Not worth reading from Rust. The template validator's `state.npz` (`step`, `scores`, `hotkeys`) is a plain NumPy zip (`$TPL/template/base/validator.py:366-385`) and *is* readable.
7. **CLI**: v9 = 11 groups (+ `crowd`), ~120 commands, dotted/underscored/dashed flag spellings (`$BTCLI/cli.py:861-1396`); v11 = hand-written groups + **generated** `query <read>` and `tx <intent>` groups + `call`, `tools`, `explain` (`$SDK11/cli/main.py:77-145, 321, 405`). Full trees in §4 and Appendices C/D.
8. **Docs corpus**: `llms.txt` (139 lines) is a curated index with raw-markdown links and explicit grep/catalog/code-search instructions; `llms-full.txt` is 36,025 lines / 728 `# Title (/docs/slug)` pages; the reference section (84 tx, 103 query, 19+391 error, 49 hyperparameter pages) and the three catalogs are **generated from the SDK registries** by `$WEB/scripts/generate.py` (`$WEB/README.md:15-22`). Catalog schemas in §5.3; the generator's source-linking mechanics in §5.4; a concrete rtao docs/llms proposal in §5.6.

---

## 1. v10 `Subtensor` / `AsyncSubtensor` public surface → v11 → rtao compat

### 1.0 Method inventory facts

- `$BT/core/subtensor.py` (9,309 lines) defines `class Subtensor(SubtensorMixin)` at line 197; `$BT/core/async_subtensor.py` (10,823 lines) defines `class AsyncSubtensor(SubtensorMixin)` at line 202; `SubtensorMixin` is in `$BT/core/types.py:44`.
- Method name sets are identical except `__enter__`, `__exit__` (sync only) and `__aenter__`, `__aexit__`, `initialize` (async only) (computed with `comm` over the def lists; see §1.4).
- Layout (sync line numbers): utilities/generic accessors 221–808; queries 834–5136; extrinsic plumbing 5160–5381; extrinsics 5418–9258.
- `$BT/__init__.py` re-exports via `$BT/utils/easy_imports.py`: `Subtensor`, `AsyncSubtensor`, `get_async_subtensor`, `Axon`, `Dendrite`, `Synapse`, `StreamingSynapse`, `Tensor`, `Metagraph`, `Config`, `Balance`, `tao`, `rao`, `logging`, `MockSubtensor`, `SubnetsAPI`, `timelock`, `SubtensorApi`, `PriorityThreadPoolExecutor`, 30 chain_data types and ~40 exception classes (`$BT/utils/easy_imports.py:33-133`).
- v10 also had a namespaced facade `SubtensorApi` with properties `block, chain, commitments, crowdloans, delegates, extrinsics, metagraphs, mev_shield, neurons, proxies, queries, staking, subnets, wallets` (`$BT/extras/subtensor_api/__init__.py:24-270`) — a precursor of v11's namespaces.

### 1.1 Compat legend

| Code | Meaning for rtao |
|---|---|
| **A** | Worth a thin alias in an `rtao-compat` (or `#[deprecated]` extension trait) because subnets in the wild call it by this name (§1.5) — the alias forwards to the core API. |
| **C** | Covered by core rtao API under the v11 name; document the rename only. |
| **G** | No named read in v11 either; reachable only through generic accessors (storage/constant/runtime-API via codegen). rtao exposes it via its generic layer; no alias. |
| **X** | Retired (chain feature removed or v11 dropped it). Do not implement. |
| **I** | Private/internal in v10. Not part of the surface. |

v11 targets are written as the v11 Python spelling from `$DOCS/migration.mdx` (M:line) or `$CAT/reads.json` (`python` field) / `$CAT/intents.json` (`python_class`). The storage/runtime target column comes from the v10 method bodies (extracted by regex over `$BT/core/subtensor.py`; see the raw list in my working notes, reproduced per row).

### 1.2 Utilities, lifecycle and generic accessors (sync lines)

| v10 method (line) | What it does / chain target | v11 equivalent | rtao |
|---|---|---|---|
| `__init__` (221) | network/endpoint resolution, `fallback_endpoints`, `archive_endpoints`, `retry_forever`, `mock` | `bt.Subtensor(...)` same kwargs; `config=`/`mock=` gone (M:140-150) | C |
| `close` (274) | close websocket | optional in v11 (M:150) | C |
| `__enter__`/`__exit__` (294/297) | context manager | `async with bt.Subtensor()` (M:142) | C |
| `initialize` (async 310) | connect | gone — `await bt.Subtensor()` (M:143) | C |
| `_decode_crowdloan_entry`, `_get_block_hash`, `_get_substrate`, `_runtime_method_exists`, `_query_with_fallback`, `_runtime_call_with_fallback` (302–471) | private | — | I |
| `determine_block_hash` (380) | block → hash | `client.at(block)` snapshot (M:184-188) | C |
| `get_hyperparameter` (507) | `SubtensorModule.<param_name>` storage read, returns `None` if subnet missing | `client.hyperparameters.*` / `client.query(bt.storage.SubtensorModule.X,[netuid])` | C |
| `block` (541) | current block property | `sub.block` (M:276) | C |
| `sim_swap` (549) | `SwapRuntimeApi.sim_swap_tao_for_alpha` / `sim_swap_alpha_for_tao` | `prices.quote_stake` / `prices.quote_unstake` (M:219-220) | C |
| `query_constant` (648), `query_map` (672), `query_map_subtensor` (701), `query_module` (727), `query_runtime_api` (756), `query_subtensor` (782), `state_call` (808) | raw accessors | `client.query`, `client.query_map`, `client.constant`, `client.runtime` (M:197-204) | C — generic accessors are core rtao |
| `wait_for_block` (5063) | poll until block | `client.wait_for_block` (+`wait_for_timestamp`, `wait_for_epoch`) (M:279) | C |
| `metagraph` (4748) | builds `Metagraph(netuid, mechid, lite)` and `sync()`s | `client.subnets.metagraph(netuid)` typed object (M:290-306) | **A** (most-called method in the template; §1.5) |
| `filter_netuids_by_registered_hotkeys` (4431) | loop over `get_netuids_for_hotkey` | compose `neurons.netuids_for_hotkey` | C |
| `validate_extrinsic_params` (5160), `compose_call` (5222), `sign_and_send_extrinsic` (5260), `get_extrinsic_fee` (5381) | call composition; `sign_with` ∈ {coldkey,hotkey,coldkeypub}; `period` default 128; `wait_for_inclusion=True`, `wait_for_finalization=False` (5260-5300) | `client.submit_call(bt.calls.Pallet.fn(...), wallet, signer=...)`; fee via `plan(...).fee` (M:222, 383) | C |

### 1.3 Queries (sync line) → v11 → rtao

Grouped by topic. "storage" = `SubtensorModule` storage unless a pallet is given.

**Chain / blocks / time**

| v10 (line) | target | v11 | rtao |
|---|---|---|---|
| `get_current_block` (1420) | `System.Number` via substrate | `sub.block` / `await client.block()` (M:276) | **A** |
| `get_block_hash` (1433) | rpc | `client.block_info(block)` (M:277) | **A** |
| `get_block_info` (1455) | block + extrinsics, extracts `Timestamp.set` | `client.block_info` (M:277); read `block_info` | C |
| `get_timestamp` (4269) | `Timestamp.Now` | `client.timestamp` (M:278); read `timestamp` | C |
| `tx_rate_limit` (5045) | `TxRateLimit` | `chain.tx_rate_limit()` (M:280) | C |
| `is_fast_blocks` (4547) | checks `Aura` pallet storage | `client.is_fast_blocks()` (M:280) | C |
| `last_drand_round` (4706) | `Drand.LastStoredRound` | `bt.timelock.current_round()` (M:281) | C |
| `get_mev_shield_current_key` (2738) | `MevShield.CurrentKey` | no read (only `chain.mev_shield_next_key`) † | G |
| `get_mev_shield_next_key` (2780) | `MevShield.NextKey` | `chain.mev_shield_next_key()` (M:286) | C |
| `get_vote_data` (4376) | `Triumvirate/Voting` | senate removed (M:704) | X |

**Balances / accounts**

| v10 (line) | target | v11 | rtao |
|---|---|---|---|
| `get_balance` (1358) | `System.Account` | `balances.get` (M:210); read `balance` | **A** |
| `get_balances` (1379) | `query_multi` on `System.Account` | `balances.get_many` (M:211); read `balances` | C |
| `get_existential_deposit` (2260) | const `Balances.ExistentialDeposit` | `balances.existential_deposit()` (M:212) | C |
| `get_transfer_fee` (4300) | `payment_info` on `Balances.transfer_*` | `plan(intent).fee` (M:222) | C |
| `get_proxies` (3126) / `get_proxies_for_real_account` (3158) | `Proxy.Proxies` | `balances.proxies` (M:282) | C |
| `get_proxy_announcement(s)` (3193/3226) | `Proxy.Announcements` | no named read † | G |
| `get_proxy_constants` (3260) | `Proxy` pallet constants | `client.constant(bt.constants.Proxy.*)` † | G |
| `get_proxy_filter` (3307) | runtime api `get_proxy_filter` | runtime API renamed to `get_proxy_filters` (M:627) | G |
| `get_proxy_types` (3344) | runtime api `get_proxy_types` | generic runtime call † | G |
| `get_coldkey_swap_announcement` (1633) / `..._announcements` (1670) | `ColdkeySwapAnnouncements` | `balances.coldkey_swap_announcement` (M:283) | C |
| `get_coldkey_swap_announcement_delay` (1699), `_reannouncement_delay` (1773), `get_coldkey_swap_constants` (1727), `get_coldkey_swap_dispute(s)` (1801/1834) | storage/constants `ColdkeySwap*` | no named reads † | G |
| `query_identity` (4901) | `IdentitiesV2` | `identity.identity` (M:266) | C |
| `get_delegate_identities` (2145) | `IdentitiesV2` keyed by hotkey owner | `identity.hotkey_identities` (M:264) | C |

**Subnets / hyperparameters / epochs**

| v10 (line) | target | v11 | rtao |
|---|---|---|---|
| `all_subnets` (834) | runtime `get_all_dynamic_info` | `subnets.all()` (M:232) | C |
| `subnet` (4955) | runtime `get_dynamic_info` | `subnets.info(netuid)` (M:233) | C |
| `get_all_subnets_info` (1091) | runtime `SubnetInfoRuntimeApi.get_subnets_info_v2` | `subnets.all()` (M:232) | C |
| `get_subnet_info` (4065) | runtime `get_subnet_info_v2` | `subnets.info` (M:233) | C |
| `get_all_subnets_netuid` (1298) | `NetworksAdded` map | derive from `subnets.all()` (M:234) | C |
| `get_total_subnets` (4282) | `TotalNetworks` | derive (M:234) | C |
| `subnet_exists` (4986) | `NetworksAdded[netuid]` | `subnets.subnet(netuid)` and check (M:235) | **A** |
| `get_subnet_burn_cost` (3999) | runtime `SubnetRegistrationRuntimeApi.get_network_registration_cost` | `subnets.subnet_registration_cost()` (M:236) | C |
| `recycle` (4935) | `Burn` | `subnets.burn(netuid)` (M:237) | C |
| `get_subnet_price` (4113) / `get_subnet_prices` (4143) | runtime `current_alpha_price(_all)` | `prices.alpha_price` / `alpha_prices` (M:238) | C |
| `get_subnet_hyperparameters` (4040) | runtime `get_subnet_hyperparams_v3` (fallback v2, v1) | `subnets.subnet_hyperparameters` (M:239) | **A** (Chi calls it) |
| `difficulty` (996), `immunity_period` (4494), `min_allowed_weights` (4783), `max_weight_limit` (4725), `weights_rate_limit` (5136) | storage `Difficulty`, `ImmunityPeriod`, `MinAllowedWeights`, `MaxWeightsLimit` (u16→float via `u16_normalized_float`), `WeightsSetRateLimit` | same names on `client.hyperparameters` (M:240) | **A** for `min_allowed_weights`/`max_weight_limit` (used by `process_weights_for_netuid`, `$BT/utils/weight_utils.py:257-258`); C otherwise |
| `commit_reveal_enabled` (976) | `CommitRevealWeightsEnabled` | `subnets.commit_reveal_enabled` (M:241) | C |
| `get_subnet_reveal_period_epochs` (4182) | `RevealPeriodEpochs` | `hyperparameters.reveal_period` (M:242) | C |
| `tempo` (5024) / `subnetwork_n` (5008) | `Tempo` / `SubnetworkN` | fields on `subnets.info` / metagraph (M:243) | **A** (cheap; used in epoch math) |
| `blocks_since_last_step` (865) / `blocks_until_next_epoch` (906) / `get_next_epoch_start_block` (3008) | `BlocksSinceLastStep`; runtime `get_next_epoch_start_block` | same names on `client.epochs` (M:244) | C |
| `blocks_since_last_update` (886) | `LastUpdate[netuid][uid]` | `epochs.blocks_since_last_update` (M:245) | **A** (set_weights rate-limit gate in v10, `$BT/core/subtensor.py:8220-8224`) |
| `is_subnet_active` (4678) | `FirstEmissionBlockNumber` | `epochs.epoch_status` / `subnets.subnet_start_schedule` (M:246) | C |
| `get_mechanism_count` (2611) / `get_mechanism_emission_split` (2580) | `MechanismCountCurrent` (returns 1 if storage absent) / `MechanismEmissionSplit` | `subnets.mechanism_count` / `mechanism_emission_split` (M:247) | C |
| `get_epoch_schedule_state` (2332) | composes `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex`, `Tempo`, `BlocksSinceLastStep` | `epochs.epoch_status` † | C |
| `get_last_epoch_block` (2469), `get_pending_epoch_at` (3111), `get_subnet_epoch_index` (4025) | storage of the same names | no named reads † | G |
| `get_activity_cutoff_factor_milli` (1049), `get_max_/min_activity_cutoff_factor_milli` (2511/2820), `get_max_epochs_per_block` (2534), `get_max_tempo` (2557), `get_min_tempo` (2843), `get_admin_freeze_window` (1067), `get_owner_hyperparam_rate_limit` (3060), `get_start_call_delay` (3983) | storage / constants of the same names | no named reads † (`activity_cutoff_factor` exists as a hyperparameter name, `$SDK11/hyperparams.py:198`) | G |
| `is_in_admin_freeze_window` (4517) | computed from tempo/pending/last-epoch/freeze window (`$BT/utils/epoch_schedule.py`) | not in v11 † | G (pure function worth porting) |
| `get_ema_tao_inflow` (2287) / `get_all_ema_tao_inflow` (1156) | `SubnetEmaTaoFlow` | `subnets.subnet_tao_flows` † | C |
| `get_subnet_owner_hotkey` (4092) | `SubnetOwnerHotkey` | metagraph `owner_hotkey` † | C |
| `get_subnet_validator_permits` (4208) | `ValidatorPermit` | metagraph per-neuron `validator_permit` / `mg.validators` † | C |
| `get_stake_weight` (3942) | `StakeWeight` (u16 normalized) | no named read † | G |
| `get_liquidity_list` (2484) | returns `[]` always | removed (M:288) | X |

**Neurons / registration / commitments**

| v10 (line) | target | v11 | rtao |
|---|---|---|---|
| `neurons` (4843) / `neurons_lite` (4871) | runtime `NeuronInfoRuntimeApi.get_neurons(_lite)` | `neurons.all(netuid, lite=)` (M:256); read `neurons` | **A** |
| `neuron_for_uid` (4809) | runtime `get_neuron` | `subnets.metagraph(netuid).neuron(uid)` (M:257) | C |
| `get_uid_for_hotkey_on_subnet` (4406) | `Uids` | `neurons.uid` (M:258) | **A** |
| `get_neuron_for_pubkey_and_subnet` (2973) | `Uids` then neuron | compose † | C |
| `is_hotkey_registered` (4582) / `_any` (4612) / `_on_subnet` (4630) | via `get_netuids_for_hotkey` / `get_uid_for_hotkey_on_subnet` | `neurons.uid` / `neurons.netuids_for_hotkey` (M:259) | **A** (template ×4) |
| `get_netuids_for_hotkey` (2911) | `IsNetworkMember` map | `neurons.netuids_for_hotkey` (M:260) | C |
| `get_hotkey_owner` (2386) / `does_hotkey_exist` (1022) | `Owner` | `neurons.hotkey_owner` (M:261) | C |
| `get_owned_hotkeys` (3036) | `OwnedHotkeys` | `neurons.owned_hotkeys` (M:262) | C |
| `get_commitment` (1863) / `get_commitment_metadata` (1900) | `Commitments.CommitmentOf` | `identity.commitment` (M:267) | C |
| `get_all_commitments` (1119) | `CommitmentOf` map | `subnets.commitments` (M:268); read `commitments` | C |
| `get_revealed_commitment` (3374) / `_by_hotkey` (3414) / `get_all_revealed_commitments` (1254) | `RevealedCommitments` | `identity.revealed_commitment` (M:269) | C |
| `get_last_bonds_reset` (2414) / `get_last_commitment_bonds_reset_block` (2440) | `LastBondsReset` | no named read † | G |
| `get_neuron_certificate` (2942) / `get_all_neuron_certificates` (1224) | `NeuronCertificates` | "no named read — generic storage query" (M:270) | G |
| `get_metagraph_info` (2643) | runtime `get_metagraph` / `get_selective_metagraph` / `get_mechagraph` (mechid>0) | `subnets.metagraph` (M:290-306); raw via `client.runtime(bt.runtime_api.SubnetInfoRuntimeApi.get_metagraph,[netuid])` (M:321-325) | C |
| `get_all_metagraphs_info` (1194) | runtime `get_all_metagraphs` / `get_all_mechagraphs` | no named read † (loop over `subnets.all()`+metagraph) | G |
| `weights` (5103) / `bonds` (927) | `Weights` / `Bonds` DMAP `(NetUidStorageIndex, uid) → Vec<(u16,u16)>` | `weights.weights` / `weights.bonds` now `dict[uid, dict[uid, float]]` 0..1 (M:248-249) | **A** (keep a raw-u16 variant: validators compare matrices) |
| `get_timelocked_weight_commits` (4228) | `TimelockedWeightCommits` | `weights.timelocked_weight_commits` (M:250) | C |

**Staking / delegation / locks / root**

| v10 (line) | target | v11 | rtao |
|---|---|---|---|
| `get_stake` (3653) / `get_stake_for_coldkey_and_hotkey` (3681) | runtime `StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid` | `staking.get` / `staking.stake` (M:213,216) | **A** |
| `get_stake_info_for_coldkey(s)` (3718/3742) | runtime `get_stake_info_for_coldkey(s)` | `staking.stake_for_coldkey(s)` (M:214-215) | C |
| `get_stake_for_hotkey` (3767) | `TotalHotkeyAlpha` | no named read † (`total_alpha_staked` is per subnet) | G |
| `get_stake_availability_for_coldkeys` (3808) | runtime `get_stake_availability_for_coldkeys` | `staking.stake_availability_for_coldkey` † | C |
| `get_staking_hotkeys` (3962) | `StakingHotkeys` | `staking.staking_hotkeys` (M:217) | C |
| `get_auto_stakes` (1323) | `AutoStakeDestination` | `staking.auto_stake_all` (M:218) | C |
| `get_stake_add_fee` (3785) / `get_unstake_fee` (4348) / `get_stake_movement_fee` (3914) | `sim_swap` fee fields | `prices.quote_stake` / `quote_unstake` (M:219-220) | C |
| `get_stake_lock` (3848) / `get_coldkey_lock` (1598) / `get_stake_locks` (3880) | `Lock` storage; runtime `get_coldkey_lock` | `locks.coldkey_lock` / `locks.locks_for_coldkey` (M:223-224) | C |
| `get_hotkey_conviction` (2359) / `get_most_convicted_hotkey_on_subnet` (2889) | runtime `get_hotkey_conviction` / `get_most_convicted_hotkey_on_subnet` | `locks.hotkey_conviction` / `most_convicted_hotkey` (M:225-226) | C |
| `is_perpetual_lock` (4652) | `DecayingLock` | `locks.coldkey_lock(...)` fields † | C |
| `get_minimum_required_stake` (2866) | `NominatorMinRequiredStake` | no named read † | G |
| `get_delegates` (2232) / `get_delegate_by_hotkey` (2113) / `is_hotkey_delegate` (4564) / `get_delegate_take` (2176) / `get_delegated` (2199) | runtime `DelegateInfoRuntimeApi.*`; `Delegates` | same names on `client.delegation` (M:263) | C |
| `get_children` (1501) / `get_children_pending` (1549) / `get_parents` (3076) | `ChildKeys` / `PendingChildKeys` / `ParentKeys` | `delegation.children` / `pending_children` / `parents` (M:265) | C |
| `get_root_claim_type` (3449) | `RootClaimType` | removed (M:287) | X |
| `get_root_alpha_dividends_per_subnet` (3485), `get_root_claimable_rate` (3512), `get_root_claimable_all_rates` (3543), `get_root_claimable_stake` (3572), `get_root_claimed` (3623) | `RootAlphaDividendsPerSubnet`, `RootClaimable`, `RootClaimed` | `staking.root_basket_owed` and other basket reads (M:287) | C |
| `get_crowdloans` (2072), `get_crowdloan_by_id` (2013), `get_crowdloan_contributions` (1975), `get_crowdloan_next_id` (2045), `get_crowdloan_constants` (1928) | `Crowdloan.Crowdloans` / `Contributions` / `NextCrowdloanId` / constants | `leasing.crowdloans` / `crowdloan` / `crowdloan_contributors` (M:284); next-id/constants → generic † | C / G |

### 1.4 Extrinsics (sync line) → v11 intent → rtao

All v10 extrinsic methods share trailing kwargs `wait_for_inclusion`, `wait_for_finalization`, `period`, `raise_error`, `mev_protection` (e.g. `set_weights`, `$BT/core/subtensor.py:8154-8172`); v11 moves those to `execute(...)` and `result.raise_for_failure()` (M:331-338).

| v10 (line) | v10 extrinsic fn → pallet call | v11 intent (`python_class`) | rtao |
|---|---|---|---|
| `transfer` (8656) | `transfer_extrinsic` → `Balances.transfer_keep_alive/allow_death/all` | `Transfer` / `TransferAll` (M:343-344) | **A** |
| `add_stake` (5418) | `add_stake_extrinsic` → `add_stake` / `add_stake_limit` | `AddStake` / `AddStakeLimit` (M:345-346) | **A** |
| `add_stake_burn` (5493) | `add_stake_burn_extrinsic` → `add_stake_burn` | `StakeBurn` † | C |
| `add_stake_multiple` (5551) / `unstake_multiple` (9027) | batch | `Batch(intents=[...])` (M:350) | C |
| `unstake` (8868) | `unstake_extrinsic` → `remove_stake(_limit)` | `RemoveStake` / `RemoveStakeLimit` (M:347-348) | **A** |
| `unstake_all` (8941) | `unstake_all_extrinsic` | `UnstakeAll` / `UnstakeAllAlpha` (M:349) | C |
| `move_stake` (6763) / `swap_stake` (8530) / `transfer_stake` (8763) | → `move_stake(_limit)`, `swap_stake(_limit)`, `transfer_stake(_and_hotkey)` | `MoveStake` / `SwapStake` / `TransferStake` (M:351-353) | C |
| `set_auto_stake` (7771) | → `set_coldkey_auto_stake_hotkey` | `SetAutoStake` (M:354) | C |
| `lock_stake` (6527) / `move_lock` (6720) / `set_perpetual_lock` (7952) | → same-name calls | `LockStake` / `MoveLock` / `SetPerpetualLock` (M:355) | C |
| `burned_register` (5837) | `burned_register_extrinsic` (root → `root_register_extrinsic`) | `BurnedRegister` (M:356) | **A** |
| `register` (7106) / `register_limit` (7184) | `register_limit_extrinsic` → `SubtensorModule.register_limit` (burn with max price); **no PoW path in v10** (`$BT/core/extrinsics/registration.py:21-384` has only burned/limit/subnet/identity) | `BurnedRegister`; PoW via `PowRegister` + `client.mine_pow_registration` (`$SDK11/client.py` method list) | C |
| `register_subnet` (7240) / `root_register` (7587) / `start_call` (8485) | → `register_network` / `root_register` / `start_call` | `RegisterSubnet` / `RootRegister` / `StartCall` (M:357-359) | C |
| `serve_axon` (8303) | `serve_axon_extrinsic` → `serve_axon`; ip encoded via `ip_to_int`, `ip_version` (`$BT/core/extrinsics/serving.py`) | `ServeAxon(netuid, ip, port)` / `ServeAxonTls` (M:360); fields `ip:str, port:int, protocol=4, version=1` (`$SDK11/intents/serving.py`) | **A** |
| `set_weights` (8154) | commit-reveal auto-select, retry loop gated by `blocks_since_last_update > weights_rate_limit` (8220-8224), `version_key=version_as_int` default (8164) | `SetWeights` (M:361); `version_key` default 0 (M:402-404) | **A** |
| `commit_weights` (5949) / `reveal_weights` (7501) | `commit_weights_extrinsic` (salt hash) / `reveal_weights_extrinsic` → `commit_mechanism_weights` / `reveal_mechanism_weights` | `CommitWeights` / `RevealWeights` (M:362) | C |
| `set_delegate_take` (7874) | `set_take_extrinsic` → `increase_take` / `decrease_take` | `SetTake` (unit change, M:363, 405-409) | C |
| `set_children` (7821) | `set_children_extrinsic` | `SetChildren` (unit change, M:364, 410-412) | C |
| `set_subnet_identity` (8052) | → `set_subnet_identity` | `SetSubnetIdentity` (M:365) | C |
| `claim_root` (5898) | → `claim_root` | `ClaimRoot` (kept) / prefer `ClaimRootWithHotkey` (M:367) | C |
| `set_root_claim_type` (7991) | → removed call | removed (M:368) | X |
| `add_proxy` (5665), `remove_proxy` (7443), `remove_proxies` (7393), `create_pure_proxy` (6167), `kill_pure_proxy` (6439) | `Proxy.*` | same-name intents (M:369) | C |
| `proxy` (6879) | `Proxy.proxy` wrapper | `client.execute(intent, wallet, proxy_for=real_ss58)` (M:370) | C |
| `announce_proxy` (5781), `proxy_announced` (6938), `reject_proxy_announcement` (7052), `remove_proxy_announcement` (7284), `poke_deposit` (6827) | `Proxy.announce / proxy_announced / reject_announcement / remove_announcement / poke_deposit` | `ExecuteProxyAnnounced` only (`$CAT/intents.json`); rest via `submit_call` † | G |
| `announce_coldkey_swap` (5725), `swap_coldkey_announced` (8710), `dispute_coldkey_swap` (6237), `clear_coldkey_swap_announcement` (6281) | `SubtensorModule.*` | same-name intents (M:371) | C |
| `create_crowdloan` (6099), `contribute_crowdloan` (6043), `withdraw_crowdloan` (9258), `finalize_crowdloan` (6379), `refund_crowdloan` (7000), `dissolve_crowdloan` (6325), `update_cap_crowdloan` (9088), `update_end_crowdloan` (9144), `update_min_contribution_crowdloan` (9202) | `Crowdloan.*` | same-name intents (M:372) | C |
| `set_commitment` (8359) / `set_reveal_commitment` (8420) | `publish_metadata_extrinsic` → `Commitments.set_commitment` | no intent — `submit_call(bt.calls.Commitments.set_commitment(...))` (M:373) | G |
| `set_tempo` (8107), `set_activity_cutoff_factor` (7724), `root_set_activity_cutoff_factor` (7632), `root_set_pending_childkey_cooldown` (7679) | `SubtensorModule.set_tempo` (now deprecated no-op, M:600-605), etc. | `SetHyperparameter(name="tempo"/"activity_cutoff_factor")` via `AdminUtils` (M:374, 600-608) | C |
| `trigger_epoch` (8824) | → `trigger_epoch`? (v10 `tempo_control.py`) | not in `$CAT/intents.json`; `submit_call` † | G |
| `add_liquidity` (5606), `modify_liquidity` (6643), `remove_liquidity` (7339), `toggle_user_liquidity` (8608) | `Swap.*` | removed (M:377) | X |
| `mev_submit_encrypted` (6574) | `submit_encrypted_extrinsic` → `MevShield.submit_encrypted` | `client.submit_shielded(intent, wallet)`; default on stake intents (M:378) | C |

### 1.5 Which of these are "in the wild" (evidence)

Call-site counts (`grep -ho` over `*.py`, see my command log):

- `$TPL` (archived template; identical copy at `upstream/RaoFoundation_bittensor-subnet-template`): `metagraph.axons`×12, `metagraph.n`×11, `metagraph.hotkeys`×11, `.add_args(`×10, `bt.Synapse`×8, `metagraph.S`×6, `subtensor.get_current_block(`×5, `subtensor.metagraph(`×4, `subtensor.is_hotkey_registered(`×4, `process_weights_for_netuid`×4, `metagraph.validator_permit`×3, `metagraph.uids`×3, `metagraph.last_update`×3, `convert_weights_and_uids_for_emit`×3, `axon.start/serve/attach`×2 each, and ×1 each of `subtensor.subnet_exists`, `set_weights`, `serve_axon`, `neurons`, `min_allowed_weights`, `max_weight_limit`, `metagraph.T/R/I/E/C/block`.
- `upstream/prompting` (SN1): `metagraph.uids`×2, `metagraph.sync`×2, `weight_utils.process_weights_for_netuid`×1, `subtensor.set_weights(`×1, `metagraph.weights/validator_permit/stake/hotkeys`×1.
- `upstream/unconst_Chi`: `metagraph.sync`×2, `metagraph.hotkeys`×2, `subtensor.set_weights`×1, `get_subnet_hyperparameters`×1, `get_current_block`×1, `metagraph.n/block`×1.

So the **A** set is: `metagraph` (+ array properties `S, I, E, C, Tv, D, W, B, TS, AS, hotkeys, coldkeys, axons, uids, n, block, validator_permit, last_update`, `$BT/core/metagraph.py:289-490, 705-760`), `get_current_block`, `get_block_hash`, `is_hotkey_registered*`, `subnet_exists`, `get_uid_for_hotkey_on_subnet`, `neurons`/`neurons_lite`, `min_allowed_weights`, `max_weight_limit`, `blocks_since_last_update`, `tempo`, `subnetwork_n`, `get_subnet_hyperparameters`, `get_balance`, `get_stake`, `weights`/`bonds`, `set_weights`, `serve_axon`, `burned_register`, `add_stake`, `unstake`, `transfer`. Everything else is C/G/X.

### 1.6 Sync vs async parity (for rtao API shape)

- v10 maintained two hand-written classes with identical method lists (235 vs 236, §1.0) and a parallel `extrinsics/asyncex/` tree (20 modules: `children, coldkey_swap, crowdloan, liquidity, lock, mev_shield, move_stake, proxy, registration, root, serving, staking, start_call, sudo, take, tempo_control, transfer, unstaking, utils, weights` — `ls $BT/core/extrinsics/asyncex`).
- v11 collapsed to one `Subtensor` "blocking when used directly and async when awaited" (M:107-116) backed by `SyncClient` (`$SDK11/sync.py`) and a one-call `bt.set_weights(netuid, weights, wallet=, hotkey=, mechid=0, version_key=0, network=, retries=2)` (`$SDK11/_subtensor.py` `def set_weights`; `$DOCS/guides/validating.mdx` "Set weights").
- rtao being tokio-native avoids the duplication; a blocking facade (if any) should be a thin `block_on` wrapper, not a second surface.

---

## 2. v10 weight_utils semantics and Balance/rao/tao conventions

### 2.1 `normalize_max_weight(x, limit=0.1)` (`$BT/utils/weight_utils.py:25-71`)

Algorithm (numpy, `@legacy_torch_api_compat`):
1. `epsilon = 1e-7`; if `x.sum() == 0` or `x.shape[0] * limit <= 1` → return uniform `ones/n` (L43-44).
2. `estimation = sorted(x)/sum`; if `estimation.max() <= limit` → return `x/sum` (L46-49).
3. `cumsum = cumsum(estimation)`; `estimation_sum[i] = (n-i-1)*estimation[i]`; `n_values = count(estimation/(estimation_sum + cumsum + eps) < limit)` (L52-58).
4. `cutoff_scale = (limit*cumsum[n_values-1] - eps) / (1 - limit*(n - n_values))`; `cutoff = cutoff_scale * sum(x)`; clip `x[x>cutoff]=cutoff`; return `x/sum` (L61-71).

v11 `clip_to_max_weight(weights, limit)` (`$SDK11/intents/weights.py:83-114`) is a line-for-line port of steps 1–4 (same `epsilon`, same `n_values` and `cutoff_scale` formulas), except all-zero input **raises** instead of returning uniform (L96-97). Mass is redistributed, not discarded (docstring L87-90).

### 2.2 `process_weights_for_netuid` / `process_weights` (`$BT/utils/weight_utils.py:214-384`)

- `process_weights_for_netuid(uids, weights, netuid, subtensor, metagraph=None, exclude_quantile=0)` fetches `metagraph` if `None` (L251), then calls `process_weights` with `num_neurons=metagraph.n`, `min_allowed_weights=subtensor.min_allowed_weights(netuid)`, `max_weight_limit=subtensor.max_weight_limit(netuid)` (L253-260). Comment at L213: "The community uses / bittensor does not".
- `process_weights` branches (L308-384):
  - `quantile = exclude_quantile / U16_MAX` (L303).
  - non-zero mask; if **no non-zero weights or `num_neurons < min_allowed_weights`** → return `(arange(n), ones(n)/n)` — i.e. **uniform weights over every UID** with a warning (L317-334).
  - elif **`nzw_size < min_allowed_weights`** → `weights = ones(n)*1e-5; weights[nz] += nz_weights`; then `normalize_max_weight(limit=max_weight_limit)` over **all n** uids (L336-354).
  - else: `max_exclude = max(0, nz - min_allowed)/nz`; `exclude_quantile = min(quantile, max_exclude)`; drop non-zero weights **below** the quantile (`lowest_quantile <= w` kept) (L359-376); then `normalize_max_weight(x=non_zero_weights, limit=max_weight_limit)` and return `(non_zero_uids, normalized)` (L379-384).
- Note `max_weight_limit` here is the v10 float `MaxWeightsLimit/65535` (`$BT/core/subtensor.py:4725-4736`, via `u16_normalized_float`, `$BT/utils/__init__.py:211-212`).

### 2.3 `convert_weights_and_uids_for_emit(uids, weights)` (`$BT/utils/weight_utils.py:165-210`)

- Rejects negative weights/uids and length mismatch (L182-189); all-zero → `([], [])` (L190-191).
- **Max-upscale**: `w / max(w)` (L193-196); `uint16 = round(w * 65535)` (L201-203); **zeros dropped** (L206-208). Result is *not* sum-normalized (max element = 65535) — the chain normalizes.
- `convert_and_normalize_weights_and_uids` (L460-475) = list→ndarray (`int64`/`float32`, L440-457) then the above; used by `set_weights_extrinsic`, `reveal_weights_extrinsic`, `commit_timelocked_weights_extrinsic` (`$BT/core/extrinsics/weights.py:79, 310, 409`).
- Inverse: `convert_weight_uids_and_vals_to_tensor(n, uids, weights)` → dense row normalized to sum 1 (L74-101); bonds variant keeps raw ints (L142-162); root variant maps uid→subnet index (L104-138).

v11 `normalize(uids, weights)` (`$SDK11/intents/weights.py:54-81`) is the same: finite/non-negative/length checks, `U16` uid range check, `round((w/top)*65535)`, drop zeros, all-zero → `([], [])`.

### 2.4 Chain-side rules and the v11 `_conform` pipeline (what rtao should treat as canonical)

- Chain: on submission the runtime max-upscales u16 values (`vec_u16_max_upscale_to_u16`), sum-normalizes, and rejects with `MaxWeightExceeded` if `max/sum > max_weights_limit/65535`; a single self-weight is exempt; the enforced limit is currently a constant `u16::MAX` so the check always passes, but `MaxWeightsLimit` storage still exists and the SDK proactively clips to it (`$CAT/bittensor.com_llms-full.txt:11734-11765`, hyperparameter page). Storage defaults from metadata: `MaxWeightsLimit` u16 default `[255,255]`=65535, `MinAllowedWeights` u16 default `[0,4]`=1024, `WeightsSetRateLimit` u64 default 100, `WeightsVersionKey` u64 default 0, `CommitRevealWeightsEnabled` bool default true (`$META/explore/_storage_table.tsv`, rows `MaxWeightsLimit`, `MinAllowedWeights`, `WeightsSetRateLimit`, `WeightsVersionKey`, `CommitRevealWeightsEnabled`).
- `min_allowed_weights` semantics: "checks `weights.len() >= MinAllowedWeights`" (`$BT/core/subtensor.py:4783-4808` docstring); the v11 read says a pure self-weight bypasses it and the owner bypasses permit rules (`$CAT/reads.json` entry `min_allowed_weights`).
- Wire call: `SubtensorModule.set_mechanism_weights { netuid: NetUid(u16), mecid: MechId(u8), dests: Vec<u16>, weights: Vec<u16>, version_key: u64 }` (`subxt explore --file=$META/finney.scale pallet SubtensorModule calls set_mechanism_weights`). Related calls present in finney: `set_weights`, `commit_weights`, `commit_mechanism_weights`, `commit_timelocked_weights`, `commit_timelocked_mechanism_weights`, `commit_crv3_mechanism_weights`, `reveal_weights`, `reveal_mechanism_weights`, `batch_reveal_weights` (`$META/explore/SubtensorModule_calls.txt`).
- `Weights`/`Bonds` storage are `DMAP (NetUidStorageIndex, u16) → Vec<(u16,u16)>` and `LastUpdate` is `MAP NetUidStorageIndex → Vec<u64>` (`$META/explore/_storage_table.tsv`); `NetUidStorageIndex = mechid * 4096 + netuid` (`$BT/utils/__init__.py:67-77`, `GLOBAL_MAX_SUBNET_COUNT = 4096` at L45; same constant in `$SDK11/settings.py:31`).
- v11 `_conform(uids, weights, preflight, netuid, raw_u16)` (`$SDK11/intents/weights.py:218-258`): `is_self_weight = uids == [preflight.uid]`; non-raw path: if not self-weight and `max_weight_limit < U16_MAX` → `clip_to_max_weight(floats, limit/65535)`; then `normalize`; error if nothing non-zero; if not self-weight and `len < min_allowed_weights` → `ChainError(code=INVALID_ARGUMENT)`. `raw_u16=True` requires `SubnetEpochConsensus == Null` (`_ensure_raw_null`, L260-265) and integer weights in 0..65535 that already satisfy the limit (L228-243).
- v10 `set_weights` orchestration (`$BT/core/subtensor.py:8154-8302`): checks UID registration; if `commit_reveal_enabled(netuid)` → loop `commit_timelocked_weights_extrinsic` up to `max_attempts=5` while `blocks_since_last_update > weights_rate_limit`; else `set_weights_extrinsic` with the same gate; `commit_reveal_version=4`, `block_time=12.0`. The timelocked path computes `(commit, reveal_round)` with `bittensor_drand.get_encrypted_commit_v2(... last_epoch_block, pending_epoch_at, subnet_epoch_index, subnet_reveal_period_epochs, block_time)` then calls `commit_timelocked_mechanism_weights(reveal_round, commit_reveal_version)` (`$BT/core/extrinsics/weights.py:5, 55-76`). Legacy salt commits hash `blake2b_256(pubkey ‖ netuid_le16 ‖ Vec<u16> uids ‖ Vec<u16> values ‖ Vec<u16> salt ‖ version_key_le64)` (`generate_weight_hash`, `$BT/utils/weight_utils.py:387-437`).
- `convert_maybe_split_to_u16` (L478-514): normalize to sum 1 → `round(x*65535)` → fix rounding so the sum is exactly 65535 (used for mechanism emission splits).

**Recommendation (rtao `weights` module):** implement v11's `_conform` as the canonical `conform(uids, weights, &SubnetWeightRules) -> Result<(Vec<u16>, Vec<u16>)>` with `rules {uid, min_allowed_weights, max_weight_limit_u16, epoch_consensus}`; provide `normalize_max_weight` and `max_upscale_u16` as pure functions with golden tests against the Python formulas; offer the v10 `process_weights` (`exclude_quantile`, uniform/1e-5 fallbacks) only behind an explicit `legacy` function because its "all ones when below min" behaviour silently rewards every UID (L317-334). Keep `version_key` default 0 (v11) but document that v10 subnets used `version_as_int` (`$BT/core/settings.py:157-168`: `major*1000² + minor*1000 + patch`) and the template used `__spec_version__ = 1000*major + 10*minor + patch` (`$TPL/template/__init__.py:22-28`).

### 2.5 Subnet-template scoring conventions (what validators' code assumes)

- Scores are a float vector indexed by UID; `update_scores` scatters rewards by uid and applies EMA `scores = alpha*scattered + (1-alpha)*scores` with `--neuron.moving_average_alpha` default 0.1 (`$TPL/template/base/validator.py:319-364`; default at `$TPL/template/utils/config.py:212-215`).
- `set_weights`: L1-normalize scores (`np.linalg.norm(ord=1)`), then `process_weights_for_netuid(uids=metagraph.uids, weights=raw, netuid, subtensor, metagraph)`, then `convert_weights_and_uids_for_emit`, then `subtensor.set_weights(..., version_key=self.spec_version, wait_for_inclusion=False, wait_for_finalization=False)` (`$TPL/template/base/validator.py:222-284`).
- `resync_metagraph`: zero the score of any UID whose hotkey changed; grow/shrink the scores vector to `metagraph.n` (`$TPL/template/base/validator.py:286-318`).
- State: `np.savez(full_path + "/state.npz", step=, scores=, hotkeys=)` and `np.load` of the same keys (`$TPL/template/base/validator.py:366-385`).
- `should_set_weights` / `should_sync_metagraph` gate on `--neuron.epoch_length` (default 100 blocks) (`$TPL/template/base/neuron.py:153-167`; default at `$TPL/template/utils/config.py:85-88`).

### 2.6 Balance, rao, tao, alpha — conventions

v10 (`$BT/utils/balance.py`):
- `Balance(balance)`: `int` → rao, `float` → tao (`rao = int(balance * 1e9)`), else `TypeError` (L88-103). `.tao = rao / 1e9` (L106-108); `int()`→rao, `float()`→tao (L110-116).
- `from_tao(amount, netuid=0)`, `from_float`, `from_rao(amount, netuid=0)` (L306-347); module helpers `tao(amount, netuid=0)`, `rao(amount, netuid=0)` (L814-825); `check_balance_amount` enforces "must be a Balance" (`BalanceTypeError`, L828-855).
- Unit: `netuid` attribute, `unit`/`rao_unit` symbol from `UNITS[netuid]` (`set_unit`, `get_unit` with base-438 digit composition for netuid ≥ len(UNITS), L350-366); `UNITS` has **438 entries** (τ for 0, then α, β, γ … up to index 437; L369-811; count computed by awk over the list). `settings.TAO_SYMBOL = "τ"`, `RAO_SYMBOL = "ρ"` (`$BT/core/settings.py:55-56`).
- Arithmetic/comparison between two `Balance`s with different `netuid` raises `BalanceUnitMismatchError` (`_check_currencies`, L12-36; used in `__eq__`, `__gt__`, `__add__`, … L140-303); mixing with bare `int` treats the int as rao (L198-207).
- `__str__` prints 9 decimals with the symbol; non-TAO units use LRM marks `‎` (L118-125).

v11 (`$SDK11/balance.py`, `$DOCS/concepts/money.mdx`, `$DOCS/migration.mdx:414-432`):
- `Balance(rao: int, netuid=0, symbol=None)`; `from_rao`, `from_tao(float|str|Decimal)` (**no netuid**), `from_alpha(amount, netuid)`, `from_amount` (L47-90); exact `Decimal` path (L84-90, L119-125); `RAO_PER_TAO = 10**9` (`$SDK11/settings.py:15`).
- `.tao` only on TAO balances, `.alpha` only on alpha, `.amount` either (L92-116); `.tao` on alpha raises (M:422-424).
- `__eq__` never raises (different units are unequal), `__hash__ = (rao, netuid)`; ordering/arithmetic go through `_rao_of` which rejects `bool` and floats (L154-210); `UnitMismatchError(TypeError)` (L40).
- Shorthands `bt.tao(1.5)`, `bt.alpha(2.5, 42)`, `bt.rao(n)` (L211-225). Intent money fields are unit-named (`amount_tao`, `amount_alpha`, `limit_price_rao`) and accept `int|float|str|Decimal|Balance` and `"all"` (M:426-432; `$CAT/intents.json` `amount_alpha` schema `anyOf [number, decimal-string, "all"]`).

**Recommendation:** rtao `Balance` = `struct { rao: u64, unit: Unit }` with `Unit::Tao | Unit::Alpha(NetUid)` (or `netuid: u16` with 0=TAO, matching both SDKs), `Add/Sub` only within the same unit (type-level: `TaoBalance`/`AlphaBalance` newtypes exist in the runtime metadata — `TaoBalance(u64)`, `AlphaBalance(u64)` appear in `get_metagraph` output, `subxt explore ... api SubnetInfoRuntimeApi get_metagraph`), `Display` as `τ1.000000000` / `α₄₂`-style, and a `UNITS` table generated from `$BT/utils/balance.py:369-811` only if rtao wants v10-compatible symbols (v11 prints the chain-registered symbol from `token_symbols`, `$CAT/reads.json` `token_symbols`). Never expose a float→rao constructor without explicit `Decimal`-like parsing (v10's `int(balance * 1e9)` truncates).

---

## 3. v10 config / argparse conventions, env vars, on-disk formats

### 3.1 `bt.Config` mechanics (`$BT/core/config.py`)

- `Config(parser, args=None, strict=False, default=DEFAULTS)`: starts from `settings.DEFAULTS` converted to a nested dict (L128-147); if `BT_NO_PARSE_CLI_ARGS` says parse (default true; `no_parse_cli`, `$BT/core/settings.py:172-178`) it adds `--config`, `--strict`, `--no_version_checking` (L295-329), parses `sys.argv[1:]` with `parse_known_args` (L237-253), loads `--config <yaml>` via `parser.set_defaults(**yaml)` (L199-207), and **builds a nested tree by splitting argument names on "."** (L209-218). `config.is_set("subtensor.network")` reports explicitly-passed params (L220-235, 287-289). `merge`, `to_dict`, YAML `__str__` (L170-173, 266-293).
- Each component contributes args through `add_args(parser, prefix=None)`: `Subtensor.add_args` adds `--subtensor.network` (default `DEFAULTS.subtensor.network`), `--subtensor.chain_endpoint`, `--subtensor._mock` (`$BT/core/types.py:134-188`); precedence in `setup_config`: explicit network arg > `is_set(chain_endpoint)` > `is_set(network)` > configured chain_endpoint > configured network (`$BT/core/types.py:80-124`).
- `Axon.add_args`: `--axon.port` (8091), `--axon.ip` (`[::]`), `--axon.external_port`, `--axon.external_ip`, `--axon.max_workers` (10) (`$BT/core/axon.py:627-677`; defaults from `$BT/core/settings.py:121-126`).
- `logging.add_args`: `--logging.debug`, `--logging.trace`, `--logging.info`, `--logging.record_log`, `--logging.logging_dir` (default `~/.bittensor/miners`), `--logging.enable_third_party_loggers` (`$BT/utils/btlogging/loggingmachine.py:638-677`; defaults `$BT/core/settings.py:128-140`).
- Wallet args came from the separate `bittensor_wallet` package: `--wallet.name` (default "default"), `--wallet.hotkey` ("default"), `--wallet.path` (`~/.bittensor/wallets`) with env defaults `BT_WALLET_NAME/HOTKEY/PATH` (`$BT/core/settings.py:151-154`).
- Template neuron args (`$TPL/template/utils/config.py:70-248`): `--netuid` (1), `--neuron.device`, `--neuron.epoch_length` (100), `--mock`, `--neuron.events_retention_size` (2 GiB), `--neuron.dont_save_events`, `--wandb.off/offline/notes/project_name/entity`; miner: `--neuron.name` ("miner"), `--blacklist.force_validator_permit`, `--blacklist.allow_non_registered`; validator: `--neuron.name` ("validator"), `--neuron.timeout` (10), `--neuron.num_concurrent_forwards` (1), `--neuron.sample_size` (50), `--neuron.disable_set_weights`, `--neuron.moving_average_alpha` (0.1), `--neuron.axon_off`/`--axon_off`, `--neuron.vpermit_tao_limit` (4096). `check_config` derives `neuron.full_path = ~/.bittensor/miners/<wallet.name>/<wallet.hotkey>/netuid<netuid>/<neuron.name>` (`$TPL/template/utils/config.py:44-60`).

### 3.2 Environment variables (v10 → v11)

| v10 (`$BT/core/settings.py`) | v11 (`$SDK11` grep of `"BT_*"` literals; `$DOCS/migration.mdx:164-176`) |
|---|---|
| `BT_SUBTENSOR_NETWORK` (L148), `BT_SUBTENSOR_CHAIN_ENDPOINT` (L32, L147) | `BT_NETWORK`; `BT_CHAIN_ENDPOINT` overrides `local` only (`$SDK11/settings.py:38`) |
| `BT_WALLET_NAME`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH` (L152-154) | `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`; new `BT_WALLET_PASSWORD`, `BT_WALLET_PASSWORD_FILE` |
| `BT_AXON_PORT/IP/EXTERNAL_PORT/EXTERNAL_IP/MAX_WORKERS` (L110-126), `BT_PRIORITY_MAX_WORKERS/MAXSIZE` (L112-144), `BT_LOGGING_*` (L129-140), `BT_MEV_PROTECTION` (L77), `BT_NO_PARSE_CLI_ARGS` (L173), `READ_ONLY` (L8) | gone |
| — | `BT_FALLBACK_ENDPOINTS`, `BT_ARCHIVE_ENDPOINTS`, `BT_RETRY_FOREVER`, `BT_EVM_ENDPOINT`, `BT_SIGNER_ADDRESS`, `BT_EXTENSION_BROWSER/BRIDGE`, `BTCLI_CONFIG` (`~/.bittensor/btcli.json`), `BTCLI_ADDRESSES_PATH`, `BTCLI_PROXIES_PATH`, `BTCLI_MULTISIGS_PATH`, `BTCLI_*_CACHE` |

Network names/endpoints: v10 `NETWORKS = [finney, test, archive, local, latent-lite]` → `wss://entrypoint-finney.opentensor.ai:443`, `wss://test.finney.opentensor.ai:443`, `wss://archive.chain.opentensor.ai:443`, `ws://127.0.0.1:9944`, `wss://lite.sub.latent.to:443` (`$BT/core/settings.py:26-52`); `determine_chain_endpoint_and_network` also reverse-maps endpoint substrings and returns `("unknown", url)` otherwise (`$BT/utils/__init__.py:458-480`). v11 `NETWORKS = {finney, test, archive, devnet (wss://dev.chain.opentensor.ai:443), local}` plus `FALLBACK_ENDPOINTS` (`lite.chain.opentensor.ai`, `lite.sub.latent.to`) and `ARCHIVE_ENDPOINTS` used automatically when a read hits pruned state (`$SDK11/settings.py:33-80`). v9 btcli additionally knew `rao`, `dev` (`$BTCLI/src/__init__.py:14-21`), dropped in v11 (M:154-156).

### 3.3 Metagraph save/load formats (`$BT/core/metagraph.py`)

- Directory: `get_save_dir(network, netuid, root_dir=["~", ".bittensor", "metagraphs"])` → `~/.bittensor/metagraphs/network-<network>/netuid-<netuid>/` (L94-116); file `block-<block>.pt` (L793, L800); `latest_block_path` picks the max `block-N` by parsing the filename (L119-143).
- Content: `state_dict()` = `{netuid, network, version, n, block, consensus, validator_trust, incentive, emission, dividends, active, last_update, validator_permit, weights, bonds, uids, axons, neurons, alpha_stake, tao_stake, stake}` (L595-617). With torch: `torch.save` (L793-798); without torch: **`pickle.dump`** of the dict of numpy arrays (L800-803). Load tries `pickle.load` first and falls back to `torch.load` under `safe_globals()` (L1218-1239); `METAGRAPH_STATE_DICT_NDARRAY_KEYS` lists the array keys (L50-69).
- The `.npz` format is **not** used by the metagraph; it is used by the template validator's `state.npz` (§2.5). v11 removed `save()/load()` ("persist `mg.raw` yourself", M:316).
- Array semantics: `uids,n,block,active,last_update` int64; `consensus,incentive,dividends,emission,validator_trust` float32; `validator_permit` bool (L705-756); `S=stake`, `I=incentive`, `E=emission`, `C=consensus`, `Tv=validator_trust`, `D=dividends`, `B=bonds`, `W=weights`, `TS=tao_stake`, `AS=alpha_stake`; `hotkeys/coldkeys/addresses` derived from `axons` (L289-490). `lite=True` skips weights/bonds (`sync`, L1638-1722).

**Recommendation:** do not read `.pt` (pickle/torch, Python-specific and unsafe). Optionally read/write `state.npz` (`npz` = zip of `.npy`) so a Rust validator can inherit a Python validator's `scores` — decision for architects (§7). rtao's own metagraph snapshot should be SCALE (the raw `get_metagraph` bytes + block hash) or a versioned serde format.

### 3.4 A Rust precedent for flag naming (SN2)

`$SN2/crates/sn2-validator/src/cli.rs:6-25` and `$SN2/crates/sn2-miner/src/cli.rs:21-49`: clap `--netuid`, `--network` (`alias = "subtensor.network"`, default finney), `--subtensor-chain-endpoint` (`alias = "subtensor.chain_endpoint"`), `--wallet-name` (`alias = "wallet.name"`, default "default"), `--wallet-hotkey` (`alias = "wallet.hotkey"`), `--wallet-path` (`alias = "wallet.path"`), `--log-level` (`alias = "logging.level"`), miner `--axon-host` (`alias = "axon.host"`, 0.0.0.0), `--axon-port` (`alias = "axon.port"`, 8091), `--external-ip` (`alias = "axon.external_ip"`, env `BT_AXON_EXTERNAL_IP`). This is the pattern rtao should ship in an `rtao-config` crate: kebab-case primary flags, dotted v10 aliases, v11 env names primary with v10 env names as fallbacks.

---

## 4. v9 btcli vs v11 btcli

### 4.1 v9 (`bittensor-cli` 9.23.2) command tree (`$BTCLI/cli.py:861-1396`)

Groups registered on the root Typer app (`app.add_typer`, L888-1000, 1353-1361) with hidden aliases: `config` (`c`, `conf`), `wallet` (`w`, `wallets`), `stake` (`st`), `sudo` (`su`), `subnets` (`s`, `subnet`), `weights` (`wt`, `weight`; group itself hidden), `view`, `utils`, `axon`, `proxy`, `lock`, `crowd` (`cr`, `crowdloan`). Sub-groups: `stake child|children` (L1139-1153), `subnets mechanisms|mech` (L951-962).

Commands (visible; hidden snake_case twins exist for most dashed names, e.g. `wallet new_coldkey`, L1289-1338):

- `config`: set, get, clear, add-proxy, proxies, remove-proxy, update-proxy, clear-proxies (L1008-1015).
- `wallet`: list, create, new-coldkey, new-hotkey, regen-coldkey, regen-coldkeypub, regen-hotkey, regen-hotkeypub, balance, transfer, overview, inspect [hidden], history [hidden], sign, verify, encrypt, decrypt, swap-hotkey, swap-coldkey, swap-check, set-identity, get-identity, associate-hotkey (L1019-1092, 1285-1327).
- `axon`: set, reset (L1095-1096).
- `stake`: add, remove, list, move, transfer, swap, auto, set-auto, set-claim, process-claim, wizard, locks [hidden]; `child get|set|revoke|take` (L1099-1153).
- `lock`: list, show, add, mode, move (L1156-1160).
- `subnets mechanisms`: count, set, emissions, split-emissions (L1163-1174).
- `sudo`: set, get, set-take, get-take, trim, stake-burn (`buyback` hidden), trigger-epoch, senate, proposals, senate-vote (L1177-1206, 1337-1338).
- `subnets`: list, show (`metagraph` hidden alias), create, register, price, burn-cost, start, check-start, conviction, hyperparameters (→ `sudo_get`), set-identity, get-identity, set-symbol (L1209-1250, 1328-1331).
- `weights`: commit, reveal (L1253-1258).
- `view`: dashboard (L1261-1264).
- `proxy`: create, add, remove, kill, execute (L1266-1283).
- `crowd`: create, contribute, withdraw, finalize, list, info, contributors, update, refund, dissolve (L1363-1392).
- `utils`: convert, latency (L1395-1396).

Global/shared options (`class Options`, `$BTCLI/cli.py:172-460`): wallet name `--wallet-name/--name/--wallet_name/--wallet.name/-w`, hotkey `--wallet-hotkey/--hotkey/--wallet_hotkey/--wallet.hotkey/-H`, path `--wallet-path/--wallet_path/--wallet.path/-p`, network `--network/--subtensor.network/--chain/--subtensor.chain_endpoint` (L280-288), `--netuid`, `--netuids`, `--mechid/--mech-id/--mechanism-id/...`, `--prompt/--no-prompt`, `--json-output/--json-out`, `--quiet`, `--verbose`, `--era`, `--rate-tolerance/--tolerance`, `--safe-staking/--safe/--unsafe`, `--allow-partial-stake/--partial`, `--safe-register/--unsafe-register`, `--mev-protection/--no-mev-protection`, `--proxy`, `--proxy-type`, `--dashboard-path`, `--reuse-last`, `--html`, `--live`, `--uri`, `--crypto-type`. Config file `~/.bittensor/config.yml` with keys `network, wallet_path, wallet_name, wallet_hotkey, use_cache, disk_cache, metagraph_cols{...}`; proxies DB `~/.bittensor/bittensor.db`; dashboard `~/.bittensor/dashboard/` (`$BTCLI/src/__init__.py:38-99`). Hyperparameter table `HYPERPARAMS` maps names → `AdminUtils` sudo call + root-only flag (`$BTCLI/src/__init__.py:582-652`).

### 4.2 v11 btcli (bundled in `bittensor` 11) command tree (`$SDK11/cli/main.py`, `$SDK11/cli/commands/*.py`)

- Primary groups (L99-104): `wallet`, `stake`, `root`, `subnets`, `conviction` (module `lock`), `hparams` (module `sudo`). Accounts panel (L106-109): `addr`, `evm`, `proxy`, `multisig`. Chain/agents panel (L138-140, 321, 405): **`query` (generated from `reads` REGISTRY, one command per read, `name.replace("_","-")`, grouped by `category`** — `$SDK11/cli/query.py:258-263`), **`tx` (generated from the intents REGISTRY, grouped by pallet/privilege** — `$SDK11/cli/tx.py:487-495`), `call` (raw `Pallet.function --args`), `tools` (prints `list_tools()` JSON), `explain <code|ChainErrorName>`. Additional panel (L143-144): `config`, `misc` (which mounts `weights`, `axon`, `collateral`, `timelock`, `crowd`, `extension`, `upgrade`; `$SDK11/cli/commands/misc.py:42-51`).
- Hidden aliases for v9 habits (L118-135): `w`/`wallets`, `st`, `rt`, `s`/`subnet`, `su`/`sudo`, `lock`, `c`/`conf`, `wt`/`weight`/`weights`, `cr`/`crowdloan`/`crowd`, `addresses`, `extension`, `timelock`, `axon`, `collateral`, `upgrade`, `utils`.
- Commands per module (from `@app.command(` greps): `addr add|list|show|remove`; `axon set|reset`; `collateral show|list|add|set-min`; `config set|get|unset|clear|path`; `crowd list|info|contributors` (writes moved to `tx create-crowdloan`…); `extension bridge|stop|accounts`; `conviction list|targeting|show|add|mode|accept|move`; `misc convert|latency`; `multisig list|show|remove`; `proxy create|add|remove|kill|list|execute` + `proxy book add|list|remove|update|clear`; `root list|claim|allocate|register`; `stake add|remove|move|move-swap|transfer|swap|burn` (aliases of intent commands, `$SDK11/cli/commands/stake.py:50-62`), `stake unstake-all|unstake-all-alpha` (`$SDK11/cli/stake_exit.py:212-223`), `stake show|list|auto|set-auto|wizard`, `stake child|children get|set|revoke|take`; `subnets list|show|hyperparameters|burn-cost|create-cost|price|conviction|register|create|metagraph`; `hparams set|get|get-take|set-take|check-start|start|set-symbol|get-identity|set-identity|trim|trim-null-batch|trim-null|stake-burn` + `mechanisms|mech count|emissions`; `timelock encrypt|decrypt|show`; `upgrade pending`; `wallet create|new-coldkey|new-hotkey|regen-*|unlock|show|list|balance|overview|registrations|history|transfer|inspect|sign|verify|encrypt|decrypt|set-identity|get-identity|associate-hotkey|swap-hotkey|swap-coldkey|swap-check|announce-coldkey-swap` + `keychain save|show|delete`; `weights commit|reveal|set` (`$SDK11/cli/commands/weights.py:50-53`); `evm` has `key new|import|export|list|show`, `stake add|remove|show`, and `abi, associate, balance, call, claim-deposit, config, deploy, deposit-address, doctor, fund, mirror, networks, precompiles, pubkey, send, send-to-ss58, setup-localnet`.
- Global flags (`$SDK11/cli/globals.py`): `--network/-n`, `--wallet/-w`, `--wallet-hotkey/-H`, `--wallet-path`, `--wallet-password-file`, `--json`, `--yes/-y`, `--no-prompt`, `--dry-run`, `--quiet/-q`, `--verbose/-v`, `--proxy-for`, `--force-proxy-type`, `--signer`, `--signer-address`, `--signatory`, `--ledger`, `--ledger-account`, `--ledger-index`, `--extension-bridge/browser/source`, `--keychain-password`, `--macos-password`, `--fallback-endpoints`, `--archive-endpoints`, `--retry-forever`. Precedence flag > env > `~/.bittensor/btcli.json` > default (`$DOCS/cli.mdx:11-34`). Non-TTY without `--yes` → declined with exit 1; missing required options exit 2 (M:670-674).

### 4.3 v9 → v11 delta (authoritative table at `$DOCS/migration.mdx:679-737`)

Removed: `sudo senate|proposals|senate-vote` (M:704), `liquidity *` (M:716), `view dashboard` (M:717), `deriv *` (M:719), spec-448 `root subscribe|show` (M:724-737). Renamed: `subnets start|check-start|set-identity|get-identity|set-symbol|mechanisms` → `sudo …` (M:711-713); `config add-proxy|proxies|remove-proxy` → `proxy book add|list|remove` (M:715); `stake set-claim|process-claim` → `root claim` / `tx claim-root-with-hotkey` (M:702); `sudo set --param` → `--name` (M:705). New: `weights set`, `wallet registrations`, `query *`, `tx *`, `tools`, `explain`, `call`, `--dry-run`.

### 4.4 Scoping an optional rtao CLI

- The v11 design shows the cheap path: **generate `query`/`tx` from the same registries the library uses**, and hand-write only the ergonomic groups. For rtao that means the registry must be a Rust artifact (a `static` table produced by a proc-macro on intent structs/read fns) usable both by `rtao-cli` and the docs generator (§5.6).
- Minimum viable rtao CLI for subnet owners/validators: `query <read>`, `tx <intent>` (+`--dry-run`, `--json`, `--yes`), `wallet list|balance|new-*|regen-*`, `subnets metagraph|hyperparameters`, `weights set|commit|reveal`, `axon set|reset`, `hparams set|get`, `tools`, `explain`. Everything under `evm`, `multisig`, `extension`, `ledger`, `vault`, `root`, `crowd` is out of scope for a subnet-building toolkit unless cheap.
- Flag spellings: adopt v11 (`-w`, `-H`, `-n`, `--json`, `--yes`), accept v9 dotted aliases through clap `alias`/`visible_alias` as SN2 does (§3.4).

---

## 5. The bittensor.com docs corpus

### 5.1 Endpoints, index, and taxonomy

- `llms.txt` (`$CAT/bittensor.com_llms.txt`, 139 lines): a "Searching these docs" preamble (L3-15) instructing agents to `curl` the index, `rg` the full corpus, fetch one page at `/llms.mdx/docs/<slug>/content.md`, use the three JSON catalogs, search chain source via `/code/search.json?q=`, browse `/code/index.json` + `/code/raw/<path>`, or clone the repo; a "Reference catalogs" block (L17-27) stating that per-op/per-query pages are omitted from the index and that each catalog entry carries `markdown_url`, `docs_url`, and `sources`/`hits` with `raw_url`; the page tree (L29-129) with one-line descriptions; and a "Chain source code" footer pinned to commit `850f2755…` (L131-139). It is produced by `$WEB/src/app/llms.txt/route.ts` (`slimTree` collapses folders, `toAgentUrl` rewrites `/docs/foo` → `/llms.mdx/docs/foo/content.md`, `referenceHint` and `codeSection` emit the blocks; L17-144).
- `llms-full.txt` (`$CAT/bittensor.com_llms-full.txt`, 36,025 lines): concatenation of 728 pages, each starting with `# <Title> (/docs/<slug>)` and `## Heading [#anchor]` sub-headers (L1-40 sample). Page counts by prefix: errors 410 (19 semantic + 391 chain), query 103, tx 84, guides 52, hyperparameters 49, internals 14, concepts 10, plus `index`, `quickstart`, `sdk`, `agents`, `cli`, `migration`.
- Source tree `$DOCS`: `index, quickstart, sdk, agents, cli, migration` + `concepts/` (10), `guides/` (19 incl. `evm/`, `mining/`), `internals/` (14), `hyperparameters/` (49), `tx/` (84), `query/` (103), `errors/` (19) + `errors/chain/` (391); ordering in `$DOCS/meta.json` (`index, quickstart, sdk, agents, cli, concepts, guides, migration, [Releases], [Code], ---Reference--- tx, query, errors, hyperparameters, ---Contributing--- internals`).
- Only the hand-written parts are human-maintained; "The reference section (`docs/tx`, `docs/query`, `docs/errors`) and the JSON catalogs (`public/catalog/`) are generated from the SDK's own registries — never edit them by hand" (`$WEB/README.md:15-22`; regenerate with `uv run python ../../website/apps/bittensor-website/scripts/generate.py`, `--check` is the CI drift gate).
- Note: the three `docs.learnbittensor.org_*` / `docs.bittensor.com_llms.txt` files on disk are byte-identical (md5 `0dd9c532…`) and are an HTML page, not an llms index — the old site has no usable llms.txt. The old `developer-docs` repo is an empty relic pointing to latent-to/developer-docs (`upstream/developer-docs/README.md:11-13`).

### 5.2 Generated page templates (what an rtao generator should emit)

- **tx page** (`$CAT/bittensor.com_llms-full.txt:23708-23800`, `set-weights`): H1 `# set-weights (/docs/tx/set-weights)`; description paragraph (= intent docstring); a table `Signer | Origin | Pallet | Wraps` with `/code/...#Lx-Ly` links; `## Parameters` table (`Parameter | Type | Required | Description` from `input_schema`); note on address-argument resolution; `## CLI` (`btcli tx set-weights --netuid <int> --dry-run` then submit); `## Python` (`bt.SetWeights(...)`, `sub.plan`, `sub.execute`, `execute_tool`); `## On-chain implementation` with the dispatchable's Rust source excerpt (`#[pallet::call_index(119)] … pub fn set_mechanism_weights(...)`). Generated by `intent_page` (`$WEB/scripts/generate.py:637`), implementation section by `implementation_section`/`find_dispatchable` (L273-307, 538).
- **query page** (L15175-15210, `metagraph`): H1, docstring, `**Category:**`, `## Parameters` (`Parameter | Type | Description`), `## CLI` (`btcli query metagraph --netuid <integer> --json`), `## Python` (namespace call and/or `sub.read("metagraph", netuid=1)`), `## On-chain implementation` listing `Runtime API`/`Storage`/`Constant` hits with `/code` links. Generated by `read_page` (L812) using `read_targets` (L432-487).
- **hyperparameter page** (L11734-11786, `max_weights_limit`): prose, `## How it works`, `## Reading and setting` (`btcli sudo get --netuid N --name …`, who can set), `## Related`; MDX components like `<HyperparamWeightsRules focus=… />` appear in the raw dump.
- **chain error page** (L33039-33052, `SettingWeightsTooFast`): explanation, "Declared by the `SubtensorModule` pallet; it classifies to the semantic code `rate_limited`", "Declared at `pallets/subtensor/src/macros/errors.rs#L81`", `## Remediation`, and the `btcli explain <Name>` hint. Generated by `chain_error_page` (L1038).
- **index pages**: `Transactions (/docs/tx)` groups intents by pallet with `Operation | Signer | Origin | Summary` (L20117-20150); `Queries`, `Errors`, `Hyperparameters` similar.

### 5.3 Catalog JSON schemas (verified with `jq` over `$CAT/*.json`)

**`intents.json`** — array of 83 objects. Keys (union): `name, summary, description, signer, origin, verify, wraps, docs_url, input_schema, cli, python_class, markdown_url, sources`.

| field | type / values | notes |
|---|---|---|
| `name` | snake_case op | `"add_collateral"` |
| `summary` / `description` | string | first docstring line / full docstring (`list_tools`, `$SDK11/intents/registry.py:43-64`) |
| `signer` | `"coldkey"` (75) or `"hotkey"` (8) | hotkey-signed: `associate_evm_key, commit_weights, reset_axon, reveal_weights, serve_axon, serve_axon_tls, serve_prometheus, set_weights` |
| `origin` | `"signed"` (74), `"subnet_owner"` (7), `"root"` (2) | root ops wrap `Sudo.sudo` (`set_root_claim_threshold`, `set_subnet_emission_enabled`) |
| `verify` | `null` or read name | only the two root ops set it (`root_claim_threshold`, `subnet_emission_enabled`) — "a privileged write with no paired verify is a docs bug" (`$SDK11/intents/base.py:156-157`) |
| `wraps` | `[[pallet, call], …]` | e.g. `set_weights` → `SubtensorModule.set_mechanism_weights`, `commit_timelocked_mechanism_weights`; `set_hyperparameter` lists 33 `AdminUtils.sudo_set_*` calls |
| `docs_url` / `markdown_url` | `/docs/tx/<kebab>` / `/llms.mdx/docs/tx/<kebab>/content.md` | added by `write_catalogs` (`$WEB/scripts/generate.py:1226-1236`) |
| `input_schema` | JSON Schema object (`type: object`, `properties`, `required`, `additionalProperties: false`) | money fields use `anyOf [number, {string pattern ^\d+(\.\d+)?$}, {enum:["all"]}]`; arrays `{type: array, items: {type: integer}}` |
| `cli` | string | `"btcli tx add-collateral --netuid <int> --amount-alpha <amount|all>"` (from `intent_examples`, L1181) |
| `python_class` | PascalCase class | `"AddCollateral"` |
| `sources` | array of `{pallet, call, path, line, end_line, url, raw_url}` | absent for 6 Balances/EVM/Multisig ops (`find_dispatchable` only searches in-repo pallets; L211-245 `PALLET_DIRS`/`UPSTREAM_PALLETS`) |

Example (abridged from `$CAT/intents.json[0]`):
```json
{"name":"add_collateral","summary":"Lock additional miner collateral on your own hotkey.",
 "signer":"coldkey","origin":"signed","verify":null,
 "wraps":[["SubtensorModule","add_collateral"]],
 "docs_url":"/docs/tx/add-collateral",
 "input_schema":{"type":"object","properties":{"netuid":{"type":"integer","description":"Subnet to lock collateral on."},
   "amount_alpha":{"anyOf":[{"type":"number"},{"type":"string","pattern":"^\\d+(\\.\\d+)?$"},{"type":"string","enum":["all"]}]},
   "hotkey_ss58":{"type":"string"},"rate_tolerance":{"type":"number"}},
   "required":["netuid","amount_alpha"],"additionalProperties":false},
 "cli":"btcli tx add-collateral --netuid <int> --amount-alpha <amount|all>",
 "python_class":"AddCollateral","markdown_url":"/llms.mdx/docs/tx/add-collateral/content.md",
 "sources":[{"pallet":"SubtensorModule","call":"add_collateral","path":"pallets/subtensor/src/macros/dispatches.rs",
   "line":2875,"end_line":2883,"url":"/code/pallets/subtensor/src/macros/dispatches.rs#L2873-L2883",
   "raw_url":"/code/raw/pallets/subtensor/src/macros/dispatches.rs"}]}
```

**`reads.json`** — array of 102 objects. Keys: `name, summary, description, params, param_docs, category, docs_url, markdown_url, python, hits`.

| field | type / values |
|---|---|
| `params` | `{param: "integer"|"string"|"array"|"number"|"boolean"}` (57 integer, 45 string, 5 array, 2 number, 1 boolean params in total) |
| `param_docs` | `{param: description}` |
| `category` | one of 14: Accounts & keys (6), Chain (6), Delegation (9), Epochs & timing (5), Hyperparameters (7), Identity & commitments (5), Leases & crowdloans (5), Locks & conviction (7), Mining & collateral (3), Neurons & registration (6), Prices & swaps (4), Staking (22), Subnets (14), Weights & bonds (3) |
| `python` | `"sub.<namespace>.<name>(...)"`; absent when a curated helper shadows the name (only `metagraph`) |
| `hits` | array of `{kind: storage|runtime|constant, container, name, path, line, [end_line], url, raw_url}`; 86 storage, 32 runtime, 2 constant hits; absent for 10 computed reads (`basket_position, basket_trading_status, block_info, block_time, collateral_policy, commitments, is_fast_blocks, root_basket_portfolio, root_claim_threshold, timestamp`) |

Example (`$CAT/reads.json[0]`):
```json
{"name":"accepts_locked_alpha","summary":"Whether a coldkey accepts incoming locked alpha transfers.",
 "params":{"coldkey_ss58":"string"},"param_docs":{"coldkey_ss58":"Coldkey whose locked-alpha acceptance flag to read."},
 "category":"Locks & conviction","docs_url":"/docs/query/accepts-locked-alpha",
 "markdown_url":"/llms.mdx/docs/query/accepts-locked-alpha/content.md",
 "python":"sub.locks.accepts_locked_alpha(...)",
 "hits":[{"kind":"storage","container":"SubtensorModule","name":"AccountFlags","path":"pallets/subtensor/src/lib.rs",
   "line":1616,"url":"/code/pallets/subtensor/src/lib.rs#L1616","raw_url":"/code/raw/pallets/subtensor/src/lib.rs"}]}
```

**`errors.json`** — object `{note, codes, chain_errors}`.
- `note`: "Semantic ErrorCode values are under `codes` (snake_case keys). Exact on-chain error names are under `chain_errors` (CamelCase keys). Each entry has `docs_url` (HTML page) and `markdown_url` (raw markdown)."
- `codes`: 18 keys → `{remediation, docs_url, markdown_url}`: `already_exists, disabled, expired, insufficient_balance, insufficient_liquidity, internal, invalid_argument, limit_exceeded, not_authorized, not_found, not_registered, policy_violation, rate_limited, subnet_not_exists, subtoken_disabled, too_early, unit_mismatch, unknown`. Example: `insufficient_balance → {"remediation":"fund the signing account or reduce the amount; check with `btcli wallet balance`", "docs_url":"/docs/errors/insufficient-balance", …}`.
- `chain_errors`: 390 keys → `{code, description, docs_url, markdown_url, pallets: [..], sources?: [{pallet, path, line, url, raw_url}]}`. Pallet distribution: SubtensorModule 175, Contracts 37, LimitOrders 28, Crowdloan 28, AdminUtils 17, EVM 15, Multisig 14, Swap 13, Balances 12, Proxy 10, System 9, Preimage 8, SafeMode 7, Grandpa 7, Drand 6, Scheduler 5, Commitments 5, MevShield 4, Utility 2, Ethereum 2, Sudo 1. Code distribution: invalid_argument 145, limit_exceeded 45, already_exists 26, insufficient_balance 25, not_authorized 22, disabled 22, too_early 21, not_found 21, internal 19, rate_limited 17, insufficient_liquidity 9, expired 6, subnet_not_exists 4, not_registered 4, policy_violation 3, subtoken_disabled 1. Same name in several pallets → multiple `pallets`/`sources` (e.g. `Deprecated` in SubtensorModule, AdminUtils, Swap). Example: `WeightVecNotEqualSize → {code:"invalid_argument", description:"The `uids` and `values` vectors … different lengths…", pallets:["SubtensorModule"], sources:[{pallet:"SubtensorModule", path:"pallets/subtensor/src/macros/errors.rs", line:51, …}]}`. The migration page warns the `chain_errors` values changed from bare strings to objects (M:631-633).

**`index.json`** — `{commit: "850f2755bb05a3851f948489f0ab27b0645cebce", note, search: "/code/search.json?q=<literal>", files: [{path, lines, bytes}] (273 files)}`; roots: pallets 225, precompiles 24, runtime 13, common 5, primitives 4, chain-extensions 2.

### 5.4 How `markdown_url` / `sources` / `hits` are derived (generator mechanics)

- `write_catalogs` (`$WEB/scripts/generate.py:1226-1283`) starts from the SDK's own `list_tools()` (`$SDK11/intents/registry.py:43-64`, fields `name, summary, description, signer, origin, verify, wraps, docs_url, input_schema`) and `list_reads()` (`$SDK11/reads/base.py:140-154`, fields `name, summary, description, params, param_docs, category, docs_url`), then **adds** `cli`/`python_class` (`intent_examples`, L1181), `markdown_url` (`docs_markdown_url`, L79), `sources` (`intent_sources` → `find_dispatchable` regex `pub fn <call>(` over pallet sources, L273-307), `python` (namespace call unless shadowed, L766-785), and `hits` (`read_targets`, L432-487).
- `read_targets` **scrapes the read's Python source** with `inspect.getsource` for `st.<Pallet>.<Item>`, `api.<Api>.<fn>`, `constants.<Pallet>.<Name>` references (regexes L414-416) plus closure-captured descriptors for factory-made reads (`_closure_targets`, L419-429), and resolves each to a file:line via `find_storage_item` (`pub type <Name>` regex, L326-350), `find_runtime_api_fn` (searched in `pallets/subtensor`, `pallets/swap`, `runtime`, L378-393) or `find_runtime_constant` (L396-410). Errors: `chain_error_sources` → `find_error_variant` (L353-375); pallets from `CHAIN_ERROR_PALLETS`; descriptions from `error_descriptions.DESCRIPTIONS`; classification from `error_map.NAME_TO_CODE` (L1250-1270).
- Consequence for rtao: the "which storage/runtime items does this read touch" linkage is only as good as source scraping. In Rust this can be made exact: a read implemented as `client.storage(SubtensorModule::AccountFlags)` can carry its targets as *data* (e.g. a `const TARGETS: &[ChainTarget]` emitted by the macro), so rtao's catalog `hits` need no regex pass.

### 5.5 The "For agents" flow (`$DOCS/agents.mdx`, 143 lines)

1. Search: fetch `/llms.txt`, `rg` `/llms-full.txt`, fetch `/llms.mdx/docs/<slug>/content.md`, search/fetch chain source, or clone (L12-37).
2. Discover: `btcli tools` / `bt.intents.list_tools()` / `bt.reads.list_reads()`; static catalogs at `/catalog/{intents,reads,errors}.json` with `docs_url`, `markdown_url`, `sources`/`hits`+`raw_url` (L39-57).
3. Execute by name: `client.execute_tool("transfer", {...}, wallet)`; CLI `btcli tx <op-name>` (underscores→dashes), `btcli query <name>` (L59-72).
4. Preview: `client.plan(intent, wallet)` → `fee, effects, warnings, ok`; CLI `--dry-run` (L74-85).
5. Bound: `bt.Policy(max_spend_tao=5.0, allowed_netuids=[1,2])`; unboundable-cost ops blocked by spend cap; raw calls refused unless `allow_raw_calls=True` (L87-101).
6. Branch on `result.error.code` (`ErrorCode`), never parse prose; `remediation` always present (L103-116).
7. CLI non-interactive rules: `--json`, `--yes` (declined, not blocked), `--dry-run`, config precedence flags > `BT_*` env > `btcli config` (L118-125).
8. Escape hatch: `bt.calls.<Pallet>.<fn>(...)` + `client.submit_call`; generic `client.query(bt.storage…)`, `client.constant(bt.constants…)` (L127-143).

### 5.6 Proposal: rtao docs + LLM-context story

**(a) Mirror the taxonomy, not the prose.** Ship `rtao/docs/` with the same top-level shape (`index, quickstart, sdk, agents, cli, concepts, guides, migration, tx/, query/, errors/, hyperparameters/`) so an LLM that already knows bittensor.com's `llms.txt` can navigate rtao's by analogy. Hand-write only: `quickstart` (cargo add, connect, read, submit), `sdk` (the five primitives in Rust: `Subtensor`, `Wallet`/`Signer`, `Balance`, intents, `ExtrinsicResult`), `agents` (same eight steps as §5.5 with Rust/CLI commands), `cli`, `migration` (two tables: v10 Python → rtao, and bittensor-core (Rust core crate) → rtao), and Rust-specific guides (validator loop, miner server with btauth, owner tooling). For chain concepts link to `https://www.bittensor.com/llms.mdx/docs/concepts/<slug>/content.md` instead of duplicating (the upstream pages are raw-markdown-fetchable by design, `$CAT/bittensor.com_llms.txt:5-10`).

**(b) Generate the reference from the catalogs + rtao's own registry.** Inputs: upstream `intents.json`, `reads.json`, `errors.json` (vendored at a pinned commit, refreshed by a `cargo xtask sync-catalogs` that fetches `https://www.bittensor.com/catalog/*.json`), `finney.scale`, and rtao's registry (a `static` produced by `#[rtao::intent]` / `#[rtao::read]` macros carrying `op`, `signer`, `origin`, `verify`, `wraps`, `schemars` JSON schema, Rust type path, and chain targets). Outputs: `docs/tx/<kebab>.md` (same sections as §5.2 with `## Rust` replacing `## Python`), `docs/query/<kebab>.md`, `docs/errors/{<code>,chain/<Name>}.md`, `docs/hyperparameters/<name>.md` (from the 110-entry v11 `HYPERPARAMS` table shape: `kind, doc, short, signed, bits, minimum, maximum`, `$SDK11/hyperparams.py:91-110`), **and** `public/catalog/{intents,reads,errors}.json` using the upstream schema verbatim plus a `rust` object per entry: `{"crate":"rtao-intents","type":"rtao::intents::AddStake","op_const":"AddStake::OP","docs_rs":"https://docs.rs/..."}` for intents and `{"method":"client.staking().stake_for_coldkey(coldkey_ss58)","returns":"Vec<StakePosition>"}` for reads. A `--check` mode diffs regenerated output against committed output (CI drift gate, as `$WEB/README.md:21-22`), and a second check diffs rtao's registry against the vendored upstream catalogs: **any upstream `name` missing from rtao fails CI** — this is the concrete mechanism for "easy to keep up to date when the Python SDK changes".

**(c) Publish `llms.txt`/`llms-full.txt` for rtao** with the same preamble conventions (raw markdown links, "grep the full corpus", catalog URLs, and — new — `cargo doc` JSON/`docs.rs` links and a `rtao-cli tools` equivalent). Include a "Mapping to bittensor.com" block: `tx/<op>` ↔ `https://www.bittensor.com/llms.mdx/docs/tx/<op>/content.md`, so an agent can read the upstream semantic description and rtao's Rust signature side by side.

**(d) Naming map (so an agent can go from catalog → rtao API mechanically).** Rules: intent `name` → `Intent::OP` string constant and CLI `rtao tx <kebab>`; `python_class` → identical PascalCase Rust struct in `rtao::intents::{group}::<Class>` (`AddStake`, `SetWeights`, …; the 83 classes are listed in Appendix A); read `name` → method of the same snake_case name on the namespace object named after the `python` prefix (`sub.staking.stake_for_coldkey(...)` → `client.staking().stake_for_coldkey(...)`; 14 namespaces = 14 Rust modules/traits; the `metagraph` read, which has no `python` field, lives at `client.subnets().metagraph(netuid)` matching M:292); `codes` keys → `ErrorCode::{AlreadyExists, …, UnitMismatch, Unknown}`; `chain_errors` keys → `ChainError { name: &'static str, pallet, code: ErrorCode, docs_url }` generated table. Appendices A and B give the full per-entry mapping.

**(e) Keep "chain source" links.** rtao docs should reuse upstream `sources`/`hits` URLs (`/code/...#Lx-Ly` at `https://www.bittensor.com`) rather than host chain source; when rtao's codegen knows the exact storage/runtime item (it does — subxt descriptors), emit the `hits` from that knowledge rather than scraping.

---

## 6. Glossary (v11 term — v10 term — meaning — citation)

| v11 term | v10 / v9 term | Meaning | Source |
|---|---|---|---|
| **intent** | extrinsic method on `Subtensor` / `*_extrinsic` function; btcli command | A small dataclass naming one mutation (`op`, `signer`, `origin`, `wraps`, JSON schema), executed through `plan`/`execute`; "Every mutation is an intent" | `$DOCS/concepts/transactions.mdx` "Intents"; `$SDK11/intents/base.py:138-177`; M:117-120 |
| **plan / execute / execute_tool** | `sign_and_send_extrinsic`, `compose_call` | `plan` previews fee/effects/warnings/policy without submitting; `execute` submits; `execute_tool(op, dict)` is name-dispatch for agents | `$DOCS/agents.mdx:59-85`; M:380-388 |
| **read** (named) / **namespace** | query method (`get_*`, `query_subtensor`, …) | Typed method on one of 14 category namespaces, also dispatchable via `client.read(name, …)`; generic accessors are the escape hatch | `$DOCS/concepts/client.mdx` "Three levels of reading"; `$SDK11/namespaces.py:279` |
| **generic accessors** | `query_subtensor/query_map/query_constant/query_runtime_api/state_call` | `client.query(bt.storage.X)`, `query_map`, `constant`, `runtime` over generated descriptors (`bt.storage`, `bt.constants`, `bt.runtime_api`, `bt.calls`) | M:197-204; `$SDK11/_generated/` |
| **ExtrinsicResult** | `ExtrinsicResponse` | `.success`, `.message`, `.fee`, `.block_hash`, `.extrinsic_id`, `.events`, `.error` (`ChainError` with `.code`/`.remediation`) | M:482-494; v10 fields `$BT/core/types.py:300-375` |
| **ErrorCode** (semantic) / **chain error name** | typed exception classes (`StakeError`, `NotRegisteredError`, …) | 18 snake_case codes classify 390 CamelCase pallet error variants | `$CAT/errors.json` note; `$BT/core/errors.py` classes |
| **Policy** | — | Client-level hard bounds (`max_spend_tao`, `allowed_netuids`, `allow_raw_calls`); `PolicyError` | `$DOCS/agents.mdx:87-101` |
| **Snapshot** (`client.at(block)`) | `block=` kwarg on every read | Same read surface pinned to one block | M:184-188 |
| **btauth/1** (`http_auth.sign/verify`) | Synapse headers `bt_header_dendrite_*`/`bt_header_axon_*`, `computed_body_hash`, `Dendrite.preprocess_synapse_for_request`, `Axon.default_verify` | Hotkey-signed HTTP: 6 `X-Bittensor-*` headers; signed payload = 8 newline-joined lines (`btauth/1`, scheme, METHOD, target, sha256(body), nonce_ns, sender, receiver-or-`-`); freshness 10 s past / 2 s future; nonce replay store. v10 signed `"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"` with sha3-256 body hash of `required_hash_fields` and a 4 s + timeout nonce window | `$SDK11/http_auth.py:58-75, 157-187`; `$DOCS/guides/signed-requests.mdx` wire-format + legacy sections; `$BT/core/dendrite.py` preprocess; `$BT/core/axon.py:930-986`; `$BT/utils/axon_utils.py:3-22` |
| **axon** (chain record) | `bt.Axon` (FastAPI server) + `AxonInfo` | v11 keeps only the chain-side endpoint record (`ServeAxon` intent; `AxonInfo{block,version,ip:u128,port,ip_type,protocol,placeholder1,placeholder2}` in the metagraph) — no server | M:455-480; `get_metagraph` struct (subxt explore) |
| **dendrite** | `bt.Dendrite` (aiohttp fan-out client) | gone; "your own client" + `http_auth` | M:462-470 |
| **synapse** | `bt.Synapse` pydantic request/response schema | gone; any schema, signing covers raw bytes | M:466 |
| **TAO / alpha / rao / beta (β)** | same (beta new in v11 docs) | TAO native (1 TAO = 10⁹ rao); alpha = per-subnet pool token; netuid 0 = root (TAO); beta = share count in one validator's root **basket** (fund of subnet tokens) | `$DOCS/concepts/network.mdx` "TAO and alpha"; `$DOCS/concepts/beta-tokens.mdx:5-13` |
| **basket / Root Reborn** | root claims (`claim_root`, `RootClaimType`) | Validator-run fund on root; `stake_into_basket`, `swap_basket`, `claim_root_with_hotkey`; `RootClaimType` retired (v441) | M:287, 367-368; `$CAT/intents.json` names |
| **tempo / epoch** | same | Epoch fires once `tempo` blocks elapsed (default 360, owner-settable 360..50,400); ≤2 subnet epochs per block; emission settled per epoch | `$DOCS/concepts/emissions.mdx` "Epochs" |
| **mechanism / mechid / mechagraph** | "subsubnet"; `mechid` kwarg | A subnet may run multiple scoring mechanisms (cap 2), each with its own weights/bonds/Yuma; miners keep one UID; `get_mechagraph`/`get_all_mechagraphs`/`get_selective_mechagraph` runtime APIs; storage index `mechid*4096+netuid` | `$DOCS/guides/subnets.mdx:218-232`; `pallets/subtensor/runtime-api/src/lib.rs:58-64`; `$BT/utils/__init__.py:67-77` |
| **conviction / lock** | `lock_stake`, `get_stake_lock`, `btcli lock` | A floor on unstaking per coldkey per subnet; conviction accrues to a chosen hotkey; perpetual vs decaying; subnet ownership can transfer to the most-convicted hotkey (>18 % of eligible alpha, subnets ≥1 year) | `$DOCS/guides/conviction.mdx` "What locking does"; M:521-526 |
| **collateral** | — (new, spec ≥435) | `collateral_lock_share` p and `collateral_drain_ratio` k: `burned=(1−p)·T`, `locked=max(0,p·T−standing)`; locked alpha is real stake, not withdrawable, released via emission; `AddCollateral`, `SetMinCollateral`, `client.collateral.*` | `$DOCS/guides/mining/collateral.mdx` "Mechanics"; M:375-376, 571-577 |
| **stake weight / tao_weight** | `metagraph.S`, `ROOT_TAO_STAKE_WEIGHT = 0.18` | `alpha_stake + tao_stake × tao_weight` (0.18 mainnet); permits = top K by stake weight | `$DOCS/guides/validating.mdx` "Permits"; `$BT/core/settings.py:6` |
| **validator permit / activity cutoff** | same | top-`MaxAllowedValidators` (128); `StakeThreshold`; cutoff = `activity_cutoff_factor × tempo / 1000` (factor default 13,889) | `$DOCS/guides/validating.mdx` "Permits" |
| **commit-reveal / timelock / drand** | `commit_weights` (salt) vs `commit_timelocked_weights` | v11 `SetWeights` auto-selects plaintext vs timelock-encrypted commit (auto-revealed at the drand round); salt-based `CommitWeights/RevealWeights` remain for legacy; `bt.timelock` for data-plane sealing | `$CAT/intents.json` `set_weights`; `$DOCS/guides/signed-requests.mdx` "Composing with timelock" |
| **MEV shield** | `mev_protection=`, `mev_submit_encrypted` | ML-KEM-768-encrypted submission (`MevShield.NextKey`); default on for stake-trading intents; `submit_shielded`; era period 8 | M:336, 378; `$SDK11/settings.py:29`; `$BT/core/settings.py:73-85` |
| **Null consensus** | — | Alternative to Yuma: integer weights preserved (`raw_u16`), one permit holder; `SubnetEpochConsensus` storage | `$SDK11/intents/weights.py:260-265`; `$CAT/intents.json` `set_weights` description |
| **burn / recycle** | `recycle(netuid)` | Registration price (`Burn`), floating; `burned_register`; v11 read `burn` | M:237 |
| **hyperparameter** (`SetHyperparameter(name, value)`) | `btcli sudo set --param`; `set_tempo`/`set_activity_cutoff_factor` methods | one intent wrapping 33 `AdminUtils.sudo_set_*`; value rules: decimal point = human form, plain int = raw | `$CAT/intents.json` `set_hyperparameter`; M:374, 405-412 |
| **coldkey / hotkey** | same | coldkey owns funds (transfers, staking, registration, ownership); hotkey operates (weights, axon) | `$DOCS/concepts/network.mdx` "Who signs what" |
| **netuid / uid / NetUidStorageIndex** | same | subnet id (u16); neuron slot; mechanism-scoped storage key | `$META/explore/_storage_table.tsv` `Weights` row |
| **childkeys / take** | `set_children`, `set_delegate_take` | proportions as 0–1 floats (raw u64 of `u64::MAX`); take 0–1 float (raw u16) | M:405-412 |

---

## 7. Decisions for architects and open questions

Decisions (with my recommendation):
1. **Compat surface.** Ship an `rtao-compat` crate (or `#[deprecated]` extension trait) with only the **A** set from §1.5 (≈25 names) plus v10 metagraph array accessors; everything else maps by documentation only. Rationale: the only measurable v10 usage is the template/SN1/Chi call-site set.
2. **Weights pipeline.** Canonical = v11 `_conform` (clip → max-upscale → u16 → min-allowed check, self-weight exemption, `raw_u16` only on Null). Provide `legacy::process_weights` with v10's quantile/uniform fallbacks behind a feature flag and a loud doc warning. `version_key` default 0.
3. **Balance type.** `u64` rao + unit tag; same-unit arithmetic only; `Decimal`-style parsing; no float constructors in the default API.
4. **Config.** clap kebab flags + dotted v10 aliases + v11 env names with v10 fallbacks (SN2 pattern); no YAML `--config` magic inside the library (v11 removed it; keep config in the binary crate).
5. **Persistence.** Do not read `.pt`; optional `npz` reader for `state.npz` (`scores`,`hotkeys`,`step`) is cheap and lets Rust validators inherit Python state — decide if worth a dependency.
6. **CLI.** Generated `query`/`tx` from the registry + a small hand-written set; v11 names primary, v9 shorthands as hidden aliases; skip evm/multisig/ledger/vault/extension.
7. **Docs/LLM.** Adopt the upstream catalog schema verbatim + `rust` extension object; generate reference pages and `llms.txt`/`llms-full.txt`; CI drift gate against vendored upstream catalogs; link to upstream for concepts and chain source.
8. **Legacy btauth bridge.** Offer `verify_v10_dendrite(headers, body_hash)` (message format in §6) as an opt-in feature for miners during transition; default off.

Open questions:
- Upstream catalogs carry no schema version; rtao's sync tool must tolerate field additions (the `chain_errors` value shape already changed once, M:631-633).
- `reads.json` lacks `hits` for 10 computed reads and `intents.json` lacks `sources` for 6 upstream-pallet ops; the rtao generator must render "no on-chain implementation link" gracefully.
- Whether rtao's typed metagraph should expose `get_mechagraph` for `mechid > 0` from day one (v11 `Metagraph` has a `mechid` field; the v10 selective/mechagraph runtime calls exist).
- Whether to reproduce v10's 438-symbol `UNITS` table or rely solely on chain `token_symbols` (v11 behaviour).
- Fast-blocks/local-dev detection (`is_fast_blocks` reads the `Aura` pallet in v10; v11 `block_time` read says 12.0 mainnet / 0.25 fast-blocks) — needed for rtao's local-dev story.
- The exact set of `SubtensorModule` calls that v11 deliberately does not wrap (e.g. `trigger_epoch`, proxy announcements, `set_commitment`) — confirm whether rtao wants intents for them or leaves them to `submit_call`.

---

## Appendix A — intents.json → proposed rtao names (generated from `$CAT/intents.json`)

Columns: op name | python_class → proposed `rtao::intents::<Class>` | signer | origin | wraps | v11 CLI.
