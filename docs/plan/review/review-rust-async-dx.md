# Review: Rust API design, async/tokio correctness, performance and DX

Lens: crate layering, features and role profiles, error types, Send/Sync bounds, cancellation, backpressure,
blocking work, metagraph cost, fan-out, compile cost, MSRV, and what a subnet owner writes.
Paths: `A` = `final/docs/plan/architecture.md`, `P` = `final/PLAN.md`, `MM` = `final/docs/plan/python-module-map.md`,
`REG` = `~/.cargo/registry/src/index.crates.io-*/`, `W` = `scratchpad/review/work/` (scratch programs I ran; rustc/clippy 1.99).

Checked and found correct (no finding): `RpcClientT` signature and `Error::Client(Box<dyn Error + Send + Sync>)`
(REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26-52, src/lib.rs:69-77); subxt-rpcs without default features has no
jsonrpsee; tungstenite `Error::Http(Box<Response<Option<Vec<u8>>>>)` and `CloseCode::Policy`=1008; scale-decode derive
looks fields up by name (REG/scale-decode-derive-0.16.2/src/lib.rs:222-236, 356-371); `max_in_flight` 64 matches the
node's `RPC_DEFAULT_MESSAGE_CAPACITY_PER_CONN = 64`; RPITIT `+ Send` futures through a blanket `ViewExt` impl stay
`Send` in generic code (W/rpitit/t2.rs compiles); MSRV 1.89 is above every checked dependency floor
(subxt-rpcs 1.88, criterion 1.86, reqwest/clap/tokio-util 1.85).

---

## F01 [major] architecture.md — `RuntimeHandle` and `CallBytes` live above the crate that needs them (cycle)
- **Location**: §1.5 row `Runtime` → "`rtao_client::RuntimeHandle(Arc<RuntimeEntry>)`"; §2.2 `rtao-client` row; §3.7
  "`verify_surface(&RuntimeHandle) -> SurfaceReport`"; §3.8 "`pub fn compose(&self, rt: &RuntimeHandle) -> Result<CallBytes, ComposeError>`".
- **Problem**: `rtao-runtime` (L3) owns `verify_surface` and the generated `Call::compose`, both typed over
  `RuntimeHandle` and returning `CallBytes`, but both types are defined in `rtao-client` (L4), which depends on
  `rtao-runtime`. Cargo rejects the cycle; PLAN §4.3 and MM (`_transport/codec.py`, `_transport/contract.py`) copy it.
- **Evidence**: A:350, A:532 (rtao-runtime deps: core, primitives), A:536, A:558-567, A:1180, A:1206-1208; P:222.
- **Fix**: "`rtao-runtime` owns `RuntimeHandle(Arc<RuntimeEntry>)`, `RuntimeEntry` (parsed V15 `Runtime`, lazily
  decoded V16 view, `MetadataLevel`, `SurfaceReport`, shape cache, name→index caches), `MetadataLevel`, `CallBytes` and
  `ComposeError`; none does I/O. `rtao-client` keeps `RuntimeCache` (V15 fetch, needle-only V14, LRU, parent-runtime
  resolution, disk cache, upgrade watcher) and re-exports the five types." Move them between the §2.2 / P §4.3 rows; MM
  `_transport/codec.py` → `rtao_runtime::RuntimeHandle` + `rtao_client::RuntimeCache`, `_transport/contract.py` →
  `rtao_runtime::CallBytes`, `rtao_client::{UnsignedExtrinsic, ExtrinsicReport}`.

## F02 [major] architecture.md — `blocking::{SyncClient, Subtensor}` cannot live in `rtao-client`
- **Location**: §2.2 `rtao-client` row; §2.4 `blocking = ["rtao-client/blocking"]`; §3.1 "except `rtao_client::blocking`"; §14 M6; P:222, P:1279, P:1287.
- **Problem**: Python's `SyncClient`/`Subtensor` expose reads and namespaces, `plan/execute/execute_tool/submit_shielded/tools`,
  `mine_pow_registration` and `set_weights` (an intent). Those are `rtao-reads`/`rtao-tx` (L5) and `rtao-intents` (L6),
  which depend on `rtao-client` (L4). The M6 deliverable cannot be built where it is homed; MM inherits it.
- **Evidence**: PY/sync.py:24-25 (`from .intents import Policy`, `from .namespaces import NAMESPACES`), :401, :407, :463-476;
  PY/_subtensor.py:36 (`SetWeights`), :167-199; A:536, A:612, A:558-567.
- **Fix**: "`rtao::blocking::{SyncClient, Subtensor, SyncSnapshot}` lives in the facade (`crates/rtao/src/blocking.rs`,
  feature `blocking = ["intents-all", "metagraph", "rtao-client/blocking"]`; `mine_pow_registration` also needs `pow`).
  `rtao-client`'s `blocking` feature provides only `rtao_client::blocking::BlockingRuntime` (a current-thread tokio
  runtime on a dedicated thread, the port of `PY/sync.py` `_Loop`)." Update §2.2, §2.4, §3.1, §14 M6, P §4.3/§12.8, MM rows `sync.py`, `_subtensor.py`.

## F03 [major] architecture.md — the read-side error type is defined above `rtao-client`, used as both struct and enum, and drops policy refusals
- **Location**: §3.4 "`ChainError::RuntimeApiUnavailable`"; §3.5 `View` methods `-> Result<…, ChainError>`; §3.6 "`ChainError::ApiVersion { need, have }`";
  §3.7 "`ReadError::SurfaceChanged`"; §4.7 "`pub struct ChainError { pub message, pub name, pub code }`"; §3.2.3 "The executor and reads surface `RpcPolicyError`".
- **Problem**: `View` is in `rtao-client` (L4) but returns `ChainError`, which §2.2 puts in `rtao-tx` (L5); §4.7 defines it
  as a struct while §3.4/§3.6 use enum variants; the struct cannot carry `PolicyRefusal` (status, retry_after, policy,
  reason), so reads cannot surface `RpcPolicyError` and a neuron cannot honor `retry_after`; `ReadError` is never defined.
- **Evidence**: A:537, A:969, A:1002-1016, A:1086, A:1188, A:1844, A:1858, A:1904, A:879.
- **Fix**: add to §3.5 `#[non_exhaustive] pub enum ClientError { Rpc(RpcError) /* incl. Policy(PolicyRefusal), StateDiscarded, ConnectionLost, MaxRetriesExceeded */,
  Decode { what: Cow<'static, str>, source: CoreError }, RuntimeApiUnavailable { api, method }, ApiVersion { api, need: u32, have: u32 },
  SurfaceChanged { descriptor: &'static str, detail: String }, SigningDisabled { reason: String } }` with `code() -> ErrorCode`
  and `policy() -> Option<&PolicyRefusal>`; `View`/`ViewExt`/`Client`/`Snapshot` return it. §4.8: `#[non_exhaustive] pub enum ReadError
  { Client(ClientError), InvalidParam { name, reason }, NotYetPorted(ReadName), SurfaceChanged { descriptor, detail } }` with `code()`.
  §4.7: `impl From<ClientError> for TxError` (`Rpc(Policy)` → `TxError::RpcPolicy`, other `Rpc` → `TxError::Rpc`,
  `SigningDisabled`/`SurfaceChanged` → same-named variants, rest → `TxError::Chain` with the mapped code).

## F04 [major] architecture.md — role profiles do not enable the facade's `tx`/intents surface; facade module map undefined; internal default features
- **Location**: §2.4 feature block (`default = ["client","reads","tx"]`, `neuron = [..., "dep:rtao-neuron", "dep:rtao-intents"]`,
  `validator`/`miner`/`owner`); §2.2 `rtao` row "`prelude`, re-exports per feature"; P §7.4 "`use rtao::prelude::*; // [I] prelude contents are not enumerated`".
- **Problem**: role manifests use `default-features = false` and no role feature enables `tx`; with `dep:` there is no
  implicit feature to `cfg` on, so a facade gating `rtao::tx`/prelude on `feature = "tx"` exposes no `Executor`,
  `ExecuteOptions`, `PlanOptions` or `rtao::intents` in any role profile, yet P §7.2-§7.4 use them. `rtao-core`
  (`offload` → tokio) and `rtao-neuron` (`prometheus`) have default features: unless internal deps are declared
  `default-features = false`, rule 4 (no tokio in feature-less `rtao-btauth`) fails via `rtao-signer → rtao-core`, and
  the facade's `prometheus` cannot turn Prometheus off. (P §7.4 also writes `mechanism_count: 2`; the field type follows the
  call argument `MechId`, `pallets/admin-utils/src/lib.rs:2097`.)
- **Evidence**: A:585-618, A:529, A:542, A:577, A:653-694; P:510-546, P:627-651.
- **Fix**: `intents = ["tx", "reads", "dep:rtao-intents"]`; `neuron = ["intents", "metagraph", "wallet", "btauth-client", "dep:rtao-neuron"]`;
  `intents-all = ["intents", "rtao-intents/all"]`; `pow = ["intents", "rtao-intents/pow"]`. Add a facade-surface table:
  always `rtao::{core, primitives}`; `client` → `rtao::client` (prelude `Client, ClientConfig, View, ViewExt, Snapshot, BlockRef`);
  `reads` → `rtao::reads`; `tx` → `rtao::{tx, signer}` (prelude `Executor, ExecuteOptions, PlanOptions, Policy, ExtrinsicResult, TxError`);
  `intents` → `rtao::intents` (prelude `AnyIntent`); `metagraph`, `wallet` (prelude `Wallet, HotkeyFile`), `btauth`,
  `neuron` (prelude `NeuronConfig, NeuronRuntime, NeuronCx, Validator, Miner, StateCell, WeightsProposal`), `evm`,
  `blocking`; prelude always has `NetUid, MechId, Uid, AccountId, Balance, Tao, Alpha`. State that every intra-workspace
  dependency is `default-features = false`. Compile P §7.2-§7.4 as `crates/rtao/examples/{validator,miner,owner}.rs`
  with `required-features` per profile in `pr-gates`; write `mechanism_count: MechId(2)`.

## F05 [major] architecture.md — a cancelled, never-transmitted submit is still sent after reconnect (nonce reuse)
- **Location**: §3.2.1 "A submission queued while the socket was down and never sent goes out normally on the new connection, as in Python's `_resubmit_pending`"; §3.9.1 `impl Drop for NonceLease`.
- **Problem**: dropping a submit future drops its `NonceLease`, which clears the nonce cache. If the supervisor still
  transmits the queued frame later, the next lease has already re-read `account_nextIndex` = n, so two extrinsics carry
  nonce n (one fails with `Priority is too low`, or an untracked one lands). Python discards pendings whose caller is
  gone; the plan omits that condition.
- **Evidence**: PY/_transport/rpc.py:495-504 (`_submit` → `_discard_pendings` on `BaseException`, incl. cancellation),
  :516-526 ("so a reconnect never resubmits a frame nobody is awaiting"), :549 (`not p.future.done()`), :716-717
  (`if pending.future.done(): continue`); A:828-830, A:1253-1283.
- **Fix**: append to §3.2.1: "— unless its caller has gone away. Each pending entry holds the caller's `oneshot::Sender`;
  before any frame is transmitted (first send, resend after reconnect, release from the rate-limit queue) the supervisor
  drops entries whose `Sender::is_closed()`. For `author_submit*` this is mandatory: the dropped caller's `NonceLease`
  already cleared the nonce cache (port of `rpc.py:495-504, 549, 716-717`)." §3.15 row: `cancelled_unsent_submit_is_never_transmitted`.

## F06 [major] architecture.md — two owners for the nonce cache
- **Location**: §3.9 `Client::submit(…)` with `NoncePolicy // Auto | Explicit(u64)`; §3.9.1; §3.11 step 2; §4.6 `pub struct Executor { client: Client, policy: Option<Policy>, nonces: Arc<NonceManager> }`.
- **Problem**: `Client::submit` (Auto) must lease from a manager, and `Executor` carries its own. Several executors
  (scheduler, `AxonPublisher`, `cx.executor`, owner tools) or a mix of `client.submit`/`executor.execute` get separate
  caches for one account and hand out the same nonce. `Explicit(u64)` cannot carry a `NonceLease`, so the RAII
  clear-on-failure rule does not reach `Client::submit` for the shield pair. Python keeps one cache per connection.
- **Evidence**: PY/_transport/interface.py:134 (`self._nonces = NonceCache(self._session)`); A:1225-1236, A:1253-1283, A:1425, A:1743.
- **Fix**: "`Client` owns exactly one `Arc<NonceManager>` in its shared inner (`client.nonces()`); every clone,
  `Snapshot`, `Executor` and framework service uses it." `pub struct Executor { client: Client, policy: Option<Policy> }`;
  `pub enum NoncePolicy { Auto, Explicit(u64), Leased(NonceLease) }`; §3.11 uses `client.nonces().lease_pair(..)` and
  `Leased`. Scenario 17: 50 pipelined transfers split over two `Executor`s built from clones of one `Client`, no gap or collision.

## F07 [major] architecture.md — `Client` has no seam for `FakeChain`
- **Location**: §3.3 "`Client::connect(ClientConfig)`"; §3.2 Session-only `submit`/`submit_and_watch` (`SubmitError`), `state()`, `archive()`; §11.3 "`FakeChain` … Implements `RpcClientT` … for client, executor, reads, scheduler", "submission state machine run for real".
- **Problem**: the only constructor connects a `Session`, and submission is specified against Session-only methods,
  so the every-PR `FakeChain` lane cannot drive `Client`/`Executor`/scheduler.
- **Evidence**: A:785-800, A:935, A:3303-3310; REG/subxt-rpcs-0.51.1/src/client/rpc_client.rs:15-16, 42 (`RpcClient { client: Arc<dyn RpcClientT> }`, `RpcClient::new`), src/lib.rs:69-77.
- **Fix**: §3.3: "`Client` holds a `subxt_rpcs::RpcClient` plus an optional `SessionHandle` (state, events, archive) for
  real sessions. Every RPC, including `author_submitExtrinsic` (`request_raw`) and `author_submitAndWatchExtrinsic`
  (`subscribe_raw`, unsubscribe `author_unwatchExtrinsic`), goes through it; typed errors cross the trait as
  `Error::Client(Box<RpcError | SubmitError>)` and are recovered with `downcast_ref`. Constructors: `Client::connect(ClientConfig)`
  and public `Client::from_rpc(rpc: RpcClient, cfg: ClientConfig)`. `FakeChain` scripts `FateUnknown` as
  `Error::Client(Box::new(SubmitError::FateUnknown { hash }))`." Allow `subxt-rpcs` in `rtao-testkit` in rule 2.

## F08 [major] PLAN.md — "no C toolchain" is false for every TLS-using neuron, and the no-host check cannot see it
- **Location**: P §1.4 row "weight-setting validator, no C toolchain"; §1.5 #5 "need no libsodium, OpenSSL, Perl or C compiler"; §5.7; §6.1 #5; §7.1 manifest comments; §7.5; §12.4 M2 exit "builds without a C toolchain"; §17 "no-host profile"; A §1.7, §2.6, §2.7, §9.11, §16 #5.
- **Problem**: neurons connect to `wss://` and the validator fans out over reqwest, so a rustls provider is in the graph.
  reqwest 0.13 `rustls` = aws-lc-rs → `aws-lc-sys` (C library, cc builder with cmake fallback); the alternative `ring`
  compiles C too. Rule 5 bans only host crates, so CI stays green while the M2 exit criterion is unachievable. rustc's
  default linker on `x86_64-unknown-linux-gnu` is `cc`, so builders have a C compiler anyway. RES/F recommended `ring`;
  the architecture dropped it.
- **Evidence**: REG/reqwest-0.13.5/Cargo.toml (`rustls = ["__rustls-aws-lc-rs", …]`, `rustls-no-provider`);
  REG/aws-lc-sys-0.45.0/Cargo.toml build-deps `cc`, `cmake`, builder/main.rs:725-811; REG/ring-0.17.14/build.rs:55-92;
  RES/F-rust-stack-options.md:224; scratchpad/design-tagpin/Cargo.lock (no-host core: only `blake3` depends on `cc`, and
  REG/blake3-1.8.5/build.rs:155-192, 346-354 fall back without a compiler).
- **Fix**: A §2.7 TLS row: "`rustls 0.23` with the `ring` provider only (`default-features = false, features = ["ring", "std", "tls12", "logging"]`);
  `reqwest = { version = "0.13.5", default-features = false, features = ["rustls-no-provider", "json", "http2"] }`;
  tokio-tungstenite `rustls-tls-native-roots`. `ring` needs only the C compiler rustc already uses as linker driver; no
  cmake, NASM, Go or Perl." Replace every "no C toolchain/compiler" with "no Perl, OpenSSL, libsodium, cmake or
  secp256k1 builds; the only C code is `ring`". M2 exit: "the no-host validator image builds in a stock
  `rust:1.89-slim` builder without installing Perl, OpenSSL/libsodium headers or cmake". Extend rule 5 / P §5.7 with
  `aws-lc-sys`, `aws-lc-rs`, `cmake`, `openssl-sys`, `native-tls`, `secp256k1-sys`, and "only `ring` and `blake3` have a
  `cc` build-dependency" (and use the union list already in P §5.7 in A §1.7, §2.3 rule 5 and Appendix E).

## F09 [major] architecture.md — implicit rustls provider selection panics when a subnet enables both providers
- **Location**: §2.7 WebSocket/HTTP rows; §3.2 `Session::connect`; §7.3 `SignedClient`.
- **Problem**: tokio-tungstenite builds its config with `ClientConfig::builder()`, i.e. the process default or the
  provider named by rustls crate features. If both `ring` and `aws-lc-rs` are enabled in the final graph (reqwest's
  `rustls` brings aws-lc-rs; many subnet deps bring ring), rustls panics at the first `wss://` connect, so
  `Client::connect` crashes the neuron at startup. reqwest picks its own provider and is immune.
- **Evidence**: REG/tokio-tungstenite-0.28.0/src/tls.rs:17-25, 125-129; src/connect.rs:61; REG/rustls-0.23.45/src/crypto/mod.rs:243-285
  ("Could not automatically determine the process-level CryptoProvider … make sure exactly one of the 'aws-lc-rs' and 'ring' features is enabled");
  REG/reqwest-0.13.5/src/async_impl/client.rs:715-735, 2209, 2239.
- **Fix**: §3.2: "TLS never consults the process default. `Session` builds `rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
  .with_safe_default_protocol_versions()?.with_root_certificates(native_roots).with_no_client_auth()` once and connects with
  `connect_async_tls_with_config(req, Some(ws_config), false, Some(Connector::Rustls(cfg)))`; `SignedClient`, `DrandClient`
  and `EvmRpc` pass the same config via `reqwest::ClientBuilder::tls_backend_preconfigured`. rtao never calls
  `CryptoProvider::install_default`." Test: build with both `rustls/ring` and `rustls/aws-lc-rs` and connect to a TLS `FakeNode`.

## F10 [major] PLAN.md — gap #9 decision, and the tokio-tungstenite version conflicts with axum 0.8.9 under the `tungstenite` duplicate ban
- **Location**: P §14.3 #9 ("allow it in `rtao-signer` under feature `extension` [I]"); P §14.2 Q16; P §12.3 M1 task "tokio-tungstenite 0.30"; A §2.3 rule 2, §2.7, §12.4.
- **Problem**: the bridge needs a WS client (`ExtensionSigner`) and an HTTP+WS server (`rtao extension bridge`, serving
  `bridge.html` on 127.0.0.1:39295). The natural server is axum `ws`, but axum 0.8.9 requires `tokio-tungstenite ^0.29`
  while rtao-rpc pins 0.30 and deny bans duplicate `tungstenite`: `rtao-cli` (whose default features include `extension`)
  fails `cargo deny` at M7, and every miner using axum WebSockets compiles two tungstenite stacks.
- **Evidence**: crates.io index cache entry axum 0.8.9 → `{"name":"tokio-tungstenite","req":"^0.29.0","optional":true}`;
  REG/axum-0.8.8/Cargo.toml:65-68, 264-266; index tokio-tungstenite 0.30.0 → `tungstenite ^0.30.0`; A:544, A:573-575, A:705,
  A:3556-3559; PY/extension/bridge.py:14, PY/extension/client.py:11.
- **Fix**: "Decided. (a) `[workspace.dependencies] tokio-tungstenite` = the version axum's `ws` feature requires (`0.29`
  for axum 0.8.9); Renovate groups `axum` and `tokio-tungstenite`; Q16 becomes '0.29 handshake error API = 0.28', pinned
  by the same M1 `FakeNode` test. (b) Bridge server in `rtao-cli` (feature `extension`) on axum `ws`; bridge client and
  `ExtensionSigner` in `rtao-signer` (feature `extension`) on tokio-tungstenite. (c) Rule 2: '`tokio-tungstenite` only in
  `rtao-rpc`, `rtao-testkit`, and under feature `extension` in `rtao-signer` and `rtao-cli` (directly or via `axum/ws`),
  always at the single workspace version; `subxt-rpcs` only in `rtao-rpc`, `rtao-client`, `rtao-testkit`'. The deny ban stays."

## F11 [major] PLAN.md — gap #5 decision: the metagraph read must live in `rtao-reads`
- **Location**: P §14.3 #5 default "move the commitment machinery below `rtao-metagraph`; expose `metagraph()` as an extension trait or from the facade [I]"; P §12.3 M1 "`rtao-metagraph` (model and view)"; A §2.2, §4.8, §8.2.
- **Problem**: `metagraph` is one of the 102 catalog reads that `rtao_reads::dispatch` must implement (M1 ships it), and
  the curated `Subnets::{metagraph, commitments}` and `commitments` read are in `rtao-reads`; an extension trait or a
  facade method cannot give `dispatch(ReadName::Metagraph)` an implementation, so M1's exit criterion cannot compile.
- **Evidence**: A:539 (rtao-reads owns "read output models"), A:540, A:1922-1924, A:2583-2591; PY/reads/identity.py:139-162,
  PY/metagraph.py:45-125, 238-403, PY/namespaces.py:161-188; P:1096, P:1128, P:1521.
- **Fix**: "Decided: `rtao_reads::models::metagraph::{Metagraph, MetagraphNeuron, NeuronRef, MetagraphView, Commitments,
  CommitmentStatus, from_dynamic}` (port of `PY/metagraph.py:45-403`), the `metagraph`/`commitments` reads,
  `fetch_commitments` and curated `Subnets::{metagraph, commitments}` live in `rtao-reads`, with the three-path
  differential test and view fuzz target. `rtao-metagraph` keeps `MetagraphSync`, `MetagraphHandle`, `MetagraphDiff`,
  `SyncPolicy`, `Preset`/selective fetches and `persist`, and re-exports the model. No layer changes." M1 deliverable →
  "`rtao-reads::models::metagraph` (model, view, dynamic path, commitments)"; move the model types in A §2.2.

## F12 [major] PLAN.md — gap #1 decision: Value-first parsing fixes `arbitrary_precision` for rtao and for subnet handlers
- **Location**: P §14.3 #1 default; P §5.8 #10 "open"; P §7.3 note; P §13 R19.
- **Problem**: the default (per-type custom deserializers, documentation) leaves subnet authors' `axum::Json<T>` types
  with `#[serde(untagged)]`/`#[serde(flatten)]` floats failing at run time in every rtao build. A verified uniform fix
  exists. Upstream cannot simply drop the feature: `to_corpus_json` needs it for >64-bit integers.
- **Evidence**: W/apcheck2 (serde_json =1.0.154 + `arbitrary_precision`): `from_slice::<WeightsArg>` → "ERR data did not
  match any variant of untagged enum WeightsArg", Value-first → `ok List([(1, 0.5)])`; flatten f64 `from_slice` → "ERR
  invalid type: map, expected f64", Value-first → ok; a JSON-RPC frame with `data.retry_after: 1.5` in a `Value` field
  parses correctly even inside an untagged enum (transport unaffected). CORE/codec/value.rs:64-75.
- **Fix**: "Decided (M0): (1) `rtao_primitives::json::{from_slice, from_str}` parse to `serde_json::Value` then
  `from_value::<T>`; all rtao text-JSON entry points use them (CLI `--args`/`call`, `execute_tool` from text,
  `UnsignedExtrinsic` import, btcli.json and books, oracle vectors); `AnyIntent::from_tool` already takes `Value`.
  (2) `rtao_btauth::server::Json<T>`, an axum extractor with the same parse and axum's rejection shape, is what the
  template and miner guide use. (3) Oracle/interop comparisons use a numeric-normalizing comparator, never `Value ==`.
  (4) The M0 test pins both behaviors. (5) Ask upstream to render >64-bit integers without the feature (not a blocker)."
  Mark §5.8 #10 handled; update §7.3's note and R19.

## F13 [major] architecture.md — `StateCell`'s "enforced by clippy `await_holding_lock`" does not work for a tokio lock
- **Location**: §9.1 `/// … (enforced by a clippy lint await_holding_lock …) pub struct StateCell<S>(Arc<tokio::sync::RwLock<S>>);`; §9.2 and P §7.2 `s.write().await.update(&mg, &replies);`.
- **Problem**: the lint only flags std/parking_lot guards; a tokio write guard held across `.await` (e.g. across a 12 s
  fan-out) is silent, and the `WeightScheduler` must read `State` to call `weights()` at its commit slot.
- **Evidence**: W/lintcheck, clippy 0.1.99: `-W clippy::await_holding_lock` flags the std guard ("this `MutexGuard` is held
  across an await point") and nothing for the identical tokio `RwLock` pattern. With `StateCell<S>(Arc<parking_lot::RwLock<S>>)`
  and `fn step(..) -> impl Future<Output = ()> + Send`, holding the guard across an await is a compile error ("future
  cannot be sent between threads safely … `RwLockWriteGuard` … is not `Send`"); `s.write().push(1);` between awaits compiles. A:2749-2752, P:574.
- **Fix**: "`#[derive(Clone)] pub struct StateCell<S>(Arc<parking_lot::RwLock<S>>)`; `read()`/`write()` return `!Send`
  guards, and every framework callback returns `impl Future + Send`, so a guard held across `.await` does not compile
  (no lint needed). `weights()`/`on_metagraph()` run under short locks." Example: `s.write().update(&mg, &replies);`.

## F14 [major] architecture.md — a miner's request handlers cannot reach its persisted `State`
- **Location**: §9.4 `fn router(&self, cx: &NeuronCx) -> axum::Router;` with `type State`; P §7.3.
- **Problem**: `State` (saved every 300 s) is reachable only from `on_start`/`on_metagraph`; `router()` gets no
  `StateCell`, `NeuronCx` holds none, and `router()` may run before `on_start`. Stateful miners must bypass the framework.
- **Evidence**: A:2929-2935, A:2744-2748, A:3080-3085; P:590-617.
- **Fix**: `fn router(&self, cx: &NeuronCx, state: StateCell<Self::State>) -> axum::Router;` ("`StateCell` is `Clone`;
  pass it with `.with_state(state)`"); the runtime calls `on_start` before `router`. P §7.3: `fn router(&self, _cx: &NeuronCx, _state: StateCell<()>) -> Router`.

## F15 [major] architecture.md — `Validator::step` pacing, error and panic semantics are undefined
- **Location**: §9.1 diagram; §9.2 `fn step(..) -> impl Future<Output = anyhow::Result<()>> + Send;`; `fn on_metagraph(..) {} // default: reset replaced hotkeys`.
- **Problem**: no cadence, overrun bound, error policy, panic isolation or shutdown rule. A back-to-back loop busy-spins
  or drains the 1 r/s bucket; an `Err` on a transient RPC error kills the validator if `run()` propagates it. Proposal C's
  "called in a loop, paced and cancelled by the runtime" was dropped. The `on_metagraph` default is empty, not a reset.
- **Evidence**: A:2735, A:2779-2785; panel/arch-C-operator-first.md:1597; A §3.2.5.
- **Fix**: add `pub struct StepPolicy { pub cadence: StepCadence /* EveryFinalizedBlock (default) | Interval(Duration) | Continuous { min_interval } */,
  pub timeout: Option<Duration>, pub backoff: Backoff /* 1 s doubling to 60 s, reset on Ok */ }` and
  `fn step_policy(&self) -> StepPolicy { StepPolicy::default() }`; "`step` runs in its own task in the runtime's `JoinSet`;
  an `Err` or panic is logged, counted in `rtao_step_errors_total`, flips `/readyz` after 5 consecutive failures and backs
  off, but never ends `run()`, which returns only on shutdown or a framework-fatal error; the first shutdown signal cancels
  the in-flight `step`." `on_metagraph` comment: "default: no-op; `EmaScores::apply(&MetagraphDiff)` drops replaced and deregistered hotkeys".

## F16 [major] architecture.md — `FanOut` has no overall deadline or connect timeout
- **Location**: §7.3 `FanOut { concurrency: 64, timeout: 12 s, retries }`, `fan_out(..) -> Vec<..>`; P §7.2.
- **Problem**: wall time is ceil(n/64) × 12 s × (retries+1) when axons hang: 48 s at 256 UIDs, 480 s at 2,500. Dead axons
  hold a slot for the full 12 s. Upstream's recipe gathers all queries (≈ one timeout); results arrive only at the end.
- **Evidence**: A:2512-2517; UP/docs/guides/signed-requests.mdx:199-200; RES/D-neuron-protocols.md:192 (v10 `ClientTimeout(total=12)`).
- **Fix**: `FanOut { concurrency: 256 [I], connect_timeout: 3 s [I], timeout: 12 s per attempt, deadline: Option<Duration> /* Some(timeout + 1 s), from the call, covers queueing and retries; unfinished → FanOutError::Deadline */, retries: 0 }`
  and `fan_out_stream(..) -> impl Stream<Item = (Uid, Result<R, FanOutError>)> + Send` (results as they arrive; `fan_out` collects it).

## F17 [major] architecture.md — unregistered signers can fill the nonce store; overflow behavior unspecified
- **Location**: §7.1 `InMemoryNonceStore { /* … max_entries bound (DoS guard) */ }`, step 5; §7.2 steps 3-4.
- **Problem**: any fresh keypair passes signature verification, so the replay insert happens before `MetagraphPolicy`
  rejects it; the store fills in seconds. At `max_entries`, rejecting makes the miner fail closed for real validators;
  evicting reopens replays. Python's store is unbounded.
- **Evidence**: A:2409, A:2453-2454, A:2497-2504; PY/http_auth.py:126-154; UP/docs/guides/signed-requests.mdx:143-156.
- **Fix**: split `Verifier::authenticate(..) -> Result<Authenticated, AuthError>` (steps 1-4) and
  `Authenticated::commit(self, &store) -> Result<Caller, AuthError>` (step 5); `verify()` = both in Python's order.
  `BtAuthLayer`: authenticate (401) → `CallerPolicy::admit` (403) → `PerHotkeyLimiter` (429) → commit (401 replay) →
  handler. `InMemoryNonceStore` bounds per hotkey (`max_per_hotkey`) and globally (`max_entries`); full → 503 and
  `rtao_btauth_nonce_store_full_total`, never evict. Register D-024: a replayed request from a policy-denied hotkey gets 403, not 401.

## F18 [minor] PLAN.md — gap #6 decision (hyperparameter units)
- **Location**: P §14.3 #6; A §2.2, §5.5.
- **Problem**: Python's API is name-based (`to_raw(name, v)`, `kind_of(name)`), so a table-agnostic primitives function
  cannot be re-exported as it; `rtao-catalog` is 100% generated (rule 3) and cannot host hand code.
- **Evidence**: PY/hyperparams.py:40, :474-770; importers PY/metagraph.py:30, PY/reads/weights.py:7, PY/reads/delegation.py:12
  (`ratio_fraction` only), PY/intents/children.py:10 (`U64_MAX`, `proportion_to_raw`), PY/intents/hyperparameters.py:25,
  PY/cli/commands/sudo.py:10 (`kind_of`, `to_raw`); A:576.
- **Fix**: "Decided: `rtao_primitives::hyperparams` = `HyperparamSpec { name, kind, doc, short, signed, bits, minimum, maximum }`,
  `ratio_fraction`, `proportion_to_raw`, `U64_MAX`, `spec_to_raw`, `spec_normalized`, `spec_annotate`; `rtao_catalog::hyperparams`
  (generated) = `HYPERPARAMS: [HyperparamSpec; N]` + generated `spec(name)`; the name-based API (`kind_of`, `to_raw`,
  `normalized`, `annotate`, `value_forms`, `codec_of`, `raw_bounds`) lives once in `rtao_intents::hyperparameters`."

## F19 [minor] PLAN.md — gap #8 decision (`PY/config.py`)
- **Location**: P §14.3 #8 "`rtao_client::config` [I]".
- **Problem**: names a module only; contents, write path and the three consumers (M1 token symbols, M2 NeuronConfig, M4 CLI) are unstated.
- **Evidence**: PY/config.py:12-27, :58-133, :138-341, :354-447; PY/cli/main.py:26-27; A §3.3 step 6, §9.9, §10.2.
- **Fix**: "Decided: `rtao_client::config` (M1) ports `PY/config.py` whole: paths and TTLs (extracted), `BtcliConfig::{load, save, get, set, unset}`,
  `AddressBook`, `ProxyBook`, `MultisigBook`, `MultisigCache`, `SubnetNameCache`, `TokenSymbolCache`; atomic writes
  (temp + fsync + rename, `.corrupt.<ts>` on parse failure) via `rtao_core::offload`; used by `Client::connect`,
  `NeuronConfig::load` and `rtao-cli` only."

## F20 [minor] PLAN.md — gap #11 decision (clippy paths behind `host`), verified
- **Location**: P §14.3 #11; A §1.8 clippy.toml.
- **Evidence**: W/lintcheck, clippy 0.1.99: unresolvable entry → "warning: … does not refer to a reachable function …
  help: add `allow-invalid = true`"; `cargo clippy -- -D warnings` exits 0; `allow-invalid = true` silences it.
  Host-gated: CORE/lib.rs:10-29, CORE/timelock/mod.rs:241, 275, 297; not gated: `generate_commit_v2` (:160), `epoch_schedule::*`.
- **Fix**: "Decided: add `allow-invalid = true` to the seven host-gated entries (`client::Client::connect`,
  `transaction::Executor::{new, with_policy}`, `timelock::{decrypt, get_round_info, get_reveal_round_signature}`,
  `keyfiles::save_password_to_environment`); keep the other three strict; CI clippy stays `--all-features` (all ten
  resolve and are enforced); clippy runs only on the dev toolchain (the MSRV job runs `cargo check`)."

## F21 [minor] architecture.md — offload rules miss measured >1 ms work
- **Location**: §3.1 "dynamic decodes of payloads above 64 KiB" / "Inline: … typed decodes of ordinary responses"; §3.9.2 metadata hash; P §2 "large decodes".
- **Evidence**: bench-run-3.txt (2,500 UIDs, 408,250 B: `DecodeAsType` 11,000.6 µs, `Value` 2,683.1 µs); W/tlockbench
  (bittensor-core v477 no-host, release): `metadata_digest (V15, RFC-0078): best 2.5 ms, mean 2.7 ms`;
  `encrypt_and_compress n=2500: best 1.36 ms, mean 1.44 ms`; A:745-751, A:1299-1301.
- **Fix**: "Off the reactor: `Runtime::parse`; keyfile unlock; Ledger HID; PoW; any decode of a payload above 64 KiB on
  any path (`Value`, `DecodeAsType` models incl. the `MetagraphWire` fallback, `from_dynamic`, `Metagraph::to_json`);
  `decode_map_page` above 64 items; timelock encryption; fsync'd atomic writes. `metadata_digest` is computed once per
  spec off the reactor and cached on the `RuntimeHandle`. Inline: signing, composition, key hashing, typed decodes
  below 64 KiB, the `MetagraphView` parse." Align P §2.

## F22 [minor] architecture.md — "cancelled when the last clone drops" needs a handle/inner split
- **Location**: §3.1. **Problem**: tasks that hold the user-facing `Arc` keep it alive forever. **Evidence**: A:737-745.
- **Fix**: "`Client { inner: Arc<ClientInner>, _alive: Arc<DropGuard> }`: tasks hold `Arc<ClientInner>` and a child
  `CancellationToken`; dropping the last user-facing clone (incl. those in `Snapshot`, `Executor`, `MetagraphSync`)
  cancels them and closes the socket. Test: `FakeNode` sees the close within 1 s."

## F23 [minor] architecture.md — `max_message_bytes` must also raise tungstenite's frame limit
- **Evidence**: REG/tungstenite-0.28.0/src/protocol/mod.rs:81-102 (`max_frame_size: Some(16 << 20)`); REG/soketto-0.8.1/src/connection.rs:464-534
  (single-frame sends); polkadot-sdk cacb431 substrate/client/cli/src/config.rs:56 (`RPC_DEFAULT_MAX_RESPONSE_SIZE_MB = 15`); PY/_transport/rpc.py:89.
- **Fix**: "`max_message_bytes` sets both `max_message_size` and `max_frame_size`"; FakeNode test with a 20 MiB single frame.

## F24 [minor] architecture.md — `BtAuthLayer` 4 MiB vs axum's 2 MiB extractor limit
- **Evidence**: REG/axum-core-0.5.6/src/ext_traits/request.rs:316-327; A:2483, A:2497-2507.
- **Fix**: "the runtime wraps the router as `router.layer(DefaultBodyLimit::max(cfg.body_limit)).layer(BtAuthLayer { .. })`"; test with a 3 MiB signed `Json<T>` body.

## F25 [minor] architecture.md — `Send + Sync` supertraits required by the RPITIT default bodies
- **Evidence**: W/rpitit/t3.rs (the §4.2 pattern): "future cannot be sent between threads safely … `&Self` … `Self` is not `Sync`"; compiles with `IntentInterface: Send + Sync`. A:1612-1636.
- **Fix**: `IntentInterface: Send + Sync + 'static`, `DynIntent: Send + Sync`, `Clock: Send + Sync + 'static`, `PolicySubject: Send + Sync`; rule in §2.5.

## F26 [minor] architecture.md — `cargo hack --feature-powerset --depth 2 -p rtao` on every PR is ~315 checks
- **Evidence**: W/hack_count.py over the §2.4 table: 27 features, 378 combos, 314 after cargo-hack's implied-feature pruning (+1), 94 with `host`; A:4133-4136.
- **Fix**: PR job = `--each-feature -p rtao` (28) + explicit profiles + `--each-feature -p rtao-intents` + `--feature-powerset -p rtao-btauth -p rtao-signer`; facade depth-2 powerset nightly.

## F27 [minor] architecture.md — `AxonTarget` has no home
- **Evidence**: A:541, A:2512-2518, A:2569, A:2618.
- **Fix**: `rtao_primitives::axon::{AxonTarget { uid, hotkey, base_url }, axon_base_url(ip: u128, ip_type: u8, port: u16)}`; `AxonInfo::base_url()` and `Metagraph::axon_targets()` call it; btauth `client` depends only on primitives.

## F28 [minor] architecture.md — `anyhow` in library traits contradicts §2.5
- **Evidence**: A:647 ("`anyhow` only in binaries, examples and tests") vs A:2781-2782, A:2933.
- **Fix**: §2.5: "Exception: framework user-callback traits (`Validator::{on_start, step}`, `Miner::on_start`) and `NeuronRuntime::run` use `anyhow::Result`, re-exported as `rtao::neuron::anyhow`."

## F29 [minor] python-module-map.md — homes to update after F01, F02, F11, F18, F19, F10, F27
- Rows `_transport/codec.py`, `_transport/contract.py`, `sync.py`, `_subtensor.py`, `metagraph.py`, `namespaces.py`, `config.py`,
  `hyperparams.py`, `extension/bridge.py`, `extension/client.py`, and the matching "Gaps found" rows (MM:66, 71, 76, 81, 83, 92, 117, 119, 289, 291, 390-393).
