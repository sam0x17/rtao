# rtao-testkit

Fixtures and fake transports for client tests.

`FakeState` stores encoded chain state. `FakeNode` serves that state through
real loopback WebSocket connections with scripted replies, notifications and
transport faults. `FakeChain` implements the direct RPC adapter over the same
state without a socket. `Reply`, `SubmitScript` and `SubscriptionScript`
configure finite responses.

`fixtures` loads pinned golden vectors and metadata corpora. `RecordingView`
records descriptor observations for read coverage and pinned-block assertions.

The crate has no default features.

| Feature | Effect |
| --- | --- |
| `core-policy` | Reexports the upstream synchronous client and transaction APIs for constructor differential tests. |
| `docker` | Adds the retained `Localnet` and `Scenario` infrastructure. |
| `interop` | Enables `docker` for the retained interoperability test target. |
| `core-oracle` | Combines `docker` and `core-policy` for the retained core comparison target. |
| `ledger-emu` | Adds injected APDU replay channels and the upstream Ledger implementation. Scripted replies do not establish device or emulator acceptance. |
| `rpc-rate-limit` | Enables `rtao-rpc/rate-limit`. |
| `rpc-metrics` | Enables `rtao-rpc/metrics`. |

Current development uses fixtures and fake transports, with substantial
validation in hosted CI. Node, container and device campaigns are deferred
under PLAN.md Q30; enabling a feature does not run those campaigns.
