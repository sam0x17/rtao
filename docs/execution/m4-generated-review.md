# Generated tx/query preparation, 2026-10-15

Corrected tx/query source and its entrypoint/context integration are present.
The recovery patch SHA-256 is
`24307c1782043905207b1e27cde2b01e00e1f89b5306ce04fa431ab89c8a11b3`;
the patch is not retained in the repository. Only selected source was restored. No build cache or disposable checkout was recreated.

Source review led to these corrections:

- Plural account fields resolve each reference through its scalar coldkey or
  hotkey kind before dispatch. Fixtures cover balances, hotkey identities and
  coldkey stake values, including order, duplicates and invalid-entry refusal.
  Pinned Python leaves these CSV account arrays unresolved; this additive
  behavior is registered as D-043. Hosted generation supplied its declared
  parity matrix entry, with execution and oracle status still unverified.
- Availability fixtures distinguish pending behavior from feature compilation,
  including workspace `--all-features`. Pending operations remain hidden.
- Late PoW refresh uses the reviewed effective origin, including the real
  account in `ExecuteOptions.proxy_for`, after confirmation and key unlock.
  A fixture covers distinct delegate/real identities and expiry during unlock.
- Initial expired, future or difficulty-invalid work now refreshes through a
  public-only preparation hook before strict SDK planning. Review and execution
  receive the prepared intent. A three-case submission fixture checks the
  independently verified proof, proxy owner and explicit hotkey. A separate
  dry-run fixture installs a mining sentinel and requires refusal without
  signing. D-044 records the earlier actual refresh and the stale-preview
  refusal compared with Python; the SDK's challenge validation is unchanged.

The generated-command fixtures also correct an unsupported root proxy flag
and supply an explicit public hotkey to the numeric PoW parser fixture. The
prepared context obtains plan and preflight facts from the same snapshot and
call through `plan_and_preflight_dyn`, then includes those facts in human review.
Entrypoint integration retains recursive leaf-global selection, D-041's saved
multisig opening guard and retry suppression, and D-042's input budgets.

Generator changes introduce `interface_metadata`, `InterfaceMetadata` and
`ReadName::implemented`. Hosted run 38686769925 generated the four interfaces,
which were digest-checked, reviewed and adopted byte for byte, followed by the
read crate's root metadata reexport. All 83 intent and 102 read names are covered.
Combined CLI compilation still needs the normal hosted checks. Transactions require compiled, implemented behavior;
queries expose implemented reads; pending tools retain null CLI metadata.

External signer factories remain unconnected in the prepared entrypoint, and
rich staking source/target pickers remain incomplete behind pending intents.
Installed `rustfmt` checks passed for the prepared command files. No compilation
or tests have run for this integration, and no M4 acceptance is claimed.

The same entrypoint now routes `weights set/commit/reveal`, `axon set/reset`
and the implemented `hparams` family through real SDK intent/read paths.
Eleven weights/axon and six hyperparameter fixtures await hosted execution.
Sequential `hparams trim-null`, wallet and subnet workflows still need their
remaining source integration; no command placeholder is counted as complete.
