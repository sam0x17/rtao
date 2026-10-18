# rtao-transport-quic

A btlightning-compatible QUIC transport for Bittensor neurons, built on quinn
and rustls with the ring provider.

The crate reimplements the wire protocol of
[btlightning](https://github.com/inference-labs-inc/btlightning) at
`53c090b436238f78a4a0dd30cbbfcba1cbebe2e0` (v0.2.15). It does not depend on
btlightning, which requires `sp-core 34`, so it shares rtao's `sp-core`-free
signing stack.

## Wire summary

- ALPN `btlightning`, TLS 1.3, self-signed server certificate, no client auth.
- One client-initiated bidirectional stream per handshake or request. The
  client writes one frame and then FIN.
- Frames are a `u8` type, a big-endian `u32` length and a MessagePack payload.
- Envelopes are positional MessagePack arrays. Decoders also accept maps.
- The handshake signs these strings with raw sr25519 and sends the signature
  as standard base64:
  - `handshake:<validator>:<ts>:<nonce>:<fp>`
  - `handshake_response:<validator>:<miner>:<ts>:<nonce>:<fp>`

  `fp` is the base64 BLAKE2b-256 of the server certificate DER.

## Differences from btlightning

None of these changes the wire format. Each rtao peer interoperates with
btlightning's Python bindings in both directions (see `ci/interop/quic`).

- **TLS signature check.** btlightning's client verifier accepts every TLS 1.3
  `CertificateVerify` signature without checking it. A relay on the network
  path can then present the miner's public certificate without holding its
  key. The relay can also forward the signed handshake, so the fingerprint
  binding does not stop an active attacker. rtao's client verifies the
  signature against the presented certificate. It still accepts any
  self-signed certificate, unless the target pins a fingerprint.
- **Connection-scoped authorization.** btlightning authorizes requests by the
  remote `SocketAddr`. rtao's server authorizes each QUIC connection and allows
  one validator identity per connection, also when handshakes for two
  identities race on one connection. A second connection from an
  authenticated client's UDP socket gets `"authentication failed"` until that
  connection completes its own handshake.
- **Replay protection.** rtao uses `rtao_btauth::NonceStore`, keyed by
  validator and nonce. A store must keep nonces for at least the signature
  age plus the 60 s future skew. The default store keeps them 60 s longer,
  so a backward wall-clock step of up to 60 s cannot reopen a replay window
  (freshness uses the wall clock, expiry a monotonic clock). A full store
  rejects new handshakes and never evicts live nonces; `ServerStats` counts
  these rejections. The hotkey policy runs before a nonce is stored.
- **Hotkey policy.** Without a `HotkeyPolicy`, as in btlightning, any
  sr25519 key can authenticate, and enough source addresses (about 560 IPv4
  addresses at the default limits) can keep the nonce store full. `bind`
  logs a warning when no policy is set. Production miners should admit only
  known validators, for example those holding a validator permit.
- **Per-source limits.** The handshake rate limit and the pre-authentication
  cap key IPv6 sources by /64 prefix (`server::source_key`); btlightning
  keys by full address.
- **Unauthenticated connections.** They are capped globally (256) and per
  source (16). When the global cap is reached, the oldest unauthenticated
  connection of the source holding the most is closed with reason
  `preauth_evicted`, so a few sources cannot keep validators out. Any
  connection that has not completed a handshake within `handshake_timeout` is
  closed with reason `handshake_timeout`. Before authentication, a first
  frame larger than `max_preauth_frame_payload` (64 KiB) is answered without
  reading its payload: `"authentication failed"` for a `SynapsePacket`, a
  rejection for a `HandshakeRequest`.
- **Reply deadlines.** Every server reply write is bounded by the client's
  request budget (5 s plus 1 s per started MiB, at most 60 s). A peer that
  withholds flow control gets the stream reset.
- **Streaming timeouts.** A frame that has started is written to completion
  within a grace period, or the stream is reset. A `StreamEnd` never follows a
  partial chunk. Joining the handler task is bounded by the stream budget.
- **Typed enums.** `to_data` emits the same shapes as btlightning's
  `NamedSerializer`. `from_data` also accepts the index, `{index: value}`,
  variant names, and rmpv's `[index, value]` arrays. btlightning cannot decode
  the enum shapes its own encoder emits.
- **Client pool.** `max_connections` counts addresses with a live or pending
  connection; addresses whose dial failed are forgotten to make room. A
  target's fingerprint pin is checked when a pooled connection is reused.
  `retain_targets` replaces btlightning's `update_miner_registry`: it closes
  connections to dropped addresses and forgets miner hotkeys that left a
  kept address.
- **Discovery.** The chain `protocol` byte does not indicate QUIC.
  `QuicTarget::from_axon` takes the address fields only, and the operator
  decides which endpoints speak QUIC.

## Features

- `client` (default): `QuicClient`, `QuicTarget` and streaming replies.
- `server` (default): `QuicServer`, `QuicRouter`, hotkey and source policies.
  This feature depends on `rtao-btauth` for its nonce store.
