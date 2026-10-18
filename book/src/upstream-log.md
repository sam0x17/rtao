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
