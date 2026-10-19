# M1-24 public endpoint pacing observations

Current pacing checks use fixtures and `FakeNode` in free `ubuntu-latest` CI
under Q30. Any new public-endpoint sample is optional targeted hosted work with
bounded traffic. No local probe workspace or build is required.

On 2026-10-14, 08:10:00–08:14:10 UTC, all seven public URLs in the extracted
network, fallback and archive pools accepted 36 of 36 lightweight reads with
one request per second and no burst. `RateLimit::PUBLIC` therefore retains
`per_second: 1.0` and reduces `burst` from the provisional eight to one.
Eight simultaneous requests had no measured or documented justification.

| Endpoint | Accepted | Observed requests/s | Median / p95 response, ms |
|---|---:|---:|---:|
| `entrypoint-finney.opentensor.ai:443` | 36/36 | 0.9970 | 114.2 / 120.0 |
| `test.finney.opentensor.ai:443` | 36/36 | 0.9971 | 106.6 / 109.3 |
| `dev.chain.opentensor.ai:443` | 36/36 | 0.9968 | 108.3 / 111.0 |
| `lite.chain.opentensor.ai:443` | 36/36 | 0.9970 | 114.3 / 118.5 |
| `lite.sub.latent.to:443` | 36/36 | 0.9973 | 37.6 / 40.1 |
| `archive.chain.opentensor.ai:443` | 36/36 | 0.9974 | 106.9 / 166.6 |
| `archive.sub.latent.to:443` | 36/36 | 0.9974 | 35.5 / 39.2 |

Each upgrade returned HTTP 101. None disclosed `X-RateLimit-*`, `RateLimit*`
or `Retry-After`. The two latent endpoints identified `server: cloudflare`,
devnet identified `server: nginx/1.29.5`, and the remaining four supplied
upgrade/connection/date headers. No JSON-RPC error, traffic-policy refusal,
close 1008, timeout or unsolicited close occurred.

These samples measure an accepted operating rate for this workload and source
IP, over about 35 seconds per endpoint. They do not establish any provider's
quota ceiling, storage-work allowance, reset window or tolerated burst. Q6's
exact provider ceilings remain unknown. The pinned upstream guide states
“roughly 1 request per second per IP” and recommends an owned node for serious
validator, miner or indexer traffic ([v478 local-development.mdx, lines
158–163](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/docs/guides/local-development.mdx#L158-L163)).
The observations support keeping that conservative sustained rate and removing
the unverified startup burst. Expensive runtime calls, large storage batches
and historical reads were outside this experiment.

The preset is per session, while the upstream allowance is per source IP.
Several sessions or processes must coordinate their aggregate traffic;
session creation does not allocate a new provider quota. Policy refusals
remain terminal and retain provider retry information. Discovery, reconnect
replays and queued requests already consume the transport bucket.

The Rust fixture ran on a local macOS workstation with Rust/Cargo 1.99.0 and the production explicit ring TLS configuration. It used
one WebSocket at a time, sending twelve `chain_getFinalizedHead`, twelve
`state_getStorage(System.Number)` and twelve `system_health` reads per URL:
252 requests total. Its schedule measures from the actual previous send, so a
delayed response cannot cause a catch-up burst. Each response has a 15-second
deadline; any error or policy refusal ends that endpoint's sample immediately.
There are no submissions, subscriptions, retries or quota-escalation phases.
The full live probe deferred public traffic until this run ended.

The bounded raw JSONL recorded all upgrade headers selected for
policy/operational interpretation, request results and individual
send/response timings. Cookies and unrelated headers were excluded. The
fixture source, raw JSONL, summary, lock and cleanup record are not retained
in the repository. The summary used
`(request_count − 1) / (last_send − first_send)` for the operating rate and
nearest-rank p95. Validation checked all seven HTTP 101 upgrades, exact IDs
1–36, the twelve-per-method mix, every successful response and all 245 send
intervals of at least 1,000 ms. Its shortest interval was 1,000.616 ms.

The raw JSONL SHA-256 is
`b387bb451851efd27a22115a05703a13b1853e621fa723bc45e3922c37ce45de`.
The isolated fixture lock SHA-256 is
`960a09e59aaa3d9d8706b487c8be879e0df228c8918f83ac59493d176155d7dd`.
The source received only an equivalent Clippy conditional-style cleanup after
the live run.

The fixture used an independent temporary Cargo workspace with path dependency
`rtao-rpc`, `futures = "0.3"`, `serde_json = "1"` with `arbitrary_precision`,
`tokio = "1.53"` with `macros`, `rt-multi-thread`, `time`, `url = "2"`, and
`tokio-tungstenite = "0.30"` with default features disabled and `connect`,
`handshake`, `rustls-tls-native-roots`. These dependencies and the recorded
digests document that historical run. A future sample needs a reviewed
bounded hosted entry point; do not recreate its local workspace or retired
build root. Endpoint labels can restrict a hosted sample, and the fixture
never accepts a higher rate or request count.

`ratelimit_paces_public_presets` passes against the real `Session` and
`FakeNode`: discovery and two concurrently queued reads arrive at least
900 ms apart, rejecting an unpaced discovery or the former eight-token
startup burst. The existing submission-priority/cancelled-queue regression
also passes. Strict Clippy passes for `rtao-rpc --all-features --all-targets`
and the isolated fixture. The full `rtao-testkit --test rpc --features
rpc-rate-limit` suite passes all 39 tests with no failures or ignored tests.
All builds and scratch used the recorded task root;
no Python interpreter or container was run.

The disposable fixture directory and its exact binary, dependency,
incremental and fingerprint artifacts were removed after checks for running
processes, open files, launchd/configuration references, symlinks and virtual
environments. The former plan build/scratch root is retired and absent; no
current dependency on those caches remains. Shared global Cargo caches and
unrelated work are preserved.

## Burst allowance, 2026-10-19

With burst 1, every request a command makes costs a full second, so a
one-shot CLI read spent 15–20 seconds connecting. On 2026-10-19 at
04:42 UTC, one connection to each of four public endpoints sent twelve
lightweight reads back to back (`chain_getBlockHash`, `chain_getFinalizedHead`,
`chain_getHeader`, `state_getRuntimeVersion`). Every endpoint accepted all
twelve without a JSON-RPC error, policy refusal or close:

| Endpoint | Accepted | Burst duration, ms |
|---|---:|---:|
| `entrypoint-finney.opentensor.ai:443` | 12/12 | 146 |
| `lite.chain.opentensor.ai:443` | 12/12 | 148 |
| `test.chain.opentensor.ai:443` | 12/12 | 153 |
| `dev.chain.opentensor.ai:443` | 12/12 | 108 |

Earlier on 2026-10-18, a template miner and two validators ran for several
hours against `test.chain.opentensor.ai` without client pacing (as a custom
endpoint) and saw no refusal. `RateLimit::PUBLIC` therefore keeps
`per_second: 1.0` and allows `burst: 10`, below the observed twelve. Sustained
traffic stays at about one request per second, as the upstream guide asks;
only the first ten requests of an idle session go out without waiting.
`test.finney.opentensor.ai`, now the testnet fallback, was not probed because
it was already throttling this source.

