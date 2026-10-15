# M3 miner framework preparation

`NeuronRuntime::miner(config, application)` returns a `MinerRuntime` with the
miner metagraph preset, persistent `StateCell`, authenticated axum routes,
axon publication, pruning metrics, telemetry and graceful shutdown. The
application supplies `Miner::router`, optional initialization and metagraph
callbacks, and a state schema. Recovery and `on_start` complete before the
runtime builds the router or accepts requests. Startup failures close the
context; normal shutdown stops and awaits the services before saving state.

The default caller policy requires subnet registration and a validator permit.
Authentication, admission, replay commitment and per-hotkey limiting use the
btauth server owner's layer. The outer layer and inner `DefaultBodyLimit` share
the same four-MiB bound, including when overridden. A listener, nonce store,
verification policy and limiter can be supplied by an embedding application.
The default nonce store and limiter are bounded.

Neuron configuration adds the flag-only `--wait-for-registration`, defaulting
to false. Without it, authoritative finalized `Uids` refuses an unregistered
hotkey before private-key unlock. With it, finalized registration observations
are awaited; checking state also recovers an event lost during reconnect. The
wait is cancellable while reading or waiting for a head. Validator construction
continues to attach the generated registry and its original policy.

`PruneRisk` reads authoritative base-subnet storage, including `Emission(netuid)`,
at one snapshot. It models v478's exact integer emission
order, the strict non-immune count floor, immune fallback, oldest-owner
protection, primary-owner replacement refusal and subnet capacity. Missing keys
remain distinct from an actual zero account. A positive immunity countdown
does not promise protection when the pallet chooses an immune candidate.
Root-subnet stake pruning is explicitly excluded. Metrics are
`rtao_immunity_blocks_remaining` and `rtao_prune_risk`.

The facade miner example implements the signed JSON echo protocol used by the
validator example and persists its request count. The feature gate already
includes the real example in the `miner,redis` profile; root owns its manifest
entry and workspace dependency edges.

Current Rust and Rust 1.89 pass all 61 neuron library tests and the compile-fail
state-guard doctest. The original 41 validator tests remain in the suite. Eight
pruning tests include historical encoded state and missing/zero account cases.
The complete miner regression uses the production context initializer with
cache writes disabled, restores a checkpoint before `on_start` and router
construction, admits a signed three-MiB request, saves the updated count on
shutdown, and verifies that the unchanged axon was never submitted. Registration
tests cover cancellation and recovery without an event.
Strict Rust 1.89 Clippy passes the combined profile with all targets and the
miner-only profile with default features disabled. The real miner example
passes strict MSRV Clippy with `miner,redis`; the existing validator example
passes with `validator,prometheus`. The miner example also passes the final
current-Rust check with `miner,redis`.

Logs are `tmp/neuron-m3-current.log`, `tmp/neuron-m3-msrv.log`,
`tmp/neuron-miner-runtime-tests.log` and `tmp/neuron-prune-{tests,msrv}.log`
under `target/execute-plan.6Ro0PC`. The development wallet and state fixture
was created under that recorded root, then removed after the runtime, client,
metagraph and socket node had stopped. No user wallets or state were used.
The empty `tmp/rtao-neuron-tests` fallback and its empty `tmp` child were also
removed after checking processes, launchd units, configuration references,
symlinks and virtual-environment dependencies. No neuron fixture directories
remain. Shared build caches remain live. These checks are preparation for the
required localnet, interop, template and security exits.
Additional check logs are `tmp/neuron-m3-clippy.log`,
`tmp/neuron-m3-miner-only-clippy.log`, `tmp/neuron-miner-example-msrv.log` and
`tmp/neuron-validator-example-m3-msrv.log`. The current-Rust example log is
`tmp/neuron-miner-example.log`. The final neuron formatting check passes.
