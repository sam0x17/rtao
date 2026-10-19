# Upstream pin log

This log records changes to `upstream.toml` in this repository. The core pin
selects the reused Rust implementation; the parity pin selects the source
behavior and catalogs being reconciled. A pin records input identity. It does
not establish successful execution against that runtime.

## 2026-10-15: initial v478 checkpoint

Repository commit `3fec0bb` first records the pins in `upstream.toml`:

| Input | Recorded identity |
|---|---|
| Upstream repository | `https://github.com/RaoFoundation/subtensor.git` |
| Core tag and commit | `v478`, `850f2755bb05a3851f948489f0ab27b0645cebce` |
| Parity tag and commit | `v478`, `850f2755bb05a3851f948489f0ab27b0645cebce` |
| Parity runtime and Python package | Spec `478`, `11.3.1.dev0` |
| Core toolchain | Rust `1.89` |

The checkpoint retains artifact hashes, the watched source listing, network
identities, metadata inputs and localnet image identities. Localnet pins
describe optional integration inputs; they do not require a node for client
development. The imported upstream golden and shape corpus contain spec 461
recordings, so their replay evidence remains separate from the current runtime
pin.

This is the first recorded pin state, with no earlier repository pin to compare.
There is no pin-bump porting report or independent Python oracle output diff
for this entry. Source reconciliation and subsequent test results are tracked
separately in the [parity records](parity.md) and hosted CI.

## 2026-10-15: reviewed Ledger core patch

Repository commit `e39632d` adds `[core.patch]` while leaving the core and parity
pins at v478. It selects patch revision
`eafe84e8a97376e7902118ebb204b9c53e3df55e` from
`sam0x17/subtensor`, based on
`850f2755bb05a3851f948489f0ab27b0645cebce`.

The patch is associated with
[RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242).
Its recorded scope is three core signer files: `signers/mod.rs`,
`signers/ledger.rs` and `signers/hid.rs`. The pinned probes name
`bittensor_core::signers::ApduTransport` and
`bittensor_core::signers::ledger::LedgerDevice::with_transport`. The
review base, revision and diff SHA-256 in `upstream.toml` bind that scope to the
reviewed patch.

This patch adoption is not a parity pin bump. The pin change contains no
independent Python oracle output diff, and it supplies no physical Ledger
execution evidence. See the [Ledger guide](ledger.md) for the distinction
between replay, injected transport and device coverage.

## 2026-10-18: parity moves to unreleased spec 481

The parity pin moves from `v478` (`850f2755bb05a3851f948489f0ab27b0645cebce`)
to spec 481 at commit `6a972693ae9580541225bd37696ac2fee26b8ae9`, the head of
upstream's `testnet` branch, which tracks the testnet runtime (`main` and
`devnet` are at the same commit). Testnet finalizes spec 481 and Finney spec
479; upstream has published no release after v478, so the pin is recorded with
`release = "unreleased"`, `source_branch = "testnet"` and `network = "testnet"`.
`pin --check` verifies that the commit is still on `testnet`, declares spec 481
in its runtime source and that no release has since tagged `v481` elsewhere;
re-pin with `--parity v481` once upstream publishes it, or with
`--parity branch:testnet` to follow a newer testnet runtime. Code-generation metadata is the testnet 481 runtime
(`vendor/metadata/testnet/481.*`), and the pinned localnet image is
`sha-6a972693…`, built from that commit.

| Input | Recorded identity |
|---|---|
| Parity tag and commit | `v481` (unreleased), `6a972693ae9580541225bd37696ac2fee26b8ae9` |
| Parity runtime and Python package | Spec `481`, `11.3.1.dev0` |
| Core tag, commit and patch | unchanged: `v478`, `850f2755…`, patch `626f2a4f…` |

`bittensor-core` is unchanged between the two commits, so the core pin and
its reviewed patch stay at v478. The Python SDK adds the per-subnet emission
weight vector (`set_subnet_emission_weights`, the `subnet_emission_weights`
read and `subnets emission` commands), the `set_reject_staking_hotkey_growth`
account flag and the derived `owner_emission`, `validator_emission` and
`miner_emission` hyperparameters, and makes `owner_cut_enabled` read-only.
rtao ports them in `rtao-primitives::emission_weights`, `rtao-reads`,
`rtao-intents` and `rtao-cli`; the catalogs grow to 85 intents, 103 reads and
404 chain errors.

Validation for this pin: hosted pr-gates pass; the full pin-pipeline e2e
stage passes locally against the pinned localnet on fast and slow blocks
(21/21 each, including the new emission-weights scenario 18) and its
core-oracle tests; scenario 8 upgrades a live v478 chain to the 481 runtime
mid-session; and the subnet template passes its compose-localnet check. Live
CLI use on fast localnets found and fixed three client problems recorded in
[deliberate differences](divergences.md) D-077 (shield key freshness,
finality headroom and re-sealing a proven-dropped inner) and a testkit bug
where concurrent localnets peered across the shared Docker bridge. No
independent Python oracle output diff was produced for this change, and no
validated compatibility range is recorded yet.

## 2026-10-18: parity returns to spec 479, the runtime Finney runs

Policy: rtao's parity pin tracks the runtime mainnet (Finney) currently
finalizes, not upstream's newest testnet runtime. Finney finalizes spec 479
while testnet runs 481, so the parity pin moves back from spec 481
(`6a972693ae9580541225bd37696ac2fee26b8ae9`) to spec 479 at
`f4786f9087cac84cfab37cbdf1e01c399042653a`, upstream `main`'s merge of
`release-479`. Upstream has published no v479 release; the pin source is
`branch:main@f4786f9`, recorded with `release = "unreleased"`,
`source_branch = "main"` and `network = "finney"`. Code-generation metadata
is Finney's 479 runtime (`vendor/metadata/finney/479.*`) and the pinned
localnet image is `sha-f4786f9…`
(`sha256:930484f0fadefc1912929a32786dded9420b85ca47949e748dcd002821217717`).

| Input | Recorded identity |
|---|---|
| Parity tag and commit | `v479` (unreleased), `f4786f9087cac84cfab37cbdf1e01c399042653a` |
| Parity runtime and Python package | Spec `479`, `11.3.1.dev0` |
| Core tag, commit and patch | unchanged: `v478`, `850f2755…`, patch `626f2a4f…` |

Spec 479 adds the `set_reject_staking_hotkey_growth` account flag (kept as
the `SetRejectStakingHotkeyGrowth` intent), the
`AccountRejectsStakingHotkeyGrowth` and `EVM.AuthorizationListTooLarge`
errors, the `HotkeySwapBasketCredits` and `EVM.AccountNonces` storage, its
events and the stakingV2 setter, and makes `set_auto_stake` add the
destination hotkey to the coldkey's staking list. The catalogs are 84
intents, 102 reads and 392 chain errors. The spec 480/481 emission weight
vector is not on Finney, so its port (the `rtao-primitives` rules, the
`subnet_emission_weights` read, `SetSubnetEmissionWeights`, `subnets
emission`, the derived emission hyperparameters, read-only
`owner_cut_enabled`, plan previews and e2e scenario 18) is parked in commit
`b6efa92` ("Park the spec 480/481 emission weights port until mainnet runs
it"). When Finney upgrades to a runtime that carries it, re-pin and
`git revert` that commit to restore the port. The shield fixes (D-077),
per-localnet Docker networks, e2e harness fixes, compatibility validation
lanes and pin tooling from the 481 work are runtime-independent and stay.

The attested spec 481 compatibility validation (run 39141096815) no longer
matches the inputs, so its ledger and reports are removed and no validated
range is recorded. `mainnet-watch` now compares Finney's finalized runtime
with this pin every hour and opens an issue when they differ.

Local validation for this pin: extract, regen, probe-pin, compat and strict
parity checks pass; formatting, all-feature Clippy and the workspace tests
(3399 passed) pass; and the compat lane script passes against the pinned
sha-f4786f9 localnet on fast blocks (19/19 e2e scenarios, scenario 8's live
runtime upgrade and both core-oracle tests). Hosted pr-gates and attested
validation lanes have not yet run for this pin, and no independent Python
oracle output diff was produced.

## 2026-10-19: Finney upgrades to spec 481; parity pins release v481

Finney enacted spec 481 (`:code` `0x5fb504c1…`), and upstream published
release `v481` at `6a972693ae9580541225bd37696ac2fee26b8ae9`, the same commit
the parked 481 port was built from. Following the mainnet policy, the pin
moves from spec 479 to the published release (`--parity v481`, recorded with
`network = "finney"`). Code-generation metadata is Finney's 481 runtime
(`vendor/metadata/finney/481.*`, byte-identical to the earlier testnet 481
capture) and the pinned localnet image is `v481`
(`sha256:c2eb81b1a39d89ad7bcca5e36a3e7f39f589a6fa422be66d5d03b59a8584957e`).
Finney's 477, 478 and 479 metadata rows stay in the immutable index.

| Input | Recorded identity |
|---|---|
| Parity tag and commit | `v481` (published), `6a972693ae9580541225bd37696ac2fee26b8ae9` |
| Parity runtime and Python package | Spec `481`, `11.3.1.dev0` |
| Core tag, commit and patch | unchanged: `v478`, `850f2755…`, patch `626f2a4f…` |

The move back to 479 is reverted as a series rather than re-ported: commit
`b6efa92` is reverted to restore the emission-weights port, and the 479
regeneration, validation and book commits are reverted to restore the 481
generated surfaces, parity rows and e2e scenario 18. Upstream sources and
generated code are identical to the earlier 481 pin; only the pin's release
status, network and metadata provenance differ. The catalogs are 85 intents,
103 reads and 404 chain errors again. The testnet-era 481 validation covered
different pin inputs, so its ledger is removed until hosted lanes attest the
new inputs.

Local validation for this pin: extract, regen, compat and strict parity
checks pass; all-feature Clippy and the workspace tests pass (apart from the
localnet-only e2e and core-oracle targets, run below, and the hosted-only
xtask fixture); and the compat lane script passes against the pinned v481
localnet on fast blocks (20/20 e2e scenarios including scenario 18's emission
weights, scenario 8's live runtime upgrade, and both core-oracle tests).
Hosted validation run 39328208457 attests drift, e2e and interop for the
pinned, testnet, devnet and main lanes, and its reports are adopted as the
validated spec range 481-481.

## Recording the next change

Append an entry when a pin changes. Include the old and new tag/commit, affected
contracts and native consumers, the porting-report findings, intentional
differences and the exact hosted validation results. Keep an input change
separate from later adoption or execution results.

Include the oracle output comparison when authenticated outputs exist for both
states. If either side is absent, stale or unexecuted, record that limitation
and leave the comparison unverified. An unchanged pin or absent output does
not establish an empty behavioral diff. The [upgrade guide](upgrades.md)
describes how callers handle runtime compatibility between pin refreshes.
