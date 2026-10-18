# Base and PSF notice distribution repair

The release builders now distribute the executable with six ordinary notice
files: `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `LICENSE-PSF`,
`webpki-root-certs-LICENSE` and `Unicode-DFS-2016.txt`. The full PSF text comes
from `crates/rtao-primitives/LICENSE-PSF`. The authentic texts and root NOTICE
were not edited by this task. The CPython change summary had already been added
to NOTICE.

All six are required standalone assets in the release inventory, checksums,
docs build record and GitHub docs attestation verification. CLI archive checks
require the executable followed by exactly this bundle. Rustdoc and book
archives retain and validate all six notices at their roots. Existing source,
tag, tree, toolchain, build, signature, attestation and traversal checks remain
in place. Staging rejects unknown or symlinked existing notice destinations.

| Distributed file | Source | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `LICENSE-MIT` | `LICENSE-MIT` | 1064 | `ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7` |
| `LICENSE-APACHE` | `LICENSE-APACHE` | 11358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `NOTICE` | `NOTICE` | 64953 observed | `6b8f081512991967a4f5b813993fc7fc519d714c885f798633966ee521087ee8` observed |
| `LICENSE-PSF` | `crates/rtao-primitives/LICENSE-PSF` | 13936 | `3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf` |
| `webpki-root-certs-LICENSE` | `docs/licenses/webpki-root-certs-LICENSE` | 2371 | `e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8` |
| `Unicode-DFS-2016.txt` | `docs/licenses/Unicode-DFS-2016.txt` | 2315 | `68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844` |

The five license texts have immutable pins. NOTICE follows source attribution,
including imported source pins, with a nonempty size ceiling of 1 MiB.
The release verifier compares it to both compiled source bytes and the clean
release-tag checkout. The observed NOTICE hash above is evidence for this
repair, not a permanent NOTICE pin. Changing both the checkout and asset copy
after compiling the verifier still fails.

Both Dockerfiles copy the full bundle into their existing license directories:
`/usr/share/licenses/rtao/` and `/usr/share/licenses/subnet/`. Build and runtime
stages check ordinary files, sizes and immutable hashes. Each build records
the source NOTICE size and hash, then checks the runtime copy against them.
The template helper checks both images and rejects removal of notice copies,
file guards, size guards, hash pins or NOTICE identity checks. The existing
Docker context blacklist already admits all six source paths and was unchanged.

Validation used real isolated Cargo targets under a recorded local task root, with
`PYO3_NO_PYTHON=1` and the shared no-op workspace compiler wrapper. The commands
ran against the actual workspace source, without modified source snapshots.

| Check | Rust 1.99.0 | MSRV 1.89.0 |
| --- | --- | --- |
| `cargo test --locked -p xtask release::tests::` | 10 passed | 10 passed |
| `cargo test --locked -p xtask template::tests::` | 4 passed | 4 passed |
| `cargo clippy --locked -p xtask --all-features --all-targets -- -D warnings` | Passed | Passed |

The release regressions exercise complete bundles, missing members, altered
bytes, oversized members and symlinks in source files, standalone assets, CLI
archives and docs archives. They retain the previous provenance, executable,
inventory and traversal cases. The new image regression caught and fixed a
NOTICE path-prefix match that could also match `NOTICE.size`.

The actual shell staging helper passed repeat-copy checks, unknown destination
preservation, missing/changed/oversized/symlink/dangling destination cases for
all six files, fresh versus stale source NOTICE checks and NOTICE source size
and symlink rejection. Both Dockerfiles passed static instruction, pinned-base
and RUN shell syntax checks. Their actual notice guard and notice staging
commands ran in rerooted fixtures with Mac adapters for GNU `stat`, `install`
and `sha256sum`. Both source and runtime guards passed positive and negative
cases, including dynamic NOTICE identity. These checks did not build or run
images. Actionlint 1.7.7 passed `release.yml` with shellcheck disabled because
that tool was unavailable; `bash -n` checked the four release scripts and the
staging proof. Rustfmt passed the two changed Rust files.

The current-toolchain test logs contain an existing `rust-objcopy` warning
about missing `libLLVM.dylib` while stripping debug information. The tests
passed, and strictClippy passed. No host tooling was changed.

The final Cargo.lock hash remained
`f911459259190907928b93d549d6f65a2bffc3fd7560aa484e3688dd633867d4`.
No manifests, locks, PLAN/state files, primitive source/licenses or vendored
data were changed. No publication, image execution, Python, GitHub or git
actions were performed. Final workspace gates ran separately.

Bounded proof, including final source hashes, test/lint logs and shell/image
rehearsal scripts and results, is not retained in the repository.
The cleanup audit found no process, open-file, launchd, configuration, inbound
symlink or virtualenv dependency on the disposable paths. It removed the exact
task-owned `targets`, `fixtures` and `tmp` directories. About 50 KB of proof
remained locally, including the cleanup record. Shared caches, tools and the
workspace wrapper were retained; they belong to the wider integration task. No live dependency on
the removed paths was retained.

The integration gate subsequently found that the complete Unicode notice
had been placed after the generated importer attribution. The importer requires
its generated block to be last. That complete authored Unicode section was
moved before the generated block, preserving its full text, the CPython
summary, every generated byte and the total 64,953-byte size. The current
NOTICE SHA-256 is
`2d7dace46be289acac1d65183fc600660b589cfcf00eb8977c7ab47d4b5dada7`.
The earlier hash in the table identifies the author's validation source;
source-bound NOTICE checks follow the corrected current bytes. Actual canonical
`import-upstream --verify-only` passes. The raw failure and corrected
verification logs are not retained in the repository. Full gates are rerunning against the
corrected layout, without relaxing importer or attribution checks.
