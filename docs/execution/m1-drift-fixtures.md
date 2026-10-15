# M1 drift fixture acquisition

Acquired on 2026-10-14 using HTTPS JSON-RPC and Rust. No local Python was run.
The new testnet/devnet fixtures and historical Finney fixtures are immutable
drift inputs. `upstream.toml` continues to select Finney 478 for code generation.

| Network / spec | Block | Endpoint | V15 SHA256 | V16 SHA256 |
|---|---:|---|---|---|
| Finney 477 | 9270000 | `https://archive.chain.opentensor.ai:443` | `64d37ffe1ee7e006d74f0a01c8e4bfb82e04a8907ac92c5600a49510862a76fa` | `af0870c1c32b53d8543859c6e0400551620401351a19bd6d111e4a629402acba` |
| Testnet 478 | 8218568 | `https://test.finney.opentensor.ai:443` | `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502` | `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7` |
| Devnet 478 | 1650396 | `https://dev.chain.opentensor.ai:443` | `17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502` | `723240b9f3761915eed75a5e158484bfdb39f6eea8ddd06d32e77996e67b14c7` |

`vendor/metadata/<network>/index.toml` records the genesis, finalized block hash,
runtime code hash, source tag/commit and each blob's size/hash. All V15 blobs
are 400688 bytes and V16 blobs are 404727 bytes. Testnet and devnet 478 are
byte-identical to the existing Finney 478 inputs, including the runtime code
hash `0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`.

For each network, genesis was checked first. Runtime version and `:code` hash
were read at one immutable block, V15/V16 were fetched with
`state_call("Metadata_metadata_at_version", encoded_u32_version, block_hash)`,
and version/code were read again at that block. The returned SCALE
`Option<OpaqueMetadata>` envelopes were checked for canonical compact lengths
and exact exhaustion. Rust `frame-metadata` 23.0.1 then used `DecodeAll` and
verified that re-encoding reproduces every byte, with the expected metadata
version and the embedded `System.Version.spec_version`.

The historical Finney block hash was returned by both configured archive
endpoints for height 9270000. The primary archive's finalized head was
`0x1baa3b39194cb1ed2737efb38060f049504e3779d4af0378c8974961e96c9155`
at height 9280219, above the fixture's canonical height. Its runtime code hash
`0x18b52484d9c5d7523f704c86b17b2ab959fffbc6edb6766a77e9877f0ad19387`
matches the downloaded final v477 release WASM's BLAKE2b-256 and the release's
srtool digest. The downloaded WASM SHA256
`e226dd0326b4480a6da7f0534e9854bc399889c934b074c92448cbb7cd0fab63`
matches both the upgrade manifest and srtool digest. Both source records
identify `5c6e83e2f4936f257234f246957f75684c35227c`, the immutable v477 tag.

Finney 477 versus devnet 478 differs at exactly one byte per metadata version:
V15 offset 307750 and V16 offset 307775 change `0xdd` to `0xde`, the spec-number
increment. This supplies the real 477→478 input for the semantic drift test.

The acquisition environment had no Docker daemon, local listener on port 9944,
or installed Docker/OrbStack application. A real `localnet-main` fixture still
requires booting the pinned `:main` image on Ubuntu and fetching its metadata.
Localnet has a faucet call and different fast-runtime constants, so copying
devnet or Finney metadata would not establish that baseline. No localnet
fixture or image execution result is claimed by this acquisition record.

The retained JSON record is `m1-drift-fixtures-provenance.json`. Temporary
downloads, scripts and checker sources lived only under
`target/execute-plan.6Ro0PC/tmp/drift-pins`; that exact directory was audited
for live dependencies before cleanup. Shared workspace build caches remain
available to the ongoing goal work.
