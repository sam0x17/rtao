# C1 — Subtensor transaction surface for subnet owners, validators and miners (spec 477 live / 478 main)

Date: 2026-10-13. Audience: rtao architects (Rust/Substrate experts). Everything below was read from disk.

Path conventions used in citations:

- `S/…` = `SP/upstream/subtensor/…` (RaoFoundation/subtensor monorepo, commit 850f275, `spec_version: 478` at `S/runtime/src/lib.rs:241`; the clone is shallow — 1 commit — so no git history is available).
- `M/…` = `…/scratchpad/meta/…` (finney metadata V16, **spec 477**, fetched today; `M/explore/*` are subxt-cli dumps; `M/finney.json` is the JSON metadata).
- `D/…` = `…/scratchpad/docs/…` (bittensor.com corpus, `intents.json`, `errors.json`).
- `S/docs/…` = the bittensor.com docs source in the monorepo.
- "dispatches.rs" = `S/pallets/subtensor/src/macros/dispatches.rs`; "weights.rs" = `S/pallets/subtensor/src/subnets/weights.rs`; "admin-utils" = `S/pallets/admin-utils/src/lib.rs`; "lib.rs" = `S/pallets/subtensor/src/lib.rs`.

A complete machine-readable call-index map (pallet index, call index, field types) extracted from the 477 metadata is at `…/scratchpad/research/_call_index_map.txt`; an admin-utils origin/rate-limit digest is at `…/scratchpad/research/_adminutils_summary.txt`.

---

## 0. Executive findings (read these first)

1. **Call surface is identical between live finney (477) and main (478)** for every pallet in scope: I diffed the 477 metadata variant list (`_call_index_map.txt`) against the 478 source (`dispatches.rs`, admin-utils, proxy, utility, etc.) — same indices, same names, same field types (e.g. `Proxy.set_real_pays_fee` idx 11 already exists at 477, `_call_index_map.txt` line ~201).
2. **The one wire-visible 477→478 change is in the transaction-extension list**: at 477 the fee extension is the stock `pallet_transaction_payment::ChargeTransactionPayment` (identifier `ChargeTransactionPayment`, metadata type 409, one field `Compact<u64>` tip; `M/finney.json` path `[1].V16.extrinsic.transaction_extensions`). At 478 it is `ChargeTransactionPaymentWrapper` (`S/runtime/src/transaction_payment_wrapper.rs:241`, `IDENTIFIER = "ChargeTransactionPaymentWrapper"`, struct `{ inner: ChargeTransactionPayment<T> }` at :55 → **byte-identical encoding**, but a **different identifier and type path**). Any client that matches extensions by identifier (subxt's `DefaultExtrinsicParams` does) must handle both names. Treat this as the canonical example of the churn rtao must absorb.
3. **All five custom extensions carry no bytes**: `SubtensorTransactionExtension` (`type Implicit = ()`, unit struct, `S/pallets/subtensor/src/extensions/subtensor.rs:70-72,~318-321`), `SudoTransactionExtension` (`S/runtime/src/sudo_wrapper.rs:17-18,47-49`), `CheckShieldedTxValidity` (`S/pallets/shield/src/extension.rs:15-17,38-40`), `DrandPriority` (`S/pallets/drand/src/drand_priority.rs:19-21,47-49`). Metadata confirms: explicit types 410-413 are empty composites and implicit type 48 is `()` (`M/finney.json` types 48/410-413).
4. **Weight calls are `Pays::No`** (set/commit/reveal/batch, serve_axon/_tls/_prometheus, associate_evm_key, pow_register, Commitments.set_commitment), so a hotkey holding **zero TAO** can sign them. The custom `CheckNonce` only rejects provider-less accounts when `pays_fee == Yes` (`S/runtime/src/check_nonce.rs:140-147`) and, on prepare, pins `sufficients = 1` so the nonce survives balance changes (`:178-187`). Fee-bearing hotkey calls are redirected to the owning coldkey via the 478 wrapper allow-list (`S/runtime/src/fee_filters.rs:16-34`).
5. **Transaction priority is flat**: `priority = 1` for Normal/Mandatory, `10_000_000_000` for Operational, tip ignored (`S/runtime/src/transaction_payment_wrapper.rs:265-303`; `S/runtime/src/lib.rs:1183-1184`). `DrandPriority` adds `10_000` only to `Drand.write_pulse` (`drand_priority.rs:34-36,61-70`). Tips buy nothing; do not implement "bump the tip to get in".
6. **Validator per-tempo loop on a default subnet = one timelocked commit**: `commit_timelocked_mechanism_weights(netuid, mecid=0, ciphertext, reveal_round, commit_reveal_version=4)`; the chain auto-reveals at `on_initialize` of the first block of epoch `commit_epoch + RevealPeriodEpochs` (`S/pallets/subtensor/src/coinbase/reveal_commits.rs:37-57`, `block_step.rs:7-20`). The SDK's `set_weights` intent wraps `set_mechanism_weights` (119) or `commit_timelocked_mechanism_weights` (118), never the legacy `set_weights` (0) (`D/intents.json`, entry `set_weights.wraps`).
7. **`MaxWeightExceeded` can never fire**: `get_max_weight_limit` is `pub const fn … { u16::MAX }` (`S/pallets/subtensor/src/utils/misc.rs:556-559`), and `max_weight_limited` short-circuits on `u16::MAX` (weights.rs:1408-1423). The `MaxWeightsLimit` storage map still exists for reads. The SDK clips client-side anyway (`S/docs/hyperparameters/max-weights-limit.mdx`).
8. **Legacy `register` (idx 6, PoW) is now a burned registration**: its `_block_number/_nonce/_work/_coldkey` are ignored and it calls `do_register_with_post_info` (dispatches.rs:804-818). The only real PoW path is `pow_register` (idx 152) with a **new seal** (`S/pallets/subtensor/src/subnets/registration.rs:28-42`).
9. **`dissolve_network` (idx 61) is root-only** despite its doc comment "caller must be the owner" (`ensure_root`, dispatches.rs:1078-1086). Owners cannot dissolve.
10. **Owner-settable hyperparameters in source disagree with the docs table in two places**: `sudo_set_min_difficulty` (idx 4) and `sudo_set_difficulty` (idx 24) use `ensure_sn_owner_or_root_with_limits` (admin-utils:367-376, 979-988) and have `Hyperparameter::MinDifficulty=37`/`Difficulty=38` rate-limit keys (`S/pallets/subtensor/src/utils/rate_limiting.rs:225-227`), but `S/docs/hyperparameters/index.mdx` marks both "root only". Trust the source.
11. **The SDK's Rust epoch-schedule port differs from the pallet**: core uses `blocks_since_last_step > MAX_TEMPO` (`S/sdk/bittensor-core/src/timelock/epoch_schedule.rs:54`) while the pallet uses `BlocksSinceLastStep > tempo` (`S/pallets/subtensor/src/coinbase/run_coinbase.rs:1232`). rtao should port the pallet predicate verbatim and keep the vectors in `epoch_schedule_vectors.rs` as regression tests.

---

## 1. Complete call tables

Legend — Origin: `CK` = signed coldkey, `HK` = signed hotkey (the account that holds the UID), `OWNER` = `SubnetOwner[netuid]` coldkey, `OWNER|ROOT` = `ensure_subnet_owner_or_root`, `ROOT` = `ensure_root` (sudo). Relevance: **core** (every validator/miner), **owner**, **opt** (optional/advanced), **n/a** (governance/infra). Fee: `No` = `Pays::No`. Pallet indices from `M/finney.json`: System 0, Balances 5, SubtensorModule 7, Utility 11, Sudo 12, Multisig 13, Proxy 16, Commitments 18, AdminUtils 19, Drand 26, Crowdloan 27, Swap 28, MevShield 30, AlphaAssets 31, LimitOrders 32. Newtypes `NetUid(u16)`, `MechId(u8)`, `TaoBalance(u64)`, `AlphaBalance(u64)`, `NetUidStorageIndex(u16)`, `PerU16(u16)` all SCALE-encode as the bare integer but carry distinct TypeInfo paths (`S/common/src/lib.rs:383-402` test).

### 1.1 SubtensorModule (pallet 7) — 95 calls at 477 (idx 60 `faucet` is `cfg(feature="pow-faucet")`, absent on finney)

| idx | call | params (SCALE) | origin | fee | preconditions / rate limits | main errors | rtao |
|---|---|---|---|---|---|---|---|
| 0 | `set_weights` | `netuid:NetUid, dests:Vec<u16>, weights:Vec<u16>, version_key:u64` | HK | No | rejected outright if `CommitRevealWeightsEnabled[netuid]` (dispatches.rs:68-80, default **true** lib.rs:1308-1310); else `do_set_weights` → mecid 0 (§2.1) | `CommitRevealEnabled`, §2.1 list | core (legacy) |
| 119 | `set_mechanism_weights` | `+ mecid:MechId` after netuid | HK | No | same as 0, per mechanism (dispatches.rs:128-141) | same | **core** |
| 80 | `batch_set_weights` | `netuids:Vec<Compact<NetUid>>, weights:Vec<Vec<(Compact<u16>,Compact<u16>)>>, version_keys:Vec<Compact<u64>>` | HK | No | per-item `do_set_weights` continuing on error; batch len 1..=TotalNetworks (`guards/check_weights.rs:78-95`); items on CR-enabled subnets fail individually (weights.rs:1239-1241) | `InputLengthsUnequal`, `InvalidBatchLength`, per-item `BatchWeightItemFailed` events | opt |
| 96 | `commit_weights` | `netuid, commit_hash:H256` | HK | No | CRv2 hash commit (§2.2) | `CommitRevealDisabled`, `CommittingWeightsTooFast`, `TooManyUnrevealedCommits`, `HotKeyNotRegisteredInSubNet` | opt (legacy CRv2) |
| 115 | `commit_mechanism_weights` | `netuid, mecid, commit_hash:H256` | HK | No | as 96 per mechanism | same | opt |
| 100 | `batch_commit_weights` | `netuids:Vec<Compact<NetUid>>, commit_hashes:Vec<H256>` | HK | No | len match + 1..=TotalNetworks | as 96 | opt |
| 97 | `reveal_weights` | `netuid, uids:Vec<u16>, values:Vec<u16>, salt:Vec<u16>, version_key:u64` | HK | No | CRv2 reveal in exactly epoch `commit_epoch + RevealPeriodEpochs` (§2.2) | `NoWeightsCommitFound`, `ExpiredWeightCommit`, `RevealTooEarly`, `InvalidRevealCommitHashNotMatch` | opt |
| 116 | `reveal_mechanism_weights` | `+ mecid` | HK | No | as 97 | same | opt |
| 98 | `batch_reveal_weights` | `netuid, uids_list:Vec<Vec<u16>>, values_list, salts_list:Vec<Vec<u16>>, version_keys:Vec<u64>` | HK | No | atomic validate-then-apply, mecid 0 only (weights.rs:826-969) | as 97 + `InputLengthsUnequal` | opt |
| 117 | `commit_crv3_mechanism_weights` | `netuid, mecid, commit:BoundedVec<u8,32768>, reveal_round:u64` | HK | No | = 118 with `commit_reveal_version` hard-coded 4 (dispatches.rs:399-414) | §2.3 | opt |
| 113 | `commit_timelocked_weights` | `netuid, commit:BoundedVec<u8,32768>, reveal_round:u64, commit_reveal_version:u16` | HK | No | CRv3 on mecid 0 (dispatches.rs:1949-1962) | §2.3 | core (single-mech) |
| 118 | `commit_timelocked_mechanism_weights` | `netuid, mecid, commit, reveal_round, commit_reveal_version` | HK | No | **the** validator call (dispatches.rs:2042-2058) | §2.3 | **core** |
| 4 | `serve_axon` | `netuid, version:u32, ip:u128, port:u16, ip_type:u8, protocol:u8, placeholder1:u8, placeholder2:u8` | HK | No | §3 | `HotKeyNotRegisteredInNetwork`, `InvalidIpType`, `InvalidIpAddress`, `InvalidPort`, `ServingRateLimitExceeded`, `SubnetNotExists` | **core** |
| 40 | `serve_axon_tls` | `… + certificate:Vec<u8>` | HK | No | §3; cert ≤ 65 bytes else silently not stored | as 4 | core |
| 5 | `serve_prometheus` | `netuid, version:u32, ip:u128, port:u16, ip_type:u8` | HK | No | §3 (ip 0 not allowed) | as 4 | opt |
| 7 | `burned_register` | `netuid, hotkey:AccountId` | CK | Yes | §4.1 | `SubNetRegistrationDisabled`, `HotKeyAlreadyRegisteredInSubNet`, `NotEnoughBalanceToStake`, `NonAssociatedColdKey`, `NoNeuronIdAvailable`, `TooManyRegistrationsThisBlock` (only pow path), `RegistrationNotPermittedOnRootSubnet` | **core** |
| 134 | `register_limit` | `netuid, hotkey, limit_price:u64` (rao) | CK | Yes | burned_register only if `Burn[netuid] <= limit_price` (registration.rs:362-395) | `RegistrationPriceLimitExceeded` + as 7 | core |
| 6 | `register` | `netuid, block_number:u64, nonce:u64, work:Vec<u8>, hotkey, coldkey` | CK | Yes | **PoW params ignored; behaves as burned_register** (dispatches.rs:804-818) | as 7 | avoid |
| 152 | `pow_register` | `netuid, work_block:u64, nonce:u64, work:[u8;32], hotkey` | CK | **No** | §4.2; validated in the tx extension before pool admission | `SubNetRegistrationDisabled`(pow flag), `InvalidWorkBlock`, `InvalidSeal`, `InvalidDifficulty`, `TooManyRegistrationsThisBlock`, `NoNeuronIdAvailable`, `TooManyStakingHotkeys`, `CannotUseSystemAccount` | core (if owner enables PoW) |
| 62 | `root_register` | `hotkey` | CK | Yes | §4.4 | `TooManyRegistrationsThisBlock/Interval`, `HotKeyAlreadyRegisteredInSubNet`, `NonAssociatedColdKey`, `StakeTooLowForRoot`, `NoNeuronIdAvailable` | opt |
| 59 | `register_network` | `hotkey` | CK | Yes | §5.1 | `NetworkTxRateLimitExceeded`, `CannotAffordLockCost`, `SubnetLimitReached`, `NonAssociatedColdKey`, `CannotUseSystemAccount`, `SubNetRegistrationDisabled`, `LockIdOverFlow` | **owner** |
| 79 | `register_network_with_identity` | `hotkey, identity:Option<SubnetIdentityV3>` | CK | Yes | §5.1 + `is_valid_subnet_identity` | + `InvalidIdentity` | owner |
| 110 | `register_leased_network` | `emissions_share:Percent, end_block:Option<u32>` | CK (crowdloan creator, called *via* `Crowdloan.finalize`) | Yes | `S/pallets/subtensor/src/subnets/leasing.rs:70-96` | `InvalidLeaseBeneficiary`, `LeaseCannotEndInThePast`, `LeaseNetuidNotFound` | opt |
| 111 | `terminate_lease` | `lease_id:u32, hotkey` | CK (beneficiary) | Yes | after `end_block`; hotkey must be owned by beneficiary (leasing.rs:181-199) | `LeaseDoesNotExist`, `LeaseHasNoEndBlock`, `LeaseHasNotEnded`, `BeneficiaryDoesNotOwnHotkey` | opt |
| 92 | `start_call` | `netuid` | OWNER | Yes | once; `now >= NetworkRegisteredAt + StartCallDelay` (0) (`subnets/subnet.rs:598-621`) | `FirstEmissionBlockNumberAlreadySet`, `StartCallNotReady`, `BadOrigin` | **owner** |
| 61 | `dissolve_network` | `coldkey, netuid` | **ROOT** | Yes | dispatches.rs:1078-1086 (`coldkey` unused) | — | n/a |
| 120 | `root_dissolve_network` | `netuid` | ROOT | Yes | dispatches.rs:2064-2070 | — | n/a |
| 78 | `set_subnet_identity` | `netuid, subnet_name, github_repo, subnet_contact, subnet_url, discord, description, logo_url, additional (all Vec<u8>)` | OWNER | Yes | field caps: name 256, github/contact/url/logo/description/additional 1024, discord 256, plus a total cap (`utils/identity.rs:205-229`) | `NotSubnetOwner`, `InvalidIdentity`, `SubnetNotExists` | owner |
| 112 | `update_symbol` | `netuid, symbol:Vec<u8>` | OWNER|ROOT | Yes | symbol must be in the on-chain symbol list and unused (dispatches.rs:1911-1926) | `SymbolDoesNotExist`, `SymbolAlreadyInUse` | owner |
| 141 | `trigger_epoch` | `netuid` | OWNER | Yes | CR must be **disabled**; no pending trigger; `remaining >= AdminFreezeWindow`; rate key `Hyperparameter::TriggerEpoch` (2 tempos) (`coinbase/tempo_control.rs:87-126`) | `DynamicTempoBlockedByCommitReveal`, `EpochTriggerAlreadyPending`, `AutoEpochAlreadyImminent`, `TxRateLimitExceeded` | owner |
| 139 | `set_tempo` | `netuid, tempo:u16` | any | Yes | **deprecated no-op** (returns Ok, changes nothing) dispatches.rs:2729-2731 | — | avoid |
| 140 | `set_activity_cutoff_factor` | `netuid, factor_milli:u32` | any | Yes | deprecated no-op (dispatches.rs:2743-2749) | — | avoid |
| 129/130 | `enable/disable_voting_power_tracking` | `netuid` | OWNER|ROOT | Yes | dispatches.rs:2521-2551 | `VotingPowerTrackingNotEnabled` | opt |
| 131 | `sudo_set_voting_power_ema_alpha` | `netuid, alpha:u64` (1e18 = 1.0) | ROOT | Yes | — | `InvalidVotingPowerEmaAlpha` | n/a |
| 124 | `sudo_set_root_claim_threshold` | `netuid, new_value:u64` | OWNER|ROOT but only `netuid.is_root()` accepted | Yes | ≤ 10_000_000 (dispatches.rs:2347-2369) | `InvalidRootClaimThreshold` | n/a |
| 2 | `add_stake` | `hotkey, netuid, amount_staked:TaoBalance` | CK | Yes | subnet exists, `SubtokenEnabled`, ≥ min stake (0.002 TAO), balance, hotkey account exists, liquidity (`staking/stake_utils.rs` `validate_add_stake`) | `AmountTooLow`, `NotEnoughBalanceToStake`, `HotKeyAccountNotExists`, `InsufficientLiquidity`, `SubtokenDisabled` | core-ish |
| 88 | `add_stake_limit` | `+ limit_price:TaoBalance (rao/alpha), allow_partial:bool` | CK | Yes | fill-or-kill unless `allow_partial` | + `SlippageTooHigh` | opt |
| 132 | `add_stake_burn` | `hotkey, netuid, amount:TaoBalance, limit:Option<TaoBalance>` | CK | Yes | buy-then-burn (buyback); rate key `AddStakeBurn(netuid)` = 1 tempo | `AddStakeBurnRateLimitExceeded` | opt |
| 3 | `remove_stake` | `hotkey, netuid, amount_unstaked:AlphaBalance` | CK | Yes (fee may be taken in alpha) | `validate_remove_stake`; locked collateral/conviction not withdrawable | `NotEnoughStakeToWithdraw`, `AmountTooLow`, `StakeUnavailable`, `InsufficientLiquidity` | core-ish |
| 89 | `remove_stake_limit` | `+ limit_price, allow_partial` | CK | Yes | — | + `SlippageTooHigh` | opt |
| 103 | `remove_stake_full_limit` | `hotkey, netuid, limit_price:Option<TaoBalance>` | CK | Yes | — | as 89 | opt |
| 83/84 | `unstake_all` / `unstake_all_alpha` | `hotkey` | CK | Yes | ≤ 16 legs per call (`MAX_UNSTAKE_ALL_LEGS`, lib.rs:120) | — | opt |
| 85 | `move_stake` | `origin_hotkey, destination_hotkey, origin_netuid, destination_netuid, alpha_amount:AlphaBalance` | CK | Yes | `u64::MAX` = whole live position (spec 469+) | `SameNetuid`… | opt |
| 149 | `move_stake_limit` | `… + limit_price:TaoBalance (dest-alpha per origin-alpha ×1e9), allow_partial` | CK | Yes | added runtime 448 (`S/docs/migration.mdx:556`) | `SlippageTooHigh` | opt |
| 86 | `transfer_stake` | `destination_coldkey, hotkey, origin_netuid, destination_netuid, alpha_amount` | CK | Yes | `TransferToggle[netuid]` must be true | `TransferDisallowed`, `AccountRejectsLockedAlpha` | opt |
| 143 | `transfer_stake_and_hotkey` | `destination_coldkey, origin_hotkey, destination_hotkey, origin_netuid, destination_netuid, alpha_amount` | CK | Yes | as 86 | as 86 | opt |
| 87/90 | `swap_stake` / `swap_stake_limit` | `hotkey, origin_netuid, destination_netuid, alpha_amount (+limit_price, allow_partial)` | CK | Yes | — | `SameNetuid`, `SlippageTooHigh` | opt |
| 101/102 | `recycle_alpha` / `burn_alpha` | `hotkey, amount:AlphaBalance, netuid` | CK | Yes | — | `NotEnoughAlphaOutToRecycle`, `CannotBurnOrRecycleOnRootSubnet` | opt |
| 66 | `increase_take` | `hotkey, take:PerU16` | CK | Yes | strictly greater; ≤ `MaxDelegateTake` (11796 = 18%); rate `TxDelegateTakeRateLimit` 216000 blocks (`staking/increase_take.rs:42-56`) | `DelegateTakeTooLow/High`, `DelegateTxRateLimitExceeded` | opt |
| 65 | `decrease_take` | `hotkey, take:PerU16` | CK | Yes | strictly lower; ≥ `MinDelegateTake` (0); not rate-limited | `DelegateTakeTooLow` | opt |
| 67 | `set_children` | `hotkey, netuid, children:Vec<(u64 proportion, AccountId)>` | CK | Yes | ≤ 5 children, no dup, child ≠ hotkey, Σ proportion ≤ u64::MAX, non-root, min own stake; rate `SetChildren` = 150 blocks; takes effect after `PendingChildKeyCooldown` (`staking/set_children.rs:41-61, 504-605`) | `TooManyChildren`, `DuplicateChild`, `InvalidChild`, `ProportionOverflow`, `NotEnoughStakeToSetChildkeys`, `TxRateLimitExceeded` | opt |
| 75 | `set_childkey_take` | `hotkey, netuid, take:PerU16` | CK | Yes | ≤ `MaxChildkeyTake` (18%); ≥ `MinChildkeyTakePerSubnet`; rate `TxChildkeyTakeRateLimit` 216000 | `InvalidChildkeyTake`, `TxChildkeyTakeRateLimitExceeded` | opt |
| 135 | `set_auto_parent_delegation_enabled` | `hotkey, enabled:bool` | CK | Yes | dispatches.rs:2637-2650 | `NonAssociatedColdKey` | opt |
| 114 | `set_coldkey_auto_stake_hotkey` | `netuid, hotkey` | CK | Yes | hotkey registered on netuid (dispatches.rs:1976-2016) | `HotKeyNotRegisteredInSubNet`, `SameAutoStakeHotkeyAlreadySet` | opt |
| 68 | `set_identity` | `name, url, github_repo, image, discord, description, additional (Vec<u8>)` | CK | Yes | must own a hotkey registered somewhere; caps name/url/github/discord 256, image/description/additional 1024 (`utils/identity.rs:27-66,164-189`) | `HotKeyNotRegisteredInNetwork`, `InvalidIdentity` | opt |
| 91 | `try_associate_hotkey` | `hotkey` | CK | Yes | charged even if already associated | — | opt |
| 93 | `associate_evm_key` | `netuid, evm_key:H160, block_number:u64, signature:ecdsa::Signature([u8;65])` | HK | No | sig over `keccak256(hotkey ++ keccak256(scale(block_number)))`; rate 7200 blocks per uid; ≤ 32 uids per EVM key (`utils/evm.rs:46-119,232-241`) | `EvmKeyAssociateRateLimitExceeded`, `InvalidRecoveredPublicKey`, `EvmKeyAssociationLimitExceeded` | opt |
| 70 | `swap_hotkey` | `hotkey, new_hotkey, netuid:Option<NetUid>` | CK | Yes (alpha-payable) | deprecated → use 72 | `HotKeySwapOnSubnetIntervalNotPassed`, `NotEnoughBalanceToPaySwapHotKey` | opt |
| 72 | `swap_hotkey_v2` | `hotkey, new_hotkey, netuid:Option<NetUid>, keep_stake:bool` | CK | Yes | cost `KeySwapCost` 0.1 TAO (all-subnet) / `KeySwapOnSubnetCost` 0.001 TAO + 7200-block interval per subnet | `KeepStakeBlockedByCollateral`, `NewHotKeyIsSameWithOld` | opt |
| 125 | `announce_coldkey_swap` | `new_coldkey_hash:H256` (BlakeTwo256 of new AccountId) | CK | Yes | charges `KeySwapCost` once; executable after `ColdkeySwapAnnouncementDelay` 36000 blocks (dispatches.rs:2384-2414) | `ColdkeySwapReannouncedTooEarly` | opt |
| 126 | `swap_coldkey_announced` | `new_coldkey` | CK | Yes | hash must match; `now >= when` | `ColdkeySwapAnnouncementNotFound`, `AnnouncedColdkeyHashDoesNotMatch`, `ColdkeySwapTooEarly` | opt |
| 127/133 | `dispute_coldkey_swap` / `clear_coldkey_swap_announcement` | `()` | CK | Yes | — | `ColdkeySwapAlreadyDisputed`, `ColdkeySwapClearTooEarly` | opt |
| 71/128 | `swap_coldkey` / `reset_coldkey_swap` | — | ROOT | — | — | — | n/a |
| 73 | `schedule_swap_coldkey` | `new_coldkey` | — | Yes | always `Err(Deprecated)` (dispatches.rs:1143-1148) | `Deprecated` | avoid |
| 136 | `lock_stake` | `hotkey, netuid, amount:AlphaBalance` | CK | Yes | one lock per (coldkey, netuid); must match existing hotkey (`staking/lock.rs` `do_lock_stake`) | `LockHotkeyMismatch`, `InsufficientStakeForLock` | opt |
| 137 | `move_lock` | `destination_hotkey, netuid` | CK | Yes | resets conviction | `NoExistingLock` | opt |
| 138 | `set_perpetual_lock` | `netuid, enabled:bool` | CK | Yes | — | — | opt |
| 142 | `set_reject_locked_alpha` | `enabled:bool` | CK | Yes | default reject | — | opt |
| 144 | `add_collateral` | `netuid, hotkey, alpha:AlphaBalance, limit_price:TaoBalance` | CK | Yes | §4.3 | `AmountTooLow`, `SlippageTooHigh`, `StakeUnavailable`, `NotEnoughBalanceToStake` | core (collateral subnets) |
| 145 | `set_min_collateral` | `netuid, hotkey, min_locked:AlphaBalance` | CK | Yes | non-root, owns hotkey | `NonAssociatedColdKey` | core (collateral subnets) |
| 121 | `claim_root` | `subnets:BTreeSet<NetUid>` (ignored) | CK | Yes | admission bound 256 hotkeys (dispatches.rs:2100-2135) | `RootClaimTooHeavy` | opt |
| 148 | `claim_root_with_hotkey` | `hotkey` | CK | Yes | preferred | `RootClaimTooHeavy` | opt |
| 147 | `stake_into_basket` | `hotkey, amount_staked:TaoBalance` | CK | Yes | — | `HotKeyNotRegisteredInSubNet`(root) | opt |
| 150 | `swap_basket` | `hotkey, origin_netuid, destination_netuid, amount:AlphaBalance, min_amount_out:u64` | CK (or `BasketTrading` proxy) | Yes | validated in tx-ext incl. through batch/proxy wrappers (`extensions/subtensor.rs:131-213`) | `BasketTradingDisabled`, `BasketSlippage…` | n/a |
| 151 | `swap_basket_many` | `hotkey, legs:BoundedVec<(NetUid,NetUid,AlphaBalance,u64),64>` | CK | Yes | — | `BasketSwapBatchEmpty` | n/a |
| 69/76/77/109 | `sudo_set_tx_childkey_take_rate_limit(u64)`, `sudo_set_min/max_childkey_take(PerU16)`, `set_pending_childkey_cooldown(u64)` | — | ROOT | — | — | — | n/a |

Weight-call dispatch classes/weights: all weight calls are `DispatchClass::Normal, Pays::No`, and their declared weight is mode-dependent (`timelock_weight_for_mode`, weights.rs:29-36) — in Null mode the reveal/commit envelopes are larger.

### 1.2 AdminUtils (pallet 19) — 101 calls; owner-vs-root in §5.3

Signature shapes (from `_call_index_map.txt`): per-subnet setters take `netuid:NetUid` + value; fixed-point values are `U64F64` (`substrate_fixed::FixedU128<U64>` — SCALE `{ bits: u128 }`, 2^64 = 1.0) for `burn_increase_mult`(90) / `collateral_drain_ratio`(99) / `tao_flow_normalization_exponent`(82) / `emission_bar_quantile`(100) / `emission_gate_exponent`(101); `I96F32` for `subnet_moving_alpha`(63); `I64F64` for `tao_flow_cutoff`(81); `PerU16(u16)` for takes; `EpochConsensus` enum `{Yuma=0, Null=1}` (lib.rs:505-520); `ConsensusMode` enum for 104; `RecycleOrBurnEnum {Burn=0, Recycle=1}` (lib.rs:484-490); `Option<Vec<u16>>` for 77.

### 1.3 Commitments (pallet 18)

| idx | call | params | origin | fee | rules |
|---|---|---|---|---|---|
| 0 | `set_commitment` | `netuid, info:Box<CommitmentInfo{fields:BoundedVec<Data,3>}>` | HK (must be registered on netuid: `S/runtime/src/lib.rs:822-838` `AllowCommitments`) | **No** | ≤ 3 fields; per-(netuid,who) space budget `MaxSpace` = 3100 bytes per epoch (`S/pallets/commitments/src/lib.rs:219-221,258-298`), each call counted as `max(100, Σ field len)`; `ResetBondsFlag` triggers bonds reset if `BondsResetOn`; `TimelockRevealFailed` not submittable. Deposit 0 (`S/runtime/src/lib.rs:807-809`). |
| 2 | `set_max_space` | `new_limit:u32` | ROOT | — | — |

`Data` has a **custom SCALE codec** (`S/pallets/commitments/src/types.rs:102-180`): tag byte 0 = None; 1..=129 = `Raw` with length = tag−1 (≤128 bytes, inline, no compact prefix); 130/131/132/133 = `BlakeTwo256/Sha256/Keccak256/ShaThree256([u8;32])`; 134 = `TimelockEncrypted{ encrypted: BoundedVec<u8,1024> (compact-prefixed), reveal_round: u64 }`; 135 = `ResetBondsFlag`; 136 = `BigRaw(BoundedVec<u8,512>)`; 137 = `TimelockRevealFailed{…}`. Metadata exposes this as `Raw0..Raw128` variants (`M/explore` output above) — scale-encoding via a generic metadata-driven encoder works, but a hand-written codec must replicate the tag scheme exactly. Constants: `MAX_TIMELOCK_COMMITMENT_SIZE_BYTES = 1024`, `MAX_BIGRAW_COMMITMENT_SIZE_BYTES = 512` (`types.rs:421-422`).

### 1.4 Proxy (pallet 16) — subtensor fork of pallet-proxy

Calls (`S/pallets/proxy/src/lib.rs:246-720`): 0 `proxy(real:MultiAddress, force_proxy_type:Option<ProxyType>, call:Box<RuntimeCall>)`, 1 `add_proxy(delegate, proxy_type, delay:u32)`, 2 `remove_proxy`, 3 `remove_proxies`, 4 `create_pure(proxy_type, delay, index:u16)`, 5 `kill_pure(spawner, proxy_type, index, #[compact] height:u32, #[compact] ext_index:u32)`, 6 `announce(real, call_hash:H256)`, 7 `remove_announcement`, 8 `reject_announcement(delegate, call_hash)`, 9 `proxy_announced(delegate, real, force_proxy_type, call)`, 10 `poke_deposit()`, 11 `set_real_pays_fee(delegate, pays_fee:bool)` (fee-propagation opt-in used by the payment wrapper, `transaction_payment_wrapper.rs:107-206`).

`ProxyType` (u8 index): Any 0, Owner 1, NonCritical 2, NonTransfer 3, Senate 4 (dep.), NonFungible 5, Triumvirate 6 (dep.), Governance 7 (dep.), Staking 8, Registration 9, Transfer 10, SmallTransfer 11, RootWeights 12 (dep.), ChildKeys 13, SudoUncheckedSetCode 14, SwapHotkey 15, SubnetLeaseBeneficiary 16, RootClaim 17, BasketTrading 18 (`S/common/src/proxy.rs:25-67`). Deny-by-default allow-lists (`S/runtime/src/proxy_filters/mod.rs:24-118`, groups in `call_groups.rs`):
- **Owner** = `SubnetIdentityCalls` (set_subnet_identity, update_symbol) + `SubnetManagementCalls` (the owner-settable AdminUtils setters, deprecated `set_tempo`/`set_activity_cutoff_factor`) — **not** `start_call`, not `sudo_set_sn_owner_hotkey`, not `register_network`.
- **SubnetLeaseBeneficiary** = Owner + `start_call`.
- **Staking** = `StakeManagementCalls` (add/remove/move/swap stake variants, unstake_all*, stake_into_basket, add_collateral, set_min_collateral); not transfer_stake, not `Utility.batch_all`.
- **Registration** = `pow_register` + `register`/`register_limit`/`burned_register`.
- **ChildKeys** = `set_children`, `set_childkey_take`. **SwapHotkey** = `swap_hotkey(_v2)`. **RootClaim** = `claim_root*`. **BasketTrading** = `swap_basket*`.
- **NonTransfer** = InfraCommonCalls (System/Timestamp/Grandpa/Utility/Preimage/Scheduler/Proxy/Commitments/SafeMode/Ethereum/BaseFee/Drand/Swap/MevShield(minus store_encrypted)/LimitOrders) + all AdminUtils + stake mgmt + registrations + hotkey swap + dissolve + childkeys + root claim + identity + start_call + `SubtensorValueCalls` (add_stake_burn, lock_stake, move_lock, set_perpetual_lock, recycle/burn alpha, register_network*, register_leased_network) + `SubtensorCommonCalls` (all weight calls, serve_*, set_identity, take, evm, auto-stake, trigger_epoch, terminate_lease, voting power).
- **NonFungible** = infra + admin + pow_register + dissolve + childkeys + root claim + identity + start_call + `SubtensorCommonCalls` (weights/serving/identity are here → a hotkey can be a NonFungible proxy target).
- **SmallTransfer** = `transfer_keep_alive`/`transfer_allow_death` with `value < 0.5 TAO`, `transfer_stake(_and_hotkey)` with `alpha_amount < 0.5 α` (`call_groups.rs:654-662`, limits `S/common/src/lib.rs:50-51`).
Deposits: `ProxyDepositBase` 0.06 TAO, `ProxyDepositFactor` 0.033 TAO, `MaxProxies` 20, `MaxPending` 75, `AnnouncementDepositBase` deposit(1,16) (`M/explore` constants; `S/runtime/src/lib.rs:578-601`). Runtime API `ProxyFilterRuntimeApi.get_proxy_filters/get_proxy_types` returns the allow-lists as `ProxyFilterInfo{proxy_type:u8,name,deprecated,filter_mode}` (`S/common/src/proxy.rs:190-223`) — rtao can fetch the live filter instead of hard-coding it.

### 1.5 Multisig (pallet 13), Utility (pallet 11)

Multisig: 0 `as_multi_threshold_1(other_signatories:Vec<AccountId>, call)`, 1 `as_multi(threshold:u16, other_signatories, maybe_timepoint:Option<Timepoint{height:u32,index:u32}>, call:Box<RuntimeCall>, max_weight:Weight{ref_time:Compact<u64>,proof_size:Compact<u64>})`, 2 `approve_as_multi(…, call_hash:[u8;32], max_weight)`, 3 `cancel_as_multi(threshold, other_signatories, timepoint, call_hash)`, 4 `poke_deposit`. Constants: `DepositBase` 0.132 TAO, `DepositFactor` 0.032 TAO, `MaxSignatories` 100 (`M/explore`; `S/runtime/src/lib.rs:555-575`).

Utility: 0 `batch`, 1 `as_derivative(index:u16, call)`, 2 `batch_all`, 3 `dispatch_as`, 4 `force_batch`, 5 `with_weight`, 6 `if_else(main, fallback)`, 7 `dispatch_as_fallible`. `batched_calls_limit` = 10922 (metadata). **Nested batches are forbidden** by the runtime base call filter `NoNestingCallFilter` (`S/runtime/src/lib.rs:275-295`): a `batch`/`batch_all`/`force_batch` containing another batch variant is filtered. The tx extension walks batch/as_derivative/if_else/proxy wrappers for basket-trade validation only (`extensions/subtensor.rs:131-213`) — other guards (weights, rate limits, serving) are **not** re-run on inner calls at pool admission, but the same guards run as `DispatchExtension`s at dispatch (`S/runtime/src/lib.rs:354-361`), so a batched `set_weights` that fails a guard fails at dispatch (and `batch` continues, `batch_all` reverts).

### 1.6 Drand (pallet 26), Crowdloan (27), Swap (28), MevShield (30), AlphaAssets (31), LimitOrders (32)

- **Drand**: 0 `write_pulse(pulses_payload:PulsesPayload{block_number:u32, pulses:Vec<Pulse{round:u64, randomness:BoundedVec<u8,32>, signature:BoundedVec<u8,144>}>, public:MultiSigner}, signature:Option<MultiSignature>)` — **unsigned, offchain-worker only** (`ensure_none`, `S/pallets/drand/src/lib.rs:328-335`; `validate_unsigned` at :292-320 whitelists only `write_pulse`; after the first pulse only `last_stored_round + 1` is accepted, `S/docs/migration.mdx:612-616`); 1 `set_beacon_config` ROOT; 2 `set_oldest_stored_round(u64)` ROOT. Clients never call Drand; they read `Drand.Pulses[round]` / `LastStoredRound`. Quicknet config: period 3 s, genesis 1_692_803_367, chain hash `52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971`, pubkey `83cf0f28…` (`lib.rs:91-92,185-210`); `MAX_KEPT_PULSES` 216_000 (:97); `UnsignedPriority` 1_048_576.
- **Crowdloan** (`S/pallets/crowdloan/src/lib.rs:333-951`): 0 `create(#[compact] deposit, min_contribution, cap, end:u32, call:Option<Box<RuntimeCall>>, target_address:Option<AccountId>)`, 1 `contribute(#[compact] crowdloan_id:u32, #[compact] amount)`, 2 `withdraw`, 3 `finalize` (creator; dispatches the stored call — typically `register_leased_network`), 4 `refund` (≤ 50 contributors/call), 5 `dissolve`, 6 `update_min_contribution`, 7 `update_end`, 8 `update_cap`, 9 `set_max_contribution(…, Option<Balance>)`; all CK-signed. Note the `#[pallet::compact]` on amounts/ids. Constants: `MinimumDeposit` 10 TAO, `AbsoluteMinimumContribution` 0.1 TAO, duration 50_400..432_000 blocks, `MaxContributors` 500 (`S/runtime/src/lib.rs:1410-1420`).
- **Swap**: 0 `set_fee_rate(netuid, rate:u16)` ROOT; 1-5 (`add_liquidity`, `remove_liquidity`, `modify_position`, `toggle_user_liquidity`, `disable_lp`) all return `Err(Deprecated)` (`S/pallets/swap/src/pallet/mod.rs:214-269`). User liquidity is permanently disabled; `user_liquidity_enabled` hyperparam reads false (`S/docs/hyperparameters/index.mdx`). **Ignore for rtao.**
- **MevShield** (`S/pallets/shield/src/lib.rs:272-446`): 0 `announce_next_key` unsigned (block author); 1 `submit_encrypted(ciphertext:BoundedVec<u8,8192>)` CK — ciphertext layout `key_hash(16, xxhash128 of NextKey) || kem_len(u16 LE) || kem_ct (ML-KEM-768) || nonce(24) || aead_ct (XChaCha20-Poly1305 of the signed extrinsic)` (:331-349); the wrapped extrinsic must be signed with the **next** nonce; the outer tx must use `Era::Mortal(period ≤ 8)` (`S/runtime/src/check_mortality.rs:19-34,94-105`); 2 `store_encrypted` (weight-penalised queue); 3-6 ROOT knobs. The SDK routes `AddCollateral` through `submit_shielded` automatically (`S/docs/guides/mining/collateral.mdx:131`). Optional for rtao (anti-MEV for stake ops); not needed for weights.
- **AlphaAssets**: no calls (`M/explore/AlphaAssets_calls.txt`).
- **LimitOrders** (`S/pallets/limit-orders/src/lib.rs:528-660`): 0 `execute_orders(orders:BoundedVec<SignedOrder,100>, should_fail:bool)` relayer-signed, 1 `execute_batched_orders(netuid, orders)`, 2 `cancel_order(order:VersionedOrder)`, 3 `set_pallet_status(bool)` ROOT, 4 `prune_linked_output(order_id:H256)`. Off-chain signed-order relay market; **out of scope** for subnet tooling.

---

## 2. Weight-setting paths, precisely

### 2.1 Plain `set_weights` / `set_mechanism_weights` → `internal_set_weights` (weights.rs:971-1136)

Check order (each `ensure!` cites weights.rs):
1. signer = hotkey; Null-mode permit gate `ensure_null_weight_permit` (:17-26).
2. `!netuid.is_root()` → `CanNotSetRootNetworkWeights` (:988).
3. `uids.len() == values.len()` → `WeightVecNotEqualSize` (:991-994).
4. `ensure_mechanism_exists(netuid, mecid)`: subnet exists and `mecid < MechanismCountCurrent[netuid]` → `MechanismDoesNotExist` (`subnets/mechanism.rs:92-104`).
5. `uids.len() <= SubnetworkN[netuid]` → `UidsLengthExceedUidsInSubNet` (:1000-1003).
6. hotkey registered on netuid → `HotKeyNotRegisteredInSubNet` (:1006-1009).
7. `check_weights_min_stake`: owner-hotkey uid always passes; else `stake_weight(hotkey, netuid) >= StakeThreshold` (global; default 0; mainnet non-zero ≈1000 TAO-equiv per docs) → `NotEnoughStakeToSetWeights` (lib.rs:3727-3738).
8. `check_version_key`: passes iff `WeightsVersionKey[netuid] == 0 || version_key >= WeightsVersionKey[netuid]` → `IncorrectWeightVersionKey` (:1285-1292). The key is opaque; the SDK sends 0 unless told.
9. Rate limit **only when commit-reveal is disabled**: `check_rate_limit(netuid_index, uid, now)`: `LastUpdate[netuid_index][uid] == 0 || now - last >= WeightsSetRateLimit[netuid]` (default 100 blocks, lib.rs:1088-1090) → `SettingWeightsTooFast` (:1024-1032, :1295-1316).
10. `check_validator_permit`: Null mode → must hold permit; Yuma → self-weight (single entry naming own uid) OR owner-hotkey uid OR `ValidatorPermit[netuid][uid]` → `NeuronNoValidatorPermit` (:1350-1368).
11. no duplicate uids → `DuplicateUids` (:1038); all uids `< SubnetworkN` → `UidVecContainInvalidOne` (:1041-1044).
12. `check_length`: `len >= min(MinAllowedWeights[netuid], SubnetworkN)` unless self-weight → `WeightVecLengthIsLow` (:1370-1394).
13. **Normalization**: Yuma → `vec_u16_max_upscale_to_u16(values)` (largest value → 65535, others scaled, rounding; `S/pallets/subtensor/src/epoch/math.rs:78-129`); Null → values stored verbatim (:1057-1062).
14. `MaxWeightExceeded` check — dead code (§0.7) (:1065-1075).
15. Store `Weights[netuid_index][uid] = Vec<(uid,u16)>`; if CR disabled, `LastUpdate[netuid_index][uid] = now`; emit `WeightsSet(NetUidStorageIndex, uid)` (:1078-1097).

`netuid_index = mecid * 4096 + netuid` (`NetUidStorageIndex`, `mechanism.rs:28-45`); mecid 0 == netuid. Storage keys for `Weights`, `Bonds`, `Incentive`, `LastUpdate`, `WeightCommits`, `TimelockedWeightCommits` are all by `NetUidStorageIndex` (lib.rs:3029-3053).

Pool-time preflight (`SubtensorTransactionExtension` → `CheckWeights`, `CheckRateLimits`): batch shape + size, min-stake (Null: permit), CR-enabled rejection of `set_weights`/`set_mechanism_weights`, rate limit (`guards/check_weights.rs:41-46,99-141,262-271`; `guards/check_rate_limits.rs:35-105`). Errors surface as `InvalidTransaction::Custom(code)`: `RateLimitExceeded=6`, `StakeAmountTooLow=1`, `UidNotFound=19` (HotKeyNotRegisteredInSubNet), `BadRequest=255` for the rest (`S/common/src/transaction_error.rs:4-70`; mapping `extensions/subtensor.rs:33-67`).

### 2.2 Commit-reveal v2 (hash commits: 96/115/100 → 97/116/98)

- Commit: `commit_hash = BlakeTwo256::hash_of(&(hotkey:AccountId32, netuid_index:NetUidStorageIndex(u16), uids:Vec<u16>, values:Vec<u16>, salt:Vec<u16>, version_key:u64))` (weights.rs:1524-1535) — i.e. SCALE tuple: 32 raw bytes, u16 LE, three compact-length-prefixed `Vec<u16>` (LE items), u64 LE. The **values hashed must be exactly what the chain will set** (post-normalization is applied at reveal, so hash the pre-upscale u16s you submit). `batch_reveal_weights` hashes with `netuid:NetUid` instead of the index (weights.rs:869-876) — identical bytes for mecid 0.
- Commit checks: CR enabled, registered, rate limit on `netuid_index` (`CommittingWeightsTooFast`), `< 10` unrevealed → `TooManyUnrevealedCommits`; stored as `(hash, commit_epoch, commit_block, 0)` with `commit_epoch = current_epoch_with_lookahead(netuid)` (weights.rs:150-250, :1471-1481); `LastUpdate` set at **commit** time.
- Reveal window: valid iff `current_epoch_with_lookahead == commit_epoch + RevealPeriodEpochs[netuid]` (:1451-1457); expired iff `>` (:1483-1488). `RevealPeriodEpochs` default 1, bounds 1..=100 (`MIN/MAX_COMMIT_REVEAL_PEROIDS`, lib.rs:287-289). Reveal pops all commits up to and including the matched one, then runs `do_set_mechanism_weights` (so all §2.1 checks apply at reveal, with the rate-limit step skipped).
- Pool tag: commit calls provide `("weight-commit", hotkey, netuid_index)` when `WeightsSetRateLimit != 0` (`check_rate_limits.rs:113-131`) — **only one pending commit per hotkey/mechanism can sit in the pool**; a replacement with the same flat priority is rejected until the first is included or dropped. rtao retry logic must not blindly resubmit.

### 2.3 Commit-reveal v3 (timelocked; 113/117/118) — the production path

- Payload `WeightsTlockPayload { hotkey: Vec<u8>, uids: Vec<u16>, values: Vec<u16>, version_key: u64 }` SCALE-encoded (`coinbase/reveal_commits.rs:21-27`; SDK mirror `S/sdk/bittensor-core/src/timelock/mod.rs:52-58`). `hotkey` must decode as the committer's `AccountId32` (reveal_commits.rs:132-147) — i.e. the Vec contains the 32 raw key bytes. A legacy payload without `hotkey` is still accepted (`LegacyWeightsTlockPayload`, :29-35).
- Encryption: `tle::<TinyBLS381, AESGCMStreamCipherProvider>(quicknet_pubkey, esk:[u8;32] random, payload, Identity::new(b"", vec![sha256(reveal_round.to_be_bytes())]))`, then ark `serialize_compressed` (`timelock/mod.rs:104-139`). Chain decrypts with `tld` using `Pulses[reveal_round].signature` (reveal_commits.rs:58-121); ciphertext header `v`,`w` must be 32 bytes each.
- Call checks (weights.rs:379-492): mechanism exists; Null permit gate; CR enabled; `commit_reveal_version == CommitRevealWeightsVersion` (global, default **4**, lib.rs:1313-1316) → `IncorrectCommitRevealVersion`; registered; rate limit on `netuid_index` → `CommittingWeightsTooFast`; size: Yuma `<= 5000` bytes (`YUMA_COMMIT_SIZE_BYTES`, lib.rs:64) → `CommitPayloadTooLarge`, Null `<= floor(32768 / mechanism_count)` with a shared per-subnet-epoch queue of 64 KiB / 64 entries and stake-priority eviction (`enqueue_null_timelock`, :530-648; `CommitQueueFull`); `< 10` unrevealed per hotkey per epoch (Yuma). Pool-time: `reveal_round >= Drand.LastStoredRound` else `InvalidRevealRound` (code 15) (`check_weights.rs:252-257`).
- Storage: `TimelockedWeightCommits[netuid_index][commit_epoch] = VecDeque<(who, commit_block, ciphertext, reveal_round)>`, keyed by `current_epoch_with_lookahead` (weights.rs:434-440).
- **Reveal (chain-side, no client call)**: every block, `block_step` runs `reveal_crv3_commits()` **before** `run_coinbase` (`coinbase/block_step.rs:11-20`). For each subnet/mechanism: `reveal_epoch = cur_epoch(with lookahead) - RevealPeriodEpochs`; entries for `reveal_epoch` are taken; if `Pulses[reveal_round]` is missing the entry is re-queued and retried next block; decrypt failures are dropped; success calls `do_set_mechanism_weights` (all §2.1 checks incl. version key and permit at reveal time) and emits `TimelockedWeightsRevealed(netuid_index, who)`; queues for epochs `< reveal_epoch` are deleted (reveal_commits.rs:37-57,174-204). Consequence: the drand round must be **published and ingested by the chain within the reveal epoch** (≈ tempo blocks); too early (< LastStoredRound at commit) is rejected, too late is silently lost.
- **Reveal-round computation (what rtao must port)**: `generate_commit_v2` (`timelock/mod.rs:166-197`): simulate the pallet's epoch scheduler from a pinned snapshot `{LastEpochBlock, PendingEpochAt, SubnetEpochIndex, Tempo, BlocksSinceLastStep, current_block}` (SDK reads them at one block hash, `S/sdk/python/bittensor/intents/weights.py:283-306`); `extrinsic_block = head + 1` (`COMMIT_INCLUSION_BLOCK_OFFSET`); `commit_epoch = epoch_with_lookahead(extrinsic_block)`; find the first block `r` where `epoch_with_lookahead(r) == commit_epoch + reveal_period` (`epoch_schedule.rs:95-135`); `target = r + 3` (`SECURITY_BLOCK_OFFSET`); `reveal_round = floor((now_unix + (target - current_block) * block_time - GENESIS_TIME) / 3)`, min 1 (`timelock/mod.rs:174-187`). The pallet predicate: `should_run_epoch`: `tempo==0 → false; PendingEpochAt>0 && block>=pending → true; BlocksSinceLastStep > tempo → true; block - LastEpochBlock >= tempo` (`run_coinbase.rs:1224-1238`) — note §0.11. Epoch firing may also be deferred by `MaxEpochsPerBlock` (2 on prod; constant `InitialMaxEpochsPerBlock`), which the SDK does not model (`epoch_schedule.rs:73-75`) — the +3-block offset absorbs small deferrals.
- Validator per-tempo procedure on a Yuma/CR subnet: (1) score miners; (2) build `(uids, u16 values)` via max-upscale and drop zeros; (3) read schedule snapshot + `RevealPeriodEpochs` + `CommitRevealWeightsVersion` + `WeightsVersionKey`; (4) compute `reveal_round`, encrypt; (5) submit `commit_timelocked_mechanism_weights` for each mecid < `MechanismCountCurrent` (each mechanism has its own rate-limit slot, `check_rate_limit` uses `netuid_index`); (6) watch `TimelockedWeightsCommitted(who, netuid_index, blake2(commit), reveal_round)` and later `TimelockedWeightsRevealed`; (7) respect `WeightsSetRateLimit` per mechanism (100 blocks default) and the activity cutoff (`activity_cutoff_factor` per-mille of tempo, default 13 889 → 5000 blocks at tempo 360, `S/docs/guides/validating.mdx`).
- Null consensus (`EpochConsensus::Null`): only the single elected permit holder may commit/reveal/set (`ensure_null_weight_permit`, weights.rs:17-26; `check_weights.rs:97-107`); values are **exact integers** (no upscale), up to 2 500 UIDs budget (`NULL_UID_BUDGET`, mechanism.rs:35), ciphertext up to 32 KiB/mech count; `trigger_epoch` and the SDK `raw_u16` flag exist for it (`S/docs/guides/null-consensus.mdx`).

### 2.4 Multi-mechanism model

`MechanismCountCurrent[netuid]` (default 1, lib.rs:3552-3555) ≤ `MaxMechanismCount` (global default **2**, :3558-3561; hard cap 16, `mechanism.rs:33`); `MaxAllowedUids × count ≤ epoch_uid_budget` (256 Yuma / 2500 Null, `mechanism.rs:106-126`). Emission split `MechanismEmissionSplit[netuid]: Option<Vec<u16>>` summing to 65535, else even; rounding remainder to mechanism 0 (`mechanism.rs:284-337`). Reducing the count clears Weights/Bonds/Incentive/LastUpdate/commits of removed mechanisms and resets the split (:252-282). Owner calls: `sudo_set_mechanism_count` (rate 7200 blocks, `MechanismCountSetRateLimit`, lib.rs:3568-3571) and `sudo_set_mechanism_emission_split` (rate 7200, :3574-3577); both also need the admin window open.

---

## 3. Serving (`serve_axon` 4, `serve_axon_tls` 40, `serve_prometheus` 5)

`do_serve_axon` (`subnets/serving.rs:42-104`) and `validate_serve_axon` (:263-314; also run at pool admission by `CheckServingEndpoints`, `guards/check_serving_endpoints.rs:18-67`):
- signer hotkey must be registered on `netuid` → `HotKeyNotRegisteredInNetwork`; subnet must exist.
- `ip_type ∈ {4, 6}` → `InvalidIpType`; `ip: u128` is the integer form of the address (IPv4 as `u32` value in a u128, IPv6 as the 128-bit value); for axons `ip == 0` is **allowed** (`allow_zero = true`, :282) but `0x7f000001` (127.0.0.1), `>= u32::MAX` for v4, `u128::MAX` and `::1` for v6 are rejected → `InvalidIpAddress` (:222-243); prometheus disallows 0.
- `port != 0` → `InvalidPort` (:245-251).
- Rate limit: `ServingRateLimit[netuid]` (default 50 blocks; 0 disables): `last_serve == 0 || now - last >= limit` using the stored `AxonInfo.block` → `ServingRateLimitExceeded` (:165-183). Prometheus has its own independent timestamp.
- `protocol`, `placeholder1`, `placeholder2`, `version` are opaque u8/u32 fields stored verbatim in `AxonInfo { block:u64, version:u32, ip:u128, port:u16, ip_type:u8, protocol:u8, placeholder1:u8, placeholder2:u8 }` (lib.rs:308-324). Doc comment says `protocol`: TCP 0 / UDP 1 (dispatches.rs:636) — convention only.
- `certificate: Vec<u8>`: byte 0 = algorithm, bytes 1.. = public key ≤ 64 bytes → stored as `NeuronCertificate { public_key: BoundedVec<u8,64>, algorithm: u8 }` in `NeuronCertificates[netuid][hotkey]`; a cert longer than 65 bytes is **silently ignored** (`TryFrom` failure, serving.rs:69-73; lib.rs:328-351).
- Events `AxonServed(NetUid, AccountId)` / `PrometheusServed`. The SDK's `reset_axon` intent is `serve_axon` with ip 0 (`D/intents.json`).
- In v11 the "axon" is just this endpoint record; transport is btauth/1 signed HTTP (out of scope here).

---

## 4. Registration

### 4.1 `burned_register` (7) / `register_limit` (134) / legacy `register` (6)

`do_register_with_post_info` (`registration.rs:203-347`): coldkey signs and pays; checks in order: non-root (`RegistrationNotPermittedOnRootSubnet`), subnet exists, `NetworkRegistrationAllowed[netuid]` (`SubNetRegistrationDisabled`), hotkey not already registered, price = `Burn[netuid]`; split by `CollateralLockShare[netuid]` (u16/65535, default 0): burned share recycled, collateral share staked+locked on the hotkey (crediting standing collateral of a returning hotkey, `get_collateral_topup_tao`); `keep_alive_balance >= total` (`NotEnoughBalanceToStake`); creates/validates the hotkey↔coldkey pairing (`NonAssociatedColdKey`); capacity: if `SubnetworkN >= MaxAllowedUids` a prune victim must exist (`NoNeuronIdAvailable`). No per-block cap on the burn path. On success: `bump_registration_price_after_registration` (price × `BurnIncreaseMult`, default 1.26, clamped to [`MinBurn`, `MaxBurn`]) (:790-800), `RegistrationsThisBlock += 1`, `NeuronRegistered(netuid, uid, hotkey)`. Price decays every block by a factor with half-life `BurnHalfLife[netuid]` (default 360 blocks; `MaxBurnHalfLife` 36 100) toward `MinBurn` (:743-788). Fee is `Pays::Yes` plus the burn; the SDK's `--dry-run` cannot know the final price (it moves per block).

Pruning (`get_neuron_to_prune`, :513-599): skip owner-immortal hotkeys (`ImmuneOwnerUidsLimit`, default 1, max 10, lib.rs:2810-2829 — owner hotkey always first); among the rest pick lowest `Emission`, tie → older `BlockAtRegistration`, tie → lower uid; prefer non-immune (`now - BlockAtRegistration < ImmunityPeriod`) if more than `MinNonImmuneUids` (default 10) non-immune exist, else prune the lowest **immune** one. `MaxAllowedUids` default 256 (new subnets explicitly 256), floor `MinAllowedUids` 64; new subnets get `ImmunityPeriod` 5000 (not the constant 4096), `MaxAllowedValidators` 64 (not 128), `MinAllowedWeights` 1 (not 1024) — `init_new_network`, `subnets/subnet.rs:506-520`.

### 4.2 `pow_register` (152) — Pays::No, owner-enabled

Gate `NetworkPowRegistrationAllowed[netuid]` (owner-settable, admin-utils:836-872; at least one of burn/PoW must stay enabled). Seal (`registration.rs:28-42`): `H256(keccak256(sha256(b"subtensor-pow-register-v1" ‖ netuid.to_le_bytes() (u16) ‖ block_hash(work_block) (32) ‖ hotkey.encode() (32) ‖ coldkey.encode() (32) ‖ nonce.to_le_bytes() (u64))))`. Difficulty test: `U256::from_little_endian(hash).overflowing_mul(difficulty)` must **not** overflow (:632-642), `difficulty = clamp(Difficulty[netuid], max(MinDifficulty,1), MaxDifficulty)` (:838-842). Checks (`check_pow_registration`, :47-121, also run in the tx extension with longevity `5 - (now - work_block) + 1` and provides-tag `("pow-registration", hotkey, work_block)`, `extensions/subtensor.rs:98-107,335-348`): non-root; subnet exists; PoW allowed; hotkey not registered; `MaxAllowedUids != 0`; `RegistrationsThisBlock < MaxRegistrationsPerBlock` (default 1); `work_block < now && now - work_block <= 5` (`POW_MAX_WORK_AGE_BLOCKS`, :15); `work_block > LastPowRegistrationBlock[hotkey]`; block hash non-zero; seal matches; difficulty met; hotkey not a system account; hotkey unowned or owned by signer; ownership index growth bounded (`MAX_STAKING_HOTKEYS` 256). On success PoW difficulty is bumped by `BurnIncreaseMult` (:802-813) — independent of the burn price. Difficulty decays with the same half-life. No burn, no fee, no collateral.

### 4.3 Collateral (`add_collateral` 144, `set_min_collateral` 145)

`do_add_collateral` (`subnets/collateral.rs:559-660`): non-root, subnet exists, `SubtokenEnabled`, hotkey exists and is owned by signer, `alpha > 0`; covers from free stake first (`stake - locked`), buys the shortfall with TAO at the moving price, requiring `tao_to_buy <= get_max_amount_add(netuid, limit_price)` (fill-or-kill, `SlippageTooHigh`) and keep-alive balance; emits `CollateralLocked{netuid, hotkey, locked, total_locked}`. Collateral state `MinerCollateral[(netuid, hotkey, coldkey)] = { locked, drain_ratio, min_locked, earned }`; drains by `CollateralDrainRatio` (default 1.0, max 10) α per α of emission; floor via `set_min_collateral` (0 clears). Owner knobs: `collateral_lock_share` ≤ 62 258 (≈95%, `MaxCollateralLockShare`, lib.rs:696-709), `collateral_drain_ratio` (0, 10]. Unstake is blocked below locked (`StakeUnavailable`); `swap_hotkey_v2(keep_stake=true)` blocked while bonded (`KeepStakeBlockedByCollateral`).

### 4.4 `root_register` (62)

`do_root_register` (`coinbase/root.rs`): root exists; `RegistrationsThisBlock < MaxRegistrationsPerBlock[0]`; `RegistrationsThisInterval < target×3` style interval cap (reset each root tempo, registration.rs:775-786); not already registered; owns hotkey; if full (64 seats per docs) prune lowest-staked non-immune via `get_root_neuron_to_prune` (registration.rs:601-630), requiring the registrant's root stake ≥ victim's (`StakeTooLowForRoot`); burn `Burn[ROOT]` recycled; hotkey becomes a delegate at 18%; auto-childkeyed to every subnet owner unless opted out (`set_auto_parent_delegation_enabled`). Root weights (`set_root_weights`) are **gone** (errors `RootWeightSettingDisabled`/`RootWeightCapExceeded` retained for index stability, `macros/errors.rs`).

---

## 5. Subnet-owner surface

### 5.1 Creation / activation / identity

- `register_network` / `_with_identity` (`subnets/subnet.rs:152-283`): coldkey signs; hotkey must be unowned or owned by signer and not a pallet sub-account; `now >= NetworkRegistrationStartBlock`; global rate limit `NetworkRateLimit` = 7200 blocks keyed on the **last lock block** (one new subnet per day network-wide; also enforced at pool time, `check_rate_limits.rs:98-103`); lock cost `get_network_lock_cost()` (doubles per registration, decays over `NetworkLockReductionInterval` 14×7200 blocks, floor `NetworkMinLockCost` 1000 TAO — `S/docs/guides/subnets.mdx`), entire lock becomes the pool's TAO reserve (`set_new_network_state`, :289-453); if `TotalNetworks + cleanup_queue >= SubnetLimit` (128) the lowest-EMA-price non-immune subnet is dissolved (`get_network_to_prune`, `coinbase/root.rs:344-375`; immunity `NetworkImmunityPeriod` 1 296 000 blocks) and the registration is **queued** (`NetworkRegistrationQueued` event) — a client must wait for `NetworkAdded(netuid, mechid)` to learn the netuid (`S/docs/migration.mdx:596-602`). New subnet defaults: `SubnetEmissionEnabled = true` set in `init_new_network` **then overwritten to false** at :437-438 (root must enable emissions), `MechanismCountCurrent` 1, `Burn` = `DefaultNeuronBurnCost` 1 TAO (lib.rs:688-694), owner hotkey = the given hotkey (`SubnetOwnerHotkey`).
- `start_call` (92): owner-only, once; after `StartCallDelay` (0 today, root-settable) sets `FirstEmissionBlockNumber = now+1` and `SubtokenEnabled = true` (`subnet.rs:598-621`). Until then staking and alpha trading are disabled (`SubtokenDisabled`).
- `sudo_set_sn_owner_hotkey` (AdminUtils 67): owner-or-root; rate `DefaultSetSNOwnerHotkeyRateLimit` 50 400 blocks per subnet; retargets auto-parent edges (`subnet.rs:624-661`, `utils/misc.rs:945-957`).
- `set_subnet_identity` (78), `update_symbol` (112): §1.1.
- Owner cut: `SubnetOwnerCut` global 18% (11796/65535, root-set); `sudo_set_owner_cut_enabled`/`_auto_lock_enabled` (92/95) owner-or-root, no rate limit, non-root subnets (admin-utils:2509-2560).
- `trigger_epoch` (141): §1.1.
- There is **no owner dissolve** (§0.9).

### 5.2 Owner authorization mechanics

`ensure_sn_owner_or_root_with_limits(origin, netuid, &[TransactionType])` (`utils/misc.rs:38-54`): root bypasses rate limits; owner must pass `passes_rate_limit_on_subnet` for each key. `OwnerHyperparamUpdate(Hyperparameter)` limit = `Tempo[netuid] × OwnerHyperparamRateLimit` (default **2** tempos, lib.rs:1442-1444), tracked per `(netuid, hyperparam)` in `LastRateLimitedBlock` (`rate_limiting.rs:36-48,91-104`); `SetWeightsVersionKey` limit = `Tempo × WeightsVersionKeyRateLimit` (5 tempos, lib.rs:1044-1046); `MechanismCountUpdate`/`MechanismEmission` 7200 blocks; `MaxUidsTrimming` 30×7200; `TempoUpdate` 360 blocks; `SetSNOwnerHotkey` 50 400. Most per-subnet setters then call `ensure_admin_window_open(netuid)` **for owner and root alike**: rejected when `blocks_until_next_auto_epoch < AdminFreezeWindow` (default 10) or a manual trigger is pending (`misc.rs:60-82`) → `AdminActionProhibitedDuringWeightsWindow`. `do_set_tempo` / `do_set_activity_cutoff_factor` apply window+rate only to owners (`tempo_control.rs:16-85`). First call on a fresh key always passes (`last_block == 0`).

### 5.3 Hyperparameter matrix (AdminUtils idx → origin, type/unit, bounds)

Owner-settable (`ensure_sn_owner_or_root_with_limits`, rate-limited per key, admin window):

| idx | call | type / unit | bounds / notes | source line |
|---|---|---|---|---|
| 3 | `sudo_set_serving_rate_limit` | u64 blocks | 0 disables | admin-utils:341 |
| 4 | `sudo_set_min_difficulty` | u64 | non-root | :367 (docs say root-only — §0.10) |
| 5 | `sudo_set_max_difficulty` | u64 | — | :409 |
| 24 | `sudo_set_difficulty` | u64 | non-root | :979 |
| 6 | `sudo_set_weights_version_key` | u64 | rate 5 tempos | :442 |
| 9 | `sudo_set_adjustment_alpha` | u64 (u64::MAX=1.0) | inert controller | :526 |
| 13 | `sudo_set_immunity_period` | u16 blocks | — | :557 |
| 14 | `sudo_set_min_allowed_weights` | u16 | — | :590 |
| 15 | `sudo_set_max_allowed_uids` | u16 | ≥ MinAllowedUids(64), ≥ SubnetworkN, ≤ 256/2500 budget, ×mech ≤ budget | :623-676 |
| 17 | `sudo_set_rho` | u16 | — | :693 |
| 18 | `sudo_set_activity_cutoff` | u16 blocks | ≥ `MinActivityCutoff`; legacy | :723 |
| 97 | `sudo_set_activity_cutoff_factor` | u32 per-mille of tempo | 1000..=50000 | :766; tempo_control.rs:49-85 |
| 19 | `sudo_set_network_registration_allowed` | bool | non-root: owner; must keep burn or PoW on | :787-835 |
| 20 | `sudo_set_network_pow_registration_allowed` | bool | non-root; must keep one route on | :836 |
| 22 | `sudo_set_min_burn` | TaoBalance rao | < 1 TAO (`MinBurnUpperBound`), < max_burn | :901-939 |
| 23 | `sudo_set_max_burn` | TaoBalance rao | > `MaxBurnLowerBound` 0.1 TAO, > min_burn | :940 |
| 26 | `sudo_set_bonds_moving_average` | u64 (1e6=1.0) | ≤ 975 000 | :1056 |
| 60 | `sudo_set_bonds_penalty` | u16 | — | :1095 |
| 30 | `sudo_set_tempo` | u16 blocks | owner: 360..=50 400, rate 360 blocks; root: any; resets `LastEpochBlock` | :1185; tempo_control.rs:16-47 |
| 93 | `sudo_set_min_childkey_take_per_subnet` | PerU16 | — | :1362 |
| 49 | `sudo_set_commit_reveal_weights_enabled` | bool | — | :1401 |
| 50 | `sudo_set_liquid_alpha_enabled` | bool | — | :1439 |
| 51 | `sudo_set_alpha_values` | u16,u16 (65535=1.0) | liquid alpha must be on; low ≥ 1639, low ≤ high | :1463; run_epoch.rs:1815-1841 |
| 104 | `sudo_set_liquid_alpha_consensus_mode` | `ConsensusMode` | — | :1491 |
| 111 | `sudo_set_epoch_consensus` | `EpochConsensus` | no pending timelock commits; population must fit the target budget | :1514; mechanism.rs:129-186 |
| 57 | `sudo_set_commit_reveal_weights_interval` | u64 epochs | 1..=100 | :1591 |
| 61 | `sudo_set_toggle_transfer` | bool | — | :1683 |
| 80 | `sudo_set_recycle_or_burn` | enum | — | :1716 |
| 68 | `sudo_set_alpha_sigmoid_steepness` | i16 | owner non-negative | :1845 |
| 69 | `sudo_set_yuma3_enabled` | bool | — | :1890 |
| 70 | `sudo_set_bonds_reset_enabled` | bool | — | :1924 |
| 72 | `sudo_set_owner_immune_neuron_limit` | u16 | 1..=10 | :2031 |
| 76 | `sudo_set_mechanism_count` | MechId | 1..=MaxMechanismCount(2); rate 7200 | :2091 |
| 77 | `sudo_set_mechanism_emission_split` | Option<Vec<u16>> | len==count, Σ==65535; rate 7200 | :2117 |
| 78 | `sudo_trim_to_max_allowed_uids` | u16 | rate 30×7200; immune ≤ 80% | :2146 |
| 112 | `sudo_trim_null_uids_batch` | u16 target | ≤ 64 deletions/batch | :2177 |
| 89 | `sudo_set_burn_half_life` | u16 blocks | 1..=36 100, non-root | :2423 |
| 90 | `sudo_set_burn_increase_mult` | U64F64 | non-root | :2465 |
| 98 | `sudo_set_collateral_lock_share` | u16/65535 | ≤ 62 258 | :2589 |
| 99 | `sudo_set_collateral_drain_ratio` | U64F64 | > 0, ≤ 10 | :2632 |
| 92 / 95 | `sudo_set_owner_cut_enabled` / `_auto_lock_enabled` | bool | owner-or-root, **no** rate limit/window | :2509 / :2533 |
| 67 | `sudo_set_sn_owner_hotkey` | AccountId | 50 400-block rate | :1979 |

Root-only (`ensure_root`): 0 swap_authorities, 1 default_take, 2 tx_rate_limit, 7 **weights_set_rate_limit**, 8 adjustment_interval, 16 **kappa**, 21 target_registrations_per_interval, 25 **max_allowed_validators** (≤ max uids), 27 max_registrations_per_block, 28 subnet_owner_cut, 29 network_rate_limit, 33 total_issuance (deprecated), 35 network_immunity_period, 36 network_min_lock_cost, 37 subnet_limit, 38 lock_reduction_interval, 39 rao_recycled, 42 **stake_threshold**, 43 nominator_min_required_stake, 45 tx_delegate_take_rate_limit, 46 min_delegate_take, 55 dissolve_network_schedule_duration, 58 evm_chain_id, 59 schedule_grandpa_change, 62 toggle_evm_precompile, 63 subnet_moving_alpha, 65 ema_price_halving_period, 66 **subtoken_enabled**, 71 **commit_reveal_version**, 73 ck_burn, 74 admin_freeze_window, 75 owner_hparam_rate_limit, 79 min_allowed_uids, 81-83/91 tao-flow params, 84 min_non_immune_uids, 85 start_call_delay, 86/87 coldkey-swap delays, 88 max_mechanism_count, 94 **subnet_emission_enabled**, 96 max_epochs_per_block, 100-102 emission gate, 105-110/113 basket knobs. `max_weights_limit` has **no setter** at all.

The SDK's `set_hyperparameter` intent maps exactly 33 names to these calls (`D/intents.json` entry `set_hyperparameter.wraps`); human-unit conversion rules live in `S/sdk/python/bittensor/hyperparams.py:109-410` (kind per name: `blocks`, `u16` fraction, `rao`, `difficulty`, `U64F64`, bool).

---

## 6. Stake / children / take / identity / commitments — client summary

- Stake amounts: TAO in rao (`TaoBalance(u64)`), alpha in `AlphaBalance(u64)`; `u64::MAX` alpha = whole live position for move/transfer/swap (spec 469+). Minimum stake 0.002 TAO (`InitialMinStake`), minimum transfer 0.0001 TAO; default 5% slippage bound is an SDK policy, not chain. Fees for unstake-side calls can be taken in **alpha** (`SubtensorTxFeeHandler::fees_in_alpha`, `S/pallets/transaction-fee/src/lib.rs:340-420`): remove_stake*, unstake_all*, move_stake*, transfer_stake*, swap_stake*, swap_hotkey*. Do not submit the exact full balance.
- Take: `PerU16` parts of 65535; default 18% = 11 796; increase is rate-limited (216 000 blocks) and capped at `MaxDelegateTake`; both `Pays::Yes`.
- Children: ≤ 5, `(u64 proportion, AccountId)`, applied after `PendingChildKeyCooldown`; rate 150 blocks.
- Identity: `set_identity` (hotkey must be registered somewhere; stored in `IdentitiesV2`), `set_subnet_identity` (owner).
- Commitments: `set_commitment` hotkey-signed, Pays::No, 3 fields, 3100-byte/epoch budget, timelock fields auto-revealed into `RevealedCommitments` (terminal failures become `TimelockRevealFailed`, `S/docs/guides/timelock.mdx`).
- Events a client decodes (`macros/events.rs`): `WeightsSet(NetUidStorageIndex,u16)` :44, `NeuronRegistered(NetUid,u16,AccountId)` :49, `AxonServed(NetUid,AccountId)` :95, `WeightsCommitted(AccountId,NetUidStorageIndex,H256)` :263, `BatchWeightItemFailed(NetUid,DispatchError)` :292, `FirstEmissionBlockNumberSet(NetUid,u64)` :328, `TimelockedWeightsCommitted(AccountId,NetUidStorageIndex,H256,u64)` :422, `TimelockedWeightsRevealed(NetUidStorageIndex,AccountId)` :428, `EpochTriggered{netuid,by,fires_at}` :682, `NetworkRegistrationQueued{coldkey,hotkey,mechid,…}` :730, `CollateralLocked{netuid,hotkey,locked,total_locked}` :777, `NetworkAdded(NetUid,u16)` :13.
- Runtime APIs a client uses: `SubnetInfoRuntimeApi.{get_subnet_hyperparams_v3, get_metagraph, get_mechagraph, get_selective_metagraph, get_next_epoch_start_block, get_subnet_to_prune}`, `SubnetRegistrationRuntimeApi.get_network_registration_cost`, `NeuronInfoRuntimeApi.{get_neuron(s)(_lite)}`, `ProxyFilterRuntimeApi.{get_proxy_filters,get_proxy_types}` (`M/explore/api_*.txt`).

---

## 7. Transaction extensions, fees, priority, nonce — what a client must encode

Extrinsic format: `fp_self_contained::UncheckedExtrinsic<MultiAddress<AccountId32,()>, RuntimeCall, MultiSignature, TxExtension>` (`S/runtime/src/lib.rs:1732-1733`); metadata lists extrinsic versions `[4, 5]` (`M/finney.json` `.extrinsic.versions`) — v4 signed extrinsics work; v5 general transactions are also accepted. `TxExtension` tuple order (`S/runtime/src/lib.rs:1644-1664`; identical order in 477 metadata `transaction_extensions_by_version["0"] = [0..12]`):

| # | identifier (477 metadata) | explicit (signed-payload bytes in the extrinsic) | implicit (appended to the signing payload only) | notes |
|---|---|---|---|---|
| 0 | `CheckNonZeroSender` | `()` | `()` | custom: also rejects small-order Ed25519 signer keys (`check_nonzero_sender.rs:85-89`) |
| 1 | `CheckSpecVersion` | `()` | `u32` spec (477 live) | |
| 2 | `CheckTxVersion` | `()` | `u32` = 1 | `transaction_version: 1` (`lib.rs:244`) |
| 3 | `CheckGenesis` | `()` | `H256` genesis hash | |
| 4 | `CheckMortality` | `Era` (1 or 2 bytes) | `H256` (genesis for Immortal, else the era's checkpoint block hash) | custom: `MevShield.submit_encrypted` requires `Mortal(period ≤ 8)` (`check_mortality.rs:19,94-105`) |
| 5 | `CheckNonce` | `Compact<u32>` | `()` | custom semantics §0.4; `provides=(who,nonce)`, `requires=(who,nonce-1)` if gap (`check_nonce.rs:149-166`) |
| 6 | `CheckWeight` | `()` | `()` | |
| 7 | `ChargeTransactionPayment` (477) / `ChargeTransactionPaymentWrapper` (478) | `Compact<u64>` tip | `()` | wire-compatible rename (§0.2) |
| 8 | `SudoTransactionExtension` | `()` | `()` | rejects `Sudo.*` calls not signed by the sudo key at pool time (`sudo_wrapper.rs:51-80`) |
| 9 | `CheckShieldedTxValidity` | `()` | `()` | parses `submit_encrypted` ciphertext (`shield/extension.rs:50-78`) |
| 10 | `SubtensorTransactionExtension` | `()` | `()` | runs the guards §7.2 for signed origins |
| 11 | `DrandPriority` | `()` | `()` | priority +10 000 for `write_pulse` only |
| 12 | `CheckMetadataHash` | `mode: u8` (0 Disabled, 1 Enabled) | `Option<[u8;32]>` (None when disabled) | type 414/415 in metadata |

Signing payload = `call ‖ explicit(0..12) ‖ implicit(0..12)`, blake2-256-hashed if > 256 bytes (standard `generic::SignedPayload`, `lib.rs:1740`). Signature: `MultiSignature` (sr25519/ed25519/ecdsa); wallets are sr25519. SS58 prefix **42** (`lib.rs:273`; metadata `System.SS58Prefix = 42`).

### 7.1 Fee model
`fee = weight_fee + length_fee + tip`; `weight_fee = 0.00025 rao × ref_time(ps)` (`WEIGHT_FEE_PER_REF_TIME = Perbill(250_000)`), `length_fee = 0.5 rao/byte` (`LENGTH_FEE_PER_BYTE = Perbill(500_000_000)`) — both halved in spec 467 (`S/pallets/transaction-fee/src/lib.rs:49-55`); `ConstFeeMultiplier = 1`, `OperationalFeeMultiplier = 5` (`lib.rs:532-545`); `Pays::No` calls pay nothing (and tips are ignored for priority). Existential deposit 500 rao (`lib.rs:477-479`). `deposit(items, bytes) = items×0.02 TAO + bytes×0.001 TAO` (`lib.rs:198-205`). Fee estimation: `TransactionPaymentApi.query_info` / `query_fee_details` (standard). 478 adds: fee payer redirection (proxy `RealPaysFee`, coldkey-for-hotkey on the allow-list) with the tip dropped when redirected, and `FeeWeightDiscount` subsidies for staking-walk envelopes (`staking_fee.rs:24-77`).

### 7.2 Pool-time guards (signed origin only)
`SubtensorTransactionExtension::check` (`extensions/subtensor.rs:81-129`): `CheckColdkeySwap` (if the signer has an announced swap, only swap-related calls and `submit_encrypted` pass; disputed → `ColdkeySwapDisputed` code 21; else `ColdkeyInSwapSchedule` code 0 — `guards/check_coldkey_swap.rs:17-44`), basket checks, `pow_register` proof, `set_commitment` registration check, `CheckWeights`, `CheckRateLimits` (weights + `register_network`), `CheckDelegateTake` (increase/decrease_take bounds + rate), `CheckServingEndpoints`, `CheckEvmKeyAssociation`. `provides` tags: `("pow-registration", hotkey, work_block)`, `("weight-commit", who, netuid_index)`. Longevity for pow_register = remaining work age + 1 block.

### 7.3 Priority / ordering
Flat (§0.5). Normal-class block budget = 75% of 4 s ref-time (`lib.rs:255-270`), block length 10 MiB. Practical consequence: during congestion (e.g. tempo boundaries) inclusion is first-come; rtao should submit commits early in the tempo rather than tipping.

### 7.4 Nonce rules
Standard incrementing per `AccountId`; `AccountNonceApi.account_nonce` for the next nonce; a pending tx with nonce n+1 is held (`requires`) until n lands. A hotkey that never held TAO gets `sufficients=1` on its first successful signed extrinsic (`check_nonce.rs:178-187`), so its nonce is never reset — do not assume a fresh hotkey's nonce is 0 after it has signed anything. Nonce type `u32` (`S/common/src/lib.rs:50`).

### 7.5 Oddities checklist for rtao
- Era for shielded txs ≤ 8 blocks; everything else may be immortal, but prefer mortal (64) for pool hygiene.
- `CheckMetadataHash` must be present even if `Disabled` (explicit `0x00`, implicit `0x00`).
- Pool errors arrive as `InvalidTransaction::Custom(u8)` with the `CustomTransactionError` code table (`S/common/src/transaction_error.rs:38-70`): 1 StakeAmountTooLow, 2 BalanceTooLow, 3 SubnetNotExists, 4 HotkeyAccountDoesntExist, 5 NotEnoughStakeToWithdraw, 6 RateLimitExceeded, 7 InsufficientLiquidity, 8 SlippageTooHigh, 9 TransferDisallowed, 10 HotKeyNotRegisteredInNetwork, 11 InvalidIpAddress, 12 ServingRateLimitExceeded, 13 InvalidPort, 14 ZeroMaxAmount, 15 InvalidRevealRound, 16 CommitNotFound, 17 CommitBlockNotInRevealRange, 18 InputLengthsUnequal, 19 UidNotFound, 20 EvmKeyAssociateRateLimitExceeded, 21 ColdkeySwapDisputed, 22 InvalidRealAccount, 23 FailedShieldedTxParsing, 24 InvalidShieldedTxPubKeyHash, 25 NonAssociatedColdKey, 26 DelegateTakeTooLow, 27 DelegateTakeTooHigh, 255 BadRequest, 0 ColdkeyInSwapSchedule (deprecated name, still emitted).
- `batch_set_weights` et al. return `Ok` even when items fail — inspect `BatchWeightItemFailed` events.
- `commit_crv3_mechanism_weights` (117) pins `commit_reveal_version = 4`; prefer 118 and read `CommitRevealWeightsVersion` (root can bump it to force client upgrades).
- Only one pending weight commit per (hotkey, mechanism) in the pool (provides tag).
- `register` (6) is not PoW any more; `pow_register` seal is new and Pays::No.
- `dissolve_network` is root-only; `set_tempo`/`set_activity_cutoff_factor` on SubtensorModule are fee-charging no-ops; `schedule_swap_coldkey`, Swap LP calls return `Deprecated`.
- `Compact` fields appear in `batch_set_weights`/`batch_commit_weights` (netuids, weights, version_keys), Crowdloan amounts/ids, `Multisig.max_weight` (Compact inside `Weight`), `kill_pure` height/ext_index, `CheckNonce`, the tip.

---

## 8. Constants a client needs (verified values)

| constant | value | source |
|---|---|---|
| Block time | 12 000 ms (fast-blocks builds 250 ms) | `S/common/src/lib.rs:183-187` |
| `spec_version` / `transaction_version` | 477 live (metadata), 478 main / 1 | `lib.rs:241-244`, `M/finney.json` |
| SS58 prefix | 42 | `lib.rs:273` |
| Existential deposit | 500 rao | `lib.rs:477-479` |
| `BlockHashCount` | 2400 | `lib.rs:263` |
| Block weight / length | 4 s ref-time, 75% normal; 10 MiB | `lib.rs:249-271` |
| Tempo default / owner bounds | 360 / 360..=50 400 | `lib.rs:904`, `pallets/subtensor/src/lib.rs:2325-2327` |
| `MaxEpochsPerBlock` | 2 (prod) | `lib.rs:985` |
| Max UIDs default / min / Null budget | 256 / 64 / 2500 | `lib.rs:916,961`; `mechanism.rs:35` |
| Max validators (constant / new subnet) | 128 / 64 | `lib.rs:922`; `subnet.rs:508` |
| Immunity period (constant / new subnet) | 4096 / 5000 blocks | `lib.rs:928`; `subnet.rs:510` |
| Min allowed weights (constant / new subnet) | 1024 / 1 | `lib.rs:918`; `subnet.rs:509` |
| Weights set rate limit | 100 blocks | `pallets/subtensor/src/lib.rs:1088-1090` |
| Serving rate limit | 50 blocks | `lib.rs:943` |
| Commit-reveal: enabled default / version / reveal period / bounds / max unrevealed | true / 4 / 1 epoch / 1..=100 / 10 | `pallets/subtensor/src/lib.rs:1308-1316,1219-1221,287-289`; weights.rs:200 |
| CRv3 ciphertext caps | 5000 B (Yuma), 32 KiB (bound), 64 KiB/64 entries Null queue | `pallets/subtensor/src/lib.rs:63-68` |
| Burn: initial / min / max / new-subnet / half-life / mult | 0.1 TAO / 500 000 rao / 100 TAO / 1 TAO / 360 blocks / 1.26 | `lib.rs:944-948`; `pallets/subtensor/src/lib.rs:676-694` |
| `MinBurnUpperBound` / `MaxBurnLowerBound` | 1 TAO / 0.1 TAO | `lib.rs:949-950` |
| PoW difficulty initial / min / max; work age | 10 000 000 / 10 000 000 / u64::MAX/4; ≤ 5 blocks | `lib.rs:924,941-942`; registration.rs:15 |
| Max registrations per block | 1 | `lib.rs:930` |
| Network (subnet) registration rate limit / min lock / lock reduction / immunity / subnet limit | 7200 blocks / 1000 TAO / 100 800 blocks / 1 296 000 blocks / 128 | `lib.rs:963-967`; `pallets/subtensor/src/lib.rs:1453-1455` |
| Start-call delay | 0 | `lib.rs:980` |
| Owner hparam rate limit / admin freeze window / SN-owner-hotkey rate | 2 tempos / 10 blocks / 50 400 blocks | `pallets/subtensor/src/lib.rs:1436-1444,1424-1426` |
| Mechanisms: default / max / hard cap / storage stride | 1 / 2 / 16 / 4096 | `pallets/subtensor/src/lib.rs:3552-3561`; `mechanism.rs:30-33` |
| Takes: default / max childkey / min / rate limits | 18% (11 796) / 18% / 0 / 216 000 blocks | `lib.rs:935-939,958-959` |
| Children: max / rate / cooldown | 5 / 150 blocks / `PendingChildKeyCooldown` storage | `set_children.rs:41`; `rate_limiting.rs:26` |
| Stake threshold / min stake / min transfer | 0 (runtime; mainnet non-zero) / 0.002 TAO / 0.0001 TAO | `pallets/subtensor/src/lib.rs:1213-1216`; `lib.rs:946-947` |
| Key swap cost / on-subnet cost / interval | 0.1 TAO / 0.001 TAO / 7200 blocks | `lib.rs:968,981-982` |
| Coldkey swap announcement / reannouncement / dissolve schedule | 36 000 / 7200 / 36 000 blocks | `lib.rs:973-975` |
| EVM key association rate limit / max uids per key | 7200 blocks / 32 | `lib.rs:909`; `pallets/subtensor/src/lib.rs:210` |
| Collateral: lock-share max / drain max / default drain | 62 258 / 10 / 1.0 | `pallets/subtensor/src/lib.rs:696-720` |
| Commitments: max fields / space per epoch / timelock / BigRaw / Raw | 3 / 3100 B / 1024 / 512 / 128 | `lib.rs:807`; `commitments/src/lib.rs:219`; `types.rs:421-422` |
| Proxy: deposits / max / pending | 0.06 + 0.033 TAO / 20 / 75 | metadata; `lib.rs:578-586` |
| Multisig: deposits / max signatories | 0.132 + 0.032 TAO / 100 | metadata; `lib.rs:558-563` |
| Utility batch limit | 10 922 | metadata |
| Drand quicknet | period 3 s, genesis 1 692 803 367, hash `52db9b…`, kept pulses 216 000 | `drand/src/lib.rs:91-97,199-200` |
| Fees | 0.00025 rao/ps ref-time; 0.5 rao/byte; multiplier 1; operational ×5 | `transaction-fee/src/lib.rs:49-55`; `lib.rs:532-545` |
| Priority | Normal 1 / Operational 1e10 / drand +1e4 | `lib.rs:1183-1184`; `drand_priority.rs:35` |
| Small-transfer proxy limits | 0.5 TAO / 0.5 α | `S/common/src/lib.rs:50-51` |

---

## 9. Churn / recency (what changed when, from on-disk evidence)

- git history is unavailable (shallow clone); the docs anchor these: v437 key-lineage maps & hotkey swap semantics; v441 "Root Reborn" (root_register burn-based, claim types retired → `claim_root(subnets)` kept for SCALE compatibility, indices 122/123 retired, `set_root_weights` removed); v445/v446 EVM registry, `LiquidAlphaConsensusMode`, terminal timelock failures (`TimelockRevealFailed`, Data tag 137), timelock envelope; v448 `move_stake_limit` (149); v450 standardized pricing; v461 basket trading (`swap_basket` 150, `swap_basket_many` 151, `stake_into_basket` 147, `claim_root_with_hotkey` 148, `BasketTrading` proxy type 18); 463 testnet concentration cap; 467 fee halving; 468 basket trading enabled on mainnet; 469 fee envelopes and `u64::MAX` = full position (`S/docs/migration.mdx:511-645`; `D/bittensor.com_llms-full.txt` lines 568-7961 cited above).
- Highest SubtensorModule call indices: 147-152 (basket/claim/move_stake_limit/pow_register) are the newest; 139/140 are deprecated no-op shims; 122/123 retired. AdminUtils newest: 104 (liquid alpha mode), 105-110/113 (basket), 111 `sudo_set_epoch_consensus`, 112 `sudo_trim_null_uids_batch`, 97-99 (activity factor, collateral). Proxy 11 `set_real_pays_fee` and the deny-by-default filters are v11-era.
- Release cadence: every merge to `main` with a `spec_version` bump ships (`S/docs/internals/release-process.mdx:32-40`); the 477→478 delta (fee-extension rename, wrapper fee logic) illustrates per-release client-visible churn of the extension set even when calls are unchanged. Metadata is V16 with typed newtypes (`TaoBalance`, `NetUid`…), so metadata-driven codegen (subxt) stays correct across renames while hand-written extension lists do not.
- Deprecated-but-encodable: `register` (6, now burn), `swap_hotkey` (70, "removed after June 2026"), `schedule_swap_coldkey` (73), `set_tempo`/`set_activity_cutoff_factor` (139/140), `claim_root` (121 argument ignored), `sudo_set_total_issuance` (33), `sudo_set_activity_cutoff` (18, legacy), Swap 1-5, proxy types 4/6/7/12.

---

## 10. Open questions / things I could not verify on disk

1. Live mainnet values of `StakeThreshold`, `TaoWeight`, `CommitRevealWeightsVersion`, `AdminFreezeWindow`, `MaxMechanismCount` — only genesis/runtime defaults are on disk (no storage snapshot).
2. Whether finney (477) already runs the proxy `RealPaysFee` fee redirection — the pallet storage exists at 477 but the wrapper extension that consults it is only in 478 source.
3. Exact rtao-relevant behaviour of extrinsic **v5** encoding on this chain (metadata advertises versions 4 and 5); v4 signed is the safe default.
4. `MaxEpochsPerBlock` deferral is not modelled by the SDK's reveal-round simulation; the +3 block offset is an empirical margin, not a proof.
5. `is_valid_identity` total-length rule (the `max_length` computation at `utils/identity.rs:174-182,212-221`) was not read line-by-line; per-field caps above are exact.
