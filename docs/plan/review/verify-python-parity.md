# Skeptic verification: python-parity findings (PP-01 … PP-25)

Every finding was re-checked against the plan text and the evidence on disk (`UP/` at `850f2755`, `CAT/`, `meta/finney.json`
spec 477, `scratchpad/review-tools/litcheck.py`, AST scans with `python3 -I`). Path prefixes follow architecture.md
"Evidence conventions". Nothing was refuted. 9 confirmed, 16 partially confirmed: the problem is real, but the fix
needed correction or the claim was overstated.

| ID | Verdict | Severity after check |
|---|---|---|
| PP-01 | partially_confirmed | blocker |
| PP-02 | partially_confirmed | major |
| PP-03 | confirmed | major |
| PP-04 | partially_confirmed | major |
| PP-05 | partially_confirmed | major |
| PP-06 | partially_confirmed | major |
| PP-07 | partially_confirmed | minor (was major) |
| PP-08 | partially_confirmed | major |
| PP-09 | partially_confirmed | minor (was major) |
| PP-10 | partially_confirmed | major |
| PP-11 | confirmed | major |
| PP-12 | partially_confirmed | major |
| PP-13 | partially_confirmed | minor (was major) |
| PP-14 | partially_confirmed | minor (was major) |
| PP-15 | confirmed | minor |
| PP-16 | confirmed | minor |
| PP-17 | confirmed | minor |
| PP-18 | confirmed | minor |
| PP-19 | partially_confirmed | minor |
| PP-20 | confirmed | minor |
| PP-21 | confirmed | minor |
| PP-22 | partially_confirmed | minor |
| PP-23 | partially_confirmed | minor |
| PP-24 | partially_confirmed | minor |
| PP-25 | partially_confirmed | minor |

---

## PP-01: partially_confirmed (blocker)

**Verified.** In architecture §5.2 (lines 2089-2091), `Balance<Alpha>::new` rejects netuid 0 ("netuid 0 is TAO: use `Balance<Tao>`"),
and `Amount<U>` wraps `Balance<U>`. Serde parses the field without knowing the sibling netuid, and §4.2 (line 1602) types money
by name suffix only.

Python does it differently:
- It tags alpha with the sibling netuid and accepts 0: `call_amount` sets `expected_netuid = netuid` for ALPHA parameters (`PY/intents/_money.py:147-165`).
- `RemoveStake.__post_init__` passes `netuid=self.netuid` (`staking.py:701-704`).
- Move, swap, transfer and move-swap pass `origin_netuid` (`:837, 1262, 1364, 1479`).
- `swap_basket_many` legs use `alpha_amount(leg, origin/destination)` (`:1892-1897`).
- Root branches exist at `staking.py:306-345, 772-776, 914-916, 1102-1104`.

Spec-477 metadata types `remove_stake.amount_unstaked` as `AlphaBalance`, and `swap_basket.min_amount_out` as a bare `u64`, so
the fallback table is needed. `CAT/intents.json` has 26 money properties. Three of them have no suffix: `swap_basket.amount`,
`swap_basket.min_amount_out` and `set_root_claim_threshold.threshold`. Every upstream staking sample uses netuid 1 or 2
(`PYT/harness/samples.py:43-76`). Only the basket samples hit netuid 0 (`:145-151`).

**What is wrong with the fix.**
1. Architecture §4.2 declares `validate(&self) -> Result<(), IntentError>`. That signature cannot store a resolved value.
2. A unit-free `Amount<Unresolved>` drops Python's UnitMismatch check for inputs that are already typed as `Balance`, which is
   `call_amount`'s main job (`_money.py:152-163`).
3. `unstake_all` has no amount or netuid field.

**Corrected fix.** In architecture.md §5.2, replace `pub enum Amount<U: Unit> { Exact(Balance<U>), All }` with
`pub enum Money { Untagged { rao: u64 }, Tagged(DynBalance) }` and `pub enum Amount { Exact(Money), All }`. Then replace the
"Intent money fields" bullet with:

> **Intent money fields** hold `Amount`. Serde accepts a number, a decimal string matching `money_schema`'s pattern (exact,
> at most 9 fractional digits) or `"all"` (only for `ALL_AMOUNT_FIELDS`). It yields `Money::Untagged` and rejects negatives.
> Typed constructors also take `Balance<Tao>`, `Balance<Alpha>` or `DynBalance` (`Money::Tagged`). Each field is resolved by a
> port of `call_amount`/`tao_amount`/`alpha_amount` (`PY/intents/_money.py:51-165`):
> - **Unit.** The wrapped call argument's runtime type ident from the `CallDescriptor` (`TaoBalance`/`AlphaBalance`). Otherwise
>   the extracted `_FALLBACK_AMOUNT_UNITS` (`:89-111`); for example, `swap_basket.min_amount_out` is a bare `u64` at spec 477.
> - **Expected netuid.** 0 for TAO. Otherwise the sibling field named in `codegen/intent_types.toml`:
>   - `amount_alpha` → `netuid` for `remove_stake(_limit)`, `lock_stake`, `add_collateral`;
>   - `amount_alpha` → `origin_netuid` for `move_stake`, `swap_stake`, `move_swap_stake`, `transfer_stake`;
>   - `min_alpha` → `netuid`;
>   - `swap_basket.amount` → `origin_netuid`; `swap_basket.min_amount_out` → `dest_netuid`;
>   - each `swap_basket_many` leg's `amount`/`min_amount_out` → its `origin_netuid`/`dest_netuid`;
>   - `set_root_claim_threshold.threshold` and every `*_tao` → TAO.
> - **Tagging.** `Untagged` is tagged with the expected netuid, so netuid 0 yields a TAO-denominated amount exactly as Python
>   tags root alpha (`:93-98`). `Tagged` with another netuid fails with `UnitError::UnitMismatch` and Python's message
>   (`:152-163`).
>
> The generated accessor `fn <field>_resolved(&self) -> Result<AmountOf<DynBalance>, IntentError>` performs this. `validate()`
> calls every accessor (Python normalizes in `__post_init__`), and `build()`, `summary()` and `spend()` use them.
> `Balance<Alpha>` (netuid ≠ 0) stays the framework's domain type, with `TryFrom<DynBalance>`.

Other edits:
- **§4.2, money rule:** "money (`anyOf` with the decimal pattern) → `Amount`, resolved per §5.2. A money property without a
  `_tao`/`_alpha` suffix (today `swap_basket.amount`, `swap_basket.min_amount_out`, `set_root_claim_threshold.threshold`)
  needs an explicit `intent_types.toml` entry naming its unit source; otherwise the generator fails."
- **§4.2, `AddStake` example:** `pub amount_tao: Amount`.
- **§4.3:** replace "`Amount<Alpha>` checked against the call's netuid" with "money resolved per §5.2".
- **§11.5:** "`vendor/oracle/samples_extra.json` adds root-subnet samples, dumped by the Python oracle like upstream samples
  (M5): `remove_stake` and `remove_stake_limit` with `netuid: 0`; `move_stake`, `swap_stake`, `move_swap_stake` and
  `transfer_stake` with `origin_netuid: 0`. Each comes with a number and, where allowed, `"all"`. Add
  `move_stake {origin_netuid: 0, claim: true}` and `unstake_all {claim: true}` for the root-claim branches
  (`PY/intents/staking.py:306-345`)."

## PP-02: partially_confirmed (major)

**Verified.**
- `Multisig` is in `__all__` (`PY/__init__.py:62, 186`). `approve` defaults are `signer="coldkey"`, `wait_for_inclusion=True`,
  `wait_for_finalization=False`, `shielded=False` (`PY/multisig.py:166-313`).
- `Client.multisig`, `decode_scale`, `estimate_weight` and `estimate_shielded_carrier_fee` are at `PY/client.py:264-271, 464-466, 564-590`.
- `submit_call(signer, shielded)` is at `PY/executor.py:1216-1256`.
- Consumers: `cli/call.py` (`client.multisig`, `ms.approve(..., shielded=)`, `submit_call(..., shielded=)`),
  `cli/commands/multisig.py:136, 187, 311`, `cli/multisig_helpers.py:897`, `cli/commands/upgrade.py:400` and
  `cli/upgrade_helpers.py:389, 462`.
- All four plan documents have 0 hits for `decode_scale`, `estimate_weight`, `estimate_shielded`, `MultisigFundingError` and
  an `approve` API. §4.6 `submit_call` has no shielded option.

**Overstated.** The typed `submit_call<R: Role, …>` already chooses the role. What is missing is a dyn twin for the CLI.

**What is wrong with the fix.**
- It adds `signer: SignerRole` to the options, which conflicts with `R`.
- It puts the read-side `decode_scale`/`estimate_*` on `Executor`. Python has them on `Client`, and rtao-client owns the fee
  probe (§3.10) and the shield (§3.11).
- It sets M4 without moving PLAN §12.7's raw-path bullet (M5) to M4, where `rtao call` (PLAN §12.6) needs it.
- The funds preflight must sit in rtao-tx (L5), because `Multisig::approve` (rtao-tx) uses it. The module map puts it in
  rtao-intents (L6).

**Corrected fix.** Add to architecture.md §4.6 and §3.12:

```rust
impl Client {   // rtao-client: read-side, as Python's Client methods
    pub async fn decode_scale(&self, type_name: &str, bytes: &[u8]) -> Result<DynValue, ClientError>;             // PY/client.py:264-271
    pub async fn estimate_weight(&self, call: &CallBytes, address: AccountId) -> Result<Weight, ClientError>;      // :564-572
    pub async fn estimate_shielded_carrier_fee(&self, fee_payer: AccountId) -> Result<Balance<Tao>, ClientError>; // :464-466
}
pub struct Multisig { pub signatories: Vec<AccountId>, pub threshold: u16, pub address: AccountId }          // rtao_tx::multisig; PY/multisig.py:166-182
pub struct ApproveOptions { pub signer: SignerRole /* Coldkey */, pub wait_for_inclusion: bool /* true */,
                            pub wait_for_finalization: bool /* false */, pub shielded: bool /* false */ }    // :184-193
impl Executor { pub fn multisig(&self, signatories: &[AccountId], threshold: u16) -> Result<Multisig, TxError>; } // core multisig_account_id
impl Multisig { pub async fn approve(&self, ex: &Executor, call: CallBytes, keys: &dyn DynKeySource, o: &ApproveOptions)
                    -> Result<ExtrinsicResult, TxError>; }
```

`approve` ports four pieces of Python:
1. **`_preflight_funds`.** Best effort. The opener is the signer when no `Multisig.Multisigs[address, call_hash]` entry exists.
   The required balance is `DepositBase + threshold × DepositFactor` plus the `as_multi` fee. A shortfall raises
   `ChainError { code: InsufficientBalance }` with Python's message and `fund_help` as remediation (`:32-49, 121-162, 270-313`).
   This preflight lives in `rtao_tx::multisig`, and the M5 multisig intents reuse it.
2. **`_compose_as_multi`.** Timepoint from `Multisig.Multisigs`, `max_weight` from `estimate_weight`, other signatories
   sorted by account bytes.
3. The clear submission path.
4. The shielded path through §3.11 with `ShieldPeriod::P8`.

Raw path:
- `ExecuteOptions` gains `shielded: bool` (false). With the default period, a shielded raw call uses `ShieldPeriod::P8`
  (`PY/executor.py:1216-1256`).
- The typed `submit_call<R, K>` keeps its role type parameter.
- A dyn twin, `submit_call_dyn(&self, call: CallBytes, keys: &dyn DynKeySource, signer: SignerRole, o: &ExecuteOptions)`,
  serves `rtao call --signer coldkey|hotkey`.

Milestone M4, because `rtao call --multisig/--multisig-threshold/--signatories/--other-signatories` is CLI v0. Move PLAN
§12.7's "Raw path (`submit_call` under `allow_raw_calls`)" bullet to §12.6.

python-module-map.md:
- `multisig.py` → `rtao_tx::multisig::{Multisig, ApproveOptions}` and the funds preflight, plus `rtao_client::compose::as_multi` (M4).
- `client.py` → M1–M5, adding `decode_scale`, `estimate_weight`, `estimate_shielded_carrier_fee` and `multisig`.

Tests: port `PYT/unit/test_multisig_safety.py`. Interop lane 7: rtao opens a raw-call 2-of-3 multisig that btcli approves
and executes, and the reverse.

## PP-03: confirmed (major)

Verified:
- Architecture §2.2 puts `blocking::{SyncClient, Subtensor}` and feature `blocking` in rtao-client. §2.4 has
  `blocking = ["rtao-client/blocking"]`. §2.3 places rtao-client at L4, tx and reads at L5, intents at L6.
- `PY/sync.py` wraps read namespaces and `read` (401), `mine_pow_registration` (407), and
  `submit_call`/`plan`/`execute`/`execute_tool`/`submit_shielded`/`multisig`/`tools` (431-476).
- `PY/_subtensor.py` imports `SetWeights` (35-41) and `set_weights` builds it (167-205).
- `SyncClient` declares 13 namespace attributes without `collateral` (`sync.py:186-241`) and has no `__getattr__`.
  `SyncSnapshot` loops `NAMESPACES`, which has 14 (`namespaces.py:279-294`).

The fix is correct. Note: PLAN.md §4.3 (rtao-client row, line 222) and §4.4 repeat the placement and need the same edit.

## PP-04: partially_confirmed (major)

**Verified: the defaults part.**
- `CAT/reads.json` params are bare type strings, with no default and no required flag.
- Python defaults: `PY/reads/neurons.py:41` (`lite=True`), `weights.py:32, 59, 89` (`mechid=0`), `collateral.py:67-68`
  (`coldkey_ss58=None`), `base.py:156-166` (`netuid=None`). CLI rules: `PY/cli/query.py:30, 39-58, 218-246`.
- Architecture §4.8 `ParamSpec` has `required` with no source. Nothing in the plan set covers read defaults.

**Already resolved: the namespace part.** parity-inventory §8 #12 decides `category`, and PLAN §12.8 adopts it. Only the
sentence in architecture §4.8 is stale. The fix replaces that decision with a new rule based on the `python` prefix.

**Corrected fix.** Add a row to architecture §4.1:

> read parameter defaults and requiredness | the `@read` signatures in `PY/reads/*.py` (`neurons` `lite=True` `neurons.py:41`;
> `weights`/`bonds`/`timelocked_weight_commits` `mechid=0` `weights.py:32, 59, 89`; `miner_collateral` `coldkey_ss58=None`
> `collateral.py:67-68`), `scalar_read` `netuid=None` (`PY/reads/base.py:156-166`), and the own-key rule
> (`PY/cli/query.py:30, 45-58`) | extractor → `vendor/upstream/extracted/read_params.json` {read, param, default (JSON literal
> or null), required, own_key}, using `_is_required`: a param with no default, or with default `None`, is required, except
> `hotkey_ss58`/`coldkey_ss58`.

Other edits:
- **§4.8 `ParamSpec`:** `(name, JsonType, doc, required: bool, default: Option<&'static str>, own_key: bool)`. `dispatch` fills
  defaults before calling the typed fn. `rtao query` emits `--x/--no-x` for boolean params and the wallet fallback for own-key
  params (`PY/cli/query.py:218-246`). The catalog layer asserts the (name, type) set equals `CAT`.
- **§4.8 namespaces:** replace "accessor structs named from the catalog's `python` field" with "accessor structs named from
  the catalog `category` (14 categories, one namespace each; names as in the parity-inventory §5 headings, §8 #12). `python`
  names only the method, and xtask asserts that its `sub.<ns>.` prefix, where present, equals the category's namespace (the
  `metagraph` read has no `python` key)."

Milestones: M2 (first generated dispatch); the CLI part in M4. Oracle check: `client.read("neurons", netuid=1)` with no
`lite` equals Python's lite payload. `READ_SAMPLES` already omit `lite`, `mechid` and `coldkey_ss58`
(`PYT/harness/samples.py:219, 225, 261, 295, 305`).

## PP-05: partially_confirmed (major)

**Verified with litcheck.**

| Location | Non-literal expression |
|---|---|
| `settings.py:15` | `RAO_PER_TAO = 10**9` |
| `settings.py:33-39` | `NETWORKS` uses `os.getenv(...) or ...` |
| `hyperparams.py:32-37` | `U16_MAX` |
| `hyperparams.py:110-408` | `HYPERPARAMS` uses `FIXED128_ONE`, `3 * FIXED128_ONE` (`:330-331`) |
| `hyperparams.py:416-461` | `STORAGE_ITEMS` uses `st.SubtensorModule.*` |
| `evm/networks.py:41` | `os.getenv(...)` |
| `fake_substrate.py:36, 70, 76` | `10 ** 9`; `GENESIS_HASH` |

Intent defaults use `DEFAULT_RATE_TOLERANCE` (7 intents) and `DEFAULT_COMMIT_REVEAL_VERSION`, and `ClaimRoot` uses
`default_factory=lambda: [0]`. Two citations are also wrong:
- §4.1 cites `hyperparams.py:91-110`, which is the `Hyperparam` dataclass, not the table.
- §4.1 cites `rpc.py:51-65`, but `_NON_IDEMPOTENT_METHODS` is at `:66`.

**What is wrong with the fix.**
- Its folding is integer-only. `PYT/harness/samples.py` (`'0x' + 'ab' * 32`, `'0x' + '22' * 65`) and `fake_substrate.py:76`
  would still fail, yet the fix's own M1 test covers both files.
- It merges `os.getenv(X) or lit`, where an empty value counts as unset, with `os.environ.get(X, lit)`.
- It lacks the builtin type tags and home-relative paths that `PY/config.py` needs (PP-14).

**Corrected fix.** Replace the subset sentence in architecture §4.9 layer 4 with:

> The extractor accepts:
> - literals (`str`, `int`, `float`, `bool`, `None`, implicit string concatenation, tuples, lists, dicts, sets, `frozenset(...)`);
> - enum attribute references;
> - constructor calls whose arguments are accepted;
> - and these resolved forms:
>   1. constant folding of integer `+ - * // ** << |`, string `+`, and string × integer `*` over accepted operands;
>   2. bare names bound at module level to an accepted value: in the same module, in the module the name is imported from, or
>      in `PY/settings.py`. In test-harness tables, dev-key names resolve through the fixed ss58 table of §4.1;
>   3. environment defaults resolved at run time: `os.getenv("N") or <lit>` → `{env, default, empty_is_unset: true}`, and
>      `os.environ.get("N", <lit>)` → `{env, default, empty_is_unset: false}`;
>   4. attribute chains into the generated descriptor modules (`st.<Pallet>.<Item>`, `api.<Api>.<method>`) → `{pallet, item}`,
>      checked against the pinned metadata;
>   5. `field(default_factory=lambda: <accepted>)`;
>   6. the builtin type names `str`, `int`, `float`, `bool` used as values (type tags);
>   7. `Path.home() / <str> / …` → `{home_relative: [...]}`.
>
> Anything else fails loudly with file and line. Derived tables (comprehensions such as `_REVERSE_NETWORKS` and
> `_CATALOG_DOCS_BY_NAME`) are not extracted; rtao derives them.

Add an M1 test, `extract_check_covers_named_tables`, that runs `xtask extract --check` over every table that §4.1 and the
module map name as extracted:
- `PY/settings.py`;
- `PY/_transport/rpc.py:52-66, 294-300`;
- `PY/error_map.py:488, 524, 555`;
- `PY/result.py:27-36, 78, 116, 291, 308, 475`;
- `PY/executor.py:52-66`;
- `PY/multisig.py:162`;
- `PY/hyperparams.py:32-37, 83, 110-408, 416-461, 467-471, 626-643, 646-657`;
- `PY/intents/hyperparameters.py:31-66`, `PY/intents/proxy.py:21-41`, `PY/intents/_money.py:89-111`;
- intent field defaults and class vars;
- `PY/fee_filters.py`, `PY/evm/networks.py:27-43`, `PY/config.py:12-56`, `PY/basket_index_data.py`;
- `codegen/check.py:58-72, 74-300`;
- `PYT/harness/samples.py` and `PYT/harness/fake_substrate.py:34-77`.

Correct these citations:
- `HYPERPARAMS` → `:110-408` (§4.1 row, module map).
- `RATIO_TYPE_DENOMINATORS` → `:32-37` (§5.4).
- `_NON_IDEMPOTENT_METHODS` → `rpc.py:66` (§3.2.1, §16 #7). The §4.1 range becomes `:52-66`.

## PP-06: partially_confirmed (major)

**Verified.** Layer 1 of §4.9 fails on any watched file that has no row. At `850f2755` the watched roots outside the package
hold 111 files:

| Root | Files |
|---|---:|
| codegen | 6 |
| tests: unit / harness / fixtures / e2e | 34 / 4 / 2 / 3 |
| `sdk/bittensor-core` | 35 |
| `rpc_info` | 9 |
| `eco-tests` | 7 |
| named runtime/common/coinbase/docs files | 8 |
| catalogs | 3 |

python-module-map.md covers only the 220 package files. PLAN §12.2 seeds 3 rows in M0, and §12.4 completes them from the module
map in M2, so the gate fails at M0 and again at M2. `pyproject.toml`, `completions` (3 files) and `scripts` (8 files) exist and
are not watched.

**What is wrong with the fix.** Its count: 220 + 111 + the 7 newly watched files = **338** rows, not 331.

**Corrected fix.** Add to python-module-map.md a section "Other watched roots" with these rows:

| Files | Kind | Home / note | Milestone |
|---|---|---|---|
| `sdk/python/codegen/check.py` | extracted | `rtao_catalog::py::{COVERED_PALLETS, RAW_ONLY}` and `tests/hyperparam_units.rs` | M2 |
| `codegen/{__init__, __main__, emit_python, emit_namespaces, metadata}.py` | n/a | replaced by `xtask regen` | M2 |
| `tests/unit/*.py` (34) | oracle-only, except the 11 ported tests below | ported tests carry `// pyt:` provenance | milestone of the code they test |
| `tests/harness/{samples, fake_substrate}.py` | extracted | | M2 |
| `tests/harness/fake_node.py` | ported | `rtao-testkit::FakeNode` | M1 |
| `tests/harness/__init__.py` | n/a | | |
| `tests/fixtures/golden.json`, `shape_corpus/corpus.json` | vendored | | M0 |
| `tests/e2e/*.py` (3) | oracle-only | mapped in `e2e_map.toml` | |
| `sdk/bittensor-core/**` (35) | n/a | guarded by `core-surface` and `staged --check` | M0 |
| `pallets/subtensor/src/rpc_info/**` (9) | extracted | `freeze_struct` hashes, `SelectiveMetagraphIndex` | M1 |
| the 7 `common/runtime/coinbase` files | extracted via `syn` | | M1–M2 |
| `docs/guides/signed-requests.mdx`, the three catalogs | vendored | | M0 |
| `eco-tests/**` (7) | n/a | | |
| `sdk/python/pyproject.toml` | extracted | version, `[project.scripts]` | |
| `sdk/python/completions/**` (3) | n/a | | M8 |
| `sdk/python/scripts/{record_golden, record_shape_corpus, build_basket_index}.py` | oracle-only | | |

The 11 ported unit tests are `test_rpc`, `test_http_auth`, `test_balance_properties`, `test_null_weights`,
`test_slippage_protection`, `test_root_claim_fee`, `test_hyperparams_units`, `test_fee_filters`, `test_multisig_safety`,
`test_prepare_attach` and `test_wallet_compat`.

Other edits:
- Add the last three rows (`pyproject.toml`, `completions/**`, the three scripts) to the §4.9 watched roots.
- Module-map scope: "220 package files plus 118 other watched files".
- Replace PLAN §12.2's file-layer bullet with: "seed `parity/map.toml` with every row of python-module-map.md (338) with
  `synced_blob = "pending"`. From M0, layer 1 checks presence (new or vanished files); it checks blob equality only for
  blessed rows. Strict mode (M8) requires zero pending rows."
- PLAN §12.4's bullet becomes "bless the rows ported so far".

## PP-07: partially_confirmed (minor, was major)

**Verified.** An AST scan of the package finds 28 literal environment-variable names plus the computed `BT_PW_*` family
(`keyfiles.py:104`). 13 of them never appear in architecture.md. `PY/wallet.py` reads no environment variable
(`DEFAULT_WALLET_PATH`, `:11`); `BT_WALLET_PATH` exists only as the CLI option at `cli/main.py:203-206`. §6.1 and the module-map
`wallet.py` row therefore misattribute it.

**Overstated.** Every name sits in a module the plan already ports (`config.py`, `evm/networks.py`, `extension/*`,
`cli/globals.py`, with "flag > env > btcli.json"). What is missing is an inventory and a gate, not a design component.

**What is wrong with the fix.** Its "all 27" seed list omits `BT_CHAIN_ENDPOINT` (`settings.py:38`) and `BITTENSOR_LOG`
(`cli/logs.py:32, 51`, read through the `ENV_VAR` constant), so the proposed gate would fail while seeding.

**Corrected fix.** Add a layer to §4.9:

> **Environment layer.** `xtask extract` collects into `vendor/upstream/extracted/env_vars.json` (name, file:line, literal
> default) every `os.getenv("X")`, `os.environ.get("X")`, `os.environ["X"]` and Typer `envvar="X"` in `PY/**`. A module-level
> string constant used as the name is resolved (`ENV_VAR`, `PY/cli/logs.py:32, 51`). `parity/env.toml` maps each name to an
> rtao consumer or a divergence id. The gate fails on an unmapped or vanished name. `cli_tree.json` records each option's
> `envvar`.

Seed `env.toml` with the 28 names plus `BT_PW_*`:

| Names | rtao consumer |
|---|---|
| `BTCLI_CONFIG`, `BTCLI_ADDRESSES_PATH`, `BTCLI_PROXIES_PATH`, `BTCLI_MULTISIGS_PATH`, `BTCLI_MULTISIG_CACHE`, `BTCLI_SUBNET_NAMES_CACHE`, `BTCLI_TOKEN_SYMBOLS_CACHE`, `BITTENSOR_RUNTIME_CACHE_DIR` | `rtao_client::config` and the runtime cache |
| `BT_CHAIN_ENDPOINT` | `rtao_client::Network` (the `local` preset) |
| `BT_NETWORK`, `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`, `BT_FALLBACK_ENDPOINTS`, `BT_ARCHIVE_ENDPOINTS`, `BT_RETRY_FOREVER`, `BT_SIGNER_ADDRESS`, `BT_EXTENSION_BROWSER`, `BT_EXTENSION_BRIDGE` | rtao-cli globals (`BT_EXTENSION_BROWSER` also `rtao_signer::extension`, `extension/browser.py:26`) |
| `BITTENSOR_LOG` | rtao-cli logging (§10.3) |
| `BT_WALLET_PASSWORD`, `BT_WALLET_PASSWORD_FILE`, `BT_PW_*` | `rtao_wallet` (`BT_WALLET_PASSWORD` also the `rtao_evm` keystore, `evm/keys.py:122`) |
| `BT_EVM_ENDPOINT` | `rtao_evm::EvmNetwork` |
| `BITTENSOR_EXTENSION_BRIDGE_TOKEN`, `BITTENSOR_EXTENSION_BRIDGE_PID` | `rtao_signer::extension` |
| `GH_TOKEN`, `GITHUB_TOKEN` | rtao-cli `upgrade` |
| `TERM_PROGRAM` | n/a |

Replace the §6.1 sentence with: "The default path is `~/.bittensor/wallets` (`PY/wallet.py:11`). `BT_WALLET_PATH` is read only
by the CLI's `--wallet-path` option (`PY/cli/main.py:203-206`), and rtao's library `Wallet` does the same." Make the same
correction in the module-map `wallet.py` row.

## PP-08: partially_confirmed (major)

**Verified.** Architecture §9.8 (line 3024) specifies `Reveal::AtRound | Reveal::InBlocks`. Python's `encrypt` takes
`reveal_in`, `reveal_at` or `reveal_round` and has no block form (`PY/timelock.py:254-304`). The rest of the module:
- `parse_duration` (`:103-129`); ISO-8601 with UTC as default (`:132-152`);
- `current_round`, `round_at`, `reveal_time` (`:155-178`);
- `TimelockNotReady` (`:72-87`); the 30 s grace (`:62, 339-358`);
- `encrypted` vs the envelope (`:229-240`).

btcli has `--in/--at/--round` (`cli/commands/timelock.py:135-146`). The PYT e2e uses `encrypt(reveal_round=)` and
`sealed.encrypted`.

**What is wrong with the fix.**
- It defaults `wait: false` for the async `open`. Python's async counterpart, `decrypt_async`, defaults to `wait=True`
  (`:361-377`).
- It omits `Timelocked::parse`.
- It lists only part of the CLI JSON keys (`:208-228`).
- It puts `current_round`/`round_at`/`reveal_time` in rtao-tx (L5, M7), but `NeuronCommitment.reveals_at`
  (`PY/metagraph.py:120-124`) needs `reveal_time` from rtao-reads in M1 (see PP-12).

**Corrected fix.** Replace the first two bullets of §9.8 with:

> `rtao_tx::timelock` ports `PY/timelock.py` (its whole `__all__`, `:380-392`):
> - **`seal(data, Reveal) -> Result<Timelocked, TimelockError>`**, with `enum Reveal { In(Duration), At(SystemTime), Round(u64) }`.
>   Exactly one form is given, as in `encrypt` (`:254-304`). `In` also parses Python's duration grammar (`:103-129`); `At` also
>   parses ISO-8601, where no offset means UTC (`:132-152`). A target round at or below `current_round()` is refused with
>   Python's message.
> - **`Timelocked { ciphertext, reveal_round }`**, with `parse(bytes | hex)` (trailer `AES_GCM_` + little-endian u64 round,
>   `:192-207`), `reveal_at()`, `remaining()`, `revealed()`, `hex()` (the portable envelope) and `encrypted()` (the inner TLE
>   bytes for `Commitments.set_commitment`, `:229-237`).
> - **`open(&Timelocked, &DrandClient, OpenOptions { wait, timeout })`**. Before the reveal time it returns
>   `TimelockError::NotReady { reveal_round, reveal_at, remaining }`; after it, it retries the beacon fetch every 1 s for up
>   to 30 s (`:307-358`). The async `open` defaults to `wait: true`, like `decrypt_async` (`:361-377`). `rtao::blocking`'s
>   `decrypt` defaults to `wait: false`, like the sync `decrypt`.
> - **Pure helpers.** `current_round`, `round_at`, `reveal_time`, `parse_duration` and `format_duration` live in
>   `rtao_primitives::drand`, over `rtao_core::timelock::constants::{GENESIS_TIME, DRAND_PERIOD}` (equal to Python's
>   `QUICKNET_GENESIS` and `QUICKNET_PERIOD`). They land in M1, are re-exported here, and are pinned by oracle vectors.
> - **Block-count form.** `Reveal::InBlocks(n)` is an rtao addition, registered in Appendix C with class `addition`.

`rtao timelock encrypt` keeps `--in`/`--at`/`--round` and Python's JSON keys: `ciphertext` (or `path` with an output file),
`reveal_round`, `reveal_at`, `reveal_in_seconds`, `bytes` (`PY/cli/commands/timelock.py:208-228`).

## PP-09: partially_confirmed (minor, was major)

**Verified.**
- `multisig_threshold_1`'s `build` derives the multisig account view and composes the inner intent
  (`PY/intents/multisig.py:59-84, 185-197`).
- `transfer_all` composes one call (`transfer.py:103-106`).
- Architecture §14 M5 generates all 46 bodies, while one of them is an M2 op and nine are M4 ops.

**Already handled.** The membership is resolved in parity-inventory §4.1-4.2 and in PLAN §12.6/§12.7. §4.3 only reports the
heuristic's count, so that half is a doc sync.

**Still open.** The generator milestone (PLAN §14.3 #2, §12.4 "decide"). M2 is the sound choice: regen already emits interfaces,
`AnyIntent` and `defaults.rs` in M2, so nothing is redone.

**Corrected fix.** Architecture.md §14:
- **M2** adds: "`xtask regen` emits `generated/passthrough.rs`, the `build()` bodies of the 46 pass-through ops in
  parity-inventory §4.1. Ops still in `pending.toml` keep their `NotYetPorted` stubs. `transfer_all` uses it."
- **M4** adds: "the 9 owner and registration pass-through ops (`burned_register`, `register_subnet`, `root_register`,
  `set_mechanism_count`, `set_subnet_identity`, `start_call`, `trim_null_subnet_batch`, `trim_subnet`, `update_symbol`) use the
  generated bodies".
- **M5** becomes: "the remaining 63 intents (36 generated pass-through bodies, 27 ported)".

Replace the first sentence of §4.3 with: "Checked against the `build()` bodies, 46 of 83 intents compose exactly one call whose
arguments are fields renamed only by suffix stripping, a generic origin default or a field-type conversion (parity-inventory
§4.1). The field-name heuristic of [J-MA §3.4] gets the count right but counts `multisig_threshold_1` and misses
`transfer_all` (parity-inventory §4.2)."

PLAN §12.4: replace "decide whether the generator lands now" with "`transfer_all` through the generated pass-through body", and
add the generator bullet. Mark PLAN §14.3 #2 and inventory §8 #2 resolved.

## PP-10: partially_confirmed (major)

**Verified.**
- `read_targets` scrapes only the fetch function's source plus closure cells (`generate.py:405-480`).
- Hits outside that scope: `_lock_record` (`locks.py:33-40`); module-level `Item(...)` (`collateral.py:15-17`); module-level
  `Item`/`Method` constants (`root.py:28-44`).
- Inventory §5.15 lists 12 affected reads, and the test in §4.8/§11.3 fails for 7 of them. PLAN §14.3 #3 is open.

**What is wrong with the fix.** "Record == extracted hits for every read" compares one run against a static all-branches set.
Reads with branches touch different items depending on parameters (`miner_collateral` reads `Owner` only when `coldkey_ss58` is
omitted, `collateral.py:86-92`), so the equality would fail spuriously.

**Corrected fix.** Replace the "Exact hits" paragraph of architecture §4.8 with:

> `xtask extract` computes each read's static hit set into `vendor/upstream/extracted/read_hits.json`. It scans
> `PY/reads/<module>.py` helper-aware: the read function plus the module-local functions it calls, transitively, resolving
> `st.`/`api.`/`constants.` chains and module-level `Item(...)`/`Method(...)`/`Constant(...)` literals.
>
> Items reached outside the module go in `parity/read_hits_extra.toml`, each with a reviewed reason. Examples are view methods
> (`view.timestamp()` → `Timestamp.Now`) and `PY/metagraph.py` `fetch_commitments`.
>
> Let H = extracted ∪ extra. The checks:
> - the catalog layer asserts `CAT` hits ⊆ H;
> - the `RecordingView` test asserts that every recorded run touches a subset of H;
> - it also asserts that the union of runs over the read's samples equals H. The samples are `READ_SAMPLES` plus per-branch
>   samples, for example `miner_collateral` with and without `coldkey_ss58`.
>
> `ReadSpec.hits` and `verify_surface` use H.

Update §11.3's "exact hits" bullet to match. Propose the helper-aware scan upstream for `generate.py`. Milestone M2, before the
first read enters the test. Mark PLAN §14.3 #3 and inventory §8 #7 resolved.

## PP-11: confirmed (major)

Verified:
- 11 ops have no `intent_*` name. Three are covered elsewhere: `pow_register` (PYT e2e and #14), `add_collateral` (#11) and
  `trim_null_subnet_batch` (#4). The other 8 have no localnet test (inventory §2.5, §8 #9).
- PLAN §14.3 #4 defaults to "oracle vectors only". The Sudo-wrapped `set_root_claim_threshold` and
  `set_subnet_emission_enabled` therefore never execute, and that dispatch hits the `Sudid` receipt path PLAN §14.3 #14 flags.
- The §4.9 `e2e_map` row covers only the 112 manifest names.
- `PYT/e2e/test_timelock_commitment.py::test_timelocked_commitment_reveals_via_python_sdk` matches none of scenarios 1-17.
- Read #73 `root_claim_threshold` is M6 (inventory line 597), while its op is M5 (verify read at `root.py:110`; tx help at
  `cli/tx.py:249-250`).

The fix is concrete and consistent. Also mirror it in PLAN §12.7 (scenario 18) and §12.9 (scenario 19).

## PP-12: partially_confirmed (major)

**Verified.**
- The curated `metagraph`/`commitments` call `metagraph_module` (`PY/namespaces.py:161-188`).
- The `commitments` read calls `fetch_commitments` (`reads/identity.py:9, 139-162`).
- Architecture §2.3 has rtao-metagraph (L6) depend on rtao-reads.
- The M1 exit criterion uses `client.subnets().metagraph(NetUid(1))`.
- PLAN §14.3 #5 has only an [I] default.

**What is wrong with the fix.**
- Its signature `fn metagraph(&self, netuid, MetagraphOptions {...})` does not compile as `client.subnets().metagraph(NetUid(1))`.
- `NeuronCommitment.reveals_at` calls `timelock.reveal_time` (`PY/metagraph.py:25, 120-124`). The plan homes that function in
  `rtao_tx::timelock` (L5, M7), which rtao-reads cannot use. The core constants equal Python's (`CORE/timelock/constants.rs`).

**Corrected fix.** Decide in architecture §2.2/§4.8/§8.2:
- **`rtao_reads::commitments`** owns `NeuronCommitment`, `CommitmentMap`, the status machine and the decoders
  (`PY/metagraph.py:37-124, 406-560`), plus `fetch_commitments`/`fetch_commitment_map` (`:258-291`). The `commitments` read and
  the curated `Subnets::commitments()` live there. rtao-metagraph uses it for `Metagraph.commitments`.
- **`rtao_primitives::drand`** holds `reveal_time`, `round_at` and `current_round` (`PY/timelock.py:155-178`), over
  `rtao_core::timelock::constants::{GENESIS_TIME, DRAND_PERIOD}`. They land in M1 and `rtao_tx::timelock` re-exports them.
- **`rtao_metagraph::SubnetsMetagraphExt`** is the curated metagraph: `async fn metagraph(&self, netuid: NetUid) ->
  Result<Option<Metagraph>, ReadError>` (with commitments, Python's default) and `async fn metagraph_with(&self, netuid: NetUid,
  o: MetagraphOptions) -> Result<Option<Metagraph>, ReadError>`. It is implemented for `rtao_reads::Subnets<'_, V>` and
  re-exported in `rtao::prelude` under feature `metagraph`.
- **The raw `metagraph` read** stays in rtao-reads (runtime call plus `utf8_text`, `PY/reads/subnets.py:170-189`).

Module map: the commitment parts of `metagraph.py` → `rtao_reads::commitments` (M1); `timelock.py`'s pure helpers →
`rtao_primitives::drand` (M1). Mark PLAN §14.3 #5 resolved.

## PP-13: partially_confirmed (minor, was major)

**Verified.**
- Citations: `HYPERPARAMS` is at `:110-408` and `RATIO_TYPE_DENOMINATORS` at `:32-37`.
- The layering conflict is real: L2 functions read an L3 table.
- `kind_of`/`codec_of` read `STORAGE_ITEMS` `value_type_ident` (`hyperparams.py:474-499, 663-679`).

**Overstated.** M1 and M2 call only `ratio_fraction("PerU16", x) = x / 65535` (`metagraph.py:346`, `reads/weights.py:73`,
`delegation.py:25-28`). The §5.4 ratio types in rtao-primitives (M0) already cover that. PLAN §14.3 #6's default is the same
decision as the fix.

**What is wrong with the fix.**
- It keeps "functions `:581-770`", but `metadata_kind`/`kind_of`/`normalized`/`annotate`/`value_forms` sit at `:474-578`.
- It makes `proportion_to_raw` a table method. That function is table-free and used by M5 intents (`children.py:29, 80`).
- It places the extracted `RATIO_TYPE_DENOMINATORS` in rtao-primitives, which cannot read rtao-catalog.

**Corrected fix.** Replace the module-map row `hyperparams.py` with:

> | `hyperparams.py` | 769 | `rtao_primitives::ratio::{ratio_fraction, RATIO_TYPE_DENOMINATORS}` (derived from the §5.4
> ratio types; a catalog-layer test asserts equality with the extracted `RATIO_TYPE_DENOMINATORS` `:32-37`) and
> `rtao_primitives::hyperparams::proportion_to_raw` (`:581-621`) (M1); `rtao_primitives::hyperparams::{Hyperparam,
> HyperparamTable}`, with `metadata_kind`, `kind_of`, `doc_of`, `short_of`, `normalized`, `annotate`, `value_forms`, `codec_of`,
> `raw_bounds` and `to_raw` as methods on `&HyperparamTable` (`:474-769`) (M4); data generated into
> `rtao_catalog::hyperparams::TABLE: HyperparamTable` from `HYPERPARAMS` `:110-408`, `STORAGE_ITEMS` `:416-461` (each item
> resolved at extraction to its `value_type_ident` and cross-checked against the pinned metadata), `_IDENT_KINDS` `:467-471`,
> `_IDENT_CODEC` `:626-643`, `_KIND_CODEC` `:646-657` and `_FRACTION_DENOMINATOR` `:83`; `rtao_intents::hyperparameters::to_raw`
> is a `pub use` | extracted literal table + ported | `extracted` | M1–M4 | consumers before M4: `metagraph.py:346` (M1),
> `reads/weights.py:73` (M2), `reads/delegation.py:25-28`, `intents/children.py:29, 80` (M5) |

Other edits:
- Mark the Gaps row resolved.
- Architecture §5.5: "ports `PY/hyperparams.py:474-769` as methods on `HyperparamTable`; the table is generated into
  `rtao_catalog::hyperparams::TABLE`".
- Fix the citations: §5.4 `:32-37`; §4.1 `:110-408`.
- §2.2: rtao-catalog `hyperparams::TABLE`; `rtao_intents::hyperparameters::to_raw` as a re-export.
- PLAN: mark §14.3 #6 resolved, and add `ratio_fraction` to §12.3 (M1).

## PP-14: partially_confirmed (minor, was major)

**Verified.** `config.py` manages 7 files (`:12-18`), each with an environment override (`:59, 139, 212, 274, 327, 355, 416`).
§10.2 omits `btcli_multisig_cache.json` and 5 of the overrides. PLAN §14.3 #8 has only an [I] default.

**Overstated.** btcli resolves a pending call from the cache first, then `--call-data`, then the opening `as_multi` at the
timepoint (`PY/cli/multisig_helpers.py:907-975`, through `client.block_info`). Without the cache, rtao can still finish a
multisig that btcli opened, unless the block is pruned and no archive is configured.

**Corrected fix.** Replace the shared-file bullets of architecture §10.2 with:

> rtao reads and writes btcli's seven files with the same schemas and environment overrides (`PY/config.py:12-18, 59, 139, 212,
> 274, 327, 355, 416`):
>
> | File | Override | Note |
> |---|---|---|
> | `btcli.json` | `BTCLI_CONFIG` | |
> | `btcli_addresses.json` | `BTCLI_ADDRESSES_PATH` | |
> | `btcli_proxies.json` | `BTCLI_PROXIES_PATH` | |
> | `btcli_multisigs.json` | `BTCLI_MULTISIGS_PATH` | |
> | `btcli_multisig_cache.json` | `BTCLI_MULTISIG_CACHE` | call specs keyed by call hash (`:326-352, 470-480`) |
> | `btcli_subnet_names.json` | `BTCLI_SUBNET_NAMES_CACHE` | 24 h |
> | `btcli_token_symbols.json` | `BTCLI_TOKEN_SYMBOLS_CACHE` | 7 d |
>
> A pending multisig's call resolves as in btcli: cache, then `--call-data`, then the opening `as_multi` at the timepoint
> (`PY/cli/multisig_helpers.py:907-975`).

Homes:
- **`rtao_client::config` (M1):** paths with their overrides; `SETTABLE`, extracted with the builtin type tags of §4.9 layer 4;
  `btcli.json` load and save; the token-symbol and subnet-name caches.
- **`rtao-cli::books`:** readers in M4, for `*_ss58` resolution in `tx` and `call`. Writers ship with their commands: the
  multisig book and multisig cache in M5, the proxy and address books in M8.

Interop lane 7 adds: a multisig opened by btcli is approved and executed by rtao through the shared cache, and again through the
timepoint fallback with the cache removed; and the reverse. Update the module-map `config.py` row and its Gaps row, and mark
PLAN §14.3 #8 resolved.

## PP-15: confirmed (minor)

Verified:
- `basket_index` is CLI-only: `cli/root_helpers.py:18` (`staker_yield`), `cli/commands/root.py:23, 156, 240` (`index_level`
  feeds the JSON `basket_index`), `root_move.py:11`. `reads/root.py:81` mentions it only in a docstring.
- The module's own docstring keeps it as the fallback for pre-v3 nodes and historical blocks, so "drop it" would break
  `root list --json`.
- `basket_index_data.py` passes the literal subset (6/6 tables).
- rtao-client depends on rtao-catalog (§2.2), so data placed in rtao-catalog would compile into every neuron build.

The fix is correct. Also change PLAN §12.8's "decide `basket_index.py`" to "port".

## PP-16: confirmed (minor)

Verified:
- The bridge is an HTTP and WebSocket server (`extension/bridge.py:1, 14, 19`); the client is WebSocket (`client.py:11`).
- `ensure_bridge` spawns `sys.executable -m bittensor.extension.daemon` (`session.py:121-140`). The token and pid files have
  environment overrides (`tokens.py:14`, `session.py:24`).
- The bridge API is in `__all__` (`PY/__init__.py:198-206`).
- Rule 2 (§2.3) forbids tokio-tungstenite outside rtao-rpc and rtao-testkit, and §6.5 never says what rtao spawns.

The fix decides both. Also mirror the rule change in PLAN §4.2 rule 2 (line 199).

## PP-17: confirmed (minor)

Verified:
- `CORE/transaction.rs:251-826` defines 22 `pub fn` constructors. Four have no catalog op: `transfer_allow_death` (`:285`),
  `move_stake_limit` (`:499`), `transfer_stake_and_hotkey` (`:603`) and `set_root_claim_type` (`:720`).
- Three of them are call variants of existing intents:
  - `Transfer { keep_alive: false }` → `transfer_allow_death` (`transfer.py:52-63`);
  - a cross-subnet `MoveStake` with slippage protection → `move_stake_limit` (`staking.py:848-874`);
  - a `TransferStake` with a different destination hotkey → `transfer_stake_and_hotkey` (`staking.py:1487-1499`).
- Spec-477 `SubtensorModule` has 85 calls and none is `set_root_claim_type`.

The fix is correct. Also update PLAN §12.4 ("18 paired constructors plus 4 hand-built subjects").

## PP-18: confirmed (minor)

Verified:
- The module map tags vendored assets as `extracted` with an [I] note (`python-module-map.md:28`), across 24 rows.
- §4.9 layer 4 and the auto-bless rule both assume extracted literal tables, which byte copies do not have.
- PLAN §14.3 #13 defaults to `extracted` without defining what that means for a byte copy.

The fix is correct.

## PP-19: partially_confirmed (minor)

**Verified.** The architecture text is stale:
- §10.6 M4 includes `set-take` and `stake-burn`;
- `inspect` appears in both M4 and M8;
- "proxy, multisig basics" is undefined;
- §11.2 and §14 say "73";
- §14 M6 says "all 102 reads";
- scenario 11 includes `add_collateral`.

**Mostly handled.** PLAN §12.1 states that item assignments follow parity-inventory, and inventory §8 #3, #4, #6, #8 and #11
already resolve these; PLAN §12.6 and §12.7 encode the results.

**Still open.** `wallet create` (inventory §8 #5: "followed the text, likely an omission"). It is `new-coldkey` plus
`new-hotkey` (`PY/cli/commands/wallet.py:264-327` → `wallets.create`, `:214-246`), so M4 is right.

**Corrected fix.**

§10.6:
- **M4:** "globals, `config` (5), `doctor`, `localnet`, `explain`, `tools`, `call`; generated `tx`/`query` for every implemented
  op and read; `wallet` create, new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub, regen-hotkeypub, show,
  list, balance, inspect; `weights` set, commit, reveal; `axon` set, reset; `hparams` set, get, get-take, check-start, start,
  set-symbol, get-identity, set-identity, trim, trim-null-batch, trim-null, mechanisms count, mechanisms emissions; `subnets`
  register, create, metagraph (40 commands)".
- **M5:** "generated `tx` complete (83); `stake` add, remove, move, move-swap, transfer, swap, burn; `hparams` set-take,
  stake-burn; `proxy` create, add, remove, kill, list, execute; `multisig` add, list, show, remove, pending (20 commands)".
- **M8:** drop `wallet inspect` and `wallet create` (45 commands).

§14 and §11.2:
- M4 names `set_subnet_identity`, `update_symbol`, `set_mechanism_count`, `trim_subnet`, `trim_null_subnet_batch`.
- M4 exit: "shielded scenario 11 green for `burned_register`".
- M5 exit: "the 72 live `intent_*` names green; `intent_set_root_claim_type` (retired, absent from spec-477 metadata) mapped n/a".
- M1–M6 list reads per inventory §1 (2/13/1/22/1/63).
- §11.2: "the 72 live intent-plan tests collapse into one table-driven test over `AnyIntent`; `intent_set_root_claim_type` is
  mapped n/a".

Propagate `wallet create` → M4:
- inventory §1: CLI cumulative 40/60/84/118/163; M8 45;
- inventory §7.2 wallet `create` → M4; §8 #5 resolved;
- PLAN §8.2; PLAN §12.6 (40 commands, add `create`); PLAN §12.10 (45 remaining; `wallet` 16).

## PP-20: confirmed (minor)

Verified:
- `from_args` ignores `"op"` (`PY/intents/base.py:366-372`).
- `to_dict` emits `"op"` and documents an exact round-trip (`:357-364`).
- `registry.build` calls `from_args` (`registry.py:34-40`).
- Architecture §4.2 uses `deny_unknown_fields` with a comment claiming parity, and §4.5 `from_tool` is serde plus `validate()`,
  so an `"op"` key is rejected. The plan has no intent `to_json`.

The fix is correct.

## PP-21: confirmed (minor)

Verified:
- Nested module errors resolve through the static `ERRORS` table (`PY/result.py:636-656`).
- Top-level module errors resolve through the live codec (`receipt.py:55-65`, `_substrate.py:796-810`).
- Named-error messages come from `_CATALOG_DOCS_BY_NAME` (`:35, 565-571`), where the last pallet wins. 10 names have different
  docs across pallets.
- 22 of the 29 custom transaction names are module-error names.
- Architecture §4.7 says "never through a stale table", registers no divergence, and names no message source.

The static table equals spec-477 metadata (405/405), so the difference appears only across upgrades and on historical blocks.
The fix is correct. Suggested id: D-028.

## PP-22: partially_confirmed (minor)

**Verified.**
- `list_tools` has 9 keys (`registry.py:43-66`).
- The Python CLI has no `--period` and no `allow_raw`.
- btcli's `Client` has no `Policy` (`cli/context.py:1990-1994`).
- `--signer` accepts wallet, extension, ledger and vault (`globals.py:276-286`).
- Appendix C has no rows for any of these.

**Overstated.** §11.6 lane 8 already compares `tools` on "upstream fields", so the `rust` key does not break or weaken it.

**What is wrong with the fix.** It leaves `--allow-raw` undecided ("drop it, or register it as stricter").

**Corrected fix.**
- Drop `--allow-raw` from `rtao call`. btcli has no `Policy` (`PY/cli/context.py:1990-1994`); the library's
  `Policy.allow_raw_calls` is unchanged.
- §10.1 then reads: "`call Pallet.function --args '{…}' [--sudo] [--multisig NAME]` is a raw call (no `Policy` in the CLI, as
  btcli) that uses the same wrapper nesting as intents". parity-inventory §7.1 `call` row: "raw call (no policy, as btcli)".
- Appendix C:

| ID | Upstream symbol | Python behavior | rtao behavior | Class | Test | Upstream |
|---|---|---|---|---|---|---|
| D-025 | `btcli tools` / `list_tools` | 9 keys | adds `rust {type, role, output, cli}` | addition | `tools_json_equals_python_on_upstream_fields` | — |
| D-026 | `btcli tx` | no era flag (period 128) | `--period <n>` | addition | `tx_period_flag` | — |
| D-027 | `--signer` | wallet, extension, ledger, vault | adds `remote` | addition | `signer_remote_flag` | — |

## PP-23: partially_confirmed (minor)

**Verified.**
- PLAN §12.10 (M8) lists the prompt module and the aliases. The module map puts `_live.py` and `masked_input.py` in M4, and
  inventory §7.3 lands each alias with its group.
- M4 wallet commands prompt for passwords (`keyfiles.py:42-62` uses `masked_input`; `wallet.py:27, 202, 821`).
- PLAN §12.4 claims the whole chain `PY/wallets.py:62-118`, but steps 5-6 import `macos_password` (`:98-115`), which the module
  map puts in M7.
- `--keychain-password` and `--macos-password` are M4 globals (`globals.py:257, 267`).

**What is wrong with the fix.** It leaves the macOS part undecided ("move it to M4, or error until M7").

**Corrected fix.**
- **PLAN §12.6 (M4)** adds "the CLI prompt module (`_live.py`, `masked_input.py` equivalents: `rpassword`,
  `PromptPolicy::IfTty`, re-ask on empty or mismatched input as in `PY/keyfiles.py:42-62`) and `macos_password.py` (Keychain read
  and write through `/usr/bin/security`, osascript dialog) behind `--keychain-password`/`--macos-password`".
- **PLAN §12.10 (M8):** replace "Hidden v9 aliases and snake_case aliases; …; the CLI prompt module (…)" with "verify every
  group's hidden v9 and snake_case aliases against `cli_tree.json` (each alias lands with its group, parity-inventory §7.3);
  `PyJsonFormatter`; shell completions".
- **PLAN §12.4 (M2):** "the password chain of `PY/wallets.py:62-118` (steps 1-4 and 7 in M2; Keychain and the macOS dialog,
  steps 5-6, in M4)".
- **PLAN §12.9 (M7):** "`wallet keychain` save/show/delete over the M4 Keychain code; optional Linux `keyring`".
- **Module map:** `macos_password.py` → M4.

## PP-24: partially_confirmed (minor)

**Verified.** The 7 tables exist at `result.py:27, 31, 35, 36` (5 entries), `:291` (10), `:308` (42) and `:475` (1), and none of
them appears in the inventory §6 tracking table.

**Overstated.** Architecture §4.1 already lists `PY/result.py:27-36, 78, 116, 291, 308, 475` as extracted, and PLAN §12.4
extracts `PY/result.py` in M2, so the tables do have a home and a milestone. Only the inventory's table is incomplete.

**Corrected fix.** Add these rows to parity-inventory §6, "Tables that travel with the codes":

| Table | Python source | rtao home | How | M |
|---|---|---|---|---|
| per-name remediation overrides (42) | `PY/result.py` `_NAME_HELP_OVERRIDES` (`:308`) | `rtao_catalog::py` | extracted | M2 |
| substring remediation override (1) | `_HELP_OVERRIDES` (`:475`) | `rtao_catalog::py` | extracted | M2 |
| no-name classification needles (10) | `_SUBSTRING_FALLBACK` (`:291`) | `rtao_catalog::py` | extracted | M2 |
| watch-status fatals (5) | `_WATCH_STATUS_RE` (`:31`), `_WATCH_STATUS_NAMES` (`:36`) | `rtao_catalog::py` (regex source string), used by `rtao_tx::classify` | extracted | M2 |
| custom-error regex | `_CUSTOM_ERROR_RE` (`:27`) | `rtao_catalog::py` (regex source string), used by `rtao_tx::classify` | extracted | M2 |
| named-error message text | `_CATALOG_DOCS_BY_NAME` (`:35`, a comprehension over `_generated/errors.py`) | derived in `rtao_runtime::generated::errors` from `MODULE_ERRORS`, pallet-index order, last name wins | generated | M2 |

## PP-25: partially_confirmed (minor)

**Verified.** These names have 0 hits in all four documents: `BlockHeader`, `BlockInfo`, `EpochEvent`, `close_shared_clients`,
`UnitMismatchError`, `RpcSubstrate`, `WalletLike`, `parse_crypto_type`/`format_crypto_type`, the extension functions and
`_REMOVED_V10_HINTS`. The symbol layer is diff-only, so nothing checks that every export has a home.

**What is wrong with the fix.**
- It maps `BlockInfo` to M6, but `multisig pending` (PLAN M5) recovers call data through `fetch_call_from_timepoint` →
  `client.block_info` (`PY/cli/multisig_helpers.py:945-975`, used at `:1422`).
- It lists rows only as examples ("e.g.") rather than one row per name.

**Corrected fix.** Add to python-module-map.md a section, "Top-level exports (`PY/__init__.py:122-219` `__all__`)":

| `__all__` names | rtao home | Milestone |
|---|---|---|
| `Subtensor`, `subtensor`, `SyncClient`, `set_weights`, `close_shared_clients` | `rtao::blocking` | M6 |
| `Client`, `Snapshot` | `rtao_client` | M1 |
| `Substrate` | `rtao_client::View` + `Client` over a custom `RpcClientT` | M1 |
| `RpcSubstrate` | `rtao_rpc::Session` | M1 |
| `BlockHeader` | `rtao_client::BlockHeader {number, parent_hash, raw}` | M1 |
| `BlockInfo` | `rtao_client::BlockInfo` | M5 |
| `EpochEvent` | `rtao_client::EpochEvent {netuid, block, previous}` | M2 |
| `Metagraph`, `MetagraphNeuron` | `rtao_metagraph` | M1 |
| `NeuronCommitment` | `rtao_reads::commitments` | M1 |
| `Neuron`, `SubnetInfo`, `StakePosition`, `StakeValuation`, `DelegateInfo`, `DelegatedStake`, `Commitment`, `SwapQuote` | `rtao_reads` models | milestone of their read (inventory §5) |
| `Balance`, `tao`, `alpha`, `rao` | `rtao_primitives::DynBalance` constructors | M0 |
| `UnitMismatchError` | `rtao_primitives::UnitError::UnitMismatch` | M0 |
| `ExtrinsicResult`, `BittensorError`, `ChainError`, `ConnectionNotReady`, `ErrorCode`, `PolicyError`, `RpcConnectionError`, `RpcPolicyError` | `rtao_tx` / `rtao_catalog` | M2 |
| `Intent`, `Plan`, `Policy` | `rtao_tx` | M2 |
| `Multisig` | `rtao_tx::multisig` | M4 |
| `Signer`, `KeyedWallet`, `WalletLike`, `WalletSigner`, `public_view`, `resolve_signer`, `SigningContext` | `rtao_signer` | M2 |
| `UnsignedExtrinsic` | `rtao_client` | M5 |
| `Wallet`, `wallet`, `wallets` | `rtao_wallet` | M2 |
| `Timelocked`, `TimelockError`, `TimelockNotReady`, `timelock` | `rtao_tx::timelock`, pure helpers in `rtao_primitives::drand` | M7 / M1 |
| `BridgeClient`, `BridgeError`, `BridgeServer`, `ExtensionAccount`, `ExtensionSigner`, `connect_extension_signer`, `ensure_bridge`, `run_bridge`, `stop_bridge_daemon` | `rtao_signer::extension` | M7 |
| `LedgerSigner`, `LedgerError`, `VaultSigner`, `VaultError` | `rtao_signer` | M7 |
| `CRYPTO_ED25519`, `CRYPTO_SR25519`, `DEFAULT_CRYPTO_TYPE`, `parse_crypto_type`, `format_crypto_type` | `rtao_primitives::CryptoType` | M0 |
| `reads`, `intents` | crate roots | M1–M6 / M2–M5 |
| `evm` | `rtao_evm`; h160↔ss58 in `rtao_primitives::evm` | M7 |
| `http_auth` | `rtao_btauth` | M0 |
| `storage`, `runtime_api`, `constants`, `calls` | `rtao_runtime` | M1 |
| the 83 intent classes (`_INTENT_EXPORTS`) | `rtao_intents::<Op>` | per parity-inventory §3 |
| `_REMOVED_V10_HINTS` | n/a (migration guide) | |

Make `xtask parity check` assert that every `__all__` name has a row here or a `#[doc(alias)]` hit.
