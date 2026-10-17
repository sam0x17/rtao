# rtao-remote-signer

Policy-checked remote transaction signing for rtao, over the rtao-authored
`rtao-remote-signer/1` protocol (divergence D-031; Python has no counterpart).

- `wire` (always): request, response and error bodies with strict `0x` hex and
  SS58 encodings.
- `client` feature: `RemoteSigner`, an `rtao_signer::Signer` that sends the public
  views of a prepared transaction to `POST <url>/sign`, authenticated with
  btauth/1 by a client key that must differ from the signing account. The
  returned signature is verified locally before use. Raw message signing is
  refused.
- `server` feature: `RemoteSignerServer`, an axum router behind `BtAuthLayer`
  with a client allowlist. The handler decodes the call with the server's own
  trusted runtime, re-derives the signed extension bytes and the payload from
  the SignerPayloadJSON view, checks era and call policy, signs only the bytes it
  derived and self-verifies the result.
- `server-online` feature: `ClientRuntimeSource`, which trusts an operator-chosen
  node and checks era anchors from headers alone (number, then phase and live
  window, then canonical hash, never the anchor's runtime) and can estimate
  fees.
- `server-bin` feature: the `rtao-remote-signerd` daemon with a TOML
  configuration (see the `daemon` module documentation), TLS, and per-connection
  limits (connection cap, handshake, header, request and lifetime timeouts).

Policy limits are enforced only where the server can measure them: spend and
fee caps apply to modeled transfers, so `allow_raw_calls` cannot be combined
with `max_spend` or `max_fee`, and `allowed_netuids` is refused because no
decoded call is modeled with a subnet.
- `redis` feature: a Redis replay store for multi-process servers.

Replayed requests are refused by the btauth/1 nonce store, tampering in transit
by the btauth/1 body signature, and internally inconsistent requests by the
server's re-derivation (`422 tampered_request`).
