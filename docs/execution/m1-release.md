# M1 release automation

The release implementation is ready for review. M1-23 is not complete: no
milestone tag or hosted release run was created, and local release key approval
and authenticated tagged Mac artifacts remain pending.

`release.yml` prepares Linux x86_64 and ARM CLI archives on free
`ubuntu-latest`, using the exact Ubuntu 22.04 amd64 manifest
`sha256:08ea48a03a3e78ebc7cd526e6a275053223aadd88bfc09cc49b06d5281525fde`.
Jammy ARM target libraries, cross compilers and QEMU preserve the glibc 2.35
floor. The binary's imported GLIBC versions are checked; toolchain, package
inventory, elapsed time and target storage are recorded. Hosted resource use
and cross/QEMU execution still need a real run.

The hosted docs job generates exact compatibility checks, pin-based notes,
strict rustdoc, a first mdBook skeleton with the 357-row parity index, and LLM
text containing public Rust signatures. Both jobs retain GitHub attestation
bundles for only their actual build outputs. Complete draft assembly happens
locally after both Mac targets, ten check logs, signatures, checksums and every
hosted artifact/provenance pass verification. A separate read-only dispatch
checks the complete draft; publishing repeats verification immediately before
changing its draft state. No Pages, paid or hosted Mac/ARM runners are used.

`cargo xtask release verify` binds records to the exact clean source tag/commit/
tree and pinned Rust/Cargo versions. It authenticates GitHub attestations against
the repository, release workflow, source ref and source/signer commit, and
rejects self-hosted attestations. It separately authenticates the local Mac
record with an approved Ed25519 OpenSSH signature in namespace `rtao-release`.
The trust-anchor file is intentionally absent until the user approves the key
design. Missing keys, targets, logs, hashes or provenance fail verification.
Archive verification rejects links and paths outside the extraction directory.
No script or xtask mutates the user's Keychain.

`release-plz.yml` exits unless upkeep is enabled or a read-only dry-run is
dispatched. Its isolated rehearsal runs the pinned updater and SemVer checker,
regenerates compatibility, and executes full gates. The live upkeep path uses
the App token, tags only the facade, and regenerates/checks compatibility in
the actual release PR before pushing generated files. Git-only mode disables
crates.io; release-plz's GitHub release creation is disabled. This path has not
been dispatched or used to create a PR.

Validation on the Mac Studio worktree:

- All eight release verification tests pass on Rust 1.99 and 1.89, including
  authentic ephemeral Ed25519 signature verification, record tampering, wrong
  source/toolchain, missing targets/keys, extra assets, archive traversal and
  docs links. Test keys existed only in newly-created fixture directories and
  were removed at test completion.
- The LLM signature visibility/owner test passes on both toolchains. Docs output
  generation and offline checking pass; the generated parity index has 357 rows
  and the LLM text has 5,989 lines.
- Strict xtask Clippy passes. actionlint 1.7.12 and ShellCheck 0.11.0 pass for
  both workflows and all release shell scripts.
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps
  --all-features` passes, without a facade/CLI output collision. mdBook 0.5.4
  builds the skeleton and generated index. Local downloadable archive
  rehearsals were 7.5 MiB rustdoc and 8.2 MiB book including rustdoc.
- Both optimized CLI targets build: native `aarch64-apple-darwin` and cross-built
  `x86_64-apple-darwin`. Both facade all-feature checks and all seven CLI tests
  pass. Native and Rosetta `--version` report `rtao 0.1.0 (bittensor-core v478)`.
  Both `doctor` commands correctly fail the still-unapproved literal two-entry
  BLS lock criterion.
- Intel/Rosetta nextest runs 631 tests: 630 pass, one fails at the literal BLS
  lock criterion, two are skipped. Native Apple ARM runs 633 tests: 631 pass,
  two fail (the BLS criterion and a concurrently-added canary compatibility test
  with `missing evidence string tag`), two are skipped. These are incomplete
  platform gates, not passing release provenance. Root/owning agents received
  the failures.

The builds used Rust/Cargo 1.99.0 on macOS 26.6.2 with Apple SDK 27.0. Native
binary SHA-256 was
`ac8b4ed0350154b97af3d3028c8ab28e08f145f943e1405cb7d81d5dc23a88bf`;
Intel binary SHA-256 was
`f84c83a20d28f131864b62a48b676049cb251b020aea6775efcd2f2f66434caf`.
Source HEAD was `7efef6f68eedc7cfb85e6e9d0a5ae3da7c8d442a` with a dirty shared
implementation worktree. These hashes describe rehearsals, not assets built
from a release tag. Final tagged artifacts must be rebuilt and authenticated.

An isolated checksum-verified Intel standard library was provisioned under the
recorded task root for the cross build; the user's installed Rust targets and
defaults were not changed. Disposable release tools/docs/target fixtures are
audited and removed. Bounded logs are retained alongside this report as
`m1-release-local-build.txt`, `m1-release-{intel,native}-failure*.txt` and
`m1-release-rustdoc.txt`.
One mdBook rehearsal initially interpreted a relative output path against the
workspace parent; that newly-created output was audited, removed immediately,
and the build repeated with the absolute task-root path used by the shipped
script. The shared build and gate/source caches remain live for
root and other workers. Full root gates, hosted Linux/resource evidence,
authenticated final-tag Mac checks, approval of the local trust anchor, and
actual draft verification/publication remain prerequisites.
