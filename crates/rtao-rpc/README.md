# rtao-rpc

Asynchronous JSON-RPC over WebSocket for Bittensor clients.

Create a `Session` from `SessionConfig`. `EndpointPool` separates primary,
fallback and archive endpoints. The session correlates requests, reconnects
after transport loss and exposes connection state, events and statistics.
Typed `ReadMethod` values identify reads that can be resent safely.

`request_many` handles batches, while `subscribe` returns a bounded
`Subscription` stream. `submit` and `submit_and_watch` preserve submission
uncertainty through `SubmitError`. Configure queue, frame, in-flight request
and timeout limits through `SessionConfig`; close the session when finished.

Secure connections use an explicit Rustls ring provider, native certificate
roots and optional additional roots. `PlaintextPolicy` controls `ws://`
endpoints. `policy` classifies gateway refusals with bounded diagnostics.

| Feature | Effect |
| --- | --- |
| `rate-limit` (default) | Enables the configured request token bucket. `SessionConfig::public` selects its public endpoint preset. |
| `metrics` | Records transport metrics through the `metrics` crate. |
| `fuzzing` | Exposes the hidden production frame-dispatch entry point for the standalone fuzz harness. |
