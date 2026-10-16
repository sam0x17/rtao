# M5-01 client unsigned transaction JSON

The client subset was recovered in memory from
`m5-offline/proof/m5-source-candidate.patch`. Its SHA-256 matches both the
recovery manifest and the expected value
`0afc4b1f4010c7031c2d8683935fd3c28da38f9e14a96775b8802240aea63031`.
Only `crates/rtao-client/src/{lib,offline,submit}.rs` and
`crates/rtao-client/tests/offline.rs` were adopted. No snapshots, generated
files, dependency manifests or other milestone source were restored.

The public API adds `PublicPrepareOptions`, `UntrustedUnsignedExtrinsic`,
`Client::prepare_public`, `Client::import_unsigned_json`, unsigned JSON
serialization, tip getters and strict signature verification. Existing
`prepare_call`, `signing_request`, `attach_signature` and `submit_signature`
remain available. Public preparation uses a fresh chain nonce or an explicit
nonce without leasing cached state. External submission verifies the signature
before taking its explicit nonce lease. Client deadlines cover import and
submission; the executor can delegate without adding a retry or timeout loop.

Review compared all 17 wire fields with pinned Python
`sdk/python/bittensor/_transport/contract.py` and the payload construction in
`_transport/extrinsics.py`. The import requires an object with every field,
including nullable fields, and rejects unknown fields. Native unsigned bounds
remain enforced; the JSON signer view also enforces its four-byte nonce width.
The importer checks account/public-key agreement, crypto type, both runtime
versions, genesis, canonical era anchor and current mortal validity. It
reconstructs calls through strict runtime import and recomputes the normalized
signing payload, both signed-extension parts and every JSON signing field.
Supplied metadata hashes must match the independently authenticated metadata
and display constants. Imported JSON carries no policy authorization.

Source review corrected three gaps in the recovered candidate:

- Duplicate-key rejection now walks every object before `Value` parsing can
  collapse keys. It includes `era`, `payload_json` and objects nested in arrays.
  Root arrays and trailing input are rejected. The walk shares a one-million
  node limit and a 128-level depth limit.
  Independent review found that serde_json's enabled `raw_value` feature can
  reparse a literal private carrier string after this scan. The scanner now
  refuses private RawValue and Number keys in every object, preventing an
  embedded document from bypassing duplicate, schema or structural limits.
  Whole-document and nested signer-view/era/nonce fixtures cover this refusal.
- Unsigned JSON is limited to 64 MiB before parsing or the asynchronous import's
  input copy. This bounds repeated hex signing views and parser allocations.
- Direct signature attachment verifies the negotiated scheme, account and exact
  normalized payload. The external submission entry point retains its early
  verification before nonce mutation. Expired mortal preparations are refused
  before attachment.

Independent review also moved explicit nonce reservation after successful
attachment. A valid signature for stale runtime versions, a foreign genesis or
an expired era now refuses without clearing the caller's existing nonce cache.
The corresponding refusal fixtures pin and check that cache as well as the
absence of submitted transactions.

Serialization now errors on inconsistent internal era/address state instead of
silently omitting invalid fields. The recovered fixture's environment-selected
file writer was removed; ordinary tests do not create external seed files.

Regression sources cover both signature schemes, small and hashed payloads,
all required fields, nested duplicates, object-only input, tampered views and
parts, wrong signatures before nonce mutation, runtime upgrades, foreign
genesis, metadata hashes, mortality, fee bounds, deadline ordering, byte spans
and U256 calls. Focused structural tests cover cumulative node charging and
depth refusal. Installed formatting and diff checks are the only local
validation. Compilation, ordinary fixtures and strict checks remain required
in hosted CI. Cross-language interoperability is optional targeted hosted
coverage under Q30 and remains unverified by this record. No passing result
is claimed here.
