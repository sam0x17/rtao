# Feasibility check: subxt 0.51.1 against live finney (spec_version 477, metadata V16)

Run on 2026-10-13 from `scratchpad/feasibility/subxt-check` (source in that directory).

## What was done

1. `subxt metadata --url wss://entrypoint-finney.opentensor.ai:443 -f bytes` -> `meta/finney.scale` (404,727 bytes, V16). Testnet metadata is byte-identical in size and also spec 477.
2. `#[subxt::subxt(runtime_metadata_path = "../../meta/finney.scale")]` static codegen compiled cleanly (the CLI `subxt codegen` output is ~2.4 MB / ~60k rustfmt'd lines, saved as `meta/finney_codegen.rs`).
3. Program connected with `OnlineClient::<SubstrateConfig>` and, for each of three backends, did: typed storage read `SubtensorModule.Tempo(NetUid(1))`, typed runtime API `SubnetInfoRuntimeApi.get_subnet_hyperparams(NetUid(1))`, `create_signed` of `System.remark` with the dev Alice key (never submitted), then `validate()` (= `TaggedTransactionQueue_validate_transaction` via state_call).

## Results

| backend | storage fetch | runtime api | create_signed | validate() |
|---|---|---|---|---|
| `LegacyBackend` | Tempo(1)=99 | tempo=99, max_weights_limit=65535 | 125 bytes | `Invalid(Payment)` |
| `ChainHeadBackend` | Tempo(1)=99 | same | 125 bytes | `Invalid(Payment)` |
| default `CombinedBackend` (what `from_url` uses in 0.51) | **error: RPC -32601 Method not found** | ok | 125 bytes | `Invalid(Payment)` |

Interpretation:
- `Invalid(Payment)` is the expected result for an unfunded dev account: the node decoded the signed extrinsic, including all 13 transaction extensions, and got as far as fee payment. So subxt's default `SubstrateConfig` extrinsic params (`AnyOf<...>` with empty-extension skipping) correctly handle subtensor's four custom extensions (`SubtensorTransactionExtension`, `SudoTransactionExtension`, `DrandPriority`, `CheckShieldedTxValidity` — all empty composites with `()` implicit data in the metadata) and `CheckMetadataHash` (mode byte = Disabled). No custom `Config` impl was needed for signing.
- The 0.51 default `CombinedBackend` calls at least one RPC method the finney WS endpoint does not serve (the HTTP `rpc_methods` list does advertise `archive_v1_*`, `chainHead_v1_*`, `transaction_v1_*`, `transactionWatch_v1_*`, so this is probably a load-balancer/pool difference between WS and HTTP, or a specific method such as `archive_v1_storage` paging). rtao must pick the backend explicitly and test each supported public endpoint; `LegacyBackend` is the safe default today, `ChainHeadBackend` also works.
- Subtensor wraps netuid in the newtype `subtensor_runtime_common::NetUid(u16)` (and similar newtypes elsewhere), so generated APIs take `NetUid`, not `u16`; rtao's ergonomic layer should convert.
- The finney endpoint also serves legacy custom RPCs (`subnetInfo_*`, `neuronInfo_*`, `delegateInfo_*`, `stakeInfo_getColdkeyLock`) but the runtime-API path through `state_call` is the one subxt uses and it works.

## Other observations

- subxt 0.51 API is the redesigned "at block" API: `api.at_current_block().await?` -> `.storage().entry(addr)?.fetch(keys)`, `.runtime_apis().call(payload)`, `.transactions().create_signed(..)` / `.sign_and_submit_then_watch_default(..)`; `api.stream_blocks()`; backends are constructed via `subxt::backend::{LegacyBackend, ChainHeadBackend, CombinedBackend}` builders and `OnlineClient::from_backend`.
- `subxt-rpcs` provides `ReconnectingRpcClient` and `LegacyRpcMethods` for raw RPC alongside the typed client.

## Dependency weight (measured)

- `cargo tree -e normal` for the feasibility crate (subxt 0.51 + subxt-signer + tokio, static codegen of the full finney metadata): **228 unique crates**.
- Clean `cargo build --release` on this machine (Apple Silicon, Rust 1.99): **108 s wall / 171 s user** (dependencies + the ~60k-line generated module). Incremental rebuilds after editing main.rs took a few seconds.
