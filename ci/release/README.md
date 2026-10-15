# Release automation

Every hosted job uses free `ubuntu-latest`. `release.yml` has two phases because
this runner does not build the supported macOS CLI targets.

1. Pushing `rtao-vX.Y.Z` builds/checks both Linux targets in a digest-pinned
   Ubuntu 22.04 amd64 container, records package/toolchain/source identities and
   the highest imported GLIBC version, and requires that version to be at most
   2.35. ARM uses the cross compiler, jammy ARM target libraries and QEMU. A
   separate job generates compatibility, notes, rustdoc, mdBook and LLM docs.
   Pinned GitHub actions attest only artifacts those jobs built. All outputs
   remain downloadable Actions artifacts; this phase creates no release.
2. From the same clean source tag on Mac Studio, download the successful tag
   run's artifacts into `$RTAO_TEMP_ROOT/dist` with `gh run download <run-id>
   --repo sam0x17/rtao --pattern 'release-rtao-vX.Y.Z-*' --dir <scratch>`, then
   merge their files into that dist directory. Run `bash
   ci/release/macos-build.sh rtao-vX.Y.Z`. It requires existing Rust targets and
   Rosetta, checks both facade targets, runs the `ci` nextest suite and CLI tests
   on both targets, and runs the CLI's
   version/doctor checks natively and through Rosetta. It never installs or
   changes host settings. If the Intel Rust target is absent, `bash
   ci/release/mac-intel-target.sh` can provision the checksum-pinned standard
   library entirely under the task root. Set the printed
   `RTAO_MAC_INTEL_SYSROOT` for the builder. Both archives, ten check logs and
   an unsigned local record are created under the task root. The nextest runner
   must already match the repository's exact tool pin (the task-local gate path
   `$RTAO_TEMP_ROOT/ci/tools/bin` takes precedence); the Mac builder does not
   install it.
3. Authenticate the exact `macos-build.json` with the approved Ed25519 local
   release key. `ci/release/allowed-signers` is deliberately absent until the
   user approves the key/trust design. Verification fails closed without that
   anchor and a valid `macos-build.json.sig`. No Keychain mutation is part of
   these scripts or `cargo xtask gates`.
4. `bash ci/release/complete-draft.sh rtao-vX.Y.Z` verifies all four archives,
   source tag/tree/toolchains, Mac signature and logs, and GitHub attestations
   bound to this source commit/tag and the release workflow. It generates the
   complete `SHA256SUMS` before uploading any asset. Only then does it create a
   complete draft; it refuses to overwrite an existing release.
5. Dispatch `release.yml` with the tag and `mode=dry-run`. This downloads and
   checks the complete draft without publishing. `mode=publish` repeats that
   check and, immediately before publication, downloads and checks the current
   draft again. `mode=prepare` re-runs hosted builds; select `--ref rtao-vX.Y.Z`
   as well as the tag input so GitHub's attested source identity is the tag.

The candidate local signature format is OpenSSH's Ed25519 detached signature
with namespace `rtao-release` and identity `rtao-mac`. The trust anchor contains
only approved lines of the form `rtao-mac ssh-ed25519 <public-key>`. A separate
approved adapter may use the user's Keychain-backed release key to sign that
record. This is still a prerequisite, not an implemented Keychain claim.
Never substitute a hosted Linux attestation for the local Mac build record.

Every CLI archive contains exactly the executable and these six notices.
All six are also standalone release assets, included in `SHA256SUMS` and the
docs build's authenticated artifact inventory and attestations. The rustdoc and
book archives retain the bundle at their roots. Builders and verifiers use the
authentic source files:

| Notice | Source | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `LICENSE-MIT` | `LICENSE-MIT` | 1064 | `ff96227eea95d57ded50c95da31b338bc70cb8880aa34f9062ce33cffa2c7ac7` |
| `LICENSE-APACHE` | `LICENSE-APACHE` | 11358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `NOTICE` | `NOTICE` | Source size, at most 1 MiB | Source/tag-bound |
| `LICENSE-PSF` | `crates/rtao-primitives/LICENSE-PSF` | 13936 | `3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf` |
| `webpki-root-certs-LICENSE` | `docs/licenses/webpki-root-certs-LICENSE` | 2371 | `e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8` |
| `Unicode-DFS-2016.txt` | `docs/licenses/Unicode-DFS-2016.txt` | 2315 | `68f5b9f5ea36881a0942ba02f558e9e1faf76cc09cb165ad801744c61b738844` |

Missing, changed, oversized, or symlinked notice copies fail verification.
The verifier compares NOTICE with its compiled source bytes and the clean
release tag. Its size/hash changes when source attribution changes, including
upstream imports. The docs record and GitHub attestation bind the standalone
copy to that tag; archive hashes bind the bundled copies.

The validator and subnet Dockerfiles ship the full bundle under
`/usr/share/licenses/rtao/` and `/usr/share/licenses/subnet/`, respectively.
Both stages check ordinary files, sizes and immutable hashes. The build stage
records the exact source NOTICE size/hash; the runtime stage checks its copy
against those records. The template guard checks both Dockerfiles and rejects
missing copies or validation steps.

Use one recorded task-scoped root for artifacts, logs, builds and scratch. Retain
bounded verification evidence before removing disposable build directories.
Audit live process, launchd, configuration, symlink and virtualenv dependencies
first. The signing key, wallets and shared caches are not disposable.

`release-plz.yml` exits early unless `RTAO_UPKEEP=on` or an explicit `dry-run`
dispatch is requested. Dry-run executes pinned `release-plz update`, regenerates
compatibility and runs every local gate, without obtaining an App token,
pushing, or creating a PR. After the handover, the App token enables release PRs
and the facade's `rtao-v{{ version }}` tags. All packages use git-only mode;
crates.io and release-plz's GitHub release creation are disabled.
The checker is installed before the rehearsal, so release-plz cannot silently
skip SemVer analysis. Upkeep also regenerates compatibility in the actual
release PR, runs the full gates and pushes the generated files with the App
token; the files embed the bumped workspace version.
