# Core patch maintenance

The v478 core patch now contains two reviewed changes:

1. PR-U17, [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242), adds the injectable Ledger APDU transport.
2. The M2-S02 repair tracked by [RaoFoundation/subtensor#3251](https://github.com/RaoFoundation/subtensor/issues/3251) gives the stored Ed25519 signing key a wiping owner.

The maintained fork branch is `rtao-core-patch/v478` at
`5f2477f5290f5f4cb467524754cc025626b52974`. An attribution-only history rewrite replaced
`626f2a4f1f67ab731c1fc3884828751496bc125d`; both commits have the same source tree and
the reviewed diff digest below is unchanged. The four historical commit IDs
listed below identify the original review. The branch remains four commits
ahead of the pinned v478 base `850f2755bb05a3851f948489f0ab27b0645cebce`:

1. `eafe84e8a97376e7902118ebb204b9c53e3df55e`, the exact Ledger PR commit.
2. `774cc3ac78e442bfde7ef399b36f79e4bec4d460`, the Ed25519 ownership repair.
3. `5a43348e8dc95311e9750781a28a33380c454409`, the corrected wiping derivation path.
4. `626f2a4f1f67ab731c1fc3884828751496bc125d`, which wipes the hard-derivation
   preimage and the decrypted PKCS8 import copies.

The Ledger PR branch now names `09f40dc4cf0af6668398adf1565358e77e910772`,
the attribution-only replacement for the first historical commit above.
`upstream_pr_rev` records this active PR revision, which is an ancestor of the
rewritten combined patch. Its complete source tree and parent are identical
to the original Ledger PR revision; the original review identifiers above
remain historical evidence. The source and ancestry checks continue to apply.

The combined diff is restricted to the six paths recorded in `upstream.toml`
and has SHA-256
`8cf8ff28467ef20a8e47add1a600f60703941dbf3d9af6c2406b9f613e86c489`.
The previous three-commit review at `5a43348e` had digest
`404c285c98003fb845eed23ec9e48640a25a8e5b570ac1b7312c2df6aa922f6e`.
The authenticated review identity separates `upstream_pr_rev` from the combined
`review_rev`, so a later Ledger PR update cannot silently replace the security
repair.

## Ed25519 ownership repair

The core stores Ed25519 private state as `Zeroizing<ed25519_zebra::SigningKey>`.
That signing object (`ed25519-zebra` 4.2.0) contains the seed, expanded scalar
and nonce prefix and implements `Zeroize`, so its final owner wipes the complete
stored object before the allocation is released. This closes M2-S02. URI and
mnemonic derivation use a private `WipingEd25519Pair`; hard-derivation seeds
and derived copies also have wiping owners. Hard-junction derivation hashes the
parent seed through a fixed `Zeroizing` preimage buffer with the same bytes as
the former SCALE encoding, rather than an unwiped `using_encoded` vector.
Encrypted JSON import holds the decrypted PKCS8 secrets in `Zeroizing` buffers
and constructs the same stored type. Public key and signature bytes remain
unchanged: an RFC 8032 seed/public/signature vector and the Substrate `//Alice`
and `//Bob` Ed25519 vectors guard compatibility. Fork tests
`zeroize_clears_every_signing_key_secret` and
`hard_derivation_keeps_substrate_vectors` cover the owner and the derivation
bytes.

This repair establishes erasure for the core-owned stored Ed25519 signing
object, the hard-derivation preimage and the import copies named above. It does
not establish erasure of allocator or OS copies, stack copies left by moving the
`Copy` `SigningKey`, short-lived values inside `ed25519-zebra`, BLAKE2,
schnorrkel or KDFs, or caller-owned inputs. No hardware or platform execution
is claimed. Static Ed25519 signature verification still uses
`sp_core::ed25519::Pair` and holds no private key.

`rtao_core::keyfiles` re-exports the upstream plaintext codec and
environment-password functions without these repairs. Its module rustdoc says
so and points users to `rtao_core::keyfile_json` and the `rtao_wallet` password
sources.

## Provenance and maintenance

`core-patch rebase v478` verifies the base, linear review series, permitted
regular-file paths, exact resulting tree and combined diff digest. It validates
the Ledger API and the Ed25519 source structure. The Ed25519 guard parses
every core source file with `syn`. It rejects any `ed25519::Pair` type position
or import, requires the exact `Zeroizing<SigningKey>` owner and variant, the
named regression tests and the compatibility vectors, and forbids the unwiped
`using_encoded` derivation. It replaced an earlier substring check. A rebase
must preserve both reviewed components unless each component has an
independently authenticated replacement.

For a future release or review change that needs new commits, the command
prepares a candidate under the hosted task root and exits with
`core-patch-publish-required`. The bounded evidence contains a mail patch,
proposed identity and JSON hashes. An authorized publisher can apply it to the
recorded publication parent and publish an ordinary fast-forward. The proposed
commit hash is advisory because applying a mail patch can change committer
metadata. The tool never force-pushes or deletes refs.

The exact-tree check applies the recorded review diff in a separate Git index
when the release base changes. A conflict stops with `core-patch-conflict`.
Successful checks remove their disposable index and worktree; a hosted task
owns any retained conflict directory and candidate evidence.

`pin --check` checks the base, branch, combined paths and digest, fork-only core
lock entry, unchanged upstream BLS/macro sources and Clippy bans. Isolated
hosted probes compile the injected Ledger transport names against the patch and
must fail against the unpatched release specifically because those names are
absent. A dependency, inference, TLS or resolver failure cannot prove API
absence.

A successful unpatched Ledger API probe now shows only that the Ledger portion
has shipped. It does not authorize dropping the combined patch. Remove the
Ledger delta only after authenticating its upstream replacement, and retain the
Ed25519 delta until its own upstream replacement has been reviewed.

`rtao-testkit::ledger_emu` supplies bounded replay and loopback TCP transports
for the core driver. Replay failures are sticky, every request must match and
every script must be exhausted. TCP reads use one total I/O deadline, include
the status word outside Speculos's data-length prefix, reject excessive lengths
and retire partially consumed streams. The ordinary all-features hosted suite
executes these fake-transport fixtures. No node, Docker service, emulator or
hardware run is required.

Hosted root-lock refresh
[39004028813](https://github.com/sam0x17/rtao/actions/runs/39004028813) and
TLS-fixture lock refresh
[39004030113](https://github.com/sam0x17/rtao/actions/runs/39004030113)
resolved the earlier reviewed core revision `5a43348e`. The root lock preserved
all other package identities and the fuzz lock stayed byte-identical. The TLS
lock changed only the core revision and its `ed25519-zebra` dependency edge;
unrelated resolver updates were excluded. The move to `626f2a4f` changes only
the core revision in the root and TLS locks.

Local evidence for `626f2a4f`: the fork `bittensor-core` library
tests pass (72), the rtao core, core-ext, signer and wallet tests pass (250),
and `cargo xtask pin --check` passes against the fork revision. Hosted CI for
the final head is recorded in `docs/execution/state.md`.

No new emulator transcript was recorded, and no hardware, HID, firmware, app
acceptance or screen evidence is claimed.

## The v488 adaptation (D-139)

v488 adds `keys/hashed.rs`, which derived each hashed-account Ed25519 key as
`sp_core::ed25519::Pair`: the defect the repair above closes for ordinary
keys. The erasure guard refused the carried tree (runtime-adopt run
[42424054620](https://github.com/sam0x17/rtao/actions/runs/42424054620)).
The reviewed combined patch also applies to v488 only textually: v488's new
post-quantum and hashed `from_mnemonic` arms still returned a plain
`Vec<u8>` seed where the repair made it `Zeroizing<Vec<u8>>`, so the
carried core did not compile.

The fix is a reviewed adaptation, recorded as `[[core.patch.adaptation]]`
in `upstream.toml`, not a change to the combined review. It is the diff
between two fork commits on `rtao-core-patch/v488`:

- base `f06ee33f12973ae890ed473255092e151d95a86e`, the four reviewed commits
  cherry-picked onto v488 (its tree equals the combined diff applied three-way
  to v488, as the verifier computes it);
- rev `cd1a2e6ae41bf558eef8e21d00348c5a65e77d33`, two commits: the
  `from_mnemonic` seeds as `Zeroizing<Vec<u8>>`, and `HashedKeypair`'s
  Ed25519 derivation into `Zeroizing<ed25519_zebra::SigningKey>` with the
  master seed copied straight into its wiping buffer;
- digest `7dee4055f6ca8f29427912161fa51f279b9dfe91c412f80283dda3d0171a7f2b`.

The same `hashed.rs` change (identical blob) is proposed upstream in
[RaoFoundation/subtensor#3317](https://github.com/RaoFoundation/subtensor/pull/3317),
with the `ed25519-zebra` dependency that the combined patch already adds.
A regression test pins the hashed Ed25519 public keys, a transaction proof
and a message envelope recorded with the previous derivation; the fork's 136
core library tests pass on the adapted tree.

Rules. `keys/hashed.rs` joins the path allowlist. Every carried path is
modify-only, so the combined review (based at v478, where the file does not
exist) cannot touch it; only an adaptation can. An adaptation is 1..=20
linear fork commits whose net diff changes only allowlisted regular sources
and matches its digest. The applied adaptation is the one recorded for the
latest tag not after the core's; one for a later tag is staged (verified, not
applied), which is how this one is recorded while the core is still v478.
The exact-tree check applies the combined diff and then the adaptation. A
rebase cherry-picks both series and keeps the applied adaptation and any
staged ones. The erasure guard is unchanged and additionally requires, when
the core has `keys/hashed.rs`, that `derive_ed` and `ed_at` return
`Result<Zeroizing<SigningKey>, CoreError>` and that the hashed regression
test and its vectors remain. `cargo xtask core-patch verify-security
<commit> [<repository>]` runs the guard locally and read-only; it refuses
v488 with the combined patch alone and passes the adapted tip.

v488's core also has more in-repository path packages (`subtensor-hashed`
and its `fips204`). A core patched from the fork would take them from the
fork's checkout, so `[core.patch] shared` lists every path package of the
core at its commit (computed from the core's manifests and checked by
`pin --check`), and the fork's patch table keeps each at the core tag. The
lock still holds exactly one package from the fork.

## The v490 adaptation (D-169)

Upstream shipped the hashed Ed25519 wipe: v490's `keys/hashed.rs` is the
v488 adaptation's file byte for byte (blob `5b72f6c24b259df23cae033ed5acf79f483f0ef2`,
from [RaoFoundation/subtensor#3317](https://github.com/RaoFoundation/subtensor/pull/3317)
by way of Release 489), and its `Cargo.toml` already declares the
`ed25519-zebra` dependency the combined patch adds. Unpatched v490 still fails
`core-patch verify-security` (`keys/mod.rs` names `ed25519::Pair`), and the
combined patch still leaves v488's post-quantum and hashed `from_mnemonic`
arms returning a plain `Vec<u8>`.

`rtao-core-patch/v490` therefore carries five commits on v490
(`d967d13588834f4f5aaa8ef4c8fc1a865d581462`):

- the four reviewed commits cherry-picked, ending at
  `61d2902f5bc8d7e1d97a59a22594cd9651e6b832` (tree
  `8bf0190a3577f43789004c58a34f04f41beab4b1`, equal to the combined diff
  applied three-way to v490; the `Cargo.toml` hunk merges into v490's
  identical line);
- the v488 adaptation's seeds commit (`7ff3c1d0`) cherry-picked unchanged,
  `d7fd8762625a93b9837cde28d496b0819a976f0e`.

The adaptation recorded for `v490` is base `61d2902f`, rev `d7fd8762`,
digest `1d71ea78bc18f20bd27c6221edb95e93747297b6a89c7cfeb47f78f1cf25fb14`.
The v488 adaptation's `hashed.rs` commit is not carried; once the core is
v490 only the v490 adaptation stays recorded. The erasure guard is unchanged:
it now finds the hashed derivation's wiping owner and regression test in
upstream's own source. Local evidence: `verify-security` passes the tip and
refuses unpatched v490; the patched core's tests pass on toolchain 1.89 (138
library tests, 143 with `ledger`, and the 8 integration tests).

## How the patched core reaches users (D-183)

Before D-183 the root manifest and every secondary manifest carried `[patch]`
tables binding `bittensor-core` to the fork revision and its own path packages
to the core tag, and downstream workspaces had to copy them. crates.io accepts
no Git dependency, so the patched core is now republished instead: `cargo
xtask forks` generates `upstream/core` (`rtao-upstream-core`) from
`[core.patch].rev`, and the five packages it takes from Git (`subtensor-hashed`,
`subtensor-macros`, the vendored `fips204` and `w3f-bls`, and `tle`) from the
core tag and the timelock revision the monorepo pins. Every file except
`Cargo.toml`, `NOTICE` and `MODIFICATIONS` is the revision's file byte for byte;
`upstream.toml` `[publish]` records each package's source, version
(`0.<core tag>.<revision>`) and, once published, its `.crate` SHA256.

The checks above are unchanged and extended:

- `pin --check` (through `core_patch::verify_source`) regenerates every
  package from the upstream cache and requires `upstream/` to equal it, and runs
  the Ed25519 erasure check on the generated core's sources. The lock must
  hold each republished package once at its recorded version and no package
  from the fork, the polkadot-sdk Git repository or ideal-lab5/timelock.
- No manifest may patch the core graph: the root, the template, the fuzz crate
  and the two CI fixtures carry no `[patch]` for the monorepo, the fork or
  `w3f-bls`.
- The patched hosted probe builds `upstream/core`; the unpatched probe still
  builds the tag from Git. `core-patch strip` rewrites the root manifest to the
  original Git packages at the tag (`forks::as_git`), so "shipped crates
  against the unpatched core" keeps its meaning; the core canary does the same
  at `main`.
- `forks verify-published` downloads each published `.crate`, compares every
  file with `git show <rev>:<path>`, requires the published `Cargo.toml.orig` to
  equal the generated manifest, repeats the Ed25519 erasure check on the
  extracted core and compares the archive's SHA256 with the recorded one.

A core bump (`pin --core-bump`, `core-patch rebase`) refreshes `[publish]`,
regenerates `upstream/` and moves the root manifest's exact requirements; the
maintainer's standing approval covers publishing the new versions.
