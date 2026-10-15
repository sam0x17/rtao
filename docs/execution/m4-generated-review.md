# Recovered generated-command review, 2026-10-15

The tx/query candidate is not adopted. Its recovery patch SHA-256 is
`24307c1782043905207b1e27cde2b01e00e1f89b5306ce04fa431ab89c8a11b3`
(`m4-generated-commands.1M4CZP/proof/generated-commands.patch`). Review was
source-only; no builds, tests, source copies or upstream filings were made.

Three concrete corrections are needed:

- Resolve plural account fields such as `coldkey_ss58s` through their scalar
  account kind before query dispatch. The candidate's CSV-contact regression
  expects AccountIds, but both current resolution branches leave these strings
  untouched. This affects balances, hotkey identities and coldkey stake values.
- Do not assert that pending `add_stake` is uncompiled in all-feature tests.
  Hosted workspace checks enable staking. Assert pending availability separately
  from feature compilation; test a disabled-feature graph in a scoped hosted job.
- Bind late PoW refresh to the reviewed effective origin, including
  `ExecuteOptions.proxy_for`. A proof expiring while a delegate unlocks is
  currently refreshed for the delegate, then refused by the real owner's seal
  validation. Cover distinct delegate/real identities and expiry during unlock.

The patch changes generators but omits regenerated APIs, including
`interface_metadata`, `InterfaceMetadata` and `ReadName::implemented`.
Generate and review these outputs in hosted CI before compiling dependent
commands. Merge command registration into current main and preserve recursive
leaf-global selection, saved multisig opening guards and retry suppression.
The review card still omits preflight facts; external signer factories remain
unconnected in the proposed binary. Both are explicit incomplete work.

Availability filtering itself is sound: register transactions only when compiled
and implemented, expose only implemented reads, and retain null CLI metadata for
pending tools. Source review does not establish combined CLI acceptance.
