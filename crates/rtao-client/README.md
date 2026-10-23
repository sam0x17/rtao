# rtao-client

Asynchronous Bittensor chain access over a supervised RPC session.

`ClientConfig` selects a network, transport, cache paths and startup policy.
`Client::connect` initializes the connection and checks chain preconditions.
Cloned clients share the session; `close` provides explicit shutdown.
Runtime metadata is resolved through `RuntimeCache`.

`Snapshot`, `View` and `DynView` pin typed or dynamic reads to chain state.
Head streams expose best/finalized progress and gap recovery. Submission APIs
prepare calls, manage nonce leases and report pool, inclusion and dispatch
outcomes. `UnsignedExtrinsic` and `UntrustedUnsignedExtrinsic` distinguish a
prepared transaction from imported unsigned JSON that needs authentication.

The `compose` module handles runtime-checked origin wrappers. `shield`
provides bounded MEV envelopes and paired-nonce submission. Calls remain
subject to the connected runtime's metadata and compatibility checks.

## Features

The crate has no default features.

| Feature | API |
| --- | --- |
| `blocking` | Private-runtime support used by the facade's synchronous adapters. |
| `subxt` | Optional legacy Subxt client sharing the authenticated RPC session. |
| `metrics` | Client/RPC instrumentation. |
| `test-util` | Declared fixture integration flag; `Client::from_rpc` is available without it. |

Configuration and cache paths can use local files; callers can disable the
optional disk caches in `ClientConfig`. Constructing configuration does not
start a node or a network connection.

`journal::OperationJournal` records logical operations and signed attempts
before dispatch, with a versioned storage interface and exclusive native
atomic-file backend. Supply a binding through `SubmitOptions::journal`, then
use finite recovery before replacement. Missing history remains unresolved;
Debug omits signed bytes/material. See `docs/recovery/durable-work.md` for the
storage contract, pause controls, semantic guards, limits and offline example.
