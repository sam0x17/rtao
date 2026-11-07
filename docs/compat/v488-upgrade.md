# Finalized v488 runtime upgrade

The parity pin advances from v486 (`72323e13249c9e25c7e792a2b3c08502c84651d5`)
to v488 (`b6a81d144078eac18bd25c3af7cb31579bb670f3`). Finney finalized spec 488
at block 9,442,816 on 2026-11-05; testnet runs the same code. Upstream v488
contains release 487 ("Gamma and quantum-resistant wallets"), which was never
enacted on Finney by itself.

The core stays at v478 (`850f2755bb05a3851f948489f0ab27b0645cebce`) with patch
`5f2477f5290f5f4cb467524754cc025626b52974`. The core inputs did change at v488
(post-quantum and hashed keys in `bittensor-core`, the new
`primitives/hashed`, vendored `fips204`), so `[parity] core_fingerprint` records
the parity's core-input fingerprint and the core bump is due (D-127). No
journal or recovery format changes.

## Public provenance

Candidate preparation pinned finalized Finney head
`0x0cb6e583fc4bf8c0c7ed52543a35bcd65f66db0477f0b51a52cfd7aa6cb05fbc` with spec
488 and `:code` hash
`0x3147daab804046184359d8a929cb333d75f490323b9e1327887a172c64eafa94`. The
runtime-adopt evidence stage, run locally, found four public endpoints
(`entrypoint-finney`, `archive`, `lite` and OnFinality) agreeing at that block
on genesis, spec, the `:code` hash, the BLAKE2b-256 of the `:code` bytes and the
V15/V16 metadata digests (V15 `494d08e9…`, V16 `69d82bd7…`). The
[v488 release](https://github.com/RaoFoundation/subtensor/releases/tag/v488)
`subtensor.wasm` hashes to the same `:code`. The pinned localnet image is
`ghcr.io/raofoundation/subtensor-localnet:v488` at
`sha256:d8740be9c270ac9d7eea5cf40ae2aa4c4564ef706178b4e057a3a7c2a3f22fc1`
(identity only; not executed).

## Metadata drift 486 -> 488

`xtask drift diff` classifies the drift as breaking: 124 additive, 9
behavioural and 9 breaking changes. The extrinsic versions, the signed
extensions, the spec name and the transaction version (1) are unchanged, so
signing parameters, the metadata hash extension and the signing envelope are
the same.

The nine breaking changes are argument shapes that grew by appended enum
variants, which leave every existing encoding unchanged:

- `Proxy.proxy`, `proxy_announced`, `add_proxy`, `remove_proxy`, `create_pure`,
  `kill_pure`: `ProxyType` gains `Lock` (21) and `GammaSpend` (22), and nested
  calls see a larger `RuntimeCall`.
- `LimitOrders.execute_orders`, `execute_batched_orders`, `cancel_order`:
  `VersionedOrder` gains `Authorized { nonce, terms }` after `V1` and `V2`.

Behavioural: the `Proxy` events `ProxyAdded`, `ProxyRemoved`, `PureCreated`,
`PureKilled`, the `ProxyDefinition` and `ProxyType` models and `Proxy.Proxies`
follow `ProxyType`; `SubtensorModule.LastRateLimitedBlock`'s key type grew;
`System.Version` changed.

Additive, among others: the `HashedAccounts` pallet (6 calls, 5 storage items,
3 events, 14 errors); gamma deposits (`mint_gamma`, `mint_gamma_with_stake`,
`move_gamma`, `spend_gamma`, `close_gamma_deposit`, `set_gamma_deposit_spender`,
`set_gamma_deposit_daily_spend_cap`, their events, `GammaDeposits`,
`GammaEnabled`, `SubnetGammaEnabled`, `SubnetGammaPrice` and
`GammaRuntimeApi`); the TAO/USD price feed (`feed_tao_usd_price`, `TaoUsdPrice`,
`TaoUsdPriceOracle`); service URLs (`set_service`, `Services`);
`LimitOrders.authorize_order` and `ApprovedOrders`; `Utility.as_safe_derivative`
and `as_protected_derivative`; the `Multisig` `*_safe` calls;
`Proxy.create_safe_pure`, `create_protected_pure`, `kill_safe_pure` and
`AnnouncementValidFrom`; deferred basket cash storage; new `AdminUtils` setters.

The commit-reveal path is unchanged: `coinbase/reveal_commits.rs`,
`subnets/weights.rs` and `CommitRevealWeightsVersion` have no upstream diff.

## What the pin regenerates and what is due

- `xtask regen` regenerates descriptors for 19 scoped pallets (adding
  `HashedAccounts`, which upstream's Python now covers) and 26 runtime APIs;
  `codegen/frozen_models.toml` gains `GammaDepositInfo` and no existing layout
  hash changed.
- The catalog grows to 98 intents and 111 reads. The 12 new intents and 6 new
  reads are generated as pending entry points (`NotYetPorted`); their calls
  compose by name today.
- `stage-487`'s `ProxyType` variants are merged (D-125).
- Of 226 changed watched sources, 31 are generated, extracted or vendored and
  re-derived; 111 hand-ported sources are listed in `parity/port_due.toml`; 84
  new upstream files, 17 new public exports and 59 new e2e keys have pending
  rows. Ordinary parity passes; strict parity names all of these.
- The D-040 read replacement of `subnet_start_schedule` is re-anchored to
  the v488 `reads/subnets.py` and `subnets/subnet.rs`: Python still reads the
  `InitialStartCallDelay` constant and the runtime still uses
  `StartCallDelay` storage.

## Automation

The hourly runtime-adopt workflow failed six times per network before this
record. The causes and fixes are D-126 to D-128 in `PLAN.md`.
