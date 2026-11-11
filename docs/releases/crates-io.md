# Publishing to crates.io

The maintainer asked on 2026-11-11 for rtao to be published to crates.io with
proper GitHub releases (D-181). This page is the checklist for that first
publication and for later ones. It adds a crates.io step to the versioned
release in [README.md](README.md) (D-121); it does not replace it.

Placeholders marked **[dependency strategy]** wait for the decision on how the
Git dependencies reach crates.io (section 2). Nothing else on this page depends
on that decision.

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

`cargo package --list` succeeds for all 24 crates. `cargo package` (with
verification) succeeds today only for `rtao-arena` and `rtao-rpc`, the two
crates outside the `bittensor-core` graph.

## 2. Blockers before the first publish

Clear these in order. Each needs a commit on `main` with its own D-number
where it decides something.

1. **Git dependencies [dependency strategy].** crates.io refuses Git
   dependencies. `rtao-core`, `rtao-core-ext` and `rtao-testkit` declare
   `bittensor-core` directly, and every other crate except `rtao-arena` and
   `rtao-rpc` reaches it through them. Through it the graph pulls in
   `subtensor-hashed`, the vendored `fips204` and `w3f-bls` forks,
   `subtensor-macros`, `tle` (ideal-lab5/timelock at a revision) and the
   polkadot-sdk crates of the RaoFoundation fork. rtao's fork patch of
   `bittensor-core` (`docs/execution/core-patch.md`) has to survive whatever
   replaces it.
   - Decision: _[dependency strategy]_.
   - Follow-ups, whichever strategy wins: `upstream.toml` and the core-patch
     checks, `deny.toml` source rules,
     `rtao_core::dependency_policy::validate_bls_lock`, `rtao doctor`, the
     "Downstream BLS patch" section of the root README, the last paragraphs of
     `crates/rtao/README.md` and `crates/rtao-core/README.md`, the patch
     paragraph of `book/src/getting-started.md`, the downstream patch line in
     `cargo xtask release-notes`, and the subnet template's patch tables
     (`cargo xtask template` enforces them).
2. **`ruint` patch.** `[patch.crates-io] ruint = { path = "patches/ruint" }`
   does nothing for crates.io users. It is crates.io `ruint` 1.20.1 (the
   RUSTSEC-2026-0220 fix) with `rust-version` lowered to 1.89. Published
   `rtao-evm` users resolve `ruint` from crates.io instead. Before publishing,
   give `rtao-evm` a direct `ruint` floor at the fixed release, and declare
   the minimum Rust that floor needs (1.90) on `rtao-evm`, or raise the
   workspace minimum.
3. **Files read from outside the crate.** A `.crate` contains only its own
   directory, so any `include_str!`, `include_bytes!` or build-script read
   above it breaks verification and every dependant's build. Known cases in
   non-test code:
   - `rtao-core/build.rs` reads `upstream.toml`;
   - `rtao-client/src/compat.rs` includes `compat.json`;
   - `rtao-evm/src/precompiles.rs` includes `vendor/upstream/evm/abi/*`;
   - `rtao-signer` includes the vault and extension pages from
     `vendor/upstream/{vault,extension}/static/` (features `vault`,
     `extension`);
   - `rtao-testkit` includes `upstream.toml` and
     `vendor/upstream/golden.json`, and reads `upstream.toml` at run time;
   - several crates include `vendor/metadata/finney/*.scale` or
     `parity/pending.toml`: `rtao-client` (`compose`, `compat`, `view`,
     `read_decode`, `events`, `cache`, `legacy_api`), `rtao-runtime`
     (`handle`, `shape`, `models/commitment_data`), `rtao-tx` (`executor`,
     `plan`), `rtao-intents` (`serving`, `limit_orders`, `registry`) and
     `rtao-cli` (`call`, `commands/multisig`). Some of these are inside
     `#[cfg(test)]` modules. The `cargo package` verification of each crate is
     the check.

   The fix is to move or copy each input into the crate that reads it, with a
   check that the copy equals its source, or to generate it there.
4. **Release policy text.** D-181 records the new directive, and PLAN §3 and
   §16 point to it. The fixed decision at the top of
   `docs/plan/architecture.md` ("crates.io publishability is not a goal"),
   its §12.8 release process, `ci/release/README.md` ("crates.io ... disabled")
   and `release-plz.toml` (`publish = false`) still describe the Git-only
   policy. Reconcile them when the dependency strategy lands.
5. **The macOS lane.** D-121 runs the operator macOS lane on a local tag
   before the tag is pushed. That stays the rule unless the maintainer amends
   D-121, either by approving a hosted macOS job or by making the first
   release a source release without macOS archives.
6. **Curated notes.** `docs/releases/rtao-v0.1.0.md` stops at D-124. Bring it
   up to the release commit, and recheck its journal compatibility section. The
   header that `cargo xtask release-notes` renders promises Linux archives,
   attestations and a downstream Git patch that a source release does not
   have. Fix it to match the release that is actually made. Today the rendered
   notes are 995 lines, 105,418 bytes. GitHub limits a release body to 125,000
   characters, and the whole CHANGELOG is appended, so link the changelog or
   include only this version's section.
7. **CI badge.** Every commit on `main` since 2026-10-23 is `[skip ci]`, so a
   `pr-gates` badge would show an old result. Add it only after `pr-gates`
   passes on the release commit.

## 3. Publish order

Normal and build dependencies decide the order. Dev-dependencies are stripped
and do not count. Crates on the same level are independent.

| Level | Crates |
|---|---|
| 0 | `rtao-arena` (0.2.0), `rtao-core-ext`, `rtao-rpc` |
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

Any crates added under the dependency strategy come first, before level 0:
_[dependency strategy]_.

crates.io rate-limits new crate names: a burst of 5, then one every 10
minutes. The 24 names take at least 3 hours 10 minutes, and each name added
under the dependency strategy adds 10 minutes. Either ask help@crates.io to raise the limit for the release
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
# Package and verify every crate in dependency order against a local overlay
# of the not-yet-published workspace crates. Nothing is uploaded.
cargo package --workspace --exclude xtask --locked
cargo publish --workspace --exclude xtask --locked --dry-run
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

- Add a second owner to every crate:
  `cargo owner --add <github-user-or-team> <crate>`.
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
- Switch `templates/subnet/Cargo.toml` from Git `branch = "main"` to
  `version = "X.Y"` and update `cargo xtask template` to match. The template's
  patch tables follow the dependency strategy:
  _[dependency strategy]_.

## 5. Later releases

- Bump `[workspace.package] version` and the matching `version` of every
  `rtao-*` entry in `[workspace.dependencies]` together. `rtao-arena` keeps its
  own number.
- Run the preflight, the D-121 steps and the publish as above. Unchanged crates
  still get the new version: all workspace crates share one version.
- A core bump that changes what the published crates depend on:
  _[dependency strategy]_.

## 6. Mistakes

crates.io versions are immutable. A crate can be deleted only within 72 hours
of publication, or later only if it has a single owner, fewer than 500
downloads a month and no reverse dependencies. Do not plan on deletion. Yank a
broken version (`cargo yank --version X.Y.Z <crate>`), which stops new
lockfiles from selecting it, and publish a fixed patch version. A pushed tag
is never moved or deleted (D-121).
