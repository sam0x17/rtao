# Runtime drift fixtures

`mutations.json` applies declarative changes to the checksum-verified, archived
Finney 477 V15 metadata. The drift tests decode the committed input, mutate its
portable metadata, re-encode it, and run the production semantic comparison.
The manifest keeps synthetic changes reviewable without duplicating entire
400 KiB registries. Every field unrelated to the named mutation stays intact.

The real Finney 477 and devnet 478 V15/V16 pair comes from the immutable rows in
`vendor/metadata/<network>/index.toml`. It changes only `System.Version`; full
regeneration must produce identical descriptor lines. `add-call` also checks
that adding an inner call cannot change Utility, Proxy or other wrapper-call
hashes through the opaque outer `RuntimeCall` type.

| Mutation | Expected class |
|---|---|
| Add one SubtensorModule call | `additive` |
| Change a storage default byte | `behavioural` |
| Move a call to an unused index | `breaking` |
| Add an extension with a u32 payload | `breaking` |
| Append a Metagraph read field | `behavioural`; projected fields unchanged |

A separate test repeats the field append on real spec 478. Event-index changes,
pallet/error indices, signed-extension ordering and implied payloads,
transaction/spec-name changes, V15 withdrawal and dropping extrinsic v4 have
independent guard tests.

Run `cargo test --locked -p xtask drift::tests`. To retain a real semantic report
and the independent `subxt diff` output, run:

```sh
cargo xtask drift diff vendor/metadata/finney/477.v15.scale \
  vendor/metadata/devnet/478.v15.scale \
  --old-v16 vendor/metadata/finney/477.v16.scale \
  --new-v16 vendor/metadata/devnet/478.v16.scale \
  --out "$RTAO_TEMP_ROOT/drift-real"
```

`subxt-cli 0.51.1` supplies the independent comparison; its result does not
replace rtao's classification of indices, events, errors or signing contracts.
