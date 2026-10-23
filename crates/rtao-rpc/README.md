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

For a caller with durable author authorization, `submit_owned` and
`submit_and_watch_owned` consume hash-bound `AuthorOwnership`. Native queued
requests and captured WebSocket frames retain its strong guard. Cancelling the
outer request sends a queue cancellation but does not release a captured writer;
an incomplete write retains ownership until the actual socket retires, including
close flush and timeout cleanup. `AuthorRetirement::wait` acknowledges release
of native ownership references after dropping their supplied guard. It does not
prove chain absence, inclusion, or permission to replace an uncertain operation.
The ownership token is single-use; private shared lifetime references cannot
create another send authorization. This native transport facility does not
provide an ownership guarantee for arbitrary injected `RpcClientT` adapters.

Secure connections use an explicit Rustls ring provider, native certificate
roots and optional additional roots. `PlaintextPolicy` controls `ws://`
endpoints. `policy` classifies gateway refusals with bounded diagnostics.

| Feature | Effect |
| --- | --- |
| `rate-limit` (default) | Enables the configured request token bucket. `SessionConfig::public` selects its public endpoint preset. |
| `metrics` | Records transport metrics through the `metrics` crate. |
| `fuzzing` | Exposes the hidden production frame-dispatch entry point for the standalone fuzz harness. |
