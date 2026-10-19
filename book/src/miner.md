# Running a miner

A miner serves signed HTTP requests from validators and publishes its public
endpoint on chain. `NeuronRuntime::miner` supplies btauth/1 authentication,
metagraph admission, bounded replay protection, per-hotkey limiting, persistent
application state and the axon publisher. Your `Miner` implementation supplies
the routes and work each request performs.

The example at `crates/rtao/examples/miner.rs` serves `POST /echo`, returns the
received JSON and persists a request count. The [validator example](validator.md)
queries this route. Replace the echo handler with your subnet's protocol before
running a miner for that subnet.

## Configure the example

The source example uses the `miner` feature; `prometheus` enables its telemetry.
Substantial example builds and tests run in hosted CI. Running a miner or
starting a local node is not a client-development prerequisite.

Provide a btcli-compatible wallet hotkey that is registered in the selected
subnet and has access to enough TAO for serving transaction fees. Set
`BT_AXON_EXTERNAL_IP` to your host's globally routable IP. Forward public TCP
port 8091 to the miner's listener. An operator's existing example build accepts
`--netuid 123 --network test --wallet miner --wallet-hotkey default
--axon.port 8091 --state-dir ./miner-state --metrics-addr 127.0.0.1:9091`.
It uses the selected network endpoint without starting a node.

Replace subnet 123 and the wallet names with your registered identity. Keep the
wallet mount read-only and the state directory writable and persistent across
restarts. By default the runtime refuses an unregistered hotkey before unlocking
its private key. `--wait-for-registration` waits for finalized registration;
it does not register the hotkey or pay the registration cost.

Configuration uses explicit flags, environment variables, supported keys in
the shared btcli file, then defaults. `BTCLI_CONFIG` selects another shared
file. Network and wallet settings accept `BT_NETWORK`, `BT_CHAIN_ENDPOINT`,
`BT_WALLET`, `BT_WALLET_HOTKEY` and `BT_WALLET_PATH`. Use `RTAO_STATE_DIR` and
`RTAO_METRICS_ADDR` for framework paths and telemetry. `BT_AXON_EXTERNAL_IP`
sets the advertised IP. The subnet identity is mandatory; the axon port and
registration wait are flag-only settings.

Your binary parses its own arguments and passes the matches to
`NeuronConfig::resolve_matches`. Embedded callers can use
`NeuronConfig::load_for(NetUid(...))`; the library does not read argv.

## Bind and advertise the endpoint

The default listener binds `0.0.0.0:8091`. A local listener address and the
advertised endpoint are separate settings. An embedded runtime can use
`with_bind` or `with_listener` for a local address, and `with_axon(AxonConfig)`
for publication. `AxonConfig::external_port` describes port forwarding when
the public port differs from the listener port.

Without an explicit external IP, the publisher tries the configured IPv4
discovery services with five-second deadlines. An explicit IP can be IPv4 or
IPv6. Publication rejects private, loopback and other non-global addresses.
The `local` network setting permits non-global addresses for callers using an
existing development endpoint. It does not start or manage a localnet. On a
public network, `--axon.allow-private` (or `BT_AXON_ALLOW_PRIVATE=true`)
permits a loopback or private address, for example to run a miner and a
validator on one host against testnet. Validators elsewhere cannot reach such
an address.

The publisher compares IP, port, IP type, protocol and version with stored axon
data. An unchanged endpoint produces no new serving transaction. The default
protocol byte is 4 and advertised version is 1. It checks publication every
five minutes, and a `ServingRateLimit` schedules the first legal retry height.
An unresolved submission retains its extrinsic hash and prevents unconditional
resubmission; reconcile that hash before clearing the publisher's uncertainty.

`AxonConfig::certificate` advertises a `NeuronCertificate` through
`serve_axon_tls`. Its public key must be at most 64 bytes; the algorithm byte
brings the encoded limit to 65. Changing only the certificate causes another
publication. Use `AxonPublisher::update_config` when managing a publisher
directly. The runtime accepts its axon settings through `with_axon` at startup.

The runtime's `axum::serve` listener speaks HTTP. Certificate publication does
not install a TLS listener. If your protocol uses HTTPS, configure TLS
termination and validator trust separately. Preserve the method, percent-encoded
path, query string and raw body bytes through any proxy, because btauth signs
them. The publisher leaves the axon registered at shutdown by default;
`AxonConfig::reset_on_shutdown` requests a reset, subject to chain rate limits.

## Implement authenticated handlers

`Miner::State` must be serializable, deserializable, default-constructible and
safe to share between tasks. Recovery completes before `Miner::on_start`, and
`Miner::router(cx, state)` runs after initialization. The same `StateCell`
reaches initialization, HTTP handlers, metagraph callbacks and checkpoints.
The echo example installs it with axum's `Extension` layer.

Extract `rtao::btauth::server::VerifiedCaller` in each handler that needs the
verified hotkey. The runtime wraps the application router in `BtAuthLayer`.
This layer checks authentication, applies the caller policy, commits the replay
nonce, then applies the per-hotkey limiter. The handler receives the caller
only after these checks pass. Use `caller.hotkey` for application identity;
a UID can later belong to another hotkey.

Use `rtao::btauth::server::Json<T>` for typed request bodies. It first parses a
`serde_json::Value`, then deserializes the target type, including untagged and
flattened float fields. Read or modify `StateCell` briefly and release its
guard before an await. Bound request work and move CPU-intensive work off the
async executor.

The default `Miner::caller_policy` requires membership in this subnet and a
validator permit. Override it to return a configured `MetagraphPolicy` when
your protocol has different admission rules. `min_stake` uses exact
alpha-equivalent `StakeWeight` units and accepts callers at the floor. The
policy also inserts `CallerPriority` for optional application scheduling;
the runtime does not create a priority work queue.

Application balance fields should retain the chain's checked `u64` rao
semantics. Keep TAO and alpha units distinct and reject negative values and
overflow before encoding. `StakeWeight` expresses admission priority rather
than a spendable TAO balance.

## Authentication, limits and refusals

Default verification requires a receiver-bound request for this miner's hotkey,
accepts nonces at most ten seconds old and tolerates a sender clock lead of two
seconds. Keep the host clock synchronized. A signed client must sign the final
request target and body and obtain a fresh nonce for every retry. The validator
example uses `cx.http`, the framework's `SignedClient`, for this exchange.

The default raw body limit is 4 MiB. The authentication layer limits the stream
before collection, and the runtime sets the inner axum `DefaultBodyLimit` to
the same size. `with_body_limit(bytes)` changes both bounds. Full request bodies
are buffered for hashing; responses may stream.

Authentication fields have a separate budget of 4,096 bytes per field and
8,192 bytes total, counting duplicate header values. Sender addresses must fit
the native 49-byte AccountId32 SS58 limit. Inputs above these limits receive
401 before expensive parsing or signature verification. The optional legacy
verifier counts its supplied body hash in the same budget.

The default in-memory nonce store retains accepted pairs for 60 seconds, with
limits of 1,024 pairs per hotkey and 65,536 globally. Live entries are never
evicted to make space. Replay retention must be **strictly greater than
`max_age + allowed_skew`**. Equality is refused because freshness includes its
boundary. `with_verification` and `with_nonce_store` configure these settings,
and an incompatible combination fails at startup.

In-memory replay state belongs to one process and is separate from application
checkpoints. Multiple workers serving the same hotkey need a shared nonce
store. With the `redis` feature, construct `RedisNonceStore::connect` and pass
it to `with_nonce_store`. Redis requires a positive memory bound and
`maxmemory-policy noeviction`. Preserve live `btauth:*` keys across restart
and failover, and prevent application credentials from changing configuration
or deleting keys. The adapter checks configuration at connection time and
fails closed after ambiguous backend errors. After loss of replay state,
keep request admission stopped for at least the configured retention before
starting with an empty store. The runtime does not persist or restore its
default in-memory nonce store.

The default limiter permits a burst of 64 requests per hotkey, replenishes one
token per second and remembers at most 4,096 hotkeys with outstanding debt.
`with_limiter` accepts a configured `PerHotkeyLimiter`.

| HTTP status | Cause | Caller action |
| --- | --- | --- |
| 401 | Missing or invalid authentication, wrong receiver, expired nonce or replay | Correct the request and sign a fresh attempt. |
| 403 | Caller admission denied | Check subnet registration, permit and any stake floor. |
| 413 | Raw request body exceeds the configured bound | Reduce the body or agree on a larger bound. |
| 429 | Caller token bucket exhausted | Respect `Retry-After` and sign the retry with a fresh nonce. |
| 503 | Nonce store, limiter or verification clock cannot safely admit the request | Restore the service dependency or capacity before retrying. |

An authenticated request consumes its replay nonce before limiting. Reusing a
nonce after a 429 receives 401. A 403 does not consume replay capacity. JSON
extraction can also return 415 for an unsupported content type, 400 for invalid
syntax or 422 when valid JSON does not fit the handler's type.

## State, graph changes and pruning

Application state saves every five minutes and at graceful shutdown. The
default directory is
`~/.bittensor/miners/<wallet>/<hotkey>/netuid<netuid>/miner`. Checkpoints use
`state.json`, `state.1.json` and `state.2.json`, with fsync and atomic replacement.
Each complete checkpoint is limited to 64 MiB. Loading and rotation validate
the opened regular file and enforce the limit while reading; special files
are refused. A failed rotation preserves the current checkpoint.
Each records schema, package version, subnet, hotkey, finalized block and save
time. Corrupt JSON can recover a preceding valid generation; identity and
schema mismatches refuse recovery. Override `Miner::state_schema_version`
when changing the saved format, and perform an explicit migration before
starting with an incompatible checkpoint.

The miner metagraph preset refreshes every ten minutes and coalesces relevant
finalized `NeuronRegistered`, `AxonServed` and `HotkeySwapped*` events into an
early refresh. It keeps membership, permit and stake fields in one last-good
snapshot. Custom interval policies have a thirty-second minimum. Use
`Miner::on_metagraph` to remove per-caller state for deregistered or replaced
hotkeys. Admission uses the current local snapshot, so it can lag a chain change
until the next successful refresh. Readiness degradation does not itself stop
the HTTP listener or replace the caller policy.

The pruning monitor publishes `rtao_immunity_blocks_remaining` and
`rtao_prune_risk`. It reads authoritative subnet storage and models the pinned
runtime's next-registration replacement choice, including integer emission
ordering, owner protection, capacity and immune fallback. A positive immunity
countdown can coexist with pruning risk when the runtime selects an immune
candidate. Treat the result as a snapshot estimate; later registrations or owner
changes can alter it. Root-subnet stake pruning is outside this monitor.

## Observe and validate the service

The separate metrics listener exposes `/healthz`, `/readyz` and `/metrics`;
it is disabled unless `--metrics-addr` is set. Health reports a live process.
Readiness includes registration, connection, runtime compatibility and metagraph
freshness. Keep this listener on a management interface. Watch
`rtao_metagraph_age_blocks`, `rtao_axon_published_total`,
`rtao_axon_retry_at_block`, `rtao_axon_errors_total` and
`rtao_prune_monitor_errors_total` alongside the pruning gauges. A ready listener
does not prove that validators can reach the published endpoint.

The first SIGINT or SIGTERM stops new work and drains framework services for up
to thirty seconds, then aborts remaining tasks, saves state and closes the
context. A second signal exits immediately with status 130. For systemd, allow
time for state persistence with `KillSignal=SIGTERM`, `TimeoutStopSec=90` and
`Restart=on-failure`. Abrupt termination can lose changes since the last save.

Before operating your handler, verify a signed request from an eligible
validator, the 401/403/413/429 refusals, unchanged-endpoint idempotence,
certificate-only rotation when configured, state recovery and shutdown. Test
the endpoint from the validator's network through the actual forwarding or
TLS setup. Hosted Rust regression tests cover framework behavior with fake
transports and committed fixtures; inspect the result for the exact source
commit. Live networking and operational behavior require separate evidence.
Localnet and Python interoperability checks are optional targeted hosted work.
They do not require local nodes, containers or a local test
campaign, and their absence is a coverage limit rather than a passing result.
