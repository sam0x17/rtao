# Skeptic verification: rust-async-dx findings (F01-F29)

Paths: `A` = final/docs/plan/architecture.md, `P` = final/PLAN.md, `MM` = final/docs/plan/python-module-map.md,
`REG` = ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f. Reproductions were built in
scratchpad/verify-rad/ (offline cargo, rustc/clippy 1.99), never inside upstream dirs.

Tally: 18 confirmed, 11 partially_confirmed, 0 refuted.

| ID | Verdict | One-line reason |
|---|---|---|
| F01 | partially_confirmed | cycle is real; the fix misses the §1.4 metadata-model row (A:332) |
| F02 | partially_confirmed | cycle is real; the facade `blocking` feature must also enable `wallet` and export `set_weights` |
| F03 | partially_confirmed | all four claims hold; the fix misses `NonceManager` (A:1261-1263), and `SubmitFailure` is undefined |
| F04 | confirmed | no role profile enables `tx`/`rtao::intents`; default-feature leaks; `MechId(2)` |
| F05 | confirmed | Python never transmits a frame whose caller is gone; the plan omits this, and the lease drop makes it a nonce collision |
| F06 | partially_confirmed | dual NonceManager is real; `Leased(NonceLease)` cannot travel inside `&SubmitOptions`, and the inner shield extrinsic is never submitted |
| F07 | confirmed | there is no way to build a `Client` over `FakeChain`; rule 2 also forbids `subxt-rpcs` in testkit |
| F08 | partially_confirmed | "no C toolchain" is unachievable; reqwest `rustls-no-provider` panics without a preconfigured TLS config; blake3 also compiles C/asm |
| F09 | confirmed | tokio-tungstenite uses the process-default provider and panics when both providers are enabled |
| F10 | partially_confirmed | version split verified; deny failure is conditional; moving the server to rtao-cli drops Python library exports |
| F11 | partially_confirmed | the gap is real but overstated: the catalog `metagraph` read is a raw dict and the extension-trait default compiles |
| F12 | confirmed | reproduced: Value-first parse fixes untagged/flatten under `arbitrary_precision` |
| F13 | partially_confirmed | clippy misses tokio guards (reproduced); parking_lot guards become `Send` with `send_guard`, so the fix should use std `RwLock` |
| F14 | confirmed | miner `router()` gets no `StateCell` |
| F15 | confirmed | `step` cadence, errors and cancellation are unspecified; the `on_metagraph` comment contradicts its empty body |
| F16 | confirmed | 64 concurrent requests × 12 s without a connect timeout or deadline; upstream gathers everything at once |
| F17 | partially_confirmed | the DoS gap is real; the fix's order (limiter before commit) lets replays drain a legitimate hotkey's rate-limit bucket |
| F18 | partially_confirmed | the conflict is real; the fix drops Python's metadata-primary `kind_of`/`codec_of` (`STORAGE_ITEMS`) |
| F19 | confirmed | the default names a module only |
| F20 | confirmed | reproduced: warning is non-fatal under `-D warnings`; `allow-invalid` silences it |
| F21 | confirmed | reproduced 2.5 ms digest, about 1.4-1.6 ms tlock; 11 ms `DecodeAsType`; fsync writes are unlisted |
| F22 | confirmed | no mechanism for "cancelled when the last clone drops" |
| F23 | confirmed | tungstenite `max_frame_size` 16 MiB; soketto sends one frame |
| F24 | confirmed | axum extractors fall back to a 2 MiB limit |
| F25 | confirmed | reproduced the E0277-style "future cannot be sent" for §4.2 and `&dyn Clock` |
| F26 | confirmed | 27 features, 378 raw combinations, about 315 runs |
| F27 | confirmed | `AxonTarget` has no owner; btauth lacks `rtao-runtime` |
| F28 | confirmed | `anyhow` in rtao-neuron's public traits contradicts §2.5 |
| F29 | partially_confirmed | rows must follow the corrected F01/F02/F10/F11/F18 decisions |

## Details

### F01: partially_confirmed
Verified: the rtao-runtime row (A:532) depends only on core and primitives, yet owns `verify_surface`/`SurfaceReport` and the generated `calls::*` builders. §3.7 (A:1180) has `verify_surface(&RuntimeHandle)` and §3.8 (A:1206) has `Call::compose(&self, rt: &RuntimeHandle) -> Result<CallBytes, ComposeError>`. `RuntimeHandle`, `MetadataLevel` and `CallBytes` sit in the rtao-client row (A:536), which depends on rtao-runtime (A:563). That is a cycle. The fix is right but leaves §1.4's metadata-model row (A:332) saying `rtao-client::RuntimeHandle`.

**Corrected fix:** apply the reviewer's fix unchanged, and also change architecture §1.4's metadata-model row (A:332) from `rtao-client::RuntimeHandle` to "`rtao_runtime::RuntimeHandle` (cached by `rtao_client::RuntimeCache`)".

### F02: partially_confirmed
Verified: `PY/sync.py:24-25` imports `Policy` and `NAMESPACES`. It proxies `read`, `mine_pow_registration`, `plan`, `execute`, `execute_tool`, `submit_shielded` and `tools` (:401-476). `PY/_subtensor.py:36` imports `SetWeights`. These rtao homes are at L5-L6, above rtao-client (L4).

Gap in the fix: `bt.set_weights(.., wallet="name", hotkey=..)` resolves wallet names through `Wallet`/`as_wallet` (`PY/_subtensor.py:152-158`), so the facade feature needs `wallet`. The fix also omits the module-level `set_weights` and `close_shared_clients` (:129-150, :167-199).

**Corrected fix:** home is `rtao::blocking::{SyncClient, Subtensor, SyncSnapshot, set_weights, close_shared_clients}` in `crates/rtao/src/blocking.rs`, with feature `blocking = ["intents-all", "metagraph", "wallet", "rtao-client/blocking"]`; `mine_pow_registration` is additionally `#[cfg(feature = "pow")]`. `rtao_client::blocking` provides only `BlockingRuntime`: a current-thread runtime on its own thread, a port of the `PY/sync.py` `_Loop` (:64-98). Then make the other edits the reviewer listed.

### F03: partially_confirmed
Verified:
- `View`/`ViewExt` (rtao-client) return `ChainError` (A:1006-1046), but `ChainError` is in rtao-tx (A:537).
- §4.7 defines `ChainError` as a struct (A:844), yet A:969 and A:1086 use enum variants.
- Only `TxError` has `RpcPolicy` (A:1858).
- `ReadError` is never defined (A:1188, A:1904).

The fix misses that rtao-client's `NonceManager::{lease, lease_pair, fresh}` also return `ChainError` (A:1261-1263). `SubmitFailure` (A:1225) is undefined.

**Corrected fix:** the reviewer's fix, plus:
- replace `ChainError` with `ClientError` in §3.9.1 as well;
- define `SubmitFailure { Client(ClientError), Submit(rtao_rpc::SubmitError), Pool(PoolRejection) }`;
- map `SubmitFailure` in the executor following the §3.9.5 table.

### F04: confirmed
Verified:
- None of `neuron`, `validator`, `miner` or `owner` enables `tx` (A:585-618). `dep:` syntax leaves no implicit feature to `cfg` on.
- The role manifests use `default-features = false` (A:665, :680, :691).
- `rtao-core` has `offload` (tokio) on by default (A:190, :529), and `rtao-neuron` has `prometheus` on by default (A:542). No text anywhere says intra-workspace dependencies use `default-features = false`.
- `mechanism_count`: the §4.2 rule ("narrowest type of the wrapped call's argument ident") plus `admin-utils/src/lib.rs:2094-2097` gives `MechId`, which §5.1 defines as `MechId(pub u8)`, so `mechanism_count: 2` does not type-check.

### F05: confirmed
Verified that Python never sends a frame nobody awaits:
- `_submit` discards on any `BaseException` (`rpc.py:495-504`);
- the docstring says as much (:516-526);
- `unsent` filters out done futures (:549);
- the resend loop skips done futures (:713-714).

§3.2.1 (A:826-830) gives the resend rule without the caller-alive condition, and `NonceLease::drop` clears the cache (A:1271-1273). A late transmit therefore reuses a re-fetched nonce. `tokio::sync::oneshot::Sender::is_closed` exists. A residual race between the check and the write remains; it is equivalent to Python's accepted FateUnknown window.

### F06: partially_confirmed
Verified: `Executor` carries its own `nonces: Arc<NonceManager>` (A:1743), nothing ties it to the `Client`, and Python keeps one cache per connection (`interface.py:134`).

Two errors in the fix:
1. `Client::submit` takes `opts: &SubmitOptions` (A:1225), and `NonceLease::settle(mut self)` consumes the lease (A:1267-1270), so a `NoncePolicy::Leased(NonceLease)` inside borrowed options cannot be settled.
2. "submits with `Leased(inner)`" is wrong. The inner extrinsic is sealed into the carrier and never submitted on its own (§3.11 steps 3-5).

**Corrected fix:**
- One `Arc<NonceManager>` lives in `Client`'s inner (`client.nonces()`), and the `Executor` field is deleted.
- Keep `NoncePolicy { Auto, Explicit(u64) }` and add `Client::submit_leased(call, signer, lease: NonceLease, opts)`, which takes the lease by value.
- §3.11: lease a pair; sign the inner call at `inner.value()`; submit the carrier with `submit_leased(carrier)`; settle the inner lease when step 6 finds the inner extrinsic, and drop it otherwise.
- Extend scenario 17 to two `Executor`s.

### F07: confirmed
Verified: `Client::connect(ClientConfig)` is the only constructor (A:935). The submission and state APIs exist only on `Session` (A:788-799). `FakeChain` is documented as `RpcClientT` "for client, executor, reads, scheduler" (A:3303-3310). `subxt-rpcs` defines `RpcClient { client: Arc<dyn RpcClientT> }` with `RpcClient::new<R: RpcClientT>` (rpc_client.rs:15-46), and `Error::Client(Box<dyn Error + Send + Sync>)` allows `downcast_ref` (lib.rs:69-77). §2.3 rule 2 (A:573-574) forbids `subxt-rpcs` in rtao-testkit, which already contradicts the plan's own `FakeChain`.

### F08: partially_confirmed
Verified:
- reqwest 0.13.5's `rustls` feature enables `__rustls-aws-lc-rs`.
- aws-lc-sys 0.45.0 has `cc` and `cmake` build-dependencies.
- ring 0.17.14 compiles C with `cc` (build.rs:53-95).
- tokio-tungstenite declares rustls with `default-features = false`.
- In the core's no-host lockfile, only blake3 depends on `cc`.
- Research F (:224) recommended `ring`.

Errors in the fix:
1. With `rustls-no-provider`, reqwest panics at client build time unless a process default is installed or a preconfigured config is passed (`REG/reqwest-0.13.5/src/async_impl/client.rs:2497-2506`). The fix has to require `tls_backend_preconfigured`, which F09 supplies.
2. "The only C code is `ring`" is false. blake3 compiles SIMD assembly with `cc` whenever a compiler exists (`REG/blake3-1.8.5/build.rs:346-354`).

The replacement list should also include A:119 (§0.2).

**Corrected fix:**
- The TLS row adds: every rtao `reqwest::Client` is built with `ClientBuilder::tls_backend_preconfigured(<shared ring ClientConfig>)`.
- The claim text becomes: "the only C code is `ring` and blake3's optional SIMD assembly, built by the system C compiler rustc already needs for linking".
- Everything else as the reviewer wrote it, including the no-host check extension and the `rust:1.89-slim` M2 exit criterion.

### F09: confirmed
Verified:
- tokio-tungstenite builds its default rustls config with `ClientConfig::builder()` (tls.rs:125-129).
- rustls's `get_default_or_install_from_crate_features` panics when the provider is ambiguous (crypto/mod.rs:243-285).
- reqwest uses its own provider (client.rs:719-729).

The fix's APIs exist: `connect_async_tls_with_config` (connect.rs:59-70), `builder_with_provider` (client_conn.rs:354) and `tls_backend_preconfigured` (client.rs:2209). Keep the shared config without ALPN, because WebSockets need HTTP/1.1.

### F10: partially_confirmed
Verified from the crates.io index cache:
- axum 0.8.9 requires `tokio-tungstenite ^0.29.0` (0.8.5-0.8.8 require ^0.28);
- tokio-tungstenite 0.30 requires `tungstenite ^0.30`;
- deny bans duplicate `tungstenite` (A:3556-3559);
- `rtao-cli` turns `extension` on by default (A:544).

The deny failure is conditional: it happens only if the bridge server uses axum `ws`, and the plan does not say how the server is built. The fix moves the server into rtao-cli. That drops Python's library exports `BridgeServer`, `run_bridge`, `ensure_bridge`, `start_bridge_daemon` and `stop_bridge_daemon` (`PY/extension/__init__.py:11-40`) and contradicts MM's `extension/bridge.py -> rtao_signer` row. The cited axum `ws` feature is at `REG/axum-0.8.8/Cargo.toml:133-139`, not :65-68.

**Corrected fix:** keep (a) the version alignment as written. For (b), the server (`bridge.py`, `session.py`, `tokens.py`), the client and `ExtensionSigner` all live in `rtao-signer` under `extension`: the server on axum `ws`, the client on tokio-tungstenite. rtao-cli's `extension bridge` (`daemon.py`) only runs the server. For (c), rule 2 allows tokio-tungstenite in rtao-signer (feature `extension`, directly or through `axum/ws`) at the single workspace version, and allows `subxt-rpcs` in rtao-rpc, rtao-client and rtao-testkit.

### F11: partially_confirmed
What holds: the `commitments` read and the curated `Subnets::{metagraph, commitments}` need rtao-metagraph code, which sits at L6.

What is wrong: "the default cannot give `dispatch(ReadName::Metagraph)` an implementation" and "M1 cannot compile". The catalog `metagraph` read is Python's raw `get_metagraph` dict with `name`/`symbol` decoded to text (`PY/reads/subnets.py:176-190`), not the `Metagraph` model. `fetch_commitments` needs only storage maps (`PY/metagraph.py:256-286`). So the default (commitments below rtao-metagraph, `metagraph()` as an extension trait) compiles.

The residual defect: the default is undecided (it says "or") and turns a Python namespace method into a trait import.

**Corrected fix:** use the reviewer's decision (model in `rtao_reads::models::metagraph`, re-exported by rtao-metagraph), and add: "The catalog `metagraph` read stays Python's raw `SubnetInfoRuntimeApi.get_metagraph` dict with `name`/`symbol` as text (`PY/reads/subnets.py:176-190`), not `Metagraph::to_json()`; `client.subnets().metagraph(..)` needs no extension-trait import."

### F12: confirmed
Reproduced with serde_json 1.0.154 and `arbitrary_precision`:
- direct parse of the untagged list/map, number-or-string `1.5` and the flatten-f64 struct fails;
- parsing to `Value` first and then `from_value` succeeds for all of them;
- transport frames with a `Value` field parse (apcheck2).

`CORE/codec/value.rs:58-75` relies on the feature for integers wider than 64 bits.

### F13: partially_confirmed
Reproduced with clippy 0.1.99: `await_holding_lock` flags the std guard but says nothing about a `tokio::sync::RwLock` write guard. parking_lot guards held across `.await` in an `impl Future + Send` method fail to compile, as the reviewer says, but they compile once any crate enables `parking_lot/send_guard` (`REG/parking_lot-0.12.5/src/lib.rs:34-39`; reproduced). std guards stay `!Send` in both cases (reproduced), and short `s.write().push(..)` calls between awaits compile.

**Corrected fix:**
```rust
#[derive(Clone)]
pub struct StateCell<S>(Arc<std::sync::RwLock<S>>);
// read()/write() recover from poisoning via unwrap_or_else(PoisonError::into_inner)
```
Add a `compile_fail` doctest. The §9.2 and P §7.2 line becomes `s.write().update(&mg, &replies);`.

### F14: confirmed
`Miner::router(&self, cx: &NeuronCx)` receives no `StateCell` (A:2931). `NeuronCx` holds no state (A:2744-2748), and the order of `router` versus `on_start` is unspecified.

### F15: confirmed
- No cadence, error or cancellation policy exists for `step` (A:2779-2785).
- Proposal C's "called in a loop, paced and cancelled by the runtime" (`panel/arch-C-operator-first.md:1597`) was dropped.
- The `on_metagraph` default body is empty, but its comment says "reset replaced hotkeys" (A:2784).

### F16: confirmed
`FanOut { concurrency: 64, timeout: 12 s }` has no connect timeout or call-level deadline (A:2512-2517). Upstream gathers every query at once (`signed-requests.mdx:199-200`). The default `MaxAllowedUids` is 256 (`runtime/src/lib.rs:916`), so the worst case is 4 × 12 s. Note: `deadline = timeout + 1 s` cuts off queued targets when the number of targets exceeds `concurrency`; the default of 256 covers default subnets.

### F17: partially_confirmed
The gap is real: Python's store is unbounded (`http_auth.py:126-154`), and the plan's `max_entries` has no defined semantics (A:2409).

The fix's order (authenticate, then policy, then **limiter**, then commit) opens a new attack. Axon traffic is plain HTTP, so an attacker can capture a legitimate validator's signed request and replay it within the 10 s window. Each replay spends that validator's PerHotkeyLimiter tokens before the commit rejects it. Today's order rejects replays with 401 before the limiter.

**Corrected fix:** authenticate (401), then policy (403), then commit (401 on replay), then limiter (429), then the handler. Keep the per-hotkey and global bounds; return 503 when full and never evict; register D-024 as written.

### F18: partially_confirmed
The conflict is real: Python's API is name-based (`hyperparams.py:474-770`), rule 3 forbids hand-written code in rtao-catalog, and the default's "re-export" cannot have the `to_raw(name, value)` signature.

The fix drops Python's metadata-primary precedence:
- `kind_of` consults the type ident of the storage value named in `STORAGE_ITEMS` before the table (`:416-499`);
- `codec_of` goes explicit bits, then type-ident codec, then kind default (`:663-680`).

`spec_to_raw(&HyperparamSpec, ..)` cannot reproduce either, and `STORAGE_ITEMS` has no home.

**Corrected fix:**
- Primitives hold the `HyperparamSpec` type, `ratio_fraction`, `proportion_to_raw`, `U64_MAX`, and pure helpers over a resolved `(kind, (signed, bits), (lo, hi))`.
- Catalog holds the generated `HYPERPARAMS`, `spec(name)`, and an extracted `STORAGE_ITEMS` of `(name, pallet, item)`.
- `rtao_intents::hyperparameters` holds the name-based `metadata_kind`, `kind_of`, `codec_of`, `raw_bounds`, `to_raw`, `normalized`, `annotate`, `value_forms`, `doc_of` and `short_of`, keeping Python's precedence through the generated rtao-runtime storage descriptors.
- Reads use only `ratio_fraction`.

### F19: confirmed
`PY/config.py:12-27`, `:58-133`, `:138-341` and `:354-468` hold the files, TTLs, books and caches. They are used by `PY/client.py:613-633` and the CLI. The default (P §14.3 #8) names a module only.

### F20: confirmed
Reproduced: an entry pointing at an absent path prints "does not refer to a reachable function … add `allow-invalid = true`". `cargo clippy -- -D warnings` exits 0, and `allow-invalid` silences the warning. Exactly seven entries are host-gated (`CORE/lib.rs:10-29`; `timelock/mod.rs:241, 275, 297`). The MSRV job runs only `cargo check`. Optionally, `xtask core-surface` could also assert that the ten paths exist under `--all-features`, because `allow-invalid` hides renames.

### F21: confirmed
Reproduced with core v477 (no host, release build): `metadata_digest` best 2.5 ms; `encrypt_and_compress` 1.41 ms (n=256) and 1.57 ms (n=2500). `bench-run-3.txt` has `DecodeAsType` at 11,000.6 µs for 408,250 bytes. A:745-751 says "dynamic decodes" while P §2 says "large decodes".

### F22: confirmed
A:737-745 requires cancellation when the last clone drops but specifies no `Weak`/`DropGuard` mechanism.

### F23: confirmed
- tungstenite defaults to `max_frame_size` 16 MiB and `max_message_size` 64 MiB (`protocol/mod.rs:95-102`).
- soketto's `send_text` sends one frame.
- The polkadot-sdk fork at `cacb431` has `RPC_DEFAULT_MAX_RESPONSE_SIZE_MB = 15` (`config.rs:56`).

### F24: confirmed
axum-core's `with_limited_body` applies 2,097,152 bytes when no `DefaultBodyLimitKind` extension is present (`request.rs:316-327`).

### F25: confirmed
Reproduced with rustc 1.99: without `Send + Sync` on the supertrait, a provided `+ Send` method that captures `&self` fails ("`Self` is not `Sync`"). `&dyn Clock` inside a `Send` future also fails. `View`, `Signer`, `KeyView` and `NonceStore` are already `Send + Sync`; `IntentInterface`, `DynIntent`, `Clock` and `PolicySubject` are not.

### F26: confirmed
§2.4 lists 27 features. `hack_count.py` gives 378 raw combinations, 314 after dedupe plus the no-default run, and 94 that enable host. The CI step is at A:4133-4136.

### F27: confirmed
- `AxonTarget` is used at A:2512 and A:2618 and owned nowhere.
- `AxonInfo` is in `rtao-runtime::models` (A:2569), but the btauth dependencies are signer, core and optional metagraph (A:541).
- Python's `_axon_endpoint` is at `metagraph.py:563-573`.

### F28: confirmed
A:647 forbids `anyhow` in library code, yet A:2781-2782 and A:2933 use `anyhow::Result` in rtao-neuron's traits.

### F29: partially_confirmed
The rows depend on F01, F02, F10, F11, F18, F19 and F27 and must follow their corrected decisions:
- `_subtensor.py`: "`rtao::blocking::{Subtensor, set_weights, close_shared_clients}` (facade, `blocking`)".
- `sync.py`: "`rtao::blocking::SyncClient` (facade, `blocking`) over `rtao_client::blocking::BlockingRuntime`".
- `_transport/codec.py` and `_transport/contract.py`: as in the reviewer's fix.
- `metagraph.py`: "`rtao_reads::models::metagraph::{..}` (re-exported by `rtao_metagraph`)", with the axon URL at `:563-573` going to `rtao_primitives::axon::axon_base_url`.
- `namespaces.py`: add "the catalog `metagraph` read stays the raw `get_metagraph` dict (`PY/reads/subnets.py:176-190`)".
- `config.py`: "`rtao_client::config` (M1)".
- `hyperparams.py`: "`rtao_catalog::hyperparams::{HYPERPARAMS, STORAGE_ITEMS}` + `rtao_primitives::hyperparams` (table-agnostic) + `rtao_intents::hyperparameters` (name-based, metadata-primary kind)".
- `extension/bridge.py`: "`rtao_signer` bridge server (feature `extension`, axum `ws` at the workspace tokio-tungstenite version); run by `rtao-cli` `extension bridge`".
- `extension/client.py`: as in the reviewer's fix.
- Mark the four Gaps rows "decided: PLAN §14.3 #8, #5, #6, #9".
