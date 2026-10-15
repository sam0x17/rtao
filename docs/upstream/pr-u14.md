# PR-U14: selective metagraph field discovery

Filed 2026-10-14 as
[RaoFoundation/subtensor#3244](https://github.com/RaoFoundation/subtensor/issues/3244).
The filing is authorized by PLAN.md execution rule 7 and Q18.

At v478 commit `850f2755bb05a3851f948489f0ab27b0645cebce`, the pallet's
`SelectiveMetagraphIndex` defines 77 selectors (0-76), with its private
`from_index` conversion. Selective metagraph/mechagraph APIs accept `Vec<u16>`,
so their metadata describes the numeric encoding without the selector names.
The source enum has no TypeInfo/SCALE derives and is not referenced by those
API signatures.

The rtao generator derives and checks every selector against the pinned pallet
source. Full regeneration passes on the verified Finney V15/V16 metadata,
including its mutation test for selector-order/mapping drift. The issue asks
for metadata-visible field identifiers or a read-only supported-index/name
API, preserving the existing numeric API. No runtime change or source patch
was submitted by this filing. Closed issues #1457 and #1810 were checked for
prior context before creating it.

The exact submitted body follows.

Selective metagraph callers currently need the pallet's Rust source to discover the meaning of `metagraph_indexes`. Exposing that mapping through runtime metadata or a read-only runtime API would let SDKs construct field presets for the connected runtime.

At v478 (`850f2755bb05a3851f948489f0ab27b0645cebce`):

- [SelectiveMetagraphIndex and its private from_index mapping](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/rpc_info/metagraph.rs#L488) define the field names and numeric indices.
- [get_selective_metagraph and get_selective_mechagraph](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/pallets/subtensor/src/rpc_info/metagraph.rs#L877) accept `Vec<u16>`. That metadata type exposes the numbers' encoding but does not describe which field each number selects.
- The enum has no `TypeInfo`/SCALE derives and is not referenced by those API signatures. Deriving TypeInfo alone would not make an otherwise unreachable type appear in runtime metadata.

In rtao, generation from pinned Finney V15/V16 metadata supplies the API descriptors, while a separate `syn` pass extracts this enum from pinned pallet source and verifies that declaration order agrees with every arm of `from_index`. The generator fails if those sources disagree. This works for a source-pinned SDK, but a live metadata consumer still cannot discover the mapping or safely select a newly added field without fetching and associating the runtime's source release.

Could the runtime expose the mapping in one of these ways?

- Include a typed field identifier in an additional metadata-visible API signature, preserving the existing numeric API for callers that already use it.
- Add a read-only API returning the supported `(index, field_name)` mapping for selective metagraphs and mechagraphs.

Either would allow SDKs to derive presets such as hotkeys/validator permits/stake directly from the connected runtime. The response's existing named optional fields already support decoding selected projections; this request concerns discovery of the input field selectors.

Related: #1457 introduced the selective API; #1810 concerned response decoding. The current v478 full/typed/dynamic decoding checks pass in rtao, so this is an API discovery request.
