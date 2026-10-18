# Finalized Finney metadata input

The metadata portion of M1-03 was prepared while the M0 BLS decision remained
pending. M1-03 also requires the runtime and code generator; these metadata
inputs alone do not complete that step.

`cargo xtask pin --parity v478` fetched V15 and V16 from
`https://entrypoint-finney.opentensor.ai:443` at finalized block
`0x2bc979722ab7c83b9992900c5463dfd1daebbac77d5e64ae1e45413c69e5401f`.
Both requests used `state_call("Metadata_metadata_at_version", version.encode(),
at)` at that exact block. The command verified spec 478, Finney genesis
`0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03`,
and code hash
`0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`
against the immutable v478 release before fetching. It repeated the runtime
version, genesis and code reads before the two metadata calls. The release
commit is `850f2755bb05a3851f948489f0ab27b0645cebce`.

| Raw input | Bytes | SHA256 |
| --- | ---: | --- |
| `vendor/metadata/finney/478.v15.scale` | 400688 | `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502` |
| `vendor/metadata/finney/478.v16.scale` | 404727 | `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7` |

The stored bytes exclude the `Option<OpaqueMetadata>` envelope. Envelope
decoding requires `Some`, canonical SCALE and complete consumption. Raw
metadata requires `meta`, the requested version, complete frame-metadata 23
decoding and canonical re-encoding. V16 is a tooling input; the runtime path
continues to use V15 through bittensor-core.

The command generated `vendor/metadata/finney/index.toml` and the metadata
paths and SHA256 fields in `upstream.toml`. Existing runtime rows and blobs
are immutable. A repeat fetch must match the recorded code hash, tag, commit
and both blob hashes, then retain the original finalized-block provenance.
Publication stages files under the recorded task root, links complete blobs
without replacing an existing file, writes the index, then publishes the
upstream pin last. A repeat generation recovers matching blobs left by an
interrupted publication. A same-tag refresh preserves the 360 imported
artifact rows instead of replacing them with bootstrap seeds.

`cargo test --locked -p xtask metadata_pin::tests` passed six tests covering
strict envelopes, full upstream V15 corruption and truncation, bounded hex,
same-block RPC requests and identity failures, and offline blob/pin/provenance
tampering plus symlink rejection, and repeat-fetch byte immutability and
original provenance retention. Strict xtask Clippy passed. Plain `pin --check` with
`RTAO_FINNEY_HTTP=https://127.0.0.1:1` verified the metadata offline, then failed
at the unchanged literal BLS requirement:
`Cargo.lock must contain exactly one w3f-bls; found 2`.

The task used a recorded local task root for its copied executable and logs,
and the shared `build/` and upstream Git cache for compilation and immutable
release checks. The copied executable and logs were removed after this note
retained their results and a process, launchd, configuration, symlink and
virtual environment audit found no live dependency. The shared build and Git
cache remain dependencies of ongoing plan execution.
GitHub had no `main` workflow runs before or after this preparation.
