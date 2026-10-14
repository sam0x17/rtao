
## Inventory step

Errors and gaps in `docs/plan/architecture.md` found while building `docs/plan/parity-inventory.md` and
`docs/plan/python-module-map.md`. Each item has evidence; the deliverables either follow the evidence (saying so) or
mark the item open.

### Intents

1. **Pass-through set (§4.3, decision 15).** The "46 of 83" comes from `classify_intents.py`, a field-name heuristic.
   Against the `build()` bodies it is wrong twice: `multisig_threshold_1` is counted as pass-through but its build
   derives the multisig account view, composes the nested intent from `call` and sorts signatories
   (`PY/intents/multisig.py:185-197`, `_account_view :59-69`, `_compose_inner :71-84`); `transfer_all` is a 1:1
   single call (`PY/intents/transfer.py:103-106`) that the heuristic skipped because its call map has no `Balances`
   (nor `EVM`) pallet. The count stays 46 with the members swapped. Followed the source in the inventory.
2. **Generator milestone (§14).** M5 says "46 pass-through bodies generated", but 9 of them are M4 deliverables
   (`burned_register`, `register_subnet`, `root_register`, `set_mechanism_count`, `set_subnet_identity`, `start_call`,
   `trim_null_subnet_batch`, `trim_subnet`, `update_symbol`) and `transfer_all` is M2. Open: move the pass-through
   generator earlier or hand-write 10 bodies first.
3. **Stricter reading of "pass-through".** Of the 46, only 19 have no hand-written hook besides `summary()`; 8 add
   only `validate()`; 19 keep preflight hooks, `"all"` resolution, the shield route or executor post-processing.
   §4.3 acknowledges this for four ops; the inventory lists all.
4. **M4 vs M5 boundary.** §10.6 puts the owner `hparams` group in M4, but §10.5 lists `set-take` and `stake-burn` in
   it and their intents (`set_take`, `stake_burn`) are not in §14 M4's intent list. §14 M4's exit criterion "shielded
   scenarios green" refers to §11.4 scenario 11, which also shields `add_collateral` (M5 "collateral"). Resolved by
   moving the two commands to M5 and limiting the M4 run of scenario 11 to `burned_register`.
5. **Ambiguous M4 names resolved:** "identity" = `set_subnet_identity` (not `set_identity`), "trims" = `trim_subnet` +
   `trim_null_subnet_batch`, "mechanisms" = `set_mechanism_count`, "symbol" = `update_symbol`; `swap_hotkey`,
   `claim_root*` (in `registration.py`) and `register_leased_network` stay M5.

### Reads

6. **Exact hits (§4.8 "Exact hits", §11.3 `RecordingView`).** The catalog `hits` come from
   `read_targets` (`UP/website/apps/bittensor-website/scripts/generate.py:405-480`), which scrapes only the read
   function's own source for `st.`/`api.`/`constants.` references. Reads that go through module helpers or
   string-built `Item(...)`/`Method(...)` descriptors are under-reported. Seven reads with non-empty hits touch more:
   `coldkey_lock`, `locks_for_coldkey`, `locks_for_hotkey` (+ `StakeInfoRuntimeApi.get_coldkey_lock`,
   `SubtensorModule.DecayingLock` via `_lock_record`, `PY/reads/locks.py:33-40`); `miner_collateral`,
   `subnet_collateral` (+ `SubtensorModule.MinerCollateral`, `PY/reads/collateral.py:15, 92, 112`); `root_baskets`
   (+ `get_all_beta_pricing`), `validator_basket_summary` (+ `get_beta_pricing`) (`PY/reads/root.py:39-40`). The
   architecture's test (superset allowed only for the 10 zero-hit reads) would fail for these. Open.
7. **Reads needed before M6 (§14).** §14 ships "all 102 reads" in M6 but §9 components and the §10.6 M4 CLI v0
   (`wallet balance`, `wallet inspect`, `hparams get*`, `hparams mechanisms emissions`, the subnet-name cache) call
   39 reads in M1-M5 (2 / 13 / 1 / 22 / 1). The inventory assigns them with explicit rules.
8. **Namespace naming (§4.8).** "accessor structs named from the catalog's `python` field": the `metagraph` read has
   `python == ""` because the curated `Subnets.metagraph` shadows it (`PY/namespaces.py:161-173`). Use `category`.
9. **Commitments/metagraph layering (§2.3 vs §4.8, §8.2).** The curated `subnets().metagraph/commitments` methods
   (rtao-reads namespaces, §4.8) and the `commitments` read (`PY/reads/identity.py:139-162`, which calls
   `PY/metagraph.py` `fetch_commitments :258`) need the commitment status machine and `Metagraph`, which §8.2 places
   in `rtao-metagraph`; `rtao-metagraph` depends on `rtao-reads` (L6 → L5). Dependency cycle; needed in M1
   (§14 M1 exit uses `client.subnets().metagraph(...)`).
10. **§8.1 model list** has no model for `System.Account` (`AccountInfo`/`AccountData`; reads `balance`, `balances`,
    needed by M4 CLI v0) or `BetaBasketRuntimeApi.get_basket_trading_status`'s `BasketTradingStatus`. DynValue works
    (§3.7), so this is a note, not a blocker.

### e2e

11. **"73 intent_* names" (§4, §11.2, §14 M5 exit).** 72 are `intent_plan_test!` instances that match catalog ops;
    `intent_set_root_claim_type` (`UP/sdk/bittensor-core/tests/e2e.rs:158-169`) asserts a retired core constructor
    (`IntentCall::set_root_claim_type`) and has no catalog op, so it cannot be "collapsed into one table-driven test
    over `AnyIntent`" or be "mapped and green" as an intent.
12. **Uncovered ops.** 11 catalog ops have no `intent_*` name; `pow_register` (PYT e2e + §11.4 #14),
    `add_collateral` (#11) and `trim_null_subnet_batch` (#4) are covered elsewhere; `move_swap_stake`,
    `set_min_collateral`, `set_reject_locked_alpha`, `set_root_claim_threshold`, `set_subnet_emission_enabled`,
    `stake_into_basket`, `swap_basket`, `swap_basket_many` have no localnet test anywhere.
13. **`e2e_map.toml` (§4.9)** is defined over the 112 manifest names; `PYT/e2e/{test_null_consensus,
    test_pow_registration,test_timelock_commitment}.py` are outside the manifest.

### CLI

14. **§10.6** lists `wallet inspect` in both M4 and M8 (kept M4); M4 omits `wallet create` although it only combines
    `new-coldkey` + `new-hotkey` (left in M8 per the text, probably an omission).
15. **"proxy, multisig basics" (§10.6 M5)** is undefined; the inventory reads it as all `multisig` commands and the
    non-`book` `proxy` commands [I].
16. **§10 "`cli/commands/*.py`: 7,872 lines"** excludes `cli/commands/evm/` (1,602 lines); correct as written but easy
    to misread as all command code.

### Module homes (also in python-module-map.md "Gaps found")

17. `PY/basket_index.py` (361) and `PY/basket_index_data.py` (975): no home anywhere in the architecture (CLI-only,
    imported by `cli/root_helpers.py:18`, `cli/commands/root.py:23`, `cli/commands/root_move.py:11`).
18. `PY/config.py` (480): no single home; needed by `rtao-client` (token-symbol cache, §3.3 step 6),
    `rtao-neuron` (`NeuronConfig` precedence, §9.9) and `rtao-cli` (§10.2). §2.2 names no config module.
19. `PY/hyperparams.py`: §5.5/§2.2 put the unit functions in `rtao_primitives::hyperparams` "over the extracted
    `HYPERPARAMS` table", but the table is in `rtao-catalog` (L3), which depends on `rtao-primitives` (L2); §2.2 also
    lists `to_raw` in `rtao_intents::hyperparameters`.
20. `PY/extension/bridge.py` / `client.py`: the bridge is a WebSocket server and client (`websockets.asyncio.server`
    `bridge.py:14`, `websockets.asyncio.client` `client.py:11`); §6.5 puts the wire-compatible `ExtensionSigner` in
    `rtao-signer`, but §2.3 rule 2 allows `tokio-tungstenite` only in `rtao-rpc` and `rtao-testkit`.
21. `PY/_live.py`, `PY/masked_input.py`: no explicit home (implied `rtao-cli`, §2.7 `rpassword`, D-003).
22. `parity/map.toml` kinds (§4.9) have no value for vendored static assets (`evm/abi/*.json`, `extension/static`,
    `vault/static`).
23. Minor: `Spend` is listed as an `rtao-tx` item (§2.2) but defined in the `rtao-primitives` code of §5.2 (fine as
    a re-export; worth stating).

## Plan step

Errors, gaps and discrepancies found while writing `PLAN.md`, with evidence, and how `PLAN.md` handles each. Items
already recorded under "Inventory step" are not repeated; `PLAN.md` follows the inventory's resolutions and lists the
open ones in its §14.3.

### New findings

1. **`serde_json/arbitrary_precision` is forced on every rtao and subnet build (not addressed by the architecture).**
   The core's manifest has `serde_json = { version = "1.0.132", features = ["arbitrary_precision"] }` as a non-optional
   dependency (`UP/sdk/bittensor-core/Cargo.toml:40-42`). Cargo unifies features, so every crate in the graph gets it.
   Measured A/B with serde_json 1.0.154 (test program in `scratchpad/plan/ap-check/`, `cargo run` vs
   `cargo run --features ap`):

   | case | without feature | with feature |
   |---|---|---|
   | `#[serde(untagged)]` map of floats, `from_str` | ok | ERR "data did not match any variant" |
   | `#[serde(untagged)]` list `[[1, 0.5]]`, `from_str` | ok | ERR |
   | same, `from_value(json!(...))` | ok | ok |
   | untagged number-or-string, `from_str("1.5")` | ok | ERR |
   | untagged number-or-string, `from_str("7")` | ok | ok |
   | `#[serde(flatten)]` struct with an f64, `from_str` | ok | ERR "invalid type: map, expected f64" |
   | `Value(1e-05) == Value(1e-5)` | true | false |

   Code path: `REG/serde_json-1.0.154/src/de.rs:118-129` (`ParserNumber::String` visited as a map),
   `:937-953` (`parse_any_number` keeps non-u64/i64 numbers as strings), `src/number.rs:20-24, 72-73` (`Number` derives
   `PartialEq` over `type N = String`). Impact: architecture §4.2's untagged `WeightsArg` and number-or-string
   `Amount<U>` inputs when parsed from text (agents, `execute_tool` JSON), §4.8's "parsed values" oracle comparison if
   done with `serde_json::Value ==`, and subnet authors' own axum `Json<T>` types. PLAN.md: §5.8 #10, §7.3 note, §13
   R19 (new), §14.3 #1 (open; M0 test, custom deserializers, numeric comparison, ask upstream).
2. **4 of the core's 22 `IntentCall` constructors have no catalog op**: `transfer_allow_death`, `move_stake_limit`,
   `transfer_stake_and_hotkey`, `set_root_claim_type` (`CORE/transaction.rs:251-826` vs `CAT/intents.json`). §4.6's
   "builds the core's 22 trusted constructors and the matching rtao intents" and §11.8's "`compose_call` bytes for the
   22 core constructors" can pair only 18 with intents. PLAN.md §5.8 #9, §12.4, §14.3 #10.
3. **[I] The core's receipt check misreads `Sudo.Sudid`.** `CORE/client.rs:1025-1037` looks up field `result` and
   falls back to the whole attribute map, then searches it for `Err`; `Sudid`'s field is `sudo_result`
   (`PY/_transport/utils/receipt.py:9`, pallet_sudo `Sudid { sudo_result }`), so a failed `Sudid` is probably reported
   as success. Code reading only, not executed. Strengthens PR-U2. PLAN.md §5.8 #4, §14.3 #14.
4. **hidapi is `ledger`-gated, not `host`-gated**, and on Linux uses the pure-Rust hidraw backend
   (`UP/sdk/bittensor-core/Cargo.toml:82-94, 126`). Architecture §1.7 and R4 list "hidapi C builds" among the costs of
   `host`. No design change; PLAN.md §5.7 note and R4 without hidapi.
5. **No-host banned-crate lists differ**: §1.7 lists `openssl-src` and the core's `reqwest` but not `libloading`;
   §2.3 rule 5 and Appendix E list `libloading` but not `openssl-src`. PLAN.md §5.7 uses the union.
6. **Line citation off by one**: `_NON_IDEMPOTENT_METHODS` is at `PY/_transport/rpc.py:66`; the architecture cites
   `:61-65` (the comment block). Cosmetic. PLAN.md §18.2 #7.
7. **M0 dependencies scheduled later**: the M0 btauth/1 core signs through `&dyn Signer` (§7.1) from `rtao-signer`,
   whose `KeypairSigner` §14 lists in M2; M0's `rtao doctor` lock check needs a binary, while `rtao-cli` starts in M4.
   PLAN.md pulls forward only the `Signer` trait, `KeypairSigner` and a `doctor`-only `rtao-cli` ([I], §12.2, §14.3 #12).
8. **"Parity machinery in place from M0"** (§14 preface) vs §14's M0 deliverables, which contain no parity task, and
   the two-engineer note ("builds the parity machinery during M0-M2"). PLAN.md adds the `parity check` file layer with
   the first `map.toml` rows to M0 and completes the other layers in M2 ([I]).
9. **Localnet harness timing**: §14 lists `Localnet` + `Scenario` in M2 but schedules the D-001 localnet check in M1.
   PLAN.md puts a minimal `Localnet` in M1 ([I]).
10. **M0 golden replay "through rtao conversions"** precedes the typed builders (`xtask regen` v1 is M1). PLAN.md reads
    it as replay through `IntoCoreValue` + `compose_call` by name in M0, then through typed builders in M1 (§11.2).
11. **Upstream PR timing**: §14 files PR-U1…U15 in M9, although evidence for PR-U4 (scenario 16), PR-U5 (vectors) and
    PR-U7 (the runtime check) exists by M2-M4, and PR-U5/PR-U7 fix live defects for other core users. PLAN.md keeps M9
    as the default and raises it as user question Q18 (new).
12. **`clippy.toml` paths behind `host`**: `client`, `transaction`, `keyfiles` and the drand fetches exist only with
    the core's `host` feature (`CORE/lib.rs:10-31`). CI runs clippy with `--all-features`, so they resolve; a no-host
    clippy run may warn about unresolvable configured paths, depending on the clippy version [I, not executed].
    PLAN.md §5.4, §14.3 #11.
13. **Repository state**: the rtao repo has no `LICENSE` file although `Cargo.toml` says MIT, and `Cargo.lock` is
    untracked. PLAN.md §12.14 day 1.
14. **§2.6's owner-tool manifest omits the patch line.** Every final workspace needs it (§1.3); PLAN.md §7.1 adds it.
15. **CLI count wording**: the inventory's 163 commands include the root commands `tools`, `explain`, `call` (160 in
    19 groups + 3), although its §7.1 lists those three under "generated at run time". PLAN.md §8.2 states the split.

### Spot-checks of carried-over claims (against `UP/` at `850f2755`, and `v477` where noted)

| # | Claim | Evidence checked | Result |
|---|---|---|---|
| 1 | core `submit_shielded` uses a 64-block era for carrier and inner; the runtime rejects `submit_encrypted` with an immortal era or period > 8 as `Stale` | `CORE/client.rs:32` (`DEFAULT_ERA_PERIOD = 64`), `:622-654` (inner `:633`, carrier `:640-646`); `UP/runtime/src/check_mortality.rs:19` (`MAX_SHIELD_ERA_PERIOD = 8`), `:91-100`; `git show v477:runtime/src/check_mortality.rs` identical | confirmed. Nuance: the runtime check applies to the `submit_encrypted` carrier only; the 8-block inner era is Python's choice (`PY/settings.py:24-29`, `PY/executor.py:1039, 1049`) |
| 2 | core `should_run_epoch` uses `blocks_since_last_step > MAX_TEMPO_U64`; the pallet uses `> tempo` and caps at `tempo + 1` | `CORE/timelock/epoch_schedule.rs:45-59` (`:54`), `CORE/timelock/constants.rs:6-9` (50,400); `run_coinbase.rs:1224-1238` (`:1232`), `:412-416`; v477 identical | confirmed; Python path `PY/intents/weights.py:308` → `bittensor-core-py/src/timelock.rs:61` → `generate_commit_v2` |
| 3 | `generate_commit_v2` reads the wall clock; reveal-round formula | `CORE/timelock/mod.rs:160-195` (`now_unix()` `:177`), constants `GENESIS_TIME`, `DRAND_PERIOD = 3`, `SECURITY_BLOCK_OFFSET = 3` | confirmed |
| 4 | core manifest inherits `w3f-bls` (`:62`) with the pallet-drand comment (`:51-53`); root patch `UP/Cargo.toml:335` | files read | confirmed |
| 5 | receipts: Python 6 nested events, core 3 | `receipt.py:8-16`; `client.rs:1025-1029` | confirmed (+ finding 3) |
| 6 | `commit_reveal_version` 4; chain equality check; `Pays::No` | `weights.py:33, 411, 472-477`; `subnets/weights.rs:318-320, 337`; `dispatches.rs:1946-1948` | confirmed |
| 7 | weight quantization with Python `round()`, epsilon `1e-7`, sorted estimation, `accumulate` | `PY/intents/weights.py:54-114` | confirmed |
| 8 | RPC policy constants; non-idempotent methods; `account_nextIndex` nonce cache | `rpc.py:52-66`; `extrinsics.py:55-84` | confirmed (finding 6) |
| 9 | V14 → `apis = Vec::new()`, V16 rejected; Python V15 needle and disk cache | `runtime/mod.rs:171-198`; `runtime.py:33-39, 117, 292-312` | confirmed |
| 10 | `host` gating; `default = ["host"]`; no-host keyfile JSON uses only non-host constructors | `CORE/lib.rs:10-31`; manifest `:105-126`; `keyfiles/mod.rs:175-372` | confirmed (+ finding 4) |
| 11 | v4-only extrinsic assembly | `codec/extrinsic.rs:268-273` | confirmed |
| 12 | core fee via `payment_queryInfo` with era 64; Python immortal zero-signature `state_call` probe; Python finalized anchor; core best-head anchor | `client.rs:326-328, 514-518, 551-576`; `interface.py:485-499, 714-731` | confirmed |
| 13 | catalog counts 83 (75/8; 74/7/2), 139 wraps / 116 distinct, 102 reads (81/11/10), 14 categories, 18 codes, 390 chain errors; 0 schema defaults; e2e 112 = 73 + 39 | `python3 -I` over `CAT/*.json` and `e2e-manifest.json` | confirmed |
| 14 | §12.1 churn table (files 32/146/97/282/263/93; core 0/0/0/1/5/0; python 7/9/6/11/36/3) | `git diff --name-only` between tags | confirmed exactly |
| 15 | line counts: PY 59,723; CLI 23,083; commands 7,872; intents 8,335; executor 1,319 | `wc -l` | confirmed |
| 16 | pins: `v477` = `5c6e83e2…`; `git diff v477 850f2755 -- sdk` empty; toolchain 1.89 | `git rev-parse`; `git diff`; `rust-toolchain.toml` | confirmed |
| 17 | 22 trusted constructors at `transaction.rs:251-826` | `awk` over `pub fn` | confirmed (+ finding 2) |
| 18 | all ten `disallowed-methods` paths exist | `client.rs:150`; `transaction.rs:1004, 1011`; `timelock/mod.rs:160, 242, 276, 298`; `epoch_schedule.rs:45, 108`; `keyfiles/mod.rs:126` | confirmed |
| 19 | public endpoints about 1 r/s per IP | `docs/guides/local-development.mdx:160` | confirmed |
| 20 | decode benchmark (6.5 / 330 / 1,380 µs at 256 UIDs; 57 µs at 2,500) and tag-pin build (241 packages, 12.93 s) | `final/docs/plan/bench/decode-bench/bench-run-3.txt`, `bench/tagpin/tagpin.log` | confirmed |

### Numbers taken as given (not recomputed)

EW per milestone (2, 3.5, 5, 3, 4.5, 6.5, 4, 4, 8, 3 = 43.5; M10 3), calendar anchors (10.5/13.5/18/43.5 and
8/10/13/26-28), the §0.4 line budget, 46 pass-through bodies (inventory membership), all per-milestone counts from the
inventory's §1, and the module-map kind totals (143/30/29/18) are used as written. The critical-path sum (32.5 EW) and
the one-engineer cumulative weeks are arithmetic on the §14 figures.
