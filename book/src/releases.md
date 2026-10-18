# Release verification

Release tags, binary distribution, publishing and release-provenance setup
are not yet in place. Current development delivers reviewed source
and applicable hosted client checks. It does not establish that CLI archives
exist, that a release was verified or that the project is ready for a release.

## Development artifacts and coverage

The active `pr-gates` workflow runs on free `ubuntu-latest` Linux x64 runners.
When its documentation stage succeeds, it can retain the book, rustdoc and
generated reference files as a `documentation-<commit>` Actions artifact.
Check the run's source commit and job results before using its outputs. A docs
artifact is development output and carries no release or platform acceptance
claim.

Native macOS, ARM, Keychain and hardware verification remain unverified unless
separate executed evidence is recorded. Cross-platform release rehearsals,
macOS builds and device runs are deferred; they are not prerequisites for
client development. The former release workflows are retained outside
GitHub's active workflow directory in `.github/deferred-workflows/`.

`COMPAT.md` and `compat.json` record pins, metadata inputs and any validated
runtime range. A metadata pin or successful Linux build is not proof of
live-chain, interoperability or hardware coverage. Read the exact commit's
hosted results alongside those records.

## Future release verification

The repository retains release builder and verifier source for future work.
Its presence does not demonstrate that the workflows ran, attestations were
issued or a local signing trust anchor was approved. A future distribution
effort must review that contract and record its own executed evidence.

For any future download, checksums establish integrity only after the source
and artifact provenance have been authenticated. Platform claims must be
supported by builds and checks for those platforms. No local release rehearsal
is required by this guide.
