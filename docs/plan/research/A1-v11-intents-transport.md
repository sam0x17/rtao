# A1 — v11 Python SDK transaction side: intents, plan/execute, results/errors, transport write path

Research report for the **rtao** architects. Everything here was read from the on-disk
RaoFoundation/subtensor monorepo at commit 850f275 (`sdk/python/bittensor` = unified
`bittensor` 11.3.x, `sdk/bittensor-core` = the Rust core) and the finney V16 metadata dump
fetched 2026-10-13 (spec 477). Paths below are relative to
`SP/upstream/subtensor/`
unless stated otherwise; `py:` abbreviates `sdk/python/bittensor/`, `core:` abbreviates
`sdk/bittensor-core/src/`, `meta:` abbreviates `…/scratchpad/meta/`, `docs:` abbreviates
`docs/`. Line references are `file:Lnn`.

Claims marked **[verified]** were read directly in source. Claims marked **[inferred]** are
deductions I did not execute code to confirm. Nothing in the upstream tree was run.

---

## 0. Executive summary (what a Rust port must know first)

1. **The write path is a strict pipeline with one choke point.** `Intent.build()` composes a
   `CallBytes` → `nest_origin_wrappers` adds `Sudo.sudo` then `Proxy.proxy` →
   `Intent.wrap_call` adds the saved-multisig layer → `Policy.check` → sign → submit → decode
   receipt → post-process (`py:executor.py:L175-219`, `L709-889`). Every mutation, including
   the raw-call escape hatch, goes through `Executor` (`py:executor.py:L1-7`).
2. **Chain-defined bytes already live in Rust** (`sdk/bittensor-core`, `publish = false`):
   signature payload layout driven by metadata extension order (`core:codec/extrinsic.rs:L38-118`),
   v4 signed-extrinsic assembly (`L260-316`), era encoding/birth (`core:codec/encode.rs:L445-485`,
   `core:codec/extrinsic.rs:L407-412`), multisig account derivation (`L419-437`), ML-KEM-768 +
   XChaCha20-Poly1305 shield sealing (`core:mlkem/mod.rs:L53-107`), RFC-0078 digest
   (`core:digest/mod.rs:L102-107`), timelock commits. Byte-equal golden vectors exist
   (`sdk/python/tests/fixtures/golden.json`, spec 461 localnet).
3. **A native Rust `Client` + `Executor` already exists in bittensor-core** behind the `host`
   feature (`core:client.rs`, `core:transaction.rs`), but it is blocking `reqwest` over
   HTTP JSON-RPC, polls blocks, has ~20 hand-written `IntentCall` constructors, no nonce cache,
   and signs MEV-shield carriers with a 64-block era that the runtime's `CheckMortality`
   rejects for `submit_encrypted` (`core:client.rs:L32,L640-646` vs
   `runtime/src/check_mortality.rs:L19,L94-100`) — **[inferred bug]**, its e2e test only asserts a
   non-empty message (`sdk/bittensor-core/tests/e2e.rs:L313-339`).
4. **Product policy you may choose to keep or drop** (not chain-defined): 128-block default era,
   8-block shield era (chain-enforced max), 5% default slippage protection on stake trades,
   500 000 rao fee headroom on `all`, transient-rejection retry substrings, the 18-code
   `ErrorCode` taxonomy and its remediation prose, explorer URLs, "MEV shield by default" for
   stake-trading ops (CLI only — the SDK's `execute` does **not** auto-shield except for
   `mev_shield_required` intents, `py:executor.py:L812-824`).
5. **Runtime facts the Python layer mirrors by hand** (and a Rust port must keep in lockstep):
   `common/src/transaction_error.rs` custom codes 0–27 + 255 (`py:error_map.py:L488-518`),
   `runtime/src/fee_filters.rs` coldkey-pays-fee call list (`py:fee_filters.py:L12-32`),
   `MAX_SHIELD_ERA_PERIOD = 8` (`py:settings.py:L29`), the 8192-byte ciphertext bound
   (`py:executor.py:L61`), the root-claim fee model constants (`py:intents/_root_claim_fee.py:L26-49`),
   the owner-settable hyperparameter table (`py:intents/hyperparameters.py:L31-66`), the
   `ProxyType` variant list (`py:intents/proxy.py:L21-41`), and the full error-name →
   semantic-code classification (`py:error_map.py:L46-476`, CI-gated both directions by
   `sdk/python/codegen/check.py:L354-420`).

---

## 1. Architecture map

Layering of the write path (top → bottom), with sizes measured on disk:

| Layer | Module(s) | Lines | Role |
| --- | --- | --- | --- |
| Entry points | `py:_subtensor.py` (205), `py:client.py` (688), `py:sync.py` (479) | | `Subtensor` (sync+async facade), `Client` (async; owns one `Substrate` + one `Executor`), `SyncClient` (private loop thread) |
| Intent layer | `py:intents/*.py` (8 335 total; `base.py` 422, `plan.py` 135, `registry.py` 66, `_money.py` 165, `staking.py` 1 925, `weights.py` 563, `registration.py` 595, `multisig.py` 536, `_root_claim_fee.py` 771 …) | | 83 registered dataclasses; `Policy`, `Plan`; registry + JSON-schema manifest |
| Executor | `py:executor.py` (1 319) | | compose → preflight → fee → policy → sign → submit → resolve; shield; raw; offline signing |
| Signing seam | `py:signing.py` (283), `py:wallet.py`, `py:wallets.py`, `py:ledger.py`, `py:vault/`, `py:extension/` | | `Signer`/`KeyedWallet`/`WalletLike`, `resolve_signer`, `public_view` |
| Results/errors | `py:result.py` (750), `py:error_map.py` (795), `py:error_descriptions/` (24 files), `py:_generated/errors.py` (420, 405 catalog entries) | | `ExtrinsicResult`, `ChainError`, `ErrorCode`, classification |
| Chain-access contract | `py:_substrate.py` (851) | | `Substrate` Protocol + `RpcSubstrate` (the only code touching the transport) |
| Transport | `py:_transport/` (3 687 total): `rpc.py` 812, `interface.py` 810, `extrinsics.py` 426, `runtime.py` 310, `codec.py` 520, `contract.py` 245, `utils/receipt.py` 237, `protocols.py` 91, `errors.py` 80 | | websocket JSON-RPC, codec cache, signing/assembly, outcome decode |
| Rust core | `core:` (13 580 lines incl. tests): `codec/extrinsic.rs` 442, `codec/encode.rs` 765, `codec/decode.rs` 880, `runtime/mod.rs` 603, `mlkem/mod.rs` 169, `digest/mod.rs` 276, `timelock/` ~820, `keys/` ~900, `client.rs` 1 770, `transaction.rs` 1 274 | | chain-defined compute + a native blocking client/executor |
| Python↔Rust binding | `sdk/bittensor-core-py/src/*.rs` (1 865) | | PyO3 forwarding only (`lib.rs:L1-5`) |

Import discipline the SDK enforces **[verified]**: `_transport/codec.py` is the only module
that imports the SCALE engine (`py:_transport/codec.py:L1-12`); `_substrate.py:RpcSubstrate` is
"the only code in the SDK that talks to the transport directly" (`py:_substrate.py:L5-8`);
intents "never touch private keys" and obtain addresses only via `hotkey_address` /
`coldkey_address` / `hotkey_public_key` (`py:intents/base.py:L248-298`).

The transport's own layering statement (`py:_transport/__init__.py:L7-16`):
`interface.py` → `runtime.py` → `storage.py` → `extrinsics.py` → `runtime_api.py` → `codec.py` →
`rpc.py` (no SCALE knowledge in `rpc.py`).

---

## 2. The intent model

### 2.1 `Intent` base contract (`py:intents/base.py`)

| Member | Kind | Semantics | Ref |
| --- | --- | --- | --- |
| `op` | ClassVar[str] | stable machine name; registry key; docs URL `/docs/tx/<op-with-dashes>` | `L146`, `py:settings.py:L129-133` |
| `signer` | ClassVar `Literal["coldkey","hotkey"]` | which wallet key signs | `L26`, `L147` |
| `origin` | ClassVar `Literal["signed","subnet_owner","root"]` | dispatch privilege; `"root"` makes the executor wrap `Sudo.sudo` | `L34`, `L153`, `py:executor.py:L206-212` |
| `verify` | ClassVar[Optional[str]] | registered read that confirms the effect; mandatory for `origin == "root"` (CI gate) | `L157`, `check.py:L325-333` |
| `wraps` | ClassVar[tuple[(pallet, call), …]] | chain calls this intent can compose; drives the codegen coverage gate | `L160`, `check.py:L303-323` |
| `all_amount_fields` | ClassVar[tuple[str,…]] | money fields that accept the sentinel `"all"` | `L164` |
| `mev_shield_default` / `mev_shield_required` | ClassVar[bool] | CLI default / hard requirement to shield | `L170`, `L174` |
| `build(substrate, wallet)` | abstract async | returns composed `CallBytes` or `BuiltCall(call, extras)` | `L177-186`, `L414-422` |
| `summary()` | abstract | one-line description | `L189` |
| `effects/warnings/blocks(substrate, signer_address)` | async hooks | preview text; `blocks` merge into `Plan.violations` | `L192-202` |
| `preflight(substrate, dispatch_origin, fee_payer, *, call)` | async | context-aware preview returning `IntentPreflight`; default composes the three hooks against `dispatch_origin` | `L204-226` |
| `wrap_call(substrate, wallet, call)` | async | execution-adapter hook applied *after* sudo/proxy (used by saved multisigs) | `L228-235` |
| `origin_view(substrate, wallet)` | sync | account view the semantic build runs against (multisig adapter returns the multisig account) | `L237-246` |
| `spend()` | sync | `Balance` (TAO, exact) / `UNBOUNDED` / `None` — "TAO leaving the signer" only | `L300-309` |
| `touches_netuids()` | sync | default `[self.netuid]` if present | `L311-318` |
| `affects_all_subnets()` | sync | forces allowlist failure | `L320-322` |
| `semantic_intent()` | sync | the intent whose safety contract governs (adapters return the inner one) | `L324-330` |
| `to_dict()` / `from_args()` / `json_schema()` | serialization | money → exact decimal strings; unknown args rejected; schema from field annotations + `metadata["help"]` | `L357-399` |

`IntentPreflight` (`L109-134`) carries `effects`, `warnings`, `blocks`, optional
`required_free` / `available_free` / `estimated_fee` (exact fee from the fully composed call,
"conservative fallbacks stay out of this field"), plus `facts: list[(label, value)]` and
`facts_title` ("Fees" or "Quote") for the CLI review card.

Signer-shape rules **[verified]**: `hotkey_address(wallet, override)` returns the override, else
`wallet.hotkey.ss58_address`, else — when the intent's `signer == "hotkey"` and the wallet is a
bare `Signer` — the signer's own address; otherwise raises `BittensorError`
(`L258-274`). `coldkey_address` uses `coldkeypub` (never unlocks) with the symmetric bare-signer
fallback (`L288-298`).

### 2.2 Money, units and spend (`py:intents/_money.py`)

- `Money = int | float | str | Decimal | Balance` at the edge; normalized to a `Balance` at
  construction; floats never held internally (`L29-31`, module docstring `L1-18`).
- `"all"` sentinel only for opt-in fields (`L34`, `L51-55`).
- `UNBOUNDED` sentinel for `spend()` (`L37-44`); `Spend = Balance | _Unbounded | None` (`L48`).
- Unit enforcement: `call_amount(value, (pallet, call), param, netuid=…)` looks up the parameter's
  runtime type identity (`TaoBalance`/`AlphaBalance`) from the generated call builder's
  annotations and falls back to a hand table `_FALLBACK_AMOUNT_UNITS` for pre-newtype metadata
  (`L89-128`); a `Balance` tagged with the wrong netuid raises `ValueError` naming the call and
  parameter (`L152-163`). Negative amounts raise (`L68-69`).
- Note the explicit unit comments: `swap_basket.amount` is origin-subnet alpha (TAO 1:1 on
  netuid 0); `swap_basket.min_amount_out` is destination-subnet alpha (`L93-98`).

### 2.3 Registry, schema, manifest (`py:intents/registry.py`)

- `@register` keys `REGISTRY` by `op`, raising on duplicates, and **builds the JSON schema at
  import time** so an un-mappable annotation fails registration (`L21-31`).
- `build(op, args)` → `cls.from_args(args)` (`L34-40`).
- `list_tools()` emits `{name, summary, description, signer, origin, verify, wraps, docs_url,
  input_schema}` sorted by op (`L43-66`); `Client.tools()`/`Executor.tools()` expose it
  (`py:client.py:L581-583`, `py:executor.py:L1317-1319`). The on-disk catalog
  `…/scratchpad/docs/intents.json` has exactly **83** entries, matching the 83 `@register`
  decorators counted across `py:intents/*.py` (**[verified]** by count).
- Every intent class is re-exported at `bittensor.<ClassName>` from the registry
  (`py:__init__.py:L109-113`).

### 2.4 Execution adapters (unregistered intents)

`MultisigIntentAdapter` / `MultisigThreshold1IntentAdapter` (`py:intents/multisig.py:L276-398`)
turn an ordinary coldkey intent into a multisig approval for saved-multisig CLI wallets: the
executor composes semantic→sudo→proxy first, then `wrap_call` adds the outer `Multisig.as_multi`
/ `as_multi_threshold_1` layer (`L317-331`). `inner_call_data` pins exact prior-round call bytes
because rebuilds are not byte-stable (timelock commits embed fresh randomness) (`L293-299`,
`py:executor.py:L185-193`). `semantic_intent()` returns the inner intent so policy/spend/netuid
semantics are unchanged (`L379-389`).

---

## 3. Complete intent inventory (83)

Column key: **Signer** coldkey/hotkey; **Origin** signed/subnet_owner/root; **Wraps** the
`pallet.call`(s) composed (`|` = alternatives chosen at build time; `+` = composed together);
**Params** `*` = required, unit noted; **Spend** None / bounded amount / UNBOUNDED; **Touches**
netuids reported to policy (`all` = `affects_all_subnets()`); **Reads/notes** = preflight or
build-time chain reads and quirks; **Result** = extras placed into `ExtrinsicResult.data`.
All "ss58" params accept a `Wallet`/keypair (coerced to the matching key's address by
`_coerce_addresses`, `py:executor.py:L142-157`). MEV flags: `D` = `mev_shield_default=True`,
`R` = `mev_shield_required=True`.

### 3.1 Staking (`py:intents/staking.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Reads / notes | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `add_stake` (AddStake) `L542-649` D | coldkey / signed | `SubtensorModule.add_stake_limit` (default) \| `add_stake` (when `slippage_protection=False`) | `hotkey_ss58*`, `netuid*:int`, `amount_tao*:Money TAO\|"all"`, `slippage_protection=True`, `rate_tolerance=0.05` | amount; UNBOUNDED for `"all"` (`L645-649`) | [netuid] | price = `SwapRuntimeApi.current_alpha_price[netuid]`; `limit_price = int(price*(1+tol))`, `allow_partial=False` (`L596-606`); `"all"` = `System.Account.free − Balances.ExistentialDeposit − 500 000 rao` (`L248-267`); preflight quote via `SwapRuntimeApi.sim_swap_tao_for_alpha` (`L624-636`, best-effort) | — |
| `remove_stake` (RemoveStake) `L652-778` D | coldkey / signed | `remove_stake_limit` \| `remove_stake`; with `claim=True` on netuid 0: `claim_root_with_hotkey` + follow-up in `Utility.batch_all` | `hotkey_ss58*`, `netuid*`, `amount_alpha*:alpha(netuid)\|"all"`, `slippage_protection`, `rate_tolerance`, `claim=False` | None | [netuid] | `"all"` = `StakeInfoRuntimeApi.get_stake_info_for_hotkey_coldkey_netuid` (`L270-293`); claim-all uses `u64::MAX` and plain `remove_stake` (`L296-313`, `L716-734`); `claim` refused unless netuid 0 and `RootStakeUnlockInterval == 0` (`L459-494`); limit = `price*(1−tol)`; warnings read `BetaBasketRuntimeApi.get_basket_payout` (`L423-451`) | — |
| `move_stake` (MoveStake) `L781-925` D | coldkey / signed | cross-subnet: `move_stake_limit`; same-subnet: `move_stake`; optional claim batch as above | `origin_hotkey_ss58*`, `origin_netuid*`, `dest_hotkey_ss58*`, `dest_netuid*`, `amount_alpha*\|"all"`, `slippage_protection`, `rate_tolerance`, `claim` | None | [origin, dest] | cross-subnet limit = `origin_price*1e9//dest_price*(1−tol)` (`L133-147`); `claim` only from root (`L333-339`) | — |
| `add_stake_limit` (AddStakeLimit) `L928-1000` D | coldkey / signed | `add_stake_limit` | `hotkey_ss58*`, `netuid*`, `amount_tao*\|"all"`, `limit_price_rao*:int`, `allow_partial=False` | amount / UNBOUNDED | [netuid] | explicit limit; `"all"` as add_stake | — |
| `remove_stake_limit` (RemoveStakeLimit) `L1003-1108` D | coldkey / signed | `remove_stake_limit` \| `remove_stake` (root claim-all) (+claim batch) | `hotkey_ss58*`, `netuid*`, `amount_alpha*\|"all"`, `limit_price_rao*`, `allow_partial`, `claim` | None | [netuid] | root claim-all requires `0 ≤ limit ≤ 1e9` (`L1056-1071`) | — |
| `unstake_all` (UnstakeAll) `L1111-1183` D | coldkey / signed | `unstake_all` (+ claim batch) | `hotkey_ss58*`, `claim=False` | None | all | warnings read basket payout | — |
| `unstake_all_alpha` (UnstakeAllAlpha) `L1186-1220` D | coldkey / signed | `unstake_all_alpha` | `hotkey_ss58*` | None | all | — | — |
| `swap_stake` (SwapStake) `L1223-1306` D | coldkey / signed | `swap_stake_limit` \| `swap_stake` | `hotkey_ss58*`, `origin_netuid*`, `dest_netuid*`, `amount_alpha*\|"all"`, `slippage_protection`, `rate_tolerance` | None | [origin, dest] | `"all"` via stake info; limit as move_stake | — |
| `move_swap_stake` (MoveSwapStake) `L1309-1411` D | coldkey / signed | `Utility.batch_all[move_stake(same subnet), swap_stake_limit\|swap_stake]` | same as swap_stake + `dest_hotkey_ss58*` | None | [origin, dest] | swap leg uses `amount − 1 rao` (share-pool floor, issue #3081) (`L184-205`); hotkeys and netuids must differ | — |
| `transfer_stake` (TransferStake) `L1414-1602` D | coldkey / signed | `transfer_stake` \| `transfer_stake_and_hotkey` (when `dest_hotkey_ss58` differs) | `dest_coldkey_ss58*`, `hotkey_ss58*`, `origin_netuid*`, `dest_netuid*`, `amount_alpha*\|"all"`, `dest_hotkey_ss58?` | UNBOUNDED (`L1599-1602`) | [origin, dest] | warnings (same-subnet only) read `get_stake_availability_for_coldkeys`, `SubtensorModule.Lock` map, stake info to explain `LockHotkeyMismatch` (`L1531-1594`) | — |
| `set_auto_stake` (SetAutoStake) `L1605-1639` | coldkey / signed | `set_coldkey_auto_stake_hotkey` | `netuid*`, `hotkey_ss58?` | None | [netuid] | — | — |
| `stake_into_basket` (StakeIntoBasket) `L1642-1710` D | coldkey / signed | `stake_into_basket` | `hotkey_ss58*`, `amount_tao*\|"all"` | amount / UNBOUNDED | [] | `"all"` via `_stakeable_rao` | — |
| `swap_basket` (SwapBasket) `L1713-1835` D | coldkey / signed | `swap_basket` | `hotkey_ss58*`, `origin_netuid*`, `dest_netuid*`, `amount*:origin alpha\|"all"`, `min_amount_out=0:dest alpha` | None | [origin, dest] | `"all"` via `BetaBasketRuntimeApi.get_validator_basket` (`L1796-1803`) | — |
| `swap_basket_many` (SwapBasketMany) `L1838-1925` D | coldkey / signed | `swap_basket_many` | `hotkey_ss58*`, `legs*:1..128 × {origin_netuid, dest_netuid, amount, min_amount_out?}` | None | all leg netuids | legs validated at construction | — |

### 3.2 Weights (`py:intents/weights.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Reads / notes | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `set_weights` (SetWeights) `L349-439` | hotkey / signed | `set_mechanism_weights` \| `commit_timelocked_mechanism_weights` (auto by `CommitRevealWeightsEnabled`) | `netuid*`, `uids?`, `weights?` (`{uid: w}` mapping **or** parallel lists), `mechid=0`, `version_key=0`, `raw_u16=False` | None | [netuid] | preflight at one pinned block: `Uids[netuid,hotkey]`, `CommitRevealWeightsEnabled`, `WeightsSetRateLimit`, `LastUpdate[mechid*4096+netuid]`, `MinAllowedWeights`, `MaxWeightsLimit`, `SubnetEpochConsensus` (+ `ValidatorPermit` under Null) (`L154-215`); raises client-side `ChainError` with codes NOT_REGISTERED / NOT_AUTHORIZED / RATE_LIMITED / INVALID_ARGUMENT; conforms via `clip_to_max_weight` + `normalize` (u16, zeros dropped) (`L54-114`, `L218-258`); timelocked path reads `Tempo`, `RevealPeriodEpochs`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex`, `BlocksSinceLastStep`, `block_time` and calls `bittensor_core.get_encrypted_commit_v2` (`L268-333`); fee charged to owning coldkey (`py:fee_filters.py:L14-18`) | `reveal_round` (timelocked only) |
| `commit_weights` (CommitWeights) `L442-510` | hotkey / signed | `commit_timelocked_mechanism_weights` | as above + `commit_reveal_version=4` | None | [netuid] | same preflight; always timelocked | `reveal_round` |
| `reveal_weights` (RevealWeights) `L513-563` | hotkey / signed | `reveal_weights` (legacy salt commits) | `netuid*`, `uids*`, `weights*`, `salt*:list[int]`, `version_key=0` | None | [netuid] | re-normalizes floats to u16 | — |

### 3.3 Registration & subnet lifecycle (`py:intents/registration.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Reads / notes | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `pow_register` (PowRegister) `L52-111` | coldkey / signed | `pow_register` | `netuid*`, `work_block*`, `nonce*:u64`, `work_hex*:32 bytes`, `hotkey_ss58?`, `mining_workers=4`, `mining_timeout=300`, `mining_backend="auto"`, `mining_device_ids?` (CLI refresh knobs) | None | [netuid] | `mev_shield_default=False`; `client.mine_pow_registration` produces the intent (`py:client.py:L301-330`) | — |
| `burned_register` (BurnedRegister) `L114-173` D R | coldkey / signed | `burned_register` | `netuid*`, `hotkey_ss58?` | UNBOUNDED | [netuid] | effects read `Burn[netuid]`, `CollateralLockShare[netuid]` to split burn/lock (`L27-44`) | — |
| `register_subnet` (RegisterSubnet) `L176-225` | coldkey / signed | `register_network` | `hotkey_ss58?` | UNBOUNDED | [] | `execute` waits for the matching `NetworkAdded` (see §4.13) | `netuid`, `registration_mode` (`immediate`/`after_deregistration`), `registered_at_block`, `registration_price_rao`, `queued_at_block`, `cleanup_netuid`, `deregistered_netuid` (`py:executor.py:L358-386`, `L501-526`) |
| `start_call` (StartCall) `L228-254` | coldkey / subnet_owner | `start_call` | `netuid*` | None | [netuid] | — | — |
| `root_register` (RootRegister) `L257-311` | coldkey / signed | `root_register` | `hotkey_ss58?` | None | [0] | effects read `Burn[0]` | — |
| `claim_root` (ClaimRoot) `L427-479` | coldkey / signed | `claim_root` | `subnets=[0]` (ignored on chain) | None | [] | fail-closed admission (`StakingHotkeys`, `get_stake_info_for_coldkey`, `BasketClaimed`, `get_validator_basket`, `NetworksAdded`), reserve (`System.Account.free`, `estimate_fee`), quote (`get_root_basket_claim_previews` / `get_root_basket_positions`, `RootClaimableThreshold`) (`py:intents/_root_claim_fee.py:L363-431`, `L468-606`, `L723-744`); sets `required_free` / `estimated_fee` (`L389-405`) | — |
| `claim_root_with_hotkey` (ClaimRootWithHotkey) `L482-539` | coldkey / signed | `claim_root_with_hotkey` | `hotkey_ss58*` | None | [] | same machinery with the 129-unit envelope (`_root_claim_fee.py:L90-94`) | — |
| `swap_hotkey` (SwapHotkey) `L542-595` | coldkey / signed | `swap_hotkey` (legacy, `keep_stake=false` semantics) | `new_hotkey_ss58*`, `hotkey_ss58?`, `netuid?` | None | [netuid] if given | — | — |

### 3.4 Transfers & EVM (`py:intents/transfer.py`, `py:intents/evm.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Reads / notes |
| --- | --- | --- | --- | --- | --- | --- |
| `transfer` (Transfer) `transfer.py:L19-78` | coldkey / signed | `Balances.transfer_keep_alive` \| `transfer_allow_death` (`keep_alive=False`) \| `transfer_all` (`"all"`) | `dest_ss58*`, `amount_tao*\|"all"`, `keep_alive=True` | amount / UNBOUNDED | [] | — |
| `transfer_all` (TransferAll) `L81-116` | coldkey / signed | `Balances.transfer_all` | `dest_ss58*`, `keep_alive=True` | UNBOUNDED | [] | — |
| `fund_evm_key` (FundEvmKey) `evm.py:L23-74` | coldkey / signed | `Balances.transfer_keep_alive` (or `transfer_all` for `"all"`) | `evm_address*:H160`, `amount_tao*\|"all"` | amount / UNBOUNDED | [] | dest = `h160_to_ss58` mirror (`blake2_256("evm:"‖h160)`, `L26-33`) |
| `evm_withdraw` (EvmWithdraw) `L77-119` | coldkey / signed | `EVM.withdraw` | `amount_tao*\|"all"` | None | [] | address = truncated h160 of coldkey; `"all"` reads `System.Account[mirror].data.free` |

### 3.5 Children & take (`py:intents/children.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `set_children` (SetChildren) `L32-100` | coldkey / signed | `set_children` | `netuid*`, `children*:[[proportion, hotkey_ss58]…]` (float 0..1 or raw u64), `hotkey_ss58?` | None | [netuid] | sum ≤ u64::MAX checked at construction (`L71-87`) |
| `set_childkey_take` (SetChildkeyTake) `L103-136` | coldkey / signed | `set_childkey_take` | `netuid*`, `take*` (0..1 float or raw u16), `hotkey_ss58?` | None | [netuid] | |
| `increase_take` (IncreaseTake) `L139-170` | coldkey / signed | `increase_take` | `take*`, `hotkey_ss58?` | None | [] | |
| `decrease_take` (DecreaseTake) `L173-203` | coldkey / signed | `decrease_take` | `take*`, `hotkey_ss58?` | None | [] | |
| `set_take` (SetTake) `L206-247` | coldkey / signed | `increase_take` \| `decrease_take` | `take*`, `hotkey_ss58?` | None | [] | reads `SubtensorModule.Delegates[hotkey]`; equal → `BittensorError` (`L233-244`) |

### 3.6 Coldkey swap (`py:intents/coldkey.py`)

| op (class) | Signer | Wraps | Params | Spend | Touches | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `announce_coldkey_swap` (AnnounceColdkeySwap) `L36-84` | coldkey | `announce_coldkey_swap` | `new_coldkey_ss58*` | None | [] | sends `blake2b-256(pubkey)` (`L30-33`) |
| `swap_coldkey_announced` (SwapColdkeyAnnounced) `L87-127` | coldkey | `swap_coldkey_announced` | `new_coldkey_ss58*` | None | all (`L126-127`) | |
| `clear_coldkey_swap_announcement` (ClearColdkeySwapAnnouncement) `L130-154` | coldkey | `clear_coldkey_swap_announcement` | — | None | [] | |
| `dispute_coldkey_swap` (DisputeColdkeySwap) `L157-185` | coldkey | `dispute_coldkey_swap` | — | None | [] | |

### 3.7 Collateral & locks (`py:intents/collateral.py`, `py:intents/lock.py`)

| op (class) | Signer | Wraps | Params | Spend | Touches | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `add_collateral` (AddCollateral) `collateral.py:L50-134` D R | coldkey | `add_collateral` | `netuid*`, `amount_alpha*\|"all"`, `hotkey_ss58?`, `rate_tolerance=0.05` | UNBOUNDED | [netuid] | `limit_price = price*(1+tol)`; `"all"` = stake − `MinerCollateral.locked` (`L26-47`) |
| `set_min_collateral` (SetMinCollateral) `L137-176` | coldkey | `set_min_collateral` (composed via raw `Call`, param `min_locked`) | `netuid*`, `min_alpha*`, `hotkey_ss58?` | None | [netuid] | |
| `lock_stake` (LockStake) `lock.py:L15-72` | coldkey | `lock_stake` | `netuid*`, `amount_alpha*\|"all"`, `hotkey_ss58?` | None | [netuid] | `"all"` via `get_stake_availability_for_coldkeys.available` (`staking.py:L354-363`) |
| `set_perpetual_lock` (SetPerpetualLock) `L75-105` | coldkey | `set_perpetual_lock` | `netuid*`, `enabled*` | None | [netuid] | |
| `set_reject_locked_alpha` (SetRejectLockedAlpha) `L108-141` | coldkey | `set_reject_locked_alpha` | `enabled*` | None | [] | |
| `move_lock` (MoveLock) `L144-174` | coldkey | `move_lock` | `netuid*`, `destination_hotkey_ss58*` | None | [netuid] | |

### 3.8 Owner / governance / hyperparameters / identity (`governance.py`, `hyperparameters.py`, `identity.py`, `root.py`)

| op (class) | Signer/Origin | Wraps | Params | Spend | Touches | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `trim_subnet` (TrimSubnet) `governance.py:L15-56` | coldkey / subnet_owner | `AdminUtils.sudo_trim_to_max_allowed_uids` | `netuid*`, `max_n*` | None | [netuid] | |
| `trim_null_subnet_batch` (TrimNullSubnetBatch) `L59-88` | coldkey / subnet_owner | `AdminUtils.sudo_trim_null_uids_batch` | `netuid*`, `target*` | None | [netuid] | |
| `stake_burn` (StakeBurn) `L91-161` D | coldkey / signed | `add_stake_burn` | `netuid*`, `amount_tao*`, `limit_price?:rao`, `rate_tolerance=0.05`, `hotkey_ss58?` | amount | [netuid] | limit derived from spot when omitted |
| `set_mechanism_count` (SetMechanismCount) `L164-195` | coldkey / subnet_owner | `AdminUtils.sudo_set_mechanism_count` | `netuid*`, `mechanism_count*` | None | [netuid] | |
| `update_symbol` (UpdateSymbol) `L198-235` | coldkey / subnet_owner | `update_symbol` | `netuid*`, `symbol*` (utf-8 bytes) | None | [netuid] | |
| `set_hyperparameter` (SetHyperparameter) `hyperparameters.py:L91-155` | coldkey / subnet_owner | 33 `AdminUtils.sudo_set_*` setters (`L31-66`) | `netuid*`, `name*`, `value*` (raw int / human float / bool; `epoch_consensus` ∈ {Yuma, Null}) | None | [netuid] | `alpha_low`/`alpha_high` read `AlphaValues[netuid]` and keep the other (`L138-143`); fixed128 → `{"bits": v}` (`L145-147`); `collateral_lock_share`/`collateral_drain_ratio` composed via raw `Call` pending codegen (`L74-77`) |
| `set_identity` (SetIdentity) `identity.py:L17-63` | coldkey / signed | `set_identity` | `name*`, `url`, `github_repo`, `image`, `discord`, `description`, `additional` | None | [] | |
| `set_subnet_identity` (SetSubnetIdentity) `L66-121` | coldkey / subnet_owner | `set_subnet_identity` | `netuid*`, `subnet_name*`, `github_repo`, `subnet_contact`, `subnet_url`, `discord`, `description`, `logo_url`, `additional` | None | [netuid] | |
| `set_subnet_emission_enabled` (SetSubnetEmissionEnabled) `root.py:L22-86` | coldkey / **root** | `AdminUtils.sudo_set_subnet_emission_enabled` (×n in `Utility.batch_all`) + `Sudo.sudo` (executor) | `netuids*:list`, `enabled*` | None | netuids | `verify="subnet_emission_enabled"` |
| `set_root_claim_threshold` (SetRootClaimThreshold) `L89-135` | coldkey / **root** | `sudo_set_root_claim_threshold(netuid=0, new_value)` + `Sudo.sudo` | `threshold*:TAO` | None | [] | `verify="root_claim_threshold"` |

### 3.9 Serving (`py:intents/serving.py`) — all hotkey-signed, fee charged to owning coldkey

| op (class) | Wraps | Params | Touches | Notes |
| --- | --- | --- | --- | --- |
| `serve_axon` (ServeAxon) `L26-66` | `serve_axon` | `netuid*`, `ip*`, `port*`, `protocol=4`, `version=1` | [netuid] | ip → int + `ip_type` 4/6 via `ipaddress`; `placeholder1/2 = 0` |
| `serve_axon_tls` (ServeAxonTls) `L69-117` | `serve_axon_tls` | + `certificate*` (hex: 1 algorithm byte + ≤64 key bytes) | [netuid] | |
| `serve_prometheus` (ServePrometheus) `L120-154` | `serve_prometheus` | `netuid*`, `ip*`, `port*`, `version=1` | [netuid] | |
| `reset_axon` (ResetAxon) `L157-190` | `serve_axon` | `netuid*` | [netuid] | publishes `version=0, ip=0, port=1, ip_type=4, protocol=4` |

### 3.10 Proxy (`py:intents/proxy.py`)

| op (class) | Wraps | Params | Spend | Notes / Result |
| --- | --- | --- | --- | --- |
| `add_proxy` (AddProxy) `L72-115` | `Proxy.add_proxy` | `delegate_ss58*`, `proxy_type="Staking"`, `delay=0` | None | `proxy_type` ∈ 19 `PROXY_TYPES` (`L21-41`) |
| `remove_proxy` (RemoveProxy) `L118-154` | `Proxy.remove_proxy` | same triple | None | |
| `remove_proxies` (RemoveProxies) `L157-179` | `Proxy.remove_proxies` | — | None | |
| `create_pure_proxy` (CreatePureProxy) `L182-219` | `Proxy.create_pure` | `proxy_type="Staking"`, `delay=0`, `index=0` | None | result `data`: `pure_proxy`, `spawner` (from `Proxy.PureCreated`), `height`, `ext_index` (`py:executor.py:L571-593`, `L851-854`) |
| `kill_pure_proxy` (KillPureProxy) `L222-269` | `Proxy.kill_pure` | `spawner_ss58*`, `proxy_type`, `index`, `height`, `ext_index` | None | must be dispatched with `proxy_for=<pure>` |
| `execute_proxy_announced` (ExecuteProxyAnnounced) `L272-320` | `Proxy.proxy_announced` | `delegate_ss58*`, `real_ss58*`, `inner_op*`, `inner_args*`, `force_proxy_type?` | None | **quirk**: inner intent is built against the *signing* wallet, not a view of `real_ss58` (`L308`) |

### 3.11 Multisig (`py:intents/multisig.py`)

| op (class) | Wraps | Params | Reads / notes | Result |
| --- | --- | --- | --- | --- |
| `multisig_threshold_1` (MultisigThreshold1) `L165-198` | `Multisig.as_multi_threshold_1` | `other_signatories*`, `call*:{op,…}` | inner built against the derived multisig account view (`L59-82`); signatories sorted by raw key bytes (`L34-36`) | — |
| `multisig_execute` (MultisigExecute) `L201-273` | `Multisig.as_multi` | `threshold*`, `other_signatories*`, `call*`, `timepoint?{height,index}` | `max_weight` from `estimate_weight` (payment info) (`L237`); `check_multisig_funds` (opening reserves `DepositBase + threshold×DepositFactor`) (`L250-258`, `py:multisig.py:L121-158`) | `multisig_call_hash`, `multisig_call_data` (`L101-111`) |
| `multisig_approve` (MultisigApprove) `L401-488` | opening: `as_multi` (call embedded); later: `approve_as_multi` (hash only) | same + `timepoint?` | same funds preflight | same extras |
| `multisig_cancel` (MultisigCancel) `L491-536` | `Multisig.cancel_as_multi` | `threshold*`, `other_signatories*`, `call*`, `timepoint*` | — | — |

### 3.12 Crowdloan & leasing (`py:intents/crowdloan.py`, `py:intents/leasing.py`)

| op (class) | Wraps | Params | Spend | Notes |
| --- | --- | --- | --- | --- |
| `create_crowdloan` (CreateCrowdloan) `crowdloan.py:L25-113` | `Crowdloan.create` | `deposit_tao*`, `min_contribution_tao*`, `cap_tao*`, `end*`, exactly one of `target_ss58?` / `call?:{op,…}` | deposit_tao | inner intent built with signing wallet |
| `contribute_crowdloan` (ContributeCrowdloan) `L116-172` | `Crowdloan.contribute` | `crowdloan_id*`, `amount_tao*\|"all"` | amount / UNBOUNDED | `"all"` via `_stakeable_rao` |
| `finalize_crowdloan` / `refund_crowdloan` / `dissolve_crowdloan` / `withdraw_crowdloan` (`L182-274`) | `Crowdloan.finalize/refund/dissolve/withdraw` | `crowdloan_id*` | None | |
| `update_crowdloan_cap` (`L277-303`) | `Crowdloan.update_cap` | `crowdloan_id*`, `new_cap_tao*` | None | |
| `update_crowdloan_end` (`L306-336`) | `Crowdloan.update_end` | `crowdloan_id*`, `new_end*` | None | |
| `update_crowdloan_min_contribution` (`L339-373`) | `Crowdloan.update_min_contribution` | `crowdloan_id*`, `new_min_contribution_tao*` | None | |
| `set_crowdloan_max_contribution` (`L376-417`) | `Crowdloan.set_max_contribution` | `crowdloan_id*`, `new_max_contribution_tao?` (None clears) | None | |
| `register_leased_network` (RegisterLeasedNetwork) `leasing.py:L21-75` | `register_leased_network` | `emissions_share*:0-100`, `end_block?` | UNBOUNDED | must run inside a crowdloan call |
| `terminate_lease` (TerminateLease) `L78-114` | `terminate_lease` | `lease_id*`, `hotkey_ss58?` | None | |

### 3.13 Association, batch

| op (class) | Signer | Wraps | Params | Notes |
| --- | --- | --- | --- | --- |
| `associate_hotkey` (AssociateHotkey) `association.py:L13-41` | coldkey | `try_associate_hotkey` | `hotkey_ss58?` | silently no-op if already owned (docstring) |
| `associate_evm_key` (AssociateEvmKey) `L44-95` | hotkey | `associate_evm_key` | `netuid*`, `evm_key*:H160`, `block_number*`, `signature*` (EIP-191 over `hotkey_pubkey ‖ keccak256(scale(u64 block))`) | fee to coldkey |
| `batch` (Batch) `batch.py:L23-167` | inherits children's common signer (`L66-74`) | `Utility.batch_all` | `intents*:[{op,…}]` (no nesting, `L54-55`) | spend aggregates: sum of bounded, any UNBOUNDED ⇒ UNBOUNDED (`L149-161`); netuids union (`L163-164`); child extras namespaced `"{i}:{op}.{key}"` (`L84-87`); `required_free` = max over children (`L132-139`) |

### 3.14 Spend classification summary

- **UNBOUNDED**: `burned_register`, `register_subnet`, `register_leased_network`, `add_collateral`,
  `transfer_stake`, `transfer_all`, and every `"all"`-amount use of `transfer`, `fund_evm_key`,
  `add_stake`, `add_stake_limit`, `contribute_crowdloan`, `stake_into_basket`; `batch` if any
  child is unbounded.
- **Bounded**: `transfer`, `fund_evm_key`, `add_stake`, `add_stake_limit`, `contribute_crowdloan`,
  `stake_into_basket`, `create_crowdloan` (deposit), `stake_burn`.
- **None** (value does not leave the signer as TAO): everything else — including unstake/move/
  swap intents, which is deliberate (`py:intents/base.py:L300-309`).

### 3.15 Chain calls deliberately *not* wrapped (raw-only)

`sdk/python/codegen/check.py:L58-300` enumerates `COVERED_PALLETS` (SubtensorModule, Balances,
Proxy, Utility, Multisig, Crowdloan, Swap, LimitOrders, MevShield, EVM, Commitments, AdminUtils,
Contracts) and a `RAW_ONLY` set per pallet; the gate fails if any call is neither wrapped nor
listed. Notable raw-only calls a subnet toolkit may still want: `SubtensorModule.set_weights`,
`commit_weights`, `batch_set_weights`, `batch_commit_weights`, `batch_reveal_weights`,
`commit_crv3_mechanism_weights`, `commit_timelocked_weights`, `reveal_mechanism_weights`,
`register` (PoW legacy), `register_limit`, `swap_hotkey_v2`, `burn_alpha`, `recycle_alpha`,
`remove_stake_full_limit`, `trigger_epoch`, `dissolve_network`, `Utility.batch`/`force_batch`,
`Proxy.announce`/`reject_announcement`/`remove_announcement`, `Commitments.set_commitment`,
`MevShield.submit_encrypted` (exposed as a submission mode instead), all `LimitOrders.*`, all
`Swap.*` (deprecated LP calls), all `Contracts.*`, and ~55 root-only `AdminUtils.sudo_set_*`.

---

## 4. Plan / execute lifecycle (exact)

### 4.1 Entry and normalization

`Client.plan/execute/execute_tool/submit_shielded/submit_call/prepare_call/submit_signature`
forward to one `Executor` created with the client-wide `Policy` (`py:client.py:L182`,
`L468-540`). Every path first does:

1. `as_wallet(wallet)` — a string becomes `Wallet("cold")` / `Wallet("cold/hot")`
   (`py:signing.py:L98-103`).
2. `_coerce_addresses(intent)` — any `*_ss58`/`*_ss58s` field holding a `Wallet`/keypair/signer is
   replaced by the matching key's address (hotkey fields → hotkey, else coldkey); returns a new
   dataclass instance only when something changed (`py:executor.py:L142-157`,
   `py:signing.py:L106-140`).

### 4.2 Call composition order (`_compose_intent_call`, `py:executor.py:L175-219`)

```
pinned inner_call_data?  → intent.wrap_call(substrate, wallet, None)        # multisig later rounds
semantic = _coerce_addresses(intent.semantic_intent())
origin   = _OriginView(proxy_for) if proxy_for else intent.origin_view(substrate, wallet)
built    = await semantic.build(substrate, origin)          # CallBytes | BuiltCall
call     = nest_origin_wrappers(call, sudo=(semantic.origin=="root"), proxy_for, proxy_type)
           #  Sudo.sudo(call=inner)  then  Proxy.proxy(real=proxy_for, force_proxy_type, call=inner)
call     = intent.wrap_call(substrate, wallet, call)        # saved-multisig adapter layer
```

- `nest_origin_wrappers` (`L99-121`) composes each inner call to bytes first; an already-wrapped
  `Sudo.sudo` is not double-wrapped (`_is_sudo_call`, `L92-96`); `proxy_type` is validated against
  `PROXY_TYPES` (`py:intents/proxy.py:L49-54`).
- The same function is used by `btcli call` so raw calls and intents wrap identically (`L107`).
- With both `proxy_for` and a multisig adapter, the semantic build runs against the proxied
  account view (`L195-200`).

### 4.3 dispatch_origin vs fee_payer (`_intent_accounts`, `L234-246`)

`fee_payer = public_view(wallet, intent.signer).ss58_address` (never unlocks). `dispatch_origin`
is `proxy_for` when proxying, else `public_view(intent.origin_view(...), semantic.signer)`. Both
are handed to `Intent.preflight` so affordability checks look at the right account
(`py:intents/base.py:L109-116`).

### 4.4 Preflight and fee estimation

- `Executor._preflight` composes (if not given) and calls `intent.preflight(substrate,
  dispatch_origin, fee_payer, call=call)` (`L677-707`).
- `Executor.plan` (`L709-772`): collects `preflight.warnings`; appends
  `COLDKEY_FEE_WARNING` when the signer is a hotkey, no proxy, and any leaf of the composed call
  is in `COLDKEY_PAYS_FEE_CALLS` (`L744-745`, `py:fee_filters.py:L68-98` walks through
  `Proxy.proxy`, `Utility.batch/batch_all/force_batch`, `Multisig.as_multi/as_multi_threshold_1`);
  uses `preflight.estimated_fee` if an intent supplied an exact one, else
  `substrate.estimate_fee(call, public_keypair)` — **best-effort**: failure becomes a warning,
  not a block (`L746-751`).
- Fee estimation mechanics (`py:_transport/interface.py:L714-731`): nonce from
  `account_nextIndex` without touching the cache; a signed extrinsic is assembled with a **zeroed
  64-byte signature** and an **immortal** era (`sign_without_nonce_tracking` with `era=None` →
  `IMMORTAL`, `L533-562`, `L485-491`); the RPC is `state_call("TransactionPaymentApi_query_info",
  extrinsic ‖ u32-LE length)`; result decoded against the runtime API output type; the SDK reads
  `partial_fee` (`py:_substrate.py:L564-568`). `estimate_weight` returns
  `{ref_time, proof_size}` from the same call (`L570-577`) and feeds multisig `max_weight`.
- `Executor.preflight` (public) additionally tries `estimate_fee` when the intent did not
  supply one, swallowing exceptions (`L638-675`).

### 4.5 Policy (`py:intents/plan.py:L21-85`)

`Policy(max_fee_tao, max_spend_tao, allowed_netuids, allow_raw_calls=False)`; money limits are
normalized to exact `Balance` (`L38-42`). `check(intent, fee)` runs against
`intent.semantic_intent()` and returns violations:

- `max_fee_tao` set and `fee is None` → **violation** ("must not fail open", `L54-63`).
- `max_spend_tao`: `UNBOUNDED` spend → violation; bounded spend > cap → violation (`L64-72`).
- `allowed_netuids`: `affects_all_subnets()` → violation; each `touches_netuids()` outside the set →
  violation (`L73-84`).
- Raw calls (`submit_call`, `submit_signature`) are refused unless `allow_raw_calls` (`L44-49`,
  `py:executor.py:L632-636`, `L1237`, `L1309`).

A per-call `policy=` overrides the client-wide one (`_active_policy`, `L612-614`).
Intent-level `blocks` are appended to violations (`L756-757`) so a dry run shows them and
`execute` refuses.

### 4.6 `Plan` (`py:intents/plan.py:L89-135`)

Fields: `op, summary, signer, signer_address, fee, effects, warnings, violations, call (not
serialized), extras, spend, args`. `to_dict()` adds `fee_tao`, `spend_tao`, `spend_unbounded`,
`ok`, `docs_url`. `execute` submits **exactly** `plan.call` (`py:executor.py:L825-840`), so the
dry run and the signed bytes cannot diverge.

### 4.7 `execute` (`py:executor.py:L774-889`)

```
if semantic.mev_shield_required → return submit_shielded(...)                       # L812-824
plan = self.plan(...); if not plan.ok: raise PolicyError(plan.violations)           # L825-829
keypair = resolve_signer(wallet, intent.signer)                                      # L831
attempts = max(0, retries) + 1
for attempt: result = substrate.submit(plan.call, keypair, period, wait_for_inclusion, wait_for_finalization)
    break if result.success or last attempt or not _is_transient(result)
    sleep(substrate.block_time())                                                    # one chain block
result = _with_nested_dispatch_failure(result)                                       # defense for fakes
if success: data += _pure_created_data (create_pure_proxy) + plan.extras             # L849-856
if success and op == "register_subnet" and wait_for_registration: _complete_subnet_registration
```

- Defaults: `period=128`, `wait_for_inclusion=True`, `wait_for_finalization=True`,
  `retries=0`, `wait_for_registration=True` (`L782-788`). The module-level
  `bt.set_weights(...)` helper uses `retries=2` and raises on failure (`py:_subtensor.py:L167-205`).
- **Retry semantics**: only pool rejections whose message contains `"priority is too low"`,
  `"transaction is outdated"` or `"stale"` are retried; a fresh nonce is fetched each attempt
  because `_failed_submit` clears the nonce cache (`L52-56`, `L87-89`,
  `py:_substrate.py:L753-772`). Dispatch failures are never retried (`L798-800`).
- Signer resolution (`py:signing.py:L227-263`): `Wallet` → `WalletSigner` (lazy unlock on first
  `sign`, one unlock per process, `L143-224`); `Signer` passes through; objects flagged
  `uses_extension_signing` pass through; `KeyedWallet` yields `coldkey`/`hotkey`.

### 4.8 Nonce handling

- `NonceCache` (`py:_transport/extrinsics.py:L55-84`): first request per account asks
  `account_nextIndex`; later requests increment locally (pipelining); `pin(address, nonce)`
  records an explicit nonce; `clear(address)` drops the entry.
- `create_signed_extrinsic` takes the cached next nonce or pins an explicit one
  (`py:_transport/interface.py:L501-531`); `prepare_unsigned` always fetches fresh
  (`use_cache=False`) because the signature may come back much later (`L589-590`).
- Any failure between signing and inclusion clears the account's cache entry ("a failed submit
  does not consume the nonce on-chain", `py:_substrate.py:L753-762`, docstring `L637-642`).
- After a websocket reconnect, in-flight `author_submitExtrinsic` /
  `author_submitAndWatchExtrinsic` frames are **never resubmitted** — their futures fail with
  "fate unknown" so a duplicate cannot hit "already imported" and clear the cache under a live
  transaction (`py:_transport/rpc.py:L62-66`, `L686-711`).
- Shielded submissions pin `nonce` (carrier) and `nonce+1` (inner) explicitly (§6.5).

### 4.9 Era / mortality

- `DEFAULT_ERA_PERIOD = 128` (`py:settings.py:L22`); docs: "valid for 128 blocks (~25 minutes)"
  (`docs:concepts/transactions.mdx:L187-194`). `period=None` → immortal (`py:_substrate.py:L649`).
- `MEV_SHIELD_ERA_PERIOD = 8` (`py:settings.py:L24-29`), chain-enforced by
  `runtime/src/check_mortality.rs:L19` (`MAX_SHIELD_ERA_PERIOD = 8`; immortal or longer eras on
  `submit_encrypted` are rejected at pool submission with `InvalidTransaction::Stale`, `L91-101`).
- Era normalization (`py:_transport/interface.py:L485-499`) **[verified quirk]**: a mortal era
  dict without `current` is anchored at the **finalized** head (`get_finalized_block_number`),
  and `era_block_hash = block_hash(era_birth(period, current))`. Immortal eras anchor at the
  genesis hash. (The Rust `Client::sign_extrinsic` anchors at the **best** head instead,
  `core:client.rs:L515-519`.)
- `era_birth(period, current)` (`core:codec/extrinsic.rs:L407-412`):
  `calculated = next_power_of_two(period).clamp(4, 1<<16)`; `quantize = max(calculated>>12, 1)`;
  `phase = (current % calculated) / quantize * quantize`;
  `birth = (max(current, phase) − phase) / calculated * calculated + phase`.
- Era SCALE encoding (`core:codec/encode.rs:L445-485`): `"00"` → `0x00`; dict with `period` and
  (`phase` | `current`) → two-byte mortal encoding with the low nibble `log2(calculated)−1` and
  `phase/quantize_factor << 4`.
- Docs `transactions.mdx:L187-194` also state an extrinsic signed past its era "lapses instead
  of lingering in the pool indefinitely".

### 4.10 Submission and outcome resolution

**Submit** (`py:_transport/interface.py:L626-663`): without waiting →
`author_submitExtrinsic` returns the hash only (`InclusionReport(extrinsic_hash)`); otherwise
`author_submitAndWatchExtrinsic` subscription, interpreted by `watch_status_block`
(`py:_transport/extrinsics.py:L352-374`): `usurped`, `retracted`, `finalitytimeout`,
`dropped`, `invalid` → raise `SubstrateRequestException("Extrinsic <status>: …")`;
`finalized` → done when `wait_for_finalization`; `inblock` → done when only inclusion is
awaited. The subscription is always unsubscribed (`L655-656`).

**Resolve** (`_resolve_inclusion`, `L677-712`): fetch `chain_getBlock(block_hash)`, locate the
extrinsic by **blake2b-256 of each raw extrinsic** (`L686-690`); if absent raise
`ExtrinsicNotFound` ("never fabricate an outcome", `L691-697`); read decoded `System.Events`
(`get_events`, `L434-437`) and keep records with `extrinsic_idx == idx`; `resolve_outcome`
(`py:_transport/extrinsics.py:L377-426`):

- fee = `TransactionPayment.TransactionFeePaid.actual_fee` if present else the sum of
  `Treasury.Deposit.value` / `Balances.Deposit.amount` (older runtimes) (`receipt.py:L75-96`);
- success = `System.ExtrinsicSuccess` present and no failure; weight from `dispatch_info.weight`;
- failure = `System.ExtrinsicFailed` (module error resolved to name/docs via
  `codec.module_error(pallet_index, first byte of error)`, `receipt.py:L170-189`; non-module
  variants via `build_system_error_message`, `L192-237`) **or** any MevShield failure event
  (`DecryptedRejected`, `DecryptionFailed`, `ExtrinsicDispatchFailed`, `ExtrinsicDecodeFailed`,
  `ExtrinsicExpired`; `L104-118`, `L130-167`);
- **nested wrapper failure**: an outer `ExtrinsicSuccess` with `Sudo.Sudid/SudoAsDone.sudo_result`,
  `Proxy.ProxyExecuted.result`, `Multisig.MultisigExecuted.result` = `Err`, or
  `Utility.BatchInterrupted/ItemFailed.error`, is reported as failure (`receipt.py:L8-52`,
  `extrinsics.py:L411-417`).

**Typed result** (`py:_substrate.py:L774-811`): `ExtrinsicResult(success, message="Success",
block_hash, extrinsic_id="{block}-{idx:04d}", explorer_url (finney/archive only), fee=Balance,
events=[…])` or on failure `message=docs|name`, `error=ChainError(text, name)`. If inclusion
was not awaited: `ExtrinsicResult(success=True, message="Submitted (not waiting).")` (`L776-777`)
— **gotcha**: `success=True` means *accepted by the pool*, not executed.
`ExtrinsicResult` is frozen; derived outcomes use `dataclasses.replace` (`py:result.py:L696-725`);
`__bool__` is `success`; `raise_for_failure()` raises the embedded `ChainError`
(`L727-731`); `to_dict()` emits `fee_tao` and `error.to_dict()` (`L733-747`).

Transport exceptions during submit → `_failed_submit` (`py:_substrate.py:L753-772`): clears the
nonce cache; `RpcPolicyRejection`/`MaxRetriesExceeded` **raise** `RpcConnectionError`/
`RpcPolicyError`; other `SubstrateRequestException` → failed `ExtrinsicResult` with
`chain_error_from_substrate_request`; anything else raises `ChainError`.

### 4.11 Timeouts and bounds

| What | Value | Ref |
| --- | --- | --- |
| websocket handshake | 10 s | `py:_transport/rpc.py:L60`, `L86-91` |
| response silence while requests pending | 60 s (`response_timeout`) → connection presumed dead, reconnect | `L323`, `L733-741` |
| response timeouts per endpoint before rotating | 5 (`max_retries`) | `L322`, `L633-634` |
| give up (non-`retry_forever`) | `max_retries × len(urls)` consecutive failures | `L635-644` |
| reconnect backoff | `min(0.25·n, 5.0)` s between consecutive failures; `min(2·cycle, 30)` s between full-pool cycles when `retry_forever` | `L648`, `L676-678` |
| subscription buffer | 2048 items, oldest dropped | `L70`, `L280-291` |
| watch for inclusion/finality | **unbounded** apart from the 60 s silence watchdog and pool fatal statuses | `interface.py:L638-663` |
| shielded inner-extrinsic scan | until `included_at + period`, at most 4 waits per remaining block; finality polling 4×(period+1) polls, 4 consecutive RPC failures allowed | `py:executor.py:L65-66`, `L1091-1214` |
| register_subnet wait | `registration_timeout` (None = unbounded) via `asyncio.wait_for` | `L568` |
| head-codec TTL | 12 s | `py:_transport/runtime.py:L143` |
| finalized-height cache TTL | 12 s | `py:_transport/interface.py:L45` |

### 4.12 Raw and offline paths

- `submit_call(call, wallet, signer="coldkey", policy, period=128, wait_*, shielded=False)`
  (`py:executor.py:L1216-1256`): policy must allow raw calls; composes and submits; `shielded`
  uses the 8-block era unless an explicit non-default `period` is given (`L1241`).
- `prepare_call(call, address, crypto_type=1, nonce, period=128, tip=0, metadata_hash)` →
  `UnsignedExtrinsic` (`L1258-1292`); `submit_signature(unsigned, signature)` reattaches and
  submits, refusing if the runtime `spec_version` changed since preparation
  (`py:_transport/interface.py:L617-623`). `UnsignedExtrinsic.to_dict/from_dict` round-trip
  through JSON (`py:_transport/contract.py:L120-134`).

### 4.13 `register_subnet` completion (`_complete_subnet_registration`, `py:executor.py:L333-568`)

A successful `register_network` can mean "created" (`NetworkAdded`) or "queued behind
dissolution" (`NetworkRegistrationQueued`). The executor scans the receipt; on the queued path it
rescans the inclusion block (on_idle events are not in the receipt), then every height up to the
head, then follows `chain_subscribeNewHeads`, accepting only a `NetworkAdded` whose
`SubnetOwner`/`SubnetOwnerHotkey` match and which is not still listed in
`NetworkRegistrationQueue` (`L468-500`). Progress callbacks: `queued`, `waiting`, `registered`.
The shielded path uses `deferred_dispatch=True` so a bare receipt without events still follows
(`L1005-1013`).

---

## 5. Error taxonomy

### 5.1 Shapes

- `ErrorCode` (18 values, `py:error_map.py:L23-41`): `insufficient_balance`,
  `insufficient_liquidity`, `rate_limited`, `not_registered`, `not_authorized`, `already_exists`,
  `not_found`, `subnet_not_exists`, `subtoken_disabled`, `disabled`, `too_early`, `expired`,
  `limit_exceeded`, `unit_mismatch`, `invalid_argument`, `policy_violation`, `internal`, `unknown`.
- `ChainError(message, name=None, code=None)` (`py:result.py:L501-562`): `.code` defaults to
  `classify_error(message, name)`; `.remediation` = per-name override → substring override →
  per-code default; `.description` = per-name prose from `error_descriptions/` or
  `DISPATCH_ERRORS`; `.docs_url` = `/docs/errors/chain/<Name>` when the name is classified else
  `/docs/errors/<code-with-dashes>`; `.to_dict()` emits all of these.
- `PolicyError(violations)` with `code = POLICY_VIOLATION` (`L687-693`).
- Hierarchy: `BittensorError` ← `ConnectionNotReady`, `RpcConnectionError` ← `RpcPolicyError`
  (`status_code`, `retry_after`, `policy`, `reason`), `ChainError`, `PolicyError` (`L45-73`);
  `UnitMismatchError` lives in `balance.py`.

### 5.2 Classification precedence (`classify_error`, `py:result.py:L483-498`)

1. name in `NAME_TO_CODE` (390 chain module error names, grouped by pallet, `error_map.py:L46-476`; the same 390 names appear in `errors.json` `chain_errors`);
2. name in `DISPATCH_ERRORS` (`L555-795`);
3. name ends with `RateLimitExceeded` or contains `RateLimit` → `RATE_LIMITED`;
4. no name: substring fallback on the message (`_SUBSTRING_FALLBACK`, `result.py:L291-304`:
   "balance too low", "inability to pay some fees", "insufficient", "not enough" →
   INSUFFICIENT_BALANCE; "rate limit", "too fast", "priority is too low" → RATE_LIMITED;
   "already imported" → ALREADY_EXISTS; "bad signature", "invalid transaction" →
   INVALID_ARGUMENT); else `UNKNOWN`.

### 5.3 Where chain errors come from and how they map

| Source | Wire form | Resolution | Ref |
| --- | --- | --- | --- |
| Dispatch `Module{index, error}` (in `System.ExtrinsicFailed` or nested wrapper events) | pallet index + 4-byte error (first byte used) | generated catalog `ERRORS[(pallet_idx, err_idx)] → (pallet, name, docs)` (`py:_generated/errors.py`, 405 entries) | `receipt.py:L170-189`, `result.py:L643-656` |
| Dispatch non-module (`BadOrigin`, `CannotLookup`, `Token{FundsUnavailable…}`, `Arithmetic{…}`, `Transactional{…}`, `Exhausted`, `Corruption`, `Unavailable`, `RootNotAllowed`, `Other`) | enum variant | `DISPATCH_ERRORS[name] → (code, prose)`; `Token/Arithmetic/Transactional` use the inner variant name | `receipt.py:L192-237`, `result.py:L657-684` |
| Pool rejection `InvalidTransaction::Custom(N)` | JSON-RPC error 1010 with `"Custom error: N"` text or `{"Custom": N}` data | `CUSTOM_TRANSACTION_ERRORS[N] → name` (codes 0–27, 255) then classify by name | `result.py:L574-614`, `error_map.py:L488-518` |
| Standard `InvalidTransaction`/`UnknownTransaction` Display strings | text needles (`"inability to pay some fees"` → `Payment`, `"will be valid in the future"` → `Future`, `"transaction is outdated"` → `Stale`, `"has a bad signature"` → `BadProof`, `"ancient birth block"`, `"block limit"` → `ExhaustsResources`, `"priority is too low"` → `PriorityTooLow`, `"already imported"` → `AlreadyImported`, …) | `INVALID_TRANSACTION_ERRORS` → name → `DISPATCH_ERRORS` | `error_map.py:L524-541` |
| Watch-status fatals | `"Extrinsic usurped: …"` etc. | regex → `Usurped`/`Retracted`/`FinalityTimeout`/`Dropped`/`Invalid` | `result.py:L31-42`, `L607-609` |
| MevShield inclusion events | `ExtrinsicDecodeFailed`, `ExtrinsicExpired`, `DecryptionFailed`, `DecryptedRejected`, `ExtrinsicDispatchFailed` | `{"type":"MevShield", name, docs}` | `receipt.py:L142-160` |

The runtime side of the custom codes: `common/src/transaction_error.rs:L4-73` (enum and
`u8` codes; `BadRequest = 255`, `L56`), produced at pool validation by
`SubtensorTransactionExtension` which maps pallet errors via `From<Error<T>> for
CustomTransactionError` (`pallets/subtensor/src/extensions/subtensor.rs:L34-68`; umbrella
mappings: `AmountTooLow | NotEnoughStakeToSetWeights → StakeAmountTooLow`,
`CommittingWeightsTooFast | SettingWeightsTooFast | NetworkTxRateLimitExceeded →
RateLimitExceeded`, `HotKeyNotRegisteredInSubNet → UidNotFound`, `_ → BadRequest`). The shield
extension rejects malformed ciphertext with `FailedShieldedTxParsing` (code 23)
(`pallets/shield/src/extension.rs:L72-75`). Docs reproduce the table
(`docs:concepts/transactions.mdx:L101-141`).

### 5.4 Catalog on disk

`…/scratchpad/docs/errors.json` has `codes` (18 entries: `remediation`, `docs_url`,
`markdown_url`) and `chain_errors` (390 names, each `{code, description, docs_url, markdown_url,
pallets, sources[{pallet, path, line, url, raw_url}]}`). Distribution by code (jq count):
invalid_argument 145, limit_exceeded 45, already_exists 26, insufficient_balance 25, disabled 22,
not_authorized 22, too_early 21, not_found 21, internal 19, rate_limited 17,
insufficient_liquidity 9, expired 6, not_registered 4, subnet_not_exists 4, policy_violation 3,
subtoken_disabled 1.

### 5.5 CI gates that keep the taxonomy honest (`sdk/python/codegen/check.py`)

`--names` (`L354-420`): every classified name must exist on chain; every chain name must
classify; every classified name needs a description; descriptions must not be orphaned/empty;
custom codes must equal `range(28) ∪ {255}` (`L350-351`); every `ErrorCode` needs an
explanation and remediation. `--drift` regenerates `_generated/` from a live node and fails on
diff (`L32-53`). Coverage (`L303-345`): every call in `COVERED_PALLETS` is wrapped or listed
raw-only; `origin == "root"` intents must declare `verify`.

---

## 6. Transport write-path design

### 6.1 `RpcSession` (`py:_transport/rpc.py`)

- One websocket per session; a single **supervisor task** owns connect/pump/reconnect
  (`L604-648`); callers send directly under a lock and await per-request futures (`L432-437`,
  `L528-567`). JSON-RPC 2.0 batches supported (`request_batch`, `L439-452`).
- **Pending resubmission**: every pending request keeps its frame; after reconnect the supervisor
  resends all unanswered frames *before* opening the `_connected` gate, except in-flight extrinsic
  submissions which fail with "Connection dropped while an extrinsic submission was in flight"
  (`L686-725`).
- **Subscriptions** cannot survive a reconnect: each open `Subscription` gets an exception pushed
  ("The transaction may already be on chain — verify balances before retrying", `L582-602`).
  Early notifications that race the subscribe response are buffered (max 64 ids; `L297-300`,
  `L775-793`); late notifications for closed ids are dropped (`L477-483`).
- **Endpoint pool**: `[url] + fallback_urls`; rotate on connect failure or every `max_retries`
  consecutive failures; `retry_forever` cycles indefinitely (`L315-345`, `L650-684`).
- **Policy refusals are terminal, never retried/rotated** (to avoid amplifying traffic): HTTP
  403/429 on handshake with `X-RateLimit-Policy`/`X-RateLimit-Reason`/`Retry-After` headers
  (`L92-124`), JSON-RPC codes `-32004`/`-32005` with the gateway's `policy/reason/retry_after`
  data (`L53-59`, `L190-230`), websocket close code 1008 (`L233-241`). They surface as
  `RpcPolicyRejection` → public `RpcPolicyError` (`py:_substrate.py:L49-58`).
- `"State already discarded for <hash>"` → `StateDiscardedError`; `RpcSubstrate._read` retries
  such reads on the lazily opened **archive** pool (`py:_substrate.py:L383-400`).
- Non-idempotent methods: `author_submitExtrinsic`, `author_submitAndWatchExtrinsic` (`L66`).
- Diagnostics are sanitized (userinfo stripped, control characters removed, bounded length,
  `L136-187`). Logging under `bittensor.transport` / `bittensor.transport.raw_websocket`.

### 6.2 `SubstrateConnection` + `RuntimeManager`

- `initialize()` connects and warms the head codec (`py:_transport/interface.py:L144-148`).
- **Runtime codec cache keyed by spec_version** (`py:_transport/runtime.py:L123-143`): LRU of 4
  `RuntimeCodec`s, 512 `block_hash → spec_version` entries, head codec re-validated against
  `state_getRuntimeVersion` at most every 12 s (so an upgrade is picked up "within about a
  block", `L150-176`). A block's codec is resolved via its **parent** hash's runtime version
  (`L178-207`). Concurrent fetches for one spec share a future (`L209-238`).
- Metadata is fetched as **V15** via `state_call("Metadata_metadata_at_version", 0x0f000000)` and
  the `Option<OpaqueMetadata>` is unwrapped; pre-V15 runtimes fall back to `state_getMetadata`
  (V14) (`L292-310`, `codec.py:L43-58`). `transactionVersion` missing → refuse to build a
  signing codec (`L42-54`).
- **Disk cache**: `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache/`,
  file `md1-<genesis16hex>-<spec>.bin` = magic `STMD` + `<IB` (transaction_version, is_v15) +
  raw `MetadataVersioned` bytes; atomic write; 8 most recent kept (`L36-121`).
- `RuntimeCodec` wraps `bittensor_core.Runtime(metadata_bytes, spec_version,
  transaction_version, ss58_format)` (`codec.py:L147-173`). The Rust `Runtime::parse` accepts
  V14/V15 only and errors on other versions (`core:runtime/mod.rs:L163-195`) — the finney
  **V16** dump in `meta:finney.json` is therefore not what the SDK consumes; it fetches V15.
- Block-hash caches only store **finalized** number↔hash mappings (reorg safety), gated by a
  finalized height probed at most every 12 s (`interface.py:L187-216`).
- `query_batch` uses `state_queryStorageAt` with all keys at a pinned hash; `query_map` pins the
  head so keys and values are read at one block (`L256-379`).

### 6.3 `CallBytes` and call nesting

`CallBytes(data: bytes, spec_version: int)` with a cached blake2b-256 `call_hash`
(`py:_transport/contract.py:L17-33`). `RuntimeCodec.compose_call(module, function, params)` →
`CallBytes`; nested calls (Sudo/Proxy/Multisig/Batch) are a byte splice because the core's call
encoder accepts pre-composed calls as raw bytes (`codec.py:L108-120`, `L226-228`). `None` encodes
as `0x00` Option-None (`L181-190`). The generated builders are typed 3-tuples
`Call(module, function, params)` (`py:_generated/calls.py:L8-16`).

### 6.4 Signing pipeline and signer protocols

`create_signed_extrinsic` = `prepare_extrinsic` → `sign_unsigned` → `attach_signature`
(`py:_transport/extrinsics.py:L268-318`):

1. `prepare_extrinsic` builds `payload = call_data ‖ included_in_extrinsic ‖
   included_in_signed_data` (via `codec.signature_payload_parts`), blake2b-256-hashing payloads
   longer than 256 bytes, and packs every field (era, nonce, tip, tip_asset_id, genesis hash, era
   block hash, spec/tx version, metadata hash, Polkadot-JS `SignerPayloadJSON`) into
   `UnsignedExtrinsic` (`L135-199`, `contract.py:L67-134`).
2. `sign_unsigned` dispatches on signer shape (`L321-347`): `UnsignedExtrinsicSigner`
   (whole prepared extrinsic — Ledger clear-signing), `ExtensionPayloadSigner`
   (`sign_extrinsic_payload(payload_json)` → `{"signature": "0x…"}`), else `sign(payload)`; any
   may be a coroutine.
3. `attach_signature` normalizes a 65-byte signature (MultiSignature version prefix in byte 0)
   or uses the signer's `crypto_type` as the version (`L202-213`) and calls
   `codec.encode_signed_extrinsic(..., metadata_hash_enabled=unsigned.metadata_hash is not None)`
   (`L216-239`).
4. `resolve_metadata_hash` calls an optional `metadata_digest(SigningContext)` hook on
   `MetadataVerifyingSigner`s (`L242-265`); the context carries raw metadata bytes, spec
   name/version, tx version, ss58 format, genesis hash (`contract.py:L49-63`).

`SignerPayloadJSON` (`L87-132`): numeric fields are **big-endian value hex at SCALE width**
(`nonce` 4 bytes, `tip` 16 bytes, `specVersion`/`transactionVersion` 4 bytes), `method`/`era` raw
SCALE hex, `signedExtensions` = runtime identifiers, `mode`/`metadataHash` = `1/"0x…"` or
`0/null` when `CheckMetadataHash` is declared.

Protocols (`py:_transport/protocols.py`): `Keypair{crypto_type, public_key, ss58_address,
ss58_format, sign}` (`L29-50`), `ExtensionPayloadSigner` (`L54-62`), `MetadataVerifyingSigner`
(`L66-76`), `UnsignedExtrinsicSigner` (`L80-91`). Concrete implementations: `WalletSigner`
(`py:signing.py:L143-224`), `LedgerSigner` (`py:ledger.py:L115,L130,L165`), `VaultSigner`
(`py:vault/signer.py:L96-224`, QR/UOS), `ExtensionSigner` (`py:extension/signer.py:L70-126`,
browser bridge). `two_stage` / `warm_up` are set by the executor before a shielded submit
(`py:executor.py:L124-139`).

### 6.5 MEV-shielded submission

Chain side (`pallets/shield/src/lib.rs`): `NextKey` is the ML-KEM-768 encapsulation key of the
**N+2** block author (`L113-115`, rotation `L269-303`), expiring at `now + 3` (`L323-326`);
`submit_encrypted(ciphertext: BoundedVec<u8, 8192>)` only emits `EncryptedSubmitted{id, who}`
(`L354-363`); the ciphertext layout is `key_hash(16) ‖ kem_len(u16 LE) ‖ kem_ct ‖ nonce(24) ‖
aead_ct` with `key_hash = twox_128(NextKey)` (doc `L336-351`); the block author decrypts via the
runtime APIs `try_decode_shielded_tx` / `try_unshield_tx` (`runtime/src/lib.rs:L2796-2805`,
`pallets/shield/src/lib.rs:L582-637`) and includes the inner extrinsic in its block; the separate
`store_encrypted`/`on_initialize` queue path emits `ExtrinsicExpired`/`ExtrinsicDecodeFailed`/
`ExtrinsicDispatchFailed`/`ExtrinsicDispatched`/`ExtrinsicPostponed`/`ExtrinsicWeightExceeded`
(`L482-580`). `CheckShieldedTxValidity` rejects unparsable ciphertext at the pool
(`pallets/shield/src/extension.rs:L51-78`); `CheckMortality` enforces era ≤ 8 (§4.9).

SDK side (`Executor._submit_encrypted_call`, `py:executor.py:L1016-1071`):

```
pubkey = query MevShield.NextKey (retry twice, one block apart)         # L1027-1034
label two-stage signer / warm up Vault                                  # L1036
nonce = account_nextIndex(signer)                                       # L1037
inner_bytes, inner_hash = sign_extrinsic(call, keypair, nonce=nonce+1, period=8)   # L1038-1040
ciphertext = bittensor_core.encrypt_mlkem768(pubkey, inner_bytes, include_key_hash=True)  # L1041
outer = compose MevShield.submit_encrypted(ciphertext)                  # L1042-1044
result = submit(outer, keypair, nonce=nonce, period=8, wait_*)          # L1045-1052
data += {shielded: True, inner_extrinsic_hash}                          # L1053-1062
result = _resolve_shielded_inner(result, inner_hash, period)            # L1063-1070
```

`_resolve_shielded_inner` (`L1073-1214`) scans blocks from the carrier's inclusion block to
`included_at + period` for the inner hash (`find_extrinsic` → `resolve_extrinsic`), with bounded
waits for head lag; with `wait_for_finalization` it polls the finalized head, re-resolves the
canonical hash at that height after finality, and rescans if a reorg replaced the block; if the
inner never lands the result is flipped to failure with "the decrypted extrinsic … was not
included before its era expired" (`L1205-1214`). Then nested-wrapper failures are re-checked.

Policy/fees in `submit_shielded` (`L897-1014`): `max_fee_tao` with no fee estimate is a hard
`PolicyError` (unlike `plan`), and when the intent reports `required_free` the executor
estimates the carrier fee by pricing `submit_encrypted` with an 8192-byte ciphertext
(`estimate_shielded_carrier_fee`, `L79-84`) and blocks if free TAO covers the inner reserve but
not reserve + carrier (`L955-979`). Defaults: `period=8`, `wait_for_inclusion=True`,
`wait_for_finalization=False` (`L905-907`).

The Rust seal (`core:mlkem/mod.rs:L53-107`): ML-KEM-768 encapsulate → 32-byte shared secret used
**directly** as the XChaCha20-Poly1305 key (no HKDF), random 24-byte nonce, empty AAD, optional
16-byte `twox_128(pk)` prefix. The chain-side `unshield` mirrors it (`pallets/shield/src/lib.rs:L653-677`).

Docs state the SDK/CLI refuse to shield hotkey-signed extrinsics as policy
(`docs:concepts/advanced.mdx:L146-150`); I found **no** such check in `executor.py` (verified)
and could not locate it in the CLI code I read — treat as unverified docs drift. The CLI does
downgrade shielding when free TAO cannot cover the carrier fee because `fees_in_alpha` cannot
route through `MevShield.submit_encrypted` (`py:cli/context.py:L1047-1080`); the CLI shield
precedence is required → `--mev-shield/--no-mev-shield` → config `mev_shield` → intent default
(`py:cli/context.py:L260-292`).

### 6.6 Proxy / Sudo / Multisig / Batch composition

- **Proxy**: `Proxy.proxy(real=proxy_for, force_proxy_type, call=inner)` wrapped by the executor
  (§4.2); dispatch origin becomes `proxy_for` for preflights; the delegate pays the fee. Allowed
  call groups per proxy type are runtime-defined (`docs:concepts/advanced.mdx:L25-50`; notably a
  `Staking` proxy cannot wrap `Utility.batch_all`).
- **Sudo**: only when `intent.origin == "root"`; the two root intents declare `verify` reads.
- **Multisig** (transport path, `py:_transport/interface.py:L746-787`): `create_multisig_extrinsic`
  prices `max_weight` from payment info, reads `Multisig.Multisigs[account, call_hash]` to carry
  the existing `when` timepoint, computes `other_signatories` from the core-sorted signatory set
  minus the signer, composes `Multisig.as_multi` with the **full call** and signs with the
  cached nonce. `Multisig.approve` (`py:multisig.py:L206-247`) adds a best-effort funding
  preflight and a `shielded=True` variant that encrypts the same `as_multi`
  (`L239-269`). Account derivation: `blake2_256(b"modlpy/utilisuba" ‖ compact(len) ‖ sorted
  AccountIds ‖ u16 LE threshold)` (`core:codec/extrinsic.rs:L419-437`); golden vectors in
  `golden.json` `multisig` (3 cases).
- **Batch**: `Utility.batch_all` only; `batch`/`force_batch` are raw-only (§3.15).

---

## 7. Settings, networks, env vars, config

| Item | Value | Ref |
| --- | --- | --- |
| `SS58_FORMAT` | 42 (also `py:_transport/const.py:L2`) | `py:settings.py:L10` |
| `RAO_PER_TAO` | 10⁹ | `L15` |
| `U16_MAX` | 65535 | `L18` |
| `BLOCKTIME` | 12.0 (constant; timing math uses `Aura.SlotDuration`/1000 instead, 0.25 s on fast-blocks) | `L20`, `py:_substrate.py:L423-433`, `py:client.py:L71,L345-347` |
| `DEFAULT_ERA_PERIOD` / `MEV_SHIELD_ERA_PERIOD` | 128 / 8 | `L22`, `L29` |
| `GLOBAL_MAX_SUBNET_COUNT` | 4096 (mechanism storage index = `mechid*4096 + netuid`) | `L31`, `py:intents/weights.py:L164-167` |
| `NETWORKS` | `finney` → `wss://entrypoint-finney.opentensor.ai:443`; `test` → `wss://test.finney.opentensor.ai:443`; `archive` → `wss://archive.chain.opentensor.ai:443`; `devnet` → `wss://dev.chain.opentensor.ai:443`; `local` → `$BT_CHAIN_ENDPOINT` or `ws://127.0.0.1:9944` | `L33-39` |
| `DEFAULT_NETWORK` | `finney` | `L41` |
| `FINNEY_GENESIS_HASH` | `0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03` | `L45` |
| `FALLBACK_ENDPOINTS` | finney: `wss://lite.chain.opentensor.ai:443`, `wss://lite.sub.latent.to:443`; archive: `wss://archive.sub.latent.to:443` | `L56-64` |
| `ARCHIVE_ENDPOINTS` | finney: `wss://archive.chain.opentensor.ai:443`, `wss://archive.sub.latent.to:443` | `L69-77` |
| `resolve_endpoint` | raw `ws(s)://` accepted directly; unknown names raise | `L93-107` |
| Docs/explorer URLs | `https://www.bittensor.com/docs` (`/tx/<op>`, `/errors/<code>`, `/errors/chain/<Name>`, `/query/<name>`); extrinsics `https://taomarketcap.com/extrinsics/{block}-{idx:04d}` (finney/archive only) | `L114-167` |
| Raw metadata disk cache dir | `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache` | `py:_transport/runtime.py:L57-61` |
| CLI config | `$BTCLI_CONFIG` or `~/.bittensor/btcli.json`; keys: `network`, `wallet`, `wallet_hotkey`, `wallet_path`, `json`, `quiet`, `signer_address`, `extension_browser`, `fallback_endpoints`, `archive_endpoints` (comma lists; `"none"` pins), `retry_forever`, `proxy_for`, `mev_shield` | `py:config.py:L12-59` |
| Other CLI files | `BTCLI_PROXIES_PATH`, `BTCLI_ADDRESSES_PATH`, `BTCLI_MULTISIGS_PATH`, `BTCLI_MULTISIG_CACHE`, `BTCLI_SUBNET_NAMES_CACHE`, `BTCLI_TOKEN_SYMBOLS_CACHE` (defaults under `~/.bittensor/`) | `py:config.py:L138-416` |
| SDK env vars (docs) | `BT_NETWORK`, `BT_CHAIN_ENDPOINT` (overrides `local`), `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`, `BT_WALLET_PASSWORD`, `BT_WALLET_PASSWORD_FILE` | `docs:migration.mdx:L162-178` |
| Token symbols cache | per-network, 7-day TTL, loaded at `Client.connect` (display only) | `py:config.py:L27`, `py:client.py:L613-633` |

Client construction defaults (`py:client.py:L139-182`): fallback/archive pools default to the
network's public pools; pass `[]` to pin; `retry_forever=False`; any `Substrate` implementation
may be injected for tests.

---

## 8. What a Rust port must replicate bit-for-bit vs what is Python product policy

### 8.1 Chain-defined (must match exactly)

1. **Signature payload** = `call ‖ extra ‖ additional`, both sections encoded by iterating the
   runtime's transaction extensions **in metadata order**, looking up a known encoder per
   identifier, and failing closed if an unknown extension contributes non-zero-sized data
   (`core:codec/extrinsic.rs:L38-118`). For finney spec 477 (V16 dump, `meta:finney.json`
   `extrinsic.transaction_extensions`, ids verified) the order is:
   `CheckNonZeroSender(ty 399 ⟨⟩, implicit 48 ⟨⟩)` → `CheckSpecVersion(400 ⟨⟩, 4 u32)` →
   `CheckTxVersion(401 ⟨⟩, 4 u32)` → `CheckGenesis(402 ⟨⟩, 14 H256)` →
   `CheckMortality(403 = Era, 14 H256)` → `CheckNonce(405 = Compact<u32>, 48)` →
   `CheckWeight(406 ⟨⟩, 48)` → `ChargeTransactionPayment(409 = Compact<u128>, 48)` →
   `SudoTransactionExtension(410 ⟨⟩, 48)` → `CheckShieldedTxValidity(411 ⟨⟩, 48)` →
   `SubtensorTransactionExtension(412 ⟨⟩, 48)` → `DrandPriority(413 ⟨⟩, 48)` →
   `CheckMetadataHash(414 = {mode: Mode{Disabled=0, Enabled=1}}, 81 = Option<[u8;32]>)`.
   Hence `extra = era ‖ compact(nonce) ‖ compact(tip) ‖ mode_u8` and
   `additional = spec_version u32 ‖ transaction_version u32 ‖ genesis H256 ‖ era_block_hash H256
   ‖ Option<metadata_hash>` (None = `0x00` when mode Disabled). The runtime's tuple is in
   `runtime/src/lib.rs:L1644-1664`.
2. **CheckMetadataHash mode**: `Disabled`/`0x00` unless a `MetadataVerifyingSigner` supplies the
   RFC-0078 digest; then mode `Enabled` and the 32-byte digest goes into `additional`
   (`core:codec/extrinsic.rs:L181-207`). Digest inputs: raw metadata bytes + `ExtraInfo{spec_version,
   spec_name="node-subtensor", base58_prefix=42, decimals=9, token_symbol="TAO"}`
   (`core:digest/mod.rs:L34-52`, test constants `L152-160`); the runtime bakes the same via
   `enable_metadata_hash("TAO", 9)` (`sdk/bittensor-core-spec.md:L449-454`).
3. **Era**: encoding (`core:codec/encode.rs:L445-485`, `L591-593`) and `era_birth`
   (`core:codec/extrinsic.rs:L407-412`); immortal era anchors at genesis.
4. **Payload hashing**: blake2b-256 when the unhashed payload exceeds 256 bytes (`L249`).
5. **Signed extrinsic v4 layout**: `compact(len) ‖ 0x84 ‖ MultiAddress::Id(pubkey) ‖
   MultiSignature{variant byte = crypto_type: 0 ed25519, 1 sr25519, 2 ecdsa} ‖ signature ‖
   extra ‖ call`; hash = blake2b-256 of the whole length-prefixed blob (`L260-316`). The core
   rejects any extrinsic version other than 4 (`L268-273`); finney V16 metadata declares
   `versions [4, 5]` and `transaction_extensions_by_version {"0": [0..12]}` (jq on
   `meta:finney.json`) — a v5 "General" extrinsic path is **not** implemented by the SDK.
6. **Nonce**: `account_nextIndex` semantics (`system_accountNextIndex` in the Rust client);
   shielded carrier at `n`, inner at `n+1` (§6.5).
7. **MEV shield**: ciphertext layout, `twox_128` key-hash prefix, raw shared secret as AEAD key,
   24-byte nonce, empty AAD, ≤ 8192 bytes, era ≤ 8 blocks (§6.5).
8. **Multisig**: account id derivation and signatory ordering (`core:codec/extrinsic.rs:L419-437`);
   `other_signatories` sorted by raw key bytes and excluding the signer.
9. **Custom transaction error codes** (`common/src/transaction_error.rs`), the coldkey-pays-fee
   call list (`runtime/src/fee_filters.rs:L14-35`), and the alpha-fee-eligible call set
   (`pallets/transaction-fee/src/lib.rs:L340-420`: `remove_stake`, `remove_stake_limit`,
   `remove_stake_full_limit`, `unstake_all`, `unstake_all_alpha`, `swap_hotkey`, `swap_hotkey_v2`).
10. **Receipt semantics**: extrinsic located by blake2b of raw bytes; events filtered by
    `extrinsic_idx`; nested wrapper `Err` is a failure; fee from `TransactionFeePaid.actual_fee`.
11. **Weights conformance**: u16 max-upscale/quantization and `max_weight_limit` clipping must
    match what the chain accepts (`py:intents/weights.py:L54-114`); timelock commit v2 bytes
    from `bittensor_core.get_encrypted_commit_v2` (`sdk/bittensor-core-py/src/timelock.rs:L36-38`).
12. **ss58**: format 42 everywhere; addresses are validated to carry that prefix
    (`py:_transport/codec.py:L74-83`).

### 8.2 Python product policy (free to redesign)

- Era defaults 128 (clear) / 8 (shield, but ≤ 8 is chain-mandated); finalized-head anchoring of
  mortal eras.
- `wait_for_inclusion=True`, `wait_for_finalization=True` defaults for `execute`; `False` for
  shielded finalization.
- Retry only on the three transient substrings; one-block sleep; `retries=0` default.
- Slippage protection on by default with 5% tolerance; fill-or-kill (`allow_partial=False`).
- `"all"` headroom of 500 000 rao over the existential deposit.
- `Policy` fields and fail-closed fee cap; raw calls refused by default.
- `ErrorCode` taxonomy, remediation/explanation prose, per-name overrides, docs URLs, explorer
  URLs.
- MEV shield "default on" for stake-trading intents is a **CLI** behaviour; the SDK's `execute`
  only redirects `mev_shield_required` intents (`burned_register`, `add_collateral`).
- Subnet-registration completion wait and its progress callbacks.
- Multisig deposit/fee preflight, `_FEE_ALLOWANCE_RAO = 5_000_000` (`py:multisig.py:L162`).
- Root-claim fee model mirror (`py:intents/_root_claim_fee.py`) — a preview; the chain charges
  what it charges.
- Connection resilience knobs (fallback/archive pools, 60 s silence watchdog, 5 retries).
- Metadata disk cache format and location.

---

## 9. The native Rust client already in `bittensor-core` (host feature)

What exists **[verified]**:

- `core:client.rs`: `Client::connect(endpoint)` maps `ws(s)://` → `http(s)://` and uses blocking
  `reqwest` JSON-RPC (`L148-179`, `L1269-1308`); bootstraps `system_properties.ss58Format`,
  `state_getRuntimeVersion`, V15 metadata (`Metadata_metadata_at_version` with
  `state_getMetadata` fallback, `L1234-1267`), genesis hash; `refresh_runtime()` re-parses on
  spec/tx-version change (`L231-252`); reads: `query`, `query_batch`, `query_map` (paged 1000),
  `runtime_call`, `constant`, `decode_call/scale`, typed helpers (`subnets`, `metagraph`,
  `neurons`, `subnet_hyperparameters`, `stake_rao`, `quote_stake`) and a `read_catalog()` of 20
  names; `sign_extrinsic` (era anchored at best head, tip 0, no metadata hash, `L507-549`);
  `estimate_fee`/`estimate_weight` via the deprecated `payment_queryInfo` RPC (`L551-576`);
  `submit` → `author_submitExtrinsic` then **poll** `chain_getBlock` per height comparing raw
  extrinsic hex, reorg-aware finalization, 120 s receipt timeout (`L578-618`, `L897-991`);
  `decode_outcome` with nested Proxy/Sudo/Multisig `Err` detection (`L993-1069`);
  `submit_shielded` (`L622-654`); `BlockStream` polling iterator (`L1115-1158`); `Snapshot`.
- `core:transaction.rs`: `IntentCall` with private security semantics (`Spend`, netuids,
  `affects_all_subnets`, `raw`) that constructors attach and callers can only tighten
  (`L94-248`); ~20 trusted constructors (`L250-826`) + `batch` (`L834-880`); `Policy`
  (`max_fee_rao`, `max_spend_rao`, `allowed_netuids`, `allow_raw_calls`; raw calls always
  unbounded and all-subnet, `L929-976`); `Plan`; `Executor::{plan, plan_with_proxy, execute,
  execute_with, submit_shielded}` (`L998-1116`).
- Semantic error codes are derived by `snake_case(name)` plus a handful of aliases
  (`core:client.rs:L1396-1411`) — **not** the Python 18-code taxonomy.

Gaps/divergences relevant to rtao **[verified unless noted]**:

- Blocking, polling, HTTP-only; no subscriptions; no nonce cache; no reconnect/fallback pool.
- `DEFAULT_ERA_PERIOD = 64` (`L32`) vs Python 128; `Executor::execute_with` passes `Some(64)`
  (`transaction.rs:L1098`).
- `submit_shielded` signs **both** inner and carrier with the 64-block era (`L633`, `L640-646`),
  which `check_mortality.rs:L94-100` rejects as `Stale` for `submit_encrypted` — **[inferred bug]**;
  the e2e test only asserts `!result.message.is_empty()` (`sdk/bittensor-core/tests/e2e.rs:L336-339`).
  It also does not scan for the inner extrinsic's receipt (the carrier's success is returned).
- No proxy/multisig preflights, no `wrap_call` adapter concept, no intent preflight reads
  (slippage limits, `"all"` resolution, weights conformance), no `register_subnet` completion.
- The core spec explicitly lists "No Rust websocket/RPC transport" and "No Rust codegen or
  intent layer" as non-goals (`sdk/bittensor-core-spec.md:L85-97`); the client/executor were added
  later (`core:lib.rs:L1-7`), and the crate is `publish = false` (`sdk/bittensor-core/Cargo.toml:L5`).
- Dependencies pinned to the runtime workspace (`sp-core`, `frame-metadata`, `scale-info`,
  `tle`/`w3f-bls` for timelock, `ml-kem 0.2.1`, `chacha20poly1305 0.10`, `twox-hash 2.1`,
  `merkleized-metadata 0.5.1`) (`Cargo.toml:L20-78`); a wasm32 seam is maintained without the
  `host` feature (`[features]` at `L105-126`).

---

## 10. Quirks and gotchas (numbered, for the design doc)

1. `ExtrinsicResult.success=True` with `message="Submitted (not waiting)."` when neither
   inclusion nor finalization was awaited — pool acceptance only (`py:_substrate.py:L776-777`).
2. Mortal eras are anchored at the **finalized** head, not the best head
   (`py:_transport/interface.py:L493-495`); a Rust port using the best head produces different
   (still valid) bytes — fine unless you reuse Python golden vectors with `current` unspecified.
3. Fee estimation signs with an immortal era and a zeroed signature; `partial_fee` is read from a
   `state_call` result, not the `payment_queryInfo` RPC (`interface.py:L714-731`).
4. `max_fee_tao` + missing estimate: a **warning** in `plan` but a **PolicyError** in
   `submit_shielded` (`py:executor.py:L942-953`).
5. The executor's transient-retry test is a substring match on the error text
   (`L52-56`); "stale" also matches the shield-era rejection, so a mis-configured shield era
   would be retried pointlessly.
6. `execute_proxy_announced` builds the inner intent against the **signing** wallet, so defaults
   like "wallet hotkey" resolve to the delegate's keys, not the real account's
   (`py:intents/proxy.py:L308`).
7. `move_swap_stake` deliberately swaps `amount − 1 rao` (share-pool floor); `remove_stake`
   claim-all passes `u64::MAX` to let the runtime cap (`staking.py:L184-205`, `L296-313`).
8. Weights preflight reads `LastUpdate` at the **mechanism storage index** `mechid*4096+netuid`
   but computes reveal rounds with the real `netuid` (`weights.py:L164-167`, `L288-290`).
9. `SetWeights` client-side `ChainError`s carry the same `ErrorCode`s the chain would produce, so
   callers cannot distinguish "caught locally" from "rejected on chain" by code alone
   (`weights.py:L154-162`).
10. `associate_hotkey` is silently a no-op when the hotkey is already owned (docstring,
    `association.py:L21-24`).
11. `claim_root.subnets` is sent but ignored on chain (`registration.py:L461-467`).
12. The `mev_shield_default` flag affects only the CLI; `client.execute(AddStake(...))` submits
    in the clear (`py:executor.py:L812-824`, `py:cli/context.py:L1012-1016`).
13. Shielded carriers cannot pay fees in alpha (`MevShield.submit_encrypted` has no netuid), so
    α-only coldkeys must unstake unshielded (`py:cli/context.py:L1047-1052`; `Payment`
    remediation `py:result.py:L337-342`).
14. `register_subnet` "success" may only mean "queued"; the executor's follow-up may run for a
    long time with no timeout by default (`L557-568`).
15. The Python transport never resubmits an in-flight extrinsic after a socket drop; the caller
    gets a "fate unknown" error and must check chain state (`rpc.py:L686-711`).
16. `attach_signature` refuses if `spec_version` changed since `prepare_call`
    (`interface.py:L617-623`); a port should also refuse on `transaction_version` change (the
    Rust client's `refresh_runtime` checks both, `core:client.rs:L234-236`).
17. V16 metadata (what subxt fetches today) is **not** parsed by bittensor-core; it requests V15
    (`core:runtime/mod.rs:L190-194`, `core:client.rs:L1238`). A subxt-based port will see
    `transaction_extensions_by_version` and `versions [4,5]`; sign v4 to stay byte-compatible.
18. Docs vs code: `transactions.mdx:L19` says "70+" intents (there are 83);
    `advanced.mdx:L146-150` describes a hotkey-shield refusal I could not find in code.
19. The Python `BLOCKTIME = 12.0` constant exists but timing uses `Aura.SlotDuration`
    (`py:_substrate.py:L423-433`); fast-blocks localnets are 0.25 s.
20. `Policy.allowed_netuids` is checked against `touches_netuids()`, which for intents without a
    `netuid` field (e.g. `transfer`, `claim_root`) is empty — they pass any allowlist unless
    `affects_all_subnets()` is true.

---

## 11. Test assets a port can reuse

- `sdk/python/tests/fixtures/golden.json` (1.5 MB; keys `block, calls, constants, endpoint,
  events, extrinsics, metadata, multisig, network, payment_info, query_maps, receipt,
  runtime_calls, signature_payloads, ss58, storage_keys, storage_values`): localnet spec 461,
  genesis `0x1dc00e76…`, `signature_payloads` (2) and `extrinsics` (2, with `signature_version`,
  `extrinsic_hex`, `extrinsic_hash`), `multisig` (3 derivations), a full `receipt` with
  `triggered_events`, raw V15 `metadata.v15_hex`. Used by `core:digest/mod.rs:L163-183` and the
  payload/assembly golden tests.
- `sdk/bittensor-core/fixtures/ledger_proof_vector.json` (RFC-0078 proof blob, cross-checked
  against polkadot-api) (`core:digest/mod.rs:L236-275`).
- `sdk/python/tests/fixtures/shape_corpus/` (decoded-value shape contract,
  `sdk/bittensor-core-spec.md:L105-127`).
- `meta:finney.scale/json` (V16, spec 477) and `meta:explore/*` dumps for extension/type checks.
- The Python fake node harness `sdk/python/tests/harness/fake_node.py` (referenced by the spec,
  `L336`, `L432-433`) for transport-level tests.

---

## 12. Open questions for the architects

1. Should rtao reuse `bittensor-core` as a git dependency (it is `publish = false`, pinned to the
   runtime workspace's `sp-core`/`frame-metadata`) or re-implement the chain-defined pieces on
   subxt/scale-value and validate against the golden vectors? Reuse gives byte-equality for free
   but drags in the full workspace lock; re-implementation needs the vectors in CI.
2. Extrinsic version: finney advertises `[4, 5]`. Sign v4 (SDK-compatible, golden-vector
   compatible) or adopt v5 General extrinsics via subxt? (Nothing on the Python side emits v5.)
3. Metadata version: subxt-native V16 vs the SDK's V15 fetch; both carry the same extension
   order, but `transaction_extensions_by_version` only exists in V16.
4. Is the 64-block shield era in `core:client.rs` a real bug (needs a localnet run to confirm)?
   If so, upstream a fix; rtao should use ≤ 8 regardless.
5. How much of the Python preflight machinery (slippage limit derivation, `"all"` resolution,
   weights conformance, root-claim fee model, subnet-registration follow-up) is in scope for a
   subnet-owner/validator/miner toolkit vs an exchange/wallet toolkit? The validator hot path
   needs only `set_weights` (+ timelock), `serve_axon`, registration, and stake reads.
6. Error taxonomy: adopt the 18 `ErrorCode`s verbatim (and keep `error_map.py` in sync via a
   generated table) or expose the raw `(pallet, name)` + a smaller Rust enum? The CI gate design
   (`check.py --names`) is worth copying either way.
