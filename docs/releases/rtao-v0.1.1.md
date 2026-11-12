## About this release

A patch release. Its main change: `cargo install --locked rtao` now installs
the `rtao` command line tool. The binary moved from the `rtao-cli` crate to
the `rtao` crate, which builds it through the new default `cli` feature. The
rendered notes (`cargo xtask release-notes --tag rtao-v0.1.1`) add the pins,
the published packages and this version's changelog section.

Every workspace crate is published at 0.1.1, following the shared version
(D-121). `rtao-arena` stays at 0.2.0 and the `rtao-upstream-*` packages stay
at 0.490.0; neither is republished. Rust 1.90 is still the minimum supported
version.

## Changes by decision

- D-190: `cargo install rtao` installs the CLI.
  - `rtao` has a `[[bin]] rtao` that needs the `cli` feature. `cli` is a
    default feature, so the install needs no flags. `cargo install --locked
    rtao --features ledger,vault,extension,evm` adds those backends: the
    facade's `ledger`, `vault`, `extension`, `evm`, `pow-gpu` and
    `remote-signer` features also enable the CLI's.
  - `rtao-cli` is the library behind the binary and has no binary of its own.
    `rtao_cli::entry::main` is the process entry point. It now depends on the
    workspace crates directly rather than on the facade, and its public API
    and resolved features are unchanged.
  - Libraries should depend on `rtao` with `default-features = false` and name
    the features they use. The README, the book and the subnet template
    already do. With the defaults on, a build also compiles the CLI's
    dependencies, including the vendored OpenSSL, libsodium and secp256k1 C
    builds. The library itself never links the CLI. `full` does not include
    `cli`, and docs.rs builds without it.
  - If you installed `rtao-cli` 0.1.0, run `cargo uninstall rtao-cli` before
    `cargo install rtao`; otherwise cargo refuses to overwrite the `rtao`
    binary. `rtao-cli` 0.1.0 is the last version that installs a binary.
    Running `cargo install rtao-cli` against 0.1.1 or later finds nothing to
    install.
  - The release lanes build `-p rtao --bin rtao`. The archives still contain
    `rtao` and the licence files. The subnet template's CI installs its doctor
    from `rtao`, or from `rtao-cli` when the resolved SDK is 0.1.0.
- D-183, D-184: the republished packages' checksums. `rtao-core` and
  `rtao-testkit` 0.1.1 record the crates.io SHA256 of every
  `rtao-upstream-*` 0.490.0 package, so `rtao doctor` compares a workspace's
  `Cargo.lock` with those checksums. In 0.1.0 the doctor checked versions
  only.
- D-188, D-189: repository automation only, with no change to any published
  crate's behaviour. The hourly runtime-adopt workflow publishes new
  `rtao-upstream-*` versions and records their checksums. Its core-bump guard
  admits only the republished packages' manifests. The subnet template
  depends on rtao from crates.io.

## Journal compatibility

No persisted format changes in this release. The operation journal reads
schemas 1 to 4, plus any newer schema whose `readable_by` is 4 or lower. It
writes schema 3 until `OperationJournal::compact` raises a finished operation
to schema 4. The weight work, attempt material and scheduler checkpoint
records stay at format 1. All of this is the same as 0.1.0.

Rollback:

- To 0.1.0: it opens every journal and record that 0.1.1 writes. Libraries
  can pin `rtao = "=0.1.0"`. For the CLI, run `cargo uninstall rtao` and then
  `cargo install --locked rtao-cli --version 0.1.0`.
- From a later release to this one: the same rule as 0.1.0 applies. A later
  format declares `readable_by`. This release opens it when the declaration
  covers it and otherwise refuses with a version error, leaving the file
  unchanged.

## Not in this release

- No validated runtime range is recorded in `COMPAT.md`.
- The hosted release workflow is still deferred, so there are no hosted Linux
  archives or attestations. Install the CLI from crates.io with
  `cargo install --locked rtao`. macOS archives are attached only when the
  operator-run macOS lane ran for this tag.
