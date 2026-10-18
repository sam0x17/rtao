M0-23 adds typed golden and shape-corpus loaders in `rtao-testkit::fixtures`.
They accept vendored and recorder outputs, use the Value-first JSON parser,
retain read/event/receipt sections, and reject malformed hex and metadata
response framing. Golden metadata parses as V15 and V14 at its recorded spec
461, transaction version 1 and SS58 prefix 42.

The six golden tests reconstruct all 10 calls, 10 storage keys, two signature
payloads, two signed extrinsics and their hashes, three multisig accounts and
four SS58 addresses. Call arguments use `IntoCoreValue`; calls are composed by
pallet and function name. The nested-call descriptions in the recorder output
are reconstructed from its documented operations and pinned SS58 vectors.
Expected call bytes are never used as nested-call inputs. Calls, storage keys,
payloads and extrinsics are replayed against both metadata versions.

`corpus_replay` covers all 2,899 samples across 797 types, verifies that their
IDs plus the five explicitly skipped IDs cover all 802 registry entries, and
compares numbers using the digit-exact JSON comparator. It exercises both core
`decode_id` and the upstream `decode_spec` root adapter. Upstream deliberately
renders an empty composite as `{}` when nested and `[]` at the root; the test
checks both behaviors before comparing the root value to the corpus. This
matches upstream's own replay; the corpus and decoder are unchanged.

`corpus_primitive_visitors` decodes all 45 samples across 15 primitive type IDs
with metadata-directed typed visitors, encodes the typed projections through
the core, and compares both bytes and normalized JSON. AccountId and
DefaultAccount share the account samples. Fixed-point projections match type
IDs and paths because identical FixedU128/FixedI128 paths have different
fractional parameters; the test resolves each metadata Frac typenum and checks
the selected wrapper's fractional precision. Out-of-domain random wire samples
retain their raw values in visitors.

The original four feature-gated external lane targets are preserved. These
fixtures run without Python, Docker, nodes, or network access. Builds used a
shared build directory; no disposable directory or independent build cache was
created.

Validation passed with all 11 testkit tests and warnings denied:

```sh
cargo test -p rtao-testkit --tests --no-default-features --locked
cargo clippy -p rtao-testkit --tests --no-default-features --locked --no-deps -- -D warnings
```
