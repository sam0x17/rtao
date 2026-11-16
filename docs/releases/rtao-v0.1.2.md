## About this release

A compatible patch for subnet conviction reads on runtime 491, with build
fixes and runtime adoption tooling updates. The generated descriptors remain
pinned to 490; the owner-lock read adapts to the queried block's metadata.
Regenerated 491 APIs belong to a subsequent minor release.

All shared-version workspace crates move to 0.1.2 (D-121). `rtao-arena`
remains at 0.2.0 and the `rtao-upstream-*` packages remain at 0.490.0;
neither is republished. The minimum supported Rust version remains 1.90.

## Changes by decision

- D-193: `subnet_convictions` reads the owner lock maps only where the
  queried runtime has them. On runtime 491, which removes both maps, it
  rolls the owner's hotkey lock rows as owner locks. The read checks the
  types of maps that are present and preserves the behavior on 490 blocks.
  Tests cover recorded 491 metadata, both older maps, either map alone,
  empty maps, changed owner hotkeys and owners without lock rows.
- D-191: the development toolchain moves to Rust 1.100.0. Optional
  dependencies follow the features that use them; unused dependencies are
  removed. The MSRV remains 1.90, and no public item changes.
- D-192: builds of `rtao-intents` with only `gamma` or `limit_orders` now
  include their required helper. The facade records intentionally enabled
  dependencies for Cargo's unused-dependency lint without changing feature
  resolution or public APIs.
- D-194: runtime adoption records new upstream environment names as
  pending parity work with authenticated source evidence. Existing
  decisions remain validated, vanished names fail, and strict parity
  continues to reject pending behavior. Ordinary extraction still requires
  exact inventory coverage. Testnet upgrade fixtures use a spec above the
  recorded testnet index even when testnet is ahead of Finney.

Testnet 491 metadata is also recorded in the runtime index. This does not
change the parity pin or claim validation of the full runtime surface.

## Journal compatibility

No persisted format changes. The operation journal reads schemas 1 to 4,
plus newer schemas whose `readable_by` is 4 or lower. It writes schema 3
until `OperationJournal::compact` raises a finished operation to schema 4.
Weight work, attempt material and scheduler checkpoints remain at format 1.

Versions 0.1.0 and 0.1.1 can open the journals and records that 0.1.2 writes.
A newer format that declares an incompatible `readable_by` is refused with
a version error, leaving the file unchanged. A rollback to 0.1.1 loses the
runtime-491 owner-lock fix; reconciling unresolved transactions still
requires finalized chain evidence.

## Release artifacts

The hosted release workflow remains deferred. Install the CLI from
crates.io with `cargo install --locked rtao --version 0.1.2`. macOS archives
are attached after the operator-run release lane passes on the tagged
commit. No validated runtime range is claimed in `COMPAT.md`.
