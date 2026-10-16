# Null consensus

Null subnets have one validator permit holder. Subnet ownership and self
weights do not bypass that permit. For client applications, the main choices
are how to represent the weight row and how to shrink the population before
returning to Yuma. The [validator guide](validator.md) covers scheduling and
commit-reveal; the [owner guide](owner.md) covers policy and owner-key handling.

## Inspect and select consensus

Use an existing endpoint and an explicit network. For example, replace subnet
123 with your subnet when inspecting its current mode and mechanism count:

```sh
rtao hparams get --network test --netuid 123 --name epoch_consensus
rtao hparams mechanisms count --network test --netuid 123
```

The owner setter accepts the exact names `Yuma` and `Null`. A raw read can
report `0` or `1`; those numbers are not accepted setter inputs. Preview an
owner change with:

```sh
rtao hparams set --network test --wallet owner --netuid 123 \
  --name epoch_consensus --value Null --dry-run
```

Review the subnet, effective owner, call and current refusal reasons before
submitting without `--dry-run`. The owner planner checks ownership,
administration freeze windows and setter cooldowns. A preview does not reserve
an execution window or prove that the chain will accept the change.

## Choose a weight representation

`rtao misc weights set` and the SDK's `SetWeights` intent select direct weights
when commit-reveal is disabled and a timelocked commit when it is enabled.
Null mode does not select that submission path. The registered hotkey must
hold the Null validator permit; registration, permit and rate-limit checks
happen before building its weight call. The chain still enforces its other
weight requirements, including the required version key.

Ordinary scores are clipped, normalized and quantized to a `u16` row under
the subnet's weight limits. Two explicit integer interfaces have different
zero handling:

| Interface | Integer row behavior |
|---|---|
| CLI `--raw-u16` or intent `raw_u16: true` | Requires Null mode, accepts integers in `0..65535`, removes zero entries and preserves the remaining values without scaling or clipping. Minimum nonzero count and maximum-share checks still apply, with the self-weight exception. |
| Framework `WeightsInput::RawU16` | Requires Null mode and this hotkey as the sole permit holder. Preserves every UID/value pair, including zeros; checks destination bounds, duplicates, count and configured share limits without changing the row. |

Use leaf `--help` for `--uids`, `--weights`, `--weights-file`, `--mechid` and
`--version-key`. Choose a version supported by the scoring implementation.
The framework refuses a key below the chain requirement and an unsupported
commit-reveal client version; it never adopts the chain's required key
automatically. Keep scores indexed by hotkey identity, since pruning can
renumber UIDs.

## Prune and return to Yuma

Pruning deregisters participants and can change survivor UIDs. Review that
effect before using `hparams trim-null`. Its default target is the pending
on-chain pruning target, or `floor(256 / mechanism_count)` when none exists.
The target must also fit the current minimum and maximum UID bounds. Root
cannot use this workflow.

`--dry-run` previews one transaction. An actual invocation submits bounded
batches, waits for finalized receipts and verifies storage progress. Use
`--max-batches`, `--wait-timeout` and `--poll-seconds` to bound the invocation.
Matching continuation batches retain the administration-freeze check even
when their initial cooldown no longer applies.

Retain the durable record selected by `--state-file`, or its default beside
the CLI configuration. Resume with the same chain, subnet, owner and target.
An unresolved submission must be reconciled before another attempt; keep
the record after errors. A successful multisig approval alone does not prove
that pruning ran. Sequential pruning requires unshielded finalized receipts;
use `--no-mev-shield` when configuration otherwise enables shielding.

`--switch-to-yuma` explicitly requests the final consensus change. The session
checks finalized population, completion of the pruning target and pending
weight-commit queues before planning that change through the owner window.
Without this option, completion leaves a Null subnet in Null mode. SDK users
can drive the same decisions through `TrimNullSession` and retain their own
submission and recovery handling.

The [generated reference](reference.md) describes `SetWeights`,
`TrimNullSubnetBatch` and `SetHyperparameter`. Committed fixtures cover client
gates and finalized-progress handling in hosted checks. Consult the exact
commit's CI results for execution evidence; this workflow makes no live-chain
or soak claim.
