# PR-U12: versioned metadata release assets

Filed 2026-10-14: https://github.com/RaoFoundation/subtensor/issues/3245.

Issue title: Publish V15 and V16 SCALE metadata with runtime releases

Issue body:

Downstream code generators and compatibility checks need immutable metadata
for the runtime in a release. The v478 release publishes `subtensor.wasm`,
`subtensor-digest.json`, `upgrade-manifest.json`, `pending-release.json` and
`proxy_proxy_blob.hex`, but no raw V15 or V16 metadata.

Could the release train publish both metadata versions from the exact runtime
WASM it builds, alongside their SHA256 checksums and the runtime spec/commit?
For example, `metadata-v15.scale` and `metadata-v16.scale` could contain the
raw `MetadataVersioned` SCALE bytes after unwrapping the runtime API's
`Option<OpaqueMetadata>` response. The asset names are a suggestion; a stable
documented contract matters more than the names.

This would let consumers regenerate descriptors and compare runtime changes
without contacting a public node or depending on that runtime remaining in
an archive. Metadata should come from the published WASM, since a network's
currently active runtime can differ from the release candidate.

Evidence for v478, commit
`850f2755bb05a3851f948489f0ab27b0645cebce`:

- Release:
  https://github.com/RaoFoundation/subtensor/releases/tag/v478
- The release train's collect/upload steps attach the WASM and digest, with no
  versioned metadata:
  https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/.github/workflows/release-train.yml
- We fetched both versions from Finney at finalized block
  `0x2bc979722ab7c83b9992900c5463dfd1daebbac77d5e64ae1e45413c69e5401f`
  and runtime code hash
  `0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`.
  V15 was 400,688 bytes, SHA256
  `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502`;
  V16 was 404,727 bytes, SHA256
  `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7`.
  These observations demonstrate availability; release assets should be
  generated from the release build itself.

Related prior metadata issues #384, #910 and #3159 concern client error data,
UI metadata and compiled signing hashes respectively. They do not request
versioned release assets. I found no existing issue for this asset contract.

Local evidence: `vendor/metadata/finney/index.toml` and
`docs/execution/m1-drift-fixtures.md`. No local Python was run. This request
starts with an issue because release maintainers need to choose where the
versioned-runtime export belongs in their train; no unvalidated workflow
patch or asset is presented as complete.
