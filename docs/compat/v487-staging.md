# Runtime 487 staging (D-125)

> Landed: Finney went from 486 to 488, which contains 487's changes, and the
> branch merged with the v488 parity pin (see `v488-upgrade.md`). The text
> below is the staging record as written.

Branch `stage-487` carries the hand-written parts of the runtime 487 surface so
the 487 pin bump only has to merge it. Runtime 487 is upstream subtensor PR
#3294; it is not merged or enacted on Finney yet, so nothing here is on `main`.

## Why it waits for the pin

`ProxyType` is a hand-written model whose variant list is reconciled against
the pinned metadata (`codegen/models.toml`, `xtask regen --check`), the vendored
chain source (`xtask extract`, `extract_proxy_contract.rs`) and the
authenticated Python `PROXY_TYPES`. Adding variants the pinned 486 metadata,
source and Python pin do not have fails those checks, so the variants can only
land together with the 487 metadata, source and Python pins.

For the same reason one unit test fails on this branch until the pin:
`rtao_intents::proxy::tests::proxy_builds_match_native_variant_and_integer_boundaries`
builds every `PROXY_TYPES` name against the pinned metadata and reports
`no variant named "Lock"` under 486. Checked on this branch: `rtao-runtime`
model tests pass; `xtask regen --check` and `xtask extract --check` are
expected to fail until the 487 pins.

Nothing in the meantime depends on this branch (D-123): under 487 metadata a
typed `Proxy` call with an existing `ProxyType` variant composes from live
metadata by name, a call with no compiled descriptor composes with
`rtao_runtime::compose_by_name` or `rtao call Pallet.function`, typed storage
reads decode by name, and the owner monitor reads `Proxy.ProxyAdded` by field
name. What the branch adds is the ability to name the new variants in typed
code.

## What the branch changes

- `rtao_runtime::models::ProxyType` gains `Lock` (index 21) and `GammaSpend`
  (index 22), with their names in `to_dynamic`.
- `codegen/models.toml` lists both variants for `ProxyType`.
- `rtao_intents::proxy::PROXY_TYPES` lists both names (23 entries) and the
  validation message names them.

## What the pin bump does itself

- `xtask regen` from the pinned 487 metadata regenerates every descriptor. The
  codegen scope is pallet-wide for `Utility`, `Multisig`, `Proxy`,
  `LimitOrders`, `SubtensorModule`, `AdminUtils` and `EVM`, so the new calls
  (`Utility.as_safe_derivative` and `as_protected_derivative`, the `Multisig`
  `*_safe` calls, `Proxy.create_safe_pure`, `create_protected_pure` and
  `kill_safe_pure`, `LimitOrders.authorize_order`, and the others) and the
  changed `Proxy.*` and `LimitOrders` order-format shapes are generated, not
  hand-written. The new `HashedAccounts` pallet is outside the Python covered
  set and stays out of scope; its calls compose by name.
- Merge `stage-487` together with the 487 pin, then run the usual pin
  checks. The provenance test does not require the 487 `:code` hash in
  `REVIEWED_CR_RUNTIME_CODE` (D-122): it still checks that the pinned reveal
  path decodes the version 4 payload without reading the stored tag, and
  adding the hash only changes what is reported.
