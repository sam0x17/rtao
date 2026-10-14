# Parity inventory: intents, reads, error codes and CLI

This appendix makes Python parity concrete and trackable. It lists every operation of the v11 Python package's
machine-readable surface: 83 intents, 102 reads and 18 semantic error codes from the catalogs, plus the 163
hand-written commands of the `btcli` command tree. Each row gives the rtao home, how the body is produced and the
milestone that delivers it. It extends the mechanically generated catalog inventory with the columns that
[architecture.md](architecture.md) defines ([§2.2](architecture.md#22-crates),
[§4.2](architecture.md#42-intent-representation)-[§4.9](architecture.md#49-parity-enforcement-the-machinery),
[§9](architecture.md#9-framework-for-owners-validators-and-miners-rtao-neuron),
[§10](architecture.md#10-cli-rtao-cli-binary-rtao), [§14](architecture.md#14-milestones)). It is the reviewable
form of `parity/pending.toml` and `parity/matrix.json` ([§4.9](architecture.md#49-parity-enforcement-the-machinery)).

Pin: `parity_tag` v477, monorepo commit `850f2755` (`UP/`), Python package `bittensor` `11.3.1.dev0`
(`UP/sdk/python/pyproject.toml:7`). Citation prefixes (`UP/`, `CORE/`, `PY/`, `PYT/`, `CAT/`) and the `[I]` marker
(inference, not executed) follow [architecture.md, Evidence conventions](architecture.md#evidence-conventions). `§n`
without a file name refers to architecture.md. The per-file view of the same surface is
[python-module-map.md](python-module-map.md).

## Contents

1. [Summary by milestone](#1-summary-by-milestone)
2. [Method](#2-method)
3. [Intents (83)](#3-intents-83)
4. [Generated pass-through bodies: which 46](#4-generated-pass-through-bodies-which-46)
5. [Reads (102)](#5-reads-102)
6. [Error codes (18) and the chain-error tables](#6-error-codes-18-and-the-chain-error-tables)
7. [CLI command tree](#7-cli-command-tree)
8. [Inconsistencies found and how this inventory resolves them](#8-inconsistencies-found-and-how-this-inventory-resolves-them)

## 1. Summary by milestone

New items per milestone, with the cumulative total in parentheses. Milestones are those of
[§14](architecture.md#14-milestones). Section 2 states how each item was assigned.

| Milestone | Intents | Reads | Error codes | CLI commands (of 163) | CLI groups complete (of 19) | Generated `tx` / `query` |
|---|---:|---:|---:|---:|---:|---|
| M0 | 0 (0) | 0 (0) | 0 (0) | 0 (0) | 0 (0) | — |
| M1 | 0 (0) | 2 (2) | 0 (0) | 0 (0) | 0 (0) | — |
| M2 | 9 (9) | 13 (15) | 18 (18) | 0 (0) | 0 (0) | — |
| M3 | 0 (9) | 1 (16) | 0 (18) | 0 (0) | 0 (0) | — |
| M4 | 11 (20) | 22 (38) | 0 (18) | 40 (40) | 3 (3) | `tx`: 20 implemented ops; `query`: 38 implemented reads |
| M5 | 63 (83) | 2 (40) | 0 (18) | 20 (60) | 2 (5) | `tx` complete (83) |
| M6 | 0 (83) | 62 (102) | 0 (18) | 24 (84) | 4 (9) | `query` complete (102) |
| M7 | 0 (83) | 0 (102) | 0 (18) | 34 (118) | 3 (12) | — |
| M8 | 0 (83) | 0 (102) | 0 (18) | 45 (163) | 7 (19) | — |
| M9 | 0 (83) | 0 (102) | 0 (18) | 0 (163) | 0 (19) | — |

Notes on the table:

- **Intents** follow the explicit lists of §14: M2 has 9, M4 has 11, and M5 the remaining 63.
- **Reads**: 2 in M1, 13 in M2, 1 in M3, 22 in M4, 2 in M5, 62 in M6 (`root_claim_threshold` moved to M5 with its
  op, section 8 #9). Of the 22 M4 reads, 13 are needed only by the CLI v0 commands that
  [§10.6](architecture.md#106-scope-per-milestone) schedules for M4 (`wallet balance`, `wallet inspect`, the read-only
  `hparams` commands and the subnet-name cache).
- **Error codes**: all 18 land in M2 with the generated `ErrorCode`/`CHAIN_ERRORS` (§14 M2).
- **CLI** counts hand-written leaf commands of the Python tree (hidden aliases excluded), split by
  [§10.6](architecture.md#106-scope-per-milestone), which spreads CLI work over M4-M8. A group counts as complete in
  the milestone of its last command.
- M0 and M9 add no inventory items (foundations, hardening); M3 adds one read; M7 adds CLI commands only (signers,
  EVM, timelock UX).

## 2. Method

### 2.1 Sources and cross-checks

Every row was checked against the sources, not copied. Results:

| Check | Result |
|---|---|
| catalogs fetched from <https://www.bittensor.com/catalog/> on 2026-10-13 (the source of the generated inventory) vs `CAT/intents.json`, `CAT/reads.json`, `CAT/errors.json` | byte-identical (sha256 `56a3ebb6…`, `1b55578a…`, `044300bd…`) |
| generated inventory vs catalog JSON: 83 intent rows (class, signer, origin, wraps, required params), 102 read rows (params, Python call, hits), 14 category counts, 18 remediation strings | 0 differences |
| catalog vs `PY/intents/*.py`: for each of 83 ops, `python_class` exists and its `op`, `signer`, `origin`, `wraps`, `verify`, field names, field order and required set equal the catalog | 0 differences. `SetHyperparameter.wraps` is computed from `OWNER_HYPERPARAMETERS` (`PY/intents/hyperparameters.py:31-66`, 34 names over 33 setters) and equals the catalog list in order |
| catalog vs `PY/reads/*.py`: every read registered, same module category | 102/102 (93 `@read`, 9 `scalar_read`) |
| catalog statistics quoted in [§4](architecture.md#4-intents-and-reads-parity) | confirmed: signer 75 coldkey / 8 hotkey; origin 74 signed / 7 subnet_owner / 2 root; 139 wrap entries over 116 distinct calls; read hits 81 single / 11 multi / 10 none; 390 chain error names |
| `PY/error_descriptions/*` vs `CAT/errors.json` `chain_errors[*].description` | same 390 keys, 0 text differences |
| `PY/result.py` `REMEDIATION` (`:78-111`) vs catalog `codes[*].remediation` | 18/18 equal |
| catalog `hits` vs the chain items each read actually touches (`PY/reads/*.py`, including module helpers and string-built `Item(...)`/`Method(...)` descriptors) | 12 reads touch items the catalog omits; see [section 5.15](#515-hits-the-catalog-omits) |
| e2e manifest `UP/sdk/bittensor-core/tests/e2e-manifest.json` | 112 names = 73 `intent_*` + 39 `test_*`; 72 `intent_*` names match catalog ops; `intent_set_root_claim_type` (`e2e.rs:164`) tests a retired call that is not a catalog op |

### 2.2 Body kind

[§4.3](architecture.md#43-generated-pass-through-bodies-and-their-limits) generates `build()` for "46 of 83" intents
that wrap a single call whose arguments map one-to-one onto schema fields after stripping `_ss58`, `_tao`, `_alpha`,
`_rao` and `_hex`. That count comes from a name-matching heuristic. This inventory re-derived the set from the
`build()` bodies in `PY/intents/*.py`, using this rule: a body is **generated** when it composes exactly one call, every
call argument is an intent field (renamed only by suffix stripping) or a generic origin default (`hotkey_ss58` falling
back to the signer's hotkey, `PY/intents/base.py:258-274`), and every conversion is expressible as a field type
(money, `Ratio → PerU16`, proportions → `U64Normalized`, string → bytes, hex → bytes, `Option`). `"all"` resolution
may appear, because §4.3 keeps it hand-written as a resolver the generated body calls. Anything else is **ported**.
The result is again 46, but with one swap against the heuristic (section 4).

Generated bodies do not make an intent free. The **hand-written hooks** column lists what stays hand-written for each
op: every Python override of the `Intent` hooks (`PY/intents/base.py:138-399`), mapped to the Rust trait methods of
[§4.2](architecture.md#42-intent-representation) (`__post_init__` → `validate`), plus `"all"` resolution, the
shield-required route and executor post-processing. `summary()` is hand-written for all 83 and is not repeated. A ¹
marks a hook inherited from a Python base class (`_RootClaimIntent`, `_OptionalRootClaimIntent`).

### 2.3 Intent milestones

§14 lists the intents of M2 and M4 by name. Everything else is M5 ("the remaining 63 intents"). Ambiguities resolved
here:

| Op | Candidates | Resolved | Reason |
|---|---|---|---|
| `set_identity` (coldkey identity) | M4 "identity" or M5 | M5 | M4's identity work is owner-side: `OwnerOps::set_identity(netuid, SubnetIdentity)` ([§9.7](architecture.md#97-owner-operations)) and `hparams set-identity` (§10.6). `wallet set-identity` is M8 CLI |
| `set_subnet_identity` | M4 "identity" | M4 | the subnet identity setter that `OwnerOps::set_identity` wraps |
| `trim_subnet`, `trim_null_subnet_batch` | M4 "trims" | M4 | `OwnerOps::trim`, `OwnerOps::trim_null` (§9.7) |
| `set_mechanism_count` | M4 "mechanisms" | M4 | `OwnerOps::set_mechanism_count` (§9.7) |
| `update_symbol` | M4 "symbol" | M4 | `OwnerOps::update_symbol` (§9.7) |
| `swap_hotkey` | M4 (lives in `registration.py`) or M5 | M5 | §14 M4 enumerates the registration intents it means: `burned_register`, `pow_register`, `root_register` |
| `claim_root`, `claim_root_with_hotkey` | M4 (`registration.py`) or M5 | M5 | §14 M5 names "root claim + fee preview" |
| `stake_burn` | M4 (`governance.py`; `hparams stake-burn` is in the M4 CLI group) or M5 | M5 | not in §14 M4's list; the `hparams stake-burn` command moves with it (section 8) |
| `set_take` | M4 (`hparams set-take`) or M5 | M5 | §14 M5 names "children/take"; the CLI command moves with it (section 8) |
| `add_collateral` | M4 (shield pipeline; §11.4 scenario 11 shields it) or M5 | M5 | §14 M5 names "collateral"; M4's shielded scenario covers `burned_register` until M5 (section 8) |
| `register_leased_network` | M4 (registers a subnet) or M5 | M5 | §14 M5 names "leasing" |
| `set_root_claim_threshold`, `set_subnet_emission_enabled` | M5 | M5 | root-origin ops; no framework consumer |

### 2.4 Read milestones

§14 originally shipped "all 102 reads" in M6 but needs some earlier; it now lists per milestone the reads assigned
here. A read lands in the earliest milestone in which one of these needs it, otherwise in M6:

1. a framework component of [§9](architecture.md#9-framework-for-owners-validators-and-miners-rtao-neuron) or a §14
   deliverable consumes the chain items the read decodes (`[I]` where the component reads the items through
   descriptors rather than through the read itself; the read is then a thin wrapper over the same decode);
2. a CLI command that [§10.6](architecture.md#106-scope-per-milestone) schedules calls the read in the Python
   implementation (`client.read("<name>")` or a namespace method);
3. never because of an intent body: Python intents query storage directly and never call the read registry
   (`grep -n "reads" PY/intents/*.py` matches only docstrings and comments; no file imports `bittensor.reads` or
   calls `.read(`).

The "why" column of section 5 cites the consumer. Reads the AxonPublisher needs do not exist in the catalog: it
consumes `Axons` and `ServingRateLimit` ([§9.5](architecture.md#95-axonpublisher)), which no catalog read exposes, so
M3 gains only `immunity_period`.

### 2.5 Upstream e2e names

The e2e column lists, for each op: the upstream plan test `intent_<op>` and any submit-and-assert `test_*` that drives
the op's chain call, both in `UP/sdk/bittensor-core/tests/e2e.rs` (the number is the line of the macro invocation or
`fn`); Python e2e tests in `PYT/e2e/` where they use the intent; and the rtao localnet scenarios of
[§11.4](architecture.md#114-localnet-e2e-docker) as `§11.4 #n`. The 72 `intent_*` tests only `plan()` a sample
intent (`e2e.rs:81-96`). Eleven ops have no `intent_*` name in the manifest: `add_collateral`, `move_swap_stake`,
`pow_register`, `set_min_collateral`, `set_reject_locked_alpha`, `set_root_claim_threshold`,
`set_subnet_emission_enabled`, `stake_into_basket`, `swap_basket`, `swap_basket_many`, `trim_null_subnet_batch`.
`pow_register` is exercised by `PYT/e2e/test_pow_registration.py` and §11.4 scenario 14, `add_collateral` by scenario
11 and `trim_null_subnet_batch` by scenario 4; the other eight are covered by §11.4 scenario 18 (section 8 #9).

**`e2e_map.toml` states.** Each key holds `rtao = "<target>::<test fn>"`, `na = "<reason>"` or `pending = "M<k>"`
([§4.9](architecture.md#49-parity-enforcement-the-machinery)); a key leaves `pending` in the milestone that ships the
code it exercises, and strict mode (M8) allows none. The 72 live `intent_*` keys map to the table-driven plan test (M5)
and `intent_set_root_claim_type` is `na`. 24 of the 39 `test_*` names drive op calls and appear in the e2e column of
section 3; each takes the latest milestone among the ops it is listed under. The other 15 exercise the client, reads or
raw calls and appear in no op row (`e2e.rs` line in parentheses):

| Milestone | Upstream `test_*` names | What they need |
|---|---|---|
| M2 | `test_block_number` (:1159), `test_block_subscription_streams_increasing_headers` (:1319), `test_generic_accessors_over_generated_descriptors` (:1187), `test_snapshot_pins_reads_to_one_block` (:1283), `test_compose_nested_sudo_call` (:440), `test_reads_catalog_nonempty` (:1207) | `Localnet`; the M1 client, views, head stream and runtime calls; `compose::sudo` and `Client::submit`; `ReadSpec` |
| M4 | `test_mev_shield_next_key_is_mlkem768` (:288), `test_balance_and_existential_deposit` (:1175), `test_typed_reads` (:1213), `test_regular_commitment_stays_readable` (:476) | reads `mev_shield_next_key`, `balance`, `subnet_hyperparameters`; the raw path with a hotkey signer (`submit_call_dyn`) |
| M6 | `test_subnets_all_batched` (:1165), `test_quote_stake_slippage_and_fee` (:1258), `test_metagraph_fast_path_matches_runtime` (:1269), `test_leases_read` (:1304) | reads `subnets`, `quote_stake`, `neurons`, `leases` |
| M7 | `test_timelocked_commitment_reveals_plaintext` (:542) | §11.4 scenario 19 (timelocked commitment) |

## 3. Intents (83)

One subsection per Python group. The group is the Rust module `rtao_intents::<group>`
(`crates/rtao-intents/src/<group>.rs`, [§4.4](architecture.md#44-hand-ported-bodies)) and the Cargo feature of the
same name ([§2.2](architecture.md#22-crates)); each `AnyIntent` variant is `#[cfg(feature = "<group>")]`
([§4.5](architecture.md#45-compile-time-completeness)). The struct name is the Python class name, generated into
`crates/rtao-intents/src/generated/interfaces.rs` with `#[doc(alias = "bittensor.intents.<group>.<Class>")]`
([§4.2](architecture.md#42-intent-representation)); the facade path is `rtao::intents::<Class>` (§13 catalog
example). `#` is the row number of the alphabetical catalog listing. Column "py" is the class line in
`PY/intents/<group>.py`; the e2e numbers are lines of `e2e.rs`.

### 3.1 `association`

`PY/intents/association.py` (95 lines) → `rtao_intents::association`, feature `association`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 6 | `associate_evm_key` | `AssociateEvmKey` (:46) | hotkey · signed | SubtensorModule.associate_evm_key | netuid, evm_key, block_number, signature | generated; `signature` 0x-hex → bytes: field-type override (`association.py:84-92`) | — | M5 | `intent_associate_evm_key` (:102) |
| 7 | `associate_hotkey` | `AssociateHotkey` (:15) | coldkey · signed | SubtensorModule.try_associate_hotkey | — | generated; `hotkey_ss58` defaults to the origin view's hotkey (generic `hotkey_address`, `base.py:258-274`) | — | M5 | `intent_associate_hotkey` (:103); `test_key_association` (:748) |

### 3.2 `batch`

`PY/intents/batch.py` (167 lines) → `rtao_intents::batch`, feature `batch`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 8 | `batch` | `Batch` (:25) | coldkey · signed | Utility.batch_all | intents | ported; composes children with `Utility.batch_all` (`batch.py:78-91`) | validate, effects, warnings, blocks, preflight, spend, touches_netuids, affects_all_subnets, namespaced child extras (`batch.py:54-164`) | M5 | `intent_batch` (:104); `test_batch_applies_all_children_atomically` (:194); `test_failed_batch_reverts_everything` (:230); `test_policy_aggregates_spend_across_batch` (:261); §11.4 #12 |

### 3.3 `children`

`PY/intents/children.py` (247 lines) → `rtao_intents::children`, feature `children`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 17 | `decrease_take` | `DecreaseTake` (:175) | coldkey · signed | SubtensorModule.decrease_take | take | generated; `take` (int, float or str) → `PerU16` by field type (§4.3) | validate | M5 | `intent_decrease_take` (:116) |
| 24 | `increase_take` | `IncreaseTake` (:141) | coldkey · signed | SubtensorModule.increase_take | take | generated; `take` → `PerU16` by field type | validate | M5 | `intent_increase_take` (:123) |
| 49 | `set_childkey_take` | `SetChildkeyTake` (:105) | coldkey · signed | SubtensorModule.set_childkey_take | netuid, take | generated; `take` → `PerU16` by field type | validate | M5 | `intent_set_childkey_take` (:146) |
| 50 | `set_children` | `SetChildren` (:34) | coldkey · signed | SubtensorModule.set_children | netuid, children | generated; proportions → `U64Normalized`; validation stays hand-written (§4.3) | validate, warnings | M5 | `intent_set_children` (:147) |
| 61 | `set_take` | `SetTake` (:208) | coldkey · signed | SubtensorModule.increase_take, SubtensorModule.decrease_take | take | ported; reads `Delegates`, picks increase or decrease (`children.py:233-244`) | validate | M5 | `intent_set_take` (:170); `test_delegation_lifecycle` (:1372) |

### 3.4 `coldkey`

`PY/intents/coldkey.py` (185 lines) → `rtao_intents::coldkey`, feature `coldkey`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 5 | `announce_coldkey_swap` | `AnnounceColdkeySwap` (:38) | coldkey · signed | SubtensorModule.announce_coldkey_swap | new_coldkey_ss58 | ported; derives `new_coldkey_hash` (`coldkey.py:69-74`) | warnings | M5 | `intent_announce_coldkey_swap` (:101); `test_coldkey_swap_announcement_flow` (:665) |
| 12 | `clear_coldkey_swap_announcement` | `ClearColdkeySwapAnnouncement` (:132) | coldkey · signed | SubtensorModule.clear_coldkey_swap_announcement | — | generated | — | M5 | `intent_clear_coldkey_swap_announcement` (:108) |
| 18 | `dispute_coldkey_swap` | `DisputeColdkeySwap` (:159) | coldkey · signed | SubtensorModule.dispute_coldkey_swap | — | generated | warnings | M5 | `intent_dispute_coldkey_swap` (:117) |
| 68 | `swap_coldkey_announced` | `SwapColdkeyAnnounced` (:89) | coldkey · signed | SubtensorModule.swap_coldkey_announced | new_coldkey_ss58 | generated | warnings, affects_all_subnets | M5 | `intent_swap_coldkey_announced` (:174); `test_coldkey_swap_announcement_flow` (:665) |

### 3.5 `collateral`

`PY/intents/collateral.py` (176 lines) → `rtao_intents::collateral`, feature `collateral`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 1 | `add_collateral` | `AddCollateral` (:52) | coldkey · signed | SubtensorModule.add_collateral | netuid, amount_alpha | ported; derives `limit_price` from the alpha price; shield required (`collateral.py:103-117`) | validate, warnings, spend, `"all"` resolver, shield-required route | M5 | §11.4 #11 |
| 55 | `set_min_collateral` | `SetMinCollateral` (:139) | coldkey · signed | SubtensorModule.set_min_collateral | netuid, min_alpha | ported; near pass-through (rename candidate, section 4.4) | validate | M5 | §11.4 #18 (no upstream e2e test; M5) |

### 3.6 `crowdloan`

`PY/intents/crowdloan.py` (417 lines) → `rtao_intents::crowdloan`, feature `crowdloan`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 14 | `contribute_crowdloan` | `ContributeCrowdloan` (:118) | coldkey · signed | Crowdloan.contribute | crowdloan_id, amount_tao | generated; build resolves `"all"` via `_stakeable_rao`: generated build calls a hand-written resolver (§4.3) | validate, warnings, spend, `"all"` resolver | M5 | `intent_contribute_crowdloan` (:113) |
| 15 | `create_crowdloan` | `CreateCrowdloan` (:27) | coldkey · signed | Crowdloan.create | deposit_tao, min_contribution_tao, cap_tao, end | ported; optional nested intent `call` (`crowdloan.py:89-107`) | validate, spend | M5 | `intent_create_crowdloan` (:114); `test_crowdloan_create_read_update` (:343) |
| 19 | `dissolve_crowdloan` | `DissolveCrowdloan` (:231) | coldkey · signed | Crowdloan.dissolve | crowdloan_id | generated | — | M5 | `intent_dissolve_crowdloan` (:118) |
| 22 | `finalize_crowdloan` | `FinalizeCrowdloan` (:184) | coldkey · signed | Crowdloan.finalize | crowdloan_id | generated | — | M5 | `intent_finalize_crowdloan` (:121) |
| 35 | `refund_crowdloan` | `RefundCrowdloan` (:207) | coldkey · signed | Crowdloan.refund | crowdloan_id | generated | — | M5 | `intent_refund_crowdloan` (:132) |
| 51 | `set_crowdloan_max_contribution` | `SetCrowdloanMaxContribution` (:378) | coldkey · signed | Crowdloan.set_max_contribution | crowdloan_id | generated | validate | M5 | `intent_set_crowdloan_max_contribution` (:148) |
| 79 | `update_crowdloan_cap` | `UpdateCrowdloanCap` (:279) | coldkey · signed | Crowdloan.update_cap | crowdloan_id, new_cap_tao | generated | validate | M5 | `intent_update_crowdloan_cap` (:184); `test_crowdloan_create_read_update` (:343) |
| 80 | `update_crowdloan_end` | `UpdateCrowdloanEnd` (:308) | coldkey · signed | Crowdloan.update_end | crowdloan_id, new_end | generated | — | M5 | `intent_update_crowdloan_end` (:185) |
| 81 | `update_crowdloan_min_contribution` | `UpdateCrowdloanMinContribution` (:341) | coldkey · signed | Crowdloan.update_min_contribution | crowdloan_id, new_min_contribution_tao | generated | validate | M5 | `intent_update_crowdloan_min_contribution` (:186) |
| 83 | `withdraw_crowdloan` | `WithdrawCrowdloan` (:255) | coldkey · signed | Crowdloan.withdraw | crowdloan_id | generated | — | M5 | `intent_withdraw_crowdloan` (:191) |

### 3.7 `evm`

`PY/intents/evm.py` (119 lines) → `rtao_intents::evm`, feature `evm`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 20 | `evm_withdraw` | `EvmWithdraw` (:79) | coldkey · signed | EVM.withdraw | amount_tao | ported; derives the truncated H160; `"all"` reads the mirror balance (`evm.py:105-115`) | validate, `"all"` resolver | M5 | `intent_evm_withdraw` (:119) |
| 23 | `fund_evm_key` | `FundEvmKey` (:25) | coldkey · signed | Balances.transfer_keep_alive | evm_address, amount_tao | ported; derives the mirror ss58; `"all"` → `transfer_all` (`evm.py:53-60`) | validate, warnings, spend, `"all"` resolver | M5 | `intent_fund_evm_key` (:122) |

### 3.8 `governance`

`PY/intents/governance.py` (235 lines) → `rtao_intents::governance`, feature `governance`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M4, M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 54 | `set_mechanism_count` | `SetMechanismCount` (:166) | coldkey · subnet_owner | AdminUtils.sudo_set_mechanism_count | netuid, mechanism_count | generated | — | M4 | `intent_set_mechanism_count` (:154); §11.4 #3, #10 |
| 63 | `stake_burn` | `StakeBurn` (:93) | coldkey · signed | SubtensorModule.add_stake_burn | netuid, amount_tao | ported; derives `limit` from the alpha price (`governance.py:141-155`) | validate, spend | M5 | `intent_stake_burn` (:172) |
| 75 | `trim_null_subnet_batch` | `TrimNullSubnetBatch` (:61) | coldkey · subnet_owner | AdminUtils.sudo_trim_null_uids_batch | netuid, target | generated | — | M4 | §11.4 #4 |
| 76 | `trim_subnet` | `TrimSubnet` (:17) | coldkey · subnet_owner | AdminUtils.sudo_trim_to_max_allowed_uids | netuid, max_n | generated | — | M4 | `intent_trim_subnet` (:181); §11.4 #10 |
| 82 | `update_symbol` | `UpdateSymbol` (:200) | coldkey · subnet_owner | SubtensorModule.update_symbol | netuid, symbol | generated; `str` → bytes | — | M4 | `intent_update_symbol` (:190); §11.4 #10 |

### 3.9 `hyperparameters`

`PY/intents/hyperparameters.py` (155 lines) → `rtao_intents::hyperparameters`, feature `hyperparameters`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M4.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 52 | `set_hyperparameter` | `SetHyperparameter` (:93) | coldkey · subnet_owner | AdminUtils.sudo_set_* (33 setters, listed in [section 3.21](#321-the-33-setters-of-set_hyperparameter)) | netuid, name, value | ported; selects 1 of 33 setters, unit conversion (`hyperparameters.py:132-152`) | validate | M4 | `intent_set_hyperparameter` (:152); `test_owner_sets_hyperparameter` (:1574); `test_unsettable_hyperparameter_rejected_at_construction` (:1602); §11.4 #5, #10 |

### 3.10 `identity`

`PY/intents/identity.py` (121 lines) → `rtao_intents::identity`, feature `identity`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M4, M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 53 | `set_identity` | `SetIdentity` (:19) | coldkey · signed | SubtensorModule.set_identity | name | generated; `str` → bytes (`.encode()`) | — | M5 | `intent_set_identity` (:153); `test_identity_coldkey_and_subnet` (:1607) |
| 60 | `set_subnet_identity` | `SetSubnetIdentity` (:68) | coldkey · subnet_owner | SubtensorModule.set_subnet_identity | netuid, subnet_name | generated; `str` → bytes | — | M4 | `intent_set_subnet_identity` (:156); `test_identity_coldkey_and_subnet` (:1607); §11.4 #10 |

### 3.11 `leasing`

`PY/intents/leasing.py` (114 lines) → `rtao_intents::leasing`, feature `leasing`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 36 | `register_leased_network` | `RegisterLeasedNetwork` (:23) | coldkey · signed | SubtensorModule.register_leased_network | emissions_share | generated | validate, spend | M5 | `intent_register_leased_network` (:133) |
| 71 | `terminate_lease` | `TerminateLease` (:80) | coldkey · signed | SubtensorModule.terminate_lease | lease_id | generated | warnings | M5 | `intent_terminate_lease` (:177) |

### 3.12 `lock`

`PY/intents/lock.py` (174 lines) → `rtao_intents::lock`, feature `lock`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 26 | `lock_stake` | `LockStake` (:17) | coldkey · signed | SubtensorModule.lock_stake | netuid, amount_alpha | generated; build resolves `"all"` via `_lockable_rao`: hand-written resolver (§4.3) | validate, warnings, `"all"` resolver | M5 | `intent_lock_stake` (:125) |
| 27 | `move_lock` | `MoveLock` (:146) | coldkey · signed | SubtensorModule.move_lock | netuid, destination_hotkey_ss58 | generated | — | M5 | `intent_move_lock` (:126) |
| 56 | `set_perpetual_lock` | `SetPerpetualLock` (:77) | coldkey · signed | SubtensorModule.set_perpetual_lock | netuid, enabled | generated | — | M5 | `intent_set_perpetual_lock` (:155) |
| 57 | `set_reject_locked_alpha` | `SetRejectLockedAlpha` (:110) | coldkey · signed | SubtensorModule.set_reject_locked_alpha | enabled | generated | — | M5 | §11.4 #18 (no upstream e2e test; M5) |

### 3.13 `multisig`

`PY/intents/multisig.py` (536 lines) → `rtao_intents::multisig`, feature `multisig`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 30 | `multisig_approve` | `MultisigApprove` (:403) | coldkey · signed | Multisig.as_multi, Multisig.approve_as_multi | threshold, other_signatories, call | ported; `as_multi` (opening) or `approve_as_multi`; weight estimate; funds check (`multisig.py:403-488`) | warnings, output `MultisigCall` | M5 | `intent_multisig_approve` (:128); §11.4 #12 |
| 31 | `multisig_cancel` | `MultisigCancel` (:493) | coldkey · signed | Multisig.cancel_as_multi | threshold, other_signatories, call, timepoint | ported; hash of the composed inner call (`multisig.py:493-536`) | output `MultisigCall` | M5 | `intent_multisig_cancel` (:129); `test_pending_op_open_read_cancel` (:926) |
| 32 | `multisig_execute` | `MultisigExecute` (:203) | coldkey · signed | Multisig.as_multi | threshold, other_signatories, call | ported; composes the inner intent under the multisig account (`multisig.py:203-273`) | warnings, output `MultisigCall` | M5 | `intent_multisig_execute` (:130); `test_pending_op_open_read_cancel` (:926); `test_account_object_m_of_n_executes` (:982); §11.4 #12 |
| 33 | `multisig_threshold_1` | `MultisigThreshold1` (:167) | coldkey · signed | Multisig.as_multi_threshold_1 | other_signatories, call | ported; in the heuristic's 46 by field names, but `build` derives the multisig account view, composes a nested intent from `call` and sorts signatories (`multisig.py:185-197`, `_account_view` `:59-69`, `_compose_inner` `:71-84`) | output `MultisigCall` | M5 | `intent_multisig_threshold_1` (:131) |

### 3.14 `proxy`

`PY/intents/proxy.py` (320 lines) → `rtao_intents::proxy`, feature `proxy`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 2 | `add_proxy` | `AddProxy` (:74) | coldkey · signed | Proxy.add_proxy | delegate_ss58 | generated | validate, warnings | M5 | `intent_add_proxy` (:98); `test_proxy_flow` (:1057) |
| 16 | `create_pure_proxy` | `CreatePureProxy` (:184) | coldkey · signed | Proxy.create_pure | — | generated | validate, output `PureCreated` (`executor.py:571-593`) | M5 | `intent_create_pure_proxy` (:115); §11.4 #12 |
| 21 | `execute_proxy_announced` | `ExecuteProxyAnnounced` (:274) | coldkey · signed | Proxy.proxy_announced | delegate_ss58, real_ss58, inner_op, inner_args | ported; builds the inner intent against the signing wallet (D-020) | — | M5 | `intent_execute_proxy_announced` (:120); §11.4 #12 |
| 25 | `kill_pure_proxy` | `KillPureProxy` (:224) | coldkey · signed | Proxy.kill_pure | spawner_ss58 | generated | validate | M5 | `intent_kill_pure_proxy` (:124) |
| 38 | `remove_proxies` | `RemoveProxies` (:159) | coldkey · signed | Proxy.remove_proxies | — | generated | warnings | M5 | `intent_remove_proxies` (:135) |
| 39 | `remove_proxy` | `RemoveProxy` (:120) | coldkey · signed | Proxy.remove_proxy | delegate_ss58 | generated | validate | M5 | `intent_remove_proxy` (:136); `test_proxy_flow` (:1057) |

### 3.15 `registration`

`PY/intents/registration.py` (595 lines) → `rtao_intents::registration`, feature `registration`. Facade features that enable it: `owner`, `intents-all`, `full`. Milestones: M4, M5.
`PowSolver` (mining for `pow_register`) is behind the separate feature `pow` (→ core `host`), `pow-gpu` for OpenCL ([§9.6](architecture.md#96-registration-immunity-pow)).

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 9 | `burned_register` | `BurnedRegister` (:116) | coldkey · signed | SubtensorModule.burned_register | netuid | generated; MEV-shield required; burn/lock preview hand-written (§4.3) | effects, spend, shield-required route | M4 | `intent_burned_register` (:105); `test_nonexistent_subnet_maps_to_semantic_code` (:413); `test_spend_cap_blocks_value_movers` (:801); §11.4 #11 |
| 10 | `claim_root` | `ClaimRoot` (:429) | coldkey · signed | SubtensorModule.claim_root | — | generated; `subnets` sent but ignored on chain; the 771-line fee preview stays hand-written (§4.3) | effects¹, warnings¹, blocks¹, preflight¹ | M5 | `intent_claim_root` (:106) |
| 11 | `claim_root_with_hotkey` | `ClaimRootWithHotkey` (:484) | coldkey · signed | SubtensorModule.claim_root_with_hotkey | hotkey_ss58 | generated | effects¹, warnings¹, blocks¹, preflight¹ | M5 | `intent_claim_root_with_hotkey` (:107) |
| 34 | `pow_register` | `PowRegister` (:54) | coldkey · signed | SubtensorModule.pow_register | netuid, work_block, nonce, work_hex | ported; seal/nonce validation; mining fields not in the call (`registration.py:87-101`) | effects | M4 | PYT `e2e/test_pow_registration.py::test_pow_register_unfunded_coldkey_and_owner_toggle` (:20); §11.4 #14 |
| 37 | `register_subnet` | `RegisterSubnet` (:178) | coldkey · signed | SubtensorModule.register_network | — | generated; queued-completion follower is executor-level (§3.13) | spend, completion follower (§3.13); output `SubnetRegistered` | M4 | `intent_register_subnet` (:134); `test_owned_subnet_is_registered` (:1507); `test_spend_cap_blocks_value_movers` (:801); §11.4 #10 |
| 44 | `root_register` | `RootRegister` (:259) | coldkey · signed | SubtensorModule.root_register | — | generated | effects, touches_netuids | M4 | `intent_root_register` (:141); `test_delegation_lifecycle` (:1372) |
| 65 | `start_call` | `StartCall` (:230) | coldkey · subnet_owner | SubtensorModule.start_call | netuid | generated | — | M4 | `intent_start_call` (:173); `test_owned_subnet_is_registered` (:1507); §11.4 #10 |
| 69 | `swap_hotkey` | `SwapHotkey` (:544) | coldkey · signed | SubtensorModule.swap_hotkey | new_hotkey_ss58 | generated; `netuid: Optional[int]` → `Option<NetUid>` | warnings | M5 | `intent_swap_hotkey` (:175) |

### 3.16 `root`

`PY/intents/root.py` (135 lines) → `rtao_intents::root`, feature `root`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 58 | `set_root_claim_threshold` | `SetRootClaimThreshold` (:91) | coldkey · root | SubtensorModule.sudo_set_root_claim_threshold, Sudo.sudo | threshold | ported; near pass-through (rename candidate, section 4.4) | validate | M5 | §11.4 #18 (no upstream e2e test; M5) |
| 59 | `set_subnet_emission_enabled` | `SetSubnetEmissionEnabled` (:24) | coldkey · root | AdminUtils.sudo_set_subnet_emission_enabled, Sudo.sudo | netuids, enabled | ported; one setter per netuid, `batch_all` when several (`root.py:69-79`) | validate, touches_netuids | M5 | §11.4 #18 (no upstream e2e test; M5) |

### 3.17 `serving`

`PY/intents/serving.py` (190 lines) → `rtao_intents::serving`, feature `serving`. Facade features that enable it: `validator`, `miner`, `intents-all`, `full`. Milestones: M2.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 42 | `reset_axon` | `ResetAxon` (:159) | hotkey · signed | SubtensorModule.serve_axon | netuid | ported; constant zero endpoint (`serving.py:175-187`) | — | M2 | `intent_reset_axon` (:139) |
| 45 | `serve_axon` | `ServeAxon` (:28) | hotkey · signed | SubtensorModule.serve_axon | netuid, ip, port | ported; IP string → `u128` + `ip_type` (`serving.py:50-63`) | — | M2 | `intent_serve_axon` (:142); §11.4 #9 |
| 46 | `serve_axon_tls` | `ServeAxonTls` (:71) | hotkey · signed | SubtensorModule.serve_axon_tls | netuid, ip, port, certificate | ported; IP → `u128` + `ip_type`; cert hex → bytes (`serving.py:100-114`) | — | M2 | `intent_serve_axon_tls` (:143); `test_serving_endpoints` (:1525) |
| 47 | `serve_prometheus` | `ServePrometheus` (:122) | hotkey · signed | SubtensorModule.serve_prometheus | netuid, ip, port | ported; IP → `u128` + `ip_type` (`serving.py:141-151`) | — | M2 | `intent_serve_prometheus` (:144); `test_serving_endpoints` (:1525) |

### 3.18 `staking`

`PY/intents/staking.py` (1,925 lines) → `rtao_intents::staking`, feature `staking`. Facade features that enable it: `intents-all`, `full`. Milestones: M5.
The `"all"` arithmetic and slippage limits are ported literally ([§4.4](architecture.md#44-hand-ported-bodies)); `_money.py` maps to `rtao_intents::money`.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 3 | `add_stake` | `AddStake` (:544) | coldkey · signed | SubtensorModule.add_stake, SubtensorModule.add_stake_limit | hotkey_ss58, netuid, amount_tao | ported; `add_stake` or `add_stake_limit` by `slippage_protection`; limit = price × (1 + tol) (`staking.py:544-649`) | validate, warnings, preflight, spend, `"all"` resolver | M5 | `intent_add_stake` (:99); `test_add_stake_increases_stake` (:1350); `test_proxy_flow` (:1057) |
| 4 | `add_stake_limit` | `AddStakeLimit` (:930) | coldkey · signed | SubtensorModule.add_stake_limit | hotkey_ss58, netuid, amount_tao, limit_price_rao | ported; near pass-through (rename candidate, section 4.4) | validate, warnings, spend, `"all"` resolver | M5 | `intent_add_stake_limit` (:100) |
| 28 | `move_stake` | `MoveStake` (:783) | coldkey · signed | SubtensorModule.move_stake, SubtensorModule.move_stake_limit, SubtensorModule.claim_root_with_hotkey, Utility.batch_all | origin_hotkey_ss58, origin_netuid, dest_hotkey_ss58, dest_netuid, amount_alpha | ported; plain or limit move, optional claim-then-move `batch_all` (`staking.py:783-925`) | validate, warnings, preflight¹, touches_netuids, `"all"` resolver | M5 | `intent_move_stake` (:127); `test_netuid_allowlist_blocks_live` (:852) |
| 29 | `move_swap_stake` | `MoveSwapStake` (:1311) | coldkey · signed | SubtensorModule.move_stake, SubtensorModule.swap_stake, SubtensorModule.swap_stake_limit, Utility.batch_all | origin_hotkey_ss58, origin_netuid, dest_hotkey_ss58, dest_netuid, amount_alpha | ported; move then swap (`_compose_move_then_swap`, `staking.py:1311-1411`) | validate, warnings, touches_netuids, `"all"` resolver | M5 | §11.4 #18 (no upstream e2e test; M5) |
| 40 | `remove_stake` | `RemoveStake` (:654) | coldkey · signed | SubtensorModule.remove_stake, SubtensorModule.remove_stake_limit, SubtensorModule.claim_root_with_hotkey, Utility.batch_all | hotkey_ss58, netuid, amount_alpha | ported; plain or limit unstake, optional claim batch (`staking.py:654-778`) | validate, warnings, preflight, `"all"` resolver | M5 | `intent_remove_stake` (:137) |
| 41 | `remove_stake_limit` | `RemoveStakeLimit` (:1005) | coldkey · signed | SubtensorModule.remove_stake_limit, SubtensorModule.remove_stake, SubtensorModule.claim_root_with_hotkey, Utility.batch_all | hotkey_ss58, netuid, amount_alpha, limit_price_rao | ported; limit or (root) plain unstake, optional claim batch (`staking.py:1005-1108`) | validate, warnings, preflight¹, `"all"` resolver | M5 | `intent_remove_stake_limit` (:138) |
| 48 | `set_auto_stake` | `SetAutoStake` (:1607) | coldkey · signed | SubtensorModule.set_coldkey_auto_stake_hotkey | netuid | generated | — | M5 | `intent_set_auto_stake` (:145); `test_auto_stake_destination` (:1667) |
| 64 | `stake_into_basket` | `StakeIntoBasket` (:1644) | coldkey · signed | SubtensorModule.stake_into_basket | hotkey_ss58, amount_tao | ported; near pass-through (rename candidate, section 4.4) | validate, warnings, spend, `"all"` resolver | M5 | §11.4 #18 (no upstream e2e test; M5) |
| 66 | `swap_basket` | `SwapBasket` (:1715) | coldkey · signed | SubtensorModule.swap_basket | hotkey_ss58, origin_netuid, dest_netuid, amount | ported; near pass-through (rename candidate, section 4.4) | validate, touches_netuids, `"all"` resolver | M5 | §11.4 #18 (no upstream e2e test; M5) |
| 67 | `swap_basket_many` | `SwapBasketMany` (:1840) | coldkey · signed | SubtensorModule.swap_basket_many | hotkey_ss58, legs | generated; `legs: list[dict]` → `Vec<(u16, u16, u64, u64)>`: needs a `BasketLeg` field-type override (borderline) | validate, touches_netuids | M5 | §11.4 #18 (no upstream e2e test; M5) |
| 70 | `swap_stake` | `SwapStake` (:1225) | coldkey · signed | SubtensorModule.swap_stake, SubtensorModule.swap_stake_limit | hotkey_ss58, origin_netuid, dest_netuid, amount_alpha | ported; plain or limit swap (`_compose_swap_stake`, `staking.py:1225-1306`) | validate, warnings, touches_netuids, `"all"` resolver | M5 | `intent_swap_stake` (:176) |
| 74 | `transfer_stake` | `TransferStake` (:1416) | coldkey · signed | SubtensorModule.transfer_stake, SubtensorModule.transfer_stake_and_hotkey | dest_coldkey_ss58, hotkey_ss58, origin_netuid, dest_netuid, amount_alpha | ported; `transfer_stake` or `transfer_stake_and_hotkey` (`staking.py:1416-1602`) | validate, warnings, spend, touches_netuids, `"all"` resolver | M5 | `intent_transfer_stake` (:180); `test_spend_cap_blocks_value_movers` (:801) |
| 77 | `unstake_all` | `UnstakeAll` (:1113) | coldkey · signed | SubtensorModule.unstake_all, SubtensorModule.claim_root_with_hotkey, Utility.batch_all | hotkey_ss58 | ported; optional claim-then-unstake batch (`staking.py:1113-1183`) | warnings, preflight¹, affects_all_subnets | M5 | `intent_unstake_all` (:182) |
| 78 | `unstake_all_alpha` | `UnstakeAllAlpha` (:1188) | coldkey · signed | SubtensorModule.unstake_all_alpha | hotkey_ss58 | generated | warnings, affects_all_subnets | M5 | `intent_unstake_all_alpha` (:183); `test_netuid_allowlist_blocks_live` (:852) |

### 3.19 `transfer`

`PY/intents/transfer.py` (116 lines) → `rtao_intents::transfer`, feature `transfer`. Facade features that enable it: `intents-all`, `full`. Milestones: M2.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 72 | `transfer` | `Transfer` (:21) | coldkey · signed | Balances.transfer_keep_alive, Balances.transfer_allow_death, Balances.transfer_all | dest_ss58, amount_tao | ported; selects among 3 calls by `"all"`/`keep_alive` (`transfer.py:52-63`) | validate, warnings, spend, `"all"` resolver | M2 | `intent_transfer` (:178); `test_transfer_moves_exactly_the_amount` (:1334); `test_submit_shielded_runs_full_pipeline` (:313); `test_plan_simulates_real_fee` (:776); `test_policy_blocks_with_live_fee` (:787); §11.4 #17 |
| 73 | `transfer_all` | `TransferAll` (:83) | coldkey · signed | Balances.transfer_all | dest_ss58 | generated; not in the heuristic's 46 (Balances absent from its call map); `transfer.py:103-106` is one call, fields 1:1 | warnings, spend | M2 | `intent_transfer_all` (:179) |

### 3.20 `weights`

`PY/intents/weights.py` (563 lines) → `rtao_intents::weights`, feature `weights`. Facade features that enable it: `validator`, `intents-all`, `full`. Milestones: M2.

| # | op | class = Rust struct (py) | signer · origin | wraps | required params | body | hand-written hooks | M | e2e |
|---:|---|---|---|---|---|---|---|---|---|
| 13 | `commit_weights` | `CommitWeights` (:444) | hotkey · signed | SubtensorModule.commit_timelocked_mechanism_weights | netuid | ported; preflight, conform, timelock encrypt (`weights.py:483-501`) | validate, warnings, output `WeightsSubmitted { reveal_round }` | M2 | `intent_commit_weights` (:112) |
| 43 | `reveal_weights` | `RevealWeights` (:515) | hotkey · signed | SubtensorModule.reveal_weights | netuid, uids, weights, salt | ported; `normalize()` before the call (`weights.py:548-560`) | — | M2 | `intent_reveal_weights` (:140) |
| 62 | `set_weights` | `SetWeights` (:351) | hotkey · signed | SubtensorModule.set_mechanism_weights, SubtensorModule.commit_timelocked_mechanism_weights | netuid | ported; preflight, conform, CR path selection, timelock (`weights.py:393-421`) | validate, effects, warnings, output `WeightsSubmitted { reveal_round }` | M2 | `intent_set_weights` (:171); PYT `e2e/test_null_consensus.py::test_null_yuma_null_emissions_via_python_sdk` (:22); §11.4 #1, #2, #3, #4, #5, #6, #7 |

### 3.21 The 33 setters of `set_hyperparameter`

`AdminUtils.` + `sudo_set_epoch_consensus`, `sudo_set_tempo`, `sudo_set_immunity_period`, `sudo_set_min_allowed_weights`, `sudo_set_weights_version_key`, `sudo_set_activity_cutoff_factor`, `sudo_set_min_burn`, `sudo_set_max_burn`, `sudo_set_bonds_moving_average`, `sudo_set_bonds_penalty`, `sudo_set_serving_rate_limit`, `sudo_set_commit_reveal_weights_interval`, `sudo_set_max_allowed_uids`, `sudo_set_burn_increase_mult`, `sudo_set_burn_half_life`, `sudo_set_collateral_lock_share`, `sudo_set_collateral_drain_ratio`, `sudo_set_adjustment_alpha`, `sudo_set_rho`, `sudo_set_max_difficulty`, `sudo_set_alpha_sigmoid_steepness`, `sudo_set_min_childkey_take_per_subnet`, `sudo_set_owner_immune_neuron_limit`, `sudo_set_alpha_values`, `sudo_set_commit_reveal_weights_enabled`, `sudo_set_liquid_alpha_enabled`, `sudo_set_network_registration_allowed`, `sudo_set_network_pow_registration_allowed`, `sudo_set_yuma3_enabled`, `sudo_set_bonds_reset_enabled`, `sudo_set_toggle_transfer`, `sudo_set_owner_cut_enabled`, `sudo_set_owner_cut_auto_lock_enabled`. Order and content equal
`OWNER_HYPERPARAMETERS` (`PY/intents/hyperparameters.py:31-66`; 34 names, `alpha_low` and `alpha_high` share
`sudo_set_alpha_values`), which the extractor reads ([§4.1](architecture.md#41-where-each-piece-comes-from)).

## 4. Generated pass-through bodies: which 46

### 4.1 The set

Generated `build()` (46): `add_proxy`, `associate_evm_key`, `associate_hotkey`, `burned_register`, `claim_root`, `claim_root_with_hotkey`, `clear_coldkey_swap_announcement`, `contribute_crowdloan`, `create_pure_proxy`, `decrease_take`, `dispute_coldkey_swap`, `dissolve_crowdloan`, `finalize_crowdloan`, `increase_take`, `kill_pure_proxy`, `lock_stake`, `move_lock`, `refund_crowdloan`, `register_leased_network`, `register_subnet`, `remove_proxies`, `remove_proxy`, `root_register`, `set_auto_stake`, `set_childkey_take`, `set_children`, `set_crowdloan_max_contribution`, `set_identity`, `set_mechanism_count`, `set_perpetual_lock`, `set_reject_locked_alpha`, `set_subnet_identity`, `start_call`, `swap_basket_many`, `swap_coldkey_announced`, `swap_hotkey`, `terminate_lease`, `transfer_all`, `trim_null_subnet_batch`, `trim_subnet`, `unstake_all_alpha`, `update_crowdloan_cap`, `update_crowdloan_end`, `update_crowdloan_min_contribution`, `update_symbol`, `withdraw_crowdloan`.

By milestone: M2 1 (`transfer_all`), M4 9 (`burned_register`, `register_subnet`,
`root_register`, `set_mechanism_count`, `set_subnet_identity`, `start_call`, `trim_null_subnet_batch`, `trim_subnet`,
`update_symbol`), M5 36. Ported (37): `add_collateral`, `add_stake`, `add_stake_limit`, `announce_coldkey_swap`, `batch`, `commit_weights`, `create_crowdloan`, `evm_withdraw`, `execute_proxy_announced`, `fund_evm_key`, `move_stake`, `move_swap_stake`, `multisig_approve`, `multisig_cancel`, `multisig_execute`, `multisig_threshold_1`, `pow_register`, `remove_stake`, `remove_stake_limit`, `reset_axon`, `reveal_weights`, `serve_axon`, `serve_axon_tls`, `serve_prometheus`, `set_hyperparameter`, `set_min_collateral`, `set_root_claim_threshold`, `set_subnet_emission_enabled`, `set_take`, `set_weights`, `stake_burn`, `stake_into_basket`, `swap_basket`, `swap_stake`, `transfer`, `transfer_stake`, `unstake_all`.

Under the stricter reading "maps arguments to one call with no hook logic beyond the generic defaults", the 46 split
into three tiers:

- 19 with no hand-written hook except `summary()`: `associate_evm_key`, `associate_hotkey`, `clear_coldkey_swap_announcement`, `dissolve_crowdloan`, `finalize_crowdloan`, `move_lock`, `refund_crowdloan`, `set_auto_stake`, `set_identity`, `set_mechanism_count`, `set_perpetual_lock`, `set_reject_locked_alpha`, `set_subnet_identity`, `start_call`, `trim_null_subnet_batch`, `trim_subnet`, `update_crowdloan_end`, `update_symbol`, `withdraw_crowdloan`;
- 8 that add only `validate()` (ported `__post_init__` normalization): `decrease_take`, `increase_take`, `kill_pure_proxy`, `remove_proxy`, `set_childkey_take`, `set_crowdloan_max_contribution`, `update_crowdloan_cap`, `update_crowdloan_min_contribution`;
- 19 that keep preflight hooks, `"all"` resolution, the shield route or executor post-processing: `add_proxy`, `burned_register`, `claim_root`, `claim_root_with_hotkey`, `contribute_crowdloan`, `create_pure_proxy`, `dispute_coldkey_swap`, `lock_stake`, `register_leased_network`, `register_subnet`, `remove_proxies`, `root_register`, `set_children`, `swap_basket_many`, `swap_coldkey_announced`, `swap_hotkey`, `terminate_lease`, `transfer_all`, `unstake_all_alpha`.

### 4.2 Differences from the heuristic behind "46"

The architecture's count comes from a field-name heuristic re-run by the maintainability judge [J-MA §3.4]. Checked
against the `build()` bodies, it misclassifies two ops in opposite directions, so the total stays 46:

| Op | Heuristic | This inventory | Evidence |
|---|---|---|---|
| `multisig_threshold_1` | pass-through (field names `other_signatories`, `call` match) | ported | `build` derives the multisig account view, composes the nested intent given in `call`, and sorts signatories by raw account id (`PY/intents/multisig.py:185-197`, `_account_view` `:59-69`, `_compose_inner` `:71-84`) |
| `transfer_all` | complex ("call-not-in-map") | generated | one call `Balances.transfer_all(dest, keep_alive)` from fields `dest_ss58`, `keep_alive` (`PY/intents/transfer.py:103-106`); the heuristic's call map has no `Balances` pallet, so it could not compare |

The heuristic's call map also lacks `EVM`, which is why `evm_withdraw` and `fund_evm_key` were "call-not-in-map";
both are ported anyway (derived addresses).

### 4.3 Borderline members of the 46

| Op | Why it is still generated |
|---|---|
| `associate_evm_key` | `signature` 0x-hex → bytes: field-type override (`association.py:84-92`) |
| `swap_basket_many` | `legs: list[dict]` → `Vec<(u16, u16, u64, u64)>`: needs a `BasketLeg` field-type override (borderline) |
| `set_children` | proportions → `U64Normalized`; validation stays hand-written (§4.3) |
| `decrease_take` | `take` arrives as int, float or str and is normalized to u16 in `__post_init__` (`PY/intents/children.py`); a `Ratio → PerU16` field type plus `validate()` reproduces it (same for `increase_take`, `set_childkey_take`) |
| `contribute_crowdloan` | build resolves `"all"` via `_stakeable_rao`: generated build calls a hand-written resolver (§4.3) |
| `lock_stake` | build resolves `"all"` via `_lockable_rao`: hand-written resolver (§4.3) |
| `claim_root` | `subnets` sent but ignored on chain; the 771-line fee preview stays hand-written (§4.3) |
| `burned_register` | MEV-shield required; burn/lock preview hand-written (§4.3) |
| `register_subnet` | queued-completion follower is executor-level (§3.13) |
| `create_pure_proxy` | the call is 1:1; the `PureCreated` output is extracted by the executor from events (§4.2), not by `build` |

### 4.4 Near pass-through: rename candidates

[§4.3](architecture.md#43-generated-pass-through-bodies-and-their-limits) allows `codegen/intent_callmap.toml` renames
to move near-pass-through intents into the generated set. Five ported ops compose a single call and differ from
pass-through only by renames or constants. They stay ported in this inventory, because the architecture names the
mechanism but does not move any op; each rename would raise the generated count by one.

| Op | What a callmap entry would have to express |
|---|---|
| `add_stake_limit` | `amount_tao`→`amount_staked` (`limit_price_rao`→`limit_price` is covered by `_rao` stripping); `"all"` resolver (`staking.py:968-981`) |
| `stake_into_basket` | `amount_tao`→`amount_staked`; `"all"` resolver (`staking.py:1684-1694`) |
| `swap_basket` | `dest_netuid`→`destination_netuid` (the §4.3 example); `"all"` resolver `_origin_holding_rao` (`staking.py:1808-1826`) |
| `set_min_collateral` | `min_alpha`→`min_locked`; Python composes it with a raw `Call(...)` (`collateral.py:165-173`) |
| `set_root_claim_threshold` | constant `netuid=0`, `threshold`→`new_value`; executor adds `Sudo.sudo` (`root.py:126-132`) |

## 5. Reads (102)

One subsection per catalog category; each category is one namespace type in `rtao-reads`
([§2.2](architecture.md#22-crates), [§4.8](architecture.md#48-reads)), reached as `client.<ns>()` on `Client` and
`Snapshot`. The function name equals the read name, so Python's `sub.<ns>.<read>(…)` becomes
`client.<ns>().<read>(…)`, and the generated `ReadName`/`dispatch` covers all 102. The hand-written body lives in
`crates/rtao-reads/src/reads/<module>.rs`, one file per `PY/reads/<module>.py`; column "py" gives that module and
line. The 11 reads of `PY/reads/root.py` belong to the `Staking` category.

Columns: **hits** are the catalog `hits` (S = storage, R = runtime API, C = constant); **body** is `scalar` for the
9 `scalar_read` registrations that §4.8 generates whole, otherwise `ported`; **typed model** is the chain-side model
of [§8.1](architecture.md#81-typed-models-by-name-decodeastype-over-the-cores-registry) the read decodes through
("—" means primitives and newtypes from generated descriptors suffice; "DynValue" means a composite with no §8.1
model, which §3.7 decodes dynamically). Every read also needs a `Serialize` output model with Python's field names
(§4.8); column "out" gives Python's return annotation.

### 5.1 Accounts & keys: `Balances` (`client.balances()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 1 | `balance` | coldkey_ss58: string | `sub.balances.balance(...)` → `Balances::balance`; `Balances::get` (`namespaces.py:104`) | `accounts.py:20` | S `System.Account` | ported · DynValue (`AccountInfo`; not in §8.1) | `Balance` | M4 | CLI v0 `wallet balance`, `wallet inspect` (`cli/helpers.py:478, 791`) |
| 2 | `balances` | coldkey_ss58s: array | `sub.balances.balances(...)` → `Balances::balances`; `Balances::get_many` (`:108`) | `accounts.py:32` | S `System.Account` | ported · DynValue (`AccountInfo`; not in §8.1) | `dict[str, Balance]` | M4 | CLI v0 `wallet balance --all` (`cli/helpers.py:415`) |
| 3 | `coldkey_swap_announcement` | coldkey_ss58: string | `sub.balances.coldkey_swap_announcement(...)` → `Balances::coldkey_swap_announcement` | `accounts.py:77` | S `SubtensorModule.ColdkeySwapAnnouncements`; S `SubtensorModule.ColdkeySwapDisputes` | ported · — | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 4 | `existential_deposit` | — | `sub.balances.existential_deposit(...)` → `Balances::existential_deposit`; same name, curated (`:114`) | `accounts.py:42` | C `Balances.ExistentialDeposit` | ported · — | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 5 | `multisig` | account_ss58: string, call_hash: string | `sub.balances.multisig(...)` → `Balances::multisig` | `accounts.py:107` | S `Multisig.Multisigs` | ported · `MultisigInfo` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 6 | `proxies` | coldkey_ss58: string | `sub.balances.proxies(...)` → `Balances::proxies` | `accounts.py:54` | S `Proxy.Proxies` | ported · `ProxyDefinition` | `dict` | M5 | CLI M5 `proxy list` (`cli/commands/proxy.py:301`) |

### 5.2 Chain: `Chain` (`client.chain()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 7 | `block_info` | block: integer | `sub.chain.block_info(...)` → `Chain::block_info` | `chain.py:37` | — (none) | ported · — | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 8 | `block_time` | — | `sub.chain.block_time(...)` → `Chain::block_time` | `chain.py:20` | — (none) | ported · — | `float` | M2 | TimelockCommitter takes block time from `Aura.SlotDuration` (§9.3 step 7) [I] |
| 9 | `is_fast_blocks` | — | `sub.chain.is_fast_blocks(...)` → `Chain::is_fast_blocks` | `chain.py:26` | — (none) | ported · — | `bool` | M6 | default: §14 M6 "all 102 reads" |
| 10 | `mev_shield_next_key` | — | `sub.chain.mev_shield_next_key(...)` → `Chain::mev_shield_next_key` | `chain.py:63` | S `MevShield.NextKey` | ported · — | `Optional[str]` | M4 | shield pipeline reads `MevShield.NextKey` (§3.11 step 1); CLI v0 downgrade check (`cli/context.py:1436`) |
| 11 | `timestamp` | — | `sub.chain.timestamp(...)` → `Chain::timestamp` | `chain.py:12` | — (none) | ported · — | `dict` | M2 | `ClockGuard` reads `Timestamp.Now` (§9.3 step 10) [I] |
| 12 | `tx_rate_limit` | — | `sub.chain.tx_rate_limit(...)` → `Chain::tx_rate_limit` | `chain.py:71` | S `SubtensorModule.TxRateLimit` | scalar · — | `int` | M6 | default: §14 M6 "all 102 reads" |

### 5.3 Delegation: `Delegation` (`client.delegation()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 13 | `childkey_threshold_suspended` | hotkey_ss58: string | `sub.delegation.childkey_threshold_suspended(...)` → `Delegation::childkey_threshold_suspended` | `delegation.py:224` | S `SubtensorModule.ChildkeyThresholdSuspended` | ported · — | `bool` | M6 | default: §14 M6 "all 102 reads" |
| 14 | `children` | hotkey_ss58: string, netuid: integer | `sub.delegation.children(...)` → `Delegation::children` | `delegation.py:156` | S `SubtensorModule.ChildKeys` | ported · — | `list[tuple[int, str]]` | M6 | default: §14 M6 "all 102 reads" |
| 15 | `delegate` | hotkey_ss58: string | `sub.delegation.delegate(...)` → `Delegation::delegate` | `delegation.py:80` | R `DelegateInfoRuntimeApi.get_delegate` | ported · `DelegateInfo` | `Optional[DelegateInfo]` | M6 | default: §14 M6 "all 102 reads" |
| 16 | `delegate_take` | hotkey_ss58: string | `sub.delegation.delegate_take(...)` → `Delegation::delegate_take` | `delegation.py:104` | S `SubtensorModule.Delegates`; S `SubtensorModule.MinDelegateTake`; S `SubtensorModule.MaxDelegateTake` | ported · — | `dict` | M4 | CLI v0 `hparams get-take` (`cli/commands/sudo.py:168`) |
| 17 | `delegated` | coldkey_ss58: string | `sub.delegation.delegated(...)` → `Delegation::delegated` | `delegation.py:130` | R `DelegateInfoRuntimeApi.get_delegated` | ported · `DelegateInfo` | `list[DelegatedStake]` | M4 | CLI v0 `wallet inspect` (`cli/helpers.py:785`) |
| 18 | `delegates` | — | `sub.delegation.delegates(...)` → `Delegation::delegates` | `delegation.py:68` | R `DelegateInfoRuntimeApi.get_delegates` | ported · `DelegateInfo` | `list[DelegateInfo]` | M6 | default: §14 M6 "all 102 reads" |
| 19 | `is_delegate` | hotkey_ss58: string | `sub.delegation.is_delegate(...)` → `Delegation::is_delegate` | `delegation.py:92` | R `DelegateInfoRuntimeApi.get_delegate` | ported · `DelegateInfo` | `bool` | M6 | default: §14 M6 "all 102 reads" |
| 20 | `parents` | hotkey_ss58: string, netuid: integer | `sub.delegation.parents(...)` → `Delegation::parents` | `delegation.py:178` | S `SubtensorModule.ParentKeys` | ported · — | `list[tuple[int, str]]` | M6 | default: §14 M6 "all 102 reads" |
| 21 | `pending_children` | hotkey_ss58: string, netuid: integer | `sub.delegation.pending_children(...)` → `Delegation::pending_children` | `delegation.py:200` | S `SubtensorModule.PendingChildKeys` | ported · — | `dict` | M6 | default: §14 M6 "all 102 reads" |

### 5.4 Epochs & timing: `Epochs` (`client.epochs()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 22 | `blocks_since_last_step` | netuid: integer | `sub.epochs.blocks_since_last_step(...)` → `Epochs::blocks_since_last_step` | `epochs.py:50` | S `SubtensorModule.BlocksSinceLastStep` | scalar · — | `int` | M2 | WeightScheduler preflight item (§9.3 step 2) |
| 23 | `blocks_since_last_update` | netuid: integer, uid: integer | `sub.epochs.blocks_since_last_update(...)` → `Epochs::blocks_since_last_update` | `epochs.py:68` | S `SubtensorModule.LastUpdate` | ported · — | `Optional[int]` | M2 | WeightScheduler preflight item `LastUpdate[idx]` (§9.3 step 2) |
| 24 | `blocks_until_next_epoch` | netuid: integer | `sub.epochs.blocks_until_next_epoch(...)` → `Epochs::blocks_until_next_epoch` | `epochs.py:37` | R `SubnetInfoRuntimeApi.get_next_epoch_start_block` | ported · — | `Optional[int]` | M4 | planner admin-freeze-window check (§9.7) [I] |
| 25 | `epoch_status` | netuid: integer | `sub.epochs.epoch_status(...)` → `Epochs::epoch_status` | `epochs.py:90` | S `SubtensorModule.Tempo`; S `SubtensorModule.LastEpochBlock`; S `SubtensorModule.BlocksSinceLastStep`; S `SubtensorModule.PendingEpochAt`; S `SubtensorModule.SubnetEpochIndex`; R `SubnetInfoRuntimeApi.get_next_epoch_start_block` | ported · `EpochStatus` (rtao-client, §2.2) | `dict` | M2 | `EpochClock`/`EpochStatus` (§9.1, §2.2); §13 catalog example returns `EpochStatus` |
| 26 | `next_epoch_start_block` | netuid: integer | `sub.epochs.next_epoch_start_block(...)` → `Epochs::next_epoch_start_block` | `epochs.py:19` | R `SubnetInfoRuntimeApi.get_next_epoch_start_block` | ported · — | `Optional[int]` | M4 | planner admin-freeze-window check (§9.7) [I] |

### 5.5 Hyperparameters: `Hyperparameters` (`client.hyperparameters()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 27 | `difficulty` | netuid: integer | `sub.hyperparameters.difficulty(...)` → `Hyperparameters::difficulty` | `hyperparameters.py:18` | S `SubtensorModule.Difficulty` | scalar · — | `int` | M4 | `PowSolver` mines against `Difficulty` (§9.6) [I]; Python reads the storage item directly (`pow_registration.py:144-157`) |
| 28 | `immunity_period` | netuid: integer | `sub.hyperparameters.immunity_period(...)` → `Hyperparameters::immunity_period` | `hyperparameters.py:37` | S `SubtensorModule.ImmunityPeriod` | scalar · — | `int` | M3 | `PruneRisk` gauge `rtao_immunity_blocks_remaining` (§9.4) [I] |
| 29 | `max_weight_limit` | netuid: integer | `sub.hyperparameters.max_weight_limit(...)` → `Hyperparameters::max_weight_limit` | `hyperparameters.py:30` | S `SubtensorModule.MaxWeightsLimit` | scalar · — | `int` | M2 | conform clip (§9.3 steps 2, 4) |
| 30 | `min_allowed_weights` | netuid: integer | `sub.hyperparameters.min_allowed_weights(...)` → `Hyperparameters::min_allowed_weights` | `hyperparameters.py:23` | S `SubtensorModule.MinAllowedWeights` | scalar · — | `int` | M2 | conform minimum (§9.3 steps 2, 4) |
| 31 | `reveal_period` | netuid: integer | `sub.hyperparameters.reveal_period(...)` → `Hyperparameters::reveal_period` | `hyperparameters.py:42` | S `SubtensorModule.RevealPeriodEpochs` | scalar · — | `int` | M2 | reveal-round prediction (§9.3 steps 2, 7) |
| 32 | `subnet_emission_enabled` | netuid: integer | `sub.hyperparameters.subnet_emission_enabled(...)` → `Hyperparameters::subnet_emission_enabled` | `hyperparameters.py:47` | S `SubtensorModule.SubnetEmissionEnabled` | scalar · — | `int` | M4 | `OwnerMonitor` exports `SubnetEmissionEnabled` (§9.7) |
| 33 | `weights_rate_limit` | netuid: integer | `sub.hyperparameters.weights_rate_limit(...)` → `Hyperparameters::weights_rate_limit` | `hyperparameters.py:13` | S `SubtensorModule.WeightsSetRateLimit` | scalar · — | `int` | M2 | rate-limit scheduling (§9.3 steps 1-2) |

### 5.6 Identity & commitments: `Identity` (`client.identity()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 34 | `commitment` | netuid: integer, hotkey_ss58: string | `sub.identity.commitment(...)` → `Identity::commitment` | `identity.py:90` | S `Commitments.CommitmentOf` | ported · `CommitmentRegistration` + `Data` | `Optional[Commitment]` | M6 | default: §14 M6 "all 102 reads" |
| 35 | `commitments` | netuid: integer | `sub.identity.commitments(...)` → `Identity::commitments` | `identity.py:139` | — (none) | ported in `rtao_reads::commitments` (§4.8) · `CommitmentRegistration` + `Data` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 36 | `hotkey_identities` | hotkey_ss58s: array | `sub.identity.hotkey_identities(...)` → `Identity::hotkey_identities` | `identity.py:43` | S `SubtensorModule.Owner`; S `SubtensorModule.IdentitiesV2` | ported · `ChainIdentityV2` | `dict[str, dict]` | M4 | CLI v0 `wallet inspect` hotkey names (`cli/helpers.py:135`) |
| 37 | `identity` | coldkey_ss58: string | `sub.identity.identity(...)` → `Identity::identity` | `identity.py:31` | S `SubtensorModule.IdentitiesV2` | ported · `ChainIdentityV2` | `Optional[dict]` | M4 | CLI v0 `wallet inspect` (`cli/helpers.py:786`) |
| 38 | `revealed_commitment` | netuid: integer, hotkey_ss58: string | `sub.identity.revealed_commitment(...)` → `Identity::revealed_commitment` | `identity.py:113` | S `Commitments.RevealedCommitments` | ported · — | `Any` | M6 | default: §14 M6 "all 102 reads" |

### 5.7 Leases & crowdloans: `Leasing` (`client.leasing()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 39 | `crowdloan` | crowdloan_id: integer | `sub.leasing.crowdloan(...)` → `Leasing::crowdloan` | `leasing.py:55` | S `Crowdloan.Crowdloans` | ported · `CrowdloanInfo` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 40 | `crowdloan_contributors` | crowdloan_id: integer | `sub.leasing.crowdloan_contributors(...)` → `Leasing::crowdloan_contributors` | `leasing.py:103` | S `Crowdloan.Contributions` | ported · — | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 41 | `crowdloans` | — | `sub.leasing.crowdloans(...)` → `Leasing::crowdloans` | `leasing.py:78` | S `Crowdloan.Crowdloans` | ported · `CrowdloanInfo` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 42 | `lease` | lease_id: integer | `sub.leasing.lease(...)` → `Leasing::lease` | `leasing.py:32` | S `SubtensorModule.SubnetLeases` | ported · `SubnetLease` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 43 | `leases` | — | `sub.leasing.leases(...)` → `Leasing::leases` | `leasing.py:43` | S `SubtensorModule.SubnetLeases` | ported · `SubnetLease` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |

### 5.8 Locks & conviction: `Locks` (`client.locks()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 44 | `accepts_locked_alpha` | coldkey_ss58: string | `sub.locks.accepts_locked_alpha(...)` → `Locks::accepts_locked_alpha` | `locks.py:124` | S `SubtensorModule.AccountFlags` | ported · — | `bool` | M6 | default: §14 M6 "all 102 reads" |
| 45 | `coldkey_lock` | coldkey_ss58: string, netuid: integer | `sub.locks.coldkey_lock(...)` → `Locks::coldkey_lock` | `locks.py:61` | S `SubtensorModule.Lock` | ported · `LockState` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 46 | `hotkey_conviction` | hotkey_ss58: string, netuid: integer | `sub.locks.hotkey_conviction(...)` → `Locks::hotkey_conviction` | `locks.py:144` | R `StakeInfoRuntimeApi.get_hotkey_conviction` | ported · — | `dict` | M6 | default: §14 M6 "all 102 reads" |
| 47 | `locks_for_coldkey` | coldkey_ss58: string | `sub.locks.locks_for_coldkey(...)` → `Locks::locks_for_coldkey` | `locks.py:80` | S `SubtensorModule.Lock` | ported · `LockState` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 48 | `locks_for_hotkey` | hotkey_ss58: string, netuid: integer | `sub.locks.locks_for_hotkey(...)` → `Locks::locks_for_hotkey` | `locks.py:99` | S `SubtensorModule.LockingColdkeys` | ported · `LockState` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 49 | `most_convicted_hotkey` | netuid: integer | `sub.locks.most_convicted_hotkey(...)` → `Locks::most_convicted_hotkey` | `locks.py:163` | R `StakeInfoRuntimeApi.get_most_convicted_hotkey_on_subnet` | ported · — | `Optional[str]` | M4 | `OwnerMonitor` conviction leader (§9.7) [I] |
| 50 | `subnet_convictions` | netuid: integer | `sub.locks.subnet_convictions(...)` → `Locks::subnet_convictions` | `locks.py:295` | S `SubtensorModule.HotkeyLock`; S `SubtensorModule.DecayingHotkeyLock`; S `SubtensorModule.OwnerLock`; S `SubtensorModule.DecayingOwnerLock`; S `SubtensorModule.SubnetOwnerHotkey`; S `SubtensorModule.SubnetAlphaOut`; S `SubtensorModule.SubnetProtocolAlpha`; S `AlphaAssets.AlphaBurned`; S `SubtensorModule.UnlockRate`; S `SubtensorModule.MaturityRate`; S `SubtensorModule.NetworkRegisteredAt` | ported · `LockState` | `dict` | M6 | default: §14 M6 "all 102 reads" |

### 5.9 Mining & collateral: `Collateral` (`client.collateral()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 51 | `collateral_policy` | netuid: integer | `sub.collateral.collateral_policy(...)` → `Collateral::collateral_policy` | `collateral.py:138` | — (none) | ported · — | `dict` | M6 | default: §14 M6 "all 102 reads" |
| 52 | `miner_collateral` | netuid: integer, hotkey_ss58: string, coldkey_ss58: string | `sub.collateral.miner_collateral(...)` → `Collateral::miner_collateral` | `collateral.py:67` | S `SubtensorModule.Owner` | ported · `MinerCollateralState` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 53 | `subnet_collateral` | netuid: integer | `sub.collateral.subnet_collateral(...)` → `Collateral::subnet_collateral` | `collateral.py:102` | S `SubtensorModule.Uids` | ported · `MinerCollateralState` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |

### 5.10 Neurons & registration: `Neurons` (`client.neurons()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 54 | `associated_evm_key` | netuid: integer, uid: integer | `sub.neurons.associated_evm_key(...)` → `Neurons::associated_evm_key` | `neurons.py:129` | S `SubtensorModule.AssociatedEvmAddress` | ported · — | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 55 | `hotkey_owner` | hotkey_ss58: string | `sub.neurons.hotkey_owner(...)` → `Neurons::hotkey_owner` | `neurons.py:75` | S `SubtensorModule.Owner` | ported · `DefaultAccount` override (D-001) | `Optional[str]` | M6 | default: §14 M6 "all 102 reads" |
| 56 | `netuids_for_hotkey` | hotkey_ss58: string | `sub.neurons.netuids_for_hotkey(...)` → `Neurons::netuids_for_hotkey` | `neurons.py:99` | S `SubtensorModule.IsNetworkMember` | ported · — | `list[int]` | M6 | default: §14 M6 "all 102 reads" |
| 57 | `neurons` | netuid: integer, lite: boolean | `sub.neurons.neurons(...)` → `Neurons::neurons`; `Neurons::all` (`:205`) | `neurons.py:41` | R `NeuronInfoRuntimeApi.get_neurons_lite`; R `NeuronInfoRuntimeApi.get_neurons` | ported · `NeuronInfoLite`, `NeuronInfo` (+ `AxonInfo`, `PrometheusInfo`) | `list[Neuron]` | M6 | default: §14 M6 "all 102 reads" |
| 58 | `owned_hotkeys` | coldkey_ss58: string | `sub.neurons.owned_hotkeys(...)` → `Neurons::owned_hotkeys` | `neurons.py:87` | S `SubtensorModule.OwnedHotkeys` | ported · — | `list[str]` | M6 | default: §14 M6 "all 102 reads" |
| 59 | `uid` | hotkey_ss58: string, netuid: integer | `sub.neurons.uid(...)` → `Neurons::uid` | `neurons.py:114` | S `SubtensorModule.Uids` | ported · — | `Optional[int]` | M2 | not-registered refusal (§9.3 step 2); `root_register` confirmation names this read (§9.6) |

### 5.11 Prices & swaps: `Prices` (`client.prices()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 60 | `alpha_price` | netuid: integer | `sub.prices.alpha_price(...)` → `Prices::alpha_price` | `prices.py:88` | R `SwapRuntimeApi.current_alpha_price` | ported · — | `dict` | M6 | default: §14 M6 "all 102 reads" |
| 61 | `alpha_prices` | — | `sub.prices.alpha_prices(...)` → `Prices::alpha_prices` | `prices.py:76` | R `SwapRuntimeApi.current_alpha_price_all` | ported · `SubnetPrice` | `dict[int, float]` | M6 | default: §14 M6 "all 102 reads" |
| 62 | `quote_stake` | netuid: integer, amount_tao: number | `sub.prices.quote_stake(...)` → `Prices::quote_stake` | `prices.py:45` | R `SwapRuntimeApi.sim_swap_tao_for_alpha` | ported · `SimSwapResult` | `SwapQuote` | M6 | default: §14 M6 "all 102 reads" |
| 63 | `quote_unstake` | netuid: integer, amount_alpha: number | `sub.prices.quote_unstake(...)` → `Prices::quote_unstake` | `prices.py:66` | R `SwapRuntimeApi.sim_swap_alpha_for_tao` | ported · `SimSwapResult` | `SwapQuote` | M6 | default: §14 M6 "all 102 reads" |

### 5.12 Staking: `Staking` (`client.staking()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 64 | `auto_stake` | coldkey_ss58: string, netuid: integer | `sub.staking.auto_stake(...)` → `Staking::auto_stake` | `staking.py:300` | S `SubtensorModule.AutoStakeDestination` | ported · — | `Optional[str]` | M6 | default: §14 M6 "all 102 reads" |
| 65 | `auto_stake_all` | coldkey_ss58: string | `sub.staking.auto_stake_all(...)` → `Staking::auto_stake_all` | `staking.py:312` | S `SubtensorModule.AutoStakeDestination` | ported · — | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 66 | `basket_position` | hotkey_ss58: string, coldkey_ss58: string | `sub.staking.basket_position(...)` → `Staking::basket_position` | `root.py:246` | — (none) | ported · `BasketPosition` | `Optional[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 67 | `basket_trading_status` | hotkey_ss58: string | `sub.staking.basket_trading_status(...)` → `Staking::basket_trading_status` | `root.py:395` | — (none) | ported · DynValue (`BasketTradingStatus`; not in §8.1) | `dict` | M6 | default: §14 M6 "all 102 reads" |
| 68 | `root_basket_owed` | coldkey_ss58: string | `sub.staking.root_basket_owed(...)` → `Staking::root_basket_owed` | `root.py:172` | R `BetaBasketRuntimeApi.get_root_basket_owed` | ported · — | `Balance` | M4 | CLI v0 `wallet balance`, `wallet inspect` (`cli/helpers.py:280`) |
| 69 | `root_basket_owed_breakdown` | coldkey_ss58: string | `sub.staking.root_basket_owed_breakdown(...)` → `Staking::root_basket_owed_breakdown` | `root.py:193` | R `BetaBasketRuntimeApi.get_root_basket_positions` | ported · — | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 70 | `root_basket_portfolio` | coldkey_ss58: string | `sub.staking.root_basket_portfolio(...)` → `Staking::root_basket_portfolio` | `root.py:273` | — (none) | ported · `BasketPosition` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 71 | `root_basket_total_nav` | — | `sub.staking.root_basket_total_nav(...)` → `Staking::root_basket_total_nav` | `root.py:428` | R `BetaBasketRuntimeApi.get_root_basket_total_nav` | ported · — | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 72 | `root_baskets` | — | `sub.staking.root_baskets(...)` → `Staking::root_baskets` | `root.py:352` | S `SubtensorModule.BasketRate`; R `BetaBasketRuntimeApi.get_all_validator_baskets` | ported · `BasketSummary` + `BetaPricing` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 73 | `root_claim_threshold` | — | `sub.staking.root_claim_threshold(...)` → `Staking::root_claim_threshold` | `root.py:442` | — (none) | ported · — | `Balance` | M5 | verify read of `set_root_claim_threshold` (M5; `PY/intents/root.py:110`) and the generated `tx` help points to it (`PY/cli/tx.py:249-250`); §11.4 scenario 18 asserts through it |
| 74 | `stake` | coldkey_ss58: string, hotkey_ss58: string, netuid: integer | `sub.staking.stake(...)` → `Staking::stake`; `Staking::get` (`:124`) | `staking.py:75` | R `StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid` | ported · `StakeInfo` | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 75 | `stake_availability` | coldkey_ss58: string, netuid: integer | `sub.staking.stake_availability(...)` → `Staking::stake_availability` | `staking.py:194` | R `StakeInfoRuntimeApi.get_stake_availability_for_coldkeys` | ported · `StakeAvailability` | `dict` | M6 | default: §14 M6 "all 102 reads" |
| 76 | `stake_availability_for_coldkey` | coldkey_ss58: string, netuids: array | `sub.staking.stake_availability_for_coldkey(...)` → `Staking::stake_availability_for_coldkey` | `staking.py:227` | R `StakeInfoRuntimeApi.get_stake_availability_for_coldkeys` | ported · `StakeAvailability` | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 77 | `stake_for_coldkey` | coldkey_ss58: string | `sub.staking.stake_for_coldkey(...)` → `Staking::stake_for_coldkey`; `Staking::positions` (`:136`) | `staking.py:90` | R `StakeInfoRuntimeApi.get_stake_info_for_coldkey` | ported · `StakeInfo` | `list[StakePosition]` | M6 | default: §14 M6 "all 102 reads" |
| 78 | `stake_for_coldkeys` | coldkey_ss58s: array | `sub.staking.stake_for_coldkeys(...)` → `Staking::stake_for_coldkeys` | `staking.py:107` | R `StakeInfoRuntimeApi.get_stake_info_for_coldkeys` | ported · `StakeInfo` | `dict[str, list[StakePosition]]` | M6 | default: §14 M6 "all 102 reads" |
| 79 | `stake_value_for_coldkey` | coldkey_ss58: string | `sub.staking.stake_value_for_coldkey(...)` → `Staking::stake_value_for_coldkey` | `staking.py:139` | R `StakeInfoRuntimeApi.get_stake_info_for_coldkey` | ported · `StakeInfo` | `StakeValuation` | M4 | CLI v0 `wallet balance`, `wallet inspect` (`cli/helpers.py:476, 784`) |
| 80 | `stake_value_for_coldkeys` | coldkey_ss58s: array | `sub.staking.stake_value_for_coldkeys(...)` → `Staking::stake_value_for_coldkeys` | `staking.py:162` | R `StakeInfoRuntimeApi.get_stake_info_for_coldkeys` | ported · `StakeInfo` | `dict[str, StakeValuation]` | M4 | CLI v0 `wallet balance --all` (`cli/helpers.py:413`) |
| 81 | `staking_hotkeys` | coldkey_ss58: string | `sub.staking.staking_hotkeys(...)` → `Staking::staking_hotkeys` | `staking.py:277` | S `SubtensorModule.StakingHotkeys` | ported · — | `list[str]` | M6 | default: §14 M6 "all 102 reads" |
| 82 | `total_alpha_staked` | netuid: integer | `sub.staking.total_alpha_staked(...)` → `Staking::total_alpha_staked` | `staking.py:257` | S `SubtensorModule.TotalAlphaStaked` | ported · — | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 83 | `validator_basket` | hotkey_ss58: string | `sub.staking.validator_basket(...)` → `Staking::validator_basket` | `root.py:295` | R `BetaBasketRuntimeApi.get_validator_basket` | ported · — | `list[dict]` | M6 | default: §14 M6 "all 102 reads" |
| 84 | `validator_basket_nav` | hotkey_ss58: string | `sub.staking.validator_basket_nav(...)` → `Staking::validator_basket_nav` | `root.py:383` | R `BetaBasketRuntimeApi.get_validator_basket_nav` | ported · — | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 85 | `validator_basket_summary` | hotkey_ss58: string | `sub.staking.validator_basket_summary(...)` → `Staking::validator_basket_summary` | `root.py:319` | S `SubtensorModule.BasketRate`; R `BetaBasketRuntimeApi.get_validator_basket_summary` | ported · `BasketSummary` + `BetaPricing` | `dict` | M4 | CLI v0 `wallet inspect` basket rows (`cli/helpers.py:751`) |

### 5.13 Subnets: `Subnets` (`client.subnets()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 86 | `burn` | netuid: integer | `sub.subnets.burn(...)` → `Subnets::burn`; same name, curated (`:153`) | `subnets.py:79` | S `SubtensorModule.Burn` | ported · — | `Balance` | M4 | `burned_register` burn/lock preview from `Burn` (§9.6) [I] |
| 87 | `commit_reveal_enabled` | netuid: integer | `sub.subnets.commit_reveal_enabled(...)` → `Subnets::commit_reveal_enabled`; same name, curated (`:157`) | `subnets.py:120` | S `SubtensorModule.CommitRevealWeightsEnabled` | ported · — | `bool` | M2 | CR path selection (§9.3 steps 2, 6) |
| 88 | `mechanism_count` | netuid: integer | `sub.subnets.mechanism_count(...)` → `Subnets::mechanism_count` | `subnets.py:260` | S `SubtensorModule.MechanismCountCurrent` | ported · — | `int` | M2 | per-mechanism loop `mech < MechanismCountCurrent` (§9.3) |
| 89 | `mechanism_emission_split` | netuid: integer | `sub.subnets.mechanism_emission_split(...)` → `Subnets::mechanism_emission_split` | `subnets.py:272` | S `SubtensorModule.MechanismEmissionSplit` | ported · — | `list[int]` | M4 | CLI v0 `hparams mechanisms emissions` (`cli/commands/sudo.py:527`) |
| 90 | `metagraph` | netuid: integer | (empty: the curated `Subnets.metagraph` shadows the read) → raw read via `dispatch(ReadName::Metagraph, …)` and `rtao_reads::reads::subnets::metagraph`; the curated `client.subnets().metagraph(..)` is `rtao_metagraph::SubnetsMetagraphExt` (in `rtao::prelude`) and returns `Metagraph` (`namespaces.py:161-173`, §4.8, §8.2); no inherent `Subnets::metagraph` is generated | `subnets.py:176` | R `SubnetInfoRuntimeApi.get_metagraph` | ported · `MetagraphWire` (+ `MetagraphView`, §8.2) | `dict` | M1 | §14 M1 exit criterion `client.subnets().metagraph(NetUid(1))`; §8.2 |
| 91 | `subnet` | netuid: integer | `sub.subnets.subnet(...)` → `Subnets::subnet`; `Subnets::info` (`:189`) | `subnets.py:30` | S `SubtensorModule.Tempo`; S `SubtensorModule.Burn`; S `SubtensorModule.SubnetworkN` | ported · — | `SubnetInfo` | M6 | default: §14 M6 "all 102 reads" |
| 92 | `subnet_hyperparameters` | netuid: integer | `sub.subnets.subnet_hyperparameters(...)` → `Subnets::subnet_hyperparameters` | `subnets.py:156` | R `SubnetInfoRuntimeApi.get_subnet_hyperparams_v3` | ported · `HyperparamEntry`/`HyperparamValue` | `Optional[dict]` | M4 | `HyperparamPlanner` current value (§9.7); CLI v0 `hparams get` (`cli/hyperparams_view.py:32`) |
| 93 | `subnet_identity` | netuid: integer | `sub.subnets.subnet_identity(...)` → `Subnets::subnet_identity` | `subnets.py:199` | S `SubtensorModule.SubnetIdentitiesV3` | ported · `SubnetIdentityV3` | `Optional[dict]` | M4 | CLI v0 `hparams get-identity` (`cli/commands/sudo.py:307`) |
| 94 | `subnet_names` | — | `sub.subnets.subnet_names(...)` → `Subnets::subnet_names` | `subnets.py:206` | S `SubtensorModule.SubnetIdentitiesV3` | ported · `SubnetIdentityV3` | `dict[int, str]` | M4 | CLI v0 subnet-name cache, every networked command (§10.2; `cli/context.py:2003`) |
| 95 | `subnet_registration_cost` | — | `sub.subnets.subnet_registration_cost(...)` → `Subnets::subnet_registration_cost` | `subnets.py:283` | R `SubnetRegistrationRuntimeApi.get_network_registration_cost` | ported · — | `Balance` | M6 | default: §14 M6 "all 102 reads" |
| 96 | `subnet_start_schedule` | netuid: integer | `sub.subnets.subnet_start_schedule(...)` → `Subnets::subnet_start_schedule` | `subnets.py:235` | S `SubtensorModule.NetworkRegisteredAt`; C `SubtensorModule.InitialStartCallDelay` | ported · — | `dict` | M4 | `OwnerOps::start` (§9.7); CLI v0 `hparams check-start` (`cli/commands/sudo.py:263`) |
| 97 | `subnet_tao_flows` | — | `sub.subnets.subnet_tao_flows(...)` → `Subnets::subnet_tao_flows` | `subnets.py:93` | S `SubtensorModule.SubnetEmaTaoFlow` | ported · — | `dict[int, float]` | M6 | default: §14 M6 "all 102 reads" |
| 98 | `subnets` | — | `sub.subnets.subnets(...)` → `Subnets::subnets`; `Subnets::all` (`:193`) | `subnets.py:47` | S `SubtensorModule.NetworksAdded`; S `SubtensorModule.Tempo`; S `SubtensorModule.Burn`; S `SubtensorModule.SubnetworkN` | ported · — | `list[SubnetInfo]` | M6 | default: §14 M6 "all 102 reads" |
| 99 | `token_symbols` | — | `sub.subnets.token_symbols(...)` → `Subnets::token_symbols` | `subnets.py:218` | S `SubtensorModule.TokenSymbol` | ported · — | `dict[int, str]` | M1 | `Client::connect` step 6 (§3.3); Python runs this read at connect (`client.py:613-633`) |

### 5.14 Weights & bonds: `Weights` (`client.weights()`)

| # | read | params | Python → rtao | py | hits | body · typed model | out | M | why this milestone |
|---:|---|---|---|---|---|---|---|---|---|
| 100 | `bonds` | netuid: integer, mechid: integer | `sub.weights.bonds(...)` → `Weights::bonds` | `weights.py:59` | S `SubtensorModule.Bonds` | ported · `SparseRows` (§8.2) | `dict[int, dict[int, float]]` | M6 | default: §14 M6 "all 102 reads" |
| 101 | `timelocked_weight_commits` | netuid: integer, mechid: integer | `sub.weights.timelocked_weight_commits(...)` → `Weights::timelocked_weight_commits` | `weights.py:89` | S `SubtensorModule.TimelockedWeightCommits` | ported · — | `dict[int, list[dict]]` | M6 | default: §14 M6 "all 102 reads" |
| 102 | `weights` | netuid: integer, mechid: integer | `sub.weights.weights(...)` → `Weights::weights` | `weights.py:32` | S `SubtensorModule.Weights` | ported · `SparseRows` (§8.2) | `dict[int, dict[int, float]]` | M2 | M2 exit scenario reads weights back (§11.4 test 1) [I] |

### 5.15 Hits the catalog omits

The catalog's `hits` come from scanning each read function for generated `st.`/`api.` references
(`read_targets`, `UP/website/apps/bittensor-website/scripts/generate.py:405-480`; [R] I §5.4). Reads that reach
chain items through module helpers or through string-built `Item(...)`/`Method(...)` descriptors are under-reported.
Scanning each read plus the module-local helpers it calls gives:

| read | catalog hits | items also touched (source) |
|---|---:|---|
| `basket_position` | 0 | R `BetaBasketRuntimeApi.get_basket_position` |
| `basket_trading_status` | 0 | R `BetaBasketRuntimeApi.get_basket_trading_status` |
| `coldkey_lock` | 1 | R `StakeInfoRuntimeApi.get_coldkey_lock`; S `SubtensorModule.DecayingLock` |
| `collateral_policy` | 0 | S `SubtensorModule.CollateralDrainRatio`; S `SubtensorModule.CollateralLockShare` |
| `locks_for_coldkey` | 1 | R `StakeInfoRuntimeApi.get_coldkey_lock`; S `SubtensorModule.DecayingLock` |
| `locks_for_hotkey` | 1 | R `StakeInfoRuntimeApi.get_coldkey_lock`; S `SubtensorModule.DecayingLock` |
| `miner_collateral` | 1 | S `SubtensorModule.MinerCollateral` |
| `root_basket_portfolio` | 0 | R `BetaBasketRuntimeApi.get_root_basket_portfolio` |
| `root_baskets` | 2 | R `BetaBasketRuntimeApi.get_all_beta_pricing` |
| `root_claim_threshold` | 0 | S `SubtensorModule.RootClaimableThreshold` |
| `subnet_collateral` | 1 | S `SubtensorModule.MinerCollateral` |
| `validator_basket_summary` | 2 | R `BetaBasketRuntimeApi.get_beta_pricing` |

Seven of these have non-empty catalog hits. [§4.8](architecture.md#48-reads) and
[§11.3](architecture.md#113-fakes-at-the-right-layers-j-pd-concern-5) assert that a `RecordingView` record equals the
catalog `hits`, allowing a superset only for the 10 zero-hit reads; as first written, that test failed for
`coldkey_lock`, `locks_for_coldkey`, `locks_for_hotkey`, `miner_collateral`, `subnet_collateral`, `root_baskets` and
`validator_basket_summary`. Architecture §4.8 now checks against H = helper-aware extracted hits ∪ reviewed extras
(`parity/read_hits_extra.toml`), with the catalog hits a subset of H (section 8 #7). The other five are zero-hit reads
whose real items are listed for completeness; the remaining zero-hit reads (`block_info`, `block_time`, `is_fast_blocks`, `timestamp`,
`commitments`) go through view methods (`Timestamp.Now`, `Aura.SlotDuration`, block RPCs) or through
`PY/metagraph.py` `fetch_commitments` (`:258`).

## 6. Error codes (18) and the chain-error tables

`ErrorCode` is generated from `CAT/errors.json` `codes` ([§4.7](architecture.md#47-results-and-the-error-model)); the
variant names below are §4.7's, in the same order as Python's `ErrorCode` enum (`PY/error_map.py:23-41`). All land
in M2 (§14 M2: "generated `ErrorCode`/`CHAIN_ERRORS`"). The remediation text is the catalog's and equals
`PY/result.py` `REMEDIATION` (`:78-111`).

| code | `ErrorCode::` | docs | remediation | M |
|---|---|---|---|---|
| `already_exists` | `AlreadyExists` | `/docs/errors/already-exists` | the object or state already exists; treat the goal as met or pick a different target | M2 |
| `disabled` | `Disabled` | `/docs/errors/disabled` | this call or feature is switched off on this network | M2 |
| `expired` | `Expired` | `/docs/errors/expired` | the window has closed; restart the flow with fresh state | M2 |
| `insufficient_balance` | `InsufficientBalance` | `/docs/errors/insufficient-balance` | fund the signing account or reduce the amount; check with `btcli wallet balance` | M2 |
| `insufficient_liquidity` | `InsufficientLiquidity` | `/docs/errors/insufficient-liquidity` | the pool cannot absorb this trade; reduce the amount or split it into smaller steps | M2 |
| `internal` | `Internal` | `/docs/errors/internal` | a chain-side invariant failed; nothing to fix client-side — report it if it persists | M2 |
| `invalid_argument` | `InvalidArgument` | `/docs/errors/invalid-argument` | check the argument values against the operation schema | M2 |
| `limit_exceeded` | `LimitExceeded` | `/docs/errors/limit-exceeded` | a chain-side capacity limit was hit; reduce the size or count of the request | M2 |
| `not_authorized` | `NotAuthorized` | `/docs/errors/not-authorized` | sign with the key or origin that owns the target object, then retry | M2 |
| `not_found` | `NotFound` | `/docs/errors/not-found` | the referenced object is not on-chain; check the identifier | M2 |
| `not_registered` | `NotRegistered` | `/docs/errors/not-registered` | register the hotkey on this subnet first with `btcli subnets register` | M2 |
| `policy_violation` | `PolicyViolation` | `/docs/errors/policy-violation` | the action exceeds a configured safety policy | M2 |
| `rate_limited` | `RateLimited` | `/docs/errors/rate-limited` | wait for the rate-limit window to pass, then retry | M2 |
| `subnet_not_exists` | `SubnetNotExists` | `/docs/errors/subnet-not-exists` | use an existing netuid; `btcli subnets list` shows valid ones | M2 |
| `subtoken_disabled` | `SubtokenDisabled` | `/docs/errors/subtoken-disabled` | the subnet is not active yet; wait for start_call | M2 |
| `too_early` | `TooEarly` | `/docs/errors/too-early` | the required window has not opened yet; wait some blocks and retry | M2 |
| `unit_mismatch` | `UnitMismatch` | `/docs/errors/unit-mismatch` | match the Balance netuid to the operation's currency | M2 |
| `unknown` | `Unknown` | `/docs/errors/unknown` | inspect the message for details | M2 |

Tables that travel with the codes (all M2 unless noted):

| Table | Python source | rtao home | How |
|---|---|---|---|
| 390 chain error names → code, description, pallets | `PY/error_map.py` `NAME_TO_CODE` (`:46`), `PY/error_descriptions/*` | `rtao_catalog::CHAIN_ERRORS` | generated from `CAT/errors.json` `chain_errors` |
| per-code explanation (18) | `PY/result.py` `EXPLANATIONS` (`:116-287`) | `rtao_catalog::py` | extracted (not in the catalog; PR-U3 would export it) |
| custom transaction codes 0-27 and 255 (29) | `PY/error_map.py` `CUSTOM_TRANSACTION_ERRORS` (`:488`) | `rtao_catalog::py` | extracted; CI checks the key set against `UP/common/src/transaction_error.rs:38` with `syn` |
| invalid-transaction needles | `PY/error_map.py` `INVALID_TRANSACTION_ERRORS` (`:524`) | `rtao_catalog::py` | extracted |
| dispatch errors (52) | `PY/error_map.py` `DISPATCH_ERRORS` (`:555`) | `rtao_catalog::py` | extracted |
| `(pallet_index, error_index)` → name | `PY/_generated/errors.py` | `rtao_runtime::generated::errors::MODULE_ERRORS` | generated from metadata (M1 `xtask regen` v1); docs, gates and message text only; run-time resolution uses `Runtime::module_error`, nested errors included (D-028) |
| per-name remediation overrides (42) | `PY/result.py` `_NAME_HELP_OVERRIDES` (`:308`) | `rtao_catalog::py` | extracted |
| substring remediation override (1) | `PY/result.py` `_HELP_OVERRIDES` (`:475`) | `rtao_catalog::py` | extracted |
| no-name classification needles (10) | `PY/result.py` `_SUBSTRING_FALLBACK` (`:291`) | `rtao_catalog::py` | extracted |
| watch-status fatals (5) | `PY/result.py` `_WATCH_STATUS_RE` (`:31`), `_WATCH_STATUS_NAMES` (`:36`) | `rtao_catalog::py` (regex source string), used by `rtao_tx::classify` | extracted (rtao resolves `retracted` and `finalityTimeout` instead of failing, D-024) |
| custom-error regex | `PY/result.py` `_CUSTOM_ERROR_RE` (`:27`) | `rtao_catalog::py` (regex source string), used by `rtao_tx::classify` | extracted |
| named-error message text | `PY/result.py` `_CATALOG_DOCS_BY_NAME` (`:35`, a comprehension over `_generated/errors.py`) | derived in `rtao_runtime::generated::errors` from `MODULE_ERRORS`, pallet-index order, last name wins (architecture §4.7) | generated |

## 7. CLI command tree

Source: `PY/cli/main.py` (groups mounted at `:99-144`, hidden aliases `:116-135`, snake_case aliases `:449-477`).
rtao home: crate `rtao-cli`, binary `rtao` ([§10](architecture.md#10-cli-rtao-cli-binary-rtao)). The architecture
fixes the crate, not its module layout; this inventory assumes one module per Python command file
(`crates/rtao-cli/src/commands/<file>.rs`) by analogy with [§4.4](architecture.md#44-hand-ported-bodies) [I].

### 7.1 Generated at run time

| Command | Python source | rtao | Milestone |
|---|---|---|---|
| `tx <op>` (83) | `PY/cli/tx.py` (`build_tx_app` `:487`, `intent_command` `:464`) | built at start-up with clap's builder API from `rtao_catalog::INTENTS` (§10.1) | M4 for implemented ops (20), complete M5 |
| `query <read>` (102) | `PY/cli/query.py` (`build_query_app` `:258`, `_jsonable` `:104-116`) | built at start-up from `READS`; `--block` is an rtao addition (D-014) | M4 for implemented reads (38), complete M6 |
| `tools` | `PY/cli/main.py:321` | from the catalog projection, plus an rtao `rust` object (§10.1, §13; an addition, D-029) | M4 |
| `explain` | `PY/cli/main.py:405` | from `ErrorCode`/`CHAIN_ERRORS` | M4 |
| `call` | `PY/cli/call.py:152` | raw call (no policy, as btcli: `PY/cli/context.py:1990-1994`); `--signer` through `submit_call_dyn`, `--multisig` through the `Multisig` account object (§4.6) | M4 |

rtao-only commands (no Python counterpart, outside the counts): `doctor`, `localnet`, `wallet save-password-env`
([§10.4](architecture.md#104-rtao-only-commands)).

### 7.2 Hand-written groups

All 19 groups are hand-written ports of `PY/cli/commands/*.py` ([§10.5](architecture.md#105-hand-written-groups-ported-from-pyclicommandspy));
the `stake` aliases and `evm claim-deposit` reuse the generated `tx` commands. Milestones per
[§10.6](architecture.md#106-scope-per-milestone), which now names every M4 and M5 command (it used to say "proxy, multisig
basics", which this inventory read as the commands listed under M5).
Module names are relative to the `rtao-cli` crate. Line numbers are the registration lines in the listed file.

| Group | Python files (lines) | rtao module [I] | M4 | M5 | M6 | M7 | M8 | complete |
|---|---|---|---|---|---|---|---|---|
| `wallet` | `cli/commands/wallet.py` (1,784) | `commands::wallet` | create, new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub, regen-hotkeypub, show, list, balance, inspect | — | — | keychain save, keychain show, keychain delete | sign, verify, encrypt, decrypt, unlock, overview, registrations, history, transfer, set-identity, get-identity, associate-hotkey, swap-hotkey, swap-coldkey, swap-check, announce-coldkey-swap | M8 |
| `stake` | `cli/commands/stake.py` (446), `cli/stake_exit.py` (222), `cli/stake_picker.py` (676) | `commands::stake`, `stake_exit`, `stake_picker` | — | add, remove, move, move-swap, transfer, swap, burn | — | — | unstake-all, unstake-all-alpha, show, list, auto, set-auto, child get, child set, child revoke, child take, wizard | M8 |
| `root` | `cli/commands/root.py` (708), `cli/commands/root_move.py` (213), `cli/commands/root_swap.py` (219), `cli/root_helpers.py` (526) | `commands::root`, `commands::root_move`, `commands::root_swap`, `root_helpers` | — | — | list, claim, allocate, move, swap, register | — | — | M6 |
| `subnets` | `cli/commands/subnets.py` (634), `cli/metagraph_view.py` (211), `cli/hyperparams_view.py` (151) | `commands::subnets`, `metagraph_view`, `hyperparams_view` | register, create, metagraph | — | list, show, hyperparameters, burn-cost, create-cost, price, conviction | — | — | M6 |
| `conviction` (alias `lock`) | `cli/commands/lock.py` (330) | `commands::lock` | — | — | list, targeting, show, add, mode, accept, move | — | — | M6 |
| `hparams` (aliases `sudo`, `su`) | `cli/commands/sudo.py` (528), `cli/null_pruning.py` (392) | `commands::sudo`, `null_pruning` | set, get, get-take, check-start, start, set-symbol, get-identity, set-identity, trim, trim-null-batch, trim-null, mechanisms count, mechanisms emissions | set-take, stake-burn | — | — | — | M5 |
| `addr` | `cli/commands/addresses.py` (271) | `commands::addresses` | — | — | — | — | add, save, list, show, remove | M8 |
| `proxy` | `cli/commands/proxy.py` (498) | `commands::proxy` | — | create, add, remove, kill, list, execute | — | — | book add, book list, book remove, book update, book clear | M8 |
| `multisig` | `cli/commands/multisig.py` (344), `cli/multisig_helpers.py` (1,455) | `commands::multisig`, `multisig_helpers` | — | add, list, show, remove, pending | — | — | — | M5 |
| `config` | `cli/commands/config.py` (94) | `commands::config` | set, get, unset, clear, path | — | — | — | — | M4 |
| `misc` | `cli/commands/misc.py` (204) | `commands::misc` | — | — | — | — | convert, latency | M8 |
| `weights` (under `misc`; hidden top-level aliases) | `cli/commands/weights.py` (195) | `commands::weights` | set, commit, reveal | — | — | — | — | M4 |
| `axon` (under `misc`) | `cli/commands/axon.py` (68) | `commands::axon` | set, reset | — | — | — | — | M4 |
| `collateral` (under `misc`) | `cli/commands/collateral.py` (171) | `commands::collateral` | — | — | show, list, add, set-min | — | — | M6 |
| `timelock` (under `misc`) | `cli/commands/timelock.py` (336) | `commands::timelock` | — | — | — | encrypt, decrypt, show | — | M7 |
| `crowd` (under `misc`) | `cli/commands/crowd.py` (58) | `commands::crowd` | — | — | — | — | list, info, contributors | M8 |
| `extension` (under `misc`) | `cli/commands/extension.py` (120) | `commands::extension` | — | — | — | bridge, stop, accounts | — | M7 |
| `upgrade` (under `misc`) | `cli/commands/upgrade.py` (651), `cli/upgrade_helpers.py` (721) | `commands::upgrade`, `upgrade_helpers` | — | — | — | — | pending, check, sign | M8 |
| `evm` | `cli/commands/evm/*.py` (1,602) | `commands::evm::*` | — | — | — | key new, key import, key export, key list, key show, stake add, stake remove, stake show, mirror, pubkey, deposit-address, balance, fund, send, send-to-ss58, associate, precompiles, abi, call, deploy, networks, config, doctor, setup-localnet, claim-deposit | — | M7 |

Per-command registration lines and notes:

- **(root commands)** — `tools` main.py:321 (M4); `explain` main.py:405 (M4); `call` call.py:152 (mounted main.py:140) (M4).
- **wallet** — `create` :264 (M4; it only combines `new-coldkey` and `new-hotkey`, `wallets.create` `:214-246`); `new-coldkey` :330 (M4); `new-hotkey` :376 (M4); `regen-coldkey` :424 (M4); `regen-hotkey` :507 (M4); `regen-coldkeypub` :567 (M4); `regen-hotkeypub` :601 (M4); `sign` :641 (M8); `verify` :685 (M8); `encrypt` :707 (M8); `decrypt` :735 (M8); `unlock` :786 (M8); `keychain save` :847 (M7); `keychain show` :888 (M7); `keychain delete` :913 (M7); `show` :934 (M4); `list` :980 (M4); `balance` :1017 (M4); `overview` :1146 (M8); `registrations` :1261 (M8); `history` :1379 (M8); `transfer` :1429 (M8); `inspect` :1467 (M4; listed in both §10.6 M4 and M8; earliest kept); `set-identity` :1566 (M8); `get-identity` :1642 (M8); `associate-hotkey` :1657 (M8); `swap-hotkey` :1671 (M8); `swap-coldkey` :1696 (M8); `swap-check` :1716 (M8); `announce-coldkey-swap` :1765 (M8).
- **stake** — `add` :53-62 (M5; alias of `tx add-stake`); `remove` :53-62 (M5; alias of `tx remove-stake`); `move` :53-62 (M5; alias of `tx move-stake`); `move-swap` :53-62 (M5; alias of `tx move-swap-stake`); `transfer` :53-62 (M5; alias of `tx transfer-stake`); `swap` :53-62 (M5; alias of `tx swap-stake`); `burn` :53-62 (M5; alias of `tx stake-burn`); `unstake-all` :64 → stake_exit.py:212-222 (M8; hand-written exit command, not an alias); `unstake-all-alpha` :64 → stake_exit.py:212-222 (M8; hand-written exit command); `show` :67 (M8); `list` :124 (M8); `auto` :278 (M8); `set-auto` :305 (M8); `child get` :333 (M8); `child set` :349 (M8); `child revoke` :386 (M8); `child take` :405 (M8); `wizard` :431 (M8; §10.6 M8 names it).
- **root** — `list` :346 (M6); `claim` :494 (M6); `allocate` :598 (M6); `move` :682 (M6); `swap` :684 (M6); `register` :687 (M6).
- **subnets** — `list` :46 (M6); `show` :139 (M6); `hyperparameters` :246 (M6; hidden alias `hparams` (:264)); `burn-cost` :267 (M6); `create-cost` :291 (M6); `price` :300 (M6); `conviction` :343 (M6); `register` :537 (M4); `create` :611 (M4); `metagraph` :625 (M4).
- **conviction (alias `lock`)** — `list` :26 (M6); `targeting` :120 (M6); `show` :207 (M6); `add` :237 (M6); `mode` :268 (M6); `accept` :289 (M6); `move` :314 (M6).
- **hparams (aliases `sudo`, `su`)** — `set` :104 (M4); `get` :140 (M4); `get-take` :157 (M4); `set-take` :225 (M5; needs `set_take` (M5); §10.6 conflict, see §8); `check-start` :255 (M4); `start` :267 (M4); `set-symbol` :283 (M4); `get-identity` :299 (M4); `set-identity` :311 (M4); `trim` :339 (M4); `trim-null-batch` :356 (M4); `trim-null` :373 (M4); `stake-burn` :450 (M5; needs `stake_burn` (M5); hidden alias `buyback` (:486)); `mechanisms count` :494 (M4); `mechanisms emissions` :519 (M4).
- **addr** — `add` :144 (M8); `save` :227 (M8); `list` :239 (M8); `show` :247 (M8); `remove` :262 (M8).
- **proxy** — `create` :60 (M5); `add` :120 (M5); `remove` :136 (M5); `kill` :172 (M5); `list` :284 (M5); `execute` :319 (M5); `book add` :353 (M8); `book list` :416 (M8); `book remove` :459 (M8); `book update` :471 (M8); `book clear` :489 (M8).
- **multisig** — `add` :67 (M5); `list` :159 (M5); `show` :170 (M5); `remove` :202 (M5); `pending` :214 (M5).
- **config** — `set` :20 (M4); `get` :37 (M4); `unset` :58 (M4); `clear` :74 (M4); `path` :89 (M4).
- **misc** — `convert` :54 (M8); `latency` :92 (M8).
- **weights (under `misc`; hidden top-level aliases)** — `set` :50 (M4); `commit` :99 (M4); `reveal` :155 (M4).
- **axon (under `misc`)** — `set` :18 (M4); `reset` :56 (M4).
- **collateral (under `misc`)** — `show` :22 (M6); `list` :70 (M6); `add` :90 (M6); `set-min` :138 (M6).
- **timelock (under `misc`)** — `encrypt` :127 (M7); `decrypt` :231 (M7); `show` :302 (M7).
- **crowd (under `misc`)** — `list` :13 (M8); `info` :30 (M8); `contributors` :45 (M8).
- **extension (under `misc`)** — `bridge` :34 (M7); `stop` :57 (M7); `accounts` :68 (M7).
- **upgrade (under `misc`)** — `pending` :151 (M8); `check` :226 (M8); `sign` :314 (M8).
- **evm** — `key new` keys.py:32 (M7); `key import` keys.py:66 (M7); `key export` keys.py:150 (M7); `key list` keys.py:214 (M7); `key show` keys.py:230 (M7); `stake add` stake.py:30 (M7); `stake remove` stake.py:63 (M7); `stake show` stake.py:96 (M7); `mirror` address.py:16 (M7); `pubkey` address.py:17 (M7); `deposit-address` address.py:18 (M7); `balance` money.py:31 (M7); `fund` money.py:32 (M7); `send` money.py:33 (M7); `send-to-ss58` money.py:34 (M7); `associate` association.py:17 (M7); `precompiles` precompiles_cmd.py:27 (M7); `abi` precompiles_cmd.py:28 (M7); `call` precompiles_cmd.py:29 (M7); `deploy` contracts_cmd.py:18 (M7); `networks` setup.py:27 (M7); `config` setup.py:28 (M7); `doctor` setup.py:29 (M7); `setup-localnet` setup.py:30 (M7); `claim-deposit` __init__.py:51 (M7; alias of `tx evm-withdraw`).

### 7.3 Aliases

- v9 shorthands and renamed groups are hidden aliases (`PY/cli/main.py:116-135`): `w`, `wallets`, `st`, `rt`, `s`,
  `subnet`, `su`, `sudo`, `lock`, `c`, `conf`, `wt`, `weight`, `weights`, `cr`, `crowdloan`, `crowd`, `addresses`,
  `extension`, `timelock`, `axon`, `collateral`, `upgrade`, `utils`. Each lands with its target group.
- Every hyphenated command also answers to its snake_case spelling (`_register_snake_case_aliases`, `:449-477`),
  including the generated `tx`/`query` commands.
- Group-level hidden aliases: `stake children` (= `stake child`, `stake.py:330`), `hparams mech` (= `mechanisms`,
  `sudo.py:491`), `subnets hparams` (`subnets.py:264`), `hparams buyback` (`sudo.py:486`).

## 8. Inconsistencies found and how this inventory resolves them

Each item cites the evidence. All twelve are now resolved, and architecture.md follows the resolutions (§4.3, §4.8,
§4.9, §10.6, §11.2, §11.4, §14).

| # | Topic | Evidence | Resolution |
|---:|---|---|---|
| 1 | Pass-through membership | §4.3 and decision 15 rest on a heuristic that counts `multisig_threshold_1` as pass-through and misses `transfer_all` (section 4.2). | Followed the source: same count, swapped members. |
| 2 | Generator milestone | §14 places "46 pass-through bodies generated" in M5, but 9 of the 46 are M4 deliverables and `transfer_all` is an M2 deliverable. | Resolved: the generator (`generated/passthrough.rs`) lands in M2 with the rest of `xtask regen`'s intent output; `transfer_all` (M2) and the 9 M4 ops use the generated bodies as they land, M5 the other 36 (architecture §4.3, §14). |
| 3 | CLI `hparams` in M4 | §10.6 puts the owner `hparams` group in M4; §10.5 lists `set-take` and `stake-burn` in it, whose intents (`set_take`, `stake_burn`) are not M4 intents in §14. | Those two commands moved to M5 with their intents; the other 13 stay M4. |
| 4 | `wallet inspect` twice | §10.6 lists `wallet inspect` in M4 and in M8. | Kept at M4 (earliest). This alone pulls 4 reads into M4: `delegated`, `identity`, `hotkey_identities`, `validator_basket_summary`. |
| 5 | `wallet create` | §10.6 M4 lists `new-coldkey`/`new-hotkey`/`regen-*` but not `create`, which only combines them. | Resolved: M4. `wallet create` only combines `new-coldkey` and `new-hotkey` (`PY/cli/commands/wallet.py:264-327` → `wallets.create`, `:214-246`); CLI v0 is 40 commands, M8 keeps 45 (architecture §10.6). |
| 6 | Shielded scenario in M4 | §14 M4 exit requires "shielded scenarios green"; §11.4 scenario 11 shields `burned_register` and `add_collateral`, but `add_collateral` is M5 ("collateral"). | `add_collateral` stays M5; the M4 run of scenario 11 covers `burned_register` only. |
| 7 | Exact-hits test | §4.8/§11.3 allow a hit superset only for the 10 zero-hit reads; 7 reads with non-empty catalog hits touch more items (section 5.15). | Resolved (architecture §4.8): a helper-aware extractor computes each read's static hit set; out-of-module items are reviewed extras in `parity/read_hits_extra.toml`; H = extracted ∪ extras ⊇ catalog hits; every recorded run touches a subset of H and the union over per-branch samples equals H; the oracle's `touched` record is a cross-check; PR-U16 proposes the scan upstream. |
| 8 | Retired e2e name | §14 M5 exit asks for "all 73 `intent_*` e2e names mapped and green"; `intent_set_root_claim_type` (`e2e.rs:158-169`) asserts a retired core constructor and has no catalog op, and §11.2's "73 intent-plan tests collapse into one table-driven test over `AnyIntent`" cannot include it. | Map it in `parity/e2e_map.toml` as n/a (retired upstream call). |
| 9 | Ops without any e2e | 8 ops have neither an upstream e2e test nor a §11.4 scenario: `move_swap_stake`, `set_min_collateral`, `set_reject_locked_alpha`, `set_root_claim_threshold`, `set_subnet_emission_enabled`, `stake_into_basket`, `swap_basket`, `swap_basket_many`. | Resolved: architecture §11.4 scenario 18 (M5) submits all eight on localnet (the two root ops signed by Alice as sudo, exercising the `Sudid` receipt path) and asserts their effect; `root_claim_threshold` moves to M5 as `set_root_claim_threshold`'s verify read. |
| 10 | `e2e_map.toml` scope | §4.9 defines `e2e_map.toml` over the 112 manifest names; the three `PYT/e2e/*.py` tests (null consensus, PoW registration, timelocked commitment) are not in the manifest. | Resolved: `e2e_map.toml` keys are the 112 manifest names plus one `pyt:e2e/<file>::<function>` key per Python e2e test (115 at v477), mapped to scenarios 4, 14 and the new scenario 19 (architecture §4.9, §11.4). |
| 11 | Reads needed before M6 | §14 delivers "all 102 reads" in M6 and names only the metagraph earlier, yet §9 components and the §10.6 CLI v0 consume 39 reads in M1-M5 (section 1). | Assigned by the rules of section 2.4 (40 reads in M1-M5 after #9); M6 keeps the other 62. |
| 12 | Namespace source for `metagraph` | §4.8 names namespace accessors "from the catalog's `python` field"; the `metagraph` read has an empty `python` because the curated method shadows it (`PY/namespaces.py:161-173`). | Resolved (architecture §4.8): the namespace comes from `category` (14 categories, one namespace each); `python` only names the method, and `xtask` asserts its `sub.<ns>.` prefix where present. |

Module-level gaps (homes missing or inconsistent with [§2.2](architecture.md#22-crates)) are in
[python-module-map.md, Gaps found](python-module-map.md#gaps-found).
