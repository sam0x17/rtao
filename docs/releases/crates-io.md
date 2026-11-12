# Publishing to crates.io

The maintainer asked on 2026-11-11 for rtao to be published to crates.io with
proper GitHub releases (D-181). This page is the checklist for that first
publication and for later ones. It adds a crates.io step to the versioned
release in [README.md](README.md) (D-121); it does not replace it.

The core graph reaches crates.io through rtao's republished `rtao-upstream-*`
packages (D-183); the transport crate is `rtao-jsonrpc` (D-182).

## 1. Packaging already in place (D-181)

- Every `rtao-*` entry in `[workspace.dependencies]` carries the version that
  `cargo publish` writes into dependants: `0.1.0`, and `0.2.0` for
  `rtao-arena`. Bump them with `[workspace.package] version`.
- Dev-dependencies on workspace crates are path-only (no `workspace = true`),
  so `cargo publish` strips them. `rtao-testkit` depends on `rtao-client`,
  which tests with `rtao-testkit`, and `rtao-tx` and `rtao-intents` test each
  other. A versioned dev-dependency would put those cycles into the published
  manifests. Keep new workspace dev-dependencies path-only.
- Each crate directory has copies of the root `LICENSE-MIT`,
  `LICENSE-APACHE` and `NOTICE`, so every `.crate` ships the licence texts and
  attribution. They are regular files, not symlinks, because the feature gate's
  workspace snapshot refuses tracked symlinks. The xtask test
  `crates_carry_root_licenses_and_path_versions` fails when a copy differs from
  the root file or a `[workspace.dependencies]` version differs from its crate;
  after editing a root file, copy it again
  (`for d in crates/*/; do cp LICENSE-MIT LICENSE-APACHE NOTICE "$d"; done`).
  `rtao-primitives` and `rtao-cli` also ship `LICENSE-PSF`, and
  `rtao-core-ext` ships `LICENSE-UNICODE`.
- Every package sets `documentation = "https://docs.rs/<crate>"`,
  `readme = "README.md"` and docs.rs metadata with `--cfg docsrs` and
  `targets = ["x86_64-unknown-linux-gnu"]`.
- Each crate README starts with crates.io, docs.rs and licence badges, and the
  root README with the facade's. They show "not found" until the first publish.
- No package uses `include` or `exclude`. The default file set is the tracked
  files in the crate directory: sources, tests, benches, examples, the README
  and the licence files, from 18 to 127 files per crate. Many tests read
  workspace files outside the crate, so tests do not run from a downloaded
  `.crate`. That is expected and does not affect dependants.
- `publish = false` is set only on `xtask`. Every crate under `crates/` is
  meant for users:
  - `rtao-core-ext` and `rtao-catalog` are normal dependencies of other crates;
  - `rtao-testkit` is a normal dependency of the subnet template, for
    `Localnet` and `Scenario`.

`cargo package --workspace --exclude xtask --locked` packages and verifies all
30 packages: the 24 crates and the six republished packages (D-183).

## 2. What the first publish needed (done)

1. **Git dependencies (D-183).** `bittensor-core` and the five packages it
   took from Git are republished as `rtao-upstream-core`, `-hashed`,
   `-macros`, `-fips204`, `-w3f-bls` and `-tle`, generated under `upstream/` by
   `cargo xtask forks` from the revisions in `upstream.toml` `[publish]`. They
   are workspace members, so CI builds and lints what is published. The
   polkadot-sdk crates come from their crates.io releases. No manifest carries
   a `[patch]`.
2. **`ruint` patch (D-183).** Gone. `rtao-evm` requires crates.io `ruint`
   1.20.1 or newer and the workspace minimum is Rust 1.90.
3. **Files read from outside the crate (D-183).** Build-time inputs
   (`upstream.toml` for `rtao-core`'s build script and `rtao-testkit`'s
   `Localnet::start`, `compat.json`, the EVM ABIs, the vault and extension
   pages, the extracted named tables `rtao-testkit`'s fake chain loads) are copied into the crates that read them; `cargo xtask
   crate-inputs --check` keeps the copies equal to their sources. Test-only
   inputs stay where they are: tests do not run from a downloaded `.crate`.
4. **Release policy text (D-184).** `docs/plan/architecture.md`,
   `ci/release/README.md` and `release-plz.toml` describe crates.io
   publication as the release step of this page.
5. **The macOS lane.** Unchanged: D-121 runs it on a local tag before the tag
   is pushed (`ssh` to the operator's Mac, as `ci/release/README.md` says). A
   release that ships without macOS archives says so in its notes.
6. **Curated notes (D-184).** `docs/releases/rtao-v0.1.0.md` covers the
   decisions to the release commit. The rendered notes name the published
   crates and the republished core, include only this version's CHANGELOG
   section and link the rest; `cargo xtask release-notes` refuses notes over
   GitHub's 125,000-character body limit.
7. **CI badge.** Still waiting: add a `pr-gates` badge only after `pr-gates`
   passes on the release commit.

Cut `## Unreleased` in `CHANGELOG.md` to `## 0.1.0 - <date>` in the release
commit.

## 3. Publish order

Normal and build dependencies decide the order. Dev-dependencies are stripped
and do not count. Crates on the same level are independent.

| Level | Crates |
|---|---|
| U0 | `rtao-upstream-macros`, `rtao-upstream-fips204`, `rtao-upstream-w3f-bls` (0.490.0) |
| U1 | `rtao-upstream-hashed`, `rtao-upstream-tle` |
| U2 | `rtao-upstream-core` |
| 0 | `rtao-arena` (0.2.0), `rtao-core-ext`, `rtao-jsonrpc` |
| 1 | `rtao-core` |
| 2 | `rtao-primitives` |
| 3 | `rtao-catalog`, `rtao-runtime`, `rtao-signer` |
| 4 | `rtao-client`, `rtao-wallet` |
| 5 | `rtao-evm`, `rtao-reads`, `rtao-testkit`, `rtao-tx` |
| 6 | `rtao-intents`, `rtao-metagraph` |
| 7 | `rtao-btauth` |
| 8 | `rtao-compat-epistula`, `rtao-compat-synapse`, `rtao-neuron`, `rtao-remote-signer`, `rtao-transport-quic` |
| 9 | `rtao` |
| 10 | `rtao-cli` |

`cargo xtask forks order` prints the U levels in order. The republished
packages go first: every rtao crate that reaches the core needs them.

crates.io rate-limits new crate names: a burst of 5, then one every 10
minutes. The 30 names (24 crates and six republished packages) take at least 4 hours
10 minutes. Either ask help@crates.io to raise the limit for the release
window, or publish level by level and resume where the limit stopped.
New versions of existing crates have a burst of 30, then one a minute.

## 4. Commands

Run everything from a clean checkout of the release commit, in a task root
under `.local/`, with the workspace `target/` (`AGENTS.md`).

### Preflight (before the tag)

```sh
git status --porcelain                          # must print nothing
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --all-features --locked
cargo xtask release-notes --tag rtao-vX.Y.Z > "$RTAO_TEMP_ROOT/notes.md"
cargo xtask forks check                         # upstream/ equals the recorded revisions
cargo xtask crate-inputs --check
# Package and verify every crate in dependency order against a local overlay
# of the not-yet-published workspace crates. Nothing is uploaded.
cargo package --workspace --exclude xtask --locked
cargo xtask forks verify-packaged                # archives equal the revisions; prints SHA256s
```

`cargo package --workspace` must pass for every crate before anything is
tagged or published. Publishing cannot be undone (section 6).

### Tag and GitHub release

Follow steps 1 to 4 of [README.md](README.md): curated notes and CHANGELOG
rename on `main`, local checks, local annotated tag, the macOS lane, then push
the tag. Publish the crates (next section) from the tagged commit before
creating the GitHub release, so the release can link the published versions:

```sh
gh release create rtao-vX.Y.Z --verify-tag --title "rtao vX.Y.Z" \
  --notes-file "$RTAO_TEMP_ROOT/notes.md" <macOS lane assets...>
```

### Publish

`cargo login` must already hold a token with the `publish-new` and
`publish-update` scopes. Publish from the tag:

```sh
git switch --detach rtao-vX.Y.Z
cargo publish --workspace --exclude xtask --locked
```

`cargo publish --workspace` uploads in the order of section 3 and waits for
each crate to appear in the index before publishing the crates that depend on
it. If the rate limit stops it, publish the remaining crates one at a time, in
the order of section 3:

```sh
for crate in <remaining crates in order>; do
  cargo publish -p "$crate" --locked || break
done
```

Each `cargo publish -p` verifies against crates.io, so a crate fails if a
crate it depends on is not published yet.

### After publishing

- Record each republished package's `.crate` SHA256 as `checksum` in its
  `upstream.toml` `[publish.<key>]` entry (the value `forks verify-packaged`
  printed for the tagged commit), commit it and run
  `cargo xtask forks verify-published`, which downloads every published
  package and compares it with the recorded revision.
- `rtao doctor` compares a consumer's lock with the checksums only when the
  resolved `rtao-core` records them. The checksums are known only after the
  republished packages are published, so the published `rtao-core` 0.1.0
  records none and its doctor checks the republished packages' versions only.
  The first release after the record carries the checksums.
- A second owner for every crate is the maintainer's decision; none is added
  by default.
- Install the CLI from crates.io in an empty `CARGO_HOME` and
  `CARGO_TARGET_DIR` under the task root, then check it:
  `cargo install --locked rtao-cli`, `rtao --version`.
- Build a scratch project that depends on `rtao = "X.Y"` with the read features
  and no `[patch]`.
- Check the first docs.rs build of each crate. The `host` and `ledger`
  features of `rtao-core` and `rtao-signer` need system OpenSSL, libsodium and
  hidapi. If docs.rs also passes `--cfg docsrs` to dependencies, one that
  still uses the removed `doc_auto_cfg` feature fails there (`base16ct` 0.2.0
  did in a local docs.rs-style build). If a build fails, narrow that crate's
  `[package.metadata.docs.rs] features` or publish a patch version.
- Set the repository homepage to `https://docs.rs/rtao` and its topics
  (`bittensor`, `rust`, `sdk`, `substrate`, `blockchain`, `cli`).
- Done for 0.1 (D-188): `templates/subnet/Cargo.toml` requires `rtao`,
  `rtao-testkit` and `rtao-runtime` at `version = "0.1"` from crates.io and
  carries no `[patch]`. The hosted template check builds the generated
  project against the checkout through `ci/patch-source.sh`, which adds a
  `[patch.crates-io]` section, and `cargo xtask template` refuses a Git or
  path source or a requirement outside the workspace's release series.

## 5. Later releases

- Bump `[workspace.package] version` and the matching `version` of every
  `rtao-*` entry in `[workspace.dependencies]` together. `rtao-arena` keeps its
  own number.
- Run the preflight, the D-121 steps and the publish as above. Unchanged crates
  still get the new version: all workspace crates share one version.
- A release that moves the minor version (0.x) moves the template's
  requirement with it (`version = "0.<minor>"` in
  `templates/subnet/Cargo.toml`); `cargo xtask template` and its unit test
  fail until it does.
- A core bump (`pin --core-bump`, `core-patch rebase`) refreshes `[publish]`,
  regenerates `upstream/` at `0.<new tag>.0` (every package, also one whose
  source did not move) and moves the exact requirements in the root
  manifest. A new adaptation revision of the same tag raises the patch number
  (`0.490.1`). Either leaves the new versions without a `checksum`.
- The hourly `runtime-adopt` workflow publishes them (D-188), under the
  maintainer's standing approval for new fork versions: a read-only job runs
  `forks check`, packages the six packages and `forks verify-packaged`; a job
  that holds `CRATES_IO_TOKEN` and builds nothing uploads the packages
  without a checksum in the order of section 3 (`cargo publish --locked
  --no-verify -p`, only when the archive it packages has the verified
  SHA256; a version crates.io already holds is skipped when its index
  checksum is that SHA256 and stops the run otherwise; a 429 waits and
  retries); a read-only job records the checksums in `upstream.toml` and its
  crate copies, runs `forks verify-published` and commits as the bot, and a
  job that builds nothing pushes the record. Any failure commits nothing,
  opens one issue and alerts. Success opens or refreshes an issue proposing
  the next rtao release (a minor bump before 1.0 when the core release
  moved, a patch bump for adaptation revisions alone). The workflow never
  publishes rtao crates or tags a release.

## 6. Mistakes

crates.io versions are immutable. A crate can be deleted only within 72 hours
of publication, or later only if it has a single owner, fewer than 500
downloads a month and no reverse dependencies. Do not plan on deletion. Yank a
broken version (`cargo yank --version X.Y.Z <crate>`), which stops new
lockfiles from selecting it, and publish a fixed patch version. A pushed tag
is never moved or deleted (D-121).
