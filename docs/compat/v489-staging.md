# Runtime 489 staging (D-163)

Branch `stage-489` carries the hand ports for upstream runtime 489 that can be
written before the pin, so the 489 adoption only has to merge it. Upstream
`main` declares spec 489 at `0b9d7403b12847a1807be0d328617517114aae6d`
("Release 489", RaoFoundation/subtensor#3308). Testnet has since finalized
489 and then 490, and Finney enacted 490 on 2026-11-10 while this branch was
being prepared (see "Runtime 490" below: 490 is 489 with a new spec number).
Nothing here is on `main` and nothing is pinned by this branch. The preview
below compares 489 with the parity pin, v488
(`b6a81d144078eac18bd25c3af7cb31579bb670f3`).

## Runtime surface 488 -> 489

- Calls (both in `AdminUtils`, inside the regenerated pallet-wide scope):
  `sudo_set_subnet_emission_weights_v2` (index 120; `netuid`, `split`: a
  bounded list of `(EmissionSlot, u64)` in strictly ascending slot order, at
  most 21, where `EmissionSlot` is `Burn | Recycle | Owner | Validator | Miner |
  Gamma { netuid }`), owner or root, the same rate limit and admin freeze
  window as v1; `sudo_set_subnet_gamma_emission_whitelisted` (index 121;
  `netuid`, `whitelisted`), root only.
- Storage (`SubtensorModule`): `SubnetGammaEmission` (netuid -> bounded
  `(NetUid, u64)` shares, at most 16), `PendingGammaEmission`,
  `UnconvertedGammaEmission` (double map), `SubnetGammaEmissionWhitelisted`,
  `SubnetLeaseTerminationHotkey`.
- Events: `SubtensorModule.SubnetGammaEmissionSet`,
  `SubtensorModule.GammaEmissionMinted`,
  `AdminUtils.SubnetGammaEmissionWhitelistedSet`, all appended.
- Errors (`SubtensorModule`, appended): `EmissionSlotsNotAscending`,
  `GammaEmissionAboveMaximum`, `GammaEmissionTargetNotWhitelisted`,
  `TooManyGammaShares`, `GammaEmissionNotSettableOnLeasedSubnet`.
- Runtime APIs: unchanged. Precompile ABI: unchanged (the staking precompile
  only meters its aggregate coldkey views).
- `ProxyType`: unchanged (23 variants). The v2 setter joins
  `SubnetManagementCalls` next to v1, the whitelist setter joins
  `RootConfigCalls`.
- Signing: `transaction_version` stays 1, the transaction extensions and
  `spec_name` are unchanged and the core's extrinsic codec is unchanged, so
  signing parameters and the signing envelope are the same.
- Behaviour: the v1 setter's sum now counts the subnet's gamma shares; a new
  gamma deposit no longer needs the hotkey to be associated (an `Owner` entry
  for another coldkey still blocks, a stored zero account included); lease
  termination settles residual positions; basket dividend minting takes an
  impact budget instead of a strict price gate; locks follow the owner status;
  the runtime stops reading the retired Alpha share maps
  (`LegacyAlphaCompatibility = false`).

## Core

The core inputs move (fingerprint
`b3:c272a5b95e518b396708eecd865a660470682638280e85f7823c496fb5f3be3b`):
`keys/hashed.rs` now carries the wiping hashed Ed25519 derivation rtao
proposed as RaoFoundation/subtensor#3317, byte for byte the v488 adaptation's
file; `keys/mod.rs` adds ECDSA account and length helpers and
`verify_strict`; the new `keyfile_env.rs` exposes pure password encoding. The
adoption holds the core at v488 with `[parity] core_fingerprint` (D-127).

Core bump rehearsal on v489 (local, nothing published): the combined
PR-U17/#3251 series and the recorded v488 adaptation cherry-pick cleanly; the
adaptation's `hashed.rs` commit (`cd1a2e6a`) becomes empty, which the rebase
keeps with `--keep-redundant-commits`; the exact-tree check (combined diff,
then the v488 adaptation, three-way) yields the carried tree
(`ec2deb061ee67e211c214ff46f62f404a1bef8c2`); `core-patch verify-security`
passes; the patched core's 138 library tests pass on toolchain 1.89. So no new
adaptation is needed for v489: the dispatched core bump stops at
`core-patch-publish-required` for `rtao-core-patch/v489`, as for v488, and
proceeds once the branch is published.

## Python changes and their ports

Staged on this branch (each tested against the v488 fixtures, where no gamma
shares exist, so it changes nothing before 489):

- `emission_weights.py`: `Rules::gamma_share`, `and_gamma`,
  `renormalize_with_gamma` (Python's `renormalize(current, fixed,
  gamma_share)`; `renormalize` keeps its signature), the sum rule and its
  table row count the gamma shares. `Rules` gains a public field
  (source-breaking for struct literals without `..Default::default()`).
- `reads/subnets.py`: `gamma_emission_share` reads
  `SubtensorModule.SubnetGammaEmission` by name and is zero where the live
  metadata has no such entry; `SubnetEmissionWeights::gamma_share` (new public
  field) feeds `rules()`.
- `intents/emission_weights.py`: build, preview and checks use the gamma
  share.
- `cli/commands/subnet_emission.py`: `show` reports `gamma_share`; `set`
  counts it in the sum; `set-value`, `increase` and `decrease` rescale to fill
  the rest.
- `intents/gamma.py`: the new-deposit owner rule
  (`_new_deposit_blocks`), comparing account IDs, through the new
  `ViewExt::storage_exists` (`_transport/interface.py`,
  `_substrate.py`: an exact key's presence without its default). This one is
  489 behaviour and lands only with the pin.

Due at adoption, not staged:

- `_transport/extrinsics.py`: `_normalize_signature` accepts 65-byte raw
  ECDSA signatures (`MultiSignature::Ecdsa`) for external signers. rtao's
  `MultiSignature` holds 64 bytes, so this is a type change, not a table edit.
- `sp_core.py`: `ecdsa_account_id`, `ecdsa_ss58_encode`, `verify_strict`,
  `encode_password_for_environment`; they come from the v489 core, so they
  follow the core bump.
- Help texts and docstrings that name the gamma shares where rtao checks help
  against the pinned Python.
- Generated surface (regen at the 489 pin): the two calls, five storage
  entries, three events and five errors, the error catalog and descriptions,
  `codegen/check.py RAW_ONLY`. `EmissionSlot` has no hand model, so
  `split` regenerates as `Vec<(DynValue, u64)>`; a typed model can follow.
- New upstream test files (`test_normalize_signature.py`,
  `test_sp_core_backend_compat.py`, `test_storage_exists.py`, the core's
  `password_environment.rs`, `verify_strict.rs`) and `keyfile_env.rs` become
  pending rows (D-128).

## Runtime 490

Upstream `main` then declared spec 490 (`d967d13588834f4f5aaa8ef4c8fc1a865d581462`,
"bump spec to 490"): after v489 only the spec version and the Python package
version (11.3.3.dev0) change, and testnet finalized 490. Testnet's 490
metadata against Finney's 488 is purely additive: the two calls, five storage
entries, three events and five errors above, with `System.Version`
surface-unchanged. Whichever of 489 or 490 Finney enacts, this branch applies
unchanged.

## Automated adoption (rehearsal)

The adoption stages were rehearsed locally on a detached `main` worktree: the
parity pin moved to v489, the published localnet image
`ghcr.io/raofoundation/subtensor-localnet:sha-0b9d7403…`
(`sha256:7c09de9b…`, revision `0b9d7403`) served spec 489,
`transaction_version` 1, and its V15/V16 metadata was indexed in place of
Finney's. Provenance, live evidence, the probe's hosted inputs and the commit
were not exercised (no network runs 489 on Finney).

- `import`, the core hold (`[parity] core_fingerprint`), `compat` after the
  build-weight remeasure, `probe pins`, every `--check`, the workspace build
  and the runtime, reads, intents and client tests pass. The one test failure
  is the Unix-socket test's path-length limit under a long scratch root; it
  passes under a short one and is unrelated to the runtime.
- `extract` stopped at v489's `sp_core.py` (`_core_helper` and seven new
  backend names), and `regen` at the D-040 replacement citation of
  `reads/subnets.py`. Both are fixed on this branch (D-163). They are tooling,
  which the adoption commit may not change (D-116 (5)), so they must reach
  `main` for the Finney 490 adoption.
- `drift`: additive, signing envelope unchanged; the run's stop condition
  (a breaking extrinsic or `System.Version` change) does not fire. The
  localnet's extra `faucet` call and two storage defaults are fast-runtime
  artefacts, absent from testnet's metadata.
- `parity` in adoption mode: 20 changed sources go to `parity/port_due.toml`
  (the emission weights, gamma, transport, `sp_core` and core sources above),
  new upstream files become pending rows, ordinary parity passes apart from
  the localnet `faucet` call. Strict parity names the due ports, as designed.

Merging this branch after the pin blesses the five staged ports
(`emission_weights.py`, `reads/subnets.py`, `intents/emission_weights.py`,
`cli/commands/subnet_emission.py`, `intents/gamma.py`) once their `// py:`
line references are checked against the pinned sources.
