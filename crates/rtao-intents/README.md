# rtao-intents

[![crates.io](https://img.shields.io/crates/v/rtao-intents.svg)](https://crates.io/crates/rtao-intents)
[![docs.rs](https://img.shields.io/docsrs/rtao-intents)](https://docs.rs/rtao-intents)
[![license](https://img.shields.io/crates/l/rtao-intents.svg)](https://github.com/sam0x17/rtao/blob/main/NOTICE)

Typed Bittensor transaction intents and the catalog dispatcher used by
`rtao_tx::Executor`. Construct an intent from a public type, or use `Registry`
to parse an enabled JSON tool operation. `executor(client)` installs that
registry on an executor sharing the client's nonce manager.

The generated interfaces retain upstream parameter descriptions, origin roles
and metadata. Execution goes through the transaction layer's planning, policy,
signing and submission checks. A catalog entry may still return an explicit
not-yet-ported error; generated presence alone does not certify behavior.

Enable operation families individually: `association`, `batch`, `children`,
`coldkey`, `collateral`, `crowdloan`, `emission_weights`, `evm`, `governance`,
`hyperparameters`, `identity`, `leasing`, `lock`, `multisig`, `proxy`,
`registration`, `root`, `serving`, `staking`, `transfer` and `weights`. The `all` feature enables these
families. `governance` also enables `hyperparameters`.

`commitments` adds `commitments::SetCommitment`, a typed
`Commitments.set_commitment` signed by the hotkey. It is an rtao addition
with no Python intent, so it is outside the generated catalog and
`AnyIntent`; `all` includes it.

`pow` enables the upstream host solver and asynchronous registration support;
`pow-gpu` adds its optional backend selection. Both are separate from `all`.
`multisig` provides adapters that preserve the semantic intent while handling
approval continuations. `weights` includes normalization, quantization and
commit-reveal construction; network scheduling belongs to `rtao-neuron`.

Use checked native balances, inspect the resulting plan and bound spend, fees
and subnet scope before execution.
