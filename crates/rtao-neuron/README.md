# rtao-neuron

Application runtimes for Bittensor validators and miners, with subnet-owner
operations. Applications supply scoring, request handlers or owner actions;
the framework coordinates configuration, public registration checks, chain
observations, signing and shutdown.

`NeuronConfig` resolves supported CLI, environment and shared btcli settings.
`StateCell` and `StateStore` manage application state, while `ChainClock`,
`StepPolicy` and readiness observations support bounded service work.

| Feature | API |
| --- | --- |
| `validator` | `Validator`, `NeuronRuntime`, weight proposals, scheduling and actual-epoch reveal observations. |
| `miner` | `Miner`, `MinerRuntime`, authenticated handlers, axon publication and pruning observations. |
| `owner` | `OwnerOps`, `HyperparamPlanner`, owner monitoring, registration and pruning workflows. |
| `prometheus` | Metrics recording; enabled by default. |

`NeuronRuntime` and `NeuronCx` are present with `validator` or `miner`.
Pruning types are present with `miner` or `owner`. The three role features
must be enabled explicitly; the default telemetry feature enables no role.

Keep wallet material and state directories persistent. Readiness reports the
framework's current observations and may lag chain changes. A successful
submission, approval or local snapshot requires the operation-specific receipt
and state checks before it establishes the intended effect.

The facade's validator, miner and owner examples supply complete application
setup.

The native validator runtime persists weight work in `weights-journal.json`
under its state directory. Embedded schedulers opt in with `with_journal`;
plain `WeightScheduler::new` stays in memory. `WeightsProposal::Operation`
supplies stable application IDs, and observation-only `recover_finalized`
restores unresolved submissions and pending reveal tracking before new work.
Missing history and ambiguous native events retain uncertainty. Pause controls
stop new weight dispatch while automatic native reveals remain observable.
See `docs/recovery/durable-work.md` for the exact recovery and chain limits.
