# Skeptic verification: chain-correctness findings CC-01 … CC-12

Every claim was re-checked against `UP/` at `850f2755`, the pinned polkadot-sdk checkout
(`~/.cargo/git/checkouts/polkadot-sdk-778d4d407c7beeae/cacb431`), finney V16 metadata (`meta/finney.json`, spec 477) and
the plan text. Two read-only live measurements against `https://entrypoint-finney.opentensor.ai:443` (2026-10-13) are
used below:

- finality lag (best − finalized), 12 samples over about 2 minutes: 2, 2, 2, 2, 3, 2, 2, 2, 2, 2, 2, 3 blocks
  (`scratchpad/verify-cc/lag.py`, output `lag.txt`);
- `SubtensorModule.Tempo` for all 129 netuids: 124 × 360, and one each of 99 (netuid 1), 100 (netuid 0), 1440, 1800,
  7200 (`scratchpad/verify-cc/tempo.py`).

| ID | Verdict | Verified severity |
|---|---|---|
| CC-01 | partially confirmed | minor (reported major) |
| CC-02 | partially confirmed | major |
| CC-03 | confirmed | major |
| CC-04 | partially confirmed | minor |
| CC-05 | partially confirmed | minor |
| CC-06 | partially confirmed | minor |
| CC-07 | confirmed | minor |
| CC-08 | confirmed | minor |
| CC-09 | partially confirmed | minor |
| CC-10 | confirmed | minor |
| CC-11 | confirmed | minor |
| CC-12 | partially confirmed | minor |

---

## CC-01: partially confirmed (minor, not major)

**Verified.** §9.3 never names the head for the step-2 fetch. With "the scheduler runs per finalized head" (l.2793),
`HeadPolicy::Finalized /* neuron runtimes */` (l.1000) and `fetch` pinning at the view's head (§3.6), `current_block` is
the finalized head F. The core assumes inclusion at `current_block + 1` "relative to the chain head queried by the SDK"
(`CORE/timelock/constants.rs:41-44`, `epoch_schedule.rs:116-117`) and converts `target − current_block` into seconds
from the wall clock (`CORE/timelock/mod.rs:169-184`). Python reads at the best head:
`PY/intents/weights.py:283-284` → `PY/_substrate.py:417-418` → `PY/_transport/interface.py:218-226`
(`chain_getHeader(None)`). With the measured lag of 2-3 blocks, every rtao round is 8-12 rounds later than Python's, so
step 8's "differ from Python's only in that edge case" is false.

**Overstated.**

1. A later round does not consume the +3-block `SECURITY_BLOCK_OFFSET`. That offset protects against a round published
   before the reveal epoch (system clock behind). A later round widens that protection and only delays the reveal by L
   blocks inside a reveal epoch of `tempo` blocks, because the pallet retries every block
   (`reveal_commits.rs:71-87`).
2. An epoch slip needs a boundary in (F+1, inclusion]. The default cadence fires 2 to 2 + tempo/10 blocks after an
   epoch start, so this cannot happen at L = 2-3.
3. "Every commit … silent loss" during a stall of about a tempo is wrong:
   - Era 128 anchored at F makes inclusion impossible once B + 1 ≥ F + 128, so no subnet with tempo ≥ 360 can lose a
     commit this way.
   - As the lag approaches a tempo, the commit also slips one epoch, which re-aligns the round with the new reveal
     epoch.
   - A loss needs L in a band a few blocks wide just under tempo − 3, and only on tempos below about 130. On finney that
     is netuid 1 alone (tempo 99).

**Corrected fix.**

- **architecture.md §9.3 step 2**, append: "The fetch is pinned at the best head's hash at the moment the scheduler
  fires (`HeadPolicy::Best` for this read only; finalized heads still trigger the scheduler and anchor the era).
  `current_block` in step 7 is that best block's number. The predictor assumes the commit lands in
  `current_block + 1` (`CORE/timelock/constants.rs:41-44`, `CORE/timelock/epoch_schedule.rs:116-117`), and Python
  reads the same items at the best head (`PY/intents/weights.py:283-306` → `PY/_substrate.py:417-418` →
  `PY/_transport/interface.py:218-226`). Pinning at the finalized head would make every round 4 × (best − finalized)
  rounds later than Python's."
- **Step 12**: replace "During the predicted reveal epoch" with "During the commit's actual reveal epoch (after
  inclusion, read `TimelockedWeightCommits[netuid_index][e]` at the inclusion block for e ∈ {predicted, predicted + 1};
  count predicted ≠ actual in `rtao_weights_commit_epoch_slip_total`)".
- **PLAN.md §7.2**: "reads 17 storage items in one RPC at one pinned block" → "reads 17 storage items in one RPC at the
  best head's hash".
- **Tests**:
  - a FakeChain case with finality lag 3 that asserts the predictor receives the best block's number and state;
  - a FakeChain case whose inclusion slips past an epoch boundary, asserting the watcher follows the actual epoch and
    raises no miss.
- **No step-10 change.**

## CC-02: partially confirmed (major)

**Verified.** Only owner hyperparameters (and `SetSNOwnerHotkey`) use `LastRateLimitedBlock`. Version key, tempo,
mechanism count, emission split and trims use `TransactionKeyLastBlock[(who, netuid, code)]`
(`utils/rate_limiting.rs:89-131`; codes at :134-152; limits at :38-53). `who` is the signing owner coldkey
(`utils/misc.rs:38-53, 94-105`; `coinbase/tempo_control.rs:16-45`). Tempo's limit is `MIN_TEMPO` = 360. The 7,200 and
216,000 limits are `prod_or_fast!` type values (`lib.rs:1128-1132, 3567-3577`) with 0 hits in `finney.json`.

**Errors in the fix.**

- **(a) Root and the freeze window.** "Root bypasses … the freeze window" is false. Every admin-utils owner setter calls
  `ensure_admin_window_open` after `ensure_sn_owner_or_root_with_limits`, for root too (admin-utils `lib.rs:454` and
  44 other sites), and so does `do_set_tempo` (`tempo_control.rs:28`). Only `activity_cutoff_factor` exempts root from
  both (`tempo_control.rs:64-72`).
- **(b) Owner-cut toggles.** `sudo_set_owner_cut_enabled` and `sudo_set_owner_cut_auto_lock_enabled` have no rate limit
  (admin-utils `lib.rs:2511-2555`), but the fix's generic row would apply one.
- **(c) Shared key.** `alpha_low` and `alpha_high` share the `AlphaValues` key.
- **(d) Tempo limit source.** The tempo limit is the metadata constant `MinTempo` (`[104,1]` = 360).
- **(e) Overstated localnet scenario.** On a fresh subnet `last == 0` passes, so scenarios 3 and 10 would not wait.

**Corrected fix.** Replace the §9.7 bullet with:

- Per-setter rate limits apply only to the owner origin. `owner` is the signing owner coldkey, and `last == 0` passes
  (`rate_limiting.rs:64-67`).

  | Setter (tx-type code) | Last block | Limit |
  |---|---|---|
  | owner hyperparameters not listed below, keyed by the setter's `Hyperparameter` variant (`alpha_low`/`alpha_high` share `AlphaValues`; `activity_cutoff_factor` is `ActivityCutoffFactorMilli`) | `LastRateLimitedBlock[RateLimitKey::OwnerHyperparamUpdate(netuid, h)]` | `OwnerHyperparamRateLimit` (default 2) × `Tempo[netuid]` |
  | `sudo_set_owner_cut_enabled`, `sudo_set_owner_cut_auto_lock_enabled` | none | none (freeze window only) |
  | `sudo_set_weights_version_key` (4) | `TransactionKeyLastBlock[(owner, netuid, 4)]` | `WeightsVersionKeyRateLimit` (default 5) × `Tempo[netuid]` |
  | `sudo_set_tempo` (11) | `TransactionKeyLastBlock[(owner, netuid, 11)]` | `MinTempo` = 360 blocks |
  | `sudo_set_mechanism_count` (7), `sudo_set_mechanism_emission_split` (8) | `TransactionKeyLastBlock[(owner, netuid, 7 or 8)]` | 7,200 blocks (fast-blocks: 1) |
  | `sudo_trim_to_max_allowed_uids`, and `sudo_trim_null_uids_batch` unless it continues the stored `NullPruningTarget` (9) | `TransactionKeyLastBlock[(owner, netuid, 9)]` | 216,000 blocks (fast-blocks: 1) |

- Root bypasses the rate limits but not the freeze window. The exception is `activity_cutoff_factor`, where root
  bypasses both (`tempo_control.rs:28, 64-72`).
- The 7,200- and 216,000-block limits are `#[pallet::type_value]`s absent from metadata (`lib.rs:1128-1132,
  3567-3577`). The chain-source `syn` layer extracts the `prod_or_fast!` pairs into `rt::rate_limits`. The planner uses
  the fast value when `Aura.SlotDuration < 12_000` (`common/src/lib.rs:183`).
- A `TxRateLimitExceeded` dispatch error re-plans once.
- Tests:
  - one FakeChain planner test per row, including `alpha_low` then `alpha_high` inside one window, and owner-cut toggles
    with no limit;
  - scenario 10 changes the mechanism count twice on one subnet and asserts that `earliest_block` equals the first
    accepted block.

## CC-03: confirmed (major)

Pinned polkadot-sdk `substrate/client/transaction-pool/api/src/lib.rs`:

- :104 says "`Retracted` transactions might be included in some next block";
- :113-119 lists the last events, and `Retracted` is not among them;
- :143-147 defines `FinalityTimeout(BlockHash)` (the block did not reach finality within 512 blocks, :106-109);
- :162-172 `is_final` excludes `Retracted`.

Python raises on all five statuses (`PY/_transport/extrinsics.py:352, 364-369`). The plan copies this in §3.9.3 step 4,
and §3.9.6 reconciles only `FateUnknown`. The default `Wait::Finalization` (l.1231) is therefore exposed to false
failures on forks. The reviewer's fix and D-024 are sound.

## CC-04: partially confirmed (minor)

**Verified.** `approve_as_multi` at the threshold only records `MultisigApproval`, and dispatch requires the call
(polkadot-sdk `substrate/frame/multisig/src/lib.rs:442, 671-715`). Python's adapter and `multisig_execute` always send
`as_multi` with the full call (`PY/intents/multisig.py:227-245, 317-330`; `PY/multisig.py:249-269`). The §3.12 bullet,
applied to `compose::as_multi` or the adapter, would never execute the final round.

**Fix error.** Refusing a hash-only approval at the threshold diverges from Python. `multisig_approve` sends
`approve_as_multi` whenever a timepoint is given (`PY/intents/multisig.py:438-462`). The chain accepts that approval,
and any signatory can execute later with `as_multi` and the call (:656-699).

**Corrected fix.**

- Replace the bullet with: "the opening approval uses `as_multi` with the full call and `maybe_timepoint = None`.
  Intermediate approvals sent through `multisig_approve` use `approve_as_multi` with the call hash and the opening
  timepoint (`PY/intents/multisig.py:438-462`). `multisig_execute` and the saved-multisig adapter always use
  `as_multi` with the full call and the pending timepoint (`PY/intents/multisig.py:227-245, 317-330`;
  `PY/multisig.py:249-269`). pallet_multisig dispatches only when the call is supplied and approvals reach the
  threshold; a hash-only approval at the threshold only records `MultisigApproval` (polkadot-sdk
  `frame/multisig/src/lib.rs:442, 671-715`)."
- Keep `compose::as_multi` full-call only, and add `compose::approve_as_multi(v, signer, signatories, threshold,
  call_hash, timepoint, est)` for the `multisig_approve` port.
- When a hash-only approval would reach the threshold, `plan` warns ("this approval cannot execute the call; use
  multisig_execute"). It does not refuse, because Python sends it.

## CC-05: partially confirmed (minor)

**Verified.** Python takes n from the cache (`PY/_transport/interface.py:735-736` → `extrinsics.py:69-77`), then
`pin(n+1)` and `pin(n)` (`PY/executor.py:1037-1052`; `interface.py:519-522`). After success the cache holds n, so the
next submission signs n+1 and gets `Stale` once the inner has landed (`runtime/src/check_nonce.rs:145-146`). The plan
already avoids `pin` (it uses `lease_pair`, §3.11 step 2), but it leaves the post-state and the settle rules unstated,
and the divergence is unregistered.

**Incomplete.** Under `Wait::Pool` there is no inner resolution. Clearing the cache would make `account_nextIndex`, which
cannot see the encrypted inner's nonce, return n+1 and collide with the inner.

**Corrected fix.** Append to §3.11 step 2:

- "`lease_pair` reserves n and n+1 under the account's fetch lock and leaves `next = n + 2`. rtao never calls `pin` in
  the shield path."
- The carrier lease settles on carrier inclusion, and the inner lease settles on inner inclusion (step 6).
- Under `Wait::Pool`, both leases settle on carrier pool acceptance, because `account_nextIndex` cannot see the
  encrypted inner's nonce.
- An inner lease dropped unsettled (the carrier is included but the inner never lands) clears the cache, so the next
  lease re-asks `account_nextIndex`.
- Python leaves its cache at n (`PY/executor.py:1037-1052`, `PY/_transport/extrinsics.py:79-81`), so its next
  submission reuses n+1 and is rejected as `Stale` (`UP/runtime/src/check_nonce.rs:145-146`).

Register D-026 (improved) with the tests `shielded_then_plain_uses_n_plus_2`,
`shield_inner_missing_clears_nonce_cache` and `shield_wait_pool_keeps_n_plus_2`.

## CC-06: partially confirmed (minor)

**Verified.** The shield era is anchored at the finalized head F (`PY/_transport/interface.py:485-498`). Validity is
`birth(current) == F`, so the extrinsic is valid only below F + p (`era.rs:86-99`; frame_system
`check_mortality.rs:64-72`). The plan says only that "the submission still proceeds" (l.1295-1297).

**Off by one.** The carrier and the inner are applied together in block B + 2:

- `NextKey` read at B is the key of the author of B + 2 (`pallets/shield/README.md:15, 34`;
  `NextKeyExpiresAt = now + 3`, `lib.rs:324`);
- the proposer includes only txs encrypted to `PendingKey` at its parent, and pushes the carrier and the inner together
  (`lib.rs:616-620`; polkadot-sdk `basic_authorship.rs:464-545`).

Validity therefore needs B + 2 < F + p, so L ≤ p − 3:

- P8 fails from L = 6: the pool accepts the carrier, which can never be included; from L = 7 the pool rejects it.
- P4 fails from L = 2, which is finney's usual measured lag.
- The fix's P4 rule (L ≤ 1) is right; its general rule (B + 1 ≥ F + p) is not.

**Corrected fix.** Add to §3.11, before step 2:

- Read the best head B (the same head used for the step-1 `NextKey` read) and the finalized head F.
- The carrier and the inner are applied together in block B + 2 by the author whose key `NextKey` holds
  (`pallets/shield/README.md:15, 34`; `pallets/shield/src/lib.rs:324, 616-620`; polkadot-sdk
  `basic_authorship.rs:464-545`).
- An era of period p anchored at F is valid only below F + p (`era.rs:86-99`).
- If B + 2 ≥ F + p (that is, B − F ≥ p − 2), refuse with `TxError::ShieldEraExpired { period, finality_lag: B − F }`.
- `P8` therefore tolerates a lag of at most 5 and `P4` at most 1.
- Finney's measured lag is 2-3 blocks, so `P4` is refused in normal operation and the CLI never offers it.

Register D-025 (stricter: Python submits and fails), with FakeChain tests for `P8` at lags 5 (accepted) and 6 (refused)
and for `P4` at lags 1 and 2. In §3.9.2, append "(shield periods: §3.11's refusal rule)".

## CC-07: confirmed (minor)

`execute` redirects `mev_shield_required` intents to `submit_shielded` before the retry loop
(`PY/executor.py:812-824` against :832-844). `submit_shielded` has no `retries` parameter (:897-909), and the CLI passes
none (`PY/cli/context.py:1491-1502`; no `retries` anywhere under `PY/cli`). [R] A1 §10 item 5 is therefore wrong, and
D-011 registers Python behavior that does not exist.

With `ShieldPeriod`, the only remaining carrier `Stale` is `CheckNonce` (`check_nonce.rs:145-146`). The other
`InvalidTransaction::Stale` sites are the shield era cap and drand unsigned txs. The fix is right. A suggestion: mark
D-011 "withdrawn" rather than deleting it, so that later D-numbers stay stable.

## CC-08: confirmed (minor)

**(a) Null permit.** On Null subnets, a missing permit fails at the pool:

- `CheckWeights::ensure_min_stake` (`guards/check_weights.rs:97-104`) runs at `validate`
  (`extensions/subtensor.rs:123-125, 367-377`) and maps to `BadRequest` = 255;
- it fails again at dispatch (`subnets/weights.rs:17-26`);
- Python refuses it client-side (`PY/intents/weights.py:184-190`).

**(b) Reveal-time checks.** The pallet's reveal re-runs `internal_set_weights`, which checks registration, stake,
version key, permit, uid validity and length (`subnets/weights.rs:1010-1053`; the reviewer's line numbers are off by one).
It drops failures with `log::trace` and no event (`reveal_commits.rs:188-202`). The watcher's recorded data cannot
attribute these misses, contrary to step 9's "any future miss is attributable". The fix is sound.

## CC-09: partially confirmed (minor)

**Verified.** finney call 132 is `add_stake_burn(hotkey, netuid, amount: TaoBalance, limit: Option<TaoBalance>)`.
`limit` reaches `do_add_stake_limit` as its limit price (`staking/recycle_alpha.rs:131-150`). Python fills it with rao
per alpha (`PY/intents/governance.py:118-153`), and `CAT` `stake_burn` wraps it. The override row and D-022 miss it.

**Not implementable as stated.** The proposed CI check depends on "filled by the wrapping Python body from a price".

**Corrected fix.**

- Add the §3.7 row: "`limit` of `add_stake_burn` (132) | `Option<TaoBalance>` | `Option<PriceRao>` | passed as the
  limit price to `do_add_stake_limit` (`UP/pallets/subtensor/src/staking/recycle_alpha.rs:131-150`); Python sends rao
  per alpha (`PY/intents/governance.py:118-153`)".
- D-022 (Appendix C and PLAN.md §8.4): "limit-price arguments (`limit_price` of 88, 89, 90, 103, 144, 149; `limit` of
  `add_stake_burn`, 132)".
- PD-9: "all six calls" → "all seven calls".
- Appendix F #25: add "132 (`limit: Option<TaoBalance>`)".
- Gate: `xtask regen` fails when an argument in a covered pallet is typed `TaoBalance`/`AlphaBalance` (or an `Option`
  of either), has a name containing `limit` or `price`, and has neither a price override nor an explicit `amount` entry
  in `codegen/overrides.toml`. At 477 this matches exactly the seven SubtensorModule price arguments, plus Contracts'
  `storage_deposit_limit`, which needs `amount` entries.

## CC-10: confirmed (minor)

`jq` over `finney.json`: `Owner`, `SubnetOwner`, `SubnetOwnerHotkey` and `Keys` are `Default` entries with 32 zero
bytes. They are the only `Default` entries with an `AccountId32` value. Both rtao (§3.6) and Python
(`PY/_transport/storage.py:22-27`) decode `default_bytes` on a miss. `SubnetOwner` is read by the `register_subnet`
completion follower (`PY/executor.py:468-481`).

The localnet gate and the M1 `Localnet` it forces are unnecessary. When applying the fix, also edit:

- architecture §14 M1 ("D-001 localnet check" → offline metadata check);
- §11.4 scenario 15 (drop "which gates the `DefaultAccount` override");
- Appendix C D-001 (test column);
- PLAN.md §8.4 D-001 ("enabled after a localnet check");
- PLAN.md §12.3 (the "`DefaultAccount` only after D-001" item).

## CC-11: confirmed (minor)

The certificate is written only to `NeuronCertificates` and only via `try_from` (`subnets/serving.rs:71-76`). The axon
fields are written separately (:79-87). §9.5 step 2 compares only `(ip, port, ip_type, protocol, version)`, so a
certificate rotation alone is never republished. The fix is sound. With no certificate configured, the comparison
correctly ignores certificates, because the pallet never removes them.

## CC-12: partially confirmed (minor)

**Verified.** The receipt check (`CORE/client.rs:1024-1044`) looks up `result` or falls back to the attribute map.
`variant` is a top-level `field` lookup (:1515-1527). finney type 74 shows `Sudo` event 0 is
`Sudid { sudo_result: DispatchResult }`. A failed `Sudid` is therefore reported as success. The [I] label and "probably"
understate this.

**Incomplete evidence.** The fix's evidence omits the link that makes the conclusion certain: named event fields decode
to a map keyed by field name (`CORE/codec/decode.rs:428-435`), so the attribute map is `{sudo_result: …}`, not
`{Err: …}`.

**Corrected fix.**

- §5.8 #4: replace "[I] The core also reads … probably missed too" with: "Verified by code reading (not executed). The
  core looks up field `result` and otherwise uses the whole attribute map (`CORE/client.rs:1025-1037`). Named event
  fields decode to a map keyed by field name (`CORE/codec/decode.rs:428-435`), so `Sudid`'s map is `{sudo_result: …}`.
  `variant` is a top-level `field` lookup (`CORE/client.rs:1515-1527`), so an `Err` under `sudo_result` is never found
  (finney metadata, spec 477: `Sudo` event 0 is `Sudid { sudo_result: DispatchResult }`). A failed `Sudo.sudo`
  therefore reports success through the core client."
- §14.3 #14: drop "[I]", add `CORE/codec/decode.rs:428-435` to the evidence, and set "Default until resolved" to "a
  FakeChain receipt test with a failed `Sudid` (M2), filed as PR-U2 evidence".
