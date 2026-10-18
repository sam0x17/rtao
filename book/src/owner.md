# Subnet owner operations

`OwnerOps` coordinates subnet creation, activation, hyperparameters, public
identity, symbols, mechanism counts and pruning. It uses a connected client
and a coldkey source; it does not start a node. Enable the `rtao` facade's
`owner` feature. The complete source example is
`crates/rtao/examples/owner.rs`.

## Preview and bound a change

Plan with public wallet information before unlocking the coldkey. The compiled
owner example restricts semantic spend to zero and permits only the selected
subnet. Set `--max-fee-rao` to an appropriate fee cap, such as 1,000,000 rao;
omitting it leaves that cap unset. Fee estimates and owner windows can change
after a preview, so execution performs its checks again. The generated Example
implementation section contains the exact exported configuration function.

The executable resolves the wallet from its arguments and closes its shared
client after the operation. It defaults to preview and requires `--execute`
for submission. Pass an explicit
`--network` or endpoint when using it; its default is `local`, which assumes
an already available endpoint and does not launch one.

`HyperparamPlanner` resolves the owner setter allowlist, native codec bounds,
administration freeze window and setter cooldowns at one pinned head. Integer
values express raw units; supported human fractions or TAO values use the
parameter's unit rules. `WindowWait::NoWait` returns the current refusal;
`UntilOpen` waits for advancing best heads and replans until its deadline.
`set_hparam` permits one controlled replan for an included
`TxRateLimitExceeded` race. Other dispatch or pool failures do not become that
retry. An operation timeout bounds reads, signing, submission and waits.

## Create, start and inspect

`create_subnet(hotkey, RegistrationWait::UntilRegistered { timeout })` follows
the registration's correlated `NetworkAdded`, including queued creation.
`RegistrationWait::Receipt` returns after the registration transaction and
can return without a subnet identifier. A receipt alone does not prove that
queued creation completed. The zero-spend policy above is for configuration;
creation needs a policy that admits its registration spend.

If completion fails after inclusion, the underlying `TxError::PostSubmission`
retains the observed receipt. Inspect it and the current registration state;
the error alone does not mean that another registration should be submitted.

`start(netuid)` rereads the mutable start schedule and ownership while waiting,
then submits `StartCall`. `set_identity`, `update_symbol` and
`set_mechanism_count` use typed intents and the same policy-bearing executor.
Use `with_execute_options` for proxy routing; the delegate remains the signer
while checks use the effective owner account.

`OwnerMonitor::new(client, netuid)?.sample().await?` reads one pinned owner
observation: owner keys, activation and emission flags, conviction gates,
proxy announcements and pruning exposure. `sample_at(&snapshot)` observes an
explicit block. Its finalized event stream has a serializable
`OwnerEventCursor` for application-managed resume storage. A conviction gate
or elapsed proxy delay is an observation, not proof that ownership changed
or that a particular announced call can execute.

## CLI and pruning

The CLI exposes `subnets create`, `hparams check-start`, `hparams start`,
`hparams get`, `hparams set`, identity and symbol commands, and
`hparams mechanisms`. Inspect leaf `--help` for the generated argument schema.
Use `--dry-run` to preview and `--json` for structured output. The CLI follows
btcli's confirmation flow and does not install a library `Policy`; embedded
applications should set their own policy as above.

`hparams trim` and `hparams trim-null` can deregister participants and change
survivor UIDs. Sequential Null pruning waits for finalized receipts and then
checks actual storage progress. It uses direct submissions; pass
`--no-mev-shield` if persistent configuration enables shielding. It uses bounded
batches and keeps a durable unresolved-submission record. Resume with the
same target and retain that record until reconciliation proves the outcome.
An approval-only multisig receipt does not establish pruning progress.
`--switch-to-yuma` is an explicit option and checks its prerequisites after
pruning. Never discard a recovery journal just to repeat a transaction.

## Coverage

Owner planner, routing, registration and pruning regressions use committed
metadata and fake transports in hosted Rust checks. They cover refusal before
private-key access, changing windows, bounded retries, finalized progress and
unknown-fate recovery. These are client checks; localnet and Python interop
are optional targeted hosted evidence. This guide does not
claim that the latest CI passed or that a subnet deployment was exercised.
