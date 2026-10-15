# M5-01 offline call import review

The recovered `m5-source-candidate.patch` matches manifest SHA-256
`0afc4b1f4010c7031c2d8683935fd3c28da38f9e14a96775b8802240aea63031`.
Its embedded runtime change matches `runtime-call-import.patch`, SHA-256
`a044e682ffff8b3de8a4e49f243c75d395fd737978ff685b0bebbe71432104f9`.
Both were read from the recovery archive in memory. No archive extraction,
build or test campaign was run locally.

The candidate first performs a strict complete core decode, then walks the
metadata to reconstruct encoder values and requires byte-for-byte re-encoding.
This preserves byte leaves such as literal UTF-8 `0x41`, whose inspection
rendering otherwise resembles encoded hex. Review found three blockers in
`crates/rtao-runtime/src/handle.rs`:

1. The bridge has no independent recursion or element budget. A
   `primitive_types::U256` whose metadata field refers back to itself passes
   the core's fixed-width special decode, then recursively creates fresh
   cursors. The first decode's budget cannot bound this second walk.
2. A one-field newtype containing a one-element `Vec<Vec<u16>>` is flattened
   again by the pinned core encoder's legacy sequence handling
   (`codec/encode.rs`, `encode_fields`). The existing nested byte-vector
   fixture avoids this ambiguity because byte leaves use `Value::Bytes`.
3. `Option<Option<T>>::Some(None)` becomes `Value::Null`. The pinned encoder
   treats that value as outer `None`, so the final equality check rejects
   valid bytes. Its current Option value carrier cannot express this case.

The prepared correction passes one depth and collection-element budget through
every bridge call, including fresh U256 limb cursors. It reconstructs the
U256 composite's complete declared field shape. For a single unnamed sequence
field, the bridge supplies the extra list layer consumed by core's existing
legacy handling.

The pinned encoder cannot express `Some(None)` through its Option shorthand:
`codec/encode.rs` returns outer `None` for `Value::Null` and outer `Some` for
every other input. A private encoder view therefore prefixes only standard
Option presentation paths. This selects core's ordinary enum encoder, which
accepts explicit `Some` and `None` variants. Variant names, indices, fields
and referenced type IDs remain unchanged. The view is constructed from the
authenticated runtime and used only for encoding; it cannot supply runtime
identity, signing decisions, metadata digests or cache entries. The original
strict complete decode and final byte-equality check remain required.

Focused regression source in `crates/rtao-runtime/src/call_import_tests.rs`
builds complete V15 metadata and independently SCALE-encodes nested options,
nested sequence newtypes and arrays. It also covers a self-referential U256
and cumulative element charging across fresh U256 cursors. Recovered tests
retain malformed/trailing input, unknown extension, V14 refusal, byte-span,
ordinary call-shaped data, Era, MultiAddress and EVM U256 cases. Formatting
and source checks are local; execution and compilation remain unverified
until hosted CI runs them.

The independent read additions (`proxies` and `root_claim_threshold`) match
the current descriptors and native `u64` balances. They can be reviewed and
validated separately. The candidate contains no multisig-book or call-cache
writers, so it does not complete M5-01 by itself.

The five-file read subset has now been adopted from the authenticated patch:
`crates/rtao-client/src/read_decode.rs`,
`crates/rtao-reads/src/reads/owner.rs`,
`crates/rtao-reads/tests/owner.rs`, `codegen/read_types.toml` and
`parity/pending.toml`. No snapshot or generated output was restored.
`Proxy.Proxies` already describes an ordered vector of `ProxyDefinition` and
a native TAO deposit. The read retains that order and Python's ordinary TAO
display without applying the connection's custom symbol. The root claim
threshold reads only netuid zero through the current `I96F32` descriptor,
shifts away the fractional bits as Python does, then checks conversion to
`u64`; negative and oversized balances are refused. Its display uses the
client's connection-scoped token symbols.

Six recovered regression sources cover proxy fields and chain order, both
metadata defaults, fractional and native-balance boundaries, truncated SCALE
refusal, generated dispatch and namespace entry points, and exact descriptor
sets at pinned block hashes. Source and formatting review pass. Hosted
regeneration must update the read interfaces and declared parity inventory
before compilation and tests can establish acceptance.

Against committed `0812681`, the read pending list changes from 64 entries to
62 by removing exactly `proxies` and `root_claim_threshold`. This declares
40 of 102 read implementations in source; current generated dispatch and
successful hosted validation are separate requirements.
