# Versioned releases

A versioned release is an immutable `rtao-vX.Y.Z` tag on `main` with curated
notes in this directory (D-121). Snapshot tags (`snapshot-YYYY-MM-DD[.n]`) stay
available for pinning a revision between releases; they are not versions and
promise nothing about compatibility.

## Version numbers

- All workspace crates share `[workspace.package] version`; `rtao-arena`
  keeps its own number. The facade tag is `rtao-v{{version}}`
  (`release-plz.toml`).
- Before 1.0, a minor bump (0.x.0) may break public APIs or raise a persisted
  format's `readable_by`; a patch bump (0.x.y) does neither.
- The first versioned release is the current workspace version, 0.1.0. No
  `rtao-v*` tag exists before it, so nothing earlier is a version.

## What a release carries

- **The tag.** An annotated tag on a commit already on `main`. A pushed tag is
  never moved or deleted; a mistake is fixed by the next patch version.
- **Curated notes**, `docs/releases/rtao-vX.Y.Z.md`, committed before the tag.
  `cargo xtask release-notes --tag rtao-vX.Y.Z` refuses to render without them
  and requires these sections:
  - `## Changes by decision`: each change cites the PLAN decision (D-number)
    that records why it was made.
  - `## Journal compatibility`: the persisted formats this release reads and
    writes and what a rollback to the previous version does (see below).
- **Rendered notes**, the output of `cargo xtask release-notes`: core and parity
  pins, the CLI archive list, the downstream patch line, the curated notes and
  `CHANGELOG.md`.
- `COMPAT.md` and `compat.json` at the tagged commit.

## Journal compatibility section

State, for this release:

- the operation journal schemas it reads (migrated, native, and newer schemas
  that declare `readable_by` within its range) and the schema it writes;
- each SDK-owned record format inside the journal (weight work, attempt
  material, scheduler checkpoint) with its current format and range;
- whether any write raises a format above what the previous release reads, and
  which explicit action (such as compaction) does it;
- the rollback rule: which previous versions can open a journal this release
  wrote, and what they do when they cannot (refuse with a version error and
  leave the file unchanged; never reset).

## Steps

1. Land every change for the version on `main`, with CHANGELOG entries and
   D-numbers. Write `docs/releases/rtao-vX.Y.Z.md`; bump
   `[workspace.package] version` if needed. Rename `## Unreleased` in
   `CHANGELOG.md` to `## X.Y.Z - YYYY-MM-DD`. Commit.
2. Run the local checks on that exact commit: `cargo xtask release-notes --tag
   rtao-vX.Y.Z`, the focused crate tests and `cargo xtask gates` where the host
   allows (`AGENTS.md`).
3. Run the operator-run macOS lane on the exact commit (step 2 of
   `ci/release/README.md`). `macos-build.sh` checks out nothing; it needs a
   local tag at `HEAD`. Create the tag locally, run the lane, and push the tag
   only if every check exits 0. On a failure, delete the local tag
   (`git tag -d`), fix on `main` and start again from step 1.
4. Push the tag. While `release.yml` stays deferred
   (`.github/deferred-workflows/`), the release is a source release: create the
   GitHub release from the tag with the rendered notes and attach the macOS
   archives, `macos-build.json` and logs from the lane. Restoring `release.yml`
   later adds the hosted Linux builds, attestations and the signed draft flow
   described in `ci/release/README.md`.
5. A snapshot tag cut from the same commit reuses the curated notes as its
   annotation.

Publishing the crates to crates.io adds a step between pushing the tag and
creating the GitHub release; [crates-io.md](crates-io.md) has the order,
commands and remaining blockers (D-181).
