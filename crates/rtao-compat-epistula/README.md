# rtao-compat-epistula

Epistula v2 signed HTTP headers, compatible with the epistula.sybil.com
reference, SN34 BitMind, the gobt boilerplate (SN4 Targon) and SN1 Apex
clients.

Epistula signs `sha256(body) "." uuid "." timestamp_ms "." receiver` with the
sender's sr25519 hotkey. `sign_headers` produces the `Epistula-*` headers and
`Verifier` checks them. A `Dialect` preset selects the upstream variant:

| Preset | Use |
| --- | --- |
| `Dialect::RTAO` (default) | BitMind wire format; canonical timestamps, 15 s age, 2 s future bound, uuid replay store |
| `Dialect::BITMIND` | BitMind parity: Python `int()` timestamps, ±15 s |
| `Dialect::GOBT` | gobt senders and receivers: always-bound, sender-subject secret signatures |
| `Dialect::SPEC` | Reference signer. Its verifier never compares `Signed-For` with self; use only for tests |
| `Dialect::APEX` | SN1 Apex clients: no uuid, unbound, unsigned `X-Request-Id` |

Verification never echoes the signed message. Duplicate authentication
headers, oversized fields, `0x0x` or whitespace in signatures and uuids
outside 1..=128 visible ASCII bytes are rejected before any curve
operation. Error `Display` strings are the BitMind `detail` strings.

A replay-enforcing `Verifier` requires bounded age and future skew and a
store retaining entries longer than their sum. The replay identity is a
sha256 of the uuid (or, without one, the signature), so btauth stores can be
shared. Every retry must re-sign. `Authenticated::commit` takes a live clock
and refuses a commit unless the time since the freshness check plus
`max_age_ms + max_future_ms` stays below the store's retention, so a server
may measure freshness at the request head without opening a replay gap.
`Verifier::verify` measures freshness when it is called.

## Server timing and binding

`server::EpistulaLayer` checks the headers and freshness when the request
head arrives, as BitMind does, so a slow upload is not rejected as stale.
The body read, policy and replay commit must finish within the layer's body
deadline (default 30 s, shortened to fit the store) or the request gets 408;
`with_body_deadline` refuses a deadline unless the store retains entries
longer than `max_age_ms + max_future_ms + deadline`. The layer raises axum's
`DefaultBodyLimit` to its own body limit for the inner service.

An Epistula signature covers the body digest, uuid, timestamp and receiver,
but not the HTTP method, path or query. Over plain HTTP an on-path attacker
can resend an undelivered signed request to another route. Mount Epistula
only on routes whose meaning is in the body. The layer therefore inserts a
btauth `Caller` (so `VerifiedCaller` and `DualAuthLayer` btauth handlers
accept Epistula requests) only with `with_btauth_caller(true)`; Epistula
requests always carry an `EpistulaCaller` extension.

## Deviations from the spec

* D-E9: `with_claimed_sender_prefilter` is off by default, although spec
  section 10.5 turns it on with the BitMind status map. The prefilter runs
  the caller policy on an unauthenticated claimed sender, which is unsafe
  for policies with per-caller side effects. When on, the policy runs once
  per request: its admission is reused for the identical verified caller.
* D-E10: the secret-signature filter verifies `Secret-Signature-1` for the
  interval of the claimed `Epistula-Timestamp`, after checking that
  timestamp's freshness, instead of the receiver's current and previous
  intervals. Every upstream sender derives its secret signatures from that
  timestamp, so this is one verification and never rejects a fresh request.
  Its cache holds exact `(sender, signature, interval)` triples.
* D-E11: freshness is measured at the request head, and the body read,
  policy and commit are bounded by a body deadline, as described above.
* Apex clients sign `{}` for an empty body and the query parameters for a
  `GET`. `client::sign_reqwest` hashes the wire body; use
  `sign_reqwest_with_digest` or `EpistulaClient::execute_with_digest` with
  `apex::canonical_json` for those requests.
* `EpistulaHeaders::apply` removes every existing `Epistula-*` header before
  inserting, so re-signing never leaves stale headers.

## Interop coverage

BitMind and Apex verdicts come from the pinned upstream Python code
(`ci/interop/epistula`, with the pinned `requirements.txt`). The spec
verifier and the secret-signature checker in the harness are transcriptions,
and the RTAO and GOBT parity verdicts reuse BitMind's with hand-written
deviations. No Go verifier has run: treat GOBT verification as unverified
until a gobt CI job exists.

## Features

The crate has no default features.

| Feature | API |
| --- | --- |
| `server` | `server::EpistulaLayer`, `DualAuthLayer`, `VerifiedEpistula` and `StatusMap` |
| `client` | `client::sign_reqwest` and `EpistulaClient` |
| `metagraph-policy` | Re-exports btauth's `MetagraphPolicy` for the server layer |
| `redis` | Re-exports btauth's `RedisNonceStore` |
| `secret-filter` | `secret::SecretSigFilter`, an optional pre-body DoS filter |
