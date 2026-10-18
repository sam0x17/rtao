# rtao-btauth

Signed HTTP authentication (btauth/1) for Bittensor hotkeys.

`build_payload` constructs the signing bytes and `sign` produces request
headers. They bind the HTTP method, percent-encoded request target including
its query string, raw body bytes, nonce and account identities. `Verifier`
checks authentication and commits accepted nonces through a `NonceStore`.
The protocol primitives require no network service.

The default verifier requires a receiver, allows a ten-second nonce age and
two-second sender clock lead, and accepts raw or Polkadot-JS wrapped
signatures. `WrappedPolicy::Strict` selects raw signatures only. A nonce
store must retain entries strictly longer than the age plus allowed skew.
Use the final transmitted target and body when signing; each retry needs a
fresh nonce.

## Features

The crate has no default features.

| Feature | API |
| --- | --- |
| `client` | `client::SignedClient` and bounded concurrent axon queries. |
| `server` | Tower authentication, caller policies and axum extractors. |
| `metagraph-policy` | Server admission based on a metagraph snapshot; enables `server`. |
| `redis` | `RedisNonceStore` for shared atomic replay checks. |
| `legacy-v10` | Temporary `legacy` module for schema-specific v10 requests. |

`InMemoryNonceStore` belongs to one process. Multiple workers sharing a hotkey
need a shared replay store. The Redis adapter requires bounded memory with
`noeviction`; lost replay state requires an admission pause for the configured
retention window.
