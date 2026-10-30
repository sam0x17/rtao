# Finalized v486 runtime and Python parity upgrade

The parity pin advances from v484 (`5749dbfbe4a78ccd3938d46ffae45e059488feb7`)
to v486 (`72323e13249c9e25c7e792a2b3c08502c84651d5`). The core remains v478
(`850f2755bb05a3851f948489f0ab27b0645cebce`) with patch
`5f2477f5290f5f4cb467524754cc025626b52974`: no core input changed between the
tags, so the core-input fingerprint is unchanged. No recovery or journal facility
is removed or replaced.

## Public provenance

Candidate preparation pinned finalized Finney block **9,393,482**,
`0xf8bea834abdd103986b19ee128522de4c3a7fb6358eb03f32b3360552a48642b`, which
reports spec 486 and code hash
`0x49781ac4cfda0ac6cf6e53cfceea7c728394d9c28da7881f47e3fbfbc239800f`.
Testnet reports the same spec and the same code hash.

The [v486 release](https://github.com/RaoFoundation/subtensor/releases/tag/v486)
is final (published 2026-10-26) and its tag resolves to the parity commit. It is
a **deployed-runtime publication** (upstream `publish-deployed-runtime.yml`,
new in v486): the runtime was enacted on mainnet at block 9,369,470 outside the
release train and published after the fact. Its assets are `subtensor.wasm`,
`subtensor-digest.json` and `deployment.json`; there is **no
`upgrade-manifest.json`**. The deployment record names spec 486, the tag commit
and the finalized mainnet code hash. The downloaded wasm has exactly that
BLAKE2b-256, and its SHA256 is
`0x46a07a802272d3b66f6e67213be8cc4c9b3f8788e91153686f192077d3df4457`. The srtool
digest (srtool 0.18.3, rustc 1.89.0) agrees on commit, SHA256, BLAKE2b and spec.

The pinned localnet image is `ghcr.io/raofoundation/subtensor-localnet:v486`
at `sha256:597778ceb7de7e39e60545a7e5f18829ca1530d8e5be7e097c17cf404e742ff0`.
This proves the registry artifact identity; the image was not executed.

## Maintenance decision: proving a deployed-runtime release

`xtask pin` required an upgrade manifest and refused v486. `validate_release`
now takes the release's provenance record: the train manifest, or the
deployment record. A deployment record must name the immutable tag commit and
the spec, and its code hash must equal the release wasm's BLAKE2b-256. The srtool
digest checks and the finalized Finney `:code` check are unchanged and apply to
both. A release that carries both records or neither is refused. Tests:
`release_provenance_fails_closed_on_each_identity` (train) and
`deployed_release_provenance_fails_closed_on_each_identity`.

## Implemented behavior and source reconciliation

- Metadata drift 484 → 486 is `surface-unchanged`: only `System.Version` moves
  (spec 484 → 486; transaction version 1). No pallet, call, call index, storage,
  event, error or runtime API that rtao uses changed. The generated surface
  hash is unchanged.
- Import all 390 authenticated artifacts. Extraction and regeneration produce
  86 intents and 105 reads. The crate changes are provenance headers only.
  Historical 477–484 metadata is retained, and V15/V16 for 486 are added.
- Reconcile the 364 watched sources: 363 unchanged and one changed.
  `sdk/python/bittensor/cli/upgrade_helpers.py` adds
  `-e CONST_RANDOM_SEED=subtensor-runtime` to the srtool reproduce recipe. Without
  it, a reproduced wasm does not match the deployed code. It is ported to
  `rtao upgrade` (`reproduce_recipe`) in the same position, with the test
  `reproduce_recipe_matches_the_upstream_srtool_recipe`. Per-file decisions are
  in [parity/v486-sources.json](../../parity/v486-sources.json).
- Fixtures pinned to the current runtime move from 484 to 486.
- Weights and commit-reveal v4 are byte-identical between the tags, including
  `subnets/weights.rs`, `coinbase/reveal_commits.rs`, the drand pallet and the core
  timelock: `commit_timelocked_mechanism_weights` (index 118) with
  `CommitRevealWeightsVersion` 4.
- The owner emission split (`AdminUtils.sudo_set_subnet_emission_weights`,
  pallet 19, call 114) is unchanged and already shipped as
  `rtao subnets emission`.

## Validation and remaining gaps

Local validation is recorded in [parity/v486-validation.json](../../parity/v486-validation.json).
The xtask import, pin source, extract, regen, probe, compat and strict parity
checks pass. So do the focused runtime, client, reads, CLI upgrade,
subnets/emission, intents/primitives emission, core-ext timelock/epoch,
intents weights, neuron (all features, 223 tests) and CLI weights/timelock
suites, plus clippy `-D warnings` and fmt. The release CLI connects
read-only to testnet on 486 without a guard refusal.

`core_patch::tests::hosted_git_fixture_*` refuses locally by design (PLAN Q30),
as in 484. No hosted CI, independent Python oracle comparison, localnet e2e or
image execution was run. The oracle remains optional-unverified, with a null
validated range and empty runtime attestations. Nothing was signed or submitted.
