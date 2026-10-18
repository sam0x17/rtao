# Testnet preparation record

On 2026-10-14 the maintainer waived the 24-hour testnet soak (PLAN.md Q29) and
required all automated tests to pass. Dedicated subnet setup, funding quotes and
soak-host deployment are stopped. No transaction, remote wallet copy or host
service has run. Automated validator, localnet, image/TLS, oracle and
interoperability checks remain required.

A new subnet and a shared remote soak host had been selected, with low compute
use. Read-only SSH inspection of the host passed with existing host-key
validation. No remote files, directories, services or settings were created or
changed.

## Retained wallet

The native helper source, its seven fixture tests (including 24 altered
complete-state cases), artifact hashes and binary dependencies were audited.
The helper's fixed-destination `prepare` and public-only `inspect` modes then ran.
Creation completed and both public outputs matched.

The durable root is a maintainer-local wallet directory outside the repository.
It contains
the encrypted coldkey, distinct `validator` and `miner` hotkeys, public
companions, private recovery password and lifecycle journal. Owned directories
use 0700 and secret files use 0600. Keys refuse replacement; incomplete state
is retained and refused. Public inspection uses identity-checked `O_NOFOLLOW`
opens of exact public companions and has no hotkey-secret fallback. No phrase,
seed, private key or password was printed.

Preserve this wallet and recovery state. It is excluded from disposable
workspace cleanup. The retained coldkey is the maintainer-provided testnet
coldkey; this record is not a funding request. Its public identities and
lifecycle record are not retained in the repository.

The exact public output SHA-256 was
`439a3b12b21926004a744771f8d4e3d4e497123f650d99f4400cb8b61353a7ca`.
The audited helper executable SHA-256 was
`b01ccf24a1e104d92e32a6027552c9fcf992940523bdd52e7588fab825d1e6c1`.
Completed helper builds and fixtures were dependency-audited and removed.
Helper source and bounded evidence are not retained in the repository.

## Historical read-only RPC observation

The probe on 2026-10-14 at approximately 18:41 UTC reported:

| Field | Observation |
| --- | --- |
| Endpoint | `https://test.finney.opentensor.ai:443` |
| rtao network selector | `test` |
| Genesis | `0x8f9cf856bf558a14440e75569c9e58594757048d7b3a84b5d25f6bd978263105` |
| Runtime spec / transaction version | `478` / `1` |
| Finalized block | `8,221,751` |
| Finalized hash | `0x2e785522ca10e2eb5a155a40e2139aff10cea15538708b5957522d8348856cf0` |
| Registration-cost runtime API output | `0x00ca9a3b00000000` |
| Decoded subnet lock | `1,000,000,000 rao`, or 1 testnet TAO |

This is a historical snapshot, not a current balance or complete funding
quote. A later read-only probe observed testnet spec 479. Finney still runs
the pinned spec 478; release and metadata drift are checked independently of
the waived soak. No compatibility range is inferred from a successful probe.
