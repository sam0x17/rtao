# Validator scenario integration

`rtao_neuron::NeuronCx::connect(&config).await` builds verified chain, hotkey,
metagraph and signed HTTP services for an already registered validator. Use a
task-owned plaintext btcli hotkey file and point `NeuronConfig` at it. The
context enforces the default subnet allowlist, zero-spend policy and raw-call
refusal. Call `cx.close().await` before removing fixture files.
`cx.executor.execute_tool` uses the generated feature-enabled registry, with
the same validator policy applied to typed and dynamic execution.

For the complete service loop, use
`NeuronRuntime::validator(config, test_validator).with_context(cx.clone())`
and `with_weight_policy(WeightPolicy { version_key: VersionKey::Fixed(key),
cadence: None })`. Spawn `run()`, observe epochs or scrape metrics, then cancel
`cx.shutdown` and await the run before inspecting state. Its state directory
comes from `RTAO_STATE_DIR`; `state_schema_version()` defaults to one.
`StateStore` recovery happens before the test validator's `on_start` callback.

For direct scheduler assertions, construct
`WeightScheduler::new(policy, Arc::new(SystemClock), guard)`. Seed the guard with
`guard.observe(SystemClock.now_unix(), best_snapshot.storage(&timestamp::NOW,
()).await?)`; `best_snapshot` must come from `cx.client.at(BlockRef::Best)`.
`Clock` and `ViewExt` supply those methods. Alternatively select
`ChainClock::new(timestamp_ms)` for a verified chain timestamp.

For each finalized `BlockHeader`, call
`scheduler.on_finalized(&cx, &header, |mech| proposal(mech)).await`. A proposal
can be `WeightsProposal::Set(WeightsInput::ByUid(BTreeMap::from([(miner_uid,
1.0)])))`. Registered miner destinations other than self and owner pass the
previous-row guard. The scheduler reads current mechanism count and handles
each mechanism independently.

Inspect `scheduler.last_attempt(mech).map(|attempt| &attempt.outcome)`:

- `Refused(WeightRefusal { name, code, remediation })` covers registration,
  Null permits, stale weights version and unsupported commit-reveal version.
- `PausedSurface { descriptor, detail }` refuses changed call metadata and
  resumes automatically after the surface verifies again.
- `Submitted { block_hash, block_number }` proves finalized inclusion.
- `RateLimited { retry_after_block, remediation }` is scheduled work.
- `Unresolved(message)` keeps the mechanism blocked pending reconciliation.

`update_policy(new_policy)` supports scenario five's explicit version-key
change and preserves pending and uncertain submissions. Do not use
`resolve_uncertainty` unless an external operator has already reconciled that
submission definitively.

`scheduler.reveals().pending_for(mech)` and `.outcomes()` expose actual-epoch
reveal tracking. `RevealOutcome` retains round, actual commit epoch, target
reveal epoch, predicted first reveal block, observed outcome block, attributed
miss and epoch notices.
Metrics include `rtao_weights_commit_epoch_slip_total` and
`rtao_weights_reveal_missed_total{mech,reason}`.

The production tests in `weights/tests.rs` and `weights/reveal.rs` already
exercise lag three, a single seventeen-key preflight RPC, actual inclusion slip,
temporary inclusion-queue read failure, reveal-history recovery, late drand,
version and deregistration attribution, cadence and Null proofs. The runtime
regression also exercises generated tool parsing through the production
constructor and refusal by the validator policy before signer unlock or
submission. Current Rust and Rust 1.89 pass all forty-one integrated library
tests; these tests supplement
the real localnet scenarios and do not complete their exits.

The image binary's wallet-free diagnostic is `rtao-validator
--check-connection --network finney --chain-endpoint wss://...`. Protocol and
soak tests can use `--synthetic-weights` to assign equal scores to registered
miner destinations other than self and owner. Normal subnet binaries should
implement their own scoring protocol, as the signed echo example demonstrates.
