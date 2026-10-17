# Compatibility crates and remote signing

v11 replaced Axon, Dendrite and Synapse with btauth/1 signed HTTP, and rtao's
neuron runtime speaks btauth/1. Many subnets still run v10 peers, Epistula
headers or btlightning QUIC, and some operators keep signing keys on a
separate host. The post-parity M10 crates cover those cases. They are
optional, outside the v11 parity target and have no effect on builds that do
not enable them.

| Facade feature | Module | Crate | Use |
| --- | --- | --- | --- |
| `compat-synapse` | `rtao::compat::synapse` | `rtao-compat-synapse` | talk to `bittensor==10.5.0` axons and dendrites |
| `compat-epistula` | `rtao::compat::epistula` | `rtao-compat-epistula` | sign and verify Epistula v2 headers |
| `transport-quic` | `rtao::quic` | `rtao-transport-quic` | talk to btlightning QUIC peers |
| `remote-signer` | `rtao::remote_signer` | `rtao-remote-signer` | sign transactions on a policy-checked server |
| `remote-signer-server` | `rtao::remote_signer::server` | `rtao-remote-signer` | embed the reference signing server |

`full` enables all five. A miner that only needs a v10 axon can enable one
feature:

```toml
[dependencies]
rtao = { path = "../rtao/crates/rtao", features = ["compat-synapse"] }
```

The facade enables the server and client halves of `compat-epistula`. Depend
on `rtao-compat-epistula` directly for `redis`, `metagraph-policy` or
`secret-filter`. The crate documentation (the README, or for
`rtao-compat-synapse` the crate-level docs) lists each deliberate difference
from upstream.

## v10 Synapse, Axon and Dendrite

Declare each synapse as a serde struct that mirrors the Python class. `NAME` is
the class name and route. `REQUIRED_HASH_FIELDS` must list Python's
`required_hash_fields` in the same order, because the body hash renders those
fields with Python's `str()` and `repr()` rules. Use `PyDict` for hashed `dict`
fields: it keeps insertion order as Python does, while `HashMap`, `BTreeMap`
and `serde_json::Value` do not.

```rust,ignore
use std::sync::Arc;

use rtao::compat::synapse::{
    Axon, DefaultHooks, Envelope, Reply, Synapse, V10AxonConfig,
};
use rtao::primitives::CryptoType;
use rtao::signer::KeypairSigner;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Echo {
    text: String,
    reply: Option<String>,
}

impl Synapse for Echo {
    const NAME: &'static str = "Echo";
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["text"];
}

let hotkey = Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519)?);
let axon = Axon::new(hotkey, V10AxonConfig::default())?.attach::<Echo, _, _, _>(
    DefaultHooks,
    |_head, mut envelope: Envelope<Echo>| async move {
        envelope.body.reply = Some(envelope.body.text.clone());
        Ok(Reply::Synapse(envelope))
    },
);
let listener = tokio::net::TcpListener::bind(("0.0.0.0", 8091)).await?;
axon.serve(listener, std::future::pending()).await?;
```

`Axon::serve` records peer addresses. To mount the synapses in an existing axum
application, merge `axon.routes()` (exact paths only) or `axon.router()` (with
Python's 404 fallback), and serve with
`into_make_service_with_connect_info::<SocketAddr>()`.

The handler receives a `VerifiedHead` and the parsed envelope. The body's
`dendrite` and `axon` objects are replaced with the verified header values
before the call, so a request cannot spoof its hotkey or choose its response
status. Implement `Hooks` for blacklist, extra verification and priority. The
blacklist hook runs before verification on a claimed hotkey; keep it to deny
decisions, and key rate limits on the verified identity in `Hooks::verify` or
the handler. Return `Reply::Stream` for streaming synapses.

The axon always applies the nonce window, refuses nonces more than 60 seconds
ahead of its clock and keeps a bounded nonce table: 100,000 entries and at most
1,024 live `hotkey:uuid` keys per hotkey, answering 503 when full. Request
bodies are limited to 16 MiB. Honest v10.5.0 dendrites are unaffected.

A dendrite calls a v10 axon and always returns the envelope, with the outcome
in `dendrite.status_code`, as Python does:

```rust,ignore
use rtao::compat::synapse::{CallOptions, Dendrite};
use rtao::primitives::{Uid, axon::AxonTarget};

let dendrite = Dendrite::new(validator_hotkey)?;
let target = AxonTarget {
    uid: Uid(0),
    hotkey: miner_hotkey,
    base_url: "http://203.0.113.7:8091".parse()?,
};
let reply = dendrite
    .call(&target, Echo { text: "hi".into(), reply: None }, &CallOptions::default())
    .await;
let status = reply.dendrite.as_ref().and_then(|d| d.status_code);
```

`forward` queries several targets with bounded concurrency, and `call_stream`
returns the raw chunks followed by the final envelope. `CallOptions::uuid_mode`
selects `UuidMode::PerRequest` to avoid the out-of-order 401s that Python
axons give concurrent calls sharing one uuid. The `tensor` feature provides
the msgpack-numpy `Tensor` field type.

Interop was run locally against real bittensor 10.5.0: Rust dendrite to Python
axon 39/39 and Python dendrite to Rust axon 37/37.

## Epistula v2

Epistula signs `sha256(body) "." uuid "." timestamp_ms "." receiver` and
carries the result in `Epistula-*` headers. Pick the `Dialect` your peers use:
`Dialect::RTAO` (the default, BitMind's wire format with tighter freshness),
`Dialect::BITMIND`, `Dialect::GOBT` or `Dialect::APEX`. `Dialect::SPEC` is for
tests only.

A miner verifies with `server::EpistulaLayer` and reads the verified sender
with the `VerifiedEpistula` extractor:

```rust,ignore
use std::sync::Arc;

use axum::{Router, routing::post};
use rtao::compat::epistula::{
    Dialect, InMemoryNonceStore, NonceStore, Verifier,
    server::{EpistulaLayer, VerifiedEpistula},
};

async fn handle(VerifiedEpistula(caller): VerifiedEpistula, body: String) -> String {
    format!("{} sent {} bytes", caller.signed_by, body.len())
}

let store: Arc<dyn NonceStore> = Arc::new(InMemoryNonceStore::new());
let verifier = Arc::new(Verifier::new(miner_hotkey.account(), Dialect::RTAO, store)?);
let app = Router::new()
    .route("/score", post(handle))
    .layer(EpistulaLayer::new(verifier));
```

The layer checks headers and freshness when the request head arrives and then
bounds the body read, policy and replay commit by a body deadline (30 seconds
by default, shortened to fit the store's retention). Use `with_policy` and
`with_limiter` for btauth caller policies and per-hotkey limits.
`DualAuthLayer::new(epistula, btauth)` accepts either Epistula or btauth/1 on
one router.

Epistula does not sign the method, path or query. Mount it only on routes whose
meaning is in the body. For the same reason the layer adds a btauth `Caller`
for `VerifiedCaller` handlers only after `with_btauth_caller(true)`.

A validator signs outgoing requests with `client::EpistulaClient`. Each call
re-signs, so retry by calling again:

```rust,ignore
use rtao::compat::epistula::{Dialect, client::EpistulaClient};

let client = EpistulaClient::new(reqwest::Client::new(), validator_hotkey, Dialect::RTAO);
let request = client.http().post(url).body(payload).build()?;
let response = client.execute(request, Some(miner_ss58.as_str())).await?;
```

Apex clients sign `{}` for an empty body and the query parameters for a `GET`.
Use `execute_with_digest` with `apex::canonical_json` for those requests.
BitMind and Apex behaviour was checked against their pinned Python code; the
gobt Go reference has not been executed, so treat `Dialect::GOBT` verification
as covered only by message vectors.

## btlightning QUIC

`rtao::quic` speaks btlightning's wire protocol (v0.2.15) on quinn with the ring
provider, without depending on btlightning or its `sp-core` version. A miner
routes synapse types to handlers and admits validators with a `HotkeyPolicy`:

```rust,ignore
use std::{collections::HashSet, net::SocketAddr};

use rtao::quic::{QuicRouter, QuicServer};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
struct AddReq { a: i64, b: i64 }
#[derive(Serialize, Deserialize)]
struct AddResp { sum: i64 }

let router = QuicRouter::new()
    .typed("add", |_caller, r: AddReq| async move { Ok(AddResp { sum: r.a + r.b }) })
    .unary_fn("echo", |_caller, data| async move { Ok(data) });
let allowed: HashSet<_> = permitted_validators.into_iter().collect();
let server = QuicServer::builder(miner_hotkey, router)
    .policy(move |hotkey: &rtao::primitives::AccountId| allowed.contains(hotkey))
    .bind(SocketAddr::from(([0, 0, 0, 0], 8092)))?;
let shutdown = CancellationToken::new();
server.serve(shutdown.clone()).await?;
```

`serve` takes a `tokio_util::sync::CancellationToken`, so the miner crate also
depends on `tokio-util`.

Without a policy any sr25519 key can authenticate, and `bind` logs a warning.
Production miners should admit only known validators, such as permit holders.
The server also caps unauthenticated connections (256 in total, 16 per source,
IPv6 grouped by /64), closes connections that do not finish the handshake in
time, and refuses replayed handshakes with a nonce store that fails closed when
full. `server.stats()` reports these counters. `streaming_fn` handlers send
chunks through a `ChunkSender`.

A validator connects and queries:

```rust,ignore
use std::time::Duration;

use rtao::quic::{QuicClient, QuicClientConfig, QuicTarget};

let client = QuicClient::new(validator_hotkey, QuicClientConfig::default())?;
let target = QuicTarget::new(miner_account, "203.0.113.7:8092".parse()?);
let sum: AddResp = client
    .query_typed(&target, "add", &AddReq { a: 2, b: 3 }, Some(Duration::from_secs(10)))
    .await?;
```

The client verifies the TLS 1.3 handshake signature against the presented
certificate, which btlightning's own client does not, and checks a pinned
`QuicTarget::fingerprint` when one is set. `QuicTarget::from_axon` builds a
target from chain axon fields; the chain does not mark QUIC endpoints, so the
operator decides which addresses speak QUIC. `retain_targets` closes
connections to miners that left the set. Local interop with btlightning's
Python bindings passed 58/58 in both directions, unary and streaming.

## Remote signing

`rtao-remote-signer` moves signing to a separate host that checks every
request against an operator policy. The client sends only the public views of
a prepared transaction, never secrets, to `POST <url>/sign`, authenticated with
btauth/1 by a client key that must differ from the signing account. It verifies
the returned signature locally before use and refuses raw message signing. The
server decodes the call against its own trusted runtime, re-derives every
signed byte, applies its policy and signs only what it derived. Replayed
requests fail the btauth/1 nonce check, tampered ones the body signature or the
re-derivation (`422 tampered_request`).

### Running `rtao-remote-signerd`

Build the daemon from a reviewed checkout:

```sh
cargo build --release -p rtao-remote-signer --features server-bin --bin rtao-remote-signerd
```

Write a configuration. Secrets never appear in the file; a keyfile password or
secret URI is read from the named environment variable:

```toml
[server]
listen = "127.0.0.1:8730"     # plain HTTP is allowed only on loopback
tls_cert = "/etc/rtao-signer/cert.pem"
tls_key = "/etc/rtao-signer/key.pem"

[chain]
endpoint = "wss://…"
genesis_hash = "0x…"
decimals = 9
token_symbol = "TAO"

[[accounts]]
keyfile = "/etc/rtao-signer/coldkey"
password_env = "RTAO_REMOTE_SIGNER_KEY_PASSWORD"

[[clients]]
address = "5…"                # the client's btauth key
accounts = ["5…"]             # accounts this client may use

[policy]
allowed_calls = ["Balances.transfer_keep_alive"]
allowed_destinations = ["5…"]
max_spend_tao = "10"
max_era_period = 128
allow_immortal = false
```

Check it, then serve:

```sh
rtao-remote-signerd --config signer.toml --check
rtao-remote-signerd --config signer.toml
```

Spend and fee caps apply to modeled transfers, so `allow_raw_calls` cannot be
combined with `max_spend_tao` or `max_fee_tao`, and `allowed_netuids` is
refused. The `daemon`
module documentation describes every option, including Redis replay storage,
rate limits and connection limits. Embed the server in another application
with `RemoteSignerServer` from the `remote-signer-server` facade feature.

### Using `--signer remote`

Build the CLI with the `remote-signer` feature. Then pass the server URL and
the keyfile holding the client key; the CLI unlocks it with its usual password
sources:

```sh
rtao --signer remote \
  --remote-signer-url https://signer.internal:8730/ \
  --remote-signer-client-key ~/.rtao/remote-client \
  --remote-signer-ca /etc/rtao-signer/ca.pem \
  --signer-address 5… \
  tx transfer --dest 5… --amount 1
```

`RTAO_REMOTE_SIGNER_URL`, `RTAO_REMOTE_SIGNER_CLIENT_KEY`,
`RTAO_REMOTE_SIGNER_RECEIVER` and `RTAO_REMOTE_SIGNER_CA` set the same values.
Without `--signer-address`, the CLI asks the server which accounts the client
key may use; pass `--remote-signer-receiver` (the server key) for that
discovery. With `--signer-address`, the receiver defaults to that account, so
pass `--remote-signer-receiver` as well when the server's `server_key` differs
from it. `--remote-signer-ca` replaces the platform roots, which pins a
private CA. `--remote-signer-timeout` takes whole seconds from 1 to 600 and
defaults to 30.

In a library, build a `RemoteSigner` and pass it wherever a `Signer` is
expected:

```rust,ignore
use std::{sync::Arc, time::Duration};

use rtao::primitives::CryptoType;
use rtao::remote_signer::{RemoteSigner, RemoteSignerConfig};

let mut config = RemoteSignerConfig::new(url, coldkey_account, CryptoType::Sr25519, client_key);
config.timeout = Duration::from_secs(30);
let (signer, _accounts) = RemoteSigner::connect(config).await?;
let signer: Arc<dyn rtao::signer::Signer> = Arc::new(signer);
```

Interop passed locally for a Python client using bittensor-wallet keys against
the Rust daemon (10/10) and the Rust client against a Python peer (2/2). The
localnet check, which signs and submits a transfer and refuses replayed and
tampered requests, runs in a hosted job whose result is pending.
