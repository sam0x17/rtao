# Testing your subnet

Test subnet client behavior with encoded fixtures and fake transports, then
use free GitHub `ubuntu-latest` CI for substantial builds and tests. A node,
Python oracle or testnet validator is not a development prerequisite.

Keep scoring rules, miner handlers and owner decisions testable independently
of the live connection. Connect them through the same public client, policy
and signing interfaces used in production so tests cover the path your
application will call.

## Build one bounded client fixture

`rtao-testkit::FakeState::try_default` loads the checked-in spec 478 metadata
and extracted Python defaults. Seed the exact state your case needs, including
registration, balances, subnet settings and runtime API responses. Use
`seed_typed` or generated descriptors with `seed` for metadata-backed encoding;
use `seed_at` when a value must belong to a particular block.

`FakeChain::from_state` turns that state into a direct `RpcClientT` transport.
Pass it through `rtao_testkit::rpc::RpcClient` to `Client::from_rpc`. Disable
`ClientConfig::runtime_cache_dir` and `config_store` in isolated fixtures so
the case does not read or write a user's runtime cache or configuration.
`crates/rtao-neuron/src/weights/tests.rs::fixture` shows this setup, including
different best and finalized heads. It uses public fixture identities.

Exercise a public read, plan or execution API, then assert both its result and
observable work. `FakeChain::requests` exposes RPC methods and parameters;
`FakeState::submissions` retains submitted SCALE bytes. `RecordingView` exposes
descriptor touches and block hashes for read assertions. These checks catch
wrong storage keys, extra RPC work, accidental submission during planning and
mixed-block reads.

For submission handling, queue a `SubmitScript` describing the selected
outcome, and seed the encoded events or storage needed for receipt processing.
Use explicit errors to cover refusal, dispatch failure and uncertain
submission fate. The fake supplies a scripted outcome; it does not run the
runtime or apply arbitrary extrinsics to chain state.

`crates/rtao-intents/tests/intent_plans.rs` exercises the real executor with
authenticated sample inputs and a key source that fails on private-key
acquisition. `crates/rtao-client/tests/offline.rs` uses public deterministic
fixture seeds to cover preparation, unsigned JSON import, signature attachment
and submission. Use these patterns to keep test wallets independent of
operator wallets.

## Give asynchronous work an end

Use finite `SubscriptionScript` notifications for a known sequence of heads
or transaction statuses. `FakeChain::advance` advances its synthetic chain and
publishes new heads; `publish_method` delivers updates only to subscriptions of
the selected method. Set finality explicitly when the case depends on it.
Assert the behavior after the required transition, then close the client.

Bound waits with a timeout or a cancellation token. Cancel and join spawned
work before the fixture ends; abort and join it if the timeout branch requires
that cleanup. The registration-wait tests in
`crates/rtao-neuron/src/runtime.rs` cover a finalized registration update and
already-cancelled work. They close the client and check that cancellation
avoids further reads. This is a finite test pattern, with no long-running
validator process.

For reconnect, TLS or frame behavior, use `FakeNode`, the loopback socket
counterpart sharing the same encoded state. `FaultPlan` can refuse a handshake,
drop a connection or close it with a selected WebSocket code.
`crates/rtao-testkit/tests/fakes.rs` covers those paths;
`tests/rpc_lifecycle.rs` in that crate covers close, initialization and
subscription ownership. Prefer the direct transport when sockets are irrelevant
to the assertion.

## Cover subnet-specific decisions

| Behavior | Useful assertions and existing patterns |
|---|---|
| Validator scoring | Known miner replies produce the expected scores; absent or failed replies leave an intentional result. Keep state guards outside awaited work, as in the compiled [validator example](validator.md). |
| Weight preparation | Check mechanism, version key, eligibility, cadence and exact raw rows with `crates/rtao-neuron/src/weights/tests.rs`; include [Null consensus](null-consensus.md) where supported. |
| Miner protocol | Exercise the real route and handler with signed requests. `crates/rtao-btauth/tests/conformance.rs` and `crates/rtao-neuron/src/miner/tests.rs` cover authentication and routing patterns. See the [miner guide](miner.md) for body and response limits. |
| Owner configuration | Seed current values, build a plan, and check policy and confirmation before signing. `crates/rtao-neuron/src/owner/tests.rs` and `owner_ops/tests.rs` cover these production paths. |
| Runtime change | Change fixture metadata or supported surfaces and assert the intended refusal or recovery. See [upgrades](upgrades.md). |

The generated [getting-started guide](getting-started.md) embeds ordinary
offline test functions from
`crates/rtao/tests/documentation_examples.rs`. The validator, miner and owner
guides embed exported items from their complete examples in
`crates/rtao/examples/`. Hosted CI compiles those examples with the required
features. Add new documentation examples to real exported source items so
the displayed code and the checked code stay together.

The offline exports cover checked balances, number formatting, an authenticated
btauth payload vector, a snapshot balance read through FakeChain and a transfer
plan that refuses private-key acquisition. Their names are
`checked_native_balances`, `python_compatible_number_formatting`,
`btauth_payload_matches_committed_vector`,
`fakechain_balance_read_stays_on_one_snapshot` and
`transfer_plan_uses_public_keys_without_signing`. The latter three require
`btauth`, `reads` and `intents-all` respectively. They are newly authored
examples and still require the final commit's hosted execution. Embedding a
source item does not establish that its test passed.

## Use the exact commit's hosted result

The ordinary `pr-gates` workflow runs `check`, `no-host`, `msrv`, `features`,
`supply` and `semver` jobs on `ubuntu-latest`. The command definitions live in
`xtask/src/gates.rs`. The `check` job runs source integrity and parity checks,
Clippy, generated-output checks, strict documentation, ordinary nextest tests
and Rust doctests. The docs step retains the rendered book and Rust API docs
as a downloadable artifact.

The nextest `ci` profile excludes the `e2e`, `interop`, `core_oracle` and
`ledger_emu` binaries. The ordinary workflow starts no live node, Python oracle
or hardware service. Optional integration machinery remains in the repository,
but its presence supplies no execution result. Substantial validation runs in
hosted CI, and no long-running testnet soak is part of the required checks.

Record which tests passed at the final commit and which features they exercised.
A fake-transport test establishes its client assertions. Actual runtime
mutation, live commit/reveal timing, endpoint availability, Python differential
behavior and hardware operation require separate evidence. Read
[understanding parity](parity.md) before turning a passing test into a broader
compatibility claim.
