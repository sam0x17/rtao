# rtao-compat-synapse

Wire compatibility with `bittensor==10.5.0` Synapse, Axon and Dendrite peers.

The v10 protocol carries a JSON-encoded pydantic model over plain HTTP. A
dendrite signs `"{nonce}.{hotkey}.{axon_hotkey}.{uuid}.{body_hash}"` with
sr25519 and sends the signature and metadata in `bt_header_*` headers. Only
the subnet's `required_hash_fields` are bound by that signature; the remaining
body is unauthenticated.

`Envelope`, `TerminalInfo`, `Synapse` and `parse_head` implement the codec,
and `PyValue` and `get_hash` reproduce Python's `str()`/`repr()` body hash.
`default_verify` ports Python's verification with stricter defaults
(`V10VerifyPolicy`) and a bounded, expiry-only nonce table
(`BoundedNonceTable`). `V10VerifyPolicy::python` restores Python's behaviour
for differential tests.

Every deliberate difference from Python keeps wire compatibility with honest
v10.5.0 peers. The crate documentation lists them: bounded nonce windows,
timeouts and bodies, refusal of duplicate signed headers, verified identities
passed to handlers, and no axon signature over unverified input by default.

Python dicts preserve insertion order and the body hash renders them in that
order. Use `PyDict` (or another order-preserving map) for hashed `dict`
fields; `HashMap`, `BTreeMap` and `serde_json::Value` objects do not
round-trip Python's order.

## Features

| Feature | API |
| --- | --- |
| `axon` (default) | `Axon`, an axum server for synapse handlers, including streaming responses. |
| `dendrite` (default) | `Dendrite`, a reqwest client for querying axons, including streaming responses. |
| `tensor` (default) | The msgpack-numpy `Tensor` wire form. |
